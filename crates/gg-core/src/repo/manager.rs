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
//! folded into that final refresh (see [`RepoManager::refresh`]).

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
    BodyHits, CommitDetails, FetchState, InProgress, InProgressKind, RefKind, RepoSnapshot,
    RowsPage, RunningOp, ViewOptions, WorktreeStatus, WorktreeUpstream,
};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::details::read_details;
use crate::git::progress::{read_in_progress_with, read_kind};
use crate::git::refs::{read_head, read_refs_and_heads, read_remotes};
use crate::git::runner::{CmdKind, GitRunner};
use crate::git::stash::read_stashes;
use crate::git::status::{STATUS_ARGS, read_status, read_status_and_conflicts};
use crate::git::worktrees::read_worktrees;
use crate::repo::fetch::{FETCH_CAP, FETCH_TIMEOUT, Schedule, unix_ms};
use crate::repo::loader::start_load;
use crate::repo::state::{RepoIdentity, RepoInfo, RepoState, Update, normalize_view};
use crate::watcher::{
    GitDirChange, RepoWatcher, TreeWatcher, watch_git_dir_reporting, watch_worktree_reporting,
};

/// How long an operation's final refresh (and a switch of worktree) waits for the working
/// tree's status.
const OP_STATUS_WAIT: Duration = Duration::from_secs(2);

/// Most worktrees whose state is read in the background at once, across repositories.
const WORKTREE_READS: usize = 4;

/// Remembers each repository's active worktree between runs (the app's store).
pub trait WorktreeMemory: Send + Sync + 'static {
    /// The worktree remembered as active for the repository whose main worktree is `root`,
    /// if one is (and its folder is still there).
    fn remembered(&self, root: &Path) -> Option<PathBuf>;
    /// Remembers `worktree` (`None`: the main one) as active for the repository at `root`.
    /// Called on Tokio's blocking pool.
    fn remember(&self, root: &Path, worktree: Option<&Path>);
    /// The base branch of the last pull request made from `branch` in the repository at
    /// `root`, if any.
    fn pr_base(&self, root: &Path, branch: &str) -> Option<String> {
        let _ = (root, branch);
        None
    }
    /// Remembers `base` as the base branch of the last pull request made from `branch`
    /// there. Called on Tokio's blocking pool.
    fn remember_pr_base(&self, root: &Path, branch: &str, base: &str) {
        let _ = (root, branch, base);
    }
}

/// Opens repositories and keeps their state. Create one per app with [`RepoManager::new`].
pub struct RepoManager {
    runner: GitRunner,
    pub(crate) sink: Arc<dyn EventSink>,
    pub(super) repos: Mutex<HashMap<u32, Arc<Repo>>>,
    /// gg is restarting to update: no operation or fetch the user asks for is admitted.
    /// Operations and user fetches are counted (`op_waiting`, `user_fetch`) with this held,
    /// and [`RepoManager::begin_restart`] looks for them with it held, so none slips in
    /// between.
    pub(crate) restarting: Mutex<bool>,
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
    /// Tests only: how long every operation waits after its turn, before its body runs.
    #[cfg(any(test, feature = "testutil"))]
    body_delay_ms: AtomicU64,
    /// Tests only: the next operation's body panics after running.
    #[cfg(any(test, feature = "testutil"))]
    op_panic: AtomicBool,
    #[cfg(any(test, feature = "testutil"))]
    op_fail: AtomicBool,
    /// Tests only: refreshes folded into an operation's final refresh so far.
    #[cfg(any(test, feature = "testutil"))]
    folded_refreshes: AtomicU64,
    /// Tests only: the next fetch the user asks for panics once it has its turn.
    #[cfg(any(test, feature = "testutil"))]
    pub(super) fetch_panic: AtomicBool,
    /// Tests only: how long a scheduled fetch pauses once it shows as running, before it
    /// goes on, in milliseconds.
    #[cfg(any(test, feature = "testutil"))]
    pub(super) scheduled_fetch_gap_ms: AtomicU64,
    /// When set, launches (editor, terminal, file manager, browser) are appended to this file
    /// instead of run (the dev server's `--record-launches`, for the UI's tests).
    pub(crate) launch_record: Mutex<Option<PathBuf>>,
    /// Tests only: when set, launches are recorded here instead of run.
    #[cfg(any(test, feature = "testutil"))]
    pub(crate) launches: Mutex<Option<Vec<Vec<String>>>>,
    /// Tests only: points where the next task to pass waits (see `pause_at`).
    #[cfg(any(test, feature = "testutil"))]
    pub(super) pauses: Mutex<HashMap<&'static str, PausePoint>>,
    /// Where each repository's active worktree is remembered, if anywhere.
    memory: Mutex<Option<Arc<dyn WorktreeMemory>>>,
    /// Numbers the writes to `memory`, so that they land in the order asked for: by root,
    /// the newest number written. A write older than the one written is dropped.
    remember_order: Arc<(AtomicU64, Mutex<HashMap<PathBuf, u64>>)>,
    /// Background reads of worktrees other than the active one take a permit each.
    worktree_reads: Arc<tokio::sync::Semaphore>,
    /// Tests only: how long every background worktree read waits before git starts.
    #[cfg(any(test, feature = "testutil"))]
    worktree_read_delay_ms: AtomicU64,
    /// Tests only: background worktree reads running now, and the most seen at once.
    #[cfg(any(test, feature = "testutil"))]
    worktree_reads_at_once: (AtomicU32, AtomicU32),
    /// Repositories (by main worktree) whose pull requests' unavailability has been said
    /// this run: said once per repository per run of the app.
    pub(super) prs_said: Mutex<std::collections::HashSet<PathBuf>>,
    /// Hosts other than github.com and `*.ghe.com`, by whether `gh` has credentials stored
    /// for them (they are GitHub then): asked once per host per run of the app.
    pub(super) gh_hosts: Mutex<std::collections::HashMap<String, bool>>,
    /// ssh address hosts (maybe aliases of the user's ssh config) → the real host name
    /// `ssh -G` gave (or the host itself when it could not tell): asked once per run.
    pub(super) ssh_hosts: Mutex<std::collections::HashMap<String, Option<String>>>,
    /// How long a `gh` command may run, in milliseconds.
    pub(super) gh_timeout_ms: AtomicU64,
    /// Tests and the devserver only: the `PATH` `gh` and `ssh` are found on and run with,
    /// instead of the login shell's (in a test build they never run without one).
    pub(super) tool_path: Mutex<Option<std::ffi::OsString>>,
    /// Tests only: the least time between two loads of the pull requests, instead of 60 s.
    #[cfg(any(test, feature = "testutil"))]
    pub(super) prs_every: Mutex<Option<Duration>>,
    /// Tests and the devserver only: the least time between two loads of a pull request's
    /// details, instead of 10 s.
    #[cfg(any(test, feature = "testutil", feature = "tool-path"))]
    pub(super) pr_details_every: Mutex<Option<Duration>>,
    /// Tests only: loads of pull request details started.
    #[cfg(any(test, feature = "testutil"))]
    pub(super) details_loads: AtomicU64,
    /// Tests only: how long comparing a branch with the remote's branches may take.
    #[cfg(any(test, feature = "testutil"))]
    pub(super) nearest_limit: Mutex<Option<Duration>>,
}

/// Tests only: a point in the core where the next task to pass stops. `reached` is
/// signalled when one arrives; it goes on once `release` is.
#[cfg(any(test, feature = "testutil"))]
#[derive(Clone, Default)]
pub struct PausePoint {
    pub reached: Arc<Notify>,
    pub release: Arc<Notify>,
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
    /// The main worktree (a bare repository's folder, when the main one is bare).
    pub(crate) root: PathBuf,
    /// The main worktree is a bare repository: there is no working tree there, and the
    /// first linked worktree with its folder stands in for it when the active one goes.
    root_bare: bool,
    /// That no worktree with a working tree is left has been said (once).
    no_worktree_said: AtomicBool,
    /// The active worktree; every command runs here. Changed only with the operation queue
    /// held (see [`RepoManager::set_active_worktree`]), together with `Slot::active`.
    worktree: Mutex<PathBuf>,
    pub(crate) common_dir: PathBuf,
    /// Runs this repository's commands (logged as this repository's).
    pub(crate) runner: GitRunner,
    pub(super) slot: RwLock<Slot>,
    /// The id of the log load that is running, if any. Set and cleared while holding the
    /// `slot` write lock, except by a load that dies, which clears its own id.
    pub(super) loading: watch::Sender<Option<u64>>,
    /// The id of the last load that ended without finishing; the next refresh reloads.
    pub(super) abandoned_load: AtomicU64,
    /// Serializes refreshes and view-option changes.
    pub(super) refresh_lock: tokio::sync::Mutex<()>,
    /// A refresh was asked for that no refresh has started on yet.
    refresh_wanted: AtomicBool,
    pub(super) load_task: Mutex<Option<JoinHandle<()>>>,
    /// Sequence number of the last snapshot emitted; older ones are dropped.
    emitted: Mutex<u64>,
    /// Refreshes on git-dir changes; `None` once closed, or if watching failed.
    watcher: Mutex<Option<RepoWatcher>>,
    /// Reads the status on changes to the active worktree's files; `None` once closed, while
    /// it starts, or if watching failed.
    tree_watcher: Mutex<Option<TreeWatcher>>,
    /// Number of the newest working-tree watch started; an older one that comes up late is
    /// dropped.
    tree_watch_seq: AtomicU64,
    /// Background work still starting: working-tree watches (reading what git ignores), and
    /// the first reads of the other worktrees (waiting for the log).
    background_starting: AtomicU32,
    /// That live updates are missing (the git dir or the working tree cannot be watched)
    /// has been said: once per repository, whichever it was.
    live_reported: Arc<AtomicBool>,
    /// The background reads of other worktrees' state, by path.
    worktree_reads: Mutex<HashMap<String, WorktreeRead>>,
    /// When the last fetch started, or when the repository was opened; the next scheduled
    /// fetch is one interval later.
    pub(super) fetched_at: Mutex<Instant>,
    /// Errors held for the UI; see [`Repo::report_error`].
    errors: Mutex<HeldErrors>,
    /// The last status read failed (said once, until one succeeds again).
    status_failed: AtomicBool,
    /// The last stash read failed (and was reported).
    stashes_failed: AtomicBool,
    /// No status read is running or wanted, and one has ended (see
    /// [`RepoManager::wait_status`]).
    status_idle: watch::Sender<bool>,
    /// The status reader: one per repository, reading outside `refresh_lock`.
    status_reader: Mutex<StatusReader>,
    /// Number of the newest status read started; a read applies only if it still is.
    status_seq: AtomicU64,
    /// The body search running, if any; a newer one aborts it.
    body_search: Mutex<Option<tokio::task::AbortHandle>>,
    /// Operations take turns here, in the order they were asked for (the mutex is fair;
    /// numbered requests join it in their numbers' order, see `op_order`).
    pub(crate) op_queue: tokio::sync::Mutex<()>,
    /// Operations waiting for their turn.
    pub(crate) op_waiting: AtomicU32,
    /// The order numbered requests join `op_queue` in.
    pub(crate) op_order: crate::ops::order::ClientOrders,
    /// How the app named the commit coming in to the merge or rebase it started last.
    pub(crate) incoming_name: Mutex<Option<crate::git::progress::KnownIncoming>>,
    /// The operation running its body, by id, and its Stop.
    pub(crate) running_stop: Mutex<Option<(u64, Arc<crate::git::runner::Stop>)>>,
    /// A fetch the user asked for is waiting for its turn or running; another one asked for
    /// meanwhile returns at once.
    pub(super) user_fetch: AtomicBool,
    /// Stops the scheduled fetch running now, if any.
    pub(super) scheduled_cancel: Mutex<Option<Arc<Notify>>>,
    /// Held by a scheduled fetch from before it shows as running until it ends;
    /// cancelling one waits for it.
    pub(super) scheduled_running: Arc<tokio::sync::Mutex<()>>,
    /// Whether the pull requests are being loaded, and when they last were.
    pub(super) prs_gate: Mutex<super::prs::PrGate>,
    /// Pull request loads ended so far (tests wait on it).
    pub(super) prs_loads: watch::Sender<u64>,
    /// Each pull request's last (or running) load of its details, by number.
    pub(super) pr_details: Mutex<HashMap<u32, super::pr_details::DetailsLoad>>,
    /// Each worktree's last (or running) read of its branch's committed files, by path.
    pub(super) branch_files: Mutex<HashMap<String, super::branch_files::BranchFilesLoad>>,
    /// Each branch's last (or running) answer for its base (steps 2 to 4 of `repo/base.rs`).
    pub(super) branch_bases: Mutex<HashMap<String, super::base::BaseLoad>>,
}

