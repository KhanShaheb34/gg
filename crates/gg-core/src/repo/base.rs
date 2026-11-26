//! The base of a branch on the base remote: the branch a pull request from it goes into by
//! default (`pr_suggest`), and what "Committed on branch" compares it with when it has no
//! pull request (`branch_files`). One rule for both, in this order:
//!
//! 1. (`branch_files` only, before this module) its open pull request's base.
//! 2. The base the user made its last pull request into ([`WorktreeMemory::pr_base`], per
//!    repository and branch), while that branch is still on the remote.
//!
//!    The default branch itself goes no further than this: on to 5 (where it finds itself,
//!    unless gh names another: "Committed on branch" has nothing to compare it with). The
//!    default branch, known without asking anyone: the remote's HEAD; else gh's default branch
//!    as gh last said it (the suggestion's `gh repo view`, never asked for this); else `main`,
//!    else `master`, when the remote has it (a repository pushed with `git push -u origin
//!    main`, never cloned, has no `refs/remotes/origin/HEAD`).
//! 3. The branch it was created from, as git's own reflogs say, nothing written: the branch's
//!    first reflog entry, `branch: Created from <ref>`, when `<ref>` names a branch (`x` or
//!    `<remote>/x`: branch `x`). When it says `HEAD`, or a commit (how gg itself makes a
//!    branch: `checkout -b <name> <commit>`), the HEAD reflog of a worktree at that moment,
//!    `checkout: moving from <a> to <branch>`, gives `<a>`, when HEAD was at the branch's first
//!    commit then (it was made where HEAD stood).
//! 4. The nearest branch on the remote by history: of its branches other than this one (the
//!    default one and the 30 most recently committed), the one with the fewest commits from it
//!    to the branch (`rev-list --count <it>..<branch>`), ties to the default branch (as known
//!    for 2), then to the newest; within one time limit, else on to 5.
//! 5. gh's default branch (the suggestion only: `branch_files` never asks gh), else the
//!    default branch as known for 2, else `main` ([`default_base`]).
//!
//! A candidate of 2 to 4 counts only when the remote has a branch of that name and it is not
//! the branch itself. Steps 2 to 4 are worked out when asked (never with a snapshot), and kept
//! per branch for its commit, the remote, the remembered base, the default branch and the
//! remote's branches' commits: asked again with those the same, no git runs.
//!
//! [`WorktreeMemory::pr_base`]: crate::repo::WorktreeMemory::pr_base

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::OnceCell;

use crate::dto::RepoSnapshot;
use crate::git::runner::CmdKind;
use crate::repo::manager::{Repo, RepoManager, lock};

/// Most of the remote's branches (besides its default one) compared by history.
const NEAREST_CANDIDATES: usize = 30;
/// How long comparing them by history may take in all.
const NEAREST_LIMIT: Duration = Duration::from_secs(5);
/// Most HEAD reflog entries read per worktree.
const HEAD_ENTRIES: usize = 5000;
/// How far apart (seconds) a branch's creation and HEAD's move to it may be recorded.
const SAME_MOMENT: i64 = 2;

/// How a base was found (steps 2 to 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Found {
    Remembered,
    CreatedFrom,
    Nearest,
}

/// What a kept answer is for: the branch's commit, the remote, the base remembered, the
/// default branch, and the remote's branches with their commits.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BaseKey {
    head: String,
    remote: String,
    remembered: Option<String>,
    default: Option<String>,
    tips: Vec<(String, String)>,
}

/// One branch's last (or running) answer.
#[derive(Debug, Clone)]
pub(crate) struct BaseLoad {
    key: BaseKey,
    answer: Arc<OnceCell<Option<(String, Found)>>>,
}

/// The snapshot's view of remote `remote`: its branches (short names, not `HEAD`), their
/// commits and committer times, and its default branch as known without asking anyone (see
/// the module comment, step 2).
struct OnRemote<'a> {
    branches: Vec<(String, &'a str, i64)>,
    default: Option<String>,
}

