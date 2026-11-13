//! Integration tests for keeping open repositories live: git-dir watching, fetch, the
//! background fetch schedule and refresh on window focus, against real temp repositories.
//!
//! Tests wait for events or poll with timeouts. Fixed sleeps appear only where a test asserts
//! that something does NOT happen within a window.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use gg_core::dto::{RefLabel, RepoSnapshot, RowKind};
use gg_core::git::refs::RefKind;
use gg_core::git::runner::{CommandLog, GitRunner};
use gg_core::repo::RepoManager;
use gg_core::testutil::{RecordingSink, TestRepo};
use gg_core::watcher::watch_git_dir;
use serde_json::Value;

/// Generous bound for loads and other work the tests only wait on.
const TIMEOUT: Duration = Duration::from_secs(20);
/// How soon an on-disk change must show up.
const CHANGE_TIMEOUT: Duration = Duration::from_secs(2);
/// Window in which something must not happen.
const QUIET_WINDOW: Duration = Duration::from_millis(1500);

struct Harness {
    m: Arc<RepoManager>,
    sink: Arc<RecordingSink>,
    log: Arc<CommandLog>,
}

async fn harness() -> Harness {
    let log = CommandLog::new();
    let runner = GitRunner::detect(log.clone())
        .await
        .expect("git is installed");
    let sink = Arc::new(RecordingSink::default());
    let m = RepoManager::new(runner, sink.clone());
    Harness { m, sink, log }
}

impl Harness {
    async fn wait(&self, id: u32) {
        tokio::time::timeout(TIMEOUT, self.m.wait_loaded(id))
            .await
            .expect("load finishes");
    }

    /// Opens `path`, waits for the log to load and returns the snapshot.
    async fn open(&self, path: &Path) -> RepoSnapshot {
        let snap = self.m.open(path).await.expect("open");
        self.wait(snap.repo_id).await;
        self.m.snapshot(snap.repo_id).await.unwrap()
    }

    /// The first `repo-changed` snapshot of `id` (recorded or still to come) matching `pred`,
    /// or `None` if none arrives within `timeout`.
    async fn changed(
        &self,
        id: u32,
        timeout: Duration,
        pred: impl Fn(&RepoSnapshot) -> bool,
    ) -> Option<RepoSnapshot> {
        self.sink
            .wait_for(timeout, |name, v| {
                name == "repo-changed" && v["repoId"] == id && pred(&snapshot_from_json(v))
            })
            .await
            .map(|(_, v)| snapshot_from_json(&v))
    }

    fn events_for(&self, id: u32) -> usize {
        self.sink
            .events()
            .iter()
            .filter(|(_, v)| v["repoId"] == id)
            .count()
    }

    /// Commands recorded after command id `since` whose subcommand is `sub`.
    fn commands_since(&self, since: u64, sub: &str) -> usize {
        self.log
            .since(since)
            .iter()
            .filter(|r| subcommand(&r.args) == Some(sub))
            .count()
    }

    fn last_command_id(&self) -> u64 {
        self.log.since(0).last().map_or(0, |r| r.id)
    }
}

/// The subcommand of a logged command, after any `-c key=value` options.
fn subcommand(args: &[String]) -> Option<&str> {
    let mut i = 0;
    while args.get(i).map(String::as_str) == Some("-c") {
        i += 2;
    }
    args.get(i).map(String::as_str)
}

/// Writes an executable shell script.
#[cfg(unix)]
fn script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Makes fetches from `origin` in `repo` go through `body` as the remote's upload-pack.
#[cfg(unix)]
fn upload_pack(repo: &TestRepo, name: &str, body: &str) {
    let path = repo.dir.path().join(name);
    script(&path, body);
    repo.git(&["config", "remote.origin.uploadpack", path.to_str().unwrap()]);
}

fn snapshot_from_json(v: &Value) -> RepoSnapshot {
    serde_json::from_value(v.clone()).expect("repo-changed payload is a RepoSnapshot")
}

