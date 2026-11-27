//! Committing the files the user ticked (`commit`), optionally pushing after, and the list
//! of changed files the commit panel shows (`worktree_files`).
//!
//! A commit of chosen files never starts from the user's staging: its tree is built in a
//! temporary index beside the user's (`GIT_INDEX_FILE`), made from HEAD with the user's
//! index's stat data (`read-tree --reset --index-output`, so git hashes only what changes;
//! nothing on an unborn branch), then given exactly the ticked paths as they are in the
//! working tree (`update-index` with each path as it is: no pathspec matching, so the
//! deletion of a file `dir` never takes what is now under a folder `dir`). So a file staged
//! but not ticked is not committed, and a failure leaves the user's index as it was. Hooks
//! run as with `git commit <paths>`, and see the temporary index. Once the commit is made,
//! the paths it changed (a file a hook added included), and only they, are set to the new
//! HEAD in the user's index, so they no longer show as changed while other staged changes
//! stay staged. The temporary index and files are always removed, also when the commit
//! fails or is stopped.
//!
//! git allows no partial commit while a merge is in progress: a merge with no conflicts
//! left is concluded with all its files, in the user's index. That index is saved first and
//! put back when no commit is made (a hook refused it), so a failure leaves it as it was
//! there too. What is in progress is the turn's read: one that changed
//! since (a merge started in a terminal) stops the commit.

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
use crate::notice::{as_sentence, name, note_of, path, said, said_const, telling_line, title_of};
use crate::ops::explain::{file_list, recognised};
use crate::ops::local::{current_branch, head_commit, kind_name, short};
use crate::ops::{OpCtx, Step, network};
use crate::repo::{RepoManager, canonical, canonical_paths};

/// Said when a merge is to be committed with only some of its changed files.
const MERGE_NEEDS_ALL: &str = "A merge is concluded with all its changed files (new files git \
                               does not track yet may stay out).";

/// Said when git does not know the user's name or email.
const NO_IDENTITY: &str = said_const!(
    "Nothing was committed: git does not know who you are",
    "Set your name and email (git config --global user.name \"Your Name\", and user.email), \
     then commit again."
);

/// Said when the signing program failed: without a terminal, a passphrase prompt
/// fails at once.
const SIGNING_FAILED: &str = said_const!(
    "Nothing was committed: the commit could not be signed",
    "gg runs git without a terminal, so a signing program that asks for a passphrase there \
     fails at once. Use an agent or a graphical passphrase prompt (see \u{201c}Actions\u{201d} \
     in the README), or commit in a terminal."
);

/// Said when the message is empty once git has removed its comment lines.
const EMPTY_MESSAGE: &str = said_const!(
    "Nothing was committed: the message is empty",
    "It is, once git removes its comment lines (lines starting with #)."
);

/// Said when git, or a hook, refused the commit for a reason the core does not recognise,
/// with the line of its output that says most (`said`), if any.
fn refused_commit(said_line: Option<String>) -> String {
    let title = "Nothing was committed: git or a hook refused the commit";
    match said_line {
        Some(line) => said(title, line),
        None => said(
            title,
            "A hook (pre-commit, commit-msg) or git said why: see the log.",
        ),
    }
}

/// Most paths given to one command, so the command line stays short on any platform.
const CHUNK: usize = 100;

/// Most bytes of paths (and what goes with each) given to one command: long names fill a
/// command line before `CHUNK` of them do.
const ARG_BUDGET: usize = 8 * 1024;

/// `items` in runs of at most [`CHUNK`] items and [`ARG_BUDGET`] bytes as `cost` counts them
/// (an item over the budget goes alone).
fn chunks_by<T>(items: &[T], cost: impl Fn(&T) -> usize) -> Vec<&[T]> {
    let mut out = Vec::new();
    let (mut start, mut bytes) = (0, 0);
    for (i, item) in items.iter().enumerate() {
        let c = cost(item);
        if i > start && (i - start == CHUNK || bytes + c > ARG_BUDGET) {
            out.push(&items[start..i]);
            (start, bytes) = (i, 0);
        }
        bytes += c;
    }
    if start < items.len() {
        out.push(&items[start..]);
    }
    out
}

