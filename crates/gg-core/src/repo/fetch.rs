//! Fetching: `git fetch` on request and on a background schedule, and the window focus
//! that both the schedule and refresh-on-focus depend on.
//!
//! A fetch the user asks for runs like one in a terminal, hooks included, and runs for as long
//! as git keeps reporting progress. A scheduled fetch happens without anyone asking, so it
//! never runs the repository's hooks and is killed after a fixed time.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{Notify, OwnedMutexGuard};
use tokio::time::Instant;

use crate::dto::FetchState;
use crate::error::{ErrorCode, Result};
use crate::git::runner::{CmdKind, Expired, TimeLimit};
use crate::repo::manager::{Repo, RepoManager, lock, unknown_repo};

/// Scheduled fetches keep running for this long after the window loses focus.
const FOCUS_GRACE: Duration = Duration::from_secs(10 * 60);
/// Arguments of every fetch.
const FETCH_ARGS: [&str; 3] = ["fetch", "--all", "--prune"];
/// A scheduled fetch running longer than this is killed, and so is a fetch the user asked
/// for that has written nothing for this long.
pub(super) const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
/// The longest a fetch the user asked for may run, even while git keeps writing.
pub(super) const FETCH_CAP: Duration = Duration::from_secs(2 * 60 * 60);
/// Most bytes of git's stderr kept as `lastErrorDetail`: the last ones.
const MAX_DETAIL: usize = 8 * 1024;
/// Where scheduled fetches look for hooks: nowhere.
const NULL_DEVICE: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

/// What a scheduled fetch holds from before it shows as running until it ends: the lock
/// that tells others it runs, and its cancel signal, already stored in `scheduled_cancel`.
/// Both are in place before `running` is set, so an operation or a user's fetch that takes
/// its turn meanwhile always finds the fetch to cancel and wait for, never a `running` that
/// seems to cover it.
struct Scheduled {
    _running: OwnedMutexGuard<()>,
    cancel: Arc<Notify>,
}

/// Clears `user_fetch` when dropped: when the task of a fetch the user asked for ends,
/// whether it ran, did not (no remotes, closed, one already running) or panicked.
struct ClearUserFetch(Arc<Repo>);

impl Drop for ClearUserFetch {
    fn drop(&mut self) {
        self.0.user_fetch.store(false, Ordering::SeqCst);
    }
}

/// Who started a fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Initiator {
    /// The user: hooks run, as in a terminal.
    User,
    /// The background schedule: hooks never run.
    Schedule,
}

/// What decides when scheduled fetches run.
pub(super) struct Schedule {
    /// Zero means off.
    interval: Duration,
    focus: Focus,
}

/// The window's focus and when it last changed.
#[derive(Debug, Clone, Copy)]
struct Focus {
    focused: bool,
    since: Instant,
}

impl Schedule {
    /// Fetching off, window focused.
    pub(super) fn new() -> Self {
        Self {
            interval: Duration::ZERO,
            focus: Focus {
                focused: true,
                since: Instant::now(),
            },
        }
    }
}

impl RepoManager {
    /// Runs `git fetch --all --prune`, then refreshes. It takes its turn in the repository's
    /// operation queue: it waits for the operations asked for before it (counted in their
    /// `op.queued`), and the ones asked for after it wait for it. `fetch.running` is set (and
    /// `repo-changed` emitted) before git starts; when it ends, `lastAtMs`, `lastError`
    /// (see [`fetch_error`]; "Fetch timed out" once git has written nothing for 120 s; `None`
    /// after a success) and `lastErrorDetail` (git's whole stderr) are set and emitted. git
    /// runs with `--progress`, so a long fetch that is getting somewhere keeps running. Only
    /// an unknown (or meanwhile closed) repository is an `Err`; a failed fetch, or a failed
    /// refresh after it, is `Ok`. Does nothing for a repository without remotes or while a
    /// fetch is running or waiting for its turn. The fetch runs on its own task, so dropping
    /// the returned future does not stop it and `running` is always cleared. While it waits
    /// or runs, closing the repository is refused as `busy`, as during an operation.
    pub async fn fetch(self: &Arc<Self>, repo_id: u32) -> Result<()> {
        self.fetch_by(repo_id, Initiator::User).await
    }