/// Polls `cond` every 20 ms until it holds; false if it still does not after `timeout`.
async fn eventually(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    tokio::time::timeout(timeout, async {
        while !cond() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok()
}

// ---------------------------------------------------------------------------------------
// git-dir watching

#[tokio::test]
async fn external_commit_triggers_refresh() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    assert_eq!(snap.total_rows, 1);

    repo.commit("two");
    let changed = h
        .changed(snap.repo_id, CHANGE_TIMEOUT, |s| s.total_rows == 2)
        .await;
    assert!(changed.is_some(), "no repo-changed with the new commit");
}

#[tokio::test]
async fn external_branch_triggers_refresh() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let snap = h.open(repo.path()).await;

    repo.git(&["branch", "x"]);
    let changed = h
        .changed(snap.repo_id, CHANGE_TIMEOUT, |s| {
            s.branches.iter().any(|b| b == "x")
        })
        .await;
    assert!(changed.is_some(), "branch x never showed up");
    let now = h.m.snapshot(snap.repo_id).await.unwrap();
    assert!(now.branches.iter().any(|b| b == "x"));
}

#[tokio::test]
async fn new_worktree_triggers_refresh() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    assert_eq!(snap.worktrees.len(), 1);
    assert!(
        !repo.path().join(".git/worktrees").exists(),
        "no linked worktree directory at open"
    );

    let wt = repo.add_worktree("feature");
    let changed = h
        .changed(snap.repo_id, CHANGE_TIMEOUT, |s| s.worktrees.len() == 2)
        .await;
    assert!(changed.is_some(), "the new worktree never showed up");

    // `worktrees/` appeared after open; changes inside it are watched too: detaching HEAD
    // in the linked worktree touches only `worktrees/<name>/HEAD`.
    let status = std::process::Command::new("git")
        .args(["checkout", "-q", "--detach"])
        .current_dir(&wt)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .unwrap();
    assert!(status.success());
    let changed = h
        .changed(snap.repo_id, CHANGE_TIMEOUT, |s| {
            s.worktrees.len() == 2 && s.worktrees.iter().all(|w| w.is_main || w.branch.is_none())
        })
        .await;
    assert!(
        changed.is_some(),
        "detached linked worktree never showed up"
    );
}

#[tokio::test]
async fn burst_is_debounced() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let before = h.last_command_id();

    for i in 0..20 {
        repo.commit(&format!("burst {i}"));
    }
    let settled = h
        .changed(snap.repo_id, TIMEOUT, |s| s.total_rows == 21 && !s.loading)
        .await;
    assert!(settled.is_some(), "the burst never fully showed up");
    // Let any refresh still scheduled run before counting.
    tokio::time::sleep(QUIET_WINDOW).await;
    let logs = h.commands_since(before, "log");
    assert!(logs < 8, "{logs} log commands for a burst of 20 commits");
}

#[tokio::test]
async fn refresh_does_not_retrigger_itself() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let snap = h.open(repo.path()).await;

    repo.commit("two");
    let changed = h
        .changed(snap.repo_id, CHANGE_TIMEOUT, |s| {
            s.total_rows == 2 && !s.loading
        })
        .await;
    assert!(changed.is_some());
    // The app's own reads must not look like changes: once the refresh is done, git stays
    // quiet.
    tokio::time::sleep(QUIET_WINDOW).await;
    let settled = h.last_command_id();
    tokio::time::sleep(QUIET_WINDOW).await;
    assert_eq!(
        h.log.since(settled).len(),
        0,
        "git ran again with nothing changed: {:?}",
        h.log.since(settled)
    );
}

#[tokio::test]
async fn close_stops_watching() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    h.m.close(snap.repo_id).await.unwrap();
    let events = h.events_for(snap.repo_id);
    let commands = h.last_command_id();

    repo.commit("two");
    repo.git(&["branch", "after-close"]);
    tokio::time::sleep(QUIET_WINDOW).await;
    assert_eq!(h.events_for(snap.repo_id), events, "events after close");
    assert_eq!(h.log.since(commands).len(), 0, "git ran after close");
}

// ---------------------------------------------------------------------------------------
// watch_git_dir on its own

/// Counts `on_change` calls.
fn counter() -> (Arc<AtomicUsize>, impl Fn() + Send + Sync + 'static) {
    let n = Arc::new(AtomicUsize::new(0));
    let m = n.clone();
    (n, move || {
        m.fetch_add(1, Ordering::SeqCst);
    })
}

