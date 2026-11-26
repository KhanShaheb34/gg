//! A stale `index.lock`. git takes the lock of a worktree's staging area by creating
//! `index.lock` beside it, and removes it when done; a git that crashed (or was killed)
//! leaves it behind, and every later command that needs the staging area fails with
//! "Unable to create '…/index.lock': File exists". git says nothing of who holds it.
//!
//! An operation that failed so has the lock named in its failure, with how long ago it was
//! made. When it is older than [`STALE_AFTER`], no git child of the app's is running for
//! the repository and no other git process on this computer has the worktree or the git
//! folder as its working folder (a `git commit` waiting
//! in a terminal's editor holds the lock for as long as the editor is open; see
//! [`crate::platform::git_running_in`]; where that cannot be looked at, the messages say
//! so), it is offered for removal (`remove_index_lock`): a red confirmation names
//! the file, its age and the warning that another git or an editor may still be using the
//! repository, with a token of the file as seen (path, modification time, size). The
//! removal deletes exactly the active worktree's `index.lock` (`git rev-parse --git-path
//! index.lock`), only a regular file (never a symlink, never anything else), and only the
//! file the user confirmed. Nothing is ever removed silently, and a fresh lock is never
//! offered.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::dto::{Danger, DangerKind, OpHint};
use crate::error::Result;
use crate::notice::said;
use crate::ops::{OpCtx, Step};
use crate::platform::GitScan;

/// A lock file at least this old, with no git of the app's running, may be left behind.
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(10 * 60);

/// How long the app's own git children of the repository are waited for before a lock is
/// judged: a read running beside the operation is over in a moment.
const IDLE_WAIT: Duration = Duration::from_millis(1500);

/// The lock file as seen: where, when it was last changed, how big, and which file it is
/// (another file made under its name is another lock).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockFile {
    pub path: PathBuf,
    pub modified: SystemTime,
    pub size: u64,
    pub identity: Option<u128>,
}

impl LockFile {
    /// What a confirmation is bound to: the file (its path and identity), its time and its
    /// size.
    pub fn token(&self) -> String {
        let nanos = self
            .modified
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let identity = self
            .identity
            .map_or_else(String::new, |id| format!("{id:x}"));
        format!(
            "{}\n{nanos}\n{}\n{identity}",
            self.path.display(),
            self.size
        )
    }

    /// How long ago it was last changed (zero if it is in the future).
    pub fn age(&self) -> Duration {
        SystemTime::now()
            .duration_since(self.modified)
            .unwrap_or_default()
    }
}

/// What is at `path`: `Ok(Some)` for a regular file, `Ok(None)` when nothing is there,
/// `Err(what it is)` for anything else (a symlink, a folder), which is never removed.
fn look(path: &Path) -> std::result::Result<Option<LockFile>, &'static str> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(_) => return Ok(None),
    };
    if meta.file_type().is_symlink() {
        return Err("a symbolic link");
    }
    if !meta.is_file() {
        return Err("not a plain file");
    }
    Ok(Some(LockFile {
        path: path.to_owned(),
        modified: meta.modified().unwrap_or(UNIX_EPOCH),
        size: meta.len(),
        identity: crate::platform::file_identity(path, &meta),
    }))
}

/// Where the staging area's lock of the worktree the operation runs in is:
/// `git rev-parse --git-path index.lock`, made absolute against the worktree.
async fn lock_path(ctx: &OpCtx<'_>) -> Result<Option<PathBuf>> {
    let out = ctx.read(&["rev-parse", "--git-path", "index.lock"]).await?;
    if out.code != 0 {
        return Ok(None);
    }
    Ok(lock_path_from(&out.text(), &ctx.cwd, cfg!(windows)))
}

/// The lock's path from `git rev-parse --git-path`'s output `text`, made absolute against
/// `cwd`, spelt as the platform does (`windows`: git writes `/` there, which the messages and
/// the hint would otherwise show, or mix with the worktree's `\`).
fn lock_path_from(text: &str, cwd: &Path, windows: bool) -> Option<PathBuf> {
    let text = text.trim_end_matches(['\n', '\r']);
    if text.is_empty() {
        return None;
    }
    if windows {
        // Spelt out rather than with `Path` (pure, so it is tested on any host): a drive
        // (`C:\`) or a share (`\\host`) is absolute; anything else is under `cwd`.
        let native = crate::platform::native_path_for(true, text);
        let b = native.as_bytes();
        let absolute = native.starts_with('\\') || (b.len() > 2 && b[1] == b':' && b[2] == b'\\');
        return Some(if absolute {
            PathBuf::from(native)
        } else {
            let cwd = cwd.to_string_lossy();
            PathBuf::from(format!("{}\\{native}", cwd.trim_end_matches('\\')))
        });
    }
    let path = Path::new(text);
    Some(if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    })
}

