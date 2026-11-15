//! Worktree operations: add, remove and prune, and where a new worktree goes by default.
//! They run from the main worktree and name the worktree they are about (P3-R5).
//!
//! Removing is the one place the app deletes a whole folder, and it never does so itself:
//! only `git worktree remove` does, for a path git lists as a linked worktree of this
//! repository (checked again in the operation's turn), with `--force` only once the work it
//! holds has been confirmed with the token of that work, and never `--force` twice (a locked
//! worktree is refused, not unlocked).

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::dto::{
    Danger, DangerKind, InProgressKind, OpHint, OpRequest, WorktreeBranch, WorktreeInfo,
    WorktreePath,
};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::names::branch_slug;
use crate::git::progress::read_kind;
use crate::git::runner::CmdKind;
use crate::git::worktrees::read_worktrees;
use crate::ops::danger::{MAX_LOST_LISTED, fnv1a, unmerged_branch};
use crate::ops::local::{kind_name, short};
use crate::ops::{OpCtx, Step, explain};
use crate::repo::{RepoManager, canonical, canonical_paths};

/// Most numbered folders (`-2`, `-3`…) tried for a default location.
const MAX_SUFFIX: u32 = 1000;

/// Said when git refuses to remove a worktree that holds submodules (it does so even for a
/// clean one).
const SUBMODULES: &str = "git does not remove a worktree that contains submodules, even a \
                          clean one (git's message is below), so nothing was removed. \
                          Deinitialize them there first (git submodule deinit --all), then \
                          remove the worktree again.";

/// `path` (absolute, or relative to the main worktree `root`) with `.` and `..` worked out,
/// without looking at the disk.
fn resolve(root: &Path, path: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for part in root.join(path).components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// `path` with links resolved as far as it exists: its deepest existing folder resolved,
/// the rest of it appended. So it compares with the paths git lists (resolved) whether or
/// not the folder is there.
fn real(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    loop {
        if let Ok(resolved) = dunce::canonicalize(&existing) {
            return rest
                .iter()
                .rev()
                .fold(resolved, |out, part: &std::ffi::OsString| out.join(part));
        }
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_owned());
                existing = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// The repository's worktrees as git lists them now, paths resolved.
async fn listed(ctx: &OpCtx<'_>) -> Result<Vec<WorktreeInfo>> {
    Ok(canonical_paths(
        read_worktrees(&ctx.runner, &ctx.repo.root).await?,
    ))
}

/// The worktree git lists at `path` (as the request names it), with the whole list.
async fn find(ctx: &OpCtx<'_>, path: &str) -> Result<(Option<WorktreeInfo>, Vec<WorktreeInfo>)> {
    let target = real(&resolve(&ctx.repo.root, path));
    let list = listed(ctx).await?;
    let found = list.iter().find(|w| Path::new(&w.path) == target).cloned();
    Ok((found, list))
}

/// The worktree other than `wt` that has `branch` checked out, if any.
fn holder<'l>(
    list: &'l [WorktreeInfo],
    wt: &WorktreeInfo,
    branch: &str,
) -> Option<&'l WorktreeInfo> {
    list.iter()
        .find(|w| w.path != wt.path && w.branch.as_deref() == Some(branch))
}

/// Why the worktree operation `request` cannot run as asked, if it cannot (nothing runs
/// then): checked in its turn, against git's list of worktrees as it is then.
pub(crate) async fn refused(ctx: &OpCtx<'_>, request: &OpRequest) -> Result<Option<String>> {
    match request {
        OpRequest::WorktreeAdd { path, branch } => add_refused(ctx, path, branch).await,
        OpRequest::WorktreeRemove {
            path,
            delete_branch,
        } => remove_refused(ctx, path, *delete_branch).await,
        _ => Ok(None),
    }
}

/// Whether the ref `full` exists.
async fn ref_exists(ctx: &OpCtx<'_>, full: &str) -> Result<bool> {
    Ok(ctx
        .read(&["rev-parse", "--verify", "--quiet", full])
        .await?
        .code
        == 0)
}