/// Whether a status read is running and whether another was asked for meanwhile.
#[derive(Default)]
struct StatusReader {
    running: bool,
    again: bool,
}

/// What a background read of a worktree reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ReadKind {
    /// Only its commits on no remote.
    Unpushed,
    /// Its status and what git is stopped in there, and its commits on no remote.
    Full,
}

/// A background read of one worktree that is running, and the one asked for meanwhile.
struct WorktreeRead {
    again: Option<ReadKind>,
}

/// What is known of a worktree beside what `git worktree list` says, read in the background
/// (the active worktree's status and in-progress state are the snapshot's own).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct WorktreeExtra {
    status: Option<WorktreeStatus>,
    in_progress: Option<InProgressKind>,
    unpushed: Option<u32>,
    /// The HEAD the status was read at; `None` while it has not been.
    status_at: Option<Option<String>>,
    /// The HEAD and the remote-tracking refs (see [`remotes_key`]) `unpushed` was counted
    /// for; `None` while it has not been.
    unpushed_at: Option<(Option<String>, u64)>,
    /// When the window's gaining focus last had it read: focus reads it again only
    /// [`FOCUS_READ_EVERY`] later.
    focus_read_at: Option<Instant>,
}

/// Least time between two reads of one other worktree that the window gaining focus asks
/// for (reads its git dir or a moved HEAD asks for are not held back).
const FOCUS_READ_EVERY: Duration = Duration::from_secs(30);

/// What a background read of a worktree found: the HEAD and remote-tracking refs it counted
/// against, the count, and (a full read) the status and what git is stopped in.
struct WorktreeReadOut {
    at: (Option<String>, u64),
    unpushed: Option<u32>,
    tree: Option<(WorktreeStatus, Option<InProgressKind>)>,
}

/// A key that changes whenever a remote-tracking branch of `info` moves (or comes or goes):
/// commits on no remote are counted again then.
fn remotes_key(info: &RepoInfo) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for r in info.refs.iter().filter(|r| r.kind == RefKind::Remote) {
        r.name.hash(&mut h);
        r.hash.hash(&mut h);
    }
    h.finish()
}

/// `repo-error` payloads of a repository nobody has asked for yet (opened in the background
/// at start), kept until `open` adopts it. Only the latest of each kind is kept.
#[derive(Default)]
struct HeldErrors {
    /// The UI has asked for this repository: errors are emitted as they happen.
    adopted: bool,
    load: Option<serde_json::Value>,
    live_updates: Option<serde_json::Value>,
    tree_watch: Option<serde_json::Value>,
    status: Option<serde_json::Value>,
    stashes: Option<serde_json::Value>,
    prs: Option<serde_json::Value>,
}

/// What a held `repo-error` is about.
#[derive(Clone, Copy)]
pub(super) enum ErrorKind {
    /// The log could not be read.
    Load,
    /// The git dir cannot be watched.
    LiveUpdates,
    /// The active worktree's files cannot be watched; or the active worktree went away.
    TreeWatch,
    /// `git status` failed or took too long.
    Status,
    /// The stash list could not be read.
    Stashes,
    /// Pull requests cannot be shown (said once per repository per run).
    Prs,
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
    /// A status read that ended while an operation ran, with its read number: kept for the
    /// operation's final refresh rather than applied mid-operation.
    held_status: Option<(u64, TreeState)>,
    /// Sequence number of the last snapshot built for emitting (or for an operation's
    /// outcome): the snapshots' `seq`.
    emit_seq: u64,
    /// The active worktree's path, as the snapshot shows it (`Repo::worktree` moves first;
    /// this with the refresh that reads the worktree switched to, see `switch_to`).
    active: String,
    /// The worktree commands run in now, when that is not yet `active`: the next `apply`
    /// makes it so, together with what it read there. `switch_known`: whether that read
    /// includes the worktree's status.
    switch_to: Option<String>,
    switch_known: bool,
    /// The active worktree is being switched: status reads are held for the switch, and
    /// refreshes are left to it, as during an operation.
    switching: bool,
    /// A status of the active worktree has been read (until then its `status` is unknown).
    active_status_known: bool,
    /// What is known of each worktree beside `git worktree list`, by path.
    extras: HashMap<String, WorktreeExtra>,
    /// The open pull requests, as last loaded (see `repo/prs.rs`).
    pub(super) prs: super::prs::PrSlot,
}

impl Slot {
    /// The state a new refresh compares against: the pending one when a reload is underway.
    pub(super) fn target(&self) -> &RepoState {
        self.pending.as_ref().unwrap_or(&self.current)
    }

    /// Makes `switch_to`, if set, the active worktree. The worktree left keeps the status it
    /// had, as one of the others.
    fn take_switch(&mut self) {
        let Some(next) = self.switch_to.take() else {
            return;
        };
        let left = std::mem::replace(&mut self.active, next);
        if self.active_status_known {
            let info = self.target().info();
            let head = info
                .worktrees
                .iter()
                .find(|w| w.path == left)
                .map(|w| w.head.clone());
            let (status, in_progress) = (
                info.status.clone(),
                info.in_progress.as_ref().map(|p| p.kind),
            );
            let extra = self.extras.entry(left).or_default();
            extra.status = Some(status);
            extra.in_progress = in_progress;
            extra.status_at = head;
        }
        self.active_status_known = self.switch_known;
    }

    /// Makes the pending state current, under a new generation.
    pub(super) fn promote_pending(&mut self) {
        if let Some(mut next) = self.pending.take() {
            next.generation = self.current.generation + 1;
            self.current = next;
        }
    }

    /// The served state's snapshot, except that `viewOptions` are the newest ones asked for,
    /// even while the reload they started is still loading, and `inProgress` and `status` are
    /// the newest read (read together, they must agree; neither depends on the rows).
    fn snapshot(&self, id: &RepoIdentity, loading: bool) -> RepoSnapshot {
        let mut snapshot = self.current.snapshot(id, loading, &self.fetch);
        if let Some(pending) = &self.pending {
            snapshot.view_options = pending.view().clone();
            snapshot.in_progress.clone_from(&pending.info().in_progress);
            snapshot.status.clone_from(&pending.info().status);
            snapshot.uncommitted_subject = crate::dto::uncommitted_subject(&snapshot.status);
        }
        snapshot.op.clone_from(&self.op);
        snapshot.prs = self.prs.state;
        snapshot.seq = self.emit_seq;
        self.describe_worktrees(&mut snapshot);
        snapshot
    }

    /// Fills in the snapshot's active worktree and its worktrees' state: which is active,
    /// their upstreams (from the refs), and what has been read of them. The active worktree's
    /// status and in-progress state are the snapshot's own.
    fn describe_worktrees(&self, snapshot: &mut RepoSnapshot) {
        snapshot.active_worktree.clone_from(&self.active);
        let refs = &self.current.info().refs;
        for wt in &mut snapshot.worktrees {
            wt.active = wt.path == self.active;
            wt.upstream = wt.branch.as_deref().and_then(|branch| {
                let r = refs
                    .iter()
                    .find(|r| r.kind == RefKind::Branch && r.name == branch)?;
                Some(WorktreeUpstream {
                    name: r.upstream.clone()?,
                    ahead: r.ahead,
                    behind: r.behind,
                })
            });
            let extra = self.extras.get(&wt.path);
            wt.unpushed = extra.and_then(|e| e.unpushed);
            wt.pr = match (&self.prs.data, &wt.branch) {
                (Some(data), Some(branch)) => data.of_branch(branch).cloned(),
                _ => None,
            };
            if wt.active {
                wt.status = self.active_status_known.then(|| snapshot.status.clone());
                wt.in_progress = snapshot.in_progress.as_ref().map(|p| p.kind);
            } else {
                wt.status = extra.and_then(|e| e.status.clone());
                wt.in_progress = extra.and_then(|e| e.in_progress);
            }
        }
    }

    /// Whether the commits may be incomplete because the last load failed or died.
    fn load_incomplete(&self, repo: &Repo) -> bool {
        self.load_failed
            || (self.load_id != 0 && repo.abandoned_load.load(Ordering::SeqCst) == self.load_id)
    }

