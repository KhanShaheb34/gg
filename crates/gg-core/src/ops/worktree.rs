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
    DEFAULT_WORKTREE_PATTERN, Danger, DangerKind, InProgressKind, InputField, OpHint, OpRequest,
    WorktreeBranch, WorktreeInfo, WorktreePath,
};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::names::branch_slug;
use crate::git::progress::read_kind;
use crate::git::runner::CmdKind;
use crate::git::worktrees::read_worktrees;
use crate::ops::danger::{MAX_LOST_LISTED, fnv1a, stat_hash, unmerged_branch};
use crate::ops::local::{kind_name, short};
use crate::ops::{OpCtx, Step, explain};
use crate::repo::{RepoManager, canonical, canonical_paths};

/// Most numbered folders (`-2`, `-3`…) tried for a default location.
const MAX_SUFFIX: u32 = 1000;

/// Said when git refuses to remove a worktree that holds submodules (it does so even for a
/// clean one).
const SUBMODULES: &str = "git does not delete a worktree that contains submodules, even a \
                          clean one (git's message is below), so nothing was deleted. \
                          Deinitialize them there first (git submodule deinit --all), then \
                          delete the worktree again.";

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
/// A refusal of `worktree_add` names the field of the dialog it is about, when it is one.
pub(crate) async fn refused(
    ctx: &OpCtx<'_>,
    request: &OpRequest,
) -> Result<Option<(String, Option<InputField>)>> {
    match request {
        OpRequest::WorktreeAdd { path, branch } => add_refused(ctx, path, branch).await,
        OpRequest::WorktreeRemove {
            path,
            delete_branch,
        } => Ok(remove_refused(ctx, path, *delete_branch)
            .await?
            .map(|why| (why, None))),
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
) -> Result<Option<(String, Option<InputField>)>> {
    let about = |field| move |why: String| Some((why, Some(field)));
    match branch {
        WorktreeBranch::Existing { existing } => {
            if !ref_exists(ctx, &format!("refs/heads/{existing}")).await? {
                return Ok(Some((
                    format!(
                        "Nothing was created: there is no local branch named \u{201c}{existing}\u{201d}."
                    ),
                    None,
                )));
            }
        }
        WorktreeBranch::Create { create, .. } => {
            if ref_exists(ctx, &format!("refs/heads/{create}")).await? {
                return Ok(about(InputField::Name)(format!(
                    "Nothing was created: a branch named \u{201c}{create}\u{201d} already exists."
                )));
            }
        }
        WorktreeBranch::Detach { .. } => {}
    }
    let location = about(InputField::Location);
    let resolved = resolve(&ctx.repo.root, path);
    let shown = resolved.display();
    let target = real(&resolved);
    for wt in listed(ctx).await? {
        let there = Path::new(&wt.path);
        if target == there {
            return Ok(location(if wt.prunable {
                format!(
                    "Nothing was created: git still lists a worktree at {shown}, whose folder \
                     is gone. Prune stale entries first."
                )
            } else {
                format!("Nothing was created: there is a worktree at {shown} already.")
            }));
        }
        if target.starts_with(there) {
            return Ok(location(format!(
                "Nothing was created: {shown} is inside the worktree at {}. A worktree cannot \
                 go inside another.",
                wt.path
            )));
        }
    }
    match fs::symlink_metadata(&resolved) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Ok(location(format!(
                "Nothing was created: {shown} is a link. Choose a new folder, or an empty one."
            )));
        }
        Ok(meta) if !meta.is_dir() => {
            return Ok(location(format!(
                "Nothing was created: {shown} exists and is not a folder."
            )));
        }
        Ok(_) => {
            let empty = fs::read_dir(&resolved)
                .map_err(|e| io_error(&resolved, &e).about(InputField::Location))?
                .next()
                .is_none();
            if !empty {
                return Ok(location(format!(
                    "Nothing was created: the folder {shown} is not empty. A new worktree needs \
                     a new folder, or an empty one."
                )));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_error(&resolved, &e).about(InputField::Location)),
    }
    let git_dir = canonical(&ctx.repo.common_dir);
    if target.starts_with(&git_dir) {
        return Ok(location(format!(
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
    let note_created = |ctx: &OpCtx<'_>| {
        *ctx.created.lock().unwrap_or_else(|e| e.into_inner()) = Some(
            crate::repo::canonical(&resolved)
                .to_string_lossy()
                .into_owned(),
        );
    };
    if out.code == 0 {
        note_created(ctx);
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
    if made {
        note_created(ctx);
    }
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
            "Nothing was deleted: {path} is not a worktree of this repository (git does not \
             list it)."
        )));
    };
    let shown = &wt.path;
    // The folder must be that worktree's own, by the path git keeps: not a link to another
    // folder, and not one two entries lead to.
    let raw = read_worktrees(&ctx.runner, &ctx.repo.root).await?;
    let same: Vec<&WorktreeInfo> = raw
        .iter()
        .filter(|w| canonical(Path::new(&w.path)) == Path::new(shown))
        .collect();
    if let Some(link) = same
        .iter()
        .find(|w| fs::symlink_metadata(&w.path).is_ok_and(|m| m.file_type().is_symlink()))
    {
        return Ok(Some(format!(
            "{} is a link to another folder; delete the worktree from a terminal.",
            link.path
        )));
    }
    if same.len() > 1 {
        return Ok(Some(format!(
            "Nothing was deleted: git lists {} entries that lead to {shown}; delete the \
             worktree from a terminal.",
            same.len()
        )));
    }
    if wt.is_main || Path::new(shown) == ctx.repo.root {
        return Ok(Some(format!(
            "Nothing was deleted: {shown} is the main worktree, which holds the repository \
             itself. Only linked worktrees can be deleted."
        )));
    }
    if wt.locked {
        let why = wt
            .lock_reason
            .as_deref()
            .map_or_else(String::new, |r| format!(" ({r})"));
        return Ok(Some(format!(
            "Nothing was deleted: the worktree at {shown} is locked{why}. git keeps a locked \
             worktree; unlock it first (git worktree unlock) if it may go."
        )));
    }
    if wt.prunable || !Path::new(shown).is_dir() {
        return Ok(Some(format!(
            "Nothing was deleted: the folder of the worktree at {shown} is missing, so there \
             is nothing to delete. Prune stale entries to make git forget it."
        )));
    }
    if delete_branch && wt.branch.is_none() {
        return Ok(Some(format!(
            "Nothing was deleted: the worktree at {shown} has no branch checked out (its HEAD \
             is detached), so there is no branch to delete with it."
        )));
    }
    let raw = status_with_ignored(ctx, &wt, true).await?;
    Ok(nested_refusal(&list, &wt, &raw).await)
}