/// Whether git child of the app's is running for the repository, after waiting up to
/// [`IDLE_WAIT`] for any to end.
async fn app_git_running(ctx: &OpCtx<'_>) -> bool {
    let repo_id = ctx.repo.identity.repo_id;
    let until = std::time::Instant::now() + IDLE_WAIT;
    loop {
        if ctx.runner.children_running(repo_id) == 0 {
            return false;
        }
        if std::time::Instant::now() >= until {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// What a lock file found in the repository is taken for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Younger than [`STALE_AFTER`]: a git may well be working.
    Fresh,
    /// A git child of the app's runs for the repository.
    AppGit,
    /// Another git process (this pid) works in the repository.
    OtherGit(u32),
    /// Probably left behind by a git that crashed: offered for removal. `checked`: whether
    /// other git processes could be looked for at all.
    Stale { checked: bool },
}

/// The verdict on a lock `age` old, with a git of the app's running or not (`app_git`) and
/// what a look for other git processes found (`scan`). "Stale" is read as: at least
/// [`STALE_AFTER`] old, and no git known to run in the repository.
fn verdict(age: Duration, app_git: bool, scan: GitScan) -> Verdict {
    if age < STALE_AFTER {
        return Verdict::Fresh;
    }
    if app_git {
        return Verdict::AppGit;
    }
    match scan {
        GitScan::Running(pid) => Verdict::OtherGit(pid),
        GitScan::NoneRunning => Verdict::Stale { checked: true },
        GitScan::Unknown => Verdict::Stale { checked: false },
    }
}

/// Looks for a git process working in the worktree `cwd` or in the git dir that holds `lock`.
async fn scan_for_git(cwd: &Path, lock: &Path) -> GitScan {
    let mut dirs = vec![dunce::canonicalize(cwd).unwrap_or_else(|_| cwd.to_owned())];
    if let Some(git_dir) = lock.parent() {
        dirs.push(dunce::canonicalize(git_dir).unwrap_or_else(|_| git_dir.to_owned()));
    }
    tokio::task::spawn_blocking(move || crate::platform::git_running_in(&dirs))
        .await
        .unwrap_or(GitScan::Unknown)
}

/// The verdict on `lock`, found in the repository the operation runs in: a fresh lock is
/// judged without looking further.
async fn judge(ctx: &OpCtx<'_>, lock: &LockFile) -> Verdict {
    if lock.age() < STALE_AFTER {
        return Verdict::Fresh;
    }
    let app_git = app_git_running(ctx).await;
    let scan = if app_git {
        GitScan::Unknown
    } else {
        scan_for_git(&ctx.cwd, &lock.path).await
    };
    verdict(lock.age(), app_git, scan)
}

/// Said when other git processes could not be looked for.
const NOT_CHECKED: &str = "gg could not check whether another git is running";

/// `path` as a message names it: relative to the worktree `cwd` when it is inside it
/// (`.git/index.lock`), else in full (a linked worktree's lock, in the main git dir).
fn named(path: &Path, cwd: &Path) -> String {
    let cwd = dunce::canonicalize(cwd).unwrap_or_else(|_| cwd.to_owned());
    let full = dunce::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    full.strip_prefix(&cwd).map_or_else(
        |_| path.display().to_string(),
        |rel| rel.display().to_string(),
    )
}

/// `d` for people: "40 s", "12 min", "3 h", "2 days".
pub(crate) fn age_text(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s / 60),
        3600..86_400 => format!("{} h", s / 3600),
        86_400..172_800 => "1 day".to_owned(),
        _ => format!("{} days", s / 86_400),
    }
}

/// The `index.lock` git's output `detail` says it could not create because it exists, as
/// git names it.
fn lock_named(detail: &str) -> Option<&str> {
    detail.lines().find_map(|l| {
        let rest = &l[l.find("Unable to create '")? + "Unable to create '".len()..];
        let end = rest.find("': File exists")?;
        let path = &rest[..end];
        path.ends_with("index.lock").then_some(path)
    })
}