/// Why a worktree cannot be added at `path` checking out `branch`, if it cannot: the
/// branch to check out is no local branch, the one to make exists, or the folder is not
/// new or empty, is a link, is (inside) a worktree git lists, or is inside the git dir.
async fn add_refused(
    ctx: &OpCtx<'_>,
    path: &str,
    branch: &WorktreeBranch,
) -> Result<Option<String>> {
    match branch {
        WorktreeBranch::Existing { existing } => {
            if !ref_exists(ctx, &format!("refs/heads/{existing}")).await? {
                return Ok(Some(format!(
                    "Nothing was created: there is no local branch named \u{201c}{existing}\u{201d}."
                )));
            }
        }
        WorktreeBranch::Create { create, .. } => {
            if ref_exists(ctx, &format!("refs/heads/{create}")).await? {
                return Ok(Some(format!(
                    "Nothing was created: a branch named \u{201c}{create}\u{201d} already exists."
                )));
            }
        }
        WorktreeBranch::Detach { .. } => {}
    }
    let resolved = resolve(&ctx.repo.root, path);
    let shown = resolved.display();
    let target = real(&resolved);
    for wt in listed(ctx).await? {
        let there = Path::new(&wt.path);
        if target == there {
            return Ok(Some(if wt.prunable {
                format!(
                    "Nothing was created: git still lists a worktree at {shown}, whose folder \
                     is gone. Prune stale entries first."
                )
            } else {
                format!("Nothing was created: there is a worktree at {shown} already.")
            }));
        }
        if target.starts_with(there) {
            return Ok(Some(format!(
                "Nothing was created: {shown} is inside the worktree at {}. A worktree cannot \
                 go inside another.",
                wt.path
            )));
        }
    }
    match fs::symlink_metadata(&resolved) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Ok(Some(format!(
                "Nothing was created: {shown} is a link. Choose a new folder, or an empty one."
            )));
        }
        Ok(meta) if !meta.is_dir() => {
            return Ok(Some(format!(
                "Nothing was created: {shown} exists and is not a folder."
            )));
        }
        Ok(_) => {
            let empty = fs::read_dir(&resolved)
                .map_err(|e| io_error(&resolved, &e))?
                .next()
                .is_none();
            if !empty {
                return Ok(Some(format!(
                    "Nothing was created: the folder {shown} is not empty. A new worktree needs \
                     a new folder, or an empty one."
                )));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_error(&resolved, &e)),
    }
    let git_dir = canonical(&ctx.repo.common_dir);
    if target.starts_with(&git_dir) {
        return Ok(Some(format!(
            "Nothing was created: {shown} is inside the repository's git folder ({}).",
            git_dir.display()
        )));
    }
    Ok(None)
}

fn io_error(path: &Path, e: &io::Error) -> CoreError {
    CoreError::new(
        ErrorCode::Io,
        format!("Could not look at {}.", path.display()),
    )
    .with_detail(e.to_string())
}

/// `git worktree add` at `path` (resolved against the main worktree), checking out
/// `branch`; paths after `--`. git makes the missing parent folders and runs the
/// `post-checkout` hook, as in a terminal. The new worktree is not made active. On failure,
/// whatever git left is reported, never deleted here.
pub(crate) async fn add(ctx: &OpCtx<'_>, path: &str, branch: &WorktreeBranch) -> Result<Step> {
    let resolved = resolve(&ctx.repo.root, path);
    let at = resolved.to_string_lossy().into_owned();
    let existed = resolved.exists();
    let args: Vec<&str> = match branch {
        WorktreeBranch::Existing { existing } => vec!["worktree", "add", "--", &at, existing],
        WorktreeBranch::Create { create, at: commit } => {
            vec!["worktree", "add", "-b", create, "--", &at, commit]
        }
        WorktreeBranch::Detach { detach } => {
            vec!["worktree", "add", "--detach", "--", &at, detach]
        }
    };
    let out = ctx.git(&args).await?;
    // Said as git lists it, links resolved.
    let shown = real(&resolved).to_string_lossy().into_owned();
    if out.code == 0 {
        let what = match branch {
            WorktreeBranch::Existing { existing } => format!("on branch {existing}"),
            WorktreeBranch::Create { create, at: commit } => {
                format!("on a new branch {create}, at {}", short(commit))
            }
            WorktreeBranch::Detach { detach } => {
                format!("at {}, detached (no branch checked out)", short(detach))
            }
        };
        return Ok(Step::Done(format!(
            "Created a worktree at {shown}, {what}."
        )));
    }
    let Step::Failed {
        message,
        detail,
        hints,
    } = explain(&args, &out)
    else {
        return Ok(explain(&args, &out));
    };
    // git may have made the worktree and failed after (a post-checkout hook that failed).
    let target = real(&resolved);
    let made = listed(ctx)
        .await
        .is_ok_and(|list| list.iter().any(|w| Path::new(&w.path) == target));
    let message = if made {
        format!(
            "The worktree at {shown} was created, but git reported a failure after making it \
             (a hook that failed, most likely; git's output is below)."
        )
    } else if !existed && resolved.exists() {
        format!("{message} git left the folder {shown} behind; gg has not deleted it.")
    } else {
        message
    };
    Ok(Step::Failed {
        message,
        detail,
        hints,
    })
}