/// Why removing `wt` would take another worktree or repository with it, if it would: one git
/// lists inside it (`list`), or a repository its status read `raw` shows inside it.
/// `git worktree remove` deletes the whole folder, whatever is inside.
async fn nested_refusal(list: &[WorktreeInfo], wt: &WorktreeInfo, raw: &[u8]) -> Option<String> {
    let shown = &wt.path;
    if let Some(inner) = list
        .iter()
        .find(|w| w.path != wt.path && Path::new(&w.path).starts_with(shown))
    {
        return Some(if inner.prunable {
            format!(
                "{shown} lists the worktree at {} inside it, whose folder is gone. Prune stale \
                 entries first.",
                inner.path
            )
        } else {
            format!(
                "{shown} contains the worktree at {}. Delete that one first.",
                inner.path
            )
        });
    }
    let (dir, raw) = (PathBuf::from(shown), raw.to_vec());
    let inside =
        tokio::task::spawn_blocking(move || nested_repository(&dir, &raw, MAX_WALKED, WALK_TIME))
            .await
            .unwrap_or_else(|e| Some(Inside::Unchecked(PathBuf::from(&wt.path), e.to_string())));
    inside.map(|inside| match inside {
        Inside::Repository(inner) => format!(
            "{shown} contains another repository at {}, which git would delete with it. Move \
             or delete it first.",
            inner.display()
        ),
        Inside::Unchecked(folder, why) => format!(
            "Nothing was deleted: gg could not make sure that {} holds no other repository \
             ({why}), and git would delete one with the worktree. Delete that folder first, \
             or delete the worktree from a terminal once you have checked it.",
            folder.display()
        ),
    })
}

