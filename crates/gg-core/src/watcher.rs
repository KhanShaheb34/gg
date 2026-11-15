//! Watches a repository's git directory, so that changes made outside the app (a commit in
//! a terminal, an agent working in a linked worktree) show up without a manual refresh.
//!
//! Watched: the common git dir itself, non-recursively (`HEAD`, `index`, `packed-refs`,
//! `FETCH_HEAD`, `config`, ...), and `refs/` and `worktrees/` recursively while they exist.
//! Linked worktrees keep their `HEAD` and `index` under `worktrees/<name>/`. A stopped
//! merge, cherry-pick or revert (`MERGE_HEAD`, ...) and a rebase (`rebase-merge/`,
//! `rebase-apply/`) appear and go away directly in the git dir, or in a linked worktree's. Changes under
//! `objects/` and `logs/` and to `*.lock` files are ignored, so a `git gc` or a large fetch
//! does not look like a stream of changes.
//!
//! The OS watcher's callback never blocks: it only notes what changed and wakes a Tokio task,
//! which debounces and then calls `on_change` with what changed ([`GitDirChange`]). Nothing
//! runs while nothing changes.
//!
//! The active worktree's files are watched too ([`watch_worktree`]): recursively, leaving out
//! `.git` and what git ignores, so an edit shows up as a status read and a build writing into
//! an ignored folder costs nothing.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use notify::event::EventKind;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout_at};

use crate::error::{CoreError, ErrorCode, Result};
use crate::git::runner::{CmdKind, GitRunner};

/// `on_change` waits for this much quiet after the last change.
const QUIET: Duration = Duration::from_millis(300);
/// ...but during continuous activity fires at least this often.
const MAX_DELAY: Duration = Duration::from_secs(1);
/// Subdirectories of the git dir watched recursively while they exist.
const RECURSIVE_DIRS: [&str; 2] = ["refs", "worktrees"];

/// What changed in a git directory over one debounce period.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitDirChange {
    /// The main worktree's own files changed (`HEAD`, `index`, `MERGE_HEAD`, `rebase-merge/`,
    /// … at the top of the git dir).
    pub main: bool,
    /// The admin folders under `worktrees/` (by name) in which something changed: a linked
    /// worktree's `HEAD`, `index` or in-progress files.
    pub worktrees: BTreeSet<String>,
    /// Refs, `packed-refs`, `FETCH_HEAD` or `config` changed.
    pub refs: bool,
    /// Something changed that cannot be placed (the OS asked for a rescan, or failed):
    /// anything may have.
    pub all: bool,
}

impl GitDirChange {
    /// Notes a change at `rel`, a path relative to the git dir that [`is_relevant`] keeps.
    fn note(&mut self, rel: &Path) {
        let mut parts = rel.components().map(|c| c.as_os_str());
        match parts.next() {
            Some(first) if first == "worktrees" => match parts.next() {
                Some(name) => {
                    self.worktrees.insert(name.to_string_lossy().into_owned());
                }
                // The folder itself: a worktree came or went.
                None => self.refs = true,
            },
            Some(first)
                if first == "refs"
                    || first == "packed-refs"
                    || first == "FETCH_HEAD"
                    || first == "config" =>
            {
                self.refs = true;
            }
            Some(_) => self.main = true,
            None => self.all = true,
        }
    }
}

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
    watch_git_dir_reporting(common_git_dir, move |_| on_change(), |_| {})
}

/// Like [`watch_git_dir`], with what changed given to `on_change`, and calls `on_error` (on
/// the debounce task) each time a directory created later (`refs/`, `worktrees/`) cannot be
/// watched, so changes inside it will be missed. Failing to watch anything that exists at the
/// start is an `Err` instead.
pub fn watch_git_dir_reporting(
    common_git_dir: &Path,
    on_change: impl Fn(GitDirChange) + Send + Sync + 'static,
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
    let changed = Arc::new(Mutex::new(GitDirChange::default()));

    let handler = {
        let git_dir = git_dir.clone();
        let (signal, changed) = (signal.clone(), changed.clone());
        move |event: notify::Result<Event>| {
            // Errors (and rescan requests) may hide changes: treat them as one of anything.
            let noted = match event {
                Err(_) => {
                    lock(&changed).all = true;
                    true
                }
                Ok(e) => note_change(&git_dir, &e, &mut lock(&changed)),
            };
            if noted {
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
                wait_for_quiet(&signal, QUIET, MAX_DELAY).await;
                // Add watches for directories created since (and drop those of removed
                // ones) before reporting, so nothing after this point is missed.
                dirs.sync(&os, &on_error);
                let change = std::mem::take(&mut *lock(&changed));
                on_change(change);
            }
        }
    });
    Ok(RepoWatcher { os, task })
}

