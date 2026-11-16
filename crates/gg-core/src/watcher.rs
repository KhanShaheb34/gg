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
//! an ignored folder costs nothing. On Linux, where a recursive watch is one inotify watch
//! per folder (and the user's limit of them is soon reached by a `node_modules` or a
//! `target`), the folders are watched one by one instead, never `.git` or a folder git
//! ignores, and the watches follow the folders as they come and go ([`TreeMode`]).

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

/// How a worktree's files are watched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TreeMode {
    /// One recursive watch of the worktree (macOS's FSEvents, Windows' directory changes: one
    /// handle for the whole tree), its events about ignored paths left out.
    Recursive,
    /// A watch of each folder on its own, except `.git` and what git ignores (Linux's
    /// inotify, where a recursive watch would be one watch per folder all the same, the
    /// ignored ones included).
    PerFolder,
}

/// How this platform watches a worktree.
const TREE_MODE: TreeMode = if cfg!(target_os = "linux") {
    TreeMode::PerFolder
} else {
    TreeMode::Recursive
};

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
    /// Past [`MAX_BATCH`] paths, the folders the rest are in (at most [`MAX_BATCH`] of them):
    /// a folder git ignores holds only ignored paths.
    dirs: HashSet<String>,
    /// More folders than that too, or a change that cannot be placed (a rescan).
    overflow: bool,
    /// A `.gitignore` changed or a folder appeared: what git ignores is read again first.
    reread: bool,
    /// The folders made, removed or renamed in (at most [`MAX_BATCH`]; past that,
    /// `named_all`): where folders are watched one by one, they are watched again.
    named: HashSet<PathBuf>,
    /// More such folders than that: every folder is watched again.
    named_all: bool,
    /// A folder went: where folders are watched one by one, the watches follow, so a folder
    /// made again later counts as new (Task 12 review I1).
    folder_gone: bool,
}

/// Longest a read of what git ignores may take: one that hangs is given up on (git is
/// stopped), so the watch goes on.
const IGNORED_READ_LIMIT: Duration = Duration::from_secs(10);

/// Reads what git ignores in the worktree at `root` (see [`Ignored`]), giving up after
/// [`IGNORED_READ_LIMIT`].
async fn read_ignored(runner: &GitRunner, root: &Path) -> Result<Ignored> {
    tokio::time::timeout(IGNORED_READ_LIMIT, read_ignored_now(runner, root))
        .await
        .map_err(|_| {
            CoreError::new(
                ErrorCode::GitFailed,
                "Reading what git ignores took too long.",
            )
        })?
}

async fn read_ignored_now(runner: &GitRunner, root: &Path) -> Result<Ignored> {
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

/// The folders of the worktree at `root` to watch one by one ([`TreeMode::PerFolder`]): `root`
/// and every folder under it, reached without following a link (nothing outside the
/// worktree), except any `.git` (the repository's own files, a submodule's) and what
/// `ignored` covers.
fn folders_to_watch(root: &Path, ignored: &Ignored) -> Vec<PathBuf> {
    let mut out = vec![root.to_path_buf()];
    let mut next = 0;
    while next < out.len() {
        let dir = out[next].clone();
        next += 1;
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue; // gone meanwhile, or unreadable
        };
        for entry in entries.flatten() {
            // The entry's own type: a link to a folder is not a folder here.
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let path = entry.path();
            if tree_relative(root, &path).is_some_and(|rel| !ignored.covers(&rel)) {
                out.push(path);
            }
        }
    }
    out
}

/// The folders watched one by one.
type Watched = HashSet<PathBuf>;

/// Which of the folders already watched are watched again by [`sync_folders`].
#[derive(Debug, Clone, Copy)]
enum Renew<'a> {
    /// Those named in a batch of changes (made, removed or renamed), and the folders inside
    /// them: one removed and made again under the same name is another folder, whose watch the
    /// platform has not got (Linux drops a removed folder's watch; the new folder may even have
    /// the old one's inode, so it cannot be told apart by its identity), and so are the folders
    /// made inside it before it was watched, which no event names.
    Named(&'a HashSet<PathBuf>),
    /// Every one: changes were lost, so which folders were made again is not known.
    All,
}