/// The output of `status --porcelain=v2 -z --untracked-files=all --ignored=matching` in the
/// worktree `wt`. With `reuse`, the read the operation's turn made of it, if any (the refusal
/// checks and the dangers share one); else (the body, after the switch) a fresh one.
async fn status_with_ignored(ctx: &OpCtx<'_>, wt: &WorktreeInfo, reuse: bool) -> Result<Vec<u8>> {
    let dir = Path::new(&wt.path);
    if reuse
        && let Some((at, raw)) = crate::lock_ignoring_poison(&ctx.held_read).as_ref()
        && at == dir
    {
        return Ok(raw.clone());
    }
    let out = ctx
        .runner
        .run_unchecked(dir, &STATUS_WITH_IGNORED, CmdKind::Read)
        .await?;
    if out.code != 0 {
        return Err(CoreError::new(
            ErrorCode::GitFailed,
            format!(
                "Could not read what the worktree at {} holds, so nothing was deleted.",
                wt.path
            ),
        )
        .with_detail(out.stderr));
    }
    *crate::lock_ignoring_poison(&ctx.held_read) = Some((dir.to_path_buf(), out.stdout.clone()));
    Ok(out.stdout)
}

/// Most entries looked at inside a worktree's ignored folders for another repository, and
/// for how long: beyond either, removal is refused (what was not looked at could hold one).
const MAX_WALKED: usize = 200_000;
const WALK_TIME: std::time::Duration = std::time::Duration::from_secs(5);

/// What looking inside a worktree's untracked and ignored folders found.
#[derive(Debug, PartialEq, Eq)]
enum Inside {
    /// Another repository (a folder with a `.git` file or folder) at this path.
    Repository(PathBuf),
    /// This folder could not be looked through in full (too many entries, too long, or a
    /// folder in it could not be read), so it may hold one.
    Unchecked(PathBuf, String),
}

/// A repository inside the worktree at `dir`, if one is found, from its status read `raw`:
/// an untracked or ignored folder git lists that is one (git does not go into another
/// repository, and stops at one inside an untracked folder), or one at any depth inside an
/// ignored folder (git lists an ignored folder as one entry and does not go in), found by
/// walking it without following links (git deletes a link, not what it points to). The walk
/// is bounded by `max` entries and `time` in all; when it stops short, the folder is
/// [`Inside::Unchecked`].
fn nested_repository(
    dir: &Path,
    raw: &[u8],
    max: usize,
    time: std::time::Duration,
) -> Option<Inside> {
    let is_repo = |p: &Path| fs::symlink_metadata(p.join(".git")).is_ok();
    let deadline = std::time::Instant::now() + time;
    let mut walked = 0;
    let mut fields = raw.split(|&b| b == 0);
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
            return Some(Inside::Repository(folder));
        }
        if !ignored {
            continue;
        }
        let mut pending = vec![folder.clone()];
        while let Some(at) = pending.pop() {
            let entries = match fs::read_dir(&at) {
                Ok(entries) => entries,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => {
                    return Some(Inside::Unchecked(
                        folder,
                        format!("{} could not be read ({e})", at.display()),
                    ));
                }
            };
            for child in entries {
                let Ok(child) = child else {
                    return Some(Inside::Unchecked(
                        folder,
                        format!("{} could not be read in full", at.display()),
                    ));
                };
                walked += 1;
                if walked > max {
                    return Some(Inside::Unchecked(
                        folder,
                        format!("there are more than {max} entries in its ignored folders"),
                    ));
                }
                if walked % 1024 == 0 && std::time::Instant::now() > deadline {
                    return Some(Inside::Unchecked(
                        folder,
                        format!(
                            "looking through its ignored folders took longer than {} seconds",
                            time.as_secs()
                        ),
                    ));
                }
                // No link is followed: `file_type` does not.
                if child.file_type().is_ok_and(|t| t.is_dir()) {
                    let path = child.path();
                    if is_repo(&path) {
                        return Some(Inside::Repository(path));
                    }
                    pending.push(path);
                }
            }
        }
    }
    None
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
    /// Tracked paths with changes, staged or not (conflicted ones too), and edited files
    /// marked assume-unchanged or skip-worktree ([`marked_changes`]).
    changed: Vec<String>,
    /// Those of `changed` that git's status does not show, with the mark that hides them.
    marked: Vec<(String, &'static str)>,
    /// Untracked files, one by one.
    untracked: Vec<String>,
    /// What git ignores there, as shown: top-level entries only (`build/`, `x.log`, or
    /// `newdir/ (2 ignored inside)` for ignored entries inside a folder that is not).
    ignored: Vec<String>,
    /// What git ignores there, as git lists it (folders it ignores whole collapsed).
    ignored_entries: Vec<String>,
    /// Hashes of the status records (all but the ignored) and of the ignored ones.
    status_hash: u64,
    ignored_hash: u64,
    /// Detached only: the commits lost with it, on no branch, tag or remote. The first
    /// [`MAX_LOST_LISTED`] as `hash subject`, how many, and a hash of the whole list.
    commits: Vec<String>,
    commit_count: usize,
    commits_hash: u64,
    /// Detached only: git could not list them (final review M3), so they may be lost.
    commits_unknown: bool,
    /// On a branch: its commits on no remote, which the branch keeps (removing a worktree
    /// never deletes its branch), said in a line, never a danger of their own.
    unpushed: usize,
    in_progress: Option<InProgressKind>,
    /// git's status output the files were read from.
    raw: Vec<u8>,
}