/// `paths` in runs that fit one command line (see [`chunks_by`]).
pub(crate) fn path_chunks<'a>(paths: &'a [&'a str]) -> Vec<&'a [&'a str]> {
    chunks_by(paths, |p| p.len() + 1)
}

/// "1 file", "3 files".
pub(crate) fn files(n: usize) -> String {
    if n == 1 {
        "1 file".to_owned()
    } else {
        format!("{n} files")
    }
}

/// Checks a commit request without git: some path (none for an `empty` commit), a message
/// that is not blank, and paths that name something inside the worktree (relative, no `.`
/// or `..`, no NUL).
pub(crate) fn check(paths: &[String], message: &str, empty: bool) -> Result<()> {
    if empty && !paths.is_empty() {
        return Err(CoreError::new(
            ErrorCode::BadRequest,
            "An empty commit takes no files: untick them, or commit them.",
        ));
    }
    if paths.is_empty() && !empty {
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

/// Whether `entry` is ticked in `ticked`: its path, or for a rename either side (a copy's
/// source is not changed by it, and does not tick it).
fn is_ticked(entry: &FileEntry, ticked: &BTreeSet<&str>) -> bool {
    ticked.contains(entry.path.as_str())
        || (entry.status == EntryStatus::R
            && entry
                .orig_path
                .as_deref()
                .is_some_and(|orig| ticked.contains(orig)))
}

/// The folder above `path` in the worktree `cwd` that is a symbolic link now, if any (git
/// does not look beyond one).
fn link_above(cwd: &Path, path: &str) -> Option<String> {
    let mut at = PathBuf::new();
    let parts: Vec<&str> = path.split('/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        at.push(part);
        if fs::symlink_metadata(cwd.join(&at)).is_ok_and(|m| m.file_type().is_symlink()) {
            return Some(at.to_string_lossy().into_owned());
        }
    }
    None
}

/// Why committing the ticked entries of `entries` exactly cannot work, if it cannot: a file
/// that takes the place of a folder removes the folder's files from the commit too, and a
/// file in a folder that takes the place of a file removes that file, so those must be
/// ticked as well; and git does not look beyond a folder that is a link now, except to
/// record that what was there is gone.
fn transition_refused(
    cwd: &Path,
    entries: &[FileEntry],
    ticked: &BTreeSet<&str>,
) -> Option<String> {
    let mine: Vec<&FileEntry> = entries.iter().filter(|e| is_ticked(e, ticked)).collect();
    let deleted_unticked: Vec<&FileEntry> = entries
        .iter()
        .filter(|e| e.status == EntryStatus::D && !is_ticked(e, ticked))
        .collect();
    for entry in &mine {
        if entry.status == EntryStatus::D {
            continue;
        }
        let file = entry.path.as_str();
        if let Some(link) = link_above(cwd, file) {
            return Some(said(
                "Nothing was committed",
                format!(
                    "{} is inside {}, which is a link to another folder now; git does not look \
                     inside it.",
                    path(file),
                    path(&link)
                ),
            ));
        }
        let path = file;
        let is_file = fs::symlink_metadata(cwd.join(path)).is_ok_and(|m| !m.is_dir());
        let under = format!("{path}/");
        let below = deleted_unticked
            .iter()
            .filter(|d| d.path.starts_with(&under))
            .count();
        if is_file && below > 0 {
            return Some(format!(
                "Committing {path} also removes {} under it; tick them too.",
                files(below)
            ));
        }
        if let Some(file) = deleted_unticked
            .iter()
            .find(|d| path.starts_with(&format!("{}/", d.path)))
        {
            return Some(format!(
                "Committing {path} also removes the file {}, whose place its folder takes; tick \
                 it too.",
                file.path
            ));
        }
    }
    None
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
/// and a commit of less than every changed file, and an `empty` one. A push needs a branch
/// (`head`) and a remote to push it to.
pub(crate) async fn refused(
    ctx: &OpCtx<'_>,
    paths: &[String],
    push: bool,
    empty: bool,
    in_progress: Option<InProgressKind>,
    head: Option<&str>,
) -> Result<Option<String>> {
    if empty && in_progress == Some(InProgressKind::Merge) {
        return Ok(Some(
            "Nothing was committed: a merge is in progress, and an empty commit cannot \
             conclude it. Commit the merge's files, or abort it."
                .to_owned(),
        ));
    }
    let entries = changed_files(ctx).await?;
    let ticked: BTreeSet<&str> = paths.iter().map(String::as_str).collect();
    if let Some(odd) = entries
        .iter()
        .find(|e| !e.committable && is_ticked(e, &ticked))
    {
        return Ok(Some(format!(
            "Nothing was committed: the name of {} is not valid UTF-8, which gg cannot hand \
             back to git exactly; commit it in a terminal.",
            odd.path
        )));
    }
    if in_progress == Some(InProgressKind::Merge) {
        let left = unmerged(&entries, None);
        if !left.is_empty() {
            let named: Vec<String> = left.iter().map(path).collect();
            let named: Vec<&str> = named.iter().map(String::as_str).collect();
            return Ok(Some(said(
                format!("The merge still has conflicts in {}", file_list(&named)),
                "Resolve them, then commit.",
            )));
        }
        // Every tracked change: git commits the whole index then.
        if !entries
            .iter()
            .filter(|e| e.status != EntryStatus::Untracked)
            .all(|e| is_ticked(e, &ticked))
        {
            return Ok(Some(MERGE_NEEDS_ALL.to_owned()));
        }
    } else {
        let conflicted = unmerged(&entries, Some(&ticked));
        if !conflicted.is_empty() {
            let verb = if conflicted.len() == 1 { "has" } else { "have" };
            let named: Vec<String> = conflicted.iter().map(path).collect();
            let named: Vec<&str> = named.iter().map(String::as_str).collect();
            return Ok(Some(said(
                "Nothing was committed",
                format!(
                    "{} still {verb} conflicts. Resolve them first: a file with conflicts cannot \
                     be committed.",
                    file_list(&named)
                ),
            )));
        }
    }
    if let Some(why) = transition_refused(&ctx.cwd, &entries, &ticked) {
        return Ok(Some(why));
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

/// The remote a commit's push goes to (the branch of the same name there): where git
/// pushes `branch` (its `pushRemote`, `remote.pushDefault`, its upstream's remote, origin,
/// the only one), the rule `pr_create` uses too.
async fn push_remote(ctx: &OpCtx<'_>, branch: &str) -> Result<Option<String>> {
    branch_push_remote(&ctx.runner, &ctx.cwd, branch).await
}

/// Where git pushes `branch` from worktree `cwd` (see [`push_remote`]).
async fn branch_push_remote(
    runner: &GitRunner,
    cwd: &Path,
    branch: &str,
) -> Result<Option<String>> {
    let facts = crate::gh::RemoteFacts::read(runner, cwd).await?;
    Ok(facts.push_remote(branch).map(str::to_owned))
}

/// A folder of the commit's temporary files (its index, the list of paths, the message)
/// beside the worktree's own index, so on the same file system; removed with everything in
/// it when dropped, however the commit ended.
struct Scratch {
    dir: PathBuf,
    /// The user's own index (git's `--git-path index`).
    real_index: PathBuf,
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
        remove_left_over(beside);
        let base = format!("gg-commit-{}-{}", std::process::id(), ctx.op_id);
        for n in 0..100 {
            let dir = if n == 0 {
                beside.join(&base)
            } else {
                beside.join(format!("{base}-{n}"))
            };
            match fs::create_dir(&dir) {
                Ok(()) => {
                    return Ok(Self {
                        dir,
                        real_index: index,
                    });
                }
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

    /// The user's own index as it is now, to put back with [`Scratch::put_back`].
    fn save_index(&self) -> Result<Vec<u8>> {
        fs::read(&self.real_index).map_err(|e| {
            CoreError::new(
                ErrorCode::Io,
                format!("Could not read {}.", self.real_index.display()),
            )
            .with_detail(e.to_string())
        })
    }

    /// Puts `saved` back as the user's index, through its lock file as git does: never over
    /// a lock another program holds. Said in `step`'s message when it cannot.
    fn put_back(&self, saved: Option<&[u8]>, step: Step) -> Step {
        let Some(saved) = saved else {
            return step;
        };
        let Err(e) = restore_index(&self.real_index, saved) else {
            return step;
        };
        match step {
            Step::Failed {
                message,
                detail,
                hints,
            } => Step::Failed {
                message: crate::notice::add_note(&message, &not_put_back(&e)),
                detail,
                hints,
            },
            other => other,
        }
    }

    /// Writes `message` as it is; returns the file's path, for `-F`.
    fn message(&self, message: &str) -> Result<String> {
        let file = self.dir.join("message");
        fs::write(&file, message).map_err(|e| io_error(&file, &e))?;
        Ok(file.display().to_string())
    }
}

/// Removes the temporary folders in `dir` that a gg process which is gone left (it crashed
/// mid-commit): `gg-commit-<pid>-…` of a process no longer running (where that
/// cannot be told, not Unix, none), and any older than a day. Failures are ignored: the
/// folders are only clutter.
fn remove_left_over(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("gg-commit-"))
            .and_then(|rest| rest.split('-').next())
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        let path = entry.path();
        if pid != std::process::id()
            && (crate::platform::process_gone(pid) || older_than_a_day(&path))
        {
            let _ = fs::remove_dir_all(path);
        }
    }
}

/// Whether folder `made` was last changed more than a day ago: no commit takes that long,
/// whatever process made it (its pid may be another program's by now).
fn older_than_a_day(made: &Path) -> bool {
    fs::metadata(made)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age > std::time::Duration::from_secs(24 * 3600))
}

/// Said when the staging area could not be put back after a refused merge conclusion.
fn not_put_back(e: &io::Error) -> String {
    format!(
        "The staging area could not be put back as it was ({e}): the ticked files stay staged \
         for the merge."
    )
}

/// Writes `saved` as the index at `index`: into `index.lock` (made new, so never one another
/// program holds), then renamed over it.
fn restore_index(index: &Path, saved: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut lock_name = index.as_os_str().to_owned();
    lock_name.push(".lock");
    let lock = PathBuf::from(lock_name);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock)?;
    let written = file.write_all(saved).and_then(|()| file.sync_all());
    drop(file);
    if let Err(e) = written.and_then(|()| fs::rename(&lock, index)) {
        let _ = fs::remove_file(&lock);
        return Err(e);
    }
    Ok(())
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
        said(
            "Nothing was committed",
            telling_line(&detail).unwrap_or_else(|| "git failed.".to_owned()),
        )
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
        recognised(args, &detail).unwrap_or_else(|| refused_commit(telling_line(&detail)))
    };
    Ok(Step::Failed {
        message,
        detail,
        hints: Vec::new(),
    })
}

/// Runs `update-index` with `options`, then `--`, then `paths`, a chunk at a time, in the
/// index `runner` uses. The first failure is returned as it is.
async fn update_index(
    ctx: &OpCtx<'_>,
    runner: &GitRunner,
    options: &[&str],
    paths: &[&str],
) -> Result<Option<(Vec<String>, GitOutput)>> {
    for chunk in path_chunks(paths) {
        let mut args = vec!["update-index"];
        args.extend_from_slice(options);
        args.push("--");
        args.extend_from_slice(chunk);
        let out = runner
            .run_unchecked(&ctx.cwd, &args, CmdKind::Write)
            .await?;
        if out.code != 0 {
            return Ok(Some((args.iter().map(|a| (*a).to_owned()).collect(), out)));
        }
    }
    Ok(None)
}

/// Puts exactly the paths of `entries` (the ticked ones) as the working tree has them into
/// the index `runner` uses, each by its own name: what is gone (or is a folder now) leaves
/// it first, then what is there is added. A rename's old name leaves by name (`--force-remove`:
/// on a file system that ignores case, `Readme.md` is still "there" as `README.md`), and so
/// does a deleted file below a folder that is a link now (git looks no further than a link).
/// The failed step, if one fails.
async fn stage(
    ctx: &OpCtx<'_>,
    runner: &GitRunner,
    entries: &[&FileEntry],
) -> Result<Option<Step>> {
    let mut forced: BTreeSet<&str> = BTreeSet::new();
    for entry in entries {
        if entry.status == EntryStatus::R
            && let Some(orig) = &entry.orig_path
        {
            forced.insert(orig);
        }
        if entry.status == EntryStatus::D && link_above(&ctx.cwd, &entry.path).is_some() {
            forced.insert(&entry.path);
        }
    }
    let forced: Vec<&str> = forced.into_iter().collect();
    if let Some((args, out)) = update_index(ctx, runner, &["--force-remove"], &forced).await? {
        return Ok(Some(step_failed(
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            &out,
        )));
    }
    let (mut gone, mut there) = (Vec::new(), Vec::new());
    for entry in entries {
        let path = entry.path.as_str();
        if forced.contains(&path) {
            continue;
        }
        match fs::symlink_metadata(ctx.cwd.join(path)) {
            Ok(meta) if !meta.is_dir() => there.push(path),
            _ => gone.push(path),
        }
    }
    gone.extend(there);
    if let Some((args, out)) = update_index(ctx, runner, &["--add", "--remove"], &gone).await? {
        return Ok(Some(step_failed(
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            &out,
        )));
    }
    Ok(None)
}

/// Of the deleted `entries`, those HEAD (`parent`) does not have: added to the index, then
/// deleted from the working tree. A commit records nothing for them; their stale index
/// entries are dropped from the user's index.
async fn stale_entries(
    ctx: &OpCtx<'_>,
    parent: Option<&str>,
    entries: &[&FileEntry],
) -> Result<Vec<String>> {
    let candidates: Vec<&str> = entries
        .iter()
        .filter(|e| e.status == EntryStatus::D && e.staged)
        .map(|e| e.path.as_str())
        .collect();
    let Some(parent) = parent else {
        return Ok(candidates.into_iter().map(str::to_owned).collect());
    };
    let mut in_head = BTreeSet::new();
    for chunk in path_chunks(&candidates) {
        let mut args = vec![
            "--literal-pathspecs",
            "ls-tree",
            "-z",
            "--name-only",
            parent,
            "--",
        ];
        args.extend_from_slice(chunk);
        let out = ctx.read(&args).await?;
        in_head.extend(
            out.stdout
                .split(|&b| b == 0)
                .filter(|p| !p.is_empty())
                .map(|p| String::from_utf8_lossy(p).into_owned()),
        );
    }
    Ok(candidates
        .into_iter()
        .filter(|p| !in_head.contains(*p))
        .map(str::to_owned)
        .collect())
}

/// Sets the user's index to commit `new` for exactly the paths it changed from `parent`
/// (from nothing, for a first commit): what it deleted leaves the index, what it added or
/// changed takes the new content (`--replace`: a file that takes a folder's place takes its
/// entries' place too). Nothing else there changes. git's output when a step fails.
async fn follow_commit(
    ctx: &OpCtx<'_>,
    runner: &GitRunner,
    parent: Option<&str>,
    new: &str,
) -> Result<Option<String>> {
    let args: Vec<&str> = match parent {
        Some(parent) => vec!["diff-tree", "-r", "-z", "--no-renames", parent, new],
        None => vec![
            "diff-tree",
            "--root",
            "--no-commit-id",
            "-r",
            "-z",
            "--no-renames",
            new,
        ],
    };
    let out = runner.run_unchecked(&ctx.cwd, &args, CmdKind::Read).await?;
    if out.code != 0 {
        return Ok(Some(output(&out)));
    }
    let (mut removed, mut set) = (Vec::new(), Vec::new());
    let mut fields = out.stdout.split(|&b| b == 0).filter(|f| !f.is_empty());
    while let (Some(meta), Some(path)) = (fields.next(), fields.next()) {
        let meta = String::from_utf8_lossy(meta);
        let parts: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
        let path = String::from_utf8_lossy(path).into_owned();
        match parts.as_slice() {
            [_, _, _, _, status] if status.starts_with('D') => removed.push(path),
            [_, mode, _, sha, _] => set.push(((*mode).to_owned(), (*sha).to_owned(), path)),
            _ => {}
        }
    }
    let removed: Vec<&str> = removed.iter().map(String::as_str).collect();
    if let Some((_, out)) = update_index(ctx, runner, &["--force-remove"], &removed).await? {
        return Ok(Some(output(&out)));
    }
    // `--cacheinfo <mode> <sha> <path>` for each.
    for chunk in chunks_by(&set, |(mode, sha, path)| {
        mode.len() + sha.len() + path.len() + 15
    }) {
        let mut args = vec!["update-index", "--add", "--replace"];
        for (mode, sha, path) in chunk {
            args.extend(["--cacheinfo", mode.as_str(), sha.as_str(), path.as_str()]);
        }
        let out = runner
            .run_unchecked(&ctx.cwd, &args, CmdKind::Write)
            .await?;
        if out.code != 0 {
            return Ok(Some(output(&out)));
        }
    }
    // Entries set from the commit have no stat data: one refresh records it (each committed
    // file is read once more), so later reads do not read them again and again. Its exit code
    // says whether anything is modified, which is no failure here.
    if !set.is_empty() {
        runner
            .run_unchecked(
                &ctx.cwd,
                &["update-index", "-q", "--refresh"],
                CmdKind::Write,
            )
            .await?;
    }
    Ok(None)
}

/// Brings the user's index to commit `new` for the paths it changed from `parent`, and
/// drops the `stale` entries (see [`follow_commit`]); git's output when a step fails.
async fn follow_into_index(
    ctx: &OpCtx<'_>,
    runner: &GitRunner,
    parent: Option<&str>,
    new: &str,
    stale: &[String],
) -> Result<Option<String>> {
    if let Some(failed) = follow_commit(ctx, runner, parent, new).await? {
        return Ok(Some(failed));
    }
    let stale: Vec<&str> = stale.iter().map(String::as_str).collect();
    Ok(update_index(ctx, runner, &["--force-remove"], &stale)
        .await?
        .map(|(_, out)| output(&out)))
}

/// The paths commit `new` changed from `parent` (from nothing, for a first commit), without
/// rename detection; none if they cannot be read.
async fn committed_paths(
    ctx: &OpCtx<'_>,
    runner: &GitRunner,
    parent: Option<&str>,
    new: &str,
) -> Vec<String> {
    let args: Vec<&str> = match parent {
        Some(parent) => vec![
            "diff-tree",
            "-r",
            "-z",
            "--name-only",
            "--no-renames",
            parent,
            new,
        ],
        None => vec![
            "diff-tree",
            "--root",
            "--no-commit-id",
            "-r",
            "-z",
            "--name-only",
            new,
        ],
    };
    match runner.run_unchecked(&ctx.cwd, &args, CmdKind::Read).await {
        Ok(out) if out.code == 0 => out
            .stdout
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect(),
        _ => Vec::new(),
    }
}

/// Why git could not write the index, in a few words, from its `output`.
fn index_trouble(output: &str) -> String {
    if output.contains("index.lock") {
        return "another program holds its lock, index.lock".to_owned();
    }
    let line = output
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("git failed");
    let line = line.strip_prefix("fatal: ").unwrap_or(line);
    match line.char_indices().nth(120) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_owned(),
    }
}

/// Most paths the reset hint lists; past it, it resets every path.
const RESET_HINT_PATHS: usize = 20;

/// The command the user is told to run when the staging area could not follow a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ResetHint {
    /// `git reset -q -- <paths>`: every path plain (see [`plain_path`]), at most
    /// [`RESET_HINT_PATHS`] of them.
    Paths(String),
    /// `git reset -q`: the whole staging area made to match the last commit.
    All,
}

impl ResetHint {
    fn command(&self) -> &str {
        match self {
            Self::Paths(command) => command,
            Self::All => "git reset -q",
        }
    }
}

/// Whether `path` is safe to paste unquoted into any shell (cmd.exe, PowerShell, POSIX):
/// only letters, digits and `. _ / \ -`, not starting with `-`. Inside quotes cmd.exe still
/// expands `%VAR%` and PowerShell `$(..)`, so nothing else is listed at all.
fn plain_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('-')
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._/\\-".contains(c))
}