impl<'a> OnRemote<'a> {
    /// `gh_said`: gh's default branch as gh last said it, if it did.
    fn of(snap: &'a RepoSnapshot, remote: &str, gh_said: Option<&str>) -> Self {
        let prefix = format!("{remote}/");
        let branches = snap
            .remote_branches
            .iter()
            .zip(&snap.remote_branch_tips)
            .filter_map(|(name, tip)| {
                let short = name.strip_prefix(&prefix)?;
                // A remote named "a" must not take "a/b"'s branches: the longest remote wins.
                let longer = snap.remotes.iter().any(|r| {
                    r.len() > remote.len()
                        && name.starts_with(r.as_str())
                        && name.as_bytes().get(r.len()) == Some(&b'/')
                });
                (!longer && short != "HEAD")
                    .then(|| (short.to_owned(), tip.hash.as_str(), tip.time))
            })
            .collect();
        let mut on = Self {
            branches,
            default: None,
        };
        on.default = snap
            .remote_heads
            .get(remote)
            .cloned()
            .or_else(|| gh_said.map(str::to_owned))
            .or_else(|| {
                ["main", "master"]
                    .into_iter()
                    .find(|b| on.has(b))
                    .map(str::to_owned)
            });
        on
    }

    fn has(&self, branch: &str) -> bool {
        self.branches.iter().any(|(b, _, _)| b == branch)
    }
}

/// Branch `r` (as a reflog names it: `x`, `refs/heads/x`, `origin/x`, `refs/remotes/origin/x`)
/// as a branch name on a remote: `x`; `None` for `HEAD` or a commit.
fn branch_named(r: &str, remotes: &[String]) -> Option<String> {
    let r = r.trim();
    if r.is_empty() || r == "HEAD" || is_object_name(r) {
        return None;
    }
    if let Some(local) = r.strip_prefix("refs/heads/") {
        return Some(local.to_owned());
    }
    let remote_ref = r
        .strip_prefix("refs/remotes/")
        .or_else(|| r.strip_prefix("remotes/"))
        .unwrap_or(r);
    let on = remotes
        .iter()
        .filter(|rem| {
            remote_ref.len() > rem.len()
                && remote_ref.starts_with(rem.as_str())
                && remote_ref.as_bytes()[rem.len()] == b'/'
        })
        .max_by_key(|rem| rem.len());
    match on {
        Some(rem) => Some(remote_ref[rem.len() + 1..].to_owned()),
        None => Some(remote_ref.to_owned()),
    }
}

fn is_object_name(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// One reflog entry: when (unix seconds), the commit it moved to, its message.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    at: i64,
    commit: String,
    message: String,
}

/// Parses `reflog show --date=unix --format=%gd%x00%H%x00%gs` output, newest first.
fn parse_reflog(out: &[u8]) -> Vec<Entry> {
    String::from_utf8_lossy(out)
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\0');
            let (selector, commit, message) = (parts.next()?, parts.next()?, parts.next()?);
            let at = selector
                .rsplit_once("@{")?
                .1
                .strip_suffix('}')?
                .parse()
                .ok()?;
            Some(Entry {
                at,
                commit: commit.to_owned(),
                message: message.to_owned(),
            })
        })
        .collect()
}

/// In a worktree's HEAD reflog (newest first), the branch HEAD moved from to `branch` at
/// about `at`, when HEAD was at `commit` before the move (the branch was made where it stood).
fn moved_from(head: &[Entry], branch: &str, at: i64, commit: &str) -> Option<String> {
    let to = format!(" to {branch}");
    // The oldest such move: the one that came with the branch's creation.
    head.iter().enumerate().rev().find_map(|(i, e)| {
        let from = e
            .message
            .strip_prefix("checkout: moving from ")?
            .strip_suffix(&to)?;
        let before = head.get(i + 1)?;
        ((e.at - at).abs() <= SAME_MOMENT && before.commit == commit && e.commit == commit)
            .then(|| from.to_owned())
    })
}

/// The base when steps 2 to 4 found none: `gh_default` (gh's default branch, when asked),
/// else the default branch of `remote` as known without asking (its HEAD; gh's default
/// branch as gh last said it, `gh_said`; `main` or `master` on it), else `main`.
pub(crate) fn default_base(
    snap: &RepoSnapshot,
    remote: Option<&str>,
    gh_default: Option<String>,
    gh_said: Option<&str>,
) -> String {
    gh_default
        .or_else(|| remote.and_then(|r| OnRemote::of(snap, r, gh_said).default))
        .unwrap_or_else(|| "main".to_owned())
}

impl RepoManager {
    /// gh's default branch for `remote` as gh last said it (the suggestion's `gh repo view`);
    /// never asks.
    pub(crate) fn gh_said(&self, repo: &Repo, remote: &str) -> Option<String> {
        lock(&repo.gh_defaults).get(remote).cloned()
    }

