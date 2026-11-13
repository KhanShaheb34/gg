//! `RepoManager` opens repositories, loads each one's log in a background task, serves rows
//! and other queries from memory, and refreshes on request, when the git dir changes and
//! when the window gains focus. It also fetches, on request and on a schedule.
//!
//! Each repository's state sits behind a `tokio::sync::RwLock`. Queries take the read lock
//! briefly; the loader takes the write lock once per batch. Refreshes and view-option
//! changes of one repository are serialized by a separate mutex, so they never wait on a
//! load either. Events are emitted after the state lock is released.
//!
//! Idle costs nothing: each repository's watcher wakes only on a change. Fetching and its
//! schedule live in `repo/fetch.rs`.
//!
//! Operations (`ops/`) queue on their own per-repository mutex, `op_queue`, which only
//! operations take: reads, view-option changes and refreshes never wait for an operation,
//! however long its network command runs. An operation takes `refresh_lock` only to publish
//! its start and for its final refresh; while it runs, refreshes the watcher asks for are
//! folded into that final refresh (see [`RepoManager::refresh_on_change`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::{Notify, RwLock, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::EventSink;
use crate::dto::{
    BodyHits, CommitDetails, FetchState, RepoSnapshot, RowsPage, RunningOp, ViewOptions,
    WorktreeStatus,
};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::details::read_details;
use crate::git::refs::{read_head, read_refs, read_remotes};
use crate::git::runner::{CmdKind, GitRunner};
use crate::git::status::{STATUS_ARGS, read_status};
use crate::git::worktrees::read_worktrees;
use crate::repo::fetch::{FETCH_CAP, FETCH_TIMEOUT, Schedule, unix_ms};
use crate::repo::loader::start_load;
use crate::repo::state::{RepoIdentity, RepoInfo, RepoState, normalize_view};
use crate::watcher::{RepoWatcher, watch_git_dir_reporting};

/// Opens repositories and keeps their state. Create one per app with [`RepoManager::new`].
pub struct RepoManager {
    runner: GitRunner,
    pub(crate) sink: Arc<dyn EventSink>,
    pub(super) repos: Mutex<HashMap<u32, Arc<Repo>>>,
    next_id: AtomicU32,
    /// Fetch interval and window focus.
    pub(super) schedule: Mutex<Schedule>,
    /// Wakes the schedule task to recompute when the next fetch is due.
    pub(super) schedule_changed: Arc<Notify>,
    /// The schedule task, started with the first `open`.
    pub(super) schedule_task: Mutex<Option<JoinHandle<()>>>,
    /// How long a scheduled fetch may run, or a user's fetch stay silent, before it is
    /// killed, in milliseconds.
    pub(super) fetch_timeout_ms: AtomicU64,
    /// How long a fetch the user asked for may run in any case, in milliseconds.
    pub(super) fetch_cap_ms: AtomicU64,
    /// How long `git status` may take before it is given up on, in milliseconds.
    status_timeout_ms: AtomicU64,
    /// How long a body search may take before it is given up on, in milliseconds.
    body_search_timeout_ms: AtomicU64,
    /// Tests only: how long every body search waits before git starts, in milliseconds.
    #[cfg(any(test, feature = "testutil"))]
    body_search_delay_ms: AtomicU64,
    /// Tests only: how long every status read waits before it starts, in milliseconds.
    #[cfg(any(test, feature = "testutil"))]
    status_delay_ms: AtomicU64,
    /// Repositories (by common git dir) being opened right now. Another `open` of the same
    /// repository waits for that one instead of reading and watching it a second time; the
    /// receiver's sender is dropped when that open ends.
    opening: Mutex<HashMap<PathBuf, watch::Receiver<()>>>,
    /// Id of the last operation asked for, across repositories.
    last_op_id: AtomicU64,
    /// Tests only: how long every operation waits after its git commands, in milliseconds.
    #[cfg(any(test, feature = "testutil"))]
    op_delay_ms: AtomicU64,
}

impl Drop for RepoManager {
    fn drop(&mut self) {
        if let Some(task) = lock(&self.schedule_task).take() {
            task.abort();
        }
    }
}

/// One open repository.
pub(crate) struct Repo {
    pub(crate) identity: RepoIdentity,
    /// The main worktree.
    root: PathBuf,
    /// The active worktree; every command runs here.
    pub(crate) worktree: PathBuf,
    common_dir: PathBuf,
    /// Runs this repository's commands (logged as this repository's).
    pub(crate) runner: GitRunner,
    pub(super) slot: RwLock<Slot>,
    /// The id of the log load that is running, if any. Set and cleared while holding the
    /// `slot` write lock, except by a load that dies, which clears its own id.
    pub(super) loading: watch::Sender<Option<u64>>,
    /// The id of the last load that ended without finishing; the next refresh reloads.
    pub(super) abandoned_load: AtomicU64,
    /// Serializes refreshes and view-option changes.
    refresh_lock: tokio::sync::Mutex<()>,
    /// A refresh was asked for that no refresh has started on yet.
    refresh_wanted: AtomicBool,
    pub(super) load_task: Mutex<Option<JoinHandle<()>>>,
    /// Sequence number of the last snapshot emitted; older ones are dropped.
    emitted: Mutex<u64>,
    /// Refreshes on git-dir changes; `None` once closed, or if watching failed.
    watcher: Mutex<Option<RepoWatcher>>,
    /// When the last fetch started, or when the repository was opened; the next scheduled
    /// fetch is one interval later.
    pub(super) fetched_at: Mutex<Instant>,
    /// Errors held for the UI; see [`Repo::report_error`].
    errors: Mutex<HeldErrors>,
    /// The last status read failed (said once, until one succeeds again).
    status_failed: AtomicBool,
    /// No status read is running or wanted, and one has ended (see
    /// [`RepoManager::wait_status`]).
    status_idle: watch::Sender<bool>,
    /// The status reader: one per repository, reading outside `refresh_lock`.
    status_reader: Mutex<StatusReader>,
    /// Number of the newest status read started; a read applies only if it still is.
    status_seq: AtomicU64,
    /// The body search running, if any; a newer one aborts it.
    body_search: Mutex<Option<tokio::task::AbortHandle>>,
    /// Operations take turns here, in the order they were asked for (the mutex is fair).
    pub(crate) op_queue: tokio::sync::Mutex<()>,
    /// Operations waiting for their turn.
    pub(crate) op_waiting: AtomicU32,
}