/// Waits for a change, then until `quiet` passes without one or `max` has passed since the
/// first.
async fn wait_for_quiet(signal: &Notify, quiet: Duration, max: Duration) {
    signal.notified().await;
    let first = Instant::now();
    let latest = first + max;
    let mut deadline = (first + quiet).min(latest);
    while timeout_at(deadline, signal.notified()).await.is_ok() {
        deadline = (Instant::now() + quiet).min(latest);
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
#[cfg(test)]
fn is_change(git_dir: &Path, event: &Event) -> bool {
    note_change(git_dir, event, &mut GitDirChange::default())
}

/// Notes in `changed` what `event` changed, if it may change what the app shows (see
/// [`is_change`]); whether it may.
fn note_change(git_dir: &Path, event: &Event, changed: &mut GitDirChange) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    if event.paths.is_empty() {
        changed.all = true;
        return true;
    }
    let mut any = false;
    for path in &event.paths {
        if is_relevant(git_dir, path)
            && let Ok(rel) = path.strip_prefix(git_dir)
        {
            changed.note(rel);
            any = true;
        }
    }
    any
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

/// A working tree's events wait for this much quiet after the last one...
const TREE_QUIET: Duration = Duration::from_millis(500);
/// ...but during continuous activity are reported at least this often.
const TREE_MAX_DELAY: Duration = Duration::from_secs(2);
/// Most changed paths kept per debounce period; past it, the period counts as a change.
const MAX_BATCH: usize = 4096;

/// Watches a worktree's files until dropped, like [`RepoWatcher`].
pub struct TreeWatcher {
    os: Arc<Mutex<Option<RecommendedWatcher>>>,
    task: JoinHandle<()>,
}

impl Drop for TreeWatcher {
    fn drop(&mut self) {
        self.task.abort();
        drop(lock(&self.os).take());
    }
}

/// What git ignores in a worktree, as `git ls-files -o -i --exclude-standard --directory`
/// lists it: whole folders, and files outside them; paths relative to the worktree, `/`
/// between parts.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Ignored {
    dirs: HashSet<String>,
    files: HashSet<String>,
}

impl Ignored {
    /// Reads git's NUL-separated list (folders end with `/`).
    fn parse(out: &[u8]) -> Self {
        let mut ignored = Self::default();
        for entry in out.split(|&b| b == 0).filter(|e| !e.is_empty()) {
            let entry = String::from_utf8_lossy(entry);
            match entry.strip_suffix('/') {
                Some(dir) => ignored.dirs.insert(dir.to_owned()),
                None => ignored.files.insert(entry.into_owned()),
            };
        }
        ignored
    }

    /// Whether `rel` is ignored: listed, or inside a listed folder.
    fn covers(&self, rel: &str) -> bool {
        self.files.contains(rel)
            || self.dirs.contains(rel)
            || rel
                .match_indices('/')
                .any(|(at, _)| self.dirs.contains(&rel[..at]))
    }
}

/// The changes of one debounce period that were not known to be ignored when they came.
#[derive(Debug, Default)]
struct Batch {
    paths: HashSet<String>,
    /// More than [`MAX_BATCH`] paths, or a change that cannot be placed (a rescan).
    overflow: bool,
    /// A `.gitignore` changed or a folder appeared: what git ignores is read again first.
    reread: bool,
}

/// Reads what git ignores in the worktree at `root` (see [`Ignored`]).
async fn read_ignored(runner: &GitRunner, root: &Path) -> Result<Ignored> {
    let out = runner
        .run(
            root,
            &[
                "ls-files",
                "-z",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
            ],
            CmdKind::Read,
        )
        .await?;
    Ok(Ignored::parse(&out.stdout))
}

/// `path` relative to `root`, parts joined with `/`; `None` outside it, for `root` itself,
/// and inside any `.git` (the repository's own files, a submodule's).
fn tree_relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut out = String::new();
    for part in rel.components() {
        let part = part.as_os_str();
        if part == ".git" {
            return None;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(&part.to_string_lossy());
    }
    (!out.is_empty()).then_some(out)
}

/// Notes in `batch` the paths of `event` under `root` that are not in `ignored`; whether any
/// was noted.
fn note_tree_event(root: &Path, ignored: &Ignored, event: &Event, batch: &mut Batch) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    if event.paths.is_empty() {
        batch.overflow = true;
        return true;
    }
    let mut any = false;
    for path in &event.paths {
        let Some(rel) = tree_relative(root, path) else {
            continue;
        };
        if ignored.covers(&rel) {
            continue;
        }
        let appeared = matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(notify::event::ModifyKind::Name(_))
        );
        if path.file_name().is_some_and(|n| n == ".gitignore") || (appeared && path.is_dir()) {
            batch.reread = true;
        }
        if batch.paths.len() < MAX_BATCH {
            batch.paths.insert(rel);
        } else {
            batch.overflow = true;
        }
        any = true;
    }
    any
}

