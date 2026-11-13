//! Watches a repository's git directory, so that changes made outside the app (a commit in
//! a terminal, an agent working in a linked worktree) show up without a manual refresh.
//!
//! Watched: the common git dir itself, non-recursively (`HEAD`, `index`, `packed-refs`,
//! `FETCH_HEAD`, `config`, ...), and `refs/` and `worktrees/` recursively while they exist.
//! Linked worktrees keep their `HEAD` and `index` under `worktrees/<name>/`. Changes under
//! `objects/` and `logs/` and to `*.lock` files are ignored, so a `git gc` or a large fetch
//! does not look like a stream of changes.
//!
//! The OS watcher's callback never blocks: it only wakes a Tokio task, which debounces and
//! then calls `on_change`. Nothing runs while nothing changes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::event::EventKind;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout_at};

use crate::error::{CoreError, ErrorCode, Result};

/// `on_change` waits for this much quiet after the last change.
const QUIET: Duration = Duration::from_millis(300);
/// ...but during continuous activity fires at least this often.
const MAX_DELAY: Duration = Duration::from_secs(1);
/// Subdirectories of the git dir watched recursively while they exist.
const RECURSIVE_DIRS: [&str; 2] = ["refs", "worktrees"];

/// Watches a git directory until dropped. Dropping it stops the OS watcher and the
/// debounce task. A call to `on_change` already running when it is dropped still
/// completes, but no new call starts afterwards.
pub struct RepoWatcher {
    os: Arc<Mutex<Option<RecommendedWatcher>>>,
    task: JoinHandle<()>,
}

impl Drop for RepoWatcher {
    fn drop(&mut self) {
        self.task.abort();
        // Dropping the OS watcher stops its event stream (and joins its thread, if any).
        drop(lock(&self.os).take());
    }
}

/// Starts watching `common_git_dir` (see the module docs for what is watched). `on_change`
/// is called on a Tokio task 300 ms after the last change, and at least once a second while
/// changes keep coming. Must be called inside a Tokio runtime.
pub fn watch_git_dir(
    common_git_dir: &Path,
    on_change: impl Fn() + Send + Sync + 'static,
) -> Result<RepoWatcher> {
    watch_git_dir_reporting(common_git_dir, on_change, |_| {})
}

/// Like [`watch_git_dir`], and calls `on_error` (on the debounce task) each time a
/// directory created later (`refs/`, `worktrees/`) cannot be watched, so changes inside it
/// will be missed. Failing to watch anything that exists at the start is an `Err` instead.
pub fn watch_git_dir_reporting(
    common_git_dir: &Path,
    on_change: impl Fn() + Send + Sync + 'static,
    on_error: impl Fn(CoreError) + Send + Sync + 'static,
) -> Result<RepoWatcher> {
    let runtime = tokio::runtime::Handle::try_current().map_err(|e| {
        CoreError::new(ErrorCode::Io, "no async runtime").with_detail(e.to_string())
    })?;
    let git_dir = dunce::canonicalize(common_git_dir).map_err(|e| {
        CoreError::new(
            ErrorCode::PathMissing,
            format!("Folder not found: {}", common_git_dir.display()),
        )
        .with_detail(e.to_string())
    })?;
    #[cfg(feature = "testutil")]
    if crate::testutil::is_unwatchable(&git_dir) {
        return Err(watch_error(&git_dir, "made unwatchable for a test".into()));
    }
    let signal = Arc::new(Notify::new());

    let handler = {
        let git_dir = git_dir.clone();
        let signal = signal.clone();
        move |event: notify::Result<Event>| {
            // Errors (and rescan requests) may hide changes: treat them as one.
            if event.map_or(true, |e| is_change(&git_dir, &e)) {
                signal.notify_one();
            }
        }
    };
    let mut os =
        notify::recommended_watcher(handler).map_err(|e| watch_error(&git_dir, e.to_string()))?;
    let mut dirs = DirWatches::new(&git_dir);
    {
        let mut paths = os.paths_mut();
        paths
            .add(&git_dir, RecursiveMode::NonRecursive)
            .map_err(|e| watch_error(&git_dir, e.to_string()))?;
        for (dir, watched) in dirs.dirs.iter_mut() {
            if dir.is_dir() {
                paths
                    .add(dir, RecursiveMode::Recursive)
                    .map_err(|e| watch_error(dir, e.to_string()))?;
                *watched = true;
            }
        }
        paths
            .commit()
            .map_err(|e| watch_error(&git_dir, e.to_string()))?;
    }

    let os = Arc::new(Mutex::new(Some(os)));
    let task = runtime.spawn({
        let os = os.clone();
        async move {
            loop {
                wait_for_quiet(&signal).await;
                // Add watches for directories created since (and drop those of removed
                // ones) before reporting, so nothing after this point is missed.
                dirs.sync(&os, &on_error);
                on_change();
            }
        }
    });
    Ok(RepoWatcher { os, task })
}