/// The command that brings the staging area level with the commit for `paths`: the paths
/// listed when all are plain and there are at most [`RESET_HINT_PATHS`]; else every path.
fn reset_hint(paths: &[String]) -> ResetHint {
    if paths.len() > RESET_HINT_PATHS || !paths.iter().all(|p| plain_path(p)) {
        return ResetHint::All;
    }
    ResetHint::Paths(format!("git reset -q -- {}", paths.join(" ")))
}

/// The commit HEAD is on, read with `runner` (None before the first commit).
async fn head_of(ctx: &OpCtx<'_>, runner: &GitRunner) -> Result<Option<String>> {
    let out = runner
        .run_unchecked(
            &ctx.cwd,
            &["rev-parse", "--verify", "-q", "HEAD"],
            CmdKind::Read,
        )
        .await?;
    Ok((out.code == 0).then(|| out.text().trim().to_owned()))
}

/// The branch HEAD is on, read with `runner` (None when detached).
async fn branch_of(ctx: &OpCtx<'_>, runner: &GitRunner) -> Result<Option<String>> {
    let out = runner
        .run_unchecked(
            &ctx.cwd,
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
            CmdKind::Read,
        )
        .await?;
    Ok((out.code == 0).then(|| out.text().trim().to_owned()))
}

/// "the staged entry for a deleted file", "the staged entries for 2 deleted files".
fn stale_said(n: usize) -> String {
    if n == 1 {
        "the staged entry for a deleted file".to_owned()
    } else {
        format!("the staged entries for {n} deleted files")
    }
}