#[tokio::test]
async fn watcher_ignores_objects_logs_and_lock_files() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path();
    for d in ["objects/ab", "logs/refs/heads", "refs/heads"] {
        std::fs::create_dir_all(git_dir.join(d)).unwrap();
    }
    let (fired, on_change) = counter();
    let _watcher = watch_git_dir(git_dir, on_change).unwrap();

    std::fs::write(git_dir.join("objects/ab/cdef"), "blob").unwrap();
    std::fs::create_dir_all(git_dir.join("objects/pack")).unwrap();
    std::fs::write(git_dir.join("logs/HEAD"), "x").unwrap();
    std::fs::write(git_dir.join("logs/refs/heads/main"), "x").unwrap();
    std::fs::write(git_dir.join("index.lock"), "x").unwrap();
    std::fs::write(git_dir.join("refs/heads/main.lock"), "x").unwrap();
    tokio::time::sleep(QUIET_WINDOW).await;
    assert_eq!(fired.load(Ordering::SeqCst), 0, "ignored paths fired");

    // A real change does fire.
    std::fs::rename(
        git_dir.join("refs/heads/main.lock"),
        git_dir.join("refs/heads/main"),
    )
    .unwrap();
    assert!(eventually(CHANGE_TIMEOUT, || fired.load(Ordering::SeqCst) > 0).await);
}

#[tokio::test]
async fn watcher_picks_up_directories_created_later() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path();
    let (fired, on_change) = counter();
    let _watcher = watch_git_dir(git_dir, on_change).unwrap();

    // Neither directory exists at start; creating one is a change in the git dir itself.
    for sub in ["refs", "worktrees"] {
        let before = fired.load(Ordering::SeqCst);
        std::fs::create_dir(git_dir.join(sub)).unwrap();
        assert!(
            eventually(CHANGE_TIMEOUT, || fired.load(Ordering::SeqCst) > before).await,
            "creating {sub}/ did not fire"
        );
        // Changes deep inside it are then watched as well.
        let before = fired.load(Ordering::SeqCst);
        let deep = git_dir.join(sub).join("a/b");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("HEAD"), "x").unwrap();
        assert!(
            eventually(CHANGE_TIMEOUT, || fired.load(Ordering::SeqCst) > before).await,
            "a change inside {sub}/ did not fire"
        );
    }
}

#[tokio::test]
async fn watcher_debounces_and_still_fires_during_continuous_activity() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path();
    let (fired, on_change) = counter();
    let _watcher = watch_git_dir(git_dir, on_change).unwrap();

    // A write every 50 ms for 2.5 s: never 300 ms of quiet, so only the 1 s rule fires.
    let mut ticker = tokio::time::interval(Duration::from_millis(50));
    for i in 0..50 {
        ticker.tick().await;
        std::fs::write(git_dir.join("HEAD"), format!("{i}")).unwrap();
    }
    let during = fired.load(Ordering::SeqCst);
    assert!(
        (2..=10).contains(&during),
        "{during} calls during 2.5 s of continuous activity"
    );
    // And once more after it stops.
    assert!(eventually(CHANGE_TIMEOUT, || fired.load(Ordering::SeqCst) > during).await);
}

#[tokio::test]
async fn dropping_the_watcher_stops_it() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path();
    let (fired, on_change) = counter();
    let watcher = watch_git_dir(git_dir, on_change).unwrap();
    drop(watcher);
    std::fs::write(git_dir.join("HEAD"), "x").unwrap();
    tokio::time::sleep(QUIET_WINDOW).await;
    assert_eq!(fired.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------------------
// fetch

/// A clone of `origin` that can push to it (a second user's checkout).
fn second_clone(origin: &TestRepo) -> TestRepo {
    let other = TestRepo::new();
    other.git(&["remote", "add", "origin", origin.path().to_str().unwrap()]);
    other.git(&["fetch", "-q", "origin"]);
    other.git(&["reset", "-q", "--hard", "origin/main"]);
    other
}

fn remote_label(name: &str) -> impl Fn(&RefLabel) -> bool + '_ {
    move |l| l.kind == RefKind::Remote && l.name == name
}