/// Whether a status read is running and whether another was asked for meanwhile.
#[derive(Default)]
struct StatusReader {
    running: bool,
    again: bool,
}

/// `repo-error` payloads of a repository nobody has asked for yet (opened in the background
/// at start), kept until `open` adopts it. Only the latest of each kind is kept.
#[derive(Default)]
struct HeldErrors {
    /// The UI has asked for this repository: errors are emitted as they happen.
    adopted: bool,
    load: Option<serde_json::Value>,
    live_updates: Option<serde_json::Value>,
    status: Option<serde_json::Value>,
}

/// What a held `repo-error` is about.
#[derive(Clone, Copy)]
pub(super) enum ErrorKind {
    /// The log could not be read.
    Load,
    /// The git dir cannot be watched.
    LiveUpdates,
    /// `git status` failed or took too long.
    Status,
}

/// The state lock's contents.
pub(super) struct Slot {
    /// The state rows are served from.
    pub(super) current: RepoState,
    /// A reloaded state that replaces `current` once its load has as many rows, or ends.
    pub(super) pending: Option<RepoState>,
    /// Id of the newest load; a load with another id has been superseded and must not
    /// change anything.
    pub(super) load_id: u64,
    /// The last load failed, so the commits may be incomplete; the next refresh reloads.
    pub(super) load_failed: bool,
    /// Fetch progress. Kept here rather than in `RepoState` so it survives reloads.
    pub(super) fetch: FetchState,
    /// The repository was closed; no load may start and no refresh may apply.
    pub(super) closed: bool,
    /// The operation running, if any. Set and cleared while holding `refresh_lock` too.
    op: Option<RunningOp>,
    /// Sequence number of the last snapshot built for emitting.
    emit_seq: u64,
}

impl Slot {
    /// The state a new refresh compares against: the pending one when a reload is underway.
    pub(super) fn target(&self) -> &RepoState {
        self.pending.as_ref().unwrap_or(&self.current)
    }

    /// Makes the pending state current, under a new generation.
    pub(super) fn promote_pending(&mut self) {
        if let Some(mut next) = self.pending.take() {
            next.generation = self.current.generation + 1;
            self.current = next;
        }
    }

    /// The served state's snapshot, except that `viewOptions` are the newest ones asked for,
    /// even while the reload they started is still loading.
    fn snapshot(&self, id: &RepoIdentity, loading: bool) -> RepoSnapshot {
        let mut snapshot = self.current.snapshot(id, loading, &self.fetch);
        if let Some(pending) = &self.pending {
            snapshot.view_options = pending.view().clone();
        }
        snapshot.op.clone_from(&self.op);
        snapshot
    }

    /// Whether the commits may be incomplete because the last load failed or died.
    fn load_incomplete(&self, repo: &Repo) -> bool {
        self.load_failed
            || (self.load_id != 0 && repo.abandoned_load.load(Ordering::SeqCst) == self.load_id)
    }

    /// A snapshot to emit, stamped so that emits arriving out of order can be dropped.
    pub(super) fn snapshot_to_emit(&mut self, id: &RepoIdentity, loading: bool) -> Stamped {
        self.emit_seq += 1;
        Stamped(self.emit_seq, self.snapshot(id, loading))
    }
}

/// A snapshot and its emit sequence number.
pub(super) struct Stamped(u64, RepoSnapshot);

impl Repo {
    /// Emits `repo-error` with `payload`, or, while nobody has asked for this repository yet,
    /// keeps it (the latest of each kind) until [`Repo::adopt`].
    pub(super) fn report_error(
        &self,
        sink: &dyn EventSink,
        kind: ErrorKind,
        payload: serde_json::Value,
    ) {
        let mut held = lock(&self.errors);
        if held.adopted {
            drop(held);
            sink.emit("repo-error", payload);
            return;
        }
        match kind {
            ErrorKind::Load => held.load = Some(payload),
            ErrorKind::LiveUpdates => held.live_updates = Some(payload),
            ErrorKind::Status => held.status = Some(payload),
        }
    }

    /// Forgets a held error of `kind` that no longer applies: a later load succeeded.
    pub(super) fn clear_held_error(&self, kind: ErrorKind) {
        let mut held = lock(&self.errors);
        match kind {
            ErrorKind::Load => held.load = None,
            ErrorKind::LiveUpdates => held.live_updates = None,
            ErrorKind::Status => held.status = None,
        }
    }

    /// Marks the repository as asked for by the UI and emits the errors held until now.
    fn adopt(&self, sink: &dyn EventSink) {
        let held = {
            let mut held = lock(&self.errors);
            if held.adopted {
                return;
            }
            held.adopted = true;
            [
                held.live_updates.take(),
                held.load.take(),
                held.status.take(),
            ]
        };
        for payload in held.into_iter().flatten() {
            sink.emit("repo-error", payload);
        }
    }

    pub(super) fn is_loading(&self) -> bool {
        self.loading.borrow().is_some()
    }

    async fn snapshot(&self) -> RepoSnapshot {
        let slot = self.slot.read().await;
        slot.snapshot(&self.identity, self.is_loading())
    }

    /// Emits `repo-changed` unless a newer snapshot has been emitted already. Call without
    /// holding the state lock.
    pub(super) fn publish(&self, sink: &dyn EventSink, Stamped(seq, snap): Stamped) {
        let mut last = lock(&self.emitted);
        if seq > *last {
            *last = seq;
            if let Ok(payload) = serde_json::to_value(&snap) {
                sink.emit("repo-changed", payload);
            }
        }
    }

    /// Reads what the rows need (HEAD, refs, remotes, worktrees), with `status` as the
    /// status.
    async fn read_info(&self, status: WorktreeStatus) -> Result<RepoInfo> {
        read_info(&self.runner, &self.worktree, status).await
    }
}

