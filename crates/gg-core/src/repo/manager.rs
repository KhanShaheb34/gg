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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::{Notify, RwLock, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::EventSink;
use crate::dto::{CommitDetails, FetchState, RepoSnapshot, RowsPage, ViewOptions};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::details::read_details;
use crate::git::refs::{read_head, read_refs, read_remotes};
use crate::git::runner::{CmdKind, GitRunner};
use crate::git::status::read_status;
use crate::git::worktrees::read_worktrees;
use crate::repo::fetch::{FETCH_TIMEOUT, Schedule};
use crate::repo::loader::start_load;
use crate::repo::state::{RepoIdentity, RepoInfo, RepoState, normalize_view};
use crate::watcher::{RepoWatcher, watch_git_dir_reporting};

/// Opens repositories and keeps their state. Create one per app with [`RepoManager::new`].
pub struct RepoManager {
    runner: GitRunner,
    pub(super) sink: Arc<dyn EventSink>,
    pub(super) repos: Mutex<HashMap<u32, Arc<Repo>>>,
    next_id: AtomicU32,
    /// Fetch interval and window focus.
    pub(super) schedule: Mutex<Schedule>,
    /// Wakes the schedule task to recompute when the next fetch is due.
    pub(super) schedule_changed: Arc<Notify>,
    /// The schedule task, started with the first `open`.
    pub(super) schedule_task: Mutex<Option<JoinHandle<()>>>,
    /// How long a fetch may run before it is killed, in milliseconds.
    pub(super) fetch_timeout_ms: AtomicU64,
}

impl Drop for RepoManager {
    fn drop(&mut self) {
        if let Some(task) = lock(&self.schedule_task).take() {
            task.abort();
        }
    }
}

/// One open repository.
pub(super) struct Repo {
    pub(super) identity: RepoIdentity,
    /// The main worktree.
    root: PathBuf,
    /// The active worktree; every command runs here.
    pub(super) worktree: PathBuf,
    common_dir: PathBuf,
    /// Runs this repository's commands.
    pub(super) runner: GitRunner,
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

    /// Reads everything but the log.
    async fn read_info(&self) -> Result<RepoInfo> {
        read_info(&self.runner, &self.worktree).await
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
        })
    }

    /// Opens the repository containing `path`, starts loading its log in the background and
    /// starts watching its git dir. Returns once refs, HEAD, worktrees and status are read.
    /// Opening any path of an already open repository returns that repository's snapshot.
    pub async fn open(self: &Arc<Self>, path: &Path) -> Result<RepoSnapshot> {
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
        if let Some(repo) = self.find_open(&location.common_dir) {
            return Ok(repo.snapshot().await);
        }

        let runner = self.runner.clone();
        let info = read_info(&runner, &location.worktree).await?;
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
            runner,
            slot: RwLock::new(Slot {
                current: RepoState::new(info, view, 1),
                pending: None,
                load_id: 0,
                load_failed: false,
                fetch: FetchState::default(),
                closed: false,
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
                report_live_updates_unavailable(self.sink.as_ref(), id, &watch_reported, error);
            }
        }
        self.start_schedule();
        Ok(snapshot)
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
        self.apply(&repo, info, Some(o)).await
    }

    /// Re-reads HEAD, refs, worktrees and status. Reloads the log only if the commits to
    /// show may have changed (a selected tip went away, or a new tip is not a loaded commit),
    /// or the last load did not finish; otherwise only labels and the uncommitted row change.
    /// During a reload the old rows stay served until the new load has caught up with them,
    /// but the returned snapshot already carries the new view options. Overlapping calls
    /// coalesce: a call made while another runs waits for it, and then at most one more
    /// refresh runs for all the calls that waited.
    pub async fn refresh(self: &Arc<Self>, repo_id: u32) -> Result<RepoSnapshot> {
        let repo = self.get(repo_id)?;
        repo.refresh_wanted.store(true, Ordering::SeqCst);
        let _serial = repo.refresh_lock.lock().await;
        if !repo.refresh_wanted.swap(false, Ordering::SeqCst) {
            // A refresh that started after this call was made has already run.
            return Ok(repo.snapshot().await);
        }
        let info = repo.read_info().await?;
        self.apply(&repo, info, None).await
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

    pub(super) fn open_ids(&self) -> Vec<u32> {
        lock(&self.repos).keys().copied().collect()
    }

    /// Refreshes `repo_id` in the background (if there is a runtime to do it on). Errors have
    /// no one to go to: a closed repository is expected, and anything else shows up on the
    /// next refresh the user asks for.
    pub(super) fn spawn_refresh(self: &Arc<Self>, repo_id: u32) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let m = self.clone();
            runtime.spawn(async move {
                let _ = m.refresh(repo_id).await;
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
            if let Some(m) = m.upgrade() {
                report_live_updates_unavailable(m.sink.as_ref(), repo_id, &reported, error);
            }
        }
    }

    pub(super) fn get(&self, repo_id: u32) -> Result<Arc<Repo>> {
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
    /// uncommitted row in place. Emits `repo-changed` if anything visible changed. Callers
    /// hold `refresh_lock`.
    async fn apply(
        &self,
        repo: &Arc<Repo>,
        info: RepoInfo,
        view: Option<ViewOptions>,
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

/// Emits `repo-error` (code `io`) saying that live updates are unavailable for repository
/// `repo_id` because of `reason`, unless `reported` shows it was said already: once per
/// repository, however often watching fails.
fn report_live_updates_unavailable(
    sink: &dyn EventSink,
    repo_id: u32,
    reported: &AtomicBool,
    reason: CoreError,
) {
    if reported.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut error = CoreError::new(
        ErrorCode::Io,
        format!(
            "Live updates are unavailable for this repository: {}",
            reason.message
        ),
    );
    error.detail = reason.detail;
    sink.emit(
        "repo-error",
        serde_json::json!({ "repoId": repo_id, "error": error }),
    );
}

pub(super) fn duration_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Where an opened path lives.
struct Location {
    worktree: PathBuf,
    common_dir: PathBuf,
}

/// Head, refs, remotes, worktrees and status, read concurrently in `worktree`.
async fn read_info(runner: &GitRunner, worktree: &Path) -> Result<RepoInfo> {
    let (head, refs, remotes, worktrees, status) = tokio::join!(
        read_head(runner, worktree),
        read_refs(runner, worktree),
        read_remotes(runner, worktree),
        read_worktrees(runner, worktree),
        read_status(runner, worktree),
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
        status: status?,
    })
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
    fn live_updates_unavailable_is_reported_once() {
        let sink = RecordingSink::default();
        let reported = AtomicBool::new(false);
        let reason = |m: &str| CoreError::new(ErrorCode::Io, m).with_detail("os said no");
        report_live_updates_unavailable(&sink, 7, &reported, reason("Could not watch x."));
        report_live_updates_unavailable(&sink, 7, &reported, reason("Could not watch y."));
        let events = sink.events();
        assert_eq!(events.len(), 1);
        let (name, payload) = &events[0];
        assert_eq!(name, "repo-error");
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