/// Why the worktree at `path` cannot be removed, if it cannot: git does not list it, it is
/// the main worktree, it is locked (with the reason given), its folder is gone (prune it
/// instead), or its branch is to be deleted and it has none.
async fn remove_refused(
    ctx: &OpCtx<'_>,
    path: &str,
    delete_branch: bool,
) -> Result<Option<String>> {
    let (found, list) = find(ctx, path).await?;
    let Some(wt) = found else {
        return Ok(Some(format!(
            "Nothing was removed: {path} is not a worktree of this repository (git does not \
             list it)."
        )));
    };
    let shown = &wt.path;
    if wt.is_main || Path::new(shown) == ctx.repo.root {
        return Ok(Some(format!(
            "Nothing was removed: {shown} is the main worktree, which holds the repository \
             itself. Only linked worktrees can be removed."
        )));
    }
    if wt.locked {
        let why = wt
            .lock_reason
            .as_deref()
            .map_or_else(String::new, |r| format!(" ({r})"));
        return Ok(Some(format!(
            "Nothing was removed: the worktree at {shown} is locked{why}. git keeps a locked \
             worktree; unlock it first (git worktree unlock) if it may go."
        )));
    }
    if wt.prunable || !Path::new(shown).is_dir() {
        return Ok(Some(format!(
            "Nothing was removed: the folder of the worktree at {shown} is missing, so there \
             is nothing to remove. Prune stale entries to make git forget it."
        )));
    }
    if delete_branch && wt.branch.is_none() {
        return Ok(Some(format!(
            "Nothing was removed: the worktree at {shown} has no branch checked out (its HEAD \
             is detached), so there is no branch to delete with it."
        )));
    }
    // `git worktree remove` deletes the whole folder, whatever is inside: never another
    // worktree or repository.
    if let Some(inner) = list
        .iter()
        .find(|w| w.path != wt.path && Path::new(&w.path).starts_with(shown))
    {
        return Ok(Some(format!(
            "{shown} contains the worktree at {}. Remove that one first.",
            inner.path
        )));
    }
    if let Some(inner) = nested_repository(ctx, &wt).await? {
        return Ok(Some(format!(
            "{shown} contains another repository at {}, which git would delete with it. Move \
             or remove it first.",
            inner.display()
        )));
    }
    Ok(None)
}

/// Most entries of an ignored folder looked into for a repository one level down.
const MAX_LOOKED_INTO: usize = 200;

/// A repository (a folder with a `.git` file or folder) inside the worktree `wt`, if one is
/// found: an untracked or ignored folder git lists that is one (git does not go into
/// another repository, and lists an ignored folder as one entry), or a folder directly
/// inside an ignored one that is ([`MAX_LOOKED_INTO`] entries at most). A repository deeper
/// inside an ignored folder is not found.
async fn nested_repository(ctx: &OpCtx<'_>, wt: &WorktreeInfo) -> Result<Option<PathBuf>> {
    let dir = Path::new(&wt.path);
    let out = ctx
        .runner
        .run_unchecked(dir, &STATUS_WITH_IGNORED, CmdKind::Read)
        .await?;
    if out.code != 0 {
        return Err(CoreError::new(
            ErrorCode::GitFailed,
            format!(
                "Could not read what the worktree at {} holds, so nothing was removed.",
                wt.path
            ),
        )
        .with_detail(out.stderr));
    }
    let is_repo = |p: &Path| fs::symlink_metadata(p.join(".git")).is_ok();
    let mut fields = out.stdout.split(|&b| b == 0);
    while let Some(entry) = fields.next() {
        if entry.starts_with(b"2 ") {
            fields.next();
            continue;
        }
        let (ignored, rest) = match entry {
            [b'?', b' ', rest @ ..] => (false, rest),
            [b'!', b' ', rest @ ..] => (true, rest),
            _ => continue,
        };
        if rest.last() != Some(&b'/') {
            continue;
        }
        let folder = dir.join(String::from_utf8_lossy(rest).trim_end_matches('/'));
        if is_repo(&folder) {
            return Ok(Some(folder));
        }
        if ignored && let Ok(entries) = fs::read_dir(&folder) {
            for child in entries.flatten().take(MAX_LOOKED_INTO) {
                let child = child.path();
                if child.is_dir() && is_repo(&child) {
                    return Ok(Some(child));
                }
            }
        }
    }
    Ok(None)
}

/// The status read of a worktree being removed: every untracked file, and what is ignored
/// (folders collapsed).
const STATUS_WITH_IGNORED: [&str; 5] = [
    "status",
    "--porcelain=v2",
    "-z",
    "--untracked-files=all",
    "--ignored=matching",
];

/// What a worktree holds that removing it would lose, read in the operation's turn.
#[derive(Debug, Default)]
struct Work {
    /// Tracked paths with changes, staged or not (conflicted ones too).
    changed: Vec<String>,
    /// Untracked files, one by one.
    untracked: Vec<String>,
    /// What git ignores there, folders collapsed (`build/`).
    ignored: Vec<String>,
    /// Hashes of the status records (all but the ignored) and of the ignored ones.
    status_hash: u64,
    ignored_hash: u64,
    /// Detached only: the commits lost with it, on no branch, tag or remote. The first
    /// [`MAX_LOST_LISTED`] as `hash subject`, how many, and a hash of the whole list.
    commits: Vec<String>,
    commit_count: usize,
    commits_hash: u64,
    /// On a branch: its commits on no remote, which the branch keeps (removing a worktree
    /// never deletes its branch), said in a line, never a danger of their own.
    unpushed: usize,
    in_progress: Option<InProgressKind>,
}

impl Work {
    /// Anything beside ignored files and commits its branch keeps: the red danger.
    fn holds_work(&self) -> bool {
        !self.changed.is_empty()
            || !self.untracked.is_empty()
            || self.commit_count > 0
            || self.in_progress.is_some()
    }
}