impl Work {
    /// Anything beside ignored files and commits its branch keeps: the red danger.
    fn holds_work(&self) -> bool {
        !self.changed.is_empty()
            || !self.untracked.is_empty()
            || self.commit_count > 0
            || self.commits_unknown
            || self.in_progress.is_some()
    }
}

/// Reads what the worktree `wt` holds: what git is stopped in there, its commits (detached:
/// those on no branch, tag or remote, lost with it; on a branch: how many are on no remote),
/// counted from the main worktree, and, read last, its files ([`read_files_held`]). Stashes are shared by all worktrees and not lost
/// with one.
async fn read_work(ctx: &OpCtx<'_>, wt: &WorktreeInfo, reuse: bool) -> Result<Work> {
    let dir = Path::new(&wt.path);
    let in_progress = read_kind(&ctx.runner, dir).await?;
    let (mut commit_count, mut commits_hash, mut commits) = (0, 0, Vec::new());
    let (mut unpushed, mut commits_unknown) = (0, false);
    if let (Some(head), Some(_)) = (&wt.head, &wt.branch) {
        // Only a line (its branch keeps them, P3-R13): not said when git cannot count them.
        let out = ctx
            .read(&["rev-list", "--count", head, "--not", "--remotes"])
            .await?;
        if out.code == 0 {
            unpushed = out.text().trim().parse().unwrap_or(0);
        }
    } else if let Some(head) = &wt.head {
        let only_here = [head.as_str(), "--not", "--branches", "--tags", "--remotes"];
        let mut list = vec!["rev-list"];
        list.extend(&only_here);
        let out = ctx.read(&list).await?;
        if out.code == 0 {
            commit_count = out.text().lines().filter(|l| !l.is_empty()).count();
            commits_hash = fnv1a(&out.stdout);
        } else {
            // Not "none": they may be lost (final review M3).
            commits_unknown = true;
            commits_hash = fnv1a(b"unknown");
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
        commits_unknown,
        ..read_files_held(ctx, wt, reuse).await?
    })
}