/// Makes the folders `os` watches one by one (`watched`) those of `wanted`: the watches of
/// folders no longer wanted (gone, or ignored now) are removed, the new ones added, and the
/// ones `renew` names watched again (watching a folder that is watched already changes
/// nothing). A folder that is there and cannot be watched (the platform's limit reached) is
/// `Err`: its changes would be missed. Blocking (a system call per folder).
fn sync_folders<W: Watcher>(
    os: &mut W,
    watched: &mut Watched,
    wanted: Vec<PathBuf>,
    renew: Renew<'_>,
) -> Result<()> {
    let wanted: HashSet<PathBuf> = wanted.into_iter().collect();
    let gone: Vec<PathBuf> = watched
        .iter()
        .filter(|d| !wanted.contains(*d))
        .cloned()
        .collect();
    for dir in gone {
        // The OS may have dropped the watch of a removed folder already.
        let _ = os.unwatch(&dir);
        watched.remove(&dir);
    }
    let mut failed = None;
    for dir in wanted {
        let again = match renew {
            Renew::All => true,
            Renew::Named(named) => dir.ancestors().any(|d| named.contains(d)),
        };
        if watched.contains(&dir) && !again {
            continue;
        }
        match os.watch(&dir, RecursiveMode::NonRecursive) {
            Ok(()) => {
                watched.insert(dir);
            }
            Err(e) if dir.is_dir() => {
                failed.get_or_insert_with(|| watch_error(&dir, e.to_string()));
            }
            Err(_) => {
                watched.remove(&dir); // gone meanwhile
            }
        }
    }
    failed.map_or(Ok(()), Err)
}