    /// Makes the next fetch the user asks for panic once it has its turn (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn panic_next_fetch(&self) {
        self.fetch_panic.store(true, Ordering::SeqCst);
    }

    /// Makes every scheduled fetch pause this long once it shows as running, before it goes
    /// on (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn set_scheduled_fetch_gap(&self, gap: Duration) {
        self.scheduled_fetch_gap_ms
            .store(crate::repo::manager::duration_ms(gap), Ordering::Relaxed);
    }

    /// Sets how long a fetch the user asked for may run in any case (tests only; normally
    /// 2 h).
    #[cfg(any(test, feature = "testutil"))]
    pub fn set_fetch_cap(&self, cap: Duration) {
        self.fetch_cap_ms
            .store(crate::repo::manager::duration_ms(cap), Ordering::Relaxed);
    }

    /// Sets how long a scheduled fetch may run, and a user's fetch stay silent, before it is
    /// killed (tests only; normally 120 s).
    #[cfg(any(test, feature = "testutil"))]
    pub fn set_fetch_timeout(&self, timeout: Duration) {
        self.fetch_timeout_ms.store(
            crate::repo::manager::duration_ms(timeout),
            Ordering::Relaxed,
        );
    }

    async fn fetch_by(self: &Arc<Self>, repo_id: u32, initiator: Initiator) -> Result<()> {
        let repo = self.get(repo_id)?;
        match initiator {
            // A fetch the user asks for waits for the operations asked for before it, and
            // holds up the ones asked for after it: it never runs beside one. It waits on
            // its own task, so a caller that goes away leaves no count behind.
            Initiator::User => {
                if repo.user_fetch.swap(true, Ordering::SeqCst) {
                    // The one waiting or running covers this one.
                    return Ok(());
                }
                let m = self.clone();
                let task = tokio::spawn(async move {
                    // However this task ends (a panic included), a fetch asked for later runs.
                    let _clear = ClearUserFetch(repo.clone());
                    repo.op_waiting.fetch_add(1, Ordering::SeqCst);
                    m.op_queue_changed(&repo).await;
                    let turn = repo.op_queue.lock().await;
                    repo.op_waiting.fetch_sub(1, Ordering::SeqCst);
                    m.cancel_scheduled_fetch(&repo).await;
                    #[cfg(any(test, feature = "testutil"))]
                    if m.fetch_panic.swap(false, Ordering::SeqCst) {
                        panic!("test: the fetch panicked");
                    }
                    let done = m.start_fetch(repo.clone(), None).await;
                    drop(turn);
                    done
                });
                task.await.unwrap_or(Ok(()))
            }
            // A scheduled one is skipped while an operation runs or waits, and cancelled
            // when one takes its turn while it runs (see `cancel_scheduled_fetch`); the next
            // one comes an interval later.
            Initiator::Schedule => {
                if repo.op_waiting.load(Ordering::SeqCst) > 0 || repo.op_queue.try_lock().is_err() {
                    return Ok(());
                }
                let running = repo.scheduled_running.clone().lock_owned().await;
                let cancel = Arc::new(Notify::new());
                *lock(&repo.scheduled_cancel) = Some(cancel.clone());
                let scheduled = Scheduled {
                    _running: running,
                    cancel: cancel.clone(),
                };
                let done = self.start_fetch(repo.clone(), Some(scheduled)).await;
                forget_cancel(&repo, &cancel);
                done
            }
        }
    }

    /// Marks a fetch of `repo` as running and runs it (see [`RepoManager::fetch`]): a
    /// scheduled one with what it holds, else one the user asked for.
    async fn start_fetch(
        self: &Arc<Self>,
        repo: Arc<Repo>,
        scheduled: Option<Scheduled>,
    ) -> Result<()> {
        let repo_id = repo.identity.repo_id;
        let started = {
            let mut slot = repo.slot.write().await;
            if slot.closed {
                return Err(unknown_repo(repo_id));
            }
            if slot.fetch.running || slot.target().info().remotes.is_empty() {
                return Ok(());
            }
            slot.fetch.running = true;
            slot.snapshot_to_emit(&repo.identity, repo.is_loading())
        };
        *lock(&repo.fetched_at) = Instant::now();
        repo.publish(self.sink.as_ref(), started);
        // No await between setting `running` and spawning: once set, the task clears it.
        let task = tokio::spawn(self.clone().run_fetch(repo, scheduled));
        task.await.unwrap_or(Ok(()))
    }

    /// Stops the scheduled fetch of `repo` running now, if any, and waits for it to end (git
    /// is asked to stop, then killed; see [`GitRunner::run_cancellable`]): called by an
    /// operation, or a fetch the user asked for, that has just taken its turn, so that
    /// nothing of ours runs beside it. The cancelled fetch shows no error.
    ///
    /// [`GitRunner::run_cancellable`]: crate::git::runner::GitRunner::run_cancellable
    pub(crate) async fn cancel_scheduled_fetch(&self, repo: &Repo) {
        if let Some(cancel) = lock(&repo.scheduled_cancel).take() {
            cancel.notify_one();
        }
        let _ended = repo.scheduled_running.lock().await;
    }

    /// Runs git for a fetch whose `running` is set, records the outcome and refreshes. A
    /// scheduled fetch can be cancelled (see [`RepoManager::cancel_scheduled_fetch`]): then
    /// the fetch state is left as it was before it, and nothing is refreshed.
    async fn run_fetch(self: Arc<Self>, repo: Arc<Repo>, held: Option<Scheduled>) -> Result<()> {
        let repo_id = repo.identity.repo_id;
        let scheduled = held.is_some();
        let initiator = if scheduled {
            Initiator::Schedule
        } else {
            Initiator::User
        };
        #[cfg(any(test, feature = "testutil"))]
        if scheduled {
            let gap = self.scheduled_fetch_gap_ms.load(Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(gap)).await;
        }
        let cancel = held
            .as_ref()
            .map_or_else(|| Arc::new(Notify::new()), |h| h.cancel.clone());
        // An operation or a user's fetch that has taken its turn since: this one does not
        // start (it may have signalled the cancel too).
        let skip = scheduled
            && (repo.op_waiting.load(Ordering::SeqCst) > 0 || repo.op_queue.try_lock().is_err());
        let no_hooks = format!("core.hooksPath={NULL_DEVICE}");
        let mut args: Vec<&str> = Vec::with_capacity(FETCH_ARGS.len() + 3);
        if initiator == Initiator::Schedule {
            args.extend(["-c", no_hooks.as_str()]);
        }
        args.extend(FETCH_ARGS);
        let timeout = Duration::from_millis(self.fetch_timeout_ms.load(Ordering::Relaxed));
        let limit = match initiator {
            Initiator::User => {
                // Progress goes to stderr even without a terminal; any output counts.
                args.push("--progress");
                let cap = Duration::from_millis(self.fetch_cap_ms.load(Ordering::Relaxed));
                TimeLimit::idle(timeout, cap)
            }
            Initiator::Schedule => TimeLimit::total(timeout),
        };
        // On timeout git and the transport helpers it started are killed.
        let run = if skip {
            None
        } else {
            Some(
                repo.runner
                    .run_cancellable(&repo.worktree, &args, CmdKind::Write, limit, &cancel)
                    .await,
            )
        };
        if scheduled {
            forget_cancel(&repo, &cancel);
        }
        let cancelled = match &run {
            None => true,
            Some(Ok(Err(gave_up))) => gave_up.limit == Expired::Cancelled,
            _ => false,
        };
        if cancelled {
            let ended = {
                let mut slot = repo.slot.write().await;
                slot.fetch.running = false;
                if slot.closed {
                    return Err(unknown_repo(repo_id));
                }
                slot.snapshot_to_emit(&repo.identity, repo.is_loading())
            };
            repo.publish(self.sink.as_ref(), ended);
            return Ok(());
        }
        let Some(run) = run else {
            return Ok(());
        };
        let (error, detail) = match run {
            Ok(Err(gave_up)) => {
                let stderr = without_progress(&gave_up.stderr);
                (
                    Some(timed_out(gave_up.limit)),
                    Some(stderr)
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| keep_tail(s, MAX_DETAIL)),
                )
            }
            Ok(Ok(out)) if out.code == 0 => (None, None),
            Ok(Ok(out)) => {
                let stderr = without_progress(&out.stderr);
                (
                    Some(fetch_error(&stderr, out.code)),
                    Some(stderr)
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| keep_tail(s, MAX_DETAIL)),
                )
            }
            Err(e) => (Some(e.message), e.detail.map(|d| keep_tail(d, MAX_DETAIL))),
        };
        let finished = {
            let mut slot = repo.slot.write().await;
            if initiator == Initiator::User {
                // From here on, a fetch asked for is a new one.
                repo.user_fetch.store(false, Ordering::SeqCst);
            }
            slot.fetch = FetchState {
                running: false,
                last_error: error,
                last_error_detail: detail,
                last_at_ms: Some(unix_ms()),
            };
            if slot.closed {
                return Err(unknown_repo(repo_id));
            }
            slot.snapshot_to_emit(&repo.identity, repo.is_loading())
        };
        repo.publish(self.sink.as_ref(), finished);
        // The watcher also sees the fetch's ref updates and refreshes ~300 ms later; that
        // second refresh finds nothing new and emits nothing. This one is kept so that the
        // state is current when `fetch` returns.
        match self.refresh_now(repo_id).await {
            Err(e) if e.code == ErrorCode::UnknownRepo => Err(e),
            _ => Ok(()),
        }
    }

    /// Sets how often every open repository is fetched in the background; 0 turns it off.
    /// Scheduled fetches run only while the window is focused or was within the last 10
    /// minutes. The first one is one interval after open (or after the last fetch).
    pub fn set_fetch_interval(self: &Arc<Self>, secs: u32) {
        lock(&self.schedule).interval = Duration::from_secs(u64::from(secs));
        self.schedule_changed.notify_one();
    }

    /// Records the window's focus, which scheduled fetches depend on. It refreshes nothing:
    /// the UI refreshes the repository it shows when the window comes back, and the others
    /// when they are shown, rather than every open repository at every activation.
    pub fn set_window_focused(self: &Arc<Self>, focused: bool) {
        lock(&self.schedule).focus = Focus {
            focused,
            since: Instant::now(),
        };
        self.schedule_changed.notify_one();
    }

    /// Starts the schedule task if it is not running, and has it recompute.
    pub(super) fn start_schedule(self: &Arc<Self>) {
        let mut task = lock(&self.schedule_task);
        if task.is_none() {
            *task = Some(tokio::spawn(run_schedule(
                Arc::downgrade(self),
                self.schedule_changed.clone(),
            )));
        }
        self.schedule_changed.notify_one();
    }

    /// Starts the scheduled fetches that are due; returns when the next one will be.
    fn start_due_fetches(self: &Arc<Self>) -> Option<Instant> {
        let (interval, focus) = {
            let s = lock(&self.schedule);
            (s.interval, s.focus)
        };
        let repos: Vec<Arc<Repo>> = lock(&self.repos).values().cloned().collect();
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        for repo in repos {
            let (start, at) = plan_fetch(now, interval, focus, &mut lock(&repo.fetched_at));
            if start {
                let m = self.clone();
                let id = repo.identity.repo_id;
                tokio::spawn(async move {
                    let _ = m.fetch_by(id, Initiator::Schedule).await;
                });
            }
            if let Some(at) = at {
                next = Some(next.map_or(at, |n| n.min(at)));
            }
        }
        next
    }
}