impl RepoManager {
    pub fn new(runner: GitRunner, sink: Arc<dyn EventSink>) -> Arc<Self> {
        Arc::new(Self {
            runner,
            sink,
            repos: Mutex::new(HashMap::new()),
            next_id: AtomicU32::new(1),
            schedule: Mutex::new(Schedule::new()),
            schedule_changed: Arc::new(Notify::new()),
            schedule_task: Mutex::new(None),
            fetch_timeout_ms: AtomicU64::new(duration_ms(FETCH_TIMEOUT)),
            fetch_cap_ms: AtomicU64::new(duration_ms(FETCH_CAP)),
            status_timeout_ms: AtomicU64::new(duration_ms(STATUS_TIMEOUT)),
            body_search_timeout_ms: AtomicU64::new(duration_ms(BODY_SEARCH_TIMEOUT)),
            #[cfg(any(test, feature = "testutil"))]
            body_search_delay_ms: AtomicU64::new(0),
            #[cfg(any(test, feature = "testutil"))]
            status_delay_ms: AtomicU64::new(0),
            opening: Mutex::new(HashMap::new()),
            last_op_id: AtomicU64::new(0),
            #[cfg(any(test, feature = "testutil"))]
            op_delay_ms: AtomicU64::new(0),
        })
    }

    /// Opens the repository containing `path`, starts loading its log in the background and
    /// starts watching its git dir. Returns once refs, HEAD and worktrees are read; the
    /// working tree's status (slow on a big one) is read beside the log, and the uncommitted
    /// row comes in when it is known (see [`RepoManager::wait_status`]).
    /// Opening any path of an already open repository returns that repository's snapshot.
    pub async fn open(self: &Arc<Self>, path: &Path) -> Result<RepoSnapshot> {
        self.open_as(path, true).await
    }

    /// Like [`RepoManager::open`], for a repository the UI has not asked for yet (opened
    /// ahead at start): its `repo-error`s are held until a later `open` adopts it.
    pub async fn open_in_background(self: &Arc<Self>, path: &Path) -> Result<RepoSnapshot> {
        self.open_as(path, false).await
    }

    async fn open_as(self: &Arc<Self>, path: &Path, adopt: bool) -> Result<RepoSnapshot> {
        if !path.exists() {
            return Err(CoreError::new(
                ErrorCode::PathMissing,
                format!("Folder not found: {}", path.display()),
            ));
        }
        if !path.is_dir() {
            return Err(not_a_repo());
        }
        let location = self.locate(path).await?;
        // Held until this open ends; then the next one waiting for it goes on.
        let _opening = loop {
            if let Some(repo) = self.find_open(&location.common_dir) {
                if adopt {
                    repo.adopt(self.sink.as_ref());
                }
                return Ok(repo.snapshot().await);
            }
            let waiting = {
                let mut opening = lock(&self.opening);
                match opening.get(&location.common_dir) {
                    Some(rx) => Err(rx.clone()),
                    None => {
                        let (tx, rx) = watch::channel(());
                        opening.insert(location.common_dir.clone(), rx);
                        Ok(Opening {
                            manager: self,
                            common_dir: location.common_dir.clone(),
                            _done: tx,
                        })
                    }
                }
            };
            match waiting {
                Ok(opening) => break opening,
                // Wakes when that open ends, whether it succeeded or not.
                Err(mut rx) => while rx.changed().await.is_ok() {},
            }
        };

        let runner = self.runner.clone();
        let info = read_info(&runner, &location.worktree, WorktreeStatus::default()).await?;
        let root = info
            .worktrees
            .iter()
            .find(|w| w.is_main)
            .map_or_else(|| location.worktree.clone(), |w| PathBuf::from(&w.path));
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // Watching is a convenience: a repository that cannot be watched still opens (and
        // says so once, with `repo-error`), and refreshes on focus and on request.
        let watch_reported = Arc::new(AtomicBool::new(false));
        let watcher = watch_git_dir_reporting(
            &location.common_dir,
            self.refresher(id),
            self.watch_failure_reporter(id, watch_reported.clone()),
        );
        let view = ViewOptions {
            branches: None,
            show_remotes: true,
            show_tags: true,
            show_stashes: true,
        };
        let repo = Arc::new(Repo {
            identity: RepoIdentity {
                repo_id: id,
                name: root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path_string(&root)),
                root_path: path_string(&root),
                active_worktree: path_string(&location.worktree),
            },
            root,
            worktree: location.worktree,
            common_dir: location.common_dir,
            runner: runner.for_repo(id),
            slot: RwLock::new(Slot {
                current: RepoState::new(info, view, 1),
                pending: None,
                load_id: 0,
                load_failed: false,
                fetch: FetchState::default(),
                closed: false,
                op: None,
                emit_seq: 0,
            }),
            loading: watch::channel(None).0,
            abandoned_load: AtomicU64::new(0),
            refresh_lock: tokio::sync::Mutex::new(()),
            refresh_wanted: AtomicBool::new(false),
            load_task: Mutex::new(None),
            emitted: Mutex::new(0),
            watcher: Mutex::new(None),
            fetched_at: Mutex::new(Instant::now()),
            errors: Mutex::new(HeldErrors {
                adopted: adopt,
                ..HeldErrors::default()
            }),
            status_failed: AtomicBool::new(false),
            status_idle: watch::channel(false).0,
            status_reader: Mutex::new(StatusReader::default()),
            status_seq: AtomicU64::new(0),
            body_search: Mutex::new(None),
            op_queue: tokio::sync::Mutex::new(()),
            op_waiting: AtomicU32::new(0),
        });