/// Reads what the worktree `wt` holds: what git is stopped in there, its commits (detached:
/// those on no branch, tag or remote, lost with it; on a branch: how many are on no remote),
/// counted from the main worktree, and, read last, its files ([`read_files_held`]). Stashes are shared by all worktrees and not lost
/// with one.
async fn read_work(ctx: &OpCtx<'_>, wt: &WorktreeInfo) -> Result<Work> {
    let dir = Path::new(&wt.path);
    let in_progress = read_kind(&ctx.runner, dir).await?;
    let (mut commit_count, mut commits_hash, mut commits) = (0, 0, Vec::new());
    let mut unpushed = 0;
    if let (Some(head), Some(_)) = (&wt.head, &wt.branch) {
        let out = ctx
            .read(&["rev-list", "--count", head, "--not", "--remotes"])
            .await?;
        unpushed = out.text().trim().parse().unwrap_or(0);
    } else if let Some(head) = &wt.head {
        let only_here = [head.as_str(), "--not", "--branches", "--tags", "--remotes"];
        let mut list = vec!["rev-list"];
        list.extend(&only_here);
        let out = ctx.read(&list).await?;
        if out.code == 0 {
            commit_count = out.text().lines().filter(|l| !l.is_empty()).count();
            commits_hash = fnv1a(&out.stdout);
        }
        if commit_count > 0 {
            let limit = MAX_LOST_LISTED.to_string();
            let mut log = vec!["log", "--abbrev=8", "--format=%h %s", "-n", &limit];
            log.extend(&only_here);
            let out = ctx.read(&log).await?;
            commits = out
                .text()
                .lines()
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
        }
    }
    Ok(Work {
        in_progress,
        unpushed,
        commit_count,
        commits_hash,
        commits,
        ..read_files_held(ctx, wt).await?
    })
}

/// The files the worktree `wt` holds, from `status --porcelain=v2 -z --untracked-files=all
/// --ignored=matching` there (ignored folders come collapsed): changed, untracked and
/// ignored, with the hashes of the records.
async fn read_files_held(ctx: &OpCtx<'_>, wt: &WorktreeInfo) -> Result<Work> {
    let dir = Path::new(&wt.path);
    let out = ctx
        .runner
        .run_unchecked(dir, &STATUS_WITH_IGNORED, CmdKind::Read)
        .await?;
    if out.code != 0 {
        return Err(CoreError::new(
            ErrorCode::GitFailed,
            format!(
                "Could not read what the worktree at {} holds, so nothing was removed.",
                wt.path
            ),
        )
        .with_detail(out.stderr));
    }
    Ok(parse_status(&out.stdout))
}

/// The status and ignored-file hashes in a `remove_worktree_*` danger's `token`, as
/// [`work_danger`] wrote them.
fn token_hashes(token: &str) -> Option<(&str, &str)> {
    let parts: Vec<&str> = token.split('\n').collect();
    match parts.as_slice() {
        [_, _, status, _, ignored] => Some((status, ignored)),
        _ => None,
    }
}

/// The paths of `status --porcelain=v2 -z` output (`1`, `2` with its original path, `u`,
/// `?`, `!`), sorted into changed, untracked and ignored, with hashes of the records.
fn parse_status(out: &[u8]) -> Work {
    let mut work = Work::default();
    let (mut kept, mut ignored) = (Vec::new(), Vec::new());
    let mut fields = out.split(|&b| b == 0);
    while let Some(entry) = fields.next() {
        let text = String::from_utf8_lossy(entry);
        let path = |n: usize| text.splitn(n + 1, ' ').nth(n).map(str::to_owned);
        match entry.first() {
            Some(b'1') => work.changed.extend(path(8)),
            Some(b'2') => {
                work.changed.extend(path(9));
                // The original path follows as a field of its own.
                if let Some(orig) = fields.next() {
                    kept.extend_from_slice(orig);
                    kept.push(0);
                }
            }
            Some(b'u') => work.changed.extend(path(10)),
            Some(b'?') => work.untracked.extend(path(1)),
            Some(b'!') => {
                work.ignored.extend(path(1));
                ignored.extend_from_slice(entry);
                ignored.push(0);
                continue;
            }
            _ => continue,
        }
        kept.extend_from_slice(entry);
        kept.push(0);
    }
    work.status_hash = fnv1a(&kept);
    work.ignored_hash = fnv1a(&ignored);
    work
}

/// The dangers of removing the worktree at `path` (and with `delete_branch`, its branch),
/// worked out from a fresh read in the operation's turn. None for a clean, pushed worktree
/// with no ignored files. A worktree with submodules carries none: it is removed without
/// `--force`, which git refuses (its message is passed on).
pub(crate) async fn removal_dangers(
    ctx: &OpCtx<'_>,
    path: &str,
    delete_branch: bool,
) -> Result<Vec<Danger>> {
    let (Some(wt), list) = find(ctx, path).await? else {
        return Ok(Vec::new());
    };
    let mut dangers = Vec::new();
    if delete_branch
        && let Some(branch) = &wt.branch
        && holder(&list, &wt, branch).is_none()
        && let Some(mut danger) = unmerged_branch(ctx, branch).await?
    {
        danger.kind = DangerKind::DeleteUnmergedBranchWithWorktree;
        let full = format!("refs/heads/{branch}");
        // Its commits on no other branch, tag or remote: what deleting it loses. (With
        // `--branches`, `--exclude` takes the name without `refs/heads/`; a branch name has no
        // glob characters.)
        let out = ctx
            .read(&[
                "rev-list",
                "--count",
                &full,
                "--not",
                &format!("--exclude={branch}"),
                "--branches",
                "--tags",
                "--remotes",
            ])
            .await?;
        match out.text().trim().parse::<u64>().unwrap_or(0) {
            0 => {}
            1 => danger.message.push_str(
                " 1 of its commits is not pushed anywhere (it is on no other branch, tag or \
                 remote).",
            ),
            n => danger.message.push_str(&format!(
                " {n} of its commits are not pushed anywhere (they are on no other branch, tag \
                 or remote)."
            )),
        }
        dangers.push(danger);
    }
    // The files last, so the dialog shows them as late as can be.
    if !has_submodules(ctx, &wt).await? {
        let work = read_work(ctx, &wt).await?;
        if let Some(danger) = work_danger(&work, &wt, delete_branch) {
            dangers.insert(0, danger);
        }
    }
    Ok(dangers)
}