#[tokio::test]
async fn fetch_updates_remote_refs() {
    let repo = TestRepo::new();
    repo.commit("one");
    let origin = repo.clone_as_remote();
    let other = second_clone(&origin);
    let pushed = other.commit("pushed by someone else");
    other.git(&["push", "-q", "origin", "main"]);

    let h = harness().await;
    let snap = h.open(repo.path()).await;
    assert_eq!(snap.fetch.last_at_ms, None);
    let before = h.last_command_id();

    h.m.fetch(snap.repo_id).await.expect("fetch");
    h.wait(snap.repo_id).await;

    let fetches: Vec<_> = h
        .log
        .since(before)
        .into_iter()
        .filter(|r| r.args.first().map(String::as_str) == Some("fetch"))
        .collect();
    assert_eq!(fetches.len(), 1);
    assert_eq!(fetches[0].args, ["fetch", "--all", "--prune"]);
    assert_eq!(
        serde_json::to_value(fetches[0].kind).unwrap(),
        "write",
        "fetch is a write command"
    );

    let now = h.m.snapshot(snap.repo_id).await.unwrap();
    assert!(!now.fetch.running);
    assert_eq!(now.fetch.last_error, None);
    assert!(now.fetch.last_at_ms.is_some());
    let row = h.m.find_row(snap.repo_id, &pushed).await.unwrap();
    let row = row.expect("the fetched commit is shown");
    let page =
        h.m.rows(snap.repo_id, now.generation, row, 1)
            .await
            .unwrap();
    assert!(
        page.rows[0].labels.iter().any(remote_label("origin/main")),
        "origin/main moved to the fetched commit: {:?}",
        page.rows[0].labels
    );

    // `running` was announced before the fetch and cleared after it.
    let running = h.changed(snap.repo_id, TIMEOUT, |s| s.fetch.running).await;
    assert!(running.is_some(), "no repo-changed with fetch.running");
    let done = h
        .changed(snap.repo_id, TIMEOUT, |s| {
            !s.fetch.running && s.fetch.last_at_ms.is_some()
        })
        .await;
    assert!(done.is_some(), "no repo-changed after the fetch finished");
}

#[tokio::test]
async fn fetch_no_remote_is_ok() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let before = h.last_command_id();
    h.m.fetch(snap.repo_id).await.expect("fetch");
    assert_eq!(h.commands_since(before, "fetch"), 0);
    let now = h.m.snapshot(snap.repo_id).await.unwrap();
    assert_eq!(now.fetch.last_error, None);
    assert!(!now.fetch.running);
}

#[tokio::test]
async fn fetch_failure_sets_last_error() {
    let repo = TestRepo::new();
    repo.commit("one");
    let missing = repo.dir.path().join("no such remote");
    repo.git(&["remote", "add", "origin", missing.to_str().unwrap()]);
    let h = harness().await;
    let snap = h.open(repo.path()).await;

    h.m.fetch(snap.repo_id)
        .await
        .expect("a failed fetch is still Ok");
    let now = h.m.snapshot(snap.repo_id).await.unwrap();
    let error = now.fetch.last_error.expect("lastError is set");
    assert!(error.starts_with("fatal:"), "{error:?}");
    assert!(!error.contains('\n'));
    assert!(now.fetch.last_at_ms.is_some());
    assert!(!now.fetch.running);
    assert!(
        !h.sink.events().iter().any(|(n, _)| n == "repo-error"),
        "a failed fetch does not emit repo-error"
    );
}

#[tokio::test]
async fn second_fetch_while_running_returns_at_once() {
    let repo = TestRepo::new();
    repo.commit("one");
    let _origin = repo.clone_as_remote();
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let before = h.last_command_id();

    let (a, b) = tokio::join!(h.m.fetch(snap.repo_id), h.m.fetch(snap.repo_id));
    a.unwrap();
    b.unwrap();
    assert_eq!(h.commands_since(before, "fetch"), 1);
}

#[tokio::test]
async fn scheduled_fetch_runs_every_interval() {
    let repo = TestRepo::new();
    repo.commit("one");
    let _origin = repo.clone_as_remote();
    let h = harness().await;
    h.m.set_fetch_interval(1);
    let snap = h.open(repo.path()).await;
    assert_eq!(h.commands_since(0, "fetch"), 0, "no fetch right at open");

    assert!(
        eventually(Duration::from_secs(5), || h.commands_since(0, "fetch") >= 2).await,
        "two scheduled fetches within 5 s at a 1 s interval"
    );
    let now = h.m.snapshot(snap.repo_id).await.unwrap();
    assert!(now.fetch.last_at_ms.is_some());

    // Interval 0 turns it off (a fetch already running may finish).
    h.m.set_fetch_interval(0);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let off = h.last_command_id();
    tokio::time::sleep(QUIET_WINDOW).await;
    assert_eq!(h.commands_since(off, "fetch"), 0, "fetched with interval 0");
}