        // Nobody else can hold this lock yet; holding it while registering the repository
        // means no query sees it before its load has started.
        let mut slot = repo.slot.write().await;
        let existing = {
            let mut repos = lock(&self.repos);
            let existing = repos
                .values()
                .find(|r| r.common_dir == repo.common_dir)
                .cloned();
            if existing.is_none() {
                repos.insert(id, repo.clone());
            }
            existing
        };
        if let Some(existing) = existing {
            // Opened concurrently by another call; use that one.
            drop(slot);
            if adopt {
                existing.adopt(self.sink.as_ref());
            }
            return Ok(existing.snapshot().await);
        }
        if slot.current.has_tips() {
            start_load(&repo, &mut slot, &self.sink);
        }
        let snapshot = slot.snapshot(&repo.identity, repo.is_loading());
        drop(slot);
        match watcher {
            Ok(watcher) => *lock(&repo.watcher) = Some(watcher),
            Err(error) => {
                report_live_updates_unavailable(self.sink.as_ref(), &repo, &watch_reported, error);
            }
        }
        self.start_schedule();
        // Status beside the log.
        self.request_status(&repo);
        Ok(snapshot)
    }

    /// Sets how long `git status` may take (tests only; normally 60 s).
    #[cfg(any(test, feature = "testutil"))]
    pub fn set_status_timeout(&self, timeout: Duration) {
        self.status_timeout_ms
            .store(duration_ms(timeout), Ordering::Relaxed);
    }

    /// Makes every status read wait this long first, as on a big working tree (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn set_status_delay(&self, delay: Duration) {
        self.status_delay_ms
            .store(duration_ms(delay), Ordering::Relaxed);
    }

    /// Resolves once no status read of the repository is running or wanted, and at least
    /// one has ended, however it ended (immediately for an unknown id).
    pub async fn wait_status(&self, repo_id: u32) {
        let Ok(repo) = self.get(repo_id) else {
            return;
        };
        let mut read = repo.status_idle.subscribe();
        let _ = read.wait_for(|read| *read).await;
    }

    /// Asks for the working tree's status to be read again, without waiting for it: one
    /// reader per repository runs outside `refresh_lock`, so a slow status holds up neither
    /// refreshes nor view-option changes. Requests made while a read runs coalesce into one
    /// more read after it.
    fn request_status(self: &Arc<Self>, repo: &Arc<Repo>) {
        {
            let mut reader = lock(&repo.status_reader);
            if reader.running {
                reader.again = true;
                return;
            }
            reader.running = true;
            repo.status_idle.send_replace(false);
        }
        let (m, repo) = (self.clone(), repo.clone());
        tokio::spawn(async move {
            loop {
                lock(&repo.status_reader).again = false;
                let seq = repo.status_seq.fetch_add(1, Ordering::SeqCst) + 1;
                if let Some(status) = m.read_status_now(&repo).await {
                    let _ = m.apply_status(&repo, status, seq).await;
                }
                let mut reader = lock(&repo.status_reader);
                if !reader.again || lock(&m.repos).get(&repo.identity.repo_id).is_none() {
                    reader.running = false;
                    repo.status_idle.send_replace(true);
                    return;
                }
            }
        });
    }

    /// Reads the working tree's status, giving up after the status timeout. A failure is
    /// said once, as `repo-error`, until a read succeeds again: `None`.
    async fn read_status_now(&self, repo: &Repo) -> Option<WorktreeStatus> {
        let timeout = Duration::from_millis(self.status_timeout_ms.load(Ordering::Relaxed));
        let started = std::time::Instant::now();
        let read = tokio::time::timeout(timeout, async {
            #[cfg(any(test, feature = "testutil"))]
            {
                let delay = self.status_delay_ms.load(Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            read_status(&repo.runner, &repo.worktree).await
        })
        .await;
        let reason = match read {
            Ok(Ok(status)) => {
                repo.status_failed.store(false, Ordering::SeqCst);
                repo.clear_held_error(ErrorKind::Status);
                return Some(status);
            }
            Ok(Err(e)) => e,
            Err(_) => {
                // Dropping the command's future killed git.
                repo.runner
                    .log_timed_out(&repo.worktree, &STATUS_ARGS, CmdKind::Read, started);
                CoreError::new(
                    ErrorCode::GitFailed,
                    format!("git status took longer than {}", seconds(timeout)),
                )
            }
        };
        if !repo.status_failed.swap(true, Ordering::SeqCst) {
            repo.report_error(
                self.sink.as_ref(),
                ErrorKind::Status,
                status_unavailable(repo.identity.repo_id, reason),
            );
        }
        None
    }

    /// Applies `status` from read number `seq`, unless a newer read has started since: the
    /// uncommitted row appears, changes or goes, in a new generation. Takes `refresh_lock`
    /// only for this, which involves no git.
    async fn apply_status(&self, repo: &Arc<Repo>, status: WorktreeStatus, seq: u64) -> Result<()> {
        let _serial = repo.refresh_lock.lock().await;
        if repo.status_seq.load(Ordering::SeqCst) != seq {
            return Ok(());
        }
        let mut info = repo.slot.read().await.target().info().clone();
        if info.status == status {
            return Ok(());
        }
        info.status = status;
        self.apply(repo, info, None, None).await.map(|_| ())
    }

    /// Closes a repository and stops its background load and its watcher.
    pub async fn close(&self, repo_id: u32) -> Result<()> {
        let repo = lock(&self.repos)
            .remove(&repo_id)
            .ok_or_else(|| unknown_repo(repo_id))?;
        drop(lock(&repo.watcher).take());
        self.schedule_changed.notify_one();
        let mut slot = repo.slot.write().await;
        slot.closed = true;
        slot.load_id += 1;
        slot.pending = None;
        if let Some(task) = lock(&repo.load_task).take() {
            task.abort();
        }
        repo.loading.send_replace(None);
        Ok(())
    }

    /// `Ok` if repository `repo_id` is open, else `unknown_repo`.
    pub fn check_open(&self, repo_id: u32) -> Result<()> {
        self.get(repo_id).map(|_| ())
    }

    pub async fn snapshot(&self, repo_id: u32) -> Result<RepoSnapshot> {
        Ok(self.get(repo_id)?.snapshot().await)
    }

    /// Rows `start..start + count` of `generation`, clamped to the rows there are (at most
    /// 1000). `StaleGeneration` if the rows have changed since `generation`.
    pub async fn rows(
        &self,
        repo_id: u32,
        generation: u64,
        start: u32,
        count: u32,
    ) -> Result<RowsPage> {
        let repo = self.get(repo_id)?;
        let slot = repo.slot.read().await;
        let state = &slot.current;
        if generation != state.generation {
            return Err(CoreError::new(
                ErrorCode::StaleGeneration,
                format!(
                    "rows changed: generation {generation} is not the current {}",
                    state.generation
                ),
            ));
        }
        Ok(RowsPage {
            generation,
            start: start.min(state.total_rows()),
            rows: state.rows(start, count),
        })
    }

    pub async fn find_row(&self, repo_id: u32, hash: &str) -> Result<Option<u32>> {
        let repo = self.get(repo_id)?;
        let slot = repo.slot.read().await;
        Ok(slot.current.find_row(hash))
    }

    /// Ascending row indices of the commits matching `query`; see [`RepoState::search`].
    pub async fn search(&self, repo_id: u32, query: &str) -> Result<Vec<u32>> {
        let repo = self.get(repo_id)?;
        let slot = repo.slot.read().await;
        Ok(slot.current.search(query))
    }

    /// Rows whose whole commit message (subject and body) contains `query`, ignoring case,
    /// in the view's commits, found by git (bodies are not kept in memory) and mapped to the
    /// rows of the generation served when git is done. Slower than [`RepoManager::search`]:
    /// the UI merges these hits in when they come. One runs at a time per repository: a
    /// newer one stops the one before (its git is killed; that call fails), and one that
    /// takes longer than 30 s gives up.
    pub async fn search_bodies(&self, repo_id: u32, query: &str) -> Result<BodyHits> {
        let repo = self.get(repo_id)?;
        let args = repo.slot.read().await.current.body_search_args(query);
        let hashes = match args {
            Some(args) => self.run_body_search(&repo, args).await?,
            None => String::new(),
        };
        let slot = repo.slot.read().await;
        let state = &slot.current;
        let mut rows: Vec<u32> = hashes
            .lines()
            .filter_map(|h| state.find_row(h.trim()))
            .collect();
        rows.sort_unstable();
        rows.dedup();
        Ok(BodyHits {
            generation: state.generation,
            rows,
        })
    }

    /// Runs the body search `args` on its own task, which a newer search or the timeout
    /// aborts (dropping git's command kills it). Returns git's output.
    async fn run_body_search(&self, repo: &Repo, args: Vec<String>) -> Result<String> {
        let (runner, worktree) = (repo.runner.clone(), repo.worktree.clone());
        #[cfg(any(test, feature = "testutil"))]
        let delay = Duration::from_millis(self.body_search_delay_ms.load(Ordering::Relaxed));
        let task = tokio::spawn(async move {
            #[cfg(any(test, feature = "testutil"))]
            tokio::time::sleep(delay).await;
            let argv: Vec<&str> = args.iter().map(String::as_str).collect();
            runner
                .run(&worktree, &argv, CmdKind::Read)
                .await
                .map(|out| out.text())
        });
        let mine = task.abort_handle();
        let id = mine.id();
        if let Some(older) = lock(&repo.body_search).replace(mine.clone()) {
            older.abort();
        }
        let timeout = Duration::from_millis(self.body_search_timeout_ms.load(Ordering::Relaxed));
        let result = tokio::time::timeout(timeout, task).await;
        {
            let mut running = lock(&repo.body_search);
            if running.as_ref().is_some_and(|h| h.id() == id) {
                *running = None;
            }
        }
        match result {
            Ok(Ok(out)) => out,
            Ok(Err(_)) => Err(CoreError::new(
                ErrorCode::GitFailed,
                "The message search was replaced by a newer one.",
            )),
            Err(_) => {
                mine.abort();
                Err(CoreError::new(
                    ErrorCode::GitFailed,
                    format!("The message search took longer than {}.", seconds(timeout)),
                ))
            }
        }
    }

    /// Sets how long a body search may take (tests only; normally 30 s).
    #[cfg(any(test, feature = "testutil"))]
    pub fn set_body_search_timeout(&self, timeout: Duration) {
        self.body_search_timeout_ms
            .store(duration_ms(timeout), Ordering::Relaxed);
    }

    /// Makes every body search wait this long before git starts (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn set_body_search_delay(&self, delay: Duration) {
        self.body_search_delay_ms
            .store(duration_ms(delay), Ordering::Relaxed);
    }

    /// Metadata, message and files of commit `hash` (a full hash).
    pub async fn details(&self, repo_id: u32, hash: &str) -> Result<CommitDetails> {
        let repo = self.get(repo_id)?;
        read_details(&repo.runner, &repo.worktree, hash).await
    }

    /// Changes which refs the graph shows. An empty branch selection, or one whose branches
    /// no longer exist, shows all branches. Reloads the log only if that changes the commits.
    pub async fn set_view_options(
        self: &Arc<Self>,
        repo_id: u32,
        o: ViewOptions,
    ) -> Result<RepoSnapshot> {
        let repo = self.get(repo_id)?;
        let _serial = repo.refresh_lock.lock().await;
        let info = repo.slot.read().await.target().info().clone();
        self.apply(&repo, info, Some(o), None).await
    }

    /// Re-reads HEAD, refs and worktrees and applies them (emitting at once), then asks for
    /// the working tree's status (see `request_status`), which lands later, so new commits
    /// never wait for a slow status. Reloads the log only if the commits to
    /// show may have changed (a selected tip went away, or a new tip is not a loaded commit),
    /// or the last load did not finish; otherwise only labels and the uncommitted row change.
    /// During a reload the old rows stay served until the new load has caught up with them,
    /// but the returned snapshot already carries the new view options. Overlapping calls
    /// coalesce: a call made while another runs waits for it, and then at most one more
    /// refresh runs for all the calls that waited.
    pub async fn refresh(self: &Arc<Self>, repo_id: u32) -> Result<RepoSnapshot> {
        let repo = self.get(repo_id)?;
        self.refresh_repo(&repo, false).await
    }

    /// The watcher's refresh: like [`RepoManager::refresh`], except while an operation runs.
    /// Then the change (usually the operation's own) is left to the refresh the operation
    /// ends with, which starts after it, so the result of an operation arrives once, with
    /// its end, and never half-done. Checked under `refresh_lock`, which an operation holds
    /// when it publishes its start, so no refresh started before an operation can apply
    /// after the operation has begun changing the repository.
    async fn refresh_on_change(self: &Arc<Self>, repo_id: u32) -> Result<RepoSnapshot> {
        let repo = self.get(repo_id)?;
        self.refresh_repo(&repo, true).await
    }

    /// [`RepoManager::refresh`] of `repo`; with `fold_into_op`, not while an operation runs.
    async fn refresh_repo(
        self: &Arc<Self>,
        repo: &Arc<Repo>,
        fold_into_op: bool,
    ) -> Result<RepoSnapshot> {
        repo.refresh_wanted.store(true, Ordering::SeqCst);
        let _serial = repo.refresh_lock.lock().await;
        if fold_into_op && repo.slot.read().await.op.is_some() {
            // `refresh_wanted` stays set; the operation's final refresh clears it.
            return Ok(repo.snapshot().await);
        }
        if !repo.refresh_wanted.swap(false, Ordering::SeqCst) {
            // A refresh that started after this call was made has already run.
            return Ok(repo.snapshot().await);
        }
        let status = repo.slot.read().await.target().info().status;
        let info = repo.read_info(status).await?;
        let snapshot = self.apply(repo, info, None, None).await?;
        self.request_status(repo);
        Ok(snapshot)
    }

    /// Next operation id (they start at 1).
    pub(crate) fn next_op_id(&self) -> u64 {
        self.last_op_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Makes every operation wait this long after its git commands, before its refresh
    /// (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn set_op_delay(&self, delay: Duration) {
        self.op_delay_ms
            .store(duration_ms(delay), Ordering::Relaxed);
    }

    /// The test delay of [`RepoManager::set_op_delay`]; zero outside tests.
    pub(crate) fn op_delay(&self) -> Duration {
        #[cfg(any(test, feature = "testutil"))]
        {
            Duration::from_millis(self.op_delay_ms.load(Ordering::Relaxed))
        }
        #[cfg(not(any(test, feature = "testutil")))]
        {
            Duration::ZERO
        }
    }

    /// How long a network operation may stay silent, and run in any case: as a fetch the
    /// user asked for.
    pub(crate) fn net_limit(&self) -> crate::git::runner::TimeLimit {
        crate::git::runner::TimeLimit::idle(
            Duration::from_millis(self.fetch_timeout_ms.load(Ordering::Relaxed)),
            Duration::from_millis(self.fetch_cap_ms.load(Ordering::Relaxed)),
        )
    }

    /// Marks operation `id` (`title`) as running on `repo` and emits `repo-changed`.
    /// `unknown_repo` once the repository is closed.
    pub(crate) async fn start_op(&self, repo: &Repo, id: u64, title: &str) -> Result<()> {
        let _serial = repo.refresh_lock.lock().await;
        let started = {
            let mut slot = repo.slot.write().await;
            if slot.closed {
                return Err(unknown_repo(repo.identity.repo_id));
            }
            slot.op = Some(RunningOp {
                id,
                title: title.to_owned(),
                started_at_ms: unix_ms(),
                queued: repo.op_waiting.load(Ordering::SeqCst),
            });
            slot.snapshot_to_emit(&repo.identity, repo.is_loading())
        };
        repo.publish(self.sink.as_ref(), started);
        Ok(())
    }

    /// Emits `repo-changed` with the running operation's new count of waiting requests,
    /// if it changed.
    pub(crate) async fn op_queue_changed(&self, repo: &Repo) {
        let changed = {
            let mut slot = repo.slot.write().await;
            let waiting = repo.op_waiting.load(Ordering::SeqCst);
            match slot.op.as_mut() {
                Some(op) if op.queued != waiting => op.queued = waiting,
                _ => return,
            }
            slot.snapshot_to_emit(&repo.identity, repo.is_loading())
        };
        repo.publish(self.sink.as_ref(), changed);
    }

    /// Ends operation `id` on `repo`: refreshes (covering every refresh asked for until
    /// now, the ones folded while it ran included) and clears `op` in the same
    /// `repo-changed`, then asks for the status. If the repository cannot be read, `op` is
    /// still cleared, and the snapshot is the last one read. `unknown_repo` once closed.
    pub(crate) async fn finish_op(
        self: &Arc<Self>,
        repo: &Arc<Repo>,
        id: u64,
    ) -> Result<RepoSnapshot> {
        let serial = repo.refresh_lock.lock().await;
        repo.refresh_wanted.store(false, Ordering::SeqCst);
        let status = repo.slot.read().await.target().info().status;
        let snapshot = match repo.read_info(status).await {
            Ok(info) => self.apply(repo, info, None, Some(id)).await?,
            Err(_) => {
                let (to_emit, snapshot) = {
                    let mut slot = repo.slot.write().await;
                    if slot.closed {
                        return Err(unknown_repo(repo.identity.repo_id));
                    }
                    let mine = slot.op.as_ref().is_some_and(|op| op.id == id);
                    if mine {
                        slot.op = None;
                    }
                    let loading = repo.is_loading();
                    (
                        mine.then(|| slot.snapshot_to_emit(&repo.identity, loading)),
                        slot.snapshot(&repo.identity, loading),
                    )
                };
                if let Some(stamped) = to_emit {
                    repo.publish(self.sink.as_ref(), stamped);
                }
                snapshot
            }
        };
        drop(serial);
        self.request_status(repo);
        Ok(snapshot)
    }

    /// Resolves once the repository's log is fully loaded (immediately for an unknown id).
    pub async fn wait_loaded(&self, repo_id: u32) {
        let Ok(repo) = self.get(repo_id) else {
            return;
        };
        let mut loading = repo.loading.subscribe();
        // The sender lives in `repo`, which this function holds; it cannot be dropped.
        let _ = loading.wait_for(Option::is_none).await;
    }

    /// Ids and main worktree paths of the open repositories, by id.
    pub fn open_paths(&self) -> Vec<(u32, PathBuf)> {
        let mut paths: Vec<(u32, PathBuf)> = lock(&self.repos)
            .values()
            .map(|r| (r.identity.repo_id, r.root.clone()))
            .collect();
        paths.sort_by_key(|(id, _)| *id);
        paths
    }

    /// The common git directory and the main worktree of an open repository.
    pub fn git_dirs(&self, repo_id: u32) -> Option<(PathBuf, PathBuf)> {
        let repo = self.get(repo_id).ok()?;
        Some((repo.common_dir.clone(), repo.root.clone()))
    }

    /// The runner used for this repository's commands.
    pub fn runner(&self, repo_id: u32) -> Option<GitRunner> {
        self.get(repo_id).ok().map(|r| r.runner.clone())
    }

    /// Refreshes `repo_id` in the background (if there is a runtime to do it on), for a
    /// change the watcher saw (see [`RepoManager::refresh_on_change`]). Errors have no one
    /// to go to: a closed repository is expected, and anything else shows up on the next
    /// refresh the user asks for.
    pub(super) fn spawn_refresh(self: &Arc<Self>, repo_id: u32) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let m = self.clone();
            runtime.spawn(async move {
                let _ = m.refresh_on_change(repo_id).await;
            });
        }
    }

    /// The watcher callback of repository `repo_id`. Holds the manager weakly, so a watcher
    /// never keeps it alive.
    fn refresher(self: &Arc<Self>, repo_id: u32) -> impl Fn() + Send + Sync + 'static {
        let m = Arc::downgrade(self);
        move || {
            if let Some(m) = m.upgrade() {
                m.spawn_refresh(repo_id);
            }
        }
    }

    /// The watcher's error callback of repository `repo_id`; see
    /// [`report_live_updates_unavailable`].
    fn watch_failure_reporter(
        self: &Arc<Self>,
        repo_id: u32,
        reported: Arc<AtomicBool>,
    ) -> impl Fn(CoreError) + Send + Sync + 'static {
        let m = Arc::downgrade(self);
        move |error| {
            if let Some(m) = m.upgrade()
                && let Ok(repo) = m.get(repo_id)
            {
                report_live_updates_unavailable(m.sink.as_ref(), &repo, &reported, error);
            }
        }
    }

    pub(crate) fn get(&self, repo_id: u32) -> Result<Arc<Repo>> {
        lock(&self.repos)
            .get(&repo_id)
            .cloned()
            .ok_or_else(|| unknown_repo(repo_id))
    }

    fn find_open(&self, common_dir: &Path) -> Option<Arc<Repo>> {
        lock(&self.repos)
            .values()
            .find(|r| r.common_dir == common_dir)
            .cloned()
    }

    /// Applies newly read `info` and, if given, new view options (else the current ones):
    /// starts a reload when the commits may change, otherwise updates labels and the
    /// uncommitted row in place. With `op_done`, also clears that operation, if it is the one
    /// running. Emits `repo-changed` if anything visible changed. Callers hold
    /// `refresh_lock`.
    async fn apply(
        &self,
        repo: &Arc<Repo>,
        info: RepoInfo,
        view: Option<ViewOptions>,
        op_done: Option<u64>,
    ) -> Result<RepoSnapshot> {
        let (to_emit, snapshot) = {
            let mut slot = repo.slot.write().await;
            if slot.closed {
                return Err(unknown_repo(repo.identity.repo_id));
            }
            let view = view.unwrap_or_else(|| slot.target().view().clone());
            let view = normalize_view(view, &info);
            let changed = if slot.load_incomplete(repo) || slot.target().needs_reload(&info, &view)
            {
                let next = RepoState::new(info, view, 0);
                let has_tips = next.has_tips();
                slot.pending = Some(next);
                if has_tips {
                    start_load(repo, &mut slot, &self.sink);
                } else {
                    // Nothing to read (no commits selected): switch now.
                    slot.load_id += 1;
                    slot.load_failed = false;
                    if let Some(task) = lock(&repo.load_task).take() {
                        task.abort();
                    }
                    slot.promote_pending();
                    repo.loading.send_replace(None);
                }
                true
            } else if let Some(pending) = slot.pending.as_mut() {
                // Not served yet: nothing visible changes until it replaces `current`.
                pending.update(info, view);
                false
            } else {
                let changed = slot.current.update(info, view);
                if changed {
                    slot.current.generation += 1;
                }
                changed
            };
            let op_cleared =
                op_done.is_some_and(|id| slot.op.as_ref().is_some_and(|op| op.id == id));
            if op_cleared {
                slot.op = None;
            }
            let changed = changed || op_cleared;
            let loading = repo.is_loading();
            let snapshot = slot.snapshot(&repo.identity, loading);
            (
                changed.then(|| slot.snapshot_to_emit(&repo.identity, loading)),
                snapshot,
            )
        };
        if let Some(stamped) = to_emit {
            repo.publish(self.sink.as_ref(), stamped);
        }
        Ok(snapshot)
    }

    /// Finds the worktree and common git dir of `path`.
    async fn locate(&self, path: &Path) -> Result<Location> {
        let out = self
            .runner
            .run_unchecked(
                path,
                &[
                    "rev-parse",
                    "--show-toplevel",
                    "--git-common-dir",
                    "--is-bare-repository",
                ],
                CmdKind::Read,
            )
            .await?;
        if out.code != 0 {
            if out.stderr.contains("dubious ownership") {
                return Err(not_owned().with_detail(out.stderr));
            }
            // `--show-toplevel` fails in a bare repository; ask about bareness alone.
            let bare = self
                .runner
                .run_unchecked(path, &["rev-parse", "--is-bare-repository"], CmdKind::Read)
                .await?;
            if bare.code == 0 && bare.text().trim() == "true" {
                return Err(bare_repo());
            }
            return Err(not_a_repo().with_detail(out.stderr));
        }
        let text = out.text();
        let mut lines = text.lines();
        let (Some(toplevel), Some(common_dir), Some(is_bare)) =
            (lines.next(), lines.next(), lines.next())
        else {
            return Err(not_a_repo().with_detail(text.clone()));
        };
        if is_bare.trim() == "true" {
            return Err(bare_repo());
        }
        Ok(Location {
            worktree: canonical(Path::new(toplevel)),
            // A relative common dir is relative to the directory git ran in.
            common_dir: canonical(&path.join(common_dir)),
        })
    }
}