/// Removes `cancel` from `repo`'s `scheduled_cancel` if it is still there (not a later
/// scheduled fetch's).
fn forget_cancel(repo: &Repo, cancel: &Arc<Notify>) {
    let mut stored = lock(&repo.scheduled_cancel);
    if stored.as_ref().is_some_and(|c| Arc::ptr_eq(c, cancel)) {
        *stored = None;
    }
}

/// Whether a repository last fetched at `fetched_at` is to be fetched now (if so,
/// `fetched_at` becomes `now`, even if the fetch turns out to have nothing to do), and when
/// its next scheduled fetch is due, if ever.
fn plan_fetch(
    now: Instant,
    interval: Duration,
    focus: Focus,
    fetched_at: &mut Instant,
) -> (bool, Option<Instant>) {
    let start = fetch_due(now, interval, *fetched_at, focus) == Due::Now;
    if start {
        *fetched_at = now;
    }
    match fetch_due(now, interval, *fetched_at, focus) {
        Due::At(at) => (start, Some(at)),
        Due::Now | Due::Never => (start, None),
    }
}

/// The schedule task: starts due fetches, then sleeps until the next one is due or the
/// schedule changes. With nothing due ever (interval 0, no repositories, unfocused for too
/// long) it sleeps until the schedule changes. Ends with the manager.
async fn run_schedule(m: Weak<RepoManager>, changed: Arc<Notify>) {
    schedule_loop(&changed, || m.upgrade().map(|m| m.start_due_fetches())).await;
}