/// Notes in `batch` the paths of `event` under `root` that are not in `ignored`; whether any
/// was noted.
fn note_tree_event(root: &Path, ignored: &Ignored, event: &Event, batch: &mut Batch) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    if event.paths.is_empty() {
        batch.overflow = true;
        batch.reread = true;
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
        let folder_appeared = appeared && path.is_dir();
        // A removed path cannot be looked at; only a file's removal is known to be one.
        let folder_removed = matches!(event.kind, EventKind::Remove(kind)
            if kind != notify::event::RemoveKind::File);
        batch.folder_gone |= folder_removed;
        if folder_appeared || folder_removed {
            if batch.named.len() < MAX_BATCH {
                batch.named.insert(path.clone());
            } else {
                batch.named_all = true;
            }
        }
        if path.file_name().is_some_and(|n| n == ".gitignore") || folder_appeared {
            batch.reread = true;
        }
        if batch.paths.len() < MAX_BATCH {
            batch.paths.insert(rel);
        } else {
            // Many changes at once (a build, an install): judged by their folders, against
            // what git ignores read again first.
            batch.reread = true;
            let dir = rel.rsplit_once('/').map_or("", |(dir, _)| dir).to_owned();
            // A folder at the top changed itself (Windows says a folder changed whenever a
            // file in it does): judged by the folder.
            let dir = if dir.is_empty() && path.is_dir() {
                rel.clone()
            } else {
                dir
            };
            if dir.is_empty() || (batch.dirs.len() >= MAX_BATCH && !batch.dirs.contains(&dir)) {
                batch.overflow = true;
            } else {
                batch.dirs.insert(dir);
            }
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
    watch_worktree_reporting(root, runner, on_change, |_| {}).await
}

/// Like [`watch_worktree`], and calls `on_error` (on the debounce task) each time a folder
/// that appeared later cannot be watched (where folders are watched one by one: the
/// platform's limit reached), so changes in it will be missed.
pub async fn watch_worktree_reporting(
    root: &Path,
    runner: GitRunner,
    on_change: impl Fn() + Send + Sync + 'static,
    on_error: impl Fn(CoreError) + Send + Sync + 'static,
) -> Result<TreeWatcher> {
    watch_worktree_in(TREE_MODE, root, runner, on_change, on_error).await
}

/// [`watch_worktree_reporting`] in `mode`.
pub(crate) async fn watch_worktree_in(
    mode: TreeMode,
    root: &Path,
    runner: GitRunner,
    on_change: impl Fn() + Send + Sync + 'static,
    on_error: impl Fn(CoreError) + Send + Sync + 'static,
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
                    let mut batch = lock(&batch);
                    batch.overflow = true;
                    batch.reread = true;
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
    // Adding a recursive watch walks the whole tree on some platforms, and the folders to
    // watch one by one are found by walking it: on the blocking pool. Links are not
    // followed, so nothing outside the worktree is watched.
    let (os, watched) = {
        let at = root.clone();
        let skip = ignored.read().unwrap_or_else(|e| e.into_inner()).clone();
        tokio::task::spawn_blocking(move || {
            let root = at;
            let config = notify::Config::default().with_follow_symlinks(false);
            let mut os = RecommendedWatcher::new(handler, config)
                .map_err(|e| watch_error(&root, e.to_string()))?;
            let mut watched = Watched::new();
            match mode {
                TreeMode::Recursive => os
                    .watch(&root, RecursiveMode::Recursive)
                    .map_err(|e| watch_error(&root, e.to_string()))?,
                TreeMode::PerFolder => {
                    let wanted = folders_to_watch(&root, &skip);
                    sync_folders(&mut os, &mut watched, wanted, Renew::All)?;
                }
            }
            Ok::<_, CoreError>((os, watched))
        })
        .await
        .map_err(|e| watch_error(&root, e.to_string()))??
    };
    let os = Arc::new(Mutex::new(Some(os)));
    let task_os = os.clone();
    let mut watched = watched;
    let task = runtime.spawn(async move {
        loop {
            wait_for_quiet(&signal, TREE_QUIET, TREE_MAX_DELAY).await;
            let mut batch = std::mem::take(&mut *lock(&batch));
            if batch.reread
                && let Ok(fresh) = read_ignored(&runner, &root).await
            {
                *ignored.write().unwrap_or_else(|e| e.into_inner()) = fresh;
            }
            // Folders came, went or are ignored now: the watches follow (on the blocking
            // pool: the tree is walked and each folder watched).
            if mode == TreeMode::PerFolder && (batch.reread || batch.overflow || batch.folder_gone)
            {
                let skip = ignored.read().unwrap_or_else(|e| e.into_inner()).clone();
                let (at, os) = (root.clone(), task_os.clone());
                let all = batch.overflow || batch.named_all;
                let named = std::mem::take(&mut batch.named);
                let mut kept = std::mem::take(&mut watched);
                let synced = tokio::task::spawn_blocking(move || {
                    let wanted = folders_to_watch(&at, &skip);
                    let renew = if all {
                        Renew::All
                    } else {
                        Renew::Named(&named)
                    };
                    let result = match lock(&os).as_mut() {
                        Some(os) => sync_folders(os, &mut kept, wanted, renew),
                        None => Ok(()),
                    };
                    (kept, result)
                })
                .await;
                if let Ok((kept, result)) = synced {
                    watched = kept;
                    if let Err(e) = result {
                        on_error(e);
                    }
                }
            }
            let relevant = batch.overflow || {
                let ignored = ignored.read().unwrap_or_else(|e| e.into_inner());
                batch.paths.iter().any(|p| !ignored.covers(p))
                    || batch.dirs.iter().any(|d| !ignored.covers(d))
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

    /// Records the folders it is asked to watch and to stop watching; refuses `refuse`.
    #[derive(Default)]
    struct Recorder {
        watches: Vec<(PathBuf, RecursiveMode)>,
        unwatches: Vec<PathBuf>,
        refuse: Option<PathBuf>,
    }

    impl Watcher for Recorder {
        fn new<F: notify::EventHandler>(_: F, _: notify::Config) -> notify::Result<Self> {
            Ok(Self::default())
        }
        fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
            if self.refuse.as_deref() == Some(path) {
                return Err(notify::Error::generic("no more watches"));
            }
            self.watches.push((path.to_owned(), mode));
            Ok(())
        }
        fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
            self.unwatches.push(path.to_owned());
            Ok(())
        }
        fn kind() -> notify::WatcherKind {
            notify::WatcherKind::NullWatcher
        }
    }

    /// Linux's per-folder watch: every folder but `.git`, what git ignores and what is only
    /// reached through a link (Task 11; Task 3 review concern 3).
    #[test]
    fn folders_watched_one_by_one_leave_out_git_and_ignored_ones() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        for d in [
            ".git/objects/ab",
            "src/app",
            "node_modules/x/y",
            "target/debug",
            "docs",
            "sub/.git/refs",
            "sub/lib",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("docs/.git"), "gitdir: elsewhere").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("src"), root.join("linked")).unwrap();
        let ignored = Ignored::parse(b"node_modules/\0target/\0");
        let mut found: Vec<String> = folders_to_watch(&root, &ignored)
            .iter()
            .map(|p| tree_relative(&root, p).unwrap_or_default())
            .collect();
        found.sort();
        assert_eq!(found, ["", "docs", "src", "src/app", "sub", "sub/lib"]);
    }

    #[test]
    fn folder_watches_follow_the_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let (a, b) = (root.join("a"), root.join("b"));
        std::fs::create_dir(&a).unwrap();
        let none = HashSet::new();
        let mut os = Recorder::default();
        let mut watched = Watched::new();
        sync_folders(
            &mut os,
            &mut watched,
            vec![root.clone(), a.clone()],
            Renew::All,
        )
        .unwrap();
        let mut first = os.watches.clone();
        first.sort_by(|x, y| x.0.cmp(&y.0));
        assert_eq!(
            first,
            [
                (root.clone(), RecursiveMode::NonRecursive),
                (a.clone(), RecursiveMode::NonRecursive)
            ]
        );
        assert_eq!(os.watches.len(), 2);
        // The same again, nothing named: nothing new.
        let same = vec![root.clone(), a.clone()];
        sync_folders(&mut os, &mut watched, same, Renew::Named(&none)).unwrap();
        assert_eq!(os.watches.len(), 2);
        // `a` gone (or ignored now), `b` new.
        std::fs::remove_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let named = HashSet::from([a.clone(), b.clone()]);
        let now = vec![root.clone(), b.clone()];
        sync_folders(&mut os, &mut watched, now, Renew::Named(&named)).unwrap();
        assert_eq!(os.unwatches, std::slice::from_ref(&a));
        assert_eq!(
            os.watches.last(),
            Some(&(b.clone(), RecursiveMode::NonRecursive))
        );
        assert_eq!(os.watches.len(), 3);
        // `b` removed and made again under the same name (on Linux, often with the same
        // inode): named in the batch, so watched again, whatever its identity.
        std::fs::remove_dir(&b).unwrap();
        std::fs::create_dir(&b).unwrap();
        let named = HashSet::from([b.clone()]);
        let now = vec![root.clone(), b.clone()];
        sync_folders(&mut os, &mut watched, now, Renew::Named(&named)).unwrap();
        assert_eq!(os.watches.len(), 4);
        assert_eq!(
            os.watches.last(),
            Some(&(b.clone(), RecursiveMode::NonRecursive))
        );
        // Changes lost: every folder is watched again.
        let now = vec![root.clone(), b.clone()];
        sync_folders(&mut os, &mut watched, now, Renew::All).unwrap();
        assert_eq!(os.watches.len(), 6);
        // A folder that cannot be watched is said: its changes would be missed.
        std::fs::create_dir(&a).unwrap();
        os.refuse = Some(a.clone());
        let err = sync_folders(
            &mut os,
            &mut watched,
            vec![root.clone(), a.clone(), b.clone()],
            Renew::Named(&none),
        )
        .unwrap_err();
        assert!(err.message.contains("Could not watch"), "{err:?}");
        assert_eq!(err.detail.as_deref(), Some("no more watches"));
    }

    /// Where folders are watched one by one, a batch names the folders made, removed or
    /// renamed, so a folder made again under an old name is watched again.
    #[test]
    fn a_batch_names_the_folders_made_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let made = root.join("made");
        std::fs::create_dir(&made).unwrap();
        let ignored = Ignored::default();
        let mut batch = Batch::default();
        let event = |kind, path: &Path| Event::new(kind).add_path(path.to_path_buf());
        let removed = root.join("removed");
        note_tree_event(
            &root,
            &ignored,
            &event(
                EventKind::Remove(notify::event::RemoveKind::Folder),
                &removed,
            ),
            &mut batch,
        );
        note_tree_event(
            &root,
            &ignored,
            &event(EventKind::Create(CreateKind::Folder), &made),
            &mut batch,
        );
        for (kind, name) in [
            (EventKind::Modify(ModifyKind::Any), "file"),
            (EventKind::Create(CreateKind::File), "new-file"),
            (
                EventKind::Remove(notify::event::RemoveKind::File),
                "old-file",
            ),
        ] {
            std::fs::write(root.join(name), "").unwrap();
            note_tree_event(&root, &ignored, &event(kind, &root.join(name)), &mut batch);
        }
        assert_eq!(batch.named, HashSet::from([removed, made]));
        assert!(!batch.named_all);
        assert!(batch.reread);
        assert!(batch.folder_gone);

        // A batch that only removes a folder still has the watches follow.
        let mut only_removed = Batch::default();
        note_tree_event(
            &root,
            &ignored,
            &event(
                EventKind::Remove(notify::event::RemoveKind::Any),
                &root.join("gone"),
            ),
            &mut only_removed,
        );
        assert!(only_removed.folder_gone && !only_removed.reread);
        let mut a_file = Batch::default();
        note_tree_event(
            &root,
            &ignored,
            &event(
                EventKind::Remove(notify::event::RemoveKind::File),
                &root.join("old-file"),
            ),
            &mut a_file,
        );
        assert!(!a_file.folder_gone);
    }

    /// `new/sub` removed in one batch (`git stash -u`) and made again in a later one (`git
    /// stash pop`): only `new` is named then, `sub` having been made before `new` was watched;
    /// both are watched again (Task 12 review I1).
    #[test]
    fn folders_inside_a_named_one_are_watched_again() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let (new, sub) = (root.join("new"), root.join("new").join("sub"));
        std::fs::create_dir_all(&sub).unwrap();
        let mut os = Recorder::default();
        let mut watched = Watched::new();
        let all = vec![root.clone(), new.clone(), sub.clone()];
        sync_folders(&mut os, &mut watched, all.clone(), Renew::All).unwrap();
        assert_eq!(os.watches.len(), 3);
        // Removed: the watches follow.
        std::fs::remove_dir_all(&new).unwrap();
        let named = HashSet::from([new.clone()]);
        sync_folders(
            &mut os,
            &mut watched,
            vec![root.clone()],
            Renew::Named(&named),
        )
        .unwrap();
        assert_eq!(watched.len(), 1);
        // Made again, only `new` named: `sub` too is watched.
        std::fs::create_dir_all(&sub).unwrap();
        sync_folders(&mut os, &mut watched, all.clone(), Renew::Named(&named)).unwrap();
        assert_eq!(os.watches.len(), 5);
        assert!(os.watches[3..].iter().any(|(d, _)| *d == sub));
        // Even had the removal gone unseen (both still in `watched`), naming `new` renews
        // `sub` with it.
        sync_folders(&mut os, &mut watched, all, Renew::Named(&named)).unwrap();
        assert_eq!(os.watches.len(), 7);
        assert!(os.watches[5..].iter().any(|(d, _)| *d == sub));
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

    /// Past [`MAX_BATCH`] paths, the folders of the rest are kept, and what git ignores is
    /// read again before they are judged.
    #[test]
    fn many_changes_are_kept_by_folder() {
        let root = Path::new("/w");
        let ignored = Ignored::default();
        let write = EventKind::Create(CreateKind::File);
        let mut batch = Batch::default();
        for i in 0..MAX_BATCH + 500 {
            let event = Event::new(write).add_path(root.join(format!("gen/{}/f{i}", i % 3)));
            assert!(note_tree_event(root, &ignored, &event, &mut batch));
        }
        assert_eq!(batch.paths.len(), MAX_BATCH);
        assert_eq!(batch.dirs.len(), 3, "{:?}", batch.dirs);
        assert!(!batch.overflow);
        assert!(batch.reread);
        let fresh = Ignored::parse(b"gen/\0");
        assert!(batch.paths.iter().all(|p| fresh.covers(p)));
        assert!(batch.dirs.iter().all(|d| fresh.covers(d)));
        // A top-level file past the limit cannot be judged by folder.
        let top = Event::new(write).add_path(root.join("loose.txt"));
        note_tree_event(root, &ignored, &top, &mut batch);
        assert!(batch.overflow);
    }

    /// Past the limit, a folder at the top that changed itself (as Windows reports a folder
    /// whose files change) is judged by that folder, not taken as a change that cannot be
    /// placed.
    #[test]
    fn a_top_folder_that_changed_is_kept_by_itself() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("gen")).unwrap();
        let ignored = Ignored::default();
        let mut batch = Batch::default();
        for i in 0..MAX_BATCH {
            let event = Event::new(EventKind::Create(CreateKind::File))
                .add_path(root.join(format!("gen/f{i}")));
            note_tree_event(&root, &ignored, &event, &mut batch);
        }
        let changed = Event::new(EventKind::Modify(ModifyKind::Any)).add_path(root.join("gen"));
        note_tree_event(&root, &ignored, &changed, &mut batch);
        assert!(!batch.overflow);
        assert!(batch.dirs.contains("gen"), "{:?}", batch.dirs);
        assert!(Ignored::parse(b"gen/\0").covers("gen"));
    }

    /// The per-folder mode end to end (the OS's own watcher, folder by folder): an edit in a
    /// folder made after the start is seen; a build writing into an ignored folder is not.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn per_folder_watching_follows_new_folders_and_skips_ignored_ones() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let repo = crate::testutil::TestRepo::new();
        repo.write(".gitignore", "build/\n");
        repo.commit_all("ignore build");
        std::fs::create_dir_all(repo.path().join("build")).unwrap();
        let runner = crate::git::runner::GitRunner::detect(crate::git::runner::CommandLog::new())
            .await
            .unwrap();
        let fired = Arc::new(AtomicUsize::new(0));
        let count = fired.clone();
        let _watcher = watch_worktree_in(
            TreeMode::PerFolder,
            repo.path(),
            runner,
            move || {
                count.fetch_add(1, Ordering::SeqCst);
            },
            |_| {},
        )
        .await
        .unwrap();
        let settle = Duration::from_millis(1500);
        let seen = |n: usize| {
            let fired = fired.clone();
            async move {
                let until = std::time::Instant::now() + Duration::from_secs(10);
                while std::time::Instant::now() < until {
                    if fired.load(Ordering::SeqCst) > n {
                        return true;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                false
            }
        };
        tokio::time::sleep(settle).await;
        let base = fired.load(Ordering::SeqCst);

        // A new folder (seen), then a file in it, once its watch is there (seen again).
        std::fs::create_dir_all(repo.path().join("src/new")).unwrap();
        assert!(seen(base).await, "the new folder");
        tokio::time::sleep(settle).await;
        let after_folder = fired.load(Ordering::SeqCst);
        std::fs::write(repo.path().join("src/new/file.txt"), "x").unwrap();
        assert!(seen(after_folder).await, "a file in the new folder");

        // A build writing into the ignored folder: nothing.
        tokio::time::sleep(settle).await;
        let quiet = fired.load(Ordering::SeqCst);
        for i in 0..50 {
            std::fs::write(repo.path().join(format!("build/out{i}.o")), "x").unwrap();
        }
        tokio::time::sleep(settle * 2).await;
        assert_eq!(
            fired.load(Ordering::SeqCst),
            quiet,
            "the ignored folder fired"
        );
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