    /// Keeps what gh said `remote`'s default branch is (`None`: it did not say).
    pub(crate) fn gh_says(&self, repo: &Repo, remote: &str, default: Option<String>) {
        let mut said = lock(&repo.gh_defaults);
        match default {
            Some(d) => said.insert(remote.to_owned(), d),
            None => said.remove(remote),
        };
    }

    /// Whether local branch `branch` is `remote`'s default branch, as known without asking
    /// anyone (see the module comment, step 2).
    pub(crate) fn is_default_branch(
        &self,
        repo: &Repo,
        snap: &RepoSnapshot,
        branch: &str,
        remote: &str,
    ) -> bool {
        let said = self.gh_said(repo, remote);
        OnRemote::of(snap, remote, said.as_deref())
            .default
            .as_deref()
            == Some(branch)
    }

    /// The base of local branch `branch` on `remote` by steps 2 to 4 (see the module
    /// comment), with how it was found; `None`: on to step 5. `snap` is the repository's
    /// snapshot now.
    pub(crate) async fn recorded_base(
        &self,
        repo: &Arc<Repo>,
        snap: &RepoSnapshot,
        branch: &str,
        remote: &str,
    ) -> Option<(String, Found)> {
        let i = snap.branches.iter().position(|b| b == branch)?;
        let head = snap.branch_tips.get(i)?.hash.clone();
        let said = self.gh_said(repo, remote);
        let on = OnRemote::of(snap, remote, said.as_deref());
        let remembered = self
            .memory()
            .and_then(|m| m.pr_base(&repo.root, branch))
            .filter(|b| crate::git::names::check_branch_name(b).is_ok());
        let key = BaseKey {
            head,
            remote: remote.to_owned(),
            remembered: remembered.clone(),
            default: on.default.clone(),
            tips: on
                .branches
                .iter()
                .map(|(b, tip, _)| (b.clone(), (*tip).to_owned()))
                .collect(),
        };
        let answer = {
            let mut loads = lock(&repo.branch_bases);
            match loads.get(branch) {
                Some(l) if l.key == key => l.answer.clone(),
                _ => {
                    let answer = Arc::new(OnceCell::new());
                    loads.insert(
                        branch.to_owned(),
                        BaseLoad {
                            key: key.clone(),
                            answer: answer.clone(),
                        },
                    );
                    answer
                }
            }
        };
        answer
            .get_or_init(|| {
                Box::pin(self.find_base(repo, snap, branch, &key.head, &on, remembered))
            })
            .await
            .clone()
    }

    async fn find_base(
        &self,
        repo: &Arc<Repo>,
        snap: &RepoSnapshot,
        branch: &str,
        head: &str,
        on: &OnRemote<'_>,
        remembered: Option<String>,
    ) -> Option<(String, Found)> {
        let usable = |b: &str| b != branch && on.has(b);
        if let Some(b) = remembered.filter(|b| usable(b)) {
            return Some((b, Found::Remembered));
        }
        // The default branch: not by where it came from, nor by history (on to step 5).
        if on.default.as_deref() == Some(branch) {
            return None;
        }
        if let Some(b) = self
            .created_from(repo, snap, branch)
            .await
            .filter(|b| usable(b))
        {
            return Some((b, Found::CreatedFrom));
        }
        self.nearest(repo, branch, head, on)
            .await
            .map(|b| (b, Found::Nearest))
    }

    /// Step 3: the branch `branch` was created from, by the reflogs (see the module comment).
    async fn created_from(
        &self,
        repo: &Arc<Repo>,
        snap: &RepoSnapshot,
        branch: &str,
    ) -> Option<String> {
        let full = format!("refs/heads/{branch}");
        let args = [
            "reflog",
            "show",
            "--date=unix",
            "--format=%gd%x00%H%x00%gs",
            &full,
            "--",
        ];
        let out = repo
            .runner
            .run_unchecked(&repo.root, &args, CmdKind::Read)
            .await
            .ok()?;
        if out.code != 0 {
            return None;
        }
        let first = parse_reflog(&out.stdout).pop()?;
        let from = first.message.strip_prefix("branch: Created from ")?.trim();
        if let Some(named) = branch_named(from, &snap.remotes) {
            return Some(named);
        }
        // HEAD, or a commit: where HEAD was when the branch was made, in the worktree it was
        // made in (HEAD's reflog is each worktree's own). Where it is checked out first.
        let mut worktrees: Vec<&crate::dto::WorktreeInfo> = snap
            .worktrees
            .iter()
            .filter(|w| !w.prunable && !w.bare && Path::new(&w.path).is_dir())
            .collect();
        worktrees.sort_by_key(|w| w.branch.as_deref() != Some(branch));
        let worktrees: Vec<PathBuf> = worktrees.iter().map(|w| PathBuf::from(&w.path)).collect();
        let count = format!("-n{HEAD_ENTRIES}");
        for wt in worktrees {
            let args = [
                "reflog",
                "show",
                &count,
                "--date=unix",
                "--format=%gd%x00%H%x00%gs",
                "HEAD",
                "--",
            ];
            let Ok(out) = repo.runner.run_unchecked(&wt, &args, CmdKind::Read).await else {
                continue;
            };
            if out.code != 0 {
                continue;
            }
            if let Some(a) = moved_from(&parse_reflog(&out.stdout), branch, first.at, &first.commit)
            {
                return branch_named(&a, &snap.remotes);
            }
        }
        None
    }