/// The loop of [`run_schedule`]: `tick` starts what is due and returns when to wake next
/// (`Some(None)`: only when `changed` is notified), or `None` to stop.
async fn schedule_loop(changed: &Notify, mut tick: impl FnMut() -> Option<Option<Instant>>) {
    while let Some(next) = tick() {
        match next {
            Some(at) => {
                tokio::select! {
                    () = tokio::time::sleep_until(at) => {}
                    () = changed.notified() => {}
                }
            }
            None => changed.notified().await,
        }
    }
}

/// When a repository's next scheduled fetch is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Due {
    Now,
    At(Instant),
    /// Not until the interval or the focus changes.
    Never,
}

/// When the scheduled fetch after one at `last` is due, at `now`, with `interval` (zero is
/// off) and the window's `focus`. Fetches happen only while the window is focused or was
/// focused within the last 10 minutes.
fn fetch_due(now: Instant, interval: Duration, last: Instant, focus: Focus) -> Due {
    if interval.is_zero() {
        return Due::Never;
    }
    let at = last + interval;
    if !focus.focused {
        let until = focus.since + FOCUS_GRACE;
        if now >= until || at >= until {
            return Due::Never;
        }
    }
    if at <= now { Due::Now } else { Due::At(at) }
}

/// The one line that says why a fetch that exited with `code` failed: git's first line
/// starting `fatal:` or `error:` (with `--all`, the progress lines of remotes that fetched
/// come before it), else its last non-empty line, else the exit code.
fn fetch_error(stderr: &str, code: i32) -> String {
    let lines = || stderr.lines().map(str::trim).filter(|l| !l.is_empty());
    lines()
        .find(|l| l.starts_with("fatal:") || l.starts_with("error:"))
        .or_else(|| lines().next_back())
        .map_or_else(
            || format!("git fetch exited with code {code}"),
            str::to_owned,
        )
}

