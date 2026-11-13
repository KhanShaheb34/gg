//! Fetching: `git fetch` on request and on a background schedule, and the window focus
//! that both the schedule and refresh-on-focus depend on.
//!
//! A fetch the user asks for runs like one in a terminal, hooks included. A scheduled fetch
//! happens without anyone asking, so it never runs the repository's hooks.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::Notify;
use tokio::time::Instant;

use crate::dto::FetchState;
use crate::error::{ErrorCode, Result};
use crate::git::runner::{CmdKind, git_failed};
use crate::repo::manager::{Repo, RepoManager, lock, unknown_repo};

/// Scheduled fetches keep running for this long after the window loses focus.
const FOCUS_GRACE: Duration = Duration::from_secs(10 * 60);
/// Arguments of every fetch.
const FETCH_ARGS: [&str; 3] = ["fetch", "--all", "--prune"];
/// A fetch running longer than this is killed.
pub(super) const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
/// `lastError` of a fetch that was killed for running too long.
const TIMED_OUT: &str = "Fetch timed out";
/// Where scheduled fetches look for hooks: nowhere.
const NULL_DEVICE: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

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
    /// Runs `git fetch --all --prune`, then refreshes. `fetch.running` is set (and
    /// `repo-changed` emitted) before git starts; when it ends, `lastAtMs` and `lastError`
    /// (git's first stderr line, "Fetch timed out" after 120 s, or `None`) are set and
    /// emitted. Only an unknown (or meanwhile closed) repository is an `Err`; a failed fetch,
    /// or a failed refresh after it, is `Ok`. Does nothing for a repository without remotes
    /// or while a fetch is running. The fetch runs on its own task, so dropping the returned
    /// future does not stop it and `running` is always cleared.
    pub async fn fetch(self: &Arc<Self>, repo_id: u32) -> Result<()> {
        self.fetch_by(repo_id, Initiator::User).await
    }

    /// Sets how long a fetch may run before it is killed (tests only; normally 120 s).
    #[cfg(any(test, feature = "testutil"))]
    pub fn set_fetch_timeout(&self, timeout: Duration) {
        self.fetch_timeout_ms.store(
            crate::repo::manager::duration_ms(timeout),
            Ordering::Relaxed,
        );
    }

    async fn fetch_by(self: &Arc<Self>, repo_id: u32, initiator: Initiator) -> Result<()> {
        let repo = self.get(repo_id)?;
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
        let task = tokio::spawn(self.clone().run_fetch(repo, initiator));
        task.await.unwrap_or(Ok(()))
    }

    /// Runs git for a fetch whose `running` is set, records the outcome and refreshes.
    async fn run_fetch(self: Arc<Self>, repo: Arc<Repo>, initiator: Initiator) -> Result<()> {
        let repo_id = repo.identity.repo_id;
        let no_hooks = format!("core.hooksPath={NULL_DEVICE}");
        let mut args: Vec<&str> = Vec::with_capacity(FETCH_ARGS.len() + 2);
        if initiator == Initiator::Schedule {
            args.extend(["-c", no_hooks.as_str()]);
        }
        args.extend(FETCH_ARGS);
        let timeout = Duration::from_millis(self.fetch_timeout_ms.load(Ordering::Relaxed));
        // On timeout the git child is dropped, which kills it.
        let run = repo
            .runner
            .run_unchecked(&repo.worktree, &args, CmdKind::Write);
        let error = match tokio::time::timeout(timeout, run).await {
            Err(_) => Some(TIMED_OUT.to_owned()),
            Ok(Ok(out)) if out.code == 0 => None,
            Ok(Ok(out)) => Some(git_failed(&FETCH_ARGS, out.code, &out.stderr).message),
            Ok(Err(e)) => Some(e.message),
        };
        let finished = {
            let mut slot = repo.slot.write().await;
            slot.fetch = FetchState {
                running: false,
                last_error: error,
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
        match self.refresh(repo_id).await {
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

    /// Records the window's focus. Gaining focus refreshes every open repository, which is
    /// how working-tree changes show up (only the git dir is watched).
    pub fn set_window_focused(self: &Arc<Self>, focused: bool) {
        lock(&self.schedule).focus = Focus {
            focused,
            since: Instant::now(),
        };
        self.schedule_changed.notify_one();
        if focused {
            for id in self.open_ids() {
                self.spawn_refresh(id);
            }
        }
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

/// Current time in unix milliseconds.
fn unix_ms() -> i64 {
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