/// Reports `repo-error` (code `io`) saying that live updates are unavailable for `repo`
/// because of `reason`, unless `reported` shows it was said already: once per repository,
/// however often watching fails.
fn report_live_updates_unavailable(
    sink: &dyn EventSink,
    repo: &Repo,
    reported: &AtomicBool,
    reason: CoreError,
) {
    if reported.swap(true, Ordering::SeqCst) {
        return;
    }
    repo.report_error(
        sink,
        ErrorKind::LiveUpdates,
        live_updates_unavailable(repo.identity.repo_id, reason),
    );
}

/// The `repo-error` payload saying that live updates are unavailable for `repo_id`.
fn live_updates_unavailable(repo_id: u32, reason: CoreError) -> serde_json::Value {
    let mut error = CoreError::new(
        ErrorCode::Io,
        format!(
            "Live updates are unavailable for this repository: {}",
            reason.message
        ),
    );
    error.detail = reason.detail;
    serde_json::json!({ "repoId": repo_id, "error": error })
}

/// An `open` in progress; removes itself from `opening` when the open ends.
struct Opening<'a> {
    manager: &'a RepoManager,
    common_dir: PathBuf,
    /// Dropped with this, which wakes the opens waiting for this one.
    _done: watch::Sender<()>,
}

impl Drop for Opening<'_> {
    fn drop(&mut self) {
        lock(&self.manager.opening).remove(&self.common_dir);
    }
}