/// `lastError` of a fetch that ran into `expired`.
fn timed_out(expired: Expired) -> String {
    match expired {
        Expired::Idle(idle) => format!("Fetch timed out: no output from git for {}", span(idle)),
        Expired::Total(total) => format!("Fetch timed out: still running after {}", span(total)),
        Expired::Cancelled => "Fetch cancelled".to_owned(),
    }
}

/// `d` for people: whole hours, minutes or seconds where it is one, else milliseconds.
fn span(d: Duration) -> String {
    let (ms, s) = (d.as_millis(), d.as_secs());
    if ms % 1000 != 0 {
        format!("{ms} ms")
    } else if s >= 3600 && s % 3600 == 0 {
        format!("{} h", s / 3600)
    } else if s >= 60 && s % 60 == 0 {
        format!("{} min", s / 60)
    } else {
        format!("{s} s")
    }
}

/// `text`, or its last `max` bytes (from a char boundary) after a "truncated" line.
fn keep_tail(text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("\u{2026} (truncated)\n{}", &text[start..])
}

/// `stderr` with each progress line (redrawn with carriage returns) reduced to its last
/// state, as a terminal would show it.
fn without_progress(stderr: &str) -> String {
    let mut out = String::with_capacity(stderr.len());
    for line in stderr.split_inclusive('\n') {
        let (body, end) = line.strip_suffix('\n').map_or((line, ""), |b| (b, "\n"));
        let body = body.strip_suffix('\r').unwrap_or(body);
        out.push_str(body.rsplit('\r').next().unwrap_or(body));
        out.push_str(end);
    }
    out
}