    /// A snapshot to emit, stamped (and its `seq` set) so that emits arriving out of order
    /// can be dropped.
    pub(super) fn snapshot_to_emit(&mut self, id: &RepoIdentity, loading: bool) -> Stamped {
        self.emit_seq += 1;
        Stamped(self.emit_seq, self.snapshot(id, loading))
    }

    /// `to_emit` if there is a change to emit, with the snapshot to return (the emitted one,
    /// or else the snapshot as it is).
    fn emitted_or_current(
        &mut self,
        id: &RepoIdentity,
        loading: bool,
        changed: bool,
    ) -> (Option<Stamped>, RepoSnapshot) {
        if changed {
            let stamped = self.snapshot_to_emit(id, loading);
            let snapshot = stamped.1.clone();
            (Some(stamped), snapshot)
        } else {
            (None, self.snapshot(id, loading))
        }
    }
}

/// A snapshot and its emit sequence number.
pub(super) struct Stamped(u64, RepoSnapshot);

/// What a status read gives: the working tree's changes and what git is stopped in, read
/// together so that they land together.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TreeState {
    status: WorktreeStatus,
    in_progress: Option<InProgress>,
}

impl TreeState {
    /// The tree state `info` carries.
    fn of(info: &RepoInfo) -> Self {
        Self {
            status: info.status.clone(),
            in_progress: info.in_progress.clone(),
        }
    }
}

impl Repo {
    /// Whether local branch `name` was there when the branches were last read (no git runs).
    pub(crate) async fn knows_branch(&self, name: &str) -> bool {
        let slot = self.slot.read().await;
        slot.target()
            .info()
            .refs
            .iter()
            .any(|r| r.kind == RefKind::Branch && r.name == name)
    }

    /// Why the repository's pull requests are not shown, as the last load said (`None`
    /// while they are, or before any load).
    pub(crate) async fn prs_unavailable(&self) -> Option<crate::dto::PrUnavailable> {
        let slot = self.slot.read().await;
        (slot.prs.state.status == crate::dto::PrStatus::Unavailable)
            .then_some(slot.prs.state.reason)
            .flatten()
    }

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
            ErrorKind::TreeWatch => held.tree_watch = Some(payload),
            ErrorKind::Status => held.status = Some(payload),
            ErrorKind::Stashes => held.stashes = Some(payload),
            ErrorKind::Prs => held.prs = Some(payload),
        }
    }

    /// Forgets a held error of `kind` that no longer applies: a later load succeeded.
    pub(super) fn clear_held_error(&self, kind: ErrorKind) {
        let mut held = lock(&self.errors);
        match kind {
            ErrorKind::Load => held.load = None,
            ErrorKind::LiveUpdates => held.live_updates = None,
            ErrorKind::TreeWatch => held.tree_watch = None,
            ErrorKind::Status => held.status = None,
            ErrorKind::Stashes => held.stashes = None,
            ErrorKind::Prs => held.prs = None,
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
                held.tree_watch.take(),
                held.load.take(),
                held.status.take(),
                held.stashes.take(),
                held.prs.take(),
            ]
        };
        for payload in held.into_iter().flatten() {
            sink.emit("repo-error", payload);
        }
    }

    /// The active worktree: where every command runs.
    pub(crate) fn worktree(&self) -> PathBuf {
        lock(&self.worktree).clone()
    }

    /// Whether the repository has been closed.
    pub(crate) async fn is_closed(&self) -> bool {
        self.slot.read().await.closed
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

    /// Reads what the rows need (HEAD, refs, remotes, worktrees, stashes), with `tree` as the
    /// status and in-progress state. Stashes that cannot be read are none (see
    /// [`Repo::stashes_read`]).
    async fn read_info(&self, sink: &dyn EventSink, tree: TreeState) -> Result<RepoInfo> {
        let (info, stash_error) = read_info(&self.runner, &self.worktree(), tree).await?;
        self.stashes_read(sink, stash_error);
        Ok(info)
    }

    /// Notes how reading the stash list went: a failure is reported once with `repo-error`
    /// (until a read succeeds again), and the repository goes on without stashes.
    fn stashes_read(&self, sink: &dyn EventSink, error: Option<CoreError>) {
        match error {
            None => {
                if self.stashes_failed.swap(false, Ordering::SeqCst) {
                    self.clear_held_error(ErrorKind::Stashes);
                }
            }
            Some(reason) => {
                if !self.stashes_failed.swap(true, Ordering::SeqCst) {
                    self.report_error(
                        sink,
                        ErrorKind::Stashes,
                        stashes_unavailable(self.identity.repo_id, reason),
                    );
                }
            }
        }
    }
}

impl RepoManager {
    /// Why the git found is too old for the app, if it is.
    pub fn git_problem(&self) -> Option<String> {
        self.runner.version_problem()
    }

    pub fn new(runner: GitRunner, sink: Arc<dyn EventSink>) -> Arc<Self> {
        Arc::new(Self {
            runner,
            sink,
            repos: Mutex::new(HashMap::new()),
            restarting: Mutex::new(false),
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
            #[cfg(any(test, feature = "testutil"))]
            body_delay_ms: AtomicU64::new(0),
            #[cfg(any(test, feature = "testutil"))]
            op_panic: AtomicBool::new(false),
            #[cfg(any(test, feature = "testutil"))]
            op_fail: AtomicBool::new(false),
            #[cfg(any(test, feature = "testutil"))]
            folded_refreshes: AtomicU64::new(0),
            #[cfg(any(test, feature = "testutil"))]
            fetch_panic: AtomicBool::new(false),
            #[cfg(any(test, feature = "testutil"))]
            scheduled_fetch_gap_ms: AtomicU64::new(0),
            launch_record: Mutex::new(None),
            #[cfg(any(test, feature = "testutil"))]
            launches: Mutex::new(None),
            #[cfg(any(test, feature = "testutil"))]
            pauses: Mutex::new(HashMap::new()),
            memory: Mutex::new(None),
            remember_order: Arc::new((AtomicU64::new(0), Mutex::new(HashMap::new()))),
            worktree_reads: Arc::new(tokio::sync::Semaphore::new(WORKTREE_READS)),
            #[cfg(any(test, feature = "testutil"))]
            worktree_read_delay_ms: AtomicU64::new(0),
            #[cfg(any(test, feature = "testutil"))]
            worktree_reads_at_once: (AtomicU32::new(0), AtomicU32::new(0)),
            prs_said: Mutex::new(std::collections::HashSet::new()),
            gh_hosts: Mutex::new(std::collections::HashMap::new()),
            ssh_hosts: Mutex::new(std::collections::HashMap::new()),
            gh_timeout_ms: AtomicU64::new(duration_ms(crate::gh::GH_TIMEOUT)),
            tool_path: Mutex::new(None),
            #[cfg(any(test, feature = "testutil"))]
            prs_every: Mutex::new(None),
            #[cfg(any(test, feature = "testutil", feature = "tool-path"))]
            pr_details_every: Mutex::new(None),
            #[cfg(any(test, feature = "testutil"))]
            nearest_limit: Mutex::new(None),
            #[cfg(any(test, feature = "testutil"))]
            details_loads: AtomicU64::new(0),
        })
    }

    /// Where active worktrees and pull request bases are remembered, if anywhere.
    pub(crate) fn memory(&self) -> Option<Arc<dyn WorktreeMemory>> {
        lock(&self.memory).clone()
    }

    /// From now on, each repository's active worktree is remembered in `memory`, and a
    /// repository opened by its main worktree starts in the one remembered.
    pub fn set_worktree_memory(&self, memory: Arc<dyn WorktreeMemory>) {
        *lock(&self.memory) = Some(memory);
    }

