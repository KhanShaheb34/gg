//! Committing the files the user ticked (`commit`), optionally pushing after, and the list
//! of changed files the commit panel shows (`worktree_files`).
//!
//! A commit of chosen files never starts from the user's index: its tree is built in a
//! fresh temporary index beside it (`GIT_INDEX_FILE`), filled from HEAD (`read-tree`;
//! nothing on an unborn branch) and then with exactly the ticked paths as they are in the
//! working tree (`add -A`, literal pathspecs, read from a file). So a file staged but not
//! ticked is not committed, and a failure leaves the user's index as it was. Hooks run as
//! with `git commit <paths>`, and see the temporary index. Once the commit is made, the
//! committed paths, and only they, are set to the new HEAD in the user's index, so they no
//! longer show as changed while other staged changes stay staged. The temporary index and
//! files are always removed, also when the commit fails or is stopped.
//!
//! git allows no partial commit while a merge is in progress: a merge with no conflicts
//! left is concluded with all its files, in the user's index.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::dto::{EntryStatus, FileEntry, InProgressKind, OpHint, WorktreeFiles};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::progress::read_kind;
use crate::git::runner::{CmdKind, GitOutput, GitRunner};
use crate::git::status::{STATUS_ARGS, parse_files, read_files};
use crate::git::worktrees::read_worktrees;
use crate::ops::explain::{file_list, recognised};
use crate::ops::local::{current_branch, head_commit, short};
use crate::ops::{OpCtx, Step, network};
use crate::repo::{RepoManager, canonical, canonical_paths};

/// Said when a merge is to be committed with only some of its files.
const MERGE_NEEDS_ALL: &str = "A merge is concluded with all its files.";

/// Said when git does not know the user's name or email.
const NO_IDENTITY: &str = "Nothing was committed: git does not know who you are. Set your \
                           name and email (git config --global user.name \"Your Name\", and \
                           user.email), then commit again.";

/// Said when the signing program failed (R41): without a terminal, a passphrase prompt
/// fails at once.
const SIGNING_FAILED: &str = "Nothing was committed: the commit could not be signed. gg runs \
                              git without a terminal, so a signing program that asks for a \
                              passphrase there fails at once. Use an agent or a graphical \
                              passphrase prompt (see \u{201c}Actions\u{201d} in the README), \
                              or commit in a terminal.";

/// Said when the message is empty once git has removed its comment lines.
const EMPTY_MESSAGE: &str = "Nothing was committed: the message is empty once git removes \
                             comment lines (lines starting with #).";

/// Said when git, or a hook, refused the commit for a reason the core does not recognise.
const REFUSED: &str = "Nothing was committed: git or one of the repository's hooks \
                       (pre-commit, commit-msg) refused the commit. What it said is below.";

/// Most paths given to one `ls-files`, so the command line stays short on any platform.
const LS_CHUNK: usize = 100;

/// "1 file", "3 files".
pub(crate) fn files(n: usize) -> String {
    if n == 1 {
        "1 file".to_owned()
    } else {
        format!("{n} files")
    }
}

/// Checks a commit request without git: some path, a message that is not blank, and paths
/// that name something inside the worktree (relative, no `.` or `..`, no NUL).
pub(crate) fn check(paths: &[String], message: &str) -> Result<()> {
    if paths.is_empty() {
        return Err(CoreError::new(
            ErrorCode::NothingToDo,
            "Nothing to commit: no file is ticked.",
        ));
    }
    if message.trim().is_empty() {
        return Err(CoreError::new(
            ErrorCode::BadRequest,
            "A commit needs a message.",
        ));
    }
    for path in paths {
        let inside = !path.is_empty()
            && !path.contains('\0')
            && Path::new(path)
                .components()
                .all(|c| matches!(c, Component::Normal(_)));
        if !inside {
            return Err(CoreError::new(
                ErrorCode::BadRequest,
                format!("\u{201c}{path}\u{201d} is not a path in the worktree."),
            ));
        }
    }
    Ok(())
}

/// The error for a commit that would record no change.
fn nothing_to_commit() -> CoreError {
    CoreError::new(
        ErrorCode::NothingToDo,
        "Nothing to commit: the ticked files have no changes now.",
    )
}