/// `d` as "<n> s", or in ms below a second.
fn seconds(d: Duration) -> String {
    if d >= Duration::from_secs(1) {
        format!("{} s", d.as_secs())
    } else {
        format!("{} ms", d.as_millis())
    }
}

/// How long a search of commit messages may take before it is given up on.
const BODY_SEARCH_TIMEOUT: Duration = Duration::from_secs(30);

/// How long `git status` may take before it is given up on: long, since a big working tree
/// can take a while, but not forever.
const STATUS_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) fn duration_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Where an opened path lives.
struct Location {
    worktree: PathBuf,
    common_dir: PathBuf,
}

/// Head, refs, remotes and worktrees, read concurrently in `worktree`, with `status` as the
/// status.
async fn read_info(
    runner: &GitRunner,
    worktree: &Path,
    status: WorktreeStatus,
) -> Result<RepoInfo> {
    let (head, refs, remotes, worktrees) = tokio::join!(
        read_head(runner, worktree),
        read_refs(runner, worktree),
        read_remotes(runner, worktree),
        read_worktrees(runner, worktree),
    );
    let mut worktrees = worktrees?;
    for wt in &mut worktrees {
        wt.path = path_string(&canonical(Path::new(&wt.path)));
    }
    Ok(RepoInfo {
        head: head?,
        refs: refs?,
        remotes: remotes?,
        worktrees,
        status,
    })
}