/// Waits for a change, then until 300 ms pass without one or 1 s has passed since the first.
async fn wait_for_quiet(signal: &Notify) {
    signal.notified().await;
    let first = Instant::now();
    let latest = first + MAX_DELAY;
    let mut deadline = (first + QUIET).min(latest);
    while timeout_at(deadline, signal.notified()).await.is_ok() {
        deadline = (Instant::now() + QUIET).min(latest);
    }
}

/// The recursively watched subdirectories and whether each has a watch now.
struct DirWatches {
    dirs: Vec<(PathBuf, bool)>,
}

impl DirWatches {
    fn new(git_dir: &Path) -> Self {
        Self {
            dirs: RECURSIVE_DIRS
                .iter()
                .map(|d| (git_dir.join(d), false))
                .collect(),
        }
    }

    /// Watches the directories that now exist and unwatches those that are gone. Reports
    /// each directory that cannot be watched to `on_error` (it is tried again next time),
    /// after letting go of the watcher's lock: `on_error` may stop the watcher.
    fn sync<W: Watcher>(&mut self, os: &Mutex<Option<W>>, on_error: &dyn Fn(CoreError)) {
        let mut errors = Vec::new();
        {
            let mut os = lock(os);
            let Some(os) = os.as_mut() else {
                return;
            };
            for (dir, watched) in &mut self.dirs {
                let exists = dir.is_dir();
                if exists && !*watched {
                    match os.watch(dir, RecursiveMode::Recursive) {
                        Ok(()) => *watched = true,
                        Err(e) => errors.push(watch_error(dir, e.to_string())),
                    }
                } else if !exists && *watched {
                    let _ = os.unwatch(dir);
                    *watched = false;
                }
            }
        }
        for error in errors {
            on_error(error);
        }
    }
}

/// Whether `event` may change what the app shows. Reads (`Access`) never do; otherwise any
/// path that is not ignored counts, and an event without paths (a rescan) counts.
fn is_change(git_dir: &Path, event: &Event) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    event.paths.is_empty() || event.paths.iter().any(|p| is_relevant(git_dir, p))
}

/// Whether a change at `path` matters: anything in the git dir except `*.lock` files and
/// what lies under `objects/`, `logs/` or a linked worktree's `logs/`.
fn is_relevant(git_dir: &Path, path: &Path) -> bool {
    if path
        .file_name()
        .is_some_and(|n| n.to_string_lossy().ends_with(".lock"))
    {
        return false;
    }
    let Ok(rel) = path.strip_prefix(git_dir) else {
        return false;
    };
    let mut parts = rel.components().map(|c| c.as_os_str());
    match parts.next() {
        Some(first) if first == "objects" || first == "logs" => false,
        Some(first) if first == "worktrees" => parts.nth(1).is_none_or(|p| p != "logs"),
        _ => true,
    }
}