/// Every changed file of the worktree the operation runs in, as git sees it now.
async fn changed_files(ctx: &OpCtx<'_>) -> Result<Vec<FileEntry>> {
    let out = ctx.read(&STATUS_ARGS).await?;
    if out.code != 0 {
        return Err(
            CoreError::new(ErrorCode::GitFailed, "Could not read the changed files.")
                .with_detail(out.stderr),
        );
    }
    Ok(parse_files(&out.stdout))
}

/// Whether `entry` is ticked in `ticked`: its path, or for a rename or copy either side.
fn is_ticked(entry: &FileEntry, ticked: &BTreeSet<&str>) -> bool {
    ticked.contains(entry.path.as_str())
        || entry
            .orig_path
            .as_deref()
            .is_some_and(|orig| ticked.contains(orig))
}

/// The paths a commit of `ticked` records, given the changed files `entries`: each ticked
/// entry's path, and a rename's original path with it (both sides of a rename are one
/// change). Paths that are no changed file now record nothing and are left out.
fn chosen(entries: &[FileEntry], ticked: &[String]) -> BTreeSet<String> {
    let ticked: BTreeSet<&str> = ticked.iter().map(String::as_str).collect();
    let mut out = BTreeSet::new();
    for entry in entries.iter().filter(|e| is_ticked(e, &ticked)) {
        out.insert(entry.path.clone());
        if entry.status == EntryStatus::R
            && let Some(orig) = &entry.orig_path
        {
            out.insert(orig.clone());
        }
    }
    out
}

/// The unmerged paths among `entries` (those `ticked` only, if given).
fn unmerged<'e>(entries: &'e [FileEntry], ticked: Option<&BTreeSet<&str>>) -> Vec<&'e str> {
    entries
        .iter()
        .filter(|e| e.status == EntryStatus::U && ticked.is_none_or(|t| is_ticked(e, t)))
        .map(|e| e.path.as_str())
        .collect()
}

/// Why a commit of `paths` (pushed after, with `push`) cannot run now, if it cannot; checked
/// in its turn, before anything runs. A ticked file with conflicts is refused; while a merge
/// is in progress (`in_progress`; the others are refused before), so is any conflict left,
/// and a commit of less than every changed file. A push needs a branch (`head`) and a remote
/// to push it to.
pub(crate) async fn refused(
    ctx: &OpCtx<'_>,
    paths: &[String],
    push: bool,
    in_progress: Option<InProgressKind>,
    head: Option<&str>,
) -> Result<Option<String>> {
    let entries = changed_files(ctx).await?;
    let ticked: BTreeSet<&str> = paths.iter().map(String::as_str).collect();
    if in_progress == Some(InProgressKind::Merge) {
        let left = unmerged(&entries, None);
        if !left.is_empty() {
            return Ok(Some(format!(
                "The merge still has conflicts in {}: resolve them, then commit.",
                file_list(&left)
            )));
        }
        if !entries.iter().all(|e| is_ticked(e, &ticked)) {
            return Ok(Some(MERGE_NEEDS_ALL.to_owned()));
        }
    } else {
        let conflicted = unmerged(&entries, Some(&ticked));
        if !conflicted.is_empty() {
            let verb = if conflicted.len() == 1 { "has" } else { "have" };
            return Ok(Some(format!(
                "Nothing was committed: {} still {verb} conflicts. Resolve them first: a file \
                 with conflicts cannot be committed.",
                file_list(&conflicted)
            )));
        }
    }
    if push {
        let Some(branch) = head else {
            return Ok(Some(
                "Nothing was committed: HEAD is detached, so there is no branch to push. \
                 Commit without pushing, or check out a branch first."
                    .to_owned(),
            ));
        };
        if push_remote(ctx, branch).await?.is_none() {
            let remotes = network::remotes(ctx).await?;
            return Ok(Some(if remotes.is_empty() {
                "Nothing was committed: this repository has no remote to push to. Commit \
                 without pushing."
                    .to_owned()
            } else {
                format!(
                    "Nothing was committed: it is not clear which remote to push {branch} to \
                     (it has no upstream, and none of the remotes is named origin). Commit \
                     without pushing, then push from the branch's menu."
                )
            }));
        }
    }
    Ok(None)
}