/// Watches the files of the worktree at `root`, recursively, and calls `on_change` (on a
/// Tokio task) 500 ms after the last change, at least every 2 s while changes keep coming.
/// Changes inside `.git` and in what git ignores are left out: the ignored list is read
/// once at the start (with `runner`), and again when a `.gitignore` changes or a folder
/// appears (it may be a new build folder), before the changes that came with it are judged.
/// `Err` when the platform will not watch the tree (too many files for its limits, say).
/// Must be called inside a Tokio runtime.
pub async fn watch_worktree(
    root: &Path,
    runner: GitRunner,
    on_change: impl Fn() + Send + Sync + 'static,
) -> Result<TreeWatcher> {
    let runtime = tokio::runtime::Handle::try_current().map_err(|e| {
        CoreError::new(ErrorCode::Io, "no async runtime").with_detail(e.to_string())
    })?;
    let root = dunce::canonicalize(root).map_err(|e| {
        CoreError::new(
            ErrorCode::PathMissing,
            format!("Folder not found: {}", root.display()),
        )
        .with_detail(e.to_string())
    })?;
    #[cfg(feature = "testutil")]
    if crate::testutil::is_unwatchable(&root) {
        return Err(watch_error(&root, "made unwatchable for a test".into()));
    }
    // Without the list, nothing is left out but `.git`: more status reads, never fewer.
    let ignored = Arc::new(RwLock::new(
        read_ignored(&runner, &root).await.unwrap_or_default(),
    ));
    let signal = Arc::new(Notify::new());
    let batch = Arc::new(Mutex::new(Batch::default()));
    let handler = {
        let (root, ignored) = (root.clone(), ignored.clone());
        let (signal, batch) = (signal.clone(), batch.clone());
        move |event: notify::Result<Event>| {
            let noted = match event {
                Err(_) => {
                    lock(&batch).overflow = true;
                    true
                }
                Ok(e) => {
                    let ignored = ignored.read().unwrap_or_else(|e| e.into_inner());
                    note_tree_event(&root, &ignored, &e, &mut lock(&batch))
                }
            };
            if noted {
                signal.notify_one();
            }
        }
    };
    let mut os =
        notify::recommended_watcher(handler).map_err(|e| watch_error(&root, e.to_string()))?;
    os.watch(&root, RecursiveMode::Recursive)
        .map_err(|e| watch_error(&root, e.to_string()))?;
    let os = Arc::new(Mutex::new(Some(os)));
    let task = runtime.spawn(async move {
        loop {
            wait_for_quiet(&signal, TREE_QUIET, TREE_MAX_DELAY).await;
            let batch = std::mem::take(&mut *lock(&batch));
            if batch.reread
                && let Ok(fresh) = read_ignored(&runner, &root).await
            {
                *ignored.write().unwrap_or_else(|e| e.into_inner()) = fresh;
            }
            let relevant = batch.overflow || {
                let ignored = ignored.read().unwrap_or_else(|e| e.into_inner());
                batch.paths.iter().any(|p| !ignored.covers(p))
            };
            if relevant {
                on_change();
            }
        }
    });
    Ok(TreeWatcher { os, task })
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
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "rebase-merge",
            "rebase-merge/msgnum",
            "rebase-apply",
            "rebase-apply/next",
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
            "worktrees/wt/MERGE_HEAD",
            "worktrees/wt/rebase-merge",
            "worktrees/wt/rebase-merge/msgnum",
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

    #[test]
    fn git_dir_changes_are_placed() {
        let git_dir = Path::new("/r/.git");
        let mut change = GitDirChange::default();
        for rel in [
            "worktrees/a/index",
            "worktrees/b/rebase-merge/msgnum",
            "worktrees/a/HEAD",
        ] {
            change.note(Path::new(rel));
        }
        assert_eq!(
            change,
            GitDirChange {
                worktrees: ["a".to_owned(), "b".to_owned()].into(),
                ..Default::default()
            }
        );
        let mut change = GitDirChange::default();
        change.note(Path::new("refs/remotes/origin/main"));
        change.note(Path::new("FETCH_HEAD"));
        assert!(change.refs && !change.main && !change.all);
        change.note(Path::new("MERGE_HEAD"));
        assert!(change.main);
        let mut change = GitDirChange::default();
        assert!(note_change(
            git_dir,
            &Event::new(EventKind::Other),
            &mut change
        ));
        assert!(change.all);
        let mut change = GitDirChange::default();
        let objects =
            Event::new(EventKind::Create(CreateKind::File)).add_path(git_dir.join("objects/ab"));
        assert!(!note_change(git_dir, &objects, &mut change));
        assert_eq!(change, GitDirChange::default());
    }

    #[test]
    fn ignored_folders_and_files_cover_what_is_in_them() {
        let ignored = Ignored::parse("target/\0build/out/\0debug.log\0a b/ü.tmp\0\0".as_bytes());
        for rel in [
            "target",
            "target/debug/x.o",
            "build/out",
            "build/out/deep/file",
            "debug.log",
            "a b/ü.tmp",
        ] {
            assert!(ignored.covers(rel), "{rel} is ignored");
        }
        for rel in [
            "src/main.rs",
            "targets",
            "target2/x",
            "build",
            "build/in/x",
            "x/debug.log",
        ] {
            assert!(!ignored.covers(rel), "{rel} is not ignored");
        }
    }

    #[test]
    fn tree_events_leave_out_git_and_ignored_paths() {
        let root = Path::new("/w");
        let ignored = Ignored::parse(b"target/\0");
        let event = |kind, rel: &str| Event::new(kind).add_path(root.join(rel));
        let write = EventKind::Modify(ModifyKind::Any);
        let mut batch = Batch::default();
        for rel in [".git/index", "sub/.git/HEAD", "target/debug/x", "target"] {
            let noted = note_tree_event(root, &ignored, &event(write, rel), &mut batch);
            assert!(!noted, "{rel}");
        }
        let read = event(EventKind::Access(AccessKind::Any), "src/a.rs");
        assert!(!note_tree_event(root, &ignored, &read, &mut batch));
        let outside = Event::new(write).add_path(PathBuf::from("/elsewhere/x"));
        assert!(!note_tree_event(root, &ignored, &outside, &mut batch));
        assert!(batch.paths.is_empty() && !batch.overflow && !batch.reread);
        assert!(note_tree_event(
            root,
            &ignored,
            &event(write, "src/a.rs"),
            &mut batch
        ));
        assert!(note_tree_event(
            root,
            &ignored,
            &event(write, ".gitignore"),
            &mut batch
        ));
        assert_eq!(batch.paths.len(), 2);
        assert!(batch.reread, "a .gitignore change rereads what is ignored");
        let rescan = Event::new(EventKind::Other);
        assert!(note_tree_event(root, &ignored, &rescan, &mut batch));
        assert!(batch.overflow, "a rescan counts as a change");
    }

    #[tokio::test(start_paused = true)]
    async fn quiet_period_and_max_delay() {
        let signal = Notify::new();
        // One change: fires 300 ms later.
        signal.notify_one();
        let start = Instant::now();
        wait_for_quiet(&signal, QUIET, MAX_DELAY).await;
        assert_eq!(start.elapsed(), QUIET);

        // A change every 100 ms: fires at 1 s, not later.
        let start = Instant::now();
        signal.notify_one();
        let wait = wait_for_quiet(&signal, QUIET, MAX_DELAY);
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
