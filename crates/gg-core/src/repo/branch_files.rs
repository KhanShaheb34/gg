//! `branch_files`: the files a worktree's branch changes in its own commits, for the Commit
//! tab's "Committed on branch". Read-only: two git reads, never a write.
//!
//! The branch is the one checked out in the worktree; its base is its open pull request's
//! base (as the snapshot knows it, `branchPrs`) on the repository the pull request goes to
//! (the remote the pull requests were listed from: `upstream` before `origin` in a fork), else
//! on the remote the branch tracks, else on any remote, else that local branch; without one,
//! the branch's base on the base remote as the pull request form has it (`repo/base.rs`: the
//! base remembered for it, the branch it was created from, the nearest by history, the
//! remote's HEAD, `main`).
//! The files are those of `merge-base(base, HEAD)..HEAD` (`git diff-tree -M --raw
//! --numstat`, read as a commit's are): what the branch's commits change, not the working
//! tree. None at all (`null`) when HEAD is detached there, the branch is the base's own
//! (the default branch itself), no base is known, the two share no history, or the branch
//! changes nothing.
//!
//! Everything but the two git reads comes from the snapshot in memory. One answer is kept
//! per worktree, for its HEAD, base and the base's commit: asked again with those the same,
//! the same answer is given (no git runs); asked while a read runs for them, the request
//! waits for that read's answer rather than starting another.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::OnceCell;

use crate::dto::{BranchFiles, BranchTip, MAX_BRANCH_FILES, PullRequest, RepoSnapshot};
use crate::error::{CoreError, ErrorCode, Result};
use crate::gh::RemoteFacts;
use crate::git::details::parse_files;
use crate::git::runner::{CmdKind, git_failed};
use crate::repo::base::default_base;
use crate::repo::manager::{Repo, RepoManager, canonical, lock};

/// What one read is for: the branch, its commit, the base and the base's commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Range {
    branch: String,
    head: String,
    base: String,
    base_commit: String,
}

/// One worktree's last (or running) read: what it is for, and its answer once given.
#[derive(Debug, Clone)]
pub(crate) struct BranchFilesLoad {
    range: Range,
    answer: Arc<OnceCell<Result<Option<BranchFiles>>>>,
}

/// What the base is chosen from: the snapshot's names, tips and pull requests, and the remote
/// the pull requests were listed from.
pub(crate) struct Known<'a> {
    /// The remote naming the repository the pull requests go to (`None`: not listed).
    pub pr_remote: Option<&'a str>,
    pub remotes: &'a [String],
    pub branch_prs: &'a BTreeMap<String, PullRequest>,
    pub branches: &'a [String],
    pub branch_tips: &'a [BranchTip],
    pub remote_branches: &'a [String],
    pub remote_branch_tips: &'a [BranchTip],
}

impl<'a> Known<'a> {
    pub(crate) fn of(snap: &'a RepoSnapshot, pr_remote: Option<&'a str>) -> Self {
        Self {
            pr_remote,
            remotes: &snap.remotes,
            branch_prs: &snap.branch_prs,
            branches: &snap.branches,
            branch_tips: &snap.branch_tips,
            remote_branches: &snap.remote_branches,
            remote_branch_tips: &snap.remote_branch_tips,
        }
    }

    /// The commit remote-tracking branch `name` ("origin/main") points at.
    fn remote_tip(&self, name: &str) -> Option<&'a str> {
        let i = self.remote_branches.iter().position(|b| b == name)?;
        self.remote_branch_tips.get(i).map(|t| t.hash.as_str())
    }

    /// The commit local branch `name` points at.
    fn local_tip(&self, name: &str) -> Option<&'a str> {
        let i = self.branches.iter().position(|b| b == name)?;
        self.branch_tips.get(i).map(|t| t.hash.as_str())
    }

    /// The remote `upstream` ("origin/feature") is on: the longest remote name it starts
    /// with, then a slash (remote names may hold slashes).
    fn remote_of(&self, upstream: &str) -> Option<&'a str> {
        self.remotes
            .iter()
            .filter(|r| {
                upstream.len() > r.len()
                    && upstream.starts_with(r.as_str())
                    && upstream.as_bytes()[r.len()] == b'/'
            })
            .max_by_key(|r| r.len())
            .map(String::as_str)
    }
}