/// The `repo-error` payload saying that the working tree's changes could not be read.
fn status_unavailable(repo_id: u32, reason: CoreError) -> serde_json::Value {
    let mut error = CoreError::new(
        reason.code,
        format!(
            "Could not read the working tree, so uncommitted changes are not shown: {}",
            reason.message
        ),
    );
    error.detail = reason.detail;
    serde_json::json!({ "repoId": repo_id, "error": error })
}

/// `path` with symlinks resolved, so paths from different sources compare equal (on macOS
/// temp dirs are reached through `/var` → `/private/var`); on Windows without the `\\?\`
/// prefix. Unchanged if it cannot be resolved, for example a worktree whose directory is
/// gone.
fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn not_a_repo() -> CoreError {
    CoreError::new(ErrorCode::NotARepo, "This folder is not a git repository.")
}

/// git refuses a repository owned by another user until it is listed in `safe.directory`.
/// The app never changes git's config itself: git's own message (the detail) gives the
/// command.
fn not_owned() -> CoreError {
    CoreError::new(
        ErrorCode::GitFailed,
        "Git will not open this repository because its folder belongs to another user. \
         Git's message below gives the command that marks it as safe (safe.directory).",
    )
}

fn bare_repo() -> CoreError {
    CoreError::new(
        ErrorCode::BareRepo,
        "This folder is a bare repository, which has no working tree to show.",
    )
}