/// Whether `named` (a lock as git's message names it, relative to `cwd` if not absolute) is
/// the file `lock`: an operation in another worktree, or with a temporary index, names
/// another lock than the active worktree's.
fn same_file(named: &str, cwd: &Path, lock: &Path) -> bool {
    let named = Path::new(named);
    let named = if named.is_absolute() {
        named.to_owned()
    } else {
        cwd.join(named)
    };
    crate::platform::same_path(&named, lock)
}

/// `step`, when it failed because the staging area's lock exists, with the failure saying
/// so: the lock file named, how old it is; and, when it is stale (older than
/// [`STALE_AFTER`], and no git of the app's runs for the repository), the offer to remove
/// it (`remove_index_lock`). Any other step as it is.
pub(crate) async fn explain_failure(ctx: &OpCtx<'_>, step: Step) -> Result<Step> {
    let Step::Failed {
        message,
        detail,
        mut hints,
    } = step
    else {
        return Ok(step);
    };
    let Some(git_named) = lock_named(&detail) else {
        return Ok(Step::Failed {
            message,
            detail,
            hints,
        });
    };
    let path = lock_path(ctx).await?;
    if let Some(path) = &path {
        remove_leftovers(path);
    }
    if path
        .as_deref()
        .is_some_and(|path| !same_file(git_named, &ctx.cwd, path))
    {
        // Another lock than the active worktree's: git's own message says which.
        return Ok(Step::Failed {
            message,
            detail,
            hints,
        });
    }
    let found = path.and_then(|path| look(&path).ok().flatten());
    let Some(lock) = found else {
        // Gone already (the other git finished): a retry may work.
        return Ok(Step::Failed {
            message: said(
                "Another git process was working here",
                "git found its lock file, index.lock. It is gone now: try again.",
            ),
            detail,
            hints,
        });
    };
    let shown = crate::notice::path(named(&lock.path, &ctx.cwd));
    let age = age_text(lock.age());
    let message = match judge(ctx, &lock).await {
        Verdict::Fresh | Verdict::AppGit => said(
            "Another git process is working here",
            format!("git found its lock file, {shown}, made {age} ago. Try again once it is done."),
        ),
        Verdict::OtherGit(pid) => said(
            format!("A git process (pid {pid}) is working here"),
            format!(
                "git found its lock file, {shown}, made {age} ago. Try again once it is done (a \
                 commit waiting for its message in an editor holds it until the editor is \
                 closed)."
            ),
        ),
        Verdict::Stale { checked } => {
            hints.push(OpHint::RemoveIndexLock {
                path: lock.path.display().to_string(),
                worktree: ctx.cwd.display().to_string(),
            });
            let title = format!("A lock file from {age} ago is in the way");
            if checked {
                said(
                    title,
                    format!(
                        "No git process on this computer is working in this folder: {shown} was \
                         probably left behind by a git that crashed. Another program may still \
                         be using it; if none is, remove it."
                    ),
                )
            } else {
                said(
                    title,
                    format!(
                        "No git started by gg is running here; {NOT_CHECKED}. {shown} may have \
                         been left behind by a git that crashed: if no other git or editor is \
                         using this repository, remove it."
                    ),
                )
            }
        }
    };
    Ok(Step::Failed {
        message,
        detail,
        hints,
    })
}

/// Why the lock file cannot be removed now, if it cannot: it is gone, it is no plain file,
/// it is fresh, or a git of the app's is running for the repository.
pub(crate) async fn refused(ctx: &OpCtx<'_>) -> Result<Option<String>> {
    let Some(path) = lock_path(ctx).await? else {
        return Ok(Some(
            "Nothing was removed: git could not say where the lock file is.".to_owned(),
        ));
    };
    remove_leftovers(&path);
    let shown = named(&path, &ctx.cwd);
    let lock = match look(&path) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            return Ok(Some(format!(
                "Nothing was removed: {shown} is gone already. Try again what failed."
            )));
        }
        Err(what) => {
            return Ok(Some(format!(
                "Nothing was removed: {shown} is {what}, which gg never removes."
            )));
        }
    };
    match judge(ctx, &lock).await {
        Verdict::Fresh => Ok(Some(format!(
            "Nothing was removed: {shown} was made {} ago, so a git process may still be \
             using it. gg removes only a lock file at least {} old.",
            age_text(lock.age()),
            age_text(STALE_AFTER)
        ))),
        Verdict::AppGit => Ok(Some(
            "Nothing was removed: gg is running git in this repository right now. Try again \
             in a moment."
                .to_owned(),
        )),
        Verdict::OtherGit(pid) => Ok(Some(format!(
            "Nothing was removed: A git process (pid {pid}) is using this repository; the \
             lock is not stale."
        ))),
        Verdict::Stale { .. } => Ok(None),
    }
}