    /// Step 4: the nearest branch on the remote by history (see the module comment).
    async fn nearest(
        &self,
        repo: &Arc<Repo>,
        branch: &str,
        head: &str,
        on: &OnRemote<'_>,
    ) -> Option<String> {
        if !is_object_name(head) {
            return None;
        }
        let mut others: Vec<&(String, &str, i64)> = on
            .branches
            .iter()
            .filter(|(b, tip, _)| {
                b != branch && Some(b.as_str()) != on.default.as_deref() && is_object_name(tip)
            })
            .collect();
        others.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        let mut candidates: Vec<&(String, &str, i64)> = on
            .branches
            .iter()
            .filter(|(b, tip, _)| {
                b != branch && Some(b.as_str()) == on.default.as_deref() && is_object_name(tip)
            })
            .collect();
        candidates.extend(others.into_iter().take(NEAREST_CANDIDATES));
        if candidates.is_empty() {
            return None;
        }
        let limit = self.nearest_limit();
        let counts = tokio::time::timeout(limit, async {
            let mut counts = Vec::with_capacity(candidates.len());
            for (name, tip, time) in &candidates {
                let range = format!("{tip}..{head}");
                let args = ["rev-list", "--count", &range, "--"];
                let out = repo
                    .runner
                    .run_unchecked(&repo.root, &args, CmdKind::Read)
                    .await;
                let Ok(out) = out else { continue };
                if out.code != 0 {
                    continue;
                }
                if let Ok(n) = out.text().trim().parse::<u64>() {
                    counts.push((
                        n,
                        Some(name.as_str()) != on.default.as_deref(),
                        -time,
                        name.clone(),
                    ));
                }
            }
            counts
        })
        .await
        .ok()?;
        // Fewest commits; then the default branch; then the newest; then by name.
        counts.into_iter().min().map(|(_, _, _, name)| name)
    }

    /// How long step 4 may take (see [`RepoManager::set_nearest_limit`]).
    fn nearest_limit(&self) -> Duration {
        #[cfg(any(test, feature = "testutil"))]
        if let Some(limit) = *lock(&self.nearest_limit) {
            return limit;
        }
        NEAREST_LIMIT
    }

    /// Sets how long comparing a branch with the remote's branches by history may take
    /// (tests only; normally 5 s).
    #[cfg(any(test, feature = "testutil"))]
    pub fn set_nearest_limit(&self, limit: Duration) {
        *lock(&self.nearest_limit) = Some(limit);
    }