fn watch_error(dir: &Path, detail: String) -> CoreError {
    CoreError::new(
        ErrorCode::Io,
        format!("Could not watch {} for changes.", dir.display()),
    )
    .with_detail(detail)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, ModifyKind};

    fn relevant(rel: &str) -> bool {
        is_relevant(Path::new("/r/.git"), &Path::new("/r/.git").join(rel))
    }

    #[test]
    fn relevant_paths() {
        for rel in [
            "HEAD",
            "index",
            "packed-refs",
            "FETCH_HEAD",
            "ORIG_HEAD",
            "MERGE_HEAD",
            "config",
            "refs",
            "refs/heads/main",
            "refs/heads/logs",
            "refs/heads/objects/x",
            "refs/remotes/origin/main",
            "worktrees",
            "worktrees/wt",
            "worktrees/wt/HEAD",
            "worktrees/wt/index",
            "worktrees/logs/HEAD",
            "",
        ] {
            assert!(relevant(rel), "{rel:?} is relevant");
        }
    }

    #[test]
    fn ignored_paths() {
        for rel in [
            "objects",
            "objects/ab/cdef",
            "objects/pack/pack-1.idx",
            "logs",
            "logs/HEAD",
            "logs/refs/heads/main",
            "worktrees/wt/logs/HEAD",
            "index.lock",
            "HEAD.lock",
            "refs/heads/main.lock",
            "packed-refs.lock",
            "worktrees/wt/index.lock",
        ] {
            assert!(!relevant(rel), "{rel:?} is ignored");
        }
        // Outside the git dir.
        assert!(!is_relevant(
            Path::new("/r/.git"),
            Path::new("/r/src/main.rs")
        ));
        assert!(!is_relevant(
            Path::new("/r/.git"),
            Path::new("/r/.github/x")
        ));
    }

    #[test]
    fn reads_are_not_changes() {
        let git_dir = Path::new("/r/.git");
        let at = |kind| Event::new(kind).add_path(git_dir.join("HEAD"));
        assert!(!is_change(git_dir, &at(EventKind::Access(AccessKind::Any))));
        assert!(is_change(git_dir, &at(EventKind::Modify(ModifyKind::Any))));
        assert!(is_change(git_dir, &at(EventKind::Create(CreateKind::File))));
        assert!(is_change(git_dir, &Event::new(EventKind::Other)));
        let objects =
            Event::new(EventKind::Create(CreateKind::File)).add_path(git_dir.join("objects/ab/cd"));
        assert!(!is_change(git_dir, &objects));
        assert!(is_change(git_dir, &objects.add_path(git_dir.join("HEAD"))));
    }

    /// A watcher whose every `watch` fails.
    struct FailingWatcher;

    impl Watcher for FailingWatcher {
        fn new<F: notify::EventHandler>(_: F, _: notify::Config) -> notify::Result<Self> {
            Ok(Self)
        }
        fn watch(&mut self, _: &Path, _: RecursiveMode) -> notify::Result<()> {
            Err(notify::Error::generic("no more watches"))
        }
        fn unwatch(&mut self, _: &Path) -> notify::Result<()> {
            Ok(())
        }
        fn kind() -> notify::WatcherKind {
            notify::WatcherKind::NullWatcher
        }
    }

    #[test]
    fn late_directories_that_cannot_be_watched_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let mut dirs = DirWatches::new(dir.path());
        let os = Mutex::new(Some(FailingWatcher));
        let errors = Mutex::new(Vec::new());
        let on_error = |e: CoreError| lock(&errors).push(e);

        // Nothing exists yet: nothing to watch, nothing to report.
        dirs.sync(&os, &on_error);
        assert!(lock(&errors).is_empty());

        std::fs::create_dir(dir.path().join("worktrees")).unwrap();
        dirs.sync(&os, &on_error);
        let reported = lock(&errors).clone();
        assert_eq!(reported.len(), 1);
        assert_eq!(reported[0].code, ErrorCode::Io);
        assert!(reported[0].message.contains("worktrees"), "{reported:?}");
        assert_eq!(reported[0].detail.as_deref(), Some("no more watches"));
        // Still unwatched, so the next sync tries (and reports) again.
        dirs.sync(&os, &on_error);
        assert_eq!(lock(&errors).len(), 2);
    }

    /// `on_error` may stop the watcher (the repository closes); it must not be called with
    /// the watcher's lock held.
    #[test]
    fn errors_are_reported_without_the_watcher_lock_held() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("worktrees")).unwrap();
        let mut dirs = DirWatches::new(dir.path());
        let os = Mutex::new(Some(FailingWatcher));
        let free = Mutex::new(Vec::new());
        let on_error = |_: CoreError| lock(&free).push(os.try_lock().is_ok());
        dirs.sync(&os, &on_error);
        assert_eq!(*lock(&free), [true]);
    }

    #[tokio::test(start_paused = true)]
    async fn quiet_period_and_max_delay() {
        let signal = Notify::new();
        // One change: fires 300 ms later.
        signal.notify_one();
        let start = Instant::now();
        wait_for_quiet(&signal).await;
        assert_eq!(start.elapsed(), QUIET);

        // A change every 100 ms: fires at 1 s, not later.
        let start = Instant::now();
        signal.notify_one();
        let wait = wait_for_quiet(&signal);
        let keep_busy = async {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                signal.notify_one();
            }
        };
        tokio::select! {
            () = wait => {}
            () = keep_busy => {}
        }
        assert_eq!(start.elapsed(), MAX_DELAY);
    }
}