/// The `remove_index_lock` danger, always: the file named, its age, and that another git
/// or an editor may be using the repository; its token is the file as seen.
pub(crate) async fn danger(ctx: &OpCtx<'_>) -> Result<Vec<Danger>> {
    let Some(path) = lock_path(ctx).await? else {
        return Ok(Vec::new()); // refused before
    };
    let Ok(Some(lock)) = look(&path) else {
        return Ok(Vec::new()); // refused before
    };
    let unchecked = match scan_for_git(&ctx.cwd, &lock.path).await {
        GitScan::Unknown => format!(" {NOT_CHECKED}."),
        GitScan::Running(_) | GitScan::NoneRunning => String::new(),
    };
    Ok(vec![Danger {
        kind: DangerKind::RemoveIndexLock,
        message: format!(
            "Remove the lock file {}, made {} ago? If another git process or an editor is \
             still working in this repository, removing its lock can damage the staging area. \
             Make sure none is: gg cannot see a git started with --git-dir from another \
             folder, another user's or a container's git, or other programs that lock the \
             staging area.{unchecked}",
            named(&lock.path, &ctx.cwd),
            age_text(lock.age())
        ),
        detail: None,
        token: Some(lock.token()),
    }])
}

/// Removes the lock file the user confirmed (`dangers`: the token of the file as it was
/// seen in this turn), if it is still that file; nothing else, and never a symlink.
pub(crate) async fn remove(ctx: &OpCtx<'_>, dangers: &[Danger]) -> Result<Step> {
    let confirmed = dangers
        .iter()
        .find(|d| d.kind == DangerKind::RemoveIndexLock)
        .and_then(|d| d.token.clone());
    let path = lock_path(ctx).await?;
    let now = path.as_deref().map(look);
    let (Some(token), Some(Ok(Some(lock)))) = (confirmed, now) else {
        return Ok(failed(
            "Nothing was removed: the lock file is gone already, or is no plain file.",
        ));
    };
    let changed = || {
        failed(
            "Nothing was removed: the lock file changed while gg was getting ready, so \
             another git process may be using it.",
        )
    };
    if lock.token() != token {
        return Ok(changed());
    }
    match remove_if_still(&lock.path, &token) {
        Ok(true) => {}
        Ok(false) => return Ok(changed()),
        Err(e) => {
            return Ok(Step::Failed {
                message: format!("Could not remove {}.", lock.path.display()),
                detail: e.to_string(),
                hints: Vec::new(),
            });
        }
    }
    Ok(Step::Done(said(
        format!(
            "Removed the lock file {}",
            crate::notice::path(named(&lock.path, &ctx.cwd))
        ),
        "Try again what failed.",
    )))
}

/// Removes the lock file at `path` if it is still the plain file of `token`, with no gap
/// between that check and the removal: the file is first moved aside (a git that starts
/// meanwhile makes a new lock, which is never touched), checked there, then removed; if it
/// is not the file confirmed, it is put back (unless a new lock took its place: then it is
/// left aside, named in the error). `Ok(false)`: not the same file, nothing removed.
fn remove_if_still(path: &Path, token: &str) -> std::io::Result<bool> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let aside = path.with_file_name(format!("{ASIDE_PREFIX}{}-{nanos}", std::process::id()));
    std::fs::rename(path, &aside)?;
    let same = matches!(look(&aside), Ok(Some(moved)) if LockFile { path: path.to_owned(), ..moved }.token() == token);
    if same {
        std::fs::remove_file(&aside).map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!(
                    "the lock file was moved aside but could not be removed ({e}); it is at {}",
                    aside.display()
                ),
            )
        })?;
        return Ok(true);
    }
    if let Err(e) = crate::store::move_no_overwrite(&aside, path) {
        return Err(std::io::Error::new(
            e.kind(),
            format!(
                "the lock file changed and could not be put back; it is at {}",
                aside.display()
            ),
        ));
    }
    Ok(false)
}