/// Whether git would refuse to remove the worktree `wt` for its submodules: its git dir has
/// a `modules` folder, or a submodule (a gitlink in its index) is checked out there.
async fn has_submodules(ctx: &OpCtx<'_>, wt: &WorktreeInfo) -> Result<bool> {
    let dir = Path::new(&wt.path);
    let git_dir = ctx
        .runner
        .run_unchecked(dir, &["rev-parse", "--absolute-git-dir"], CmdKind::Read)
        .await?;
    if git_dir.code == 0 && Path::new(git_dir.text().trim()).join("modules").is_dir() {
        return Ok(true);
    }
    let staged = ctx
        .runner
        .run_unchecked(dir, &["ls-files", "--stage", "-z"], CmdKind::Read)
        .await?;
    Ok(staged.stdout.split(|&b| b == 0).any(|entry| {
        let entry = String::from_utf8_lossy(entry);
        entry.starts_with("160000 ")
            && entry
                .split_once('\t')
                .is_some_and(|(_, path)| dir.join(path).join(".git").exists())
    }))
}

/// `n` things, singular or plural.
fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// The ignored entries, counted as files, folders or both.
fn ignored_count(ignored: &[String]) -> String {
    let folders = ignored.iter().filter(|p| p.ends_with('/')).count();
    let files = ignored.len() - folders;
    match (files, folders) {
        (_, 0) => count(files, "ignored file", "ignored files"),
        (0, _) => count(folders, "ignored folder", "ignored folders"),
        _ => format!("{} ignored files and folders", ignored.len()),
    }
}

/// A titled list for a danger's detail: the first [`MAX_LOST_LISTED`] of `items` (of
/// `total`), then how many more.
fn section(lines: &mut Vec<String>, title: &str, items: &[String], total: usize) {
    if total == 0 {
        return;
    }
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push(format!("{title} ({total}):"));
    lines.extend(items.iter().take(MAX_LOST_LISTED).map(|i| format!("  {i}")));
    if total > MAX_LOST_LISTED {
        lines.push(format!("  and {} more", total - MAX_LOST_LISTED));
    }
}