#[cfg(unix)]
#[tokio::test]
async fn dropped_fetch_still_finishes() {
    let repo = TestRepo::new();
    repo.commit("one");
    let _origin = repo.clone_as_remote();
    upload_pack(
        &repo,
        "slow-upload-pack",
        "sleep 1\nexec git-upload-pack \"$@\"",
    );
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let id = snap.repo_id;

    // Drop the fetch future as soon as it reports that git is running.
    tokio::select! {
        _ = h.m.fetch(id) => panic!("the fetch finished before it could be dropped"),
        running = h.changed(id, TIMEOUT, |s| s.fetch.running) => {
            running.expect("fetch.running was announced");
        }
    }
    let finished = h
        .changed(id, Duration::from_secs(10), |s| {
            !s.fetch.running && s.fetch.last_at_ms.is_some()
        })
        .await;
    assert!(
        finished.is_some(),
        "fetch.running stayed set after the future was dropped"
    );
    assert!(!h.m.snapshot(id).await.unwrap().fetch.running);

    // And the next fetch runs git again.
    repo.git(&["config", "--unset", "remote.origin.uploadpack"]);
    let before = h.last_command_id();
    h.m.fetch(id).await.unwrap();
    assert_eq!(h.commands_since(before, "fetch"), 1);
}