/// The name a lock file moved aside for its removal starts with.
const ASIDE_PREFIX: &str = "index.lock.gg-removing-";

/// Moved-aside lock files older than this are left over from a removal that failed (or an
/// app that stopped midway) and are removed at the next lock check.
const LEFTOVER_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Removes the moved-aside lock files ([`ASIDE_PREFIX`]) older than [`LEFTOVER_AFTER`] in
/// the folder of `lock`: plain files only. Best effort.
fn remove_leftovers(lock: &Path) {
    let Some(dir) = lock.parent() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(ASIDE_PREFIX)
        {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .is_some_and(|age| age >= LEFTOVER_AFTER);
        if meta.is_file() && old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A failure with nothing to show beside its line.
fn failed(message: &str) -> Step {
    Step::Failed {
        message: message.to_owned(),
        detail: String::new(),
        hints: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_path_is_spelt_as_the_platform_does() {
        let cwd = Path::new("/r");
        assert_eq!(
            lock_path_from(".git/index.lock\n", cwd, false),
            Some(PathBuf::from("/r/.git/index.lock"))
        );
        assert_eq!(lock_path_from("\n", cwd, false), None);
        // Git for Windows: `/` in what it prints, `\` in what the app shows.
        let shown = |text: &str, cwd: &str| {
            lock_path_from(text, Path::new(cwd), true).map(|p| p.to_string_lossy().into_owned())
        };
        let relative = shown(".git/index.lock\r\n", r"C:\repo").unwrap();
        assert!(!relative.contains('/'), "{relative}");
        assert!(relative.ends_with(r".git\index.lock"), "{relative}");
        assert_eq!(
            shown("C:/repo/.git/worktrees/x/index.lock\n", r"C:\repo").as_deref(),
            Some(r"C:\repo\.git\worktrees\x\index.lock")
        );
    }

    #[test]
    fn ages_read_as_people_say_them() {
        assert_eq!(age_text(Duration::from_secs(40)), "40 s");
        assert_eq!(age_text(Duration::from_secs(12 * 60 + 5)), "12 min");
        assert_eq!(age_text(Duration::from_secs(3 * 3600)), "3 h");
        assert_eq!(age_text(Duration::from_secs(86_400 + 5)), "1 day");
        assert_eq!(age_text(Duration::from_secs(5 * 86_400)), "5 days");
    }

    #[test]
    fn only_the_staging_areas_lock_is_recognised() {
        assert!(
            lock_named(
                "fatal: Unable to create '/r/.git/index.lock': File exists.\n\nAnother git process…"
            )
            .is_some()
        );
        assert!(
            lock_named(
                "error: cannot lock ref 'refs/heads/x': Unable to create '/r/.git/refs/heads/x.lock': File exists."
            )
            .is_none()
        );
    }

    #[test]
    fn a_lock_is_removed_only_while_it_is_the_file_confirmed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.lock");
        std::fs::write(&path, "").unwrap();
        let token = look(&path).unwrap().unwrap().token();
        // Another file under the name since: put back, nothing removed, nothing left aside.
        std::fs::write(&path, "someone else's").unwrap();
        assert!(!remove_if_still(&path, &token).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "someone else's");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        // The file confirmed: removed.
        let token = look(&path).unwrap().unwrap().token();
        assert!(remove_if_still(&path, &token).unwrap());
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// Another file made under the lock's name, with the same size and time, is another
    /// lock: the token names the file itself.
    #[test]
    fn another_file_of_the_same_size_and_time_is_another_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.lock");
        std::fs::write(&path, "").unwrap();
        let first = look(&path).unwrap().unwrap();
        // The first one kept aside, so the new one cannot reuse its identity.
        std::fs::rename(&path, dir.path().join("first")).unwrap();
        let file = std::fs::File::create(&path).unwrap();
        file.set_modified(first.modified).unwrap();
        drop(file);
        let second = look(&path).unwrap().unwrap();
        assert_eq!((second.modified, second.size), (first.modified, first.size));
        assert!(first.identity.is_some());
        assert_ne!(second.token(), first.token());
        assert!(!remove_if_still(&path, &first.token()).unwrap());
        assert!(path.exists());
    }

    /// Lock files moved aside by a removal that did not finish are removed once a day old;
    /// nothing else is touched.
    #[test]
    fn leftovers_of_a_removal_go_after_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("index.lock");
        let old = dir.path().join(format!("{ASIDE_PREFIX}1-2"));
        let new = dir.path().join(format!("{ASIDE_PREFIX}3-4"));
        let other = dir.path().join("index.lock.other");
        for f in [&lock, &old, &new, &other] {
            std::fs::write(f, "").unwrap();
        }
        let day_ago = SystemTime::now() - LEFTOVER_AFTER - Duration::from_secs(60);
        for f in [&old, &other] {
            let file = std::fs::File::options().write(true).open(f).unwrap();
            file.set_modified(day_ago).unwrap();
        }
        remove_leftovers(&lock);
        assert!(!old.exists());
        assert!(new.exists() && other.exists() && lock.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_put_in_place_of_the_lock_is_put_back_not_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.lock");
        std::fs::write(&path, "").unwrap();
        let token = look(&path).unwrap().unwrap().token();
        let target = dir.path().join("precious");
        std::fs::write(&target, "keep").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(!remove_if_still(&path, &token).unwrap());
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
    }

    #[test]
    fn the_lock_git_names_is_read_from_its_message() {
        let detail =
            "fatal: Unable to create '/r/it's here/.git/index.lock': File exists.\n\nAnother";
        assert_eq!(lock_named(detail), Some("/r/it's here/.git/index.lock"));
        assert_eq!(
            lock_named("fatal: Unable to create '/tmp/gg-commit-1/index.lock': File exists."),
            Some("/tmp/gg-commit-1/index.lock")
        );
        assert_eq!(
            lock_named("fatal: Unable to create '/r/.git/refs/heads/x.lock': File exists."),
            None
        );
    }

    #[test]
    fn only_the_active_worktrees_own_lock_is_taken_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let main_git = root.join("main").join(".git");
        let linked_git = main_git.join("worktrees").join("side");
        std::fs::create_dir_all(&linked_git).unwrap();
        let linked_lock = linked_git.join("index.lock");
        let main_lock = main_git.join("index.lock");
        let cwd = root.join("main");
        // As git names it: absolute, or relative to where it ran.
        assert!(same_file(&main_lock.to_string_lossy(), &cwd, &main_lock));
        assert!(same_file(".git/index.lock", &cwd, &main_lock));
        // The main worktree's lock, or a temporary index's, is not the linked one's.
        assert!(!same_file(&main_lock.to_string_lossy(), &cwd, &linked_lock));
        let temp = root.join("gg-commit-1").join("index.lock");
        assert!(!same_file(&temp.to_string_lossy(), &cwd, &main_lock));
    }

    #[test]
    fn a_lock_is_stale_only_when_old_and_no_git_is_known_to_run() {
        let old = STALE_AFTER + Duration::from_secs(1);
        let young = STALE_AFTER - Duration::from_secs(1);
        assert_eq!(verdict(young, false, GitScan::NoneRunning), Verdict::Fresh);
        assert_eq!(verdict(young, false, GitScan::Running(7)), Verdict::Fresh);
        assert_eq!(verdict(old, true, GitScan::NoneRunning), Verdict::AppGit);
        assert_eq!(
            verdict(old, false, GitScan::Running(7)),
            Verdict::OtherGit(7)
        );
        assert_eq!(
            verdict(old, false, GitScan::NoneRunning),
            Verdict::Stale { checked: true }
        );
        // Where git processes cannot be looked for (Windows), still offered, saying so.
        assert_eq!(
            verdict(old, false, GitScan::Unknown),
            Verdict::Stale { checked: false }
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_or_a_folder_is_never_a_lock_file_to_remove() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, "x").unwrap();
        let link = dir.path().join("index.lock");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(look(&link), Err("a symbolic link"));
        let folder = dir.path().join("folder.lock");
        std::fs::create_dir(&folder).unwrap();
        assert_eq!(look(&folder), Err("not a plain file"));
        assert_eq!(look(&dir.path().join("none")), Ok(None));
        let file = look(&target).unwrap().unwrap();
        assert_eq!(file.size, 1);
        assert!(file.token().starts_with(&format!("{}\n", target.display())));
    }
}