/// Current time in unix milliseconds.
pub(super) fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    fn focused(since: Instant) -> Focus {
        Focus {
            focused: true,
            since,
        }
    }

    fn unfocused(since: Instant) -> Focus {
        Focus {
            focused: false,
            since,
        }
    }

    /// The schedule loop over one repository, with real `plan_fetch` decisions and a
    /// paused clock: counts wake-ups (ticks) and fetches started.
    #[tokio::test(start_paused = true)]
    async fn loop_sleeps_while_unfocused_past_the_grace_and_resumes_on_focus() {
        struct State {
            interval: Duration,
            focus: Focus,
            fetched_at: Instant,
            ticks: u32,
            fetches: u32,
        }
        let start = Instant::now();
        let state = Arc::new(std::sync::Mutex::new(State {
            interval: 5 * MIN,
            focus: unfocused(start),
            fetched_at: start,
            ticks: 0,
            fetches: 0,
        }));
        let changed = Arc::new(Notify::new());
        let task = tokio::spawn({
            let (state, changed) = (state.clone(), changed.clone());
            async move {
                schedule_loop(&changed, || {
                    let mut s = lock(&state);
                    s.ticks += 1;
                    let (interval, focus) = (s.interval, s.focus);
                    let (fetch, next) =
                        plan_fetch(Instant::now(), interval, focus, &mut s.fetched_at);
                    s.fetches += u32::from(fetch);
                    Some(next)
                })
                .await;
            }
        });
        let counts = || {
            let s = lock(&state);
            (s.ticks, s.fetches)
        };
        let ms = Duration::from_millis(1);

        tokio::time::sleep(ms).await;
        assert_eq!(counts(), (1, 0), "first fetch is one interval away");
        // Unfocused since `start`: the fetch at +5 min is inside the 10-minute grace.
        tokio::time::sleep(5 * MIN).await;
        assert_eq!(counts(), (2, 1));
        // The next one (+10 min) is not: the loop now waits only for a schedule change.
        tokio::time::sleep(3 * 60 * MIN).await;
        assert_eq!(counts(), (2, 1), "woke up or fetched while long unfocused");

        lock(&state).focus = focused(Instant::now());
        changed.notify_one();
        tokio::time::sleep(ms).await;
        assert_eq!(
            counts(),
            (3, 2),
            "focus makes the overdue fetch run at once"
        );
        // And the schedule continues every interval.
        tokio::time::sleep(5 * MIN).await;
        assert_eq!(counts(), (4, 3));
        task.abort();
    }

    #[test]
    fn fetch_error_is_the_first_fatal_or_error_line_else_the_last() {
        let several = "From /tmp/origin\n   1a2b..3c4d  main -> origin/main\n\
                       fatal: '/tmp/gone' does not appear to be a git repository\n\
                       fatal: Could not read from remote repository.\n\
                       error: could not fetch zbroken\n";
        assert_eq!(
            fetch_error(several, 1),
            "fatal: '/tmp/gone' does not appear to be a git repository"
        );
        assert_eq!(
            fetch_error("From x\n  error: some refs failed\n", 1),
            "error: some refs failed"
        );
        assert_eq!(fetch_error("From x\nsomething odd\n\n", 1), "something odd");
        assert_eq!(fetch_error("  \n", 128), "git fetch exited with code 128");
    }

    #[test]
    fn timeouts_say_which_limit() {
        assert_eq!(
            timed_out(Expired::Idle(FETCH_TIMEOUT)),
            "Fetch timed out: no output from git for 2 min"
        );
        assert_eq!(
            timed_out(Expired::Total(FETCH_CAP)),
            "Fetch timed out: still running after 2 h"
        );
        assert_eq!(span(Duration::from_secs(90)), "90 s");
        assert_eq!(span(Duration::from_millis(700)), "700 ms");
    }

    #[test]
    fn detail_keeps_its_tail_on_a_char_boundary() {
        assert_eq!(keep_tail("short".into(), 8), "short");
        let long = format!("{}fatal: x", "\u{e9}".repeat(10));
        let kept = keep_tail(long, 9);
        assert!(kept.starts_with("\u{2026} (truncated)\n"), "{kept:?}");
        assert!(kept.ends_with("fatal: x"));
        assert!(kept.len() <= "\u{2026} (truncated)\n".len() + 9);
    }

    #[test]
    fn progress_lines_keep_their_last_state() {
        assert_eq!(
            without_progress(
                "remote: Counting objects:  50% (1/2)\rremote: Counting objects: 100% (2/2), done.\n\
                 Receiving objects:  10% (1/9)\rReceiving objects: 100% (9/9), done.\r\n\
                 fatal: the end\n"
            ),
            "remote: Counting objects: 100% (2/2), done.\n\
             Receiving objects: 100% (9/9), done.\n\
             fatal: the end\n"
        );
        assert_eq!(without_progress("no newline"), "no newline");
        assert_eq!(without_progress(""), "");
    }

    #[test]
    fn interval_zero_is_never() {
        let t = Instant::now();
        assert_eq!(fetch_due(t, Duration::ZERO, t, focused(t)), Due::Never);
        assert_eq!(
            fetch_due(t + 60 * MIN, Duration::ZERO, t, focused(t)),
            Due::Never
        );
    }

    #[test]
    fn focused_is_due_one_interval_after_the_last_fetch() {
        let t = Instant::now();
        let five = 5 * MIN;
        assert_eq!(fetch_due(t, five, t, focused(t)), Due::At(t + five));
        assert_eq!(
            fetch_due(t + 4 * MIN, five, t, focused(t)),
            Due::At(t + five)
        );
        assert_eq!(fetch_due(t + five, five, t, focused(t)), Due::Now);
        // Overdue (for example after the interval was shortened): now.
        assert_eq!(fetch_due(t + 60 * MIN, five, t, focused(t)), Due::Now);
    }

    #[test]
    fn unfocused_keeps_fetching_for_ten_minutes() {
        let t = Instant::now();
        let five = 5 * MIN;
        // Lost focus at t: fetches at t+5 still happen.
        assert_eq!(fetch_due(t, five, t, unfocused(t)), Due::At(t + five));
        assert_eq!(fetch_due(t + five, five, t, unfocused(t)), Due::Now);
        // The one after (t+10) would be at the end of the 10 minutes: never.
        assert_eq!(
            fetch_due(t + five, five, t + five, unfocused(t)),
            Due::Never
        );
        // Overdue but still within the 10 minutes: now.
        let blurred = t + 60 * MIN;
        assert_eq!(
            fetch_due(blurred + 9 * MIN, five, t, unfocused(blurred)),
            Due::Now
        );
    }

    #[test]
    fn unfocused_for_over_ten_minutes_is_never() {
        let t = Instant::now();
        let five = 5 * MIN;
        let blurred = t + MIN;
        assert_eq!(
            fetch_due(blurred + 10 * MIN, five, t, unfocused(blurred)),
            Due::Never
        );
        assert_eq!(
            fetch_due(blurred + 3 * 60 * MIN, five, t, unfocused(blurred)),
            Due::Never
        );
        // Focus comes back: the overdue fetch runs at once.
        let back = blurred + 3 * 60 * MIN;
        assert_eq!(fetch_due(back, five, t, focused(back)), Due::Now);
    }
}