/// The files the worktree `wt` holds, from `status --porcelain=v2 -z --untracked-files=all
/// --ignored=matching` there (ignored folders come collapsed): changed, untracked and
/// ignored, with the hashes of the records. With `reuse`, from the turn's read (see
/// [`status_with_ignored`]).
async fn read_files_held(ctx: &OpCtx<'_>, wt: &WorktreeInfo, reuse: bool) -> Result<Work> {
    let dir = Path::new(&wt.path);
    let raw = status_with_ignored(ctx, wt, reuse).await?;
    let mut work = parse_status(&raw);
    work.raw = raw;
    // Edits git's status does not show (final review C1): in the list, the token and the
    // re-check before removing, as any other change.
    work.marked = marked_changes(ctx, wt).await?;
    let mut names = Vec::new();
    for (path, why) in &work.marked {
        work.changed.push(path.clone());
        names.extend_from_slice(path.as_bytes());
        names.push(0);
        names.extend_from_slice(why.as_bytes());
        names.push(0);
    }
    // Sizes and times too, so an edit after the dialog asks again.
    let files = work
        .changed
        .iter()
        .chain(&work.untracked)
        .map(String::as_str);
    let ignored = work.ignored_entries.iter().map(|p| p.trim_end_matches('/'));
    work.status_hash = fnv1a(
        &[
            &work.status_hash.to_le_bytes()[..],
            &stat_hash(dir, files).to_le_bytes(),
            &names,
        ]
        .concat(),
    );
    work.ignored_hash = fnv1a(
        &[
            work.ignored_hash.to_le_bytes(),
            stat_hash(dir, ignored).to_le_bytes(),
        ]
        .concat(),
    );
    Ok(work)
}

/// Most marked files hashed in one `hash-object` (its arguments stay short).
const HASHED_AT_ONCE: usize = 100;