/// The danger of removing the worktree `wt`, holding `work`: none when it holds nothing;
/// `remove_worktree_ignored_files` (ordinary) when ignored files are all it holds (git
/// deletes them without asking, P3-R6); else the red `remove_worktree_with_work`, with the
/// ignored files as a line of it. Commits its branch keeps are no danger (removing a worktree
/// never deletes its branch): a line in either, unless the branch goes too (`delete_branch`:
/// its own danger says what is lost). Its token is the worktree's HEAD, what is in progress
/// there, and hashes of its status, its lost commits and its ignored files.
fn work_danger(work: &Work, wt: &WorktreeInfo, delete_branch: bool) -> Option<Danger> {
    if !work.holds_work() && work.ignored.is_empty() {
        return None;
    }
    let shown = &wt.path;
    let kept = match &wt.branch {
        Some(branch) if work.unpushed > 0 && !delete_branch => {
            let (n, them) = (
                work.unpushed,
                if work.unpushed == 1 { "it" } else { "them" },
            );
            let verb = if n == 1 { "commit is" } else { "commits are" };
            format!(" {n} {verb} not pushed; branch {branch} keeps {them}.")
        }
        _ => String::new(),
    };
    let token = Some(format!(
        "{}\n{}\n{:016x}\n{:016x}\n{:016x}",
        wt.head.as_deref().unwrap_or(""),
        work.in_progress.map_or("", kind_name),
        work.status_hash,
        work.commits_hash,
        work.ignored_hash
    ));
    let mut detail = Vec::new();
    let ignored_title = "Ignored, deleted with it";
    if !work.holds_work() {
        section(
            &mut detail,
            ignored_title,
            &work.ignored,
            work.ignored.len(),
        );
        return Some(Danger {
            kind: DangerKind::RemoveWorktreeIgnoredFiles,
            message: format!(
                "The worktree at {shown} has nothing uncommitted, but it holds {}, which git \
                 deletes with it without asking (listed below).{kept}",
                ignored_count(&work.ignored)
            ),
            detail: Some(detail.join("\n")),
            token,
        });
    }
    let mut parts = Vec::new();
    if !work.changed.is_empty() {
        parts.push(format!(
            "uncommitted changes to {}",
            count(work.changed.len(), "file", "files")
        ));
    }
    if !work.untracked.is_empty() {
        parts.push(count(
            work.untracked.len(),
            "untracked file",
            "untracked files",
        ));
    }
    if work.commit_count > 0 {
        let commits = count(work.commit_count, "commit", "commits");
        parts.push(format!("{commits} on no branch, tag or remote"));
    }
    let mut message = if parts.is_empty() {
        format!("The worktree at {shown} has work in it.")
    } else {
        format!(
            "The worktree at {shown} has {}.",
            explain::file_list(&parts.iter().map(String::as_str).collect::<Vec<_>>())
        )
    };
    match (work.changed.is_empty(), work.untracked.is_empty()) {
        (false, false) => message
            .push_str(" Removing it deletes the uncommitted changes and untracked files for good."),
        (false, true) => message.push_str(" Removing it deletes the uncommitted changes for good."),
        (true, false) => message.push_str(" Removing it deletes the untracked files for good."),
        (true, true) => {}
    }
    if work.commit_count > 0 {
        let them = if work.commit_count == 1 { "it" } else { "them" };
        message.push_str(&format!(
            " Its commits on no branch are lost with it, unless a branch is made for {them} \
             first."
        ));
    }
    if let Some(kind) = work.in_progress {
        let what = kind_name(kind);
        message.push_str(&format!(
            " A {what} is in progress there; removing the worktree abandons it."
        ));
    }
    if !work.ignored.is_empty() {
        message.push_str(&format!(
            " It also holds {}, which git deletes with it (listed below).",
            ignored_count(&work.ignored)
        ));
    }
    message.push_str(&kept);
    section(
        &mut detail,
        "Uncommitted changes",
        &work.changed,
        work.changed.len(),
    );
    section(
        &mut detail,
        "Untracked files",
        &work.untracked,
        work.untracked.len(),
    );
    section(
        &mut detail,
        "Commits on no branch, tag or remote",
        &work.commits,
        work.commit_count,
    );
    section(
        &mut detail,
        ignored_title,
        &work.ignored,
        work.ignored.len(),
    );
    Some(Danger {
        kind: DangerKind::RemoveWorktreeWithWork,
        message,
        detail: (!detail.is_empty()).then(|| detail.join("\n")),
        token,
    })
}

/// `git worktree remove -- <path>` for the worktree git lists at `path`, with `--force`
/// only when changed or untracked files were confirmed (`remove_worktree_with_work` among
/// `dangers`; ignored files, commits and something in progress git removes without it). The
/// active worktree is left first: the main one becomes active. Its files are then read again
/// and must be as confirmed (the danger's token), else nothing is removed. Then, with `delete_branch`, its branch goes (`branch -d`, or `-D` when
/// `delete_unmerged_branch_with_worktree` was confirmed), unless another worktree has it
/// checked out.
pub(crate) async fn remove(
    ctx: &OpCtx<'_>,
    dangers: &[Danger],
    path: &str,
    delete_branch: bool,
) -> Result<Step> {
    let (Some(wt), list) = find(ctx, path).await? else {
        return Ok(Step::Failed {
            message: format!(
                "Nothing was removed: {path} is no longer a worktree of this repository."
            ),
            detail: String::new(),
            hints: Vec::new(),
        });
    };
    let shown = wt.path.clone();
    let left = Path::new(&shown) == ctx.repo.worktree();
    if left {
        ctx.manager.make_main_active(ctx.repo).await?;
    }
    let main_now = format!(
        " The main worktree, {}, is active now.",
        ctx.repo.root.display()
    );
    // Read again now (after the switch, for the active one): what is removed must be what
    // was confirmed.
    let confirmed = dangers.iter().find(|d| {
        matches!(
            d.kind,
            DangerKind::RemoveWorktreeWithWork | DangerKind::RemoveWorktreeIgnoredFiles
        )
    });
    let now = read_files_held(ctx, &wt).await?;
    if let Some(token) = confirmed.and_then(|d| d.token.as_deref()) {
        let (status, ignored) = (
            format!("{:016x}", now.status_hash),
            format!("{:016x}", now.ignored_hash),
        );
        if token_hashes(token) != Some((status.as_str(), ignored.as_str())) {
            let mut message = "It changed since you confirmed; nothing was removed.".to_owned();
            if left {
                message.push_str(&main_now);
            }
            return Ok(Step::Failed {
                message,
                detail: String::new(),
                hints: Vec::new(),
            });
        }
    }
    // `--force` only for what git would otherwise refuse: changed or untracked files the user
    // confirmed. Anything else (ignored files, commits, something in progress) goes with a
    // plain remove, which keeps git's own check for changes at the moment it deletes.
    let force = confirmed.is_some_and(|d| d.kind == DangerKind::RemoveWorktreeWithWork)
        && (!now.changed.is_empty() || !now.untracked.is_empty());
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.extend(["--", shown.as_str()]);
    let out = ctx.git(&args).await?;
    if out.code != 0 {
        let all = format!("{}\n{}", out.stderr, out.text());
        let step = explain(&args, &out);
        let Step::Failed {
            message,
            detail,
            hints,
        } = step
        else {
            return Ok(step);
        };
        let mut message = if all.contains("containing submodules cannot be moved or removed") {
            SUBMODULES.to_owned()
        } else {
            message
        };
        if left {
            message.push_str(&main_now);
        }
        return Ok(Step::Failed {
            message,
            detail,
            hints,
        });
    }
    let mut message = format!("Removed the worktree at {shown}.");
    if left {
        message.push_str(&main_now);
    }
    let (true, Some(branch)) = (delete_branch, wt.branch.as_deref()) else {
        return Ok(Step::Done(message));
    };
    if let Some(other) = holder(&list, &wt, branch) {
        return Ok(Step::DoneWith {
            message: format!(
                "{message} Branch \u{201c}{branch}\u{201d} was not deleted: it is checked out \
                 in the worktree at {} too.",
                other.path
            ),
            detail: None,
            hints: vec![OpHint::MakeActive {
                worktree: other.path.clone(),
            }],
        });
    }
    let unmerged = dangers
        .iter()
        .any(|d| d.kind == DangerKind::DeleteUnmergedBranchWithWorktree);
    let args = ["branch", if unmerged { "-D" } else { "-d" }, "--", branch];
    let out = ctx.git(&args).await?;
    if out.code == 0 {
        return Ok(Step::Done(format!("{message} Deleted branch {branch}.")));
    }
    Ok(match explain(&args, &out) {
        Step::Failed {
            message: why,
            detail,
            hints,
        } => Step::Failed {
            message: format!("{message} Branch \u{201c}{branch}\u{201d} was not deleted: {why}"),
            detail,
            hints,
        },
        other => other,
    })
}