/// A timed-out fetch takes what git started with it (here the remote's upload-pack, as an
/// ssh transport would be) down too, not only git.
#[cfg(unix)]
#[tokio::test]
async fn a_timed_out_fetch_kills_the_helpers_git_started() {
    let repo = TestRepo::new();
    repo.commit("one");
    let _origin = repo.clone_as_remote();
    let pid_file = repo.dir.path().join("helper.pid");
    // Hangs whether or not git is still there to talk to.
    upload_pack(
        &repo,
        "stuck-upload-pack",
        &format!("echo $$ > '{}'\nexec sleep 600", pid_file.display()),
    );
    let h = harness().await;
    h.m.set_fetch_timeout(Duration::from_millis(500));
    let snap = h.open(repo.path()).await;
    tokio::time::timeout(TIMEOUT, h.m.fetch(snap.repo_id))
        .await
        .expect("the fetch was not killed")
        .unwrap();
    let pid = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .to_owned();
    let alive = || {
        std::process::Command::new("kill")
            .args(["-0", &pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    assert!(
        eventually(Duration::from_secs(5), || !alive()).await,
        "the upload-pack ({pid}) outlived the timed-out fetch"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn hung_fetch_times_out() {
    let repo = TestRepo::new();
    repo.commit("one");
    let _origin = repo.clone_as_remote();
    // Never answers: waits for input that git sends only after the server speaks first.
    upload_pack(&repo, "hung-upload-pack", "cat >/dev/null");
    let h = harness().await;
    h.m.set_fetch_timeout(Duration::from_millis(500));
    let snap = h.open(repo.path()).await;

    let started = std::time::Instant::now();
    tokio::time::timeout(TIMEOUT, h.m.fetch(snap.repo_id))
        .await
        .expect("the fetch was not killed")
        .expect("a timed-out fetch is still Ok");
    assert!(started.elapsed() < Duration::from_secs(10));
    let now = h.m.snapshot(snap.repo_id).await.unwrap();
    assert_eq!(now.fetch.last_error.as_deref(), Some("Fetch timed out"));
    assert!(!now.fetch.running);
    assert!(now.fetch.last_at_ms.is_some());
    // The killed git still shows in the command log.
    let fetches: Vec<_> = h
        .log
        .since(0)
        .into_iter()
        .filter(|r| subcommand(&r.args) == Some("fetch"))
        .collect();
    assert_eq!(fetches.len(), 1);
    assert_eq!(fetches[0].exit_code, -1);
    assert_eq!(fetches[0].stderr, "timed out");
}

#[cfg(unix)]
#[tokio::test]
async fn fetch_is_ok_when_the_refresh_after_it_fails() {
    let repo = TestRepo::new();
    repo.commit("one");
    let origin = repo.clone_as_remote();
    let other = second_clone(&origin);
    other.commit("pushed");
    other.git(&["push", "-q", "origin", "main"]);
    // A hook that breaks the repository once the fetch has updated its refs, so every
    // later git command in it fails.
    let hooks = repo.dir.path().join("breaking-hooks");
    std::fs::create_dir(&hooks).unwrap();
    let head = repo.path().join(".git/HEAD");
    script(
        &hooks.join("reference-transaction"),
        &format!(
            "[ \"$1\" = committed ] && printf 'garbage' > '{}'\nexit 0",
            head.display()
        ),
    );
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    repo.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);

    let result = h.m.fetch(snap.repo_id).await;
    assert!(
        h.m.refresh(snap.repo_id).await.is_err(),
        "the repository is broken after the fetch"
    );
    assert!(result.is_ok(), "{result:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn scheduled_fetch_never_runs_hooks() {
    let repo = TestRepo::new();
    repo.commit("one");
    let origin = repo.clone_as_remote();
    let other = second_clone(&origin);
    let first = other.commit("pushed first");
    other.git(&["push", "-q", "origin", "main"]);
    // The hook runs on every ref update of a fetch.
    let hooks = repo.dir.path().join("marker-hooks");
    std::fs::create_dir(&hooks).unwrap();
    let marker = repo.dir.path().join("hook-ran");
    script(
        &hooks.join("reference-transaction"),
        &format!("touch '{}'", marker.display()),
    );
    repo.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let id = snap.repo_id;

    h.m.set_fetch_interval(1);
    // The fetched commit shows once the refresh after the fetch is done.
    let fetched = h
        .changed(id, TIMEOUT, |s| {
            s.total_rows == 2 && !s.fetch.running && s.fetch.last_at_ms.is_some()
        })
        .await;
    h.m.set_fetch_interval(0);
    assert!(fetched.is_some(), "no scheduled fetch");
    let row = h.m.find_row(id, &first).await.unwrap();
    assert!(row.is_some(), "the scheduled fetch updated the remote refs");
    assert!(!marker.exists(), "a scheduled fetch ran a repository hook");
    let scheduled = h
        .log
        .since(0)
        .into_iter()
        .find(|r| subcommand(&r.args) == Some("fetch"))
        .unwrap();
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    assert_eq!(
        scheduled.args,
        [
            "-c".to_owned(),
            format!("core.hooksPath={null}"),
            "fetch".into(),
            "--all".into(),
            "--prune".into()
        ]
    );

    // A fetch the user asks for runs hooks, as in a terminal.
    other.commit("pushed second");
    other.git(&["push", "-q", "origin", "main"]);
    h.m.fetch(id).await.unwrap();
    assert!(marker.exists(), "the user's fetch ran the hook");
}

// ---------------------------------------------------------------------------------------
// window focus

#[tokio::test]
async fn focus_triggers_refresh() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    assert_eq!(snap.status.untracked, 0);

    // A working-tree change: nothing in the git dir changes.
    repo.write("new.txt", "untracked\n");
    h.m.set_window_focused(false);
    h.m.set_window_focused(true);
    let changed = h
        .changed(snap.repo_id, CHANGE_TIMEOUT, |s| s.status.untracked == 1)
        .await
        .expect("focus picked up the working-tree change");
    let page =
        h.m.rows(snap.repo_id, changed.generation, 0, 1)
            .await
            .unwrap();
    assert_eq!(page.rows[0].kind, RowKind::Uncommitted);
}

// ---------------------------------------------------------------------------------------
// idle

/// Run manually: `cargo test -p gg-core --test live -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "takes 5 s; run manually to check idle behaviour"]
async fn idle_repo_runs_no_git() {
    let repo = TestRepo::new();
    repo.commit("one");
    let _origin = repo.clone_as_remote();
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let start = h.last_command_id();
    let events = h.events_for(snap.repo_id);
    tokio::time::sleep(Duration::from_secs(5)).await;
    let ran = h.log.since(start);
    println!(
        "idle 5 s: {} git commands, {} events",
        ran.len(),
        h.events_for(snap.repo_id) - events
    );
    assert_eq!(ran.len(), 0, "git ran while idle: {ran:?}");
}