/// The files of the worktree `wt` marked assume-unchanged or skip-worktree (`ls-files -v`:
/// a lowercase tag, or `S`) that differ from the index, with the mark: git's `status`, and
/// git's own check before a plain `worktree remove`, skip them, so edits to them would go
/// without a word. One that is not on disk is no danger (a sparse checkout leaves
/// skip-worktree files out; a deleted file loses nothing). One whose content git cannot
/// compare (a link, a folder, a name that is not UTF-8, a failed hash) counts as changed.
async fn marked_changes(ctx: &OpCtx<'_>, wt: &WorktreeInfo) -> Result<Vec<(String, &'static str)>> {
    let dir = Path::new(&wt.path);
    let out = ctx
        .runner
        .run_unchecked(dir, &["ls-files", "-v", "-s", "-z"], CmdKind::Read)
        .await?;
    if out.code != 0 {
        return Err(CoreError::new(
            ErrorCode::GitFailed,
            format!(
                "Could not read what the worktree at {} holds, so nothing was deleted.",
                wt.path
            ),
        )
        .with_detail(out.stderr));
    }
    let mut changed = Vec::new();
    let mut to_hash: Vec<(String, String, &'static str)> = Vec::new();
    for entry in out.stdout.split(|&b| b == 0) {
        // `<tag> <mode> <object> <stage>\t<path>`
        let Some(tab) = entry.iter().position(|&b| b == b'\t') else {
            continue;
        };
        let meta = String::from_utf8_lossy(&entry[..tab]);
        let mut fields = meta.split(' ');
        let (Some(tag), Some(mode), Some(object)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let why = match tag {
            "S" | "s" => "skip-worktree",
            t if t.chars().all(|c| c.is_ascii_lowercase()) => "assume-unchanged",
            _ => continue,
        };
        if mode == "160000" {
            // A submodule: its own repository, not a file.
            continue;
        }
        let Ok(path) = std::str::from_utf8(&entry[tab + 1..]) else {
            changed.push((String::from_utf8_lossy(&entry[tab + 1..]).into_owned(), why));
            continue;
        };
        match fs::symlink_metadata(dir.join(path)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Ok(meta) if meta.is_file() && mode != "120000" => {
                to_hash.push((path.to_owned(), object.to_owned(), why));
            }
            _ => changed.push((path.to_owned(), why)),
        }
    }
    for run in to_hash.chunks(HASHED_AT_ONCE) {
        let mut args = vec!["hash-object", "--"];
        args.extend(run.iter().map(|(path, _, _)| path.as_str()));
        let out = ctx.runner.run_unchecked(dir, &args, CmdKind::Read).await?;
        let text = out.text();
        let hashes: Vec<&str> = text.lines().collect();
        let compared = out.code == 0 && hashes.len() == run.len();
        for (i, (path, object, why)) in run.iter().enumerate() {
            if !compared || hashes[i].trim() != object {
                changed.push((path.clone(), *why));
            }
        }
    }
    Ok(changed)
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
                work.ignored_entries.extend(path(1));
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
    work.ignored = top_level(&work.ignored_entries);
    work
}

/// Ignored entries `entries` (as git lists them) by their top-level folder: an entry inside
/// a folder git does not ignore whole is counted under it (`newdir/ (2 ignored inside)`).
fn top_level(entries: &[String]) -> Vec<String> {
    let mut tops: Vec<(String, usize)> = Vec::new();
    for entry in entries {
        let (top, inside) = match entry.split_once('/') {
            Some((top, rest)) if !rest.is_empty() => (format!("{top}/"), 1),
            _ => (entry.clone(), 0),
        };
        match tops.iter_mut().find(|(t, _)| *t == top) {
            Some((_, n)) => *n += inside,
            None => tops.push((top, inside)),
        }
    }
    tops.into_iter()
        .map(|(top, n)| match n {
            0 => top,
            n => format!("{top} ({n} ignored inside)"),
        })
        .collect()
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
        match (out.code == 0)
            .then(|| out.text().trim().parse::<u64>().ok())
            .flatten()
        {
            None => danger.message.push_str(
                " gg could not count how many of its commits are on no other branch, tag or \
                 remote.",
            ),
            Some(0) => {}
            Some(1) => danger.message.push_str(
                " 1 of its commits is not pushed anywhere (it is on no other branch, tag or \
                 remote).",
            ),
            Some(n) => danger.message.push_str(&format!(
                " {n} of its commits are not pushed anywhere (they are on no other branch, tag \
                 or remote)."
            )),
        }
        dangers.push(danger);
    }
    // The files last, so the dialog shows them as late as can be.
    if !has_submodules(ctx, &wt).await? {
        let work = read_work(ctx, &wt, true).await?;
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
    // A top-level file has no `/`; a folder ends with one, or has its count after it.
    let folders = ignored.iter().filter(|p| p.contains('/')).count();
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
/// its own danger says what is lost). Its token is the worktree's path and HEAD, what is in progress
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
        "{}\n{}\n{}\n{:016x}\n{:016x}\n{:016x}",
        wt.path,
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
            .push_str(" Deleting it loses the uncommitted changes and untracked files for good."),
        (false, true) => message.push_str(" Deleting it loses the uncommitted changes for good."),
        (true, false) => message.push_str(" Deleting it loses the untracked files for good."),
        (true, true) => {}
    }
    if work.commits_unknown {
        message.push_str(
            " gg could not tell whether commits on no branch would be lost with it (git could \
             not list them); count on losing any that only this worktree has.",
        );
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
            " A {what} is in progress there; deleting the worktree abandons it."
        ));
    }
    if !work.ignored.is_empty() {
        message.push_str(&format!(
            " It also holds {}, which git deletes with it (listed below).",
            ignored_count(&work.ignored)
        ));
    }
    message.push_str(&kept);
    let changed: Vec<String> = work
        .changed
        .iter()
        .map(|path| match work.marked.iter().find(|(p, _)| p == path) {
            Some((_, why)) => format!("{path} (marked {why}: git status does not show it)"),
            None => path.clone(),
        })
        .collect();
    section(&mut detail, "Uncommitted changes", &changed, changed.len());
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
/// active worktree is left first: the main one becomes active. Then all of it is read again
/// (HEAD, what is in progress, the commits lost with it, its files) and must be as confirmed
/// (the danger's whole token), or still need no confirmation when none was asked, else
/// nothing is removed. Then, with `delete_branch`, its branch goes (`branch -d`, or `-D` when
/// `delete_unmerged_branch_with_worktree` was confirmed), unless another worktree has it
/// checked out.
pub(crate) async fn remove(
    ctx: &OpCtx<'_>,
    dangers: &[Danger],
    path: &str,
    delete_branch: bool,
) -> Result<Step> {
    let (Some(wt), _) = find(ctx, path).await? else {
        return Ok(Step::Failed {
            message: format!(
                "Nothing was deleted: {path} is no longer a worktree of this repository."
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
    let confirmed = dangers.iter().find(|d| {
        matches!(
            d.kind,
            DangerKind::RemoveWorktreeWithWork | DangerKind::RemoveWorktreeIgnoredFiles
        )
    });
    let refuse = |why: String| {
        let mut message = why;
        if left {
            message.push_str(&main_now);
        }
        Ok(Step::Failed {
            message,
            detail: String::new(),
            hints: Vec::new(),
        })
    };
    // Read all of it again now (after the switch, for the active one), as the turn did: what
    // is removed must be what was confirmed (final review M1: HEAD, what is in progress and
    // the commits lost too, not only the files).
    let (Some(now_wt), list) = find(ctx, path).await? else {
        return refuse(format!(
            "Nothing was deleted: {path} is no longer a worktree of this repository."
        ));
    };
    let now = read_work(ctx, &now_wt, false).await?;
    // Never another worktree or repository with it, as it is now.
    if let Some(why) = nested_refusal(&list, &now_wt, &now.raw).await {
        return refuse(why);
    }
    // Another commit or branch checked out there since the body began: never what was seen.
    let moved = now_wt.head != wt.head || now_wt.branch != wt.branch;
    let changed = moved
        || match (confirmed, work_danger(&now, &now_wt, delete_branch)) {
            (Some(seen), Some(danger)) => seen.kind != danger.kind || seen.token != danger.token,
            (Some(_), None) => true,
            // Nothing was confirmed: it held nothing to lose, and must hold nothing now, on the
            // same HEAD. (With submodules nothing is asked: git refuses the plain remove itself,
            // and says why.)
            (None, Some(_)) => !has_submodules(ctx, &now_wt).await?,
            (None, None) => false,
        };
    if changed {
        return refuse(if confirmed.is_some() {
            "It changed since you confirmed; nothing was deleted.".to_owned()
        } else {
            "It changed while gg was getting ready; nothing was deleted.".to_owned()
        });
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
    let mut message = format!("Deleted the worktree at {shown}.");
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
        } => Step::DoneWith {
            // The worktree is gone all the same: done, with what became of the branch.
            message: format!("{message} Branch \u{201c}{branch}\u{201d} was not deleted: {why}"),
            detail: Some(detail),
            hints,
        },
        other => other,
    })
}

/// Said with every prune confirmation.
/// Said with a prune: a folder that is only out of reach for now.
fn disconnected(n: usize) -> &'static str {
    if n == 1 {
        "If it is on a disconnected drive, reconnect it or lock the worktree instead."
    } else {
        "If one of these is on a disconnected drive, reconnect it or lock the worktree instead."
    }
}

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
    let mut unknown = 0;
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
            let out = ctx.read(&count_args).await?;
            let Some(n) = (out.code == 0)
                .then(|| out.text().trim().parse::<usize>().ok())
                .flatten()
            else {
                // Not "none": they may be lost (final review M3).
                unknown += 1;
                token.push_str("unknown\n");
                continue;
            };
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
    let them = if stale.len() == 1 {
        "1 worktree entry whose folder is gone".to_owned()
    } else {
        format!("{} worktree entries whose folders are gone", stale.len())
    };
    let disconnected = disconnected(stale.len());
    let unknown_line = match unknown {
        0 => String::new(),
        1 => " gg could not tell whether commits on no branch would be lost with one of them \
               (git could not list them); count on losing any that only it has."
            .to_owned(),
        n => format!(
            " gg could not tell whether commits on no branch would be lost with {n} of them \
             (git could not list them); count on losing any that only they have."
        ),
    };
    let (kind, message) = if lost_count == 0 && unknown > 0 {
        (
            DangerKind::PruneWorktreesLosesCommits,
            format!("Pruning makes git forget {them} (listed below).{unknown_line} {disconnected}"),
        )
    } else if lost_count == 0 {
        (
            DangerKind::PruneWorktrees,
            format!("Pruning makes git forget {them} (listed below). {disconnected}"),
        )
    } else {
        (
            DangerKind::PruneWorktreesLosesCommits,
            format!(
                "Pruning makes git forget {them} (listed below), and with {} {} on no branch, \
                 tag or remote, which {} lost.{unknown_line} {disconnected}",
                if stale.len() == 1 { "it" } else { "them" },
                count(lost_count, "commit", "commits"),
                if lost_count == 1 { "is" } else { "are" }
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
            "Pruned {}: {}.",
            if pruned.len() == 1 {
                "1 stale worktree entry, whose folder was gone".to_owned()
            } else {
                format!(
                    "{} stale worktree entries, whose folders were gone",
                    pruned.len()
                )
            },
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
    /// worktree git lists). `exists`: that folder is there (empty). A pattern that lands
    /// inside a worktree or the git dir is not used: the default one is.
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
        let slug = branch_slug(branch);
        let place = |pattern: &str| {
            resolve(
                &repo.root,
                &pattern.replace("{branch}", &slug).replace("{repo}", &name),
            )
        };
        // Never inside a worktree or the git dir: then the default pattern instead.
        let git_dir = canonical(&repo.common_dir);
        let inside = |path: &Path| {
            let path = real(path);
            path.starts_with(&git_dir)
                || listed
                    .iter()
                    .any(|w| path.starts_with(&w.path) && path != Path::new(&w.path))
        };
        let mut base = place(pattern);
        if inside(&base) {
            base = place(DEFAULT_WORKTREE_PATTERN);
        }
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
        // Ignored entries inside a folder git does not ignore whole: that folder, counted.
        let nested = parse_status(b"! newdir/i.log\0! newdir/sub/j.log\0! newdir/out/\0! z.log\0");
        assert_eq!(nested.ignored, ["newdir/ (3 ignored inside)", "z.log"]);
        assert_eq!(nested.ignored_entries.len(), 4);
        // Only the ignored part changes the ignored hash, and the rest the status hash.
        let more = [&out[..], b"! more.log\0"].concat();
        let again = parse_status(&more);
        assert_eq!(again.status_hash, work.status_hash);
        assert_ne!(again.ignored_hash, work.ignored_hash);
    }

    /// A worktree folder with an ignored `build/` holding `n` empty files and a repository
    /// at `build/deep/x/y/repo`.
    fn ignored_tree(n: usize) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let build = dir.path().join("build");
        fs::create_dir_all(build.join("deep/x/y/repo/.git")).unwrap();
        for i in 0..n {
            fs::write(build.join(format!("f{i}")), "").unwrap();
        }
        let repo = build.join("deep/x/y/repo");
        (dir, repo)
    }

    #[test]
    fn a_repository_at_any_depth_in_an_ignored_folder_is_found() {
        let (dir, repo) = ignored_tree(3);
        let time = std::time::Duration::from_secs(5);
        assert_eq!(
            nested_repository(dir.path(), b"! build/\0", 100, time),
            Some(Inside::Repository(repo))
        );
        // An ignored file is not walked into, nor an untracked folder git went through.
        assert_eq!(
            nested_repository(dir.path(), b"! x.log\0? build/f0\0", 100, time),
            None
        );
    }

    #[test]
    fn an_ignored_folder_too_big_to_look_through_is_unchecked() {
        let (dir, _repo) = ignored_tree(20);
        let time = std::time::Duration::from_secs(5);
        // 20 files and 4 folders: 24 entries. At the bound it still looks; one under it, not.
        assert!(matches!(
            nested_repository(dir.path(), b"! build/\0", 24, time),
            Some(Inside::Repository(_))
        ));
        match nested_repository(dir.path(), b"! build/\0", 10, time) {
            Some(Inside::Unchecked(folder, why)) => {
                assert_eq!(folder, dir.path().join("build"));
                assert_eq!(why, "there are more than 10 entries in its ignored folders");
            }
            other => panic!("{other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_inside_an_ignored_folder_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        fs::create_dir_all(elsewhere.path().join("repo/.git")).unwrap();
        fs::create_dir(dir.path().join("build")).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), dir.path().join("build/link")).unwrap();
        let time = std::time::Duration::from_secs(5);
        assert_eq!(
            nested_repository(dir.path(), b"! build/\0", 100, time),
            None
        );
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