    /// The base of local branch `branch` on `remote` (see the module comment, steps 2 to 5,
    /// without gh), for tests: the branch name, and how it was found (`None`: step 5).
    #[cfg(any(test, feature = "testutil"))]
    pub async fn branch_base_for_tests(
        &self,
        repo_id: u32,
        branch: &str,
        remote: &str,
    ) -> crate::error::Result<(String, Option<&'static str>)> {
        let repo = self.get(repo_id)?;
        let snap = self.snapshot(repo_id).await?;
        Ok(
            match self.recorded_base(&repo, &snap, branch, remote).await {
                Some((b, how)) => (
                    b,
                    Some(match how {
                        Found::Remembered => "remembered",
                        Found::CreatedFrom => "created_from",
                        Found::Nearest => "nearest",
                    }),
                ),
                None => {
                    let said = self.gh_said(&repo, remote);
                    (
                        default_base(&snap, Some(remote), None, said.as_deref()),
                        None,
                    )
                }
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn a_reflogs_ref_as_a_branch_on_the_remote() {
        let remotes = vec!["origin".to_owned(), "team/eu".to_owned()];
        assert_eq!(
            branch_named("staging", &remotes).as_deref(),
            Some("staging")
        );
        assert_eq!(
            branch_named("refs/heads/release/1", &remotes).as_deref(),
            Some("release/1")
        );
        assert_eq!(
            branch_named("origin/staging", &remotes).as_deref(),
            Some("staging")
        );
        assert_eq!(
            branch_named("refs/remotes/origin/pre-release", &remotes).as_deref(),
            Some("pre-release")
        );
        assert_eq!(
            branch_named("remotes/team/eu/main", &remotes).as_deref(),
            Some("main")
        );
        assert_eq!(branch_named("HEAD", &remotes), None);
        assert_eq!(branch_named(A, &remotes), None);
        assert_eq!(branch_named("", &remotes), None);
    }

    #[test]
    fn reflog_lines_and_the_move_that_made_a_branch() {
        let out = format!(
            "HEAD@{{105}}\0{B}\0commit: work\nHEAD@{{100}}\0{A}\0checkout: moving from staging to topic\nHEAD@{{90}}\0{A}\0checkout: moving from main to staging\nnot a line\n"
        );
        let head = parse_reflog(out.as_bytes());
        assert_eq!(head.len(), 3);
        assert_eq!(
            head[1],
            Entry {
                at: 100,
                commit: A.into(),
                message: "checkout: moving from staging to topic".into()
            }
        );
        assert_eq!(
            moved_from(&head, "topic", 100, A).as_deref(),
            Some("staging")
        );
        assert_eq!(
            moved_from(&head, "topic", 101, A).as_deref(),
            Some("staging")
        );
        // Too far apart in time, another commit, another branch: none.
        assert_eq!(moved_from(&head, "topic", 200, A), None);
        assert_eq!(moved_from(&head, "topic", 100, B), None);
        assert_eq!(moved_from(&head, "other", 100, A), None);
    }

    #[test]
    fn the_last_resort_is_gh_then_the_remotes_head_then_main_or_master() {
        let mut snap: RepoSnapshot = serde_json::from_value(serde_json::json!({
            "repoId": 1, "name": "r", "rootPath": "/r", "activeWorktree": "/r", "generation": 1,
            "totalRows": 0, "loading": false, "maxLane": 0, "head": {"branch": null, "hash": null},
            "branches": [], "remoteBranches": [], "branchTips": [], "remoteBranchTips": [],
            "remotes": ["origin"], "worktrees": [],
            "status": {"changed": 0, "untracked": 0, "conflicted": 0, "conflictedPaths": []},
            "uncommittedSubject": "", "viewOptions": {"branches": null, "showRemotes": true, "showTags": true, "showStashes": true},
            "fetch": {"running": false, "lastError": null, "lastErrorDetail": null, "lastAtMs": null, "fetchedAtMs": null},
            "op": null, "prs": {"status": "unknown", "reason": null, "loadedAtMs": null, "partial": false},
            "branchPrs": {}, "remoteHeads": {}, "inProgress": null, "seq": 0
        }))
        .unwrap();
        assert_eq!(default_base(&snap, Some("origin"), None, None), "main");
        // No HEAD: master when the remote has it and not main; main before master.
        for (name, at) in [("origin/develop", 3), ("origin/master", 2)] {
            snap.remote_branches.push(name.into());
            snap.remote_branch_tips.push(crate::dto::BranchTip {
                hash: A.into(),
                time: at,
                author: String::new(),
                subject: String::new(),
            });
        }
        assert_eq!(default_base(&snap, Some("origin"), None, None), "master");
        snap.remote_branches.push("origin/main".into());
        snap.remote_branch_tips
            .push(snap.remote_branch_tips[0].clone());
        assert_eq!(default_base(&snap, Some("origin"), None, None), "main");
        // What gh last said comes before main and master, after the remote's HEAD.
        assert_eq!(
            default_base(&snap, Some("origin"), None, Some("develop")),
            "develop"
        );
        snap.remote_heads.insert("origin".into(), "trunk".into());
        assert_eq!(
            default_base(&snap, Some("origin"), None, Some("develop")),
            "trunk"
        );
        assert_eq!(
            default_base(&snap, Some("origin"), Some("develop".into()), None),
            "develop"
        );
        assert_eq!(default_base(&snap, None, None, None), "main");
    }
}