    /// Remembers `worktree` as `repo`'s active one (the main one: `None`), in the background.
    fn remember_active(&self, repo: &Repo, worktree: &Path) {
        let Some(memory) = lock(&self.memory).clone() else {
            return;
        };
        let root = repo.root.clone();
        let worktree = (worktree != root).then(|| worktree.to_path_buf());
        let order = self.remember_order.clone();
        let n = order.0.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn_blocking(move || {
                // Held while writing: two writes for one repository never overlap, and one
                // asked for earlier never lands after a later one.
                let mut written = lock(&order.1);
                if written.get(&root).is_some_and(|&w| w > n) {
                    return;
                }
                memory.remember(&root, worktree.as_deref());
                written.insert(root, n);
            });
        }
    }

    /// Opens the repository containing `path`, starts loading its log in the background and
    /// starts watching its git dir. Returns once refs, HEAD and worktrees are read; the
    /// working tree's status (slow on a big one) is read beside the log, and the uncommitted
    /// row comes in when it is known (see [`RepoManager::wait_status`]).
    /// Opening any path of an already open repository returns that repository's snapshot (a
    /// path in a linked worktree also makes that worktree active; see
    /// [`RepoManager::open_noting_worktree`]).
    pub async fn open(self: &Arc<Self>, path: &Path) -> Result<RepoSnapshot> {
        Ok(self.open_as(path, true, false).await?.0)
    }

    /// Like [`RepoManager::open`], for a path the user opened (`explicit`: the app's Open,
    /// `gg`, a dropped folder) or a tab restored from the last session (not `explicit`). A
    /// path in a linked worktree makes that worktree active, in a repository already open
    /// too; so does the main worktree's folder opened explicitly (restored, it keeps the
    /// worktree remembered as active). When it cannot be made active now (an operation runs
    /// or waits there), its path comes back beside the snapshot, and the repository goes on
    /// in the worktree it was in.
    pub async fn open_noting_worktree(
        self: &Arc<Self>,
        path: &Path,
        explicit: bool,
    ) -> Result<(RepoSnapshot, Option<String>)> {
        self.open_as(path, true, explicit).await
    }

    /// Like [`RepoManager::open`], for a repository the UI has not asked for yet (opened
    /// ahead at start): its `repo-error`s are held until a later `open` adopts it.
    pub async fn open_in_background(self: &Arc<Self>, path: &Path) -> Result<RepoSnapshot> {
        Ok(self.open_as(path, false, false).await?.0)
    }

    /// Opens the repository holding `path` (see [`RepoManager::open_noting_worktree`]). A
    /// repository opened by its main worktree starts in the worktree remembered as active,
    /// if that is still one of its worktrees; else in the main one.
    async fn open_as(
        self: &Arc<Self>,
        path: &Path,
        adopt: bool,
        explicit: bool,
    ) -> Result<(RepoSnapshot, Option<String>)> {
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
                return self.reopened(&repo, &location, adopt, explicit).await;
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
        let (worktree, info, stash_error) = self.first_read(&runner, &location, explicit).await?;
        let root = info
            .worktrees
            .iter()
            .find(|w| w.is_main)
            .map_or_else(|| location.worktree.clone(), |w| PathBuf::from(&w.path));
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // Windows: ssh told not to ask, unless the user chose their ssh (read once, here).
        let ssh_command = runner.batch_ssh_command_for(&root).await;
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
            },
            root_bare: info.worktrees.iter().any(|w| w.is_main && w.bare),
            no_worktree_said: AtomicBool::new(false),
            root,
            worktree: Mutex::new(worktree.clone()),
            common_dir: location.common_dir,
            runner: runner.for_repo(id).with_ssh_command(ssh_command),
            slot: RwLock::new(Slot {
                current: RepoState::new(info, view, 1),
                pending: None,
                load_id: 0,
                load_failed: false,
                fetch: FetchState::default(),
                closed: false,
                op: None,
                held_status: None,
                emit_seq: 0,
                active: path_string(&worktree),
                switch_to: None,
                switch_known: false,
                switching: false,
                active_status_known: false,
                extras: HashMap::new(),
                prs: super::prs::PrSlot::default(),
            }),
            loading: watch::channel(None).0,
            abandoned_load: AtomicU64::new(0),
            refresh_lock: tokio::sync::Mutex::new(()),
            refresh_wanted: AtomicBool::new(false),
            load_task: Mutex::new(None),
            emitted: Mutex::new(0),
            watcher: Mutex::new(None),
            tree_watcher: Mutex::new(None),
            tree_watch_seq: AtomicU64::new(0),
            background_starting: AtomicU32::new(0),
            live_reported: watch_reported.clone(),
            worktree_reads: Mutex::new(HashMap::new()),
            fetched_at: Mutex::new(Instant::now()),
            errors: Mutex::new(HeldErrors {
                adopted: adopt,
                ..HeldErrors::default()
            }),
            status_failed: AtomicBool::new(false),
            stashes_failed: AtomicBool::new(false),
            status_idle: watch::channel(false).0,
            status_reader: Mutex::new(StatusReader::default()),
            status_seq: AtomicU64::new(0),
            body_search: Mutex::new(None),
            op_queue: tokio::sync::Mutex::new(()),
            op_waiting: AtomicU32::new(0),
            op_order: Default::default(),
            incoming_name: Mutex::new(None),
            running_stop: Mutex::new(None),
            user_fetch: AtomicBool::new(false),
            scheduled_cancel: Mutex::new(None),
            scheduled_running: Arc::new(tokio::sync::Mutex::new(())),
            prs_gate: Mutex::new(super::prs::PrGate::default()),
            prs_loads: watch::channel(0).0,
            pr_details: Mutex::new(HashMap::new()),
            branch_files: Mutex::new(HashMap::new()),
            branch_bases: Mutex::new(HashMap::new()),
        });
        repo.stashes_read(self.sink.as_ref(), stash_error);

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
            let location = Location {
                worktree: location.worktree,
                common_dir: existing.common_dir.clone(),
                is_main: location.is_main,
            };
            return self.reopened(&existing, &location, adopt, explicit).await;
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
        // Status beside the log; the other worktrees' state once the log is read (so they do
        // not slow its first load); the working tree's files watched from now on.
        self.request_status(&repo);
        self.start_tree_watch(&repo);
        {
            let (m, repo) = (self.clone(), repo.clone());
            let mut loading = repo.loading.subscribe();
            repo.background_starting.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let _started = Started(&repo.background_starting);
                let _ = loading.wait_for(Option::is_none).await;
                m.read_worktrees_as_needed(&repo, &GitDirChange::default())
                    .await;
            });
        }
        if !location.is_main || explicit {
            self.remember_active(&repo, &worktree);
        }
        Ok((snapshot, None))
    }

    /// The worktree a repository newly opened at `location` starts in, and what is read
    /// there: the worktree opened; for the main one not opened `explicit`ly (a restored tab),
    /// the worktree remembered as active, if it is still one of this repository's (and not
    /// prunable), else the main one.
    async fn first_read(
        &self,
        runner: &GitRunner,
        location: &Location,
        explicit: bool,
    ) -> Result<(PathBuf, RepoInfo, Option<CoreError>)> {
        let remembered = if location.is_main && !explicit {
            let memory = lock(&self.memory).clone();
            memory
                .and_then(|m| m.remembered(&location.worktree))
                .map(|p| canonical(&p))
                .filter(|p| *p != location.worktree)
        } else {
            None
        };
        if let Some(remembered) = remembered
            && let Ok((info, stash_error)) =
                read_info(runner, &remembered, TreeState::default()).await
        {
            let main = info.worktrees.iter().find(|w| w.is_main);
            let listed = info
                .worktrees
                .iter()
                .any(|w| !w.prunable && Path::new(&w.path) == remembered);
            if listed && main.is_some_and(|m| Path::new(&m.path) == location.worktree) {
                return Ok((remembered, info, stash_error));
            }
        }
        let (info, stash_error) =
            read_info(runner, &location.worktree, TreeState::default()).await?;
        Ok((location.worktree.clone(), info, stash_error))
    }

    /// The answer to opening `location` in `repo`, already open: a worktree other than the
    /// active one is made active (the main one only when opened `explicit`ly), if no
    /// operation runs or waits (else its path comes back beside the snapshot).
    async fn reopened(
        self: &Arc<Self>,
        repo: &Arc<Repo>,
        location: &Location,
        adopt: bool,
        explicit: bool,
    ) -> Result<(RepoSnapshot, Option<String>)> {
        if adopt {
            repo.adopt(self.sink.as_ref());
        }
        if (location.is_main && !explicit) || location.worktree == repo.worktree() {
            return Ok((repo.snapshot().await, None));
        }
        let Some(_queue) = idle_queue(repo) else {
            return Ok((repo.snapshot().await, Some(path_string(&location.worktree))));
        };
        let snapshot = self.switch_active(repo, &location.worktree, true).await?;
        self.remember_active(repo, &location.worktree);
        Ok((snapshot, None))
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
                if let Some(tree) = m.read_status_now(&repo).await {
                    let _ = m.apply_status(&repo, tree, seq).await;
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

    /// Reads the working tree's status and what git is stopped in (one more cheap read,
    /// using the status's conflicted paths), giving up after the status timeout. A failure
    /// is said once, as `repo-error`, until a read succeeds again: `None`.
    async fn read_status_now(self: &Arc<Self>, repo: &Repo) -> Option<TreeState> {
        let worktree = repo.worktree();
        if worktree != repo.root && !worktree.is_dir() {
            // The active worktree is gone: a refresh goes back to the main one and says so,
            // once; this is no failure of the status to report.
            self.spawn_refresh(repo.identity.repo_id);
            return None;
        }
        let timeout = Duration::from_millis(self.status_timeout_ms.load(Ordering::Relaxed));
        let started = std::time::Instant::now();
        let read = tokio::time::timeout(timeout, async {
            #[cfg(any(test, feature = "testutil"))]
            {
                let delay = self.status_delay_ms.load(Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            let (status, conflicted) = read_status_and_conflicts(&repo.runner, &worktree).await?;
            let known = lock(&repo.incoming_name).clone();
            let in_progress =
                read_in_progress_with(&repo.runner, &worktree, conflicted, known.as_ref()).await?;
            if in_progress.is_none() && known.is_some() {
                // What the app started has ended: its name is not for whatever comes next.
                // Unless a newer one was remembered while this read ran.
                let mut name = lock(&repo.incoming_name);
                if *name == known {
                    *name = None;
                }
            }
            Ok::<_, CoreError>(TreeState {
                status,
                in_progress,
            })
        })
        .await;
        let reason = match read {
            Ok(Ok(tree)) => {
                repo.status_failed.store(false, Ordering::SeqCst);
                repo.clear_held_error(ErrorKind::Status);
                return Some(tree);
            }
            Ok(Err(e)) => e,
            Err(_) => {
                // Dropping the command's future killed git.
                repo.runner
                    .log_timed_out(&worktree, &STATUS_ARGS, CmdKind::Read, started);
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

    /// Applies `tree` (status and in-progress state) from read number `seq`, unless a newer
    /// read has started since: the uncommitted row appears, changes or goes, in a new
    /// generation, and a change of either is emitted. While an operation runs it is held
    /// for the operation's final refresh instead. Takes `refresh_lock` only for this, which
    /// involves no git.
    async fn apply_status(&self, repo: &Arc<Repo>, tree: TreeState, seq: u64) -> Result<()> {
        let _serial = repo.refresh_lock.lock().await;
        if repo.status_seq.load(Ordering::SeqCst) != seq {
            return Ok(());
        }
        let first = {
            let mut slot = repo.slot.write().await;
            if slot.op.is_some() || slot.switching {
                // Shown with the operation's (or the switch's) end, not before it.
                slot.held_status = Some((seq, tree));
                return Ok(());
            }
            !std::mem::replace(&mut slot.active_status_known, true)
        };
        let mut info = repo.slot.read().await.target().info().clone();
        if TreeState::of(&info) == tree {
            if first {
                // Known now, though unchanged: the active worktree's status is no longer
                // unknown.
                self.emit_now(repo).await;
            }
            return Ok(());
        }
        info.status = tree.status;
        info.in_progress = tree.in_progress;
        // Read in the active worktree, but the rest of `info` is from before any switch.
        self.apply_with(repo, info, None, None, first, false)
            .await
            .map(|_| ())
    }

    /// Emits `repo-changed` with the snapshot as it is (a change `apply` cannot see, such as
    /// the state of a worktree read in the background).
    async fn emit_now(&self, repo: &Repo) {
        let stamped = {
            let mut slot = repo.slot.write().await;
            if slot.closed {
                return;
            }
            slot.snapshot_to_emit(&repo.identity, repo.is_loading())
        };
        repo.publish(self.sink.as_ref(), stamped);
    }

    /// Makes the worktree at `path` (one of the repository's, with its folder) the active
    /// one: from now on every operation runs there, and HEAD, the status, what git is stopped
    /// in and the uncommitted row are that worktree's. Returns the snapshot (a new `seq`).
    /// `busy` while an operation runs or waits on the repository; `bad_request` for a path
    /// that is not one of its worktrees; `path_missing` for one whose folder is gone. The
    /// choice is remembered for the next time the repository is opened.
    pub async fn set_active_worktree(
        self: &Arc<Self>,
        repo_id: u32,
        path: &Path,
    ) -> Result<RepoSnapshot> {
        let repo = self.get(repo_id)?;
        let Some(_queue) = idle_queue(&repo) else {
            return Err(CoreError::new(
                ErrorCode::Busy,
                "An operation is running or waiting on this repository. Switch worktrees once it is done.",
            ));
        };
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
                    "{} is the bare repository itself, which has no working tree to make active.",
                    path.display()
                ),
            ));
        }
        if wt.prunable || !target.is_dir() {
            return Err(CoreError::new(
                ErrorCode::PathMissing,
                format!(
                    "The worktree {} is gone (its folder is missing), so it cannot be made active.",
                    path.display()
                ),
            ));
        }
        if target == repo.worktree() {
            return Ok(self.outcome_snapshot(&repo).await);
        }
        let snapshot = self.switch_active(&repo, &target, true).await?;
        self.remember_active(&repo, &target);
        Ok(snapshot)
    }

    /// Switches `repo` to the worktree at `target` (canonical, checked by the caller, who
    /// holds the operation queue): the status there is read and waited for (up to
    /// [`OP_STATUS_WAIT`]) while refreshes and status reads are held, then everything is read
    /// there and applied at once, emitted (`force`: even if no row changed). The working
    /// tree watched moves with it.
    async fn switch_active(
        self: &Arc<Self>,
        repo: &Arc<Repo>,
        target: &Path,
        force: bool,
    ) -> Result<RepoSnapshot> {
        let floor = {
            let _serial = repo.refresh_lock.lock().await;
            let mut slot = repo.slot.write().await;
            if slot.closed {
                return Err(unknown_repo(repo.identity.repo_id));
            }
            slot.switching = true;
            drop(slot);
            self.set_active_path(repo, target).await?;
            repo.status_seq.load(Ordering::SeqCst)
        };
        self.request_status(repo);
        let mut idle = repo.status_idle.subscribe();
        let _ = tokio::time::timeout(OP_STATUS_WAIT, idle.wait_for(|idle| *idle)).await;

        let serial = repo.refresh_lock.lock().await;
        repo.refresh_wanted.store(false, Ordering::SeqCst);
        let tree = {
            let mut slot = repo.slot.write().await;
            slot.switching = false;
            let held = slot.held_status.take().filter(|(seq, _)| *seq > floor);
            slot.switch_known = held.is_some();
            held.map_or_else(TreeState::default, |(_, tree)| tree)
        };
        let read = repo.read_info(self.sink.as_ref(), tree).await;
        let snapshot = match read {
            Ok(info) => self.apply_forced(repo, info, None, None, force).await,
            Err(e) => Err(e),
        };
        drop(serial);
        if snapshot.is_err() {
            // What was held is not shown: read the status again.
            self.request_status(repo);
        }
        self.start_tree_watch(repo);
        self.read_worktrees_as_needed(repo, &GitDirChange::default())
            .await;
        snapshot
    }

    /// Points `repo` at the worktree at `target` (the caller holds the operation queue and
    /// refreshes it after, which shows the switch): commands run there from now on; status
    /// reads started before are dropped; what the app named an incoming commit no longer
    /// applies.
    async fn set_active_path(&self, repo: &Repo, target: &Path) -> Result<()> {
        let mut slot = repo.slot.write().await;
        if slot.closed {
            return Err(unknown_repo(repo.identity.repo_id));
        }
        slot.switch_to = Some(path_string(target));
        slot.switch_known = false;
        slot.held_status = None;
        *lock(&repo.worktree) = target.to_path_buf();
        *lock(&repo.incoming_name) = None;
        repo.status_seq.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// The active worktree of `repo` is gone (removed or moved outside the app): the main
    /// worktree becomes active, which is said once with `repo-error`, and remembered. If an
    /// operation runs or waits, that is left to its end (which checks again); the snapshot as
    /// it is returns meanwhile.
    async fn fall_back_to_main(self: &Arc<Self>, repo: &Arc<Repo>) -> Result<RepoSnapshot> {
        let Some(_queue) = idle_queue(repo) else {
            return Ok(repo.snapshot().await);
        };
        let gone = repo.worktree();
        let Some(target) = self.fallback_worktree(repo).await else {
            self.said_no_worktree(repo, &gone);
            return Ok(repo.snapshot().await);
        };
        if gone == target {
            // Another refresh did it meanwhile.
            return Ok(repo.snapshot().await);
        }
        let snapshot = self.switch_active(repo, &target, true).await?;
        self.said_gone(repo, &gone, &target);
        Ok(snapshot)
    }

    /// The worktree that becomes active when the active one goes: the main one, or, when the
    /// main one is a bare repository, the first linked worktree git lists with its folder;
    /// `None` when there is none.
    async fn fallback_worktree(&self, repo: &Repo) -> Option<PathBuf> {
        if !repo.root_bare {
            return Some(repo.root.clone());
        }
        let listed = read_worktrees(&repo.runner, &repo.root).await.ok()?;
        canonical_paths(listed)
            .into_iter()
            .find(|w| !w.bare && !w.prunable && Path::new(&w.path).is_dir())
            .map(|w| PathBuf::from(w.path))
    }

    /// Makes the main worktree active from within an operation's turn (the operation holds
    /// the queue): as [`RepoManager::set_active_worktree`] does, and remembered. Used before
    /// the active worktree is removed.
    pub(crate) async fn make_main_active(self: &Arc<Self>, repo: &Arc<Repo>) -> Result<()> {
        let root = repo.root.clone();
        self.switch_active(repo, &root, true).await?;
        self.remember_active(repo, &root);
        Ok(())
    }

    /// Says (`repo-error`) that the active worktree `gone` went away and `now` is active
    /// instead, and remembers that.
    fn said_gone(&self, repo: &Repo, gone: &Path, now: &Path) {
        let which = if now == repo.root {
            "the main worktree"
        } else {
            "the worktree"
        };
        repo.report_error(
            self.sink.as_ref(),
            ErrorKind::TreeWatch,
            serde_json::json!({
                "repoId": repo.identity.repo_id,
                "error": CoreError::new(
                    ErrorCode::PathMissing,
                    format!(
                        "The active worktree {} is gone, so gg works in {which} {} now.",
                        gone.display(),
                        now.display()
                    ),
                ),
            }),
        );
        self.remember_active(repo, now);
    }

    /// Says (`repo-error`) that the active worktree `gone` went away and no other worktree
    /// has a working tree (the main one is a bare repository).
    fn said_no_worktree(&self, repo: &Repo, gone: &Path) {
        if repo.no_worktree_said.swap(true, Ordering::SeqCst) {
            return;
        }
        repo.report_error(
            self.sink.as_ref(),
            ErrorKind::TreeWatch,
            serde_json::json!({
                "repoId": repo.identity.repo_id,
                "error": CoreError::new(
                    ErrorCode::BareRepo,
                    format!(
                        "The active worktree {} is gone, and this repository has no other working tree: {} is a bare repository.",
                        gone.display(),
                        repo.root.display()
                    ),
                ),
            }),
        );
    }

    /// Starts watching the active worktree's files (instead of the one watched before):
    /// changes ask for a status read, never a refresh. When the platform will not watch it,
    /// that is said once per repository, and the status is read when the window gains focus
    /// (the UI's refresh), as before.
    fn start_tree_watch(self: &Arc<Self>, repo: &Arc<Repo>) {
        let seq = repo.tree_watch_seq.fetch_add(1, Ordering::SeqCst) + 1;
        drop(lock(&repo.tree_watcher).take());
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (m, repo) = (Arc::downgrade(self), repo.clone());
        repo.background_starting.fetch_add(1, Ordering::SeqCst);
        runtime.spawn(async move {
            let _started = Started(&repo.background_starting);
            if repo.tree_watch_seq.load(Ordering::SeqCst) != seq || repo.is_closed().await {
                return;
            }
            let id = repo.identity.repo_id;
            let on_change = {
                let m = m.clone();
                move || {
                    if let Some(m) = m.upgrade()
                        && let Ok(repo) = m.get(id)
                    {
                        m.request_status(&repo);
                    }
                }
            };
            // A folder made later that cannot be watched (Linux, the inotify limit reached):
            // said once, like a watch refused at the start.
            let on_error = {
                let (m, repo) = (m.clone(), Arc::downgrade(&repo));
                move |reason: CoreError| {
                    if let (Some(m), Some(repo)) = (m.upgrade(), repo.upgrade())
                        && !repo.live_reported.swap(true, Ordering::SeqCst)
                    {
                        repo.report_error(
                            m.sink.as_ref(),
                            ErrorKind::TreeWatch,
                            tree_watch_unavailable(id, reason),
                        );
                    }
                }
            };
            let watched = watch_worktree_reporting(
                &repo.worktree(),
                repo.runner.clone(),
                on_change,
                on_error,
            )
            .await;
            if repo.tree_watch_seq.load(Ordering::SeqCst) != seq || repo.is_closed().await {
                return; // a newer watch (or none) is wanted now
            }
            match watched {
                Ok(watcher) => *lock(&repo.tree_watcher) = Some(watcher),
                Err(reason) => {
                    if let Some(m) = m.upgrade()
                        && !repo.live_reported.swap(true, Ordering::SeqCst)
                    {
                        repo.report_error(
                            m.sink.as_ref(),
                            ErrorKind::TreeWatch,
                            tree_watch_unavailable(id, reason),
                        );
                    }
                }
            }
        });
    }

    /// Asks for background reads of the worktrees whose state may have changed: one not
    /// read yet, one whose HEAD moved since its status was read, one `change` names (its git
    /// dir changed: its index, HEAD or in-progress state), and, for the commits on no remote,
    /// one whose HEAD or the remote-tracking branches moved since they were counted. The
    /// active worktree's status is read on its own; only its commits on no remote are read
    /// here. Forgets the worktrees no longer listed.
    async fn read_worktrees_as_needed(self: &Arc<Self>, repo: &Arc<Repo>, change: &GitDirChange) {
        self.read_worktrees_for(repo, change, false).await;
    }

    /// [`RepoManager::read_worktrees_as_needed`]; with `focus` (the window gained focus:
    /// `change` names all), a worktree read that way less than [`FOCUS_READ_EVERY`] ago is
    /// not read again for it.
    async fn read_worktrees_for(
        self: &Arc<Self>,
        repo: &Arc<Repo>,
        change: &GitDirChange,
        focus: bool,
    ) {
        let named = self.named_worktrees(repo, change);
        let wanted: Vec<(String, ReadKind)> = {
            let mut slot = repo.slot.write().await;
            if slot.closed {
                return;
            }
            let info = slot.target().info();
            let remotes = remotes_key(info);
            let worktrees = info.worktrees.clone();
            slot.extras
                .retain(|path, _| worktrees.iter().any(|w| w.path == *path));
            let now = Instant::now();
            let active_path = slot.active.clone();
            worktrees
                .iter()
                .filter_map(|wt| {
                    let extra = slot.extras.get(&wt.path);
                    let active = wt.path == active_path;
                    let focus_due = extra
                        .and_then(|e| e.focus_read_at)
                        .is_none_or(|at| now.duration_since(at) >= FOCUS_READ_EVERY);
                    let by_focus = change.all && (!focus || focus_due);
                    let full = !active
                        && !wt.prunable
                        && (by_focus
                            || named.contains(&wt.path)
                            || extra.is_none_or(|e| e.status_at.as_ref() != Some(&wt.head)));
                    let unpushed =
                        extra.is_none_or(|e| e.unpushed_at != Some((wt.head.clone(), remotes)));
                    if full && focus {
                        slot.extras
                            .entry(wt.path.clone())
                            .or_default()
                            .focus_read_at = Some(now);
                    }
                    if full {
                        Some((wt.path.clone(), ReadKind::Full))
                    } else {
                        unpushed.then(|| (wt.path.clone(), ReadKind::Unpushed))
                    }
                })
                .collect()
        };
        for (path, kind) in wanted {
            self.request_worktree_read(repo, path, kind);
        }
    }

    /// The paths of the worktrees whose git dir `change` names: the main worktree for a change
    /// at the top of the common git dir, a linked one for a change in its admin folder
    /// (`worktrees/<name>/`, whose `gitdir` file says where it is).
    fn named_worktrees(&self, repo: &Repo, change: &GitDirChange) -> Vec<String> {
        let mut paths = Vec::new();
        if change.main {
            paths.push(path_string(&repo.root));
        }
        for name in &change.worktrees {
            let gitdir = repo.common_dir.join("worktrees").join(name).join("gitdir");
            if let Ok(text) = std::fs::read_to_string(gitdir)
                && let Some(worktree) = Path::new(text.trim_end_matches(['\n', '\r'])).parent()
            {
                paths.push(path_string(&canonical(worktree)));
            }
        }
        paths
    }

    /// Reads (in the background) what `kind` says of the worktree at `path`, unless a read of
    /// it is running: then once more after it. At most [`WORKTREE_READS`] run at once.
    fn request_worktree_read(self: &Arc<Self>, repo: &Arc<Repo>, path: String, kind: ReadKind) {
        {
            let mut reads = lock(&repo.worktree_reads);
            if let Some(running) = reads.get_mut(&path) {
                running.again = running.again.max(Some(kind));
                return;
            }
            reads.insert(path.clone(), WorktreeRead { again: None });
        }
        let (m, repo) = (self.clone(), repo.clone());
        tokio::spawn(async move {
            let mut kind = kind;
            loop {
                if !repo.is_closed().await {
                    let read = {
                        let _permit = m.worktree_reads.acquire().await;
                        m.read_worktree(&repo, &path, kind).await
                    };
                    m.apply_worktree_read(&repo, &path, read).await;
                }
                let mut reads = lock(&repo.worktree_reads);
                match reads.get_mut(&path).and_then(|r| r.again.take()) {
                    Some(next) => kind = next,
                    None => {
                        reads.remove(&path);
                        return;
                    }
                }
            }
        });
    }

    /// Reads the worktree at `path` (see [`ReadKind`]), each part given up on after the status
    /// timeout. Only the part read is `Some`; a part that fails is unknown.
    async fn read_worktree(&self, repo: &Repo, path: &str, kind: ReadKind) -> WorktreeReadOut {
        #[cfg(any(test, feature = "testutil"))]
        let _counted = {
            let (now, most) = &self.worktree_reads_at_once;
            most.fetch_max(now.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
            let delay = self.worktree_read_delay_ms.load(Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(delay)).await;
            Started(now)
        };
        let timeout = Duration::from_millis(self.status_timeout_ms.load(Ordering::Relaxed));
        let (head, counted) = {
            let slot = repo.slot.read().await;
            let info = slot.target().info();
            let head = info
                .worktrees
                .iter()
                .find(|w| w.path == path)
                .and_then(|w| w.head.clone());
            let at = (head, remotes_key(info));
            // Counted already for this HEAD and these remote-tracking refs: the count stands.
            let counted = slot
                .extras
                .get(path)
                .filter(|e| e.unpushed_at.as_ref() == Some(&at) && e.unpushed.is_some())
                .map(|e| e.unpushed);
            (at, counted)
        };
        let unpushed = match (&head.0, counted) {
            (_, Some(known)) => known,
            // Nothing committed yet: nothing to push.
            (None, None) => Some(0),
            (Some(hash), None) => {
                let args = ["rev-list", "--count", hash, "--not", "--remotes"];
                let count = repo.runner.run(&repo.root, &args, CmdKind::Read);
                match tokio::time::timeout(timeout, count).await {
                    Ok(Ok(out)) => out.text().trim().parse().ok(),
                    _ => None,
                }
            }
        };
        let mut out = WorktreeReadOut {
            at: head,
            unpushed,
            tree: None,
        };
        let dir = Path::new(path);
        if kind == ReadKind::Full && dir.is_dir() {
            let read = async {
                let status = read_status(&repo.runner, dir).await?;
                let kind = read_kind(&repo.runner, dir).await?;
                Ok::<_, CoreError>((status, kind))
            };
            if let Ok(Ok(tree)) = tokio::time::timeout(timeout, read).await {
                out.tree = Some(tree);
            }
        }
        out
    }

    /// Keeps what a background read found of the worktree at `path`, and emits it if that
    /// changes anything shown.
    async fn apply_worktree_read(&self, repo: &Repo, path: &str, read: WorktreeReadOut) {
        let stamped = {
            let mut slot = repo.slot.write().await;
            if slot.closed {
                return;
            }
            let extra = slot.extras.entry(path.to_owned()).or_default();
            let before = (extra.status.clone(), extra.in_progress, extra.unpushed);
            extra.unpushed = read.unpushed;
            extra.unpushed_at = Some(read.at.clone());
            if let Some((status, in_progress)) = read.tree {
                extra.status = Some(status);
                extra.in_progress = in_progress;
                extra.status_at = Some(read.at.0);
            }
            if (extra.status.clone(), extra.in_progress, extra.unpushed) == before {
                return;
            }
            slot.snapshot_to_emit(&repo.identity, repo.is_loading())
        };
        repo.publish(self.sink.as_ref(), stamped);
    }

    /// How long the last numbered request of client `client` waited for its turn in
    /// repository `repo_id` (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn last_turn_wait(&self, repo_id: u32, client: &str) -> Result<Duration> {
        Ok(self.get(repo_id)?.op_order.of(client).last_wait())
    }

    /// The most background worktree reads that have run at once so far (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn most_worktree_reads_at_once(&self) -> u32 {
        self.worktree_reads_at_once.1.load(Ordering::SeqCst)
    }

    /// Makes every background read of a worktree wait this long first (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn set_worktree_read_delay(&self, delay: Duration) {
        self.worktree_read_delay_ms
            .store(duration_ms(delay), Ordering::Relaxed);
    }

    /// Resolves once no background read of a worktree of repository `repo_id` runs and its
    /// working-tree watch has started (or failed to) (at once for an unknown id). Tests only.
    #[cfg(any(test, feature = "testutil"))]
    pub async fn wait_background(&self, repo_id: u32) {
        let Ok(repo) = self.get(repo_id) else {
            return;
        };
        while !lock(&repo.worktree_reads).is_empty()
            || repo.background_starting.load(Ordering::SeqCst) > 0
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The window gained focus: the worktrees other than the active one are read again in
    /// the background (their files are not watched), in every open repository.
    pub(super) fn read_worktrees_on_focus(self: &Arc<Self>) {
        let repos: Vec<Arc<Repo>> = lock(&self.repos).values().cloned().collect();
        for repo in repos {
            let m = self.clone();
            tokio::spawn(async move {
                let all = GitDirChange {
                    all: true,
                    ..GitDirChange::default()
                };
                m.read_worktrees_for(&repo, &all, true).await;
            });
        }
    }

    /// Closes a repository and stops its background load and its watcher. `busy` while an
    /// operation runs on it or waits for its turn: reopened, the repository would get a
    /// queue of its own, and two operations could run on it at once.
    pub async fn close(&self, repo_id: u32) -> Result<()> {
        let repo = self.get(repo_id)?;
        // Held until the repository is marked closed: an operation that takes its turn
        // after this finds it closed and runs nothing.
        let idle = repo.op_queue.try_lock().ok();
        if idle.is_none() || repo.op_waiting.load(Ordering::SeqCst) > 0 {
            let fetching =
                repo.user_fetch.load(Ordering::SeqCst) && repo.slot.read().await.op.is_none();
            return Err(if fetching {
                busy_with_fetch()
            } else {
                busy_with_op()
            });
        }
        lock(&self.repos)
            .remove(&repo_id)
            .ok_or_else(|| unknown_repo(repo_id))?;
        drop(lock(&repo.watcher).take());
        repo.tree_watch_seq.fetch_add(1, Ordering::SeqCst);
        drop(lock(&repo.tree_watcher).take());
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

    /// Stops admitting operations and fetches the user asks for, for a restart to update:
    /// `busy`, naming the repository, while one runs or waits anywhere (then nothing
    /// changes). Until [`RepoManager::end_restart`], each new one is refused (`busy`, "gg is
    /// restarting to update.").
    pub fn begin_restart(&self) -> Result<()> {
        let mut restarting = lock(&self.restarting);
        if let Some(name) = self.busy_repo() {
            return Err(CoreError::new(
                ErrorCode::Busy,
                format!(
                    "gg cannot restart while git is working in {name}. Restart once it is done."
                ),
            ));
        }
        *restarting = true;
        Ok(())
    }

    /// The restart did not happen (the install failed): operations are admitted again.
    pub fn end_restart(&self) {
        *lock(&self.restarting) = false;
    }

    /// Counts an operation or user fetch in, unless gg is restarting to update (`busy`).
    /// `count` runs with the admission lock held.
    pub(crate) fn admit(&self, count: impl FnOnce()) -> Result<()> {
        let restarting = lock(&self.restarting);
        if *restarting {
            return Err(restarting_error());
        }
        count();
        Ok(())
    }

    /// `busy` if gg is restarting to update (a hint for the caller; [`RepoManager::admit`]
    /// is what holds).
    pub fn refuse_if_restarting(&self) -> Result<()> {
        if *lock(&self.restarting) {
            return Err(restarting_error());
        }
        Ok(())
    }

    /// The name (its folder's) of an open repository where an operation runs or waits for
    /// its turn, or a fetch the user asked for runs; `None` when nothing does. Restarting gg
    /// to update it waits until then.
    pub fn busy_repo(&self) -> Option<String> {
        let repos: Vec<Arc<Repo>> = lock(&self.repos).values().cloned().collect();
        repos
            .iter()
            .find(|repo| {
                repo.op_queue.try_lock().is_err()
                    || repo.op_waiting.load(Ordering::SeqCst) > 0
                    || repo.user_fetch.load(Ordering::SeqCst)
            })
            .map(|repo| {
                repo.root.file_name().map_or_else(
                    || repo.root.to_string_lossy().into_owned(),
                    |n| n.to_string_lossy().into_owned(),
                )
            })
    }

    /// `Ok` if repository `repo_id` is open, else `unknown_repo`.
    pub fn check_open(&self, repo_id: u32) -> Result<()> {
        self.get(repo_id).map(|_| ())
    }

    pub async fn snapshot(&self, repo_id: u32) -> Result<RepoSnapshot> {
        Ok(self.get(repo_id)?.snapshot().await)
    }

    /// The snapshot of an operation's outcome that published nothing (it needed
    /// confirmation or was refused): as it is, under a `seq` of its own.
    pub(crate) async fn outcome_snapshot(&self, repo: &Repo) -> RepoSnapshot {
        let mut slot = repo.slot.write().await;
        slot.emit_seq += 1;
        slot.snapshot(&repo.identity, repo.is_loading())
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
        let (runner, worktree) = (repo.runner.clone(), repo.worktree());
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
        read_details(&repo.runner, &repo.worktree(), hash).await
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
        // `info` is what was read before any switch of worktree now underway: it must not be
        // shown as the worktree switched to (the switch shows it, with its own read).
        self.apply_with(&repo, info, Some(o), None, false, false)
            .await
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
    ///
    /// While an operation runs, the refresh is left to the refresh the operation ends with,
    /// which starts after it; the snapshot as it is returns at once. So the result of an
    /// operation arrives once, with its end, and never half-done. This holds for the
    /// refreshes the watcher asks for (usually for the operation's own changes) and for the
    /// ones the UI asks for (when the window regains focus). It is checked under
    /// `refresh_lock`, which an operation holds when it publishes its start, so no refresh
    /// started before an operation can apply after the operation has begun changing the
    /// repository.
    pub async fn refresh(self: &Arc<Self>, repo_id: u32) -> Result<RepoSnapshot> {
        let repo = self.get(repo_id)?;
        self.refresh_repo(&repo, true).await
    }

    /// Like [`RepoManager::refresh`], also while an operation runs: for a fetch, which only
    /// moves remote-tracking branches.
    pub(super) async fn refresh_now(self: &Arc<Self>, repo_id: u32) -> Result<RepoSnapshot> {
        let repo = self.get(repo_id)?;
        self.refresh_repo(&repo, false).await
    }

    /// [`RepoManager::refresh`] of `repo`; with `fold_into_op`, not while an operation runs.
    async fn refresh_repo(
        self: &Arc<Self>,
        repo: &Arc<Repo>,
        fold_into_op: bool,
    ) -> Result<RepoSnapshot> {
        repo.refresh_wanted.store(true, Ordering::SeqCst);
        let serial = repo.refresh_lock.lock().await;
        {
            let slot = repo.slot.read().await;
            // A switch of worktree reads everything again when it ends, like an operation.
            if slot.switching || (fold_into_op && slot.op.is_some()) {
                // `refresh_wanted` stays set; the operation's final refresh clears it.
                #[cfg(any(test, feature = "testutil"))]
                self.folded_refreshes.fetch_add(1, Ordering::SeqCst);
                return Ok(slot.snapshot(&repo.identity, repo.is_loading()));
            }
        }
        if !repo.refresh_wanted.swap(false, Ordering::SeqCst) {
            // A refresh that started after this call was made has already run.
            return Ok(repo.snapshot().await);
        }
        if active_is_gone(repo, None) {
            drop(serial);
            return self.fall_back_to_main(repo).await;
        }
        let tree = TreeState::of(repo.slot.read().await.target().info());
        let info = match repo.read_info(self.sink.as_ref(), tree).await {
            Ok(info) => info,
            Err(e) => {
                drop(serial);
                // No worktree any more (removed, and a folder of that name made again)?
                let listed = read_worktrees(&repo.runner, &repo.root).await;
                if let Ok(listed) = listed
                    && active_is_gone(repo, Some(&canonical_paths(listed)))
                {
                    return self.fall_back_to_main(repo).await;
                }
                return Err(e);
            }
        };
        if active_is_gone(repo, Some(&info.worktrees)) {
            drop(serial);
            return self.fall_back_to_main(repo).await;
        }
        let snapshot = self.apply(repo, info, None, None).await?;
        drop(serial);
        self.request_status(repo);
        self.read_worktrees_as_needed(repo, &GitDirChange::default())
            .await;
        Ok(snapshot)
    }

    /// How many refreshes have been folded into an operation's final refresh (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn folded_refreshes(&self) -> u64 {
        self.folded_refreshes.load(Ordering::SeqCst)
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

    /// Makes every operation wait this long after its turn's checks, before its body runs
    /// (tests only): what changes in that moment is what the body must notice.
    #[cfg(any(test, feature = "testutil"))]
    pub fn set_body_delay(&self, delay: Duration) {
        self.body_delay_ms
            .store(duration_ms(delay), Ordering::Relaxed);
    }

    /// The test delay of [`RepoManager::set_body_delay`]; zero outside tests.
    pub(crate) fn body_delay(&self) -> Duration {
        #[cfg(any(test, feature = "testutil"))]
        {
            Duration::from_millis(self.body_delay_ms.load(Ordering::Relaxed))
        }
        #[cfg(not(any(test, feature = "testutil")))]
        {
            Duration::ZERO
        }
    }

    /// Makes the next operation's body panic once it has run, after its test delay (tests
    /// only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn panic_next_op(&self) {
        self.op_panic.store(true, Ordering::SeqCst);
    }

    /// Makes the next operation's body end in an error once it has run (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn fail_next_op(&self) {
        self.op_fail.store(true, Ordering::SeqCst);
    }

    /// Whether the body that just ran is to end in an error (see
    /// [`RepoManager::fail_next_op`]).
    #[cfg(any(test, feature = "testutil"))]
    pub(crate) fn take_op_fail(&self) -> bool {
        self.op_fail.swap(false, Ordering::SeqCst)
    }

    /// Whether the body that just ran is to panic (see [`RepoManager::panic_next_op`]).
    #[cfg(any(test, feature = "testutil"))]
    pub(crate) fn take_op_panic(&self) -> bool {
        self.op_panic.swap(false, Ordering::SeqCst)
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
            slot.held_status = None;
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
    /// `repo-changed`. The working tree's status is read first and waited for up to
    /// [`OP_STATUS_WAIT`], so that the snapshot returned (the operation's outcome) and that
    /// event carry the status after the operation; a status that takes longer lands later,
    /// in its own `repo-changed`, as after any refresh. If the repository cannot be read,
    /// `op` is still cleared, and the snapshot is the last one read. `unknown_repo` once
    /// closed.
    pub(crate) async fn finish_op(
        self: &Arc<Self>,
        repo: &Arc<Repo>,
        id: u64,
    ) -> Result<RepoSnapshot> {
        // The operation may have run where the active worktree was removed: the repository
        // goes back to the main one (said once) before it is read. The queue is still held.
        if active_is_gone(repo, None) {
            let gone = repo.worktree();
            match self.fallback_worktree(repo).await {
                Some(target) => {
                    self.set_active_path(repo, &target).await?;
                    self.said_gone(repo, &gone, &target);
                    // The files watched are that worktree's from now on.
                    self.start_tree_watch(repo);
                }
                None => self.said_no_worktree(repo, &gone),
            }
        }
        // Reads numbered above `floor` start after the operation's changes.
        let floor = repo.status_seq.load(Ordering::SeqCst);
        self.request_status(repo);
        let mut idle = repo.status_idle.subscribe();
        let _ = tokio::time::timeout(OP_STATUS_WAIT, idle.wait_for(|idle| *idle)).await;

        let serial = repo.refresh_lock.lock().await;
        repo.refresh_wanted.store(false, Ordering::SeqCst);
        let tree = {
            let mut slot = repo.slot.write().await;
            let held = slot.held_status.take().filter(|(seq, _)| *seq > floor);
            if slot.switch_to.is_some() {
                // The active worktree went away while the operation ran: what was shown is
                // not the main worktree's.
                slot.switch_known = held.is_some();
                held.map_or_else(TreeState::default, |(_, tree)| tree)
            } else {
                held.map_or_else(|| TreeState::of(slot.target().info()), |(_, tree)| tree)
            }
        };
        match repo.read_info(self.sink.as_ref(), tree).await {
            Ok(info) => {
                let snapshot = self.apply(repo, info, None, Some(id)).await?;
                drop(serial);
                self.read_worktrees_as_needed(repo, &GitDirChange::default())
                    .await;
                Ok(snapshot)
            }
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
                    slot.emitted_or_current(&repo.identity, repo.is_loading(), mine)
                };
                if let Some(stamped) = to_emit {
                    repo.publish(self.sink.as_ref(), stamped);
                }
                drop(serial);
                // The status read (if it came) was not applied: read it again.
                self.request_status(repo);
                Ok(snapshot)
            }
        }
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

    /// The `PATH` programs are found with: the login shell's, as for git, if it was read.
    pub(crate) fn login_path(&self) -> Option<std::ffi::OsString> {
        self.runner.login_path().cloned()
    }

    /// The runner used for this repository's commands.
    pub fn runner(&self, repo_id: u32) -> Option<GitRunner> {
        self.get(repo_id).ok().map(|r| r.runner.clone())
    }

    /// Refreshes `repo_id` in the background (if there is a runtime to do it on), for a
    /// change the watcher saw (see [`RepoManager::refresh`]). Errors have no one
    /// to go to: a closed repository is expected, and anything else shows up on the next
    /// refresh the user asks for.
    pub(super) fn spawn_refresh(self: &Arc<Self>, repo_id: u32) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let m = self.clone();
            runtime.spawn(async move {
                let _ = m.refresh(repo_id).await;
            });
        }
    }

    /// The watcher callback of repository `repo_id`: a refresh, and a background read of the
    /// worktrees whose git dir changed (their status may have: a commit or `git add` there).
    /// Holds the manager weakly, so a watcher never keeps it alive.
    fn refresher(self: &Arc<Self>, repo_id: u32) -> impl Fn(GitDirChange) + Send + Sync + 'static {
        let m = Arc::downgrade(self);
        move |change| {
            let Some(m) = m.upgrade() else {
                return;
            };
            m.spawn_refresh(repo_id);
            if (change.main || change.all || !change.worktrees.is_empty())
                && let Ok(repo) = m.get(repo_id)
                && let Ok(runtime) = tokio::runtime::Handle::try_current()
            {
                runtime.spawn(async move {
                    m.read_worktrees_as_needed(&repo, &change).await;
                });
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
        self.apply_forced(repo, info, view, op_done, false).await
    }

    /// Like [`RepoManager::apply`]; with `force`, emits even when `info` changes nothing
    /// (something the slot holds beside it did: the active worktree, say).
    async fn apply_forced(
        &self,
        repo: &Arc<Repo>,
        info: RepoInfo,
        view: Option<ViewOptions>,
        op_done: Option<u64>,
        force: bool,
    ) -> Result<RepoSnapshot> {
        self.apply_with(repo, info, view, op_done, force, true)
            .await
    }

    /// Like [`RepoManager::apply_forced`]. With `takes_switch` (`info` was read in the
    /// worktree commands run in now), a switch of worktree underway is shown with it;
    /// without, it stays underway (`info` is from before it).
    pub(super) async fn apply_with(
        &self,
        repo: &Arc<Repo>,
        mut info: RepoInfo,
        view: Option<ViewOptions>,
        op_done: Option<u64>,
        force: bool,
        takes_switch: bool,
    ) -> Result<RepoSnapshot> {
        let (to_emit, snapshot) = {
            let mut slot = repo.slot.write().await;
            if slot.closed {
                return Err(unknown_repo(repo.identity.repo_id));
            }
            if takes_switch {
                slot.take_switch();
            }
            // The branches' pull requests, from the list last loaded (none read from git).
            info.pr_marks = slot
                .prs
                .data
                .as_ref()
                .map(|data| data.marks(&info.refs))
                .unwrap_or_default();
            let view = view.unwrap_or_else(|| slot.target().view().clone());
            let view = normalize_view(view, &info);
            let changed = if slot.load_incomplete(repo)
                || slot.target().needs_reload(&info, &view, repo.is_loading())
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
                // Not served yet: nothing visible changes until it replaces `current`, except
                // the in-progress state and the status, which the snapshot takes from it.
                let before = TreeState::of(pending.info());
                pending.update(info, view);
                TreeState::of(pending.info()) != before
            } else {
                let update = slot.current.update(info, view);
                if update == Update::Rows {
                    slot.current.generation += 1;
                }
                update != Update::Nothing
            };
            let op_cleared =
                op_done.is_some_and(|id| slot.op.as_ref().is_some_and(|op| op.id == id));
            if op_cleared {
                slot.op = None;
            }
            let changed = changed || op_cleared || force;
            slot.emitted_or_current(&repo.identity, repo.is_loading(), changed)
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
                    "--git-dir",
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
        let (Some(toplevel), Some(common_dir), Some(is_bare), Some(git_dir)) =
            (lines.next(), lines.next(), lines.next(), lines.next())
        else {
            return Err(not_a_repo().with_detail(text.clone()));
        };
        if is_bare.trim() == "true" {
            return Err(bare_repo());
        }
        // Relative dirs are relative to the directory git ran in.
        let common_dir = canonical(&path.join(common_dir));
        Ok(Location {
            worktree: canonical(Path::new(toplevel)),
            // A linked worktree has a git dir of its own, inside the common one.
            is_main: canonical(&path.join(git_dir)) == common_dir,
            common_dir,
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

/// The `repo-error` payload saying that changes to the active worktree's files are not seen
/// as they happen (`reason`), and when they are read instead.
fn tree_watch_unavailable(repo_id: u32, reason: CoreError) -> serde_json::Value {
    let mut error = CoreError::new(
        ErrorCode::Io,
        format!(
            "Changes to files are not seen as they happen in this repository ({}); they are read when the window comes back to the front.",
            reason.message.trim_end_matches('.')
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
    /// `worktree` is the main worktree.
    is_main: bool,
}

/// Whether `repo`'s active worktree is gone: it is not the main one, and its folder is
/// missing, or `listed` (git's list of worktrees, paths canonical, when one is at hand) does
/// not have it with its folder.
fn active_is_gone(repo: &Repo, listed: Option<&[crate::dto::WorktreeInfo]>) -> bool {
    let active = repo.worktree();
    if active == repo.root {
        return false;
    }
    !active.is_dir()
        || listed.is_some_and(|list| {
            !list
                .iter()
                .any(|w| !w.prunable && Path::new(&w.path) == active)
        })
}

/// `worktrees` with their paths canonical, as the snapshot lists them.
pub(crate) fn canonical_paths(
    mut worktrees: Vec<crate::dto::WorktreeInfo>,
) -> Vec<crate::dto::WorktreeInfo> {
    for wt in &mut worktrees {
        wt.path = path_string(&canonical(Path::new(&wt.path)));
    }
    worktrees
}

/// Counts down a count of things starting when dropped.
struct Started<'a>(&'a AtomicU32);

impl Drop for Started<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The operation queue of `repo`, held, when no operation runs or waits there.
fn idle_queue(repo: &Repo) -> Option<tokio::sync::MutexGuard<'_, ()>> {
    let guard = repo.op_queue.try_lock().ok()?;
    (repo.op_waiting.load(Ordering::SeqCst) == 0).then_some(guard)
}

/// Head, refs, remotes, worktrees and stashes, read concurrently in `worktree`, with `tree`
/// as the status and in-progress state. Stashes that cannot be read (a corrupt
/// `refs/stash`) are none, and the error comes back beside the info: the repository is
/// usable without them.
async fn read_info(
    runner: &GitRunner,
    worktree: &Path,
    tree: TreeState,
) -> Result<(RepoInfo, Option<CoreError>)> {
    let (head, refs, remotes, worktrees, stashes) = tokio::join!(
        read_head(runner, worktree),
        read_refs_and_heads(runner, worktree),
        read_remotes(runner, worktree),
        read_worktrees(runner, worktree),
        read_stashes(runner, worktree),
    );
    let worktrees = canonical_paths(worktrees?);
    let (refs, remote_heads) = refs?;
    let (stashes, stash_error) = match stashes {
        Ok(stashes) => (stashes, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    Ok((
        RepoInfo {
            head: head?,
            refs,
            remotes: remotes?,
            worktrees,
            status: tree.status,
            in_progress: tree.in_progress,
            stashes,
            pr_marks: Default::default(),
            remote_heads,
        },
        stash_error,
    ))
}

/// The `repo-error` payload saying that the stashes could not be read.
fn stashes_unavailable(repo_id: u32, reason: CoreError) -> serde_json::Value {
    let mut error = CoreError::new(
        reason.code,
        format!(
            "Could not read the stashes, so none are shown: {}",
            reason.message
        ),
    );
    error.detail = reason.detail;
    serde_json::json!({ "repoId": repo_id, "error": error })
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
pub(crate) fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| {
        // Gone: its folder's path, resolved, with its name, so it compares equal to the one
        // git lists (which resolved it when the worktree was made).
        match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => {
                dunce::canonicalize(parent).map_or_else(|_| path.to_path_buf(), |p| p.join(name))
            }
            _ => path.to_path_buf(),
        }
    })
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

fn restarting_error() -> CoreError {
    CoreError::new(ErrorCode::Busy, "gg is restarting to update.")
}

fn busy_with_fetch() -> CoreError {
    CoreError::new(
        ErrorCode::Busy,
        "A fetch is running on this repository. Close it once the fetch is done.",
    )
}

fn busy_with_op() -> CoreError {
    CoreError::new(
        ErrorCode::Busy,
        "An operation is running on this repository. Close it once the operation is done.",
    )
}

pub(crate) fn unknown_repo(repo_id: u32) -> CoreError {
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

    /// While a reload is pending, the snapshot takes what git is stopped in from it, and the
    /// working tree's status with it: the two are read together and must agree (a merge
    /// ended outside the app must not leave the old conflict count to show as leftovers).
    #[test]
    fn a_pending_reload_gives_the_snapshot_its_status_and_in_progress_together() {
        use crate::dto::{InProgress, InProgressKind, WorktreeStatus};
        let stopped = RepoInfo {
            status: WorktreeStatus {
                changed: 2,
                conflicted: 2,
                conflicted_paths: vec!["a.txt".into(), "b.txt".into()],
                ..Default::default()
            },
            in_progress: Some(InProgress {
                kind: InProgressKind::Merge,
                files: Vec::new(),
                file_count: 2,
                step: None,
                total: None,
                subject: None,
                incoming: None,
                rebasing: None,
                commit: None,
                empty: false,
            }),
            ..Default::default()
        };
        let done = RepoInfo {
            status: WorktreeStatus::default(),
            in_progress: None,
            ..Default::default()
        };
        let view = ViewOptions {
            branches: None,
            show_remotes: true,
            show_tags: true,
            show_stashes: true,
        };
        let slot = Slot {
            current: RepoState::new(stopped, view.clone(), 1),
            pending: Some(RepoState::new(done, view, 0)),
            load_id: 1,
            load_failed: false,
            fetch: FetchState::default(),
            closed: false,
            op: None,
            held_status: None,
            emit_seq: 0,
            active: "/r".into(),
            switch_to: None,
            switch_known: false,
            switching: false,
            active_status_known: true,
            extras: HashMap::new(),
            prs: super::super::prs::PrSlot::default(),
        };
        let id = RepoIdentity {
            repo_id: 1,
            name: "r".into(),
            root_path: "/r".into(),
        };
        let snap = slot.snapshot(&id, true);
        assert_eq!(snap.in_progress, None);
        assert_eq!(snap.status, WorktreeStatus::default());
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
