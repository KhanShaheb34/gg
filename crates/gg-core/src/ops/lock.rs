//! A stale `index.lock` (M5). git takes the lock of a worktree's staging area by creating
//! `index.lock` beside it, and removes it when done; a git that crashed (or was killed)
//! leaves it behind, and every later command that needs the staging area fails with
//! "Unable to create '…/index.lock': File exists". git says nothing of who holds it.
//!
//! An operation that failed so has the lock named in its failure, with how long ago it was
//! made. When it is older than [`STALE_AFTER`] and no git child of the app's is running for
//! the repository, it is offered for removal (`remove_index_lock`): a red confirmation names
//! the file, its age and the warning that another git or an editor may still be using the
//! repository, with a token of the file as seen (path, modification time, size; R39). The
//! removal deletes exactly the active worktree's `index.lock` (`git rev-parse --git-path
//! index.lock`), only a regular file (never a symlink, never anything else), and only the
//! file the user confirmed. Nothing is ever removed silently, and a fresh lock is never
//! offered.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::dto::{Danger, DangerKind, OpHint};
use crate::error::Result;
use crate::ops::{OpCtx, Step};

/// A lock file at least this old, with no git of the app's running, may be left behind.
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(10 * 60);

/// How long the app's own git children of the repository are waited for before a lock is
/// judged: a read running beside the operation is over in a moment.
const IDLE_WAIT: Duration = Duration::from_millis(1500);

/// The lock file as seen: where, when it was last changed, how big.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockFile {
    pub path: PathBuf,
    pub modified: SystemTime,
    pub size: u64,
}

impl LockFile {
    /// What a confirmation is bound to: the file, its time and its size (R39).
    pub fn token(&self) -> String {
        let nanos = self
            .modified
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        format!("{}\n{nanos}\n{}", self.path.display(), self.size)
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
    }))
}

/// Where the staging area's lock of the worktree the operation runs in is:
/// `git rev-parse --git-path index.lock`, made absolute against the worktree.
async fn lock_path(ctx: &OpCtx<'_>) -> Result<Option<PathBuf>> {
    let out = ctx.read(&["rev-parse", "--git-path", "index.lock"]).await?;
    if out.code != 0 {
        return Ok(None);
    }
    let text = out.text();
    let text = text.trim_end_matches(['\n', '\r']);
    if text.is_empty() {
        return Ok(None);
    }
    let path = Path::new(text);
    Ok(Some(if path.is_absolute() {
        path.to_owned()
    } else {
        ctx.cwd.join(path)
    }))
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

/// Whether git's output `detail` says it could not take the staging area's lock because
/// `index.lock` exists.
fn index_lock_exists(detail: &str) -> bool {
    detail
        .lines()
        .any(|l| l.contains("Unable to create '") && l.contains("index.lock': File exists"))
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
    if !index_lock_exists(&detail) {
        return Ok(Step::Failed {
            message,
            detail,
            hints,
        });
    }
    let found = match lock_path(ctx).await? {
        Some(path) => look(&path).ok().flatten(),
        None => None,
    };
    let Some(lock) = found else {
        // Gone already (the other git finished): a retry may work.
        return Ok(Step::Failed {
            message: "Another git process was working in this repository: git found its lock \
                      file, index.lock. It is gone now: try again."
                .to_owned(),
            detail,
            hints,
        });
    };
    let shown = lock.path.display();
    let age = age_text(lock.age());
    let message = if lock.age() < STALE_AFTER || app_git_running(ctx).await {
        format!(
            "Another git process is working in this repository: git found its lock file, \
             {shown}, made {age} ago. Try again once it is done."
        )
    } else {
        hints.push(OpHint::RemoveIndexLock {
            path: lock.path.display().to_string(),
            worktree: ctx.cwd.display().to_string(),
        });
        format!(
            "git found a lock file, {shown}, made {age} ago, and no git started by gg is \
             running here: it was probably left behind by a git that crashed. Another \
             program (a terminal, an editor) may still be using it; if none is, remove it."
        )
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
    let shown = path.display();
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
    if lock.age() < STALE_AFTER {
        return Ok(Some(format!(
            "Nothing was removed: {shown} was made {} ago, so a git process may still be \
             using it. gg removes only a lock file at least {} old.",
            age_text(lock.age()),
            age_text(STALE_AFTER)
        )));
    }
    if app_git_running(ctx).await {
        return Ok(Some(
            "Nothing was removed: gg is running git in this repository right now. Try again \
             in a moment."
                .to_owned(),
        ));
    }
    Ok(None)
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
    Ok(vec![Danger {
        kind: DangerKind::RemoveIndexLock,
        message: format!(
            "Remove the lock file {}, made {} ago? If another git process or an editor is \
             still working in this repository, removing its lock can damage the staging area. \
             Make sure none is.",
            lock.path.display(),
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
    if lock.token() != token {
        return Ok(failed(
            "Nothing was removed: the lock file changed while gg was getting ready, so \
             another git process may be using it.",
        ));
    }
    if let Err(e) = std::fs::remove_file(&lock.path) {
        return Ok(Step::Failed {
            message: format!("Could not remove {}.", lock.path.display()),
            detail: e.to_string(),
            hints: Vec::new(),
        });
    }
    Ok(Step::Done(format!(
        "Removed the lock file {}. Try again what failed.",
        lock.path.display()
    )))
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
    fn ages_read_as_people_say_them() {
        assert_eq!(age_text(Duration::from_secs(40)), "40 s");
        assert_eq!(age_text(Duration::from_secs(12 * 60 + 5)), "12 min");
        assert_eq!(age_text(Duration::from_secs(3 * 3600)), "3 h");
        assert_eq!(age_text(Duration::from_secs(86_400 + 5)), "1 day");
        assert_eq!(age_text(Duration::from_secs(5 * 86_400)), "5 days");
    }

    #[test]
    fn only_the_staging_areas_lock_is_recognised() {
        assert!(index_lock_exists(
            "fatal: Unable to create '/r/.git/index.lock': File exists.\n\nAnother git process…"
        ));
        assert!(!index_lock_exists(
            "error: cannot lock ref 'refs/heads/x': Unable to create '/r/.git/refs/heads/x.lock': File exists."
        ));
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