/// The remote a commit's push goes to (R38: the branch of the same name there): the
/// remote of `branch`'s upstream, else origin, else the only remote.
async fn push_remote(ctx: &OpCtx<'_>, branch: &str) -> Result<Option<String>> {
    let remotes = network::remotes(ctx).await?;
    if let Some(up) = network::upstream(ctx, branch).await?
        && remotes.contains(&up.remote)
    {
        return Ok(Some(up.remote));
    }
    if remotes.iter().any(|r| r == "origin") {
        return Ok(Some("origin".to_owned()));
    }
    Ok(match remotes.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    })
}

/// A folder of the commit's temporary files (its index, the list of paths, the message)
/// beside the worktree's own index, so on the same file system; removed with everything in
/// it when dropped, however the commit ended.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    async fn new(ctx: &OpCtx<'_>) -> Result<Self> {
        let out = ctx.read(&["rev-parse", "--git-path", "index"]).await?;
        if out.code != 0 {
            return Err(
                CoreError::new(ErrorCode::GitFailed, "Could not find the index.")
                    .with_detail(out.stderr),
            );
        }
        let index = ctx.cwd.join(out.text().trim_end_matches(['\r', '\n']));
        let beside = index.parent().unwrap_or(&ctx.cwd);
        let base = format!("gg-commit-{}-{}", std::process::id(), ctx.op_id);
        for n in 0..100 {
            let dir = if n == 0 {
                beside.join(&base)
            } else {
                beside.join(format!("{base}-{n}"))
            };
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(Self { dir }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(io_error(&dir, &e)),
            }
        }
        Err(CoreError::new(
            ErrorCode::Io,
            format!("Could not make a temporary folder in {}.", beside.display()),
        ))
    }

    /// The temporary index (git makes it).
    fn index(&self) -> PathBuf {
        self.dir.join("index")
    }

    /// Writes `paths`, each ended by NUL, for `--pathspec-from-file --pathspec-file-nul`;
    /// returns the argument naming the file.
    fn paths(&self, paths: &BTreeSet<String>) -> Result<String> {
        let file = self.dir.join("paths");
        let mut bytes = Vec::new();
        for path in paths {
            bytes.extend_from_slice(path.as_bytes());
            bytes.push(0);
        }
        fs::write(&file, bytes).map_err(|e| io_error(&file, &e))?;
        Ok(format!("--pathspec-from-file={}", file.display()))
    }

    /// Writes `message` as it is; returns the file's path, for `-F`.
    fn message(&self, message: &str) -> Result<String> {
        let file = self.dir.join("message");
        fs::write(&file, message).map_err(|e| io_error(&file, &e))?;
        Ok(file.display().to_string())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn io_error(path: &Path, e: &io::Error) -> CoreError {
    CoreError::new(
        ErrorCode::Io,
        format!("Could not write {}.", path.display()),
    )
    .with_detail(e.to_string())
}

/// git's output for a failed step: stderr, and stdout after it when there is any.
fn output(out: &GitOutput) -> String {
    let (err, text) = (out.stderr.trim_end(), out.text());
    let text = text.trim_end();
    match (err.is_empty(), text.is_empty()) {
        (_, true) => err.to_owned(),
        (true, false) => text.to_owned(),
        (false, false) => format!("{err}\n{text}"),
    }
}

/// The failed step for a step of the commit other than `commit` itself (`add`, `read-tree`)
/// that ended with `out`: nothing was committed.
fn step_failed(args: &[&str], out: &GitOutput) -> Step {
    let detail = output(out);
    let message = recognised(args, &detail).unwrap_or_else(|| {
        let first = detail
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("git failed");
        format!("Nothing was committed: {first}")
    });
    Step::Failed {
        message,
        detail,
        hints: Vec::new(),
    }
}

/// How `git commit` (run with `args`) ending with `out`, not 0, went: nothing to commit is
/// `nothing_to_do`; else failed, with a plain line for what the core recognises (no identity,
/// signing, an empty message, a lock file) and git's and the hooks' output as detail.
fn commit_failed(args: &[&str], out: &GitOutput) -> Result<Step> {
    let detail = output(out);
    let has = |needle: &str| detail.contains(needle);
    let nothing = detail.lines().map(str::trim).any(|l| {
        l.starts_with("nothing to commit")
            || l.starts_with("nothing added to commit")
            || l.starts_with("no changes added to commit")
    });
    if nothing {
        return Err(nothing_to_commit());
    }
    let message = if has("Please tell me who you are")
        || has("unable to auto-detect email address")
        || has("unable to auto-detect name")
        || has("empty ident name")
    {
        NO_IDENTITY.to_owned()
    } else if has("gpg failed to sign the data") || has("failed to sign commit object") {
        SIGNING_FAILED.to_owned()
    } else if has("Aborting commit due to empty commit message") {
        EMPTY_MESSAGE.to_owned()
    } else {
        recognised(args, &detail).unwrap_or_else(|| REFUSED.to_owned())
    };
    Ok(Step::Failed {
        message,
        detail,
        hints: Vec::new(),
    })
}

/// Of `paths`, those `git add -A` can be given in the index `runner` uses: a path neither in
/// the working tree nor in that index (an untracked file deleted since, a file added and
/// deleted again) matches nothing, would fail the whole `add`, and has nothing to commit.
async fn addable(
    ctx: &OpCtx<'_>,
    runner: &GitRunner,
    paths: BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let missing: Vec<&String> = paths
        .iter()
        .filter(|p| fs::symlink_metadata(ctx.cwd.join(p)).is_err())
        .collect();
    if missing.is_empty() {
        return Ok(paths);
    }
    let mut indexed = BTreeSet::new();
    for chunk in missing.chunks(LS_CHUNK) {
        let mut args = vec!["--literal-pathspecs", "ls-files", "-z", "--cached", "--"];
        args.extend(chunk.iter().map(|p| p.as_str()));
        let out = runner.run_unchecked(&ctx.cwd, &args, CmdKind::Read).await?;
        if out.code != 0 {
            return Err(
                CoreError::new(ErrorCode::GitFailed, "Could not read the index.")
                    .with_detail(out.stderr),
            );
        }
        indexed.extend(
            out.stdout
                .split(|&b| b == 0)
                .filter(|p| !p.is_empty())
                .map(|p| String::from_utf8_lossy(p).into_owned()),
        );
    }
    let gone: BTreeSet<&String> = missing
        .into_iter()
        .filter(|p| !indexed.contains(*p))
        .collect();
    Ok(paths
        .iter()
        .filter(|p| !gone.contains(p))
        .cloned()
        .collect())
}

/// How many files commit `new` changed compared with `parent` (renames found, each one
/// file), or with nothing for a first commit.
async fn changed_count(ctx: &OpCtx<'_>, parent: Option<&str>, new: &str) -> Result<usize> {
    let args: Vec<&str> = match parent {
        Some(parent) => vec!["diff-tree", "-r", "-z", "--name-only", "-M", parent, new],
        None => vec!["diff-tree", "--root", "-r", "-z", "--name-only", new],
    };
    let out = ctx.read(&args).await?;
    // The root form names the commit first.
    let names = out.stdout.split(|&b| b == 0).filter(|p| !p.is_empty());
    Ok(match parent {
        Some(_) => names.count(),
        None => names.skip(1).count(),
    })
}

/// Commits `paths` with `message` on the branch checked out (see the module comment), then,
/// with `push`, pushes the branch (R38) and says how that went separately.
pub(crate) async fn commit(
    ctx: &OpCtx<'_>,
    paths: &[String],
    message: &str,
    push: bool,
) -> Result<Step> {
    let in_merge = read_kind(&ctx.runner, &ctx.cwd).await? == Some(InProgressKind::Merge);
    let entries = changed_files(ctx).await?;
    let chosen = chosen(&entries, paths);
    if chosen.is_empty() {
        return Err(nothing_to_commit());
    }
    let parent = head_commit(ctx).await?;
    let scratch = Scratch::new(ctx).await?;
    let message_file = scratch.message(message)?;
    let commit_args = ["commit", "--cleanup=strip", "-F", message_file.as_str()];

    let runner = if in_merge {
        // The user's index: a merge is committed whole.
        ctx.runner.clone()
    } else {
        let temp = ctx.runner.with_index_file(&scratch.index());
        if let Some(parent) = &parent {
            let args = ["read-tree", parent.as_str()];
            let out = temp.run_unchecked(&ctx.cwd, &args, CmdKind::Write).await?;
            if out.code != 0 {
                return Ok(step_failed(&args, &out));
            }
        }
        temp
    };
    let chosen = addable(ctx, &runner, chosen).await?;
    if chosen.is_empty() {
        return Err(nothing_to_commit());
    }
    let paths_arg = scratch.paths(&chosen)?;
    let add = [
        "--literal-pathspecs",
        "add",
        "-A",
        paths_arg.as_str(),
        "--pathspec-file-nul",
    ];
    let out = runner.run_unchecked(&ctx.cwd, &add, CmdKind::Write).await?;
    if out.code != 0 {
        return Ok(step_failed(&add, &out));
    }
    if !in_merge && let Some(parent) = &parent {
        // Nothing differs from HEAD: no commit, and no hook runs for nothing.
        let same = ["diff-index", "--cached", "--quiet", parent.as_str(), "--"];
        let out = runner.run_unchecked(&ctx.cwd, &same, CmdKind::Read).await?;
        if out.code == 0 {
            return Err(nothing_to_commit());
        }
    }
    let out = runner
        .run_unchecked(&ctx.cwd, &commit_args, CmdKind::Write)
        .await?;
    if out.code != 0 {
        return commit_failed(&commit_args, &out);
    }
    let new = head_commit(ctx)
        .await?
        .ok_or_else(|| CoreError::new(ErrorCode::GitFailed, "The new commit cannot be found."))?;

    // The committed paths are as the new HEAD has them in the user's index too; nothing else
    // there changes.
    let mut note = None;
    if !in_merge {
        let reset = [
            "--literal-pathspecs",
            "reset",
            "-q",
            paths_arg.as_str(),
            "--pathspec-file-nul",
        ];
        let out = ctx.git(&reset).await?;
        if out.code != 0 {
            note = Some(output(&out));
        }
    }
    drop(scratch);

    let count = changed_count(ctx, parent.as_deref(), &new).await?;
    let branch = current_branch(ctx).await?;
    let on = match &branch {
        Some(branch) => format!("to {branch}"),
        None => "on the detached HEAD".to_owned(),
    };
    let committed = if in_merge {
        format!(
            "Concluded the merge: committed {} {on} ({}).",
            files(count),
            short(&new)
        )
    } else {
        format!("Committed {} {on} ({}).", files(count), short(&new))
    };
    if let Some(detail) = note {
        return Ok(Step::DoneWith {
            message: format!(
                "{committed} The committed files may still show as changed: updating the index \
                 failed (below)."
            ),
            detail: Some(detail),
            hints: Vec::new(),
        });
    }
    if !push {
        return Ok(Step::Done(committed));
    }
    Ok(push_after(ctx, branch, committed).await)
}

/// Pushes `branch` after its commit (described by `committed`), as a push of it does (R38),
/// and says how that went: a failure says the commit was made, with git's output and a push
/// to try again.
async fn push_after(ctx: &OpCtx<'_>, branch: Option<String>, committed: String) -> Step {
    let failed = |why: &str, detail: String, mut hints: Vec<OpHint>, retry: Option<OpHint>| {
        if let Some(retry) = retry {
            hints.insert(0, retry);
        }
        Step::Failed {
            message: format!("Committed. The push failed: {why}"),
            detail: if detail.is_empty() {
                committed.clone()
            } else {
                format!("{committed}\n{detail}")
            },
            hints,
        }
    };
    let Some(branch) = branch else {
        return failed(
            "HEAD is detached, so there is no branch to push.",
            String::new(),
            Vec::new(),
            None,
        );
    };
    let remote = match push_remote(ctx, &branch).await {
        Ok(Some(remote)) => remote,
        Ok(None) => {
            return failed(
                "there is no remote to push it to.",
                String::new(),
                Vec::new(),
                None,
            );
        }
        Err(e) => return failed(&e.message, e.detail.unwrap_or_default(), Vec::new(), None),
    };
    let retry = Some(OpHint::Push {
        branch: branch.clone(),
        remote: remote.clone(),
    });
    match network::push(ctx, &branch, &remote, None, false, None, None).await {
        Ok(Step::Done(pushed)) => Step::Done(format!("{committed} {pushed}")),
        Ok(Step::DoneWith {
            message,
            detail,
            hints,
        }) => Step::DoneWith {
            message: format!("{committed} {message}"),
            detail,
            hints,
        },
        Ok(Step::Failed {
            message,
            detail,
            hints,
        }) => failed(&message, detail, hints, retry),
        Ok(Step::Stopped { message, detail }) => failed(&message, detail, Vec::new(), retry),
        Err(e) => failed(&e.message, e.detail.unwrap_or_default(), Vec::new(), retry),
    }
}

impl RepoManager {
    /// The changed files of the worktree at `path`, one of repository `repo_id`'s (any of
    /// them, not only the active one), sorted by path, at most
    /// [`crate::dto::MAX_LISTED_FILES`] (`truncated` when there are more). `bad_request` for
    /// a path git does not list as a worktree of it, or the bare repository itself;
    /// `path_missing` for a worktree whose folder is gone.
    pub async fn worktree_files(&self, repo_id: u32, path: &Path) -> Result<WorktreeFiles> {
        let repo = self.get(repo_id)?;
        let target = canonical(path);
        let listed = canonical_paths(read_worktrees(&repo.runner, &repo.root).await?);
        let Some(wt) = listed.iter().find(|w| Path::new(&w.path) == target) else {
            return Err(CoreError::new(
                ErrorCode::BadRequest,
                format!("{} is not a worktree of this repository.", path.display()),
            ));
        };
        if wt.bare {
            return Err(CoreError::new(
                ErrorCode::BadRequest,
                format!(
                    "{} is the bare repository itself, which has no files to commit.",
                    path.display()
                ),
            ));
        }
        if wt.prunable || !target.is_dir() {
            return Err(CoreError::new(
                ErrorCode::PathMissing,
                format!(
                    "The worktree {} is gone (its folder is missing).",
                    path.display()
                ),
            ));
        }
        read_files(&repo.runner, &target).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, status: EntryStatus, orig: Option<&str>) -> FileEntry {
        FileEntry {
            path: path.into(),
            orig_path: orig.map(str::to_owned),
            status,
            staged: true,
            unstaged: false,
        }
    }

    fn strings(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| (*p).to_owned()).collect()
    }

    #[test]
    fn requests_are_checked_without_git() {
        let code =
            |paths: &[&str], message: &str| check(&strings(paths), message).err().map(|e| e.code);
        assert_eq!(code(&["a"], "m"), None);
        assert_eq!(code(&["-x", "a b/\u{fc}.txt", "*"], "m"), None);
        assert_eq!(code(&[], "m"), Some(ErrorCode::NothingToDo));
        assert_eq!(code(&["a"], " \n\t"), Some(ErrorCode::BadRequest));
        for bad in ["", "/etc/passwd", "../out", "a/../../b", "./a", "a\0b"] {
            assert_eq!(code(&[bad], "m"), Some(ErrorCode::BadRequest), "{bad:?}");
        }
    }

    #[test]
    fn a_rename_is_committed_whole_and_unknown_paths_record_nothing() {
        let entries = [
            entry("new", EntryStatus::R, Some("old")),
            entry("copy", EntryStatus::C, Some("source")),
            entry("m", EntryStatus::M, None),
        ];
        let set = |paths: &[&str]| -> Vec<String> {
            chosen(&entries, &strings(paths)).into_iter().collect()
        };
        assert_eq!(set(&["new"]), ["new", "old"]);
        assert_eq!(set(&["old"]), ["new", "old"]);
        // A copy's source is not changed by it: only the copy.
        assert_eq!(set(&["copy"]), ["copy"]);
        assert_eq!(set(&["m", "not changed"]), ["m"]);
        assert!(set(&["not changed"]).is_empty());
    }

    #[test]
    fn plural() {
        assert_eq!(files(1), "1 file");
        assert_eq!(files(0), "0 files");
        assert_eq!(files(3), "3 files");
    }
}