/// Said with every prune confirmation.
const DISCONNECTED: &str = "If one of these is on a disconnected drive, reconnect it or lock \
                            the worktree instead.";

/// The danger of pruning: none when no entry is prunable; else `prune_worktrees` listing
/// each entry git would forget (its path, and its branch or detached commit), or the red
/// `prune_worktrees_loses_commits` when a detached one has commits on no branch, tag or
/// remote, listed too: they are lost with it. Its token is the entries and their HEADs.
pub(crate) async fn prune_dangers(ctx: &OpCtx<'_>) -> Result<Option<Danger>> {
    let stale: Vec<WorktreeInfo> = listed(ctx)
        .await?
        .into_iter()
        .filter(|w| w.prunable && !w.locked && !w.is_main)
        .collect();
    if stale.is_empty() {
        return Ok(None);
    }
    let mut token = String::new();
    let mut entries = Vec::new();
    let mut lost: Vec<String> = Vec::new();
    let mut lost_count = 0;
    for wt in &stale {
        let head = wt.head.as_deref().unwrap_or("");
        token.push_str(&format!("{}\0{head}\n", wt.path));
        entries.push(match (&wt.branch, &wt.head) {
            (Some(branch), _) => format!("{} (branch {branch})", wt.path),
            (None, Some(head)) => format!("{} (detached at {})", wt.path, short(head)),
            (None, None) => wt.path.clone(),
        });
        if wt.branch.is_none() && !head.is_empty() {
            let only_here = [head, "--not", "--branches", "--tags", "--remotes"];
            let mut count_args = vec!["rev-list", "--count"];
            count_args.extend(only_here);
            let n: usize = ctx
                .read(&count_args)
                .await?
                .text()
                .trim()
                .parse()
                .unwrap_or(0);
            if n > 0 {
                lost_count += n;
                let limit = MAX_LOST_LISTED.to_string();
                let mut log = vec!["log", "--abbrev=8", "--format=%h %s", "-n", &limit];
                log.extend(only_here);
                lost.extend(
                    ctx.read(&log)
                        .await?
                        .text()
                        .lines()
                        .filter(|l| !l.is_empty())
                        .map(str::to_owned),
                );
            }
        }
    }
    let mut detail = Vec::new();
    section(&mut detail, "Entries forgotten", &entries, entries.len());
    section(
        &mut detail,
        "Commits on no branch, tag or remote",
        &lost,
        lost_count,
    );
    let them = count(stale.len(), "worktree entry", "worktree entries");
    let (kind, message) = if lost_count == 0 {
        (
            DangerKind::PruneWorktrees,
            format!(
                "Pruning makes git forget {them} whose folders are gone (listed below). \
                 {DISCONNECTED}"
            ),
        )
    } else {
        (
            DangerKind::PruneWorktreesLosesCommits,
            format!(
                "Pruning makes git forget {them} whose folders are gone (listed below), and \
                 with them {} on no branch, tag or remote, which are lost. {DISCONNECTED}",
                count(lost_count, "commit", "commits")
            ),
        )
    };
    Ok(Some(Danger {
        kind,
        message,
        detail: Some(detail.join("\n")),
        token: Some(token),
    }))
}