pub(super) fn unknown_repo(repo_id: u32) -> CoreError {
    CoreError::new(
        ErrorCode::UnknownRepo,
        format!("No open repository has id {repo_id}."),
    )
}

/// Locks a std mutex, ignoring poisoning: every critical section leaves its data valid.
pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::runner::CommandLog;
    use crate::testutil::{RecordingSink, TestRepo};

    #[test]
    fn live_updates_unavailable_payload() {
        let reason = CoreError::new(ErrorCode::Io, "Could not watch x.").with_detail("os said no");
        let payload = live_updates_unavailable(7, reason);
        assert_eq!(payload["repoId"], 7);
        assert_eq!(payload["error"]["code"], "io");
        assert_eq!(
            payload["error"]["message"],
            "Live updates are unavailable for this repository: Could not watch x."
        );
        assert_eq!(payload["error"]["detail"], "os said no");
    }

    /// A load that ends without finishing (a panic, an abort) while it is still the newest
    /// must not leave `loading` set, and the next refresh must reload what it left behind.
    #[tokio::test]
    async fn a_load_that_dies_clears_loading() {
        let repo = TestRepo::new();
        repo.commit_many(1500);
        let runner = GitRunner::detect(CommandLog::new()).await.unwrap();
        let m = RepoManager::new(runner, Arc::new(RecordingSink::default()));
        let id = m.open(repo.path()).await.unwrap().repo_id;
        let task = lock(&m.get(id).unwrap().load_task)
            .take()
            .expect("a load is running");
        task.abort();
        let wait = std::time::Duration::from_secs(20);
        tokio::time::timeout(wait, m.wait_loaded(id))
            .await
            .expect("loading is cleared");
        assert!(!m.snapshot(id).await.unwrap().loading);
        m.refresh(id).await.unwrap();
        tokio::time::timeout(wait, m.wait_loaded(id))
            .await
            .expect("the reload finishes");
        assert_eq!(m.snapshot(id).await.unwrap().total_rows, 1500);
    }
}