/// Step 1: the base of branch `branch` (tracking `upstream`, if it does) from its open pull
/// request, with its commit: the pull request's base on the repository it was listed from,
/// else on the remote the branch tracks, else on the first remote that has that branch, else
/// the local branch of that name. `None` without a pull request, or with none of those.
pub(crate) fn pr_base(
    known: &Known<'_>,
    branch: &str,
    upstream: Option<&str>,
) -> Option<(String, String)> {
    let tracked = upstream.and_then(|u| known.remote_of(u));
    let pr = known.branch_prs.get(branch)?;
    if pr.base == branch {
        return None;
    }
    let on = |remote: &str| {
        let name = format!("{remote}/{}", pr.base);
        known.remote_tip(&name).map(|tip| (name, tip.to_owned()))
    };
    known
        .pr_remote
        .and_then(on)
        .or_else(|| tracked.and_then(on))
        .or_else(|| known.remotes.iter().find_map(|r| on(r)))
        .or_else(|| {
            known
                .local_tip(&pr.base)
                .map(|tip| (pr.base.clone(), tip.to_owned()))
        })
}

/// A full object name, as git prints one.
fn is_object_name(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl RepoManager {
    /// The files the branch checked out in worktree `path` of repository `repo_id` changes
    /// in its own commits (see the module comment); `None` when there is no such list.
    /// `bad_request` for a path that is not one of its worktrees.
    pub async fn branch_files(&self, repo_id: u32, path: &Path) -> Result<Option<BranchFiles>> {
        let repo = self.get(repo_id)?;
        let snap = self.snapshot(repo_id).await?;
        let target = canonical(path);
        let Some(wt) = snap.worktrees.iter().find(|w| Path::new(&w.path) == target) else {
            return Err(CoreError::new(
                ErrorCode::BadRequest,
                format!("{} is not a worktree of this repository.", path.display()),
            ));
        };
        let (Some(branch), Some(head)) = (&wt.branch, &wt.head) else {
            return Ok(None);
        };
        if wt.bare || wt.prunable {
            return Ok(None);
        }
        let upstream = wt.upstream.as_ref().map(|u| u.name.as_str());
        // The repository the pull requests were listed from (and go to).
        let pr_remote = {
            let slot = repo.slot.read().await;
            slot.prs
                .data
                .as_ref()
                .and_then(|d| d.facts.base_remote().map(str::to_owned))
        };
        let known = Known::of(&snap, pr_remote.as_deref());
        let chosen = match pr_base(&known, branch, upstream) {
            Some(chosen) => Some(chosen),
            // Steps 2 to 5 (repo/base.rs) on the base remote, as the suggestion has them.
            None => {
                let remote = match pr_remote.clone() {
                    Some(r) => Some(r),
                    None => RemoteFacts::read(&repo.runner, &repo.root)
                        .await
                        .ok()
                        .and_then(|f| f.base_remote().map(str::to_owned)),
                };
                match remote {
                    // The default branch itself: nothing to compare.
                    Some(r) if snap.remote_heads.get(&r) != Some(branch) => {
                        let name = match self.recorded_base(&repo, &snap, branch, &r).await {
                            Some((b, _)) => b,
                            None => default_base(&snap, Some(&r), None),
                        };
                        let full = format!("{r}/{name}");
                        (name != *branch)
                            .then(|| known.remote_tip(&full).map(|tip| (full, tip.to_owned())))
                            .flatten()
                    }
                    _ => None,
                }
            }
        };
        let Some((base, base_commit)) = chosen else {
            return Ok(None);
        };
        let range = Range {
            branch: branch.clone(),
            head: head.clone(),
            base,
            base_commit,
        };
        let key = wt.path.clone();
        let answer = {
            let mut loads = lock(&repo.branch_files);
            match loads.get(&key) {
                // Running, or read for the same range: its answer is this one's too.
                Some(l) if l.range == range && !matches!(l.answer.get(), Some(Err(_))) => {
                    l.answer.clone()
                }
                _ => {
                    let answer = Arc::new(OnceCell::new());
                    loads.insert(
                        key,
                        BranchFilesLoad {
                            range: range.clone(),
                            answer: answer.clone(),
                        },
                    );
                    answer
                }
            }
        };
        let cwd = if target.is_dir() {
            target
        } else {
            repo.root.clone()
        };
        answer
            .get_or_init(|| Box::pin(read_range(&repo, cwd, range)))
            .await
            .clone()
    }
}

/// Reads the files of `range` in `cwd` (see the module comment).
async fn read_range(repo: &Arc<Repo>, cwd: PathBuf, range: Range) -> Result<Option<BranchFiles>> {
    if !is_object_name(&range.head) || !is_object_name(&range.base_commit) {
        return Ok(None);
    }
    let args = [
        "merge-base",
        range.base_commit.as_str(),
        range.head.as_str(),
    ];
    let out = repo
        .runner
        .run_unchecked(&cwd, &args, CmdKind::Read)
        .await?;
    // 1: no common commit (unrelated histories): nothing to compare from.
    if out.code == 1 {
        return Ok(None);
    }
    if out.code != 0 {
        return Err(git_failed(&args, out.code, &out.stderr));
    }
    let merge_base = out.text().trim().to_owned();
    if !is_object_name(&merge_base) {
        return Err(CoreError::new(
            ErrorCode::GitFailed,
            "git merge-base gave no commit.",
        ));
    }
    let out = repo
        .runner
        .run(
            &cwd,
            &[
                "diff-tree",
                "-r",
                "-z",
                "-M",
                "--raw",
                "--numstat",
                "--no-ext-diff",
                "--no-textconv",
                &merge_base,
                &range.head,
            ],
            CmdKind::Read,
        )
        .await?;
    let mut files = parse_files(&out.stdout);
    if files.is_empty() {
        return Ok(None);
    }
    let total = u32::try_from(files.len()).unwrap_or(u32::MAX);
    files.truncate(MAX_BRANCH_FILES);
    Ok(Some(BranchFiles {
        branch: range.branch,
        base: range.base,
        base_commit: range.base_commit,
        merge_base,
        head: range.head,
        files,
        total,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::PullRequestState;

    fn tip(hash: &str) -> BranchTip {
        BranchTip {
            hash: hash.into(),
            time: 0,
            author: String::new(),
            subject: String::new(),
        }
    }

    fn pr(base: &str) -> PullRequest {
        PullRequest {
            number: 7,
            title: "t".into(),
            url: "https://github.com/o/r/pull/7".into(),
            state: PullRequestState::Open,
            head: "feature".into(),
            base: base.into(),
        }
    }

    struct Facts {
        pr_remote: Option<String>,
        remotes: Vec<String>,
        prs: BTreeMap<String, PullRequest>,
        branches: Vec<String>,
        remote_branches: Vec<String>,
    }

    impl Facts {
        fn new(remotes: &[&str], _heads: &[(&str, &str)], remote_branches: &[&str]) -> Self {
            Self {
                pr_remote: None,
                remotes: remotes.iter().map(|r| (*r).into()).collect(),
                prs: BTreeMap::new(),
                branches: vec!["main".into(), "feature".into(), "develop".into()],
                remote_branches: remote_branches.iter().map(|b| (*b).into()).collect(),
            }
        }

        /// Each branch's tip is its name ("tip:origin/main").
        fn choose(&self, branch: &str, upstream: Option<&str>) -> Option<(String, String)> {
            let tips = |names: &[String]| {
                names
                    .iter()
                    .map(|n| tip(&format!("tip:{n}")))
                    .collect::<Vec<_>>()
            };
            let (bt, rt) = (tips(&self.branches), tips(&self.remote_branches));
            let known = Known {
                pr_remote: self.pr_remote.as_deref(),
                remotes: &self.remotes,
                branch_prs: &self.prs,
                branches: &self.branches,
                branch_tips: &bt,
                remote_branches: &self.remote_branches,
                remote_branch_tips: &rt,
            };
            pr_base(&known, branch, upstream)
        }
    }

    fn base(name: &str) -> Option<(String, String)> {
        Some((name.into(), format!("tip:{name}")))
    }

    #[test]
    fn a_pull_requests_base_on_the_tracked_remote_the_first_that_has_it_or_the_local_branch() {
        let mut f = Facts::new(
            &["origin", "upstream"],
            &[("origin", "main"), ("upstream", "trunk")],
            &[
                "origin/main",
                "origin/develop",
                "upstream/trunk",
                "upstream/develop",
            ],
        );
        // No pull request: not this step's (repo/base.rs has the rest).
        assert_eq!(f.choose("feature", Some("upstream/feature")), None);
        // A pull request into develop: develop on the tracked remote, else the first that has it.
        f.prs.insert("feature".into(), pr("develop"));
        assert_eq!(
            f.choose("feature", Some("upstream/feature")),
            base("upstream/develop")
        );
        assert_eq!(f.choose("feature", None), base("origin/develop"));
        // A base no remote has: the local branch of that name; none at all, none.
        f.prs.insert("feature".into(), pr("main"));
        assert_eq!(
            f.choose("feature", Some("upstream/feature")),
            base("origin/main")
        );
        f.prs.insert("feature".into(), pr("gone"));
        assert_eq!(f.choose("feature", None), None);
        f.branches.push("gone".into());
        assert_eq!(f.choose("feature", None), base("gone"));
    }

    /// A fork: the branch tracks the fork (`origin`), its pull request goes to `upstream`
    /// (the remote the pull requests were listed from): the base is upstream's branch, not
    /// the fork's copy of it; without upstream's, the fallbacks as before.
    #[test]
    fn a_pull_requests_base_is_on_the_repository_it_goes_to() {
        let mut f = Facts::new(
            &["origin", "upstream"],
            &[("origin", "main"), ("upstream", "main")],
            &["origin/main", "upstream/main", "origin/develop"],
        );
        f.prs.insert("feature".into(), pr("main"));
        f.pr_remote = Some("upstream".into());
        assert_eq!(
            f.choose("feature", Some("origin/feature")),
            base("upstream/main")
        );
        // Its base not fetched from upstream: the tracked remote's, as before.
        f.prs.insert("feature".into(), pr("develop"));
        assert_eq!(
            f.choose("feature", Some("origin/feature")),
            base("origin/develop")
        );
        // Listed from origin: origin's.
        f.prs.insert("feature".into(), pr("main"));
        f.pr_remote = Some("origin".into());
        assert_eq!(
            f.choose("feature", Some("upstream/feature")),
            base("origin/main")
        );
    }

    #[test]
    fn the_tracked_remote_is_the_longest_name_that_fits() {
        let mut f = Facts::new(
            &["team", "team/eu"],
            &[("team", "main"), ("team/eu", "trunk")],
            &["team/trunk", "team/eu/trunk"],
        );
        f.prs.insert("feature".into(), pr("trunk"));
        assert_eq!(
            f.choose("feature", Some("team/eu/feature")),
            base("team/eu/trunk")
        );
        assert_eq!(
            f.choose("feature", Some("team/feature")),
            base("team/trunk")
        );
        // An upstream on no known remote (a local branch): the first remote that has it.
        assert_eq!(f.choose("feature", Some("develop")), base("team/trunk"));
    }

    #[test]
    fn object_names() {
        assert!(is_object_name(&"a".repeat(40)));
        assert!(is_object_name(&"0".repeat(64)));
        assert!(!is_object_name("HEAD"));
        assert!(!is_object_name(&"g".repeat(40)));
        assert!(!is_object_name(&format!("-{}", "a".repeat(39))));
    }
}