/// `git worktree prune`: git forgets the worktrees whose folders are gone (not locked
/// ones), once confirmed (see [`prune_dangers`]). The message names the entries it removed.
pub(crate) async fn prune(ctx: &OpCtx<'_>) -> Result<Step> {
    let before = listed(ctx).await?;
    let args = ["worktree", "prune"];
    let out = ctx.git(&args).await?;
    if out.code != 0 {
        return Ok(explain(&args, &out));
    }
    let after = listed(ctx).await?;
    let pruned: Vec<&str> = before
        .iter()
        .filter(|w| !after.iter().any(|a| a.path == w.path))
        .map(|w| w.path.as_str())
        .collect();
    let mut message = if pruned.is_empty() {
        "Nothing to prune: every worktree git lists has its folder.".to_owned()
    } else {
        format!(
            "Pruned {}, whose folders were gone: {}.",
            count(
                pruned.len(),
                "stale worktree entry",
                "stale worktree entries"
            ),
            explain::file_list(&pruned)
        )
    };
    // git never calls a locked worktree prunable, even with its folder gone.
    let locked = after
        .iter()
        .filter(|w| w.locked && !w.is_main && !Path::new(&w.path).is_dir())
        .count();
    if locked > 0 {
        message.push_str(&format!(
            " {} locked, so git keeps {}.",
            count(locked, "missing worktree is", "missing worktrees are"),
            if locked == 1 { "it" } else { "them" }
        ));
    }
    Ok(Step::Done(message))
}

impl RepoManager {
    /// Where a new worktree for `branch` goes by default in repository `repo_id`: `pattern`
    /// (`worktreePattern`) with `{repo}` the main worktree's folder name and `{branch}` the
    /// branch made safe for a folder name, relative to the main worktree; the first of it,
    /// `<it>-2`, `<it>-3`… that is free (no such folder, or an empty one, and not a
    /// worktree git lists). `exists`: that folder is there (empty).
    pub async fn worktree_default_path(
        &self,
        repo_id: u32,
        branch: &str,
        pattern: &str,
    ) -> Result<WorktreePath> {
        let repo = self.get(repo_id)?;
        let listed = canonical_paths(read_worktrees(&repo.runner, &repo.root).await?);
        let name = repo
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let expanded = pattern
            .replace("{branch}", &branch_slug(branch))
            .replace("{repo}", &name);
        let base = resolve(&repo.root, &expanded);
        let base_name = base
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for n in 1..=MAX_SUFFIX {
            let candidate = if n == 1 {
                base.clone()
            } else {
                base.with_file_name(format!("{base_name}-{n}"))
            };
            let target = real(&candidate);
            if listed.iter().any(|w| Path::new(&w.path) == target) {
                continue;
            }
            let exists = match fs::symlink_metadata(&candidate) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => false,
                Ok(meta)
                    if meta.is_dir()
                        && fs::read_dir(&candidate).is_ok_and(|mut d| d.next().is_none()) =>
                {
                    true
                }
                _ => continue,
            };
            return Ok(WorktreePath {
                path: candidate.to_string_lossy().into_owned(),
                exists,
            });
        }
        Err(CoreError::new(
            ErrorCode::BadRequest,
            format!(
                "No free folder was found for a worktree at {} (tried up to -{MAX_SUFFIX}).",
                base.display()
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_resolve_against_the_main_worktree() {
        let root = Path::new("/r/repo");
        assert_eq!(
            resolve(root, "../repo.worktrees/x"),
            Path::new("/r/repo.worktrees/x")
        );
        assert_eq!(resolve(root, "/abs/./y/../z"), Path::new("/abs/z"));
        assert_eq!(resolve(root, "sub"), Path::new("/r/repo/sub"));
    }

    #[test]
    fn status_records_are_sorted_into_changes_untracked_and_ignored() {
        let out = b"1 .M N... 100644 100644 100644 aaaa bbbb a file.txt\0\
                    2 R. N... 100644 100644 100644 aaaa bbbb R100 new.txt\0old.txt\0\
                    u UU N... 100644 100644 100644 100644 aaaa bbbb cccc both.txt\0\
                    ? new dir/u.txt\0\
                    ! build/\0! x.log\0";
        let work = parse_status(out);
        assert_eq!(work.changed, ["a file.txt", "new.txt", "both.txt"]);
        assert_eq!(work.untracked, ["new dir/u.txt"]);
        assert_eq!(work.ignored, ["build/", "x.log"]);
        // Only the ignored part changes the ignored hash, and the rest the status hash.
        let more = [&out[..], b"! more.log\0"].concat();
        let again = parse_status(&more);
        assert_eq!(again.status_hash, work.status_hash);
        assert_ne!(again.ignored_hash, work.ignored_hash);
    }

    #[test]
    fn counts_read_as_words() {
        assert_eq!(ignored_count(&["a.log".into()]), "1 ignored file");
        assert_eq!(
            ignored_count(&["b/".into(), "c/".into()]),
            "2 ignored folders"
        );
        assert_eq!(
            ignored_count(&["b/".into(), "x".into()]),
            "2 ignored files and folders"
        );
        let mut lines = Vec::new();
        let items: Vec<String> = (0..25).map(|i| format!("f{i}")).collect();
        section(&mut lines, "Untracked files", &items, 25);
        assert_eq!(lines.len(), 22);
        assert_eq!(lines[0], "Untracked files (25):");
        assert_eq!(lines[21], "  and 5 more");
    }
}