/// How many files commit `new` changed compared with `parent` (renames found, each one
/// file), or with nothing for a first commit.
async fn changed_count(
    ctx: &OpCtx<'_>,
    runner: &GitRunner,
    parent: Option<&str>,
    new: &str,
) -> Result<usize> {
    let args: Vec<&str> = match parent {
        Some(parent) => vec!["diff-tree", "-r", "-z", "--name-only", "-M", parent, new],
        None => vec!["diff-tree", "--root", "-r", "-z", "--name-only", new],
    };
    let out = runner.run_unchecked(&ctx.cwd, &args, CmdKind::Read).await?;
    // The root form names the commit first.
    let names = out.stdout.split(|&b| b == 0).filter(|p| !p.is_empty());
    Ok(match parent {
        Some(_) => names.count(),
        None => names.skip(1).count(),
    })
}

/// Commits `paths` with `message` on the branch checked out (see the module comment), then,
/// with `push`, pushes the branch and says how that went separately. With `empty` (and no
/// `paths`), the commit has HEAD's tree: built in the temporary index from HEAD alone, so
/// nothing the user staged goes in it.
pub(crate) async fn commit(
    ctx: &OpCtx<'_>,
    paths: &[String],
    message: &str,
    push: bool,
    empty: bool,
) -> Result<Step> {
    // What is in progress is the turn's read: one that changed since stops
    // the commit, never turns a partial commit into a merge's conclusion or back.
    let in_progress = read_kind(&ctx.runner, &ctx.cwd).await?;
    if let Some(&seen) = ctx.turn_in_progress.get()
        && seen != in_progress
    {
        let what = match (seen, in_progress) {
            (_, Some(kind)) => format!("a {} started", kind_name(kind)),
            (Some(kind), None) => format!("the {} ended", kind_name(kind)),
            (None, None) => unreachable!("they differ"),
        };
        return Ok(Step::Failed {
            message: said(
                "Nothing was committed",
                format!(
                    "{} while gg was getting ready. Look at the files again, then commit.",
                    crate::notice::capitalized(&what)
                ),
            ),
            detail: String::new(),
            hints: Vec::new(),
        });
    }
    let in_merge = in_progress == Some(InProgressKind::Merge);
    let entries = changed_files(ctx).await?;
    let ticked: BTreeSet<&str> = paths.iter().map(String::as_str).collect();
    let mine: Vec<&FileEntry> = entries.iter().filter(|e| is_ticked(e, &ticked)).collect();
    if mine.is_empty() && !empty {
        return Err(nothing_to_commit());
    }
    let parent = head_commit(ctx).await?;
    let scratch = Scratch::new(ctx).await?;
    let message_file = scratch.message(message)?;
    // The message as written, blank lines trimmed (a line starting with `#` is kept); git's
    // own comment lines of a merge's message (`# Conflicts:`) go.
    let cleanup = if in_merge {
        "--cleanup=strip"
    } else {
        "--cleanup=whitespace"
    };
    let mut commit_args = vec!["commit", cleanup, "-F", message_file.as_str()];
    if empty {
        commit_args.push("--allow-empty");
    }

    // A merge is committed whole, in the user's index: saved first, put back if no commit is
    // made.
    let saved = if in_merge {
        Some(scratch.save_index()?)
    } else {
        None
    };
    let runner = if in_merge {
        ctx.runner.clone()
    } else {
        if let Some(parent) = &parent {
            // HEAD's tree, with the stat data the user's index has for it: git hashes again
            // only what differs (never the user's staging: `--reset` takes HEAD's entries).
            let output_to = format!("--index-output={}", scratch.index().display());
            let args = ["read-tree", "--reset", output_to.as_str(), parent.as_str()];
            let out = ctx.git(&args).await?;
            if out.code != 0 {
                return Ok(step_failed(&args, &out));
            }
        }
        ctx.runner.with_index_file(&scratch.index())
    };
    match stage(ctx, &runner, &mine).await {
        Ok(None) => {}
        Ok(Some(failed)) => return Ok(scratch.put_back(saved.as_deref(), failed)),
        Err(e) => {
            if let Some(saved) = &saved
                && let Err(why) = restore_index(&scratch.real_index, saved)
            {
                return Err(CoreError {
                    message: format!("{} {}", e.message, not_put_back(&why)),
                    ..e
                });
            }
            return Err(e);
        }
    }
    let stale = if in_merge || empty {
        Vec::new()
    } else {
        stale_entries(ctx, parent.as_deref(), &mine).await?
    };
    if !in_merge && !empty {
        // Nothing differs from HEAD: no commit, and no hook runs for nothing.
        let same: Vec<&str> = match &parent {
            Some(parent) => vec!["diff-index", "--cached", "--quiet", parent.as_str(), "--"],
            None => vec!["ls-files", "--cached", "--error-unmatch", "--", "."],
        };
        let out = runner.run_unchecked(&ctx.cwd, &same, CmdKind::Read).await?;
        let nothing = if parent.is_some() {
            out.code == 0
        } else {
            out.code != 0
        };
        if nothing {
            if stale.is_empty() {
                return Err(nothing_to_commit());
            }
            let stale: Vec<&str> = stale.iter().map(String::as_str).collect();
            if let Some((args, out)) =
                update_index(ctx, &ctx.runner, &["--force-remove"], &stale).await?
            {
                return Ok(step_failed(
                    &args.iter().map(String::as_str).collect::<Vec<_>>(),
                    &out,
                ));
            }
            return Ok(Step::Done(said(
                "Nothing to commit",
                format!("Removed {}.", stale_said(stale.len())),
            )));
        }
    }
    // Where a push goes, worked out before anything is committed (a Stop later ends every
    // command but those that follow a commit made). Without `push` too: the notice of a
    // commit on a branch with somewhere to push it offers the push.
    let push_to = match current_branch(ctx).await? {
        Some(branch) => push_remote(ctx, &branch)
            .await?
            .map(|remote| (branch, remote)),
        None => None,
    };
    let ran = runner
        .run_unchecked(&ctx.cwd, &commit_args, CmdKind::Write)
        .await;
    // Whatever git's exit (a Stop, a post-commit hook that failed), a HEAD that moved is a
    // commit made: what follows runs to its end, and the outcome says it was made.
    let after = ctx.runner.unstoppable();
    let now = head_of(ctx, &after).await?;
    let made = now.is_some() && now != parent;
    let trouble = match ran {
        Ok(out) if out.code == 0 => None,
        Ok(out) if !made => {
            return commit_failed(&commit_args, &out)
                .map(|step| scratch.put_back(saved.as_deref(), step));
        }
        Ok(out) => Some((
            "git reported a problem after the commit was made: see the log.".to_owned(),
            output(&out),
        )),
        Err(e) if !made => {
            if let Some(saved) = &saved
                && let Err(why) = restore_index(&scratch.real_index, saved)
            {
                return Err(CoreError {
                    message: format!("{} {}", e.message, not_put_back(&why)),
                    ..e
                });
            }
            return Err(e);
        }
        Err(e) if ctx.runner.is_stopped() => Some((
            "It was stopped while git finished it (a post-commit hook, say).".to_owned(),
            e.detail.unwrap_or_default(),
        )),
        Err(e) => Some((e.message, e.detail.unwrap_or_default())),
    };
    let Some(new) = now.filter(|_| made) else {
        return Err(CoreError::new(
            ErrorCode::GitFailed,
            "The new commit cannot be found.",
        ));
    };
    ctx.keep_outcome_when_stopped();
    *ctx.committed.lock().unwrap_or_else(|e| e.into_inner()) = Some(new.clone());

    // The paths the commit changed are as the new HEAD has them in the user's index too;
    // nothing else there changes.
    let mut note = None;
    if !in_merge {
        note = follow_into_index(ctx, &after, parent.as_deref(), &new, &stale).await?;
        if note.is_some() {
            // Another program may hold the index a moment (an editor's git status): once more.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            note = follow_into_index(ctx, &after, parent.as_deref(), &new, &stale).await?;
        }
    }
    drop(scratch);

    let count = changed_count(ctx, &after, parent.as_deref(), &new).await?;
    let branch = branch_of(ctx, &after).await?;
    let at = match &branch {
        Some(branch) => format!("{} on {}", name(short(&new)), name(branch)),
        None => format!("{} on the detached HEAD", name(short(&new))),
    };
    let committed = if empty {
        said("Made an empty commit", at)
    } else if in_merge {
        said(
            "Concluded the merge",
            format!("{at} \u{b7} {}", files(count)),
        )
    } else {
        said(format!("Committed {}", files(count)), at)
    };
    let retry: Vec<OpHint> = push_to
        .iter()
        .map(|(branch, remote)| OpHint::Push {
            branch: branch.clone(),
            remote: remote.clone(),
        })
        .collect();
    if let Some((why, detail)) = trouble {
        let stopped = ctx.runner.is_stopped();
        let title = title_of(&committed);
        let message = if stopped && push {
            said(
                format!("{title}, stopped before the push"),
                note_of(&committed),
            )
        } else if push {
            said(
                format!("{title}, but the push did not run"),
                as_sentence(&why),
            )
        } else {
            said(
                format!("{title}, but git reported a problem"),
                as_sentence(&why),
            )
        };
        let detail = match note {
            Some(note) => format!("{detail}\n{note}"),
            None => detail,
        };
        return Ok(if push {
            Step::Failed {
                message,
                detail,
                hints: retry,
            }
        } else {
            Step::DoneWith {
                message,
                detail: Some(detail).filter(|d| !d.trim().is_empty()),
                hints: Vec::new(),
            }
        });
    }
    if let Some(output) = note {
        let mut paths = committed_paths(ctx, &after, parent.as_deref(), &new).await;
        paths.extend(stale.iter().cloned());
        let hint = reset_hint(&paths);
        let fix = match &hint {
            ResetHint::Paths(_) => "Run the command under Details in the worktree to update it.",
            ResetHint::All => {
                "Run `git reset -q` in the worktree to make the staging area match the last \
                 commit; your files are not changed."
            }
        };
        let mut why = format!(
            "gg could not update it ({}), so the committed files may show as staged changes. \
             {fix}",
            index_trouble(&output)
        );
        if push {
            why.push_str(" The push did not run.");
        }
        let message = said(
            format!(
                "{}, but the staging area was not updated",
                title_of(&committed)
            ),
            why,
        );
        return Ok(Step::DoneWith {
            message,
            detail: Some(format!("{}\n\n{output}", hint.command())),
            hints: retry,
        });
    }
    if !push {
        return Ok(if retry.is_empty() {
            Step::Done(committed)
        } else {
            Step::DoneWith {
                message: committed,
                detail: None,
                hints: retry,
            }
        });
    }
    Ok(push_after(ctx, branch, committed).await)
}

/// Pushes `branch` after its commit (described by `committed`), as a push of it does,
/// and says how that went: a failure says the commit was made, with git's output and a push
/// to try again.
async fn push_after(ctx: &OpCtx<'_>, branch: Option<String>, committed: String) -> Step {
    let failed = |why: &str, detail: String, mut hints: Vec<OpHint>, retry: Option<OpHint>| {
        if let Some(retry) = retry {
            hints.insert(0, retry);
        }
        Step::Failed {
            message: said(
                format!("{}, but the push failed", title_of(&committed)),
                as_sentence(why),
            ),
            detail: if detail.is_empty() {
                as_sentence(&committed)
            } else {
                format!("{}\n{detail}", as_sentence(&committed))
            },
            hints,
        }
    };
    let Some(branch) = branch else {
        return failed(
            "HEAD is detached: there is no branch to push.",
            String::new(),
            Vec::new(),
            None,
        );
    };
    let remote = match push_remote(ctx, &branch).await {
        Ok(Some(remote)) => remote,
        Ok(None) => {
            return failed(
                "There is no remote to push it to.",
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
    let pushed = network::push(ctx, &branch, &remote, None, false, None, None).await;
    if ctx.runner.is_stopped() {
        // The commit stands; the push did not go (or not to its end).
        return Step::Failed {
            message: said(
                format!("{}, stopped during the push", title_of(&committed)),
                format!("It may not have reached {remote}."),
            ),
            detail: match &pushed {
                Ok(Step::Failed { detail, .. } | Step::Stopped { detail, .. }) => detail.clone(),
                Err(e) => e.detail.clone().unwrap_or_default(),
                _ => String::new(),
            },
            hints: retry.into_iter().collect(),
        };
    }
    match pushed {
        Ok(Step::Done(pushed)) => Step::Done(and_pushed(&committed, &pushed, &remote)),
        Ok(Step::DoneWith {
            message,
            detail,
            hints,
        }) => Step::DoneWith {
            message: and_pushed(&committed, &message, &remote),
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

/// The notice of a commit (`committed`) and its push that went through (`pushed`, to
/// `remote`): "Committed 2 files and pushed", the commit and where it went under it.
fn and_pushed(committed: &str, pushed: &str, remote: &str) -> String {
    let push_note = note_of(pushed);
    let went = if title_of(pushed).starts_with("Nothing to push") {
        format!("{remote} had it already")
    } else if push_note.is_empty() {
        format!("pushed to {remote}")
    } else {
        // "Upstream set."
        format!(
            "pushed to {remote}, {}",
            push_note.trim_end_matches('.').to_lowercase()
        )
    };
    said(
        format!("{} and pushed", title_of(committed)),
        format!("{} \u{b7} {went}", note_of(committed)),
    )
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
        let mut files = read_files(&repo.runner, &target).await?;
        if let Some(branch) = &wt.branch {
            files.push_remote = branch_push_remote(&repo.runner, &target, branch).await?;
        }
        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_go_in_runs_short_by_count_and_by_length() {
        let short: Vec<String> = (0..250).map(|i| format!("{i}")).collect();
        let short: Vec<&str> = short.iter().map(String::as_str).collect();
        let runs: Vec<usize> = path_chunks(&short).iter().map(|c| c.len()).collect();
        assert_eq!(runs, [100, 100, 50]);
        let long = "x".repeat(3000);
        let long: Vec<&str> = vec![long.as_str(); 5];
        let runs: Vec<usize> = path_chunks(&long).iter().map(|c| c.len()).collect();
        assert_eq!(runs, [2, 2, 1]);
        let huge = "y".repeat(20_000);
        let mixed = vec!["a", huge.as_str(), "b"];
        let runs: Vec<usize> = path_chunks(&mixed).iter().map(|c| c.len()).collect();
        assert_eq!(runs, [1, 1, 1], "an item over the budget goes alone");
        assert!(path_chunks(&[]).is_empty());
    }

    /// The hint lists the paths only when every one is plain (nothing any shell reads as more
    /// than a name, pasted into cmd.exe, PowerShell or a POSIX shell), and at most 20 of them;
    /// otherwise it is the bare `git reset -q`.
    #[test]
    fn the_reset_hint_lists_only_plain_paths() {
        let paths = |p: &[&str]| p.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            reset_hint(&paths(&["a.txt", "src/b-c_d.rs", r"win\path.md"])),
            ResetHint::Paths(r"git reset -q -- a.txt src/b-c_d.rs win\path.md".to_owned())
        );
        for odd in [
            "$(x)", "%PATH%", "a`b", "a b.txt", "-x", "it's", "a\"b", "a;b", "é.txt",
        ] {
            assert_eq!(reset_hint(&paths(&["a.txt", odd])), ResetHint::All, "{odd}");
        }
        let twenty: Vec<String> = (0..20).map(|i| format!("f{i}")).collect();
        assert!(matches!(reset_hint(&twenty), ResetHint::Paths(c) if c.ends_with(" f19")));
        let many: Vec<String> = (0..21).map(|i| format!("f{i}")).collect();
        assert_eq!(reset_hint(&many), ResetHint::All);
        assert_eq!(ResetHint::All.command(), "git reset -q");
    }

    #[test]
    fn index_trouble_is_said_in_a_few_words() {
        assert_eq!(
            index_trouble("fatal: Unable to create '/r/.git/index.lock': File exists.\n"),
            "another program holds its lock, index.lock"
        );
        assert_eq!(index_trouble("\nfatal: bad thing\nmore"), "bad thing");
        assert!(index_trouble(&"z".repeat(500)).ends_with('…'));
    }

    fn entry(path: &str, status: EntryStatus, orig: Option<&str>) -> FileEntry {
        FileEntry {
            path: path.into(),
            orig_path: orig.map(str::to_owned),
            status,
            staged: true,
            unstaged: false,
            committable: true,
            reason: None,
        }
    }

    fn strings(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| (*p).to_owned()).collect()
    }

    #[test]
    fn requests_are_checked_without_git() {
        let checked = |paths: &[&str], message: &str, empty: bool| {
            check(&strings(paths), message, empty).err().map(|e| e.code)
        };
        let code = |paths: &[&str], message: &str| checked(paths, message, false);
        assert_eq!(code(&["a"], "m"), None);
        assert_eq!(code(&["-x", "a b/\u{fc}.txt", "*"], "m"), None);
        assert_eq!(code(&[], "m"), Some(ErrorCode::NothingToDo));
        assert_eq!(code(&["a"], " \n\t"), Some(ErrorCode::BadRequest));
        for bad in ["", "/etc/passwd", "../out", "a/../../b", "./a", "a\0b"] {
            assert_eq!(code(&[bad], "m"), Some(ErrorCode::BadRequest), "{bad:?}");
        }
        // An empty commit: no paths, still a message.
        assert_eq!(checked(&[], "m", true), None);
        assert_eq!(checked(&[], " ", true), Some(ErrorCode::BadRequest));
        assert_eq!(checked(&["a"], "m", true), Some(ErrorCode::BadRequest));
    }

    #[test]
    fn either_side_ticks_a_rename_and_only_its_own_path_a_copy() {
        let entries = [
            entry("new", EntryStatus::R, Some("old")),
            entry("copy", EntryStatus::C, Some("source")),
            entry("m", EntryStatus::M, None),
        ];
        let ticked_by = |paths: &[&str]| -> Vec<&str> {
            let ticked: BTreeSet<&str> = paths.iter().copied().collect();
            entries
                .iter()
                .filter(|e| is_ticked(e, &ticked))
                .map(|e| e.path.as_str())
                .collect()
        };
        assert_eq!(ticked_by(&["new"]), ["new"]);
        assert_eq!(ticked_by(&["old"]), ["new"]);
        // A copy's source is not changed by it: ticking the source does not tick the copy.
        assert_eq!(ticked_by(&["copy"]), ["copy"]);
        assert!(ticked_by(&["source"]).is_empty());
        assert_eq!(ticked_by(&["m", "not changed"]), ["m"]);
        assert!(ticked_by(&["not changed"]).is_empty());
    }

    #[test]
    fn plural() {
        assert_eq!(files(1), "1 file");
        assert_eq!(files(0), "0 files");
        assert_eq!(files(3), "3 files");
    }
}
