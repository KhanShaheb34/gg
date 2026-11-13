//! Integration tests for operations (`run_op`): the queue, the refresh after an operation,
//! name checks, failure explanations, the write log and per-repository preferences, against
//! real temp repositories.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gg_core::Core;
use gg_core::dto::{
    DangerKind, MergeTarget, MergeTargetKind, OpHint, OpOutcome, OpRequest, OpStatus, RepoSnapshot,
};
use gg_core::error::ErrorCode;
use gg_core::git::runner::{CmdKind, CommandLog, CommandRecord, GitRunner};
use gg_core::ops::{Step, explain};
use gg_core::repo::RepoManager;
use gg_core::store::Store;
use gg_core::testutil::{RecordingSink, TestRepo};
use serde_json::{Value, json};

/// Generous bound for anything asynchronous.
const TIMEOUT: Duration = Duration::from_secs(20);

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
    /// Opens `path` and waits for its log and status; returns the snapshot.
    async fn open(&self, path: &Path) -> RepoSnapshot {
        let snap = self.m.open(path).await.expect("open");
        let id = snap.repo_id;
        tokio::time::timeout(TIMEOUT, self.m.wait_loaded(id))
            .await
            .expect("load finishes");
        tokio::time::timeout(TIMEOUT, self.m.wait_status(id))
            .await
            .expect("status read");
        self.m.snapshot(id).await.unwrap()
    }

    /// The `repo-changed` payloads of repository `id` recorded so far, oldest first.
    fn changes(&self, id: u32) -> Vec<Value> {
        self.sink
            .events()
            .into_iter()
            .filter(|(n, v)| n == "repo-changed" && v["repoId"] == id)
            .map(|(_, v)| v)
            .collect()
    }

    /// Runs `request` with the dangers `confirmed`; panics if it errs or takes too long.
    async fn run_confirmed(
        &self,
        id: u32,
        request: OpRequest,
        confirmed: &[DangerKind],
    ) -> OpOutcome {
        tokio::time::timeout(TIMEOUT, self.m.run_op(id, request, confirmed))
            .await
            .expect("the operation ends")
            .expect("run_op")
    }

    /// Runs `request` with nothing confirmed.
    async fn run(&self, id: u32, request: OpRequest) -> OpOutcome {
        self.run_confirmed(id, request, &[]).await
    }

    /// The write commands logged after record `mark`.
    fn writes_since(&self, mark: u64) -> Vec<CommandRecord> {
        self.log
            .since(mark)
            .into_iter()
            .filter(|r| r.kind == CmdKind::Write)
            .collect()
    }

    /// The id of the newest command logged so far (0 if none).
    fn last_log_id(&self) -> u64 {
        self.log.since(0).last().map_or(0, |r| r.id)
    }
}

fn create_branch(name: &str, at: &str) -> OpRequest {
    OpRequest::CreateBranch {
        name: name.to_owned(),
        at: at.to_owned(),
        checkout: false,
    }
}

/// The records of operation `out` within its `logFrom..=logTo`.
fn op_records(log: &CommandLog, out: &OpOutcome) -> Vec<CommandRecord> {
    log.since(out.log_from.saturating_sub(1))
        .into_iter()
        .filter(|r| r.id <= out.log_to && r.op_id == Some(out.op_id))
        .collect()
}

#[tokio::test]
async fn create_branch_runs_and_returns_the_new_snapshot() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out =
        h.m.run_op(id, create_branch("feature/x", &head), &[])
            .await
            .expect("run_op");

    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(!out.message.is_empty());
    assert_eq!(out.detail, None);
    assert!(out.dangers.is_empty());
    assert!(out.snapshot.branches.contains(&"feature/x".to_owned()));
    assert_eq!(out.snapshot.op, None);
    assert_eq!(repo.git(&["rev-parse", "refs/heads/feature/x"]), head);
    // Not checked out.
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "main");

    assert!(
        out.log_from > 0 && out.log_from <= out.log_to,
        "{}..{}",
        out.log_from,
        out.log_to
    );
    let records = op_records(&h.log, &out);
    assert_eq!(records.first().map(|r| r.id), Some(out.log_from));
    assert_eq!(records.last().map(|r| r.id), Some(out.log_to));
    for r in &records {
        assert_eq!(r.kind, CmdKind::Write, "{r:?}");
        assert_eq!(r.repo_id, Some(id));
    }
    assert!(
        records
            .iter()
            .any(|r| r.args == ["branch", "--", "feature/x", head.as_str()]),
        "{records:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operations_on_one_repository_run_one_at_a_time() {
    const DELAY_MS: i64 = 600;
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_op_delay(Duration::from_millis(DELAY_MS as u64));

    let first = tokio::spawn({
        let (m, head) = (h.m.clone(), head.clone());
        async move { m.run_op(id, create_branch("a", &head), &[]).await }
    });
    // The first is running before the second is asked for, so the order is known.
    let running = h
        .sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"].is_object()
        })
        .await
        .expect("the first operation is published");
    assert_eq!(running.1["op"]["queued"], 0);
    let second = tokio::spawn({
        let (m, head) = (h.m.clone(), head.clone());
        async move { m.run_op(id, create_branch("b", &head), &[]).await }
    });
    let queued = h
        .sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"]["queued"] == 1
        })
        .await
        .expect("the waiting operation is counted");
    assert_eq!(queued.1["op"]["id"], running.1["op"]["id"]);

    let first = first.await.unwrap().expect("first");
    let second = second.await.unwrap().expect("second");
    assert_eq!(first.status, OpStatus::Done);
    assert_eq!(second.status, OpStatus::Done);
    assert_eq!(first.snapshot.op, None);
    assert_eq!(second.snapshot.op, None);
    assert_eq!(running.1["op"]["id"], first.op_id);
    assert!(second.log_from > first.log_to);

    // The second started (was published, ran git) only after the first's git and delay.
    let started = |op_id: u64| {
        h.changes(id)
            .iter()
            .find(|v| v["op"]["id"] == op_id)
            .and_then(|v| v["op"]["startedAtMs"].as_i64())
            .expect("published")
    };
    let write = |out: &OpOutcome| {
        op_records(&h.log, out)
            .into_iter()
            .find(|r| r.kind == CmdKind::Write)
            .expect("a write")
    };
    let (w1, w2) = (write(&first), write(&second));
    let first_body_end = w1.started_at_ms + i64::from(w1.duration_ms) + DELAY_MS;
    // 1 ms of slack for rounding to milliseconds.
    assert!(
        started(second.op_id) >= first_body_end - 1,
        "second started at {}, first ended at {first_body_end}",
        started(second.op_id)
    );
    assert!(w2.started_at_ms >= first_body_end - 1);
    assert!(started(first.op_id) <= w1.started_at_ms + 1);
    assert!(repo.git_succeeds(&["rev-parse", "--verify", "refs/heads/a"]));
    assert!(repo.git_succeeds(&["rev-parse", "--verify", "refs/heads/b"]));
}

/// The operation's own change wakes the watcher while the operation still runs (its delay
/// is longer than the watcher's quiet time): that refresh is folded into the operation's
/// final one, so the new state arrives once, together with the end of the operation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operation_refreshes_once() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let h = harness().await;
    let before = h.open(repo.path()).await;
    let id = before.repo_id;
    let seen = h.changes(id).len();
    h.m.set_op_delay(Duration::from_millis(1500));

    let out =
        h.m.run_op(id, create_branch("x", &head), &[])
            .await
            .expect("run_op");
    assert_eq!(out.status, OpStatus::Done);
    // The watcher did ask for a refresh while the operation ran, and it was folded.
    assert!(h.m.folded_refreshes() >= 1);

    let changes: Vec<Value> = h.changes(id).into_iter().skip(seen).collect();
    assert!(!changes.is_empty());
    let new_state = |v: &Value| {
        v["generation"] != before.generation || v["branches"].as_array().unwrap().len() == 2
    };
    for v in changes.iter().filter(|v| v["op"].is_object()) {
        assert!(!new_state(v), "refreshed while the operation ran: {v}");
    }
    let refreshed: Vec<&Value> = changes.iter().filter(|v| new_state(v)).collect();
    assert_eq!(refreshed.len(), 1, "{changes:#?}");
    assert!(refreshed[0]["op"].is_null());
    assert_eq!(refreshed[0]["branches"], json!(["main", "x"]));
    assert_eq!(out.snapshot.generation, refreshed[0]["generation"]);
}

/// Ruling R1: Task 1 has no operation that uses the index, so the explanation is checked
/// on git's real output for a command run against a held `index.lock`.
#[tokio::test]
async fn index_lock_held_fails_with_an_explanation() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.write("new.txt", "new\n");
    let lock = repo.path().join(".git").join("index.lock");
    fs::write(&lock, "").unwrap();
    let runner = GitRunner::detect(CommandLog::new()).await.unwrap();
    let args = ["add", "--", "new.txt"];
    let out = runner
        .run_unchecked(repo.path(), &args, CmdKind::Write)
        .await
        .unwrap();
    assert_ne!(out.code, 0);

    match explain(&args, &out) {
        Step::Failed {
            message,
            detail,
            hints,
        } => {
            assert!(
                message
                    .to_lowercase()
                    .contains("another git process is running in this repository"),
                "{message}"
            );
            assert!(detail.contains("index.lock"), "{detail}");
            assert!(hints.is_empty());
        }
        other => panic!("not a failure: {other:?}"),
    }
    // Explained without being changed: the lock file is git's business.
    assert!(lock.exists());
}

#[tokio::test]
async fn bad_names_are_rejected_before_git_runs() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    repo.branch("taken");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    for name in ["-f", "a..b", "has space", "", "HEAD", "taken"] {
        let mark = h.last_log_id();
        let err =
            h.m.run_op(id, create_branch(name, &head), &[])
                .await
                .expect_err(name);
        assert_eq!(err.code, ErrorCode::BadName, "{name:?}: {err:?}");
        assert!(!err.message.is_empty());
        let writes: Vec<_> = h
            .log
            .since(mark)
            .into_iter()
            .filter(|r| r.kind == CmdKind::Write)
            .collect();
        assert!(writes.is_empty(), "{name:?} ran {writes:?}");
    }
    assert_eq!(
        h.m.snapshot(id).await.unwrap().op,
        None,
        "a rejected request leaves no operation behind"
    );

    let name = "feature/émoji-🚀";
    let out =
        h.m.run_op(id, create_branch(name, &head), &[])
            .await
            .expect("a unicode name with a slash is fine");
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(out.snapshot.branches.contains(&name.to_owned()));
    assert_eq!(
        repo.git(&["rev-parse", &format!("refs/heads/{name}")]),
        head
    );
}

/// Through the boundary: `run_op` and `command_log` with JSON, as the UI calls them.
#[tokio::test]
async fn writes_survive_many_reads_in_the_log() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let config = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    let core = Core::new(config.path().to_path_buf(), None, sink)
        .await
        .unwrap();
    let call = |cmd: &'static str, args: Value| {
        let core = core.clone();
        async move {
            core.dispatch(cmd, args)
                .await
                .unwrap_or_else(|e| panic!("{cmd}: {e:?}"))
        }
    };
    let id = call("open_repo", json!({ "path": repo.path() })).await["repoId"].clone();
    let out = call(
        "run_op",
        json!({
            "repoId": id,
            "request": {"op": "create_branch", "name": "kept", "at": head, "checkout": false},
            "confirmed": []
        }),
    )
    .await;
    assert_eq!(out["status"], "done", "{out}");
    let write_id = out["logFrom"].as_u64().unwrap();

    // 600 commands and more after the write (all reads): more than the log keeps of reads.
    let mut newest = write_id;
    while newest < write_id + 600 {
        call("commit_details", json!({ "repoId": id, "hash": head })).await;
        let log = call("command_log", json!({ "sinceId": write_id })).await;
        newest = log
            .as_array()
            .unwrap()
            .last()
            .map_or(newest, |r| r["id"].as_u64().unwrap());
    }

    let writes = call("command_log", json!({ "sinceId": 0, "writesOnly": true })).await;
    let writes = writes.as_array().unwrap();
    assert!(
        writes.iter().any(|r| r["id"] == write_id
            && r["kind"] == "write"
            && r["opId"] == out["opId"]
            && r["repoId"] == id),
        "{writes:?}"
    );
    assert!(writes.iter().all(|r| r["kind"] == "write"));
    // The whole log, oldest first, still has it, among the newest 500 reads.
    let all = call("command_log", json!({ "sinceId": 0 })).await;
    let all = all.as_array().unwrap();
    assert!(all.iter().any(|r| r["id"] == write_id));
    let ids: Vec<u64> = all.iter().map(|r| r["id"].as_u64().unwrap()).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "ids increase");
    assert_eq!(all.iter().filter(|r| r["kind"] == "read").count(), 500);
    // Filtered by repository: every record of this repository, none of another.
    let mine = call("command_log", json!({ "sinceId": 0, "repoId": id })).await;
    assert!(mine.as_array().unwrap().iter().all(|r| r["repoId"] == id));
    let other = call("command_log", json!({ "sinceId": 0, "repoId": 999 })).await;
    assert_eq!(other, json!([]));
}

#[tokio::test]
async fn repo_prefs_round_trip_and_default() {
    let repo = TestRepo::new();
    repo.commit("one");
    let other = TestRepo::new();
    other.commit("one");
    repo.write("sub/file.txt", "x");
    let config = tempfile::tempdir().unwrap();
    let core = |sink: Arc<RecordingSink>| Core::new(config.path().to_path_buf(), None, sink);

    {
        let core = core(Arc::new(RecordingSink::default())).await.unwrap();
        let id = core
            .dispatch("open_repo", json!({ "path": repo.path() }))
            .await
            .unwrap()["repoId"]
            .clone();
        let other_id = core
            .dispatch("open_repo", json!({ "path": other.path() }))
            .await
            .unwrap()["repoId"]
            .clone();
        let prefs = core
            .dispatch("get_repo_prefs", json!({ "repoId": id }))
            .await
            .unwrap();
        assert_eq!(prefs, json!({ "mergeNoFf": true }));
        let set = core
            .dispatch(
                "set_repo_prefs",
                json!({ "repoId": id, "patch": { "mergeNoFf": false } }),
            )
            .await
            .unwrap();
        assert_eq!(set, json!({ "mergeNoFf": false }));
        // An empty patch changes nothing.
        let same = core
            .dispatch("set_repo_prefs", json!({ "repoId": id, "patch": {} }))
            .await
            .unwrap();
        assert_eq!(same, json!({ "mergeNoFf": false }));
        let theirs = core
            .dispatch("get_repo_prefs", json!({ "repoId": other_id }))
            .await
            .unwrap();
        assert_eq!(theirs, json!({ "mergeNoFf": true }), "per repository");
    }

    // Reloaded: kept, and found again when the repository is opened from a subfolder.
    let core = core(Arc::new(RecordingSink::default())).await.unwrap();
    let id = core
        .dispatch("open_repo", json!({ "path": repo.path().join("sub") }))
        .await
        .unwrap()["repoId"]
        .clone();
    let prefs = core
        .dispatch("get_repo_prefs", json!({ "repoId": id }))
        .await
        .unwrap();
    assert_eq!(prefs, json!({ "mergeNoFf": false }));

    let root = dunce::canonicalize(repo.path()).unwrap();
    let store = Store::load(config.path());
    assert!(!store.repo_prefs(&root).merge_no_ff);
    assert!(store.repo_prefs(other.path()).merge_no_ff);
    let saved: Value =
        serde_json::from_str(&fs::read_to_string(config.path().join("state.json")).unwrap())
            .unwrap();
    assert_eq!(
        saved["repoPrefs"][root.to_string_lossy().as_ref()],
        json!({ "mergeNoFf": false })
    );
}

/// A body that panics still ends its operation within its turn: the repository is
/// refreshed and no longer busy, and the operation after it runs, whether or not the
/// caller is still waiting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_panicking_body_still_ends_its_operation() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    // The caller waits.
    h.m.panic_next_op();
    let err =
        h.m.run_op(id, create_branch("a", &head), &[])
            .await
            .expect_err("the body panicked");
    assert_eq!(err.code, ErrorCode::Io);
    let snap = h.m.snapshot(id).await.unwrap();
    assert_eq!(snap.op, None);
    assert!(
        snap.branches.contains(&"a".to_owned()),
        "refreshed: {snap:?}"
    );

    // The caller goes away while the body runs; another operation is asked for meanwhile.
    h.m.set_op_delay(Duration::from_millis(400));
    h.m.panic_next_op();
    let caller = tokio::spawn({
        let (m, head) = (h.m.clone(), head.clone());
        async move { m.run_op(id, create_branch("b", &head), &[]).await }
    });
    let running = h
        .sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"]["title"] == "Create branch b"
        })
        .await
        .expect("published");
    let next = tokio::spawn({
        let (m, head) = (h.m.clone(), head.clone());
        async move { m.run_op(id, create_branch("c", &head), &[]).await }
    });
    caller.abort();
    let next = tokio::time::timeout(TIMEOUT, next)
        .await
        .expect("the next operation is not stuck")
        .unwrap()
        .expect("the next operation runs");
    assert_eq!(next.status, OpStatus::Done, "{next:?}");
    assert_eq!(next.snapshot.op, None);
    assert!(next.snapshot.branches.contains(&"b".to_owned()));

    // b ended (refreshed, op cleared) before c started.
    let changes = h.changes(id);
    let b_op = running.1["op"]["id"].clone();
    let b_ended = changes
        .iter()
        .position(|v| v["op"].is_null() && v["branches"].as_array().unwrap().contains(&json!("b")))
        .expect("b's end is published");
    let c_started = changes
        .iter()
        .position(|v| v["op"]["id"] == next.op_id)
        .expect("c is published");
    assert!(b_ended < c_started, "{changes:#?}");
    assert!(
        changes[..b_ended].iter().any(|v| v["op"]["id"] == b_op),
        "{changes:#?}"
    );
}

/// Closing a repository while an operation runs or waits is refused: reopened, it would get
/// a queue of its own and two operations could run at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_is_refused_while_an_operation_runs_or_waits() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_op_delay(Duration::from_millis(600));

    let first = tokio::spawn({
        let (m, head) = (h.m.clone(), head.clone());
        async move { m.run_op(id, create_branch("a", &head), &[]).await }
    });
    h.sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"].is_object()
        })
        .await
        .expect("running");
    let err = h.m.close(id).await.expect_err("busy while running");
    assert_eq!(err.code, ErrorCode::Busy);
    assert!(!err.message.is_empty());

    let second = tokio::spawn({
        let (m, head) = (h.m.clone(), head.clone());
        async move { m.run_op(id, create_branch("b", &head), &[]).await }
    });
    h.sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"]["queued"] == 1
        })
        .await
        .expect("queued");
    first.await.unwrap().expect("first");
    // The second may be waiting or running by now; either way the repository is busy.
    assert_eq!(h.m.close(id).await.unwrap_err().code, ErrorCode::Busy);
    second.await.unwrap().expect("second");

    h.m.close(id).await.expect("closes once idle");
}

/// Ruling 7: a `refresh` asked for while an operation runs (the UI sends one when the
/// window regains focus) is folded into the operation's final refresh: it returns the
/// current snapshot at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refresh_asked_for_during_an_operation_is_folded() {
    const DELAY: Duration = Duration::from_millis(1500);
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_op_delay(DELAY);

    let op = tokio::spawn({
        let (m, head) = (h.m.clone(), head.clone());
        async move { m.run_op(id, create_branch("x", &head), &[]).await }
    });
    h.sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"].is_object()
        })
        .await
        .expect("running");
    // The body has made the branch: a refresh that ran now would show it.
    while !repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/x"]) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let folded = h.m.folded_refreshes();
    let asked = std::time::Instant::now();
    let snap = h.m.refresh(id).await.expect("refresh");
    assert!(asked.elapsed() < DELAY / 2, "waited {:?}", asked.elapsed());
    assert!(snap.op.is_some(), "the snapshot as it is, mid-operation");
    assert!(!snap.branches.contains(&"x".to_owned()));
    assert!(h.m.folded_refreshes() > folded);

    let out = op.await.unwrap().expect("run_op");
    assert!(out.snapshot.branches.contains(&"x".to_owned()));
    assert_eq!(out.snapshot.op, None);
}

// ---- Local operations: checkout and branches ----

fn checkout_branch(branch: &str) -> OpRequest {
    OpRequest::CheckoutBranch {
        branch: branch.to_owned(),
        fast_forward: false,
    }
}

fn delete_branch(branch: &str) -> OpRequest {
    OpRequest::DeleteBranch {
        branch: branch.to_owned(),
    }
}

/// The branch HEAD is on, or `None` when detached.
fn current_branch(repo: &TestRepo) -> Option<String> {
    repo.git_succeeds(&["symbolic-ref", "-q", "HEAD"])
        .then(|| repo.git(&["symbolic-ref", "--short", "HEAD"]))
}

#[tokio::test]
async fn checkout_branch_switches_to_it() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("feature/x");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, checkout_branch("feature/x")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Check out feature/x");
    assert_eq!(current_branch(&repo).as_deref(), Some("feature/x"));
    assert_eq!(out.snapshot.head.branch.as_deref(), Some("feature/x"));
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["checkout", "feature/x", "--"]),
    );
}

#[tokio::test]
async fn checkout_branch_with_fast_forward_is_not_built_yet() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("x");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let request = OpRequest::CheckoutBranch {
        branch: "x".into(),
        fast_forward: true,
    };
    let err = h.m.run_op(id, request, &[]).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
}

#[tokio::test]
async fn checkout_remote_makes_a_tracking_branch() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("topic");
    repo.checkout("topic");
    let tip = repo.commit("on topic");
    repo.checkout("main");
    let _origin = repo.clone_as_remote();
    repo.git(&["branch", "-D", "topic"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let request = OpRequest::CheckoutRemote {
        remote_branch: "origin/topic".into(),
        local_name: "mine".into(),
    };
    let out = h.run(id, request).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(current_branch(&repo).as_deref(), Some("mine"));
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), tip);
    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "mine@{upstream}"]),
        "origin/topic"
    );
    assert!(out.snapshot.branches.contains(&"mine".to_owned()));
}

#[tokio::test]
async fn checkout_commit_detaches_head() {
    let repo = TestRepo::new();
    let first = repo.commit("one");
    repo.commit("two");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run(
            id,
            OpRequest::CheckoutCommit {
                hash: first.clone(),
            },
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(current_branch(&repo), None);
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), first);
    assert_eq!(out.snapshot.head.branch, None);
    assert_eq!(out.snapshot.head.hash.as_deref(), Some(first.as_str()));
}

#[tokio::test]
async fn create_branch_with_checkout_switches_to_it() {
    let repo = TestRepo::new();
    let first = repo.commit("one");
    repo.commit("two");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let request = OpRequest::CreateBranch {
        name: "fix".into(),
        at: first.clone(),
        checkout: true,
    };
    let out = h.run(id, request).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(current_branch(&repo).as_deref(), Some("fix"));
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), first);
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["checkout", "-b", "fix", first.as_str()]),
    );
}

#[tokio::test]
async fn rename_branch_renames_it_and_keeps_head_on_it() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    repo.branch("old");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let rename = |branch: &str, new_name: &str| OpRequest::RenameBranch {
        branch: branch.into(),
        new_name: new_name.into(),
    };
    let out = h.run(id, rename("old", "new")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/old"]));
    assert_eq!(repo.git(&["rev-parse", "refs/heads/new"]), head);
    assert_eq!(out.snapshot.branches, ["main", "new"]);

    let out = h.run(id, rename("main", "trunk")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(current_branch(&repo).as_deref(), Some("trunk"));

    let err = h.m.run_op(id, rename("new", "-x"), &[]).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::BadName);
}

/// "a branch named 'x' already exists", from rename and from a remote checkout.
#[tokio::test]
async fn an_existing_branch_name_explains() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("a");
    repo.branch("b");
    let _origin = repo.clone_as_remote();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let rename = OpRequest::RenameBranch {
        branch: "a".into(),
        new_name: "b".into(),
    };
    let remote = OpRequest::CheckoutRemote {
        remote_branch: "origin/a".into(),
        local_name: "b".into(),
    };
    for request in [rename, remote] {
        let out = h.run(id, request).await;
        assert_eq!(out.status, OpStatus::Failed, "{out:?}");
        assert_eq!(
            out.message,
            "A branch named \u{201c}b\u{201d} already exists."
        );
        assert!(out.detail.unwrap().contains("already exists"));
    }
    assert!(repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/a"]));
    assert_eq!(current_branch(&repo).as_deref(), Some("main"));
}

#[tokio::test]
async fn checkout_with_changes_in_the_way_fails_and_keeps_them() {
    let repo = TestRepo::new();
    repo.write("shared.txt", "base\n");
    repo.commit_all("base");
    repo.branch("other");
    repo.checkout("other");
    repo.write("shared.txt", "other\n");
    repo.commit_all("other");
    repo.checkout("main");
    repo.write("shared.txt", "my work\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, checkout_branch("other")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("shared.txt"), "{}", out.message);
    assert!(
        out.message.to_lowercase().contains("uncommitted changes"),
        "{}",
        out.message
    );
    assert!(
        out.detail
            .unwrap()
            .contains("would be overwritten by checkout")
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("shared.txt")).unwrap(),
        "my work\n"
    );
    assert_eq!(current_branch(&repo).as_deref(), Some("main"));
}

#[tokio::test]
async fn checkout_branch_held_by_another_worktree_explains() {
    let repo = TestRepo::new();
    repo.commit("one");
    let wt = repo.add_worktree("held");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, checkout_branch("held")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    let wt = dunce::canonicalize(&wt).unwrap();
    assert!(
        out.message.contains("\u{201c}held\u{201d}")
            && out.message.contains(wt.to_string_lossy().as_ref())
            && out.message.contains("another worktree"),
        "{}",
        out.message
    );
    assert_eq!(current_branch(&repo).as_deref(), Some("main"));
}

#[tokio::test]
async fn delete_merged_branch_runs_without_confirmation() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("done");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, delete_branch("done")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(out.dangers.is_empty());
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/done"]));
    assert_eq!(out.snapshot.branches, ["main"]);
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["branch", "-d", "--", "done"])
    );
}

#[tokio::test]
async fn delete_unmerged_branch_needs_confirmation() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("wip");
    repo.checkout("wip");
    repo.commit("two");
    repo.commit("three");
    repo.checkout("main");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let mark = h.last_log_id();
    let out = h.run(id, delete_branch("wip")).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert_eq!(out.dangers.len(), 1);
    assert_eq!(out.dangers[0].kind, DangerKind::DeleteUnmergedBranch);
    assert!(
        out.dangers[0].message.contains("2 commits"),
        "{:?}",
        out.dangers
    );
    assert!(out.message.contains("2 commits"));
    assert_eq!((out.log_from, out.log_to), (0, 0));
    assert!(
        h.writes_since(mark).is_empty(),
        "{:?}",
        h.writes_since(mark)
    );
    assert!(repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/wip"]));

    let out = h
        .run_confirmed(
            id,
            delete_branch("wip"),
            &[DangerKind::DeleteUnmergedBranch],
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/wip"]));
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["branch", "-D", "--", "wip"])
    );
}

/// Merged into HEAD (so no danger), but not into its own upstream: git refuses with "not
/// fully merged".
#[tokio::test]
async fn branch_not_merged_into_its_upstream_explains() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("topic");
    let _origin = repo.clone_as_remote();
    repo.git(&["branch", "-u", "origin/topic", "topic"]);
    repo.checkout("topic");
    repo.commit("local only");
    repo.checkout("main");
    repo.git(&["merge", "-q", "--ff-only", "topic"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, delete_branch("topic")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.contains("\u{201c}topic\u{201d}") && out.message.contains("not fully merged"),
        "{}",
        out.message
    );
    assert!(out.detail.unwrap().contains("not fully merged"));
    assert!(repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/topic"]));
}

// ---- Local operations: merge and rebase ----

fn merge(
    kind: MergeTargetKind,
    name: &str,
    no_ff: bool,
    squash: bool,
    no_commit: bool,
) -> OpRequest {
    OpRequest::Merge {
        target: MergeTarget {
            kind,
            name: name.to_owned(),
        },
        no_ff,
        squash,
        no_commit,
    }
}

fn merge_branch(name: &str) -> OpRequest {
    merge(MergeTargetKind::Branch, name, true, false, false)
}

fn rebase(onto: &str) -> OpRequest {
    OpRequest::Rebase {
        onto: onto.to_owned(),
    }
}

/// `main` and `feature`, each with one commit of its own after `base`, touching different
/// files. HEAD on `main`. Returns the tip of `feature`.
fn diverged(repo: &TestRepo) -> String {
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    let tip = repo.commit("on feature");
    repo.checkout("main");
    repo.commit("on main");
    tip
}

/// `main` and `feature` both change `clash.txt` after `base`. HEAD on `main`.
fn conflicting(repo: &TestRepo) {
    repo.write("clash.txt", "base\n");
    repo.commit_all("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("clash.txt", "feature\n");
    repo.commit_all("feature side");
    repo.checkout("main");
    repo.write("clash.txt", "main\n");
    repo.commit_all("main side");
}

/// Whether `name` (`MERGE_HEAD`, `rebase-merge`, …) exists in the repository's git dir.
fn in_git_dir(repo: &TestRepo, name: &str) -> bool {
    repo.path().join(".git").join(name).exists()
}

fn parents(repo: &TestRepo, rev: &str) -> usize {
    repo.git(&["rev-list", "--parents", "-n", "1", rev])
        .split(' ')
        .count()
        - 1
}

#[tokio::test]
async fn merge_no_ff_creates_a_merge_commit_and_ff_does_not() {
    let repo = TestRepo::new();
    repo.commit("base");
    repo.branch("a");
    repo.branch("b");
    repo.checkout("a");
    let a_tip = repo.commit("on a");
    repo.checkout("b");
    let b_tip = repo.commit("on b");
    repo.checkout("main");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run(id, merge(MergeTargetKind::Branch, "a", true, false, false))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Merge a into main");
    assert_eq!(parents(&repo, "HEAD"), 2);
    assert_eq!(repo.git(&["rev-parse", "HEAD^2"]), a_tip);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "Merge branch 'a'");
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["merge", "--no-edit", "--no-ff", "a"])
    );
    // No remote: nothing to push to.
    assert!(out.hints.is_empty(), "{:?}", out.hints);

    repo.git(&["reset", "-q", "--hard", "HEAD~1"]);
    let out = h
        .run(id, merge(MergeTargetKind::Branch, "b", false, false, false))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), b_tip, "fast-forwarded");
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["merge", "--no-edit", "--ff", "b"])
    );
}

#[tokio::test]
async fn merge_of_a_remote_branch_and_of_a_commit() {
    let repo = TestRepo::new();
    let tip = diverged(&repo);
    let _origin = repo.clone_as_remote();
    let main_tip = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run(
            id,
            merge(
                MergeTargetKind::Remote,
                "origin/feature",
                true,
                false,
                false,
            ),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD^2"]), tip);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]),
        "Merge remote-tracking branch 'origin/feature'"
    );

    repo.git(&["reset", "-q", "--hard", &main_tip]);
    let out = h
        .run(id, merge(MergeTargetKind::Commit, &tip, true, false, false))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD^2"]), tip);
}

/// A tag named like the branch does not take its place.
#[tokio::test]
async fn merge_of_a_branch_named_like_a_tag_merges_the_branch() {
    let repo = TestRepo::new();
    repo.commit("base");
    let old = repo.git(&["rev-parse", "HEAD"]);
    let tip = diverged(&repo);
    repo.git(&["tag", "feature", &old]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, merge_branch("feature")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD^2"]), tip);
}

#[tokio::test]
async fn merge_squash_leaves_changes_staged() {
    let repo = TestRepo::new();
    diverged(&repo);
    let before = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    // noFf is ignored with squash (git refuses the two together).
    let out = h
        .run(
            id,
            merge(MergeTargetKind::Branch, "feature", true, true, false),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(
        repo.git(&["rev-parse", "HEAD"]),
        before,
        "nothing committed"
    );
    assert!(!repo.git(&["diff", "--cached", "--name-only"]).is_empty());
    assert!(!in_git_dir(&repo, "MERGE_HEAD"));
    assert!(out.hints.is_empty());
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["merge", "--no-edit", "--ff", "--squash", "feature"])
    );
}

#[tokio::test]
async fn merge_no_commit_leaves_merge_in_progress_without_conflicts() {
    let repo = TestRepo::new();
    diverged(&repo);
    let before = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run(
            id,
            merge(MergeTargetKind::Branch, "feature", true, false, true),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    assert!(in_git_dir(&repo, "MERGE_HEAD"));
    assert!(out.message.contains("commit"), "{}", out.message);
}

#[tokio::test]
async fn merge_with_conflicts_stops() {
    let repo = TestRepo::new();
    conflicting(&repo);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let started = std::time::Instant::now();
    let out = h.run(id, merge_branch("feature")).await;
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    assert!(out.message.contains("conflict"), "{}", out.message);
    assert!(in_git_dir(&repo, "MERGE_HEAD"));
    assert_eq!(out.detail, None);
}

#[tokio::test]
async fn merge_already_up_to_date_is_done() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("same");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, merge_branch("same")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(
        out.message.starts_with("Already up to date"),
        "{}",
        out.message
    );
    assert!(out.hints.is_empty());
}

/// After a merge, push is offered: to the upstream, or, without one, to the only remote
/// (setting it as upstream).
#[tokio::test]
async fn merge_hints_push_when_there_is_somewhere_to_push() {
    let repo = TestRepo::new();
    diverged(&repo);
    let main_tip = repo.git(&["rev-parse", "HEAD"]);
    let _origin = repo.clone_as_remote();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, merge_branch("feature")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(
        out.hints,
        [OpHint::Push {
            branch: "main".into(),
            remote: "origin".into(),
            set_upstream: true,
        }]
    );

    repo.git(&["reset", "-q", "--hard", &main_tip]);
    repo.git(&["branch", "-q", "-u", "origin/main", "main"]);
    let out = h.run(id, merge_branch("feature")).await;
    assert_eq!(
        out.hints,
        [OpHint::Push {
            branch: "main".into(),
            remote: "origin".into(),
            set_upstream: false,
        }]
    );
}

#[tokio::test]
async fn rebase_replays_the_branch_onto_another() {
    let repo = TestRepo::new();
    let feature_tip = diverged(&repo);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, rebase("feature")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Rebase main onto feature");
    assert_eq!(repo.git(&["rev-parse", "HEAD~1"]), feature_tip);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "on main");
    assert_eq!(current_branch(&repo).as_deref(), Some("main"));

    let out = h.run(id, rebase("feature")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(
        out.message.starts_with("Already up to date"),
        "{}",
        out.message
    );
}

#[tokio::test]
async fn rebase_onto_a_remote_branch() {
    let repo = TestRepo::new();
    let feature_tip = diverged(&repo);
    let _origin = repo.clone_as_remote();
    repo.git(&["branch", "-D", "feature"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, rebase("origin/feature")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD~1"]), feature_tip);
}

#[tokio::test]
async fn rebase_with_conflicts_stops() {
    let repo = TestRepo::new();
    conflicting(&repo);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let started = std::time::Instant::now();
    let out = h.run(id, rebase("feature")).await;
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    assert!(in_git_dir(&repo, "rebase-merge") || in_git_dir(&repo, "rebase-apply"));
}

#[tokio::test]
async fn operations_on_detached_head() {
    let repo = TestRepo::new();
    let feature_tip = diverged(&repo);
    repo.git(&["checkout", "-q", "--detach"]);
    let detached = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, merge_branch("feature")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Merge feature into detached HEAD");
    assert_eq!(current_branch(&repo), None);
    assert_eq!(repo.git(&["rev-parse", "HEAD^2"]), feature_tip);
    assert!(out.hints.is_empty());

    repo.git(&["checkout", "-q", "--detach", &detached]);
    let mark = h.last_log_id();
    let out = h.run(id, rebase("feature")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("HEAD is detached"), "{}", out.message);
    assert!(h.writes_since(mark).is_empty());
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), detached);
}

/// The detached-HEAD explanation, on git's own words for it.
#[tokio::test]
async fn no_current_branch_explains() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.git(&["checkout", "-q", "--detach"]);
    let runner = GitRunner::detect(CommandLog::new()).await.unwrap();
    for args in [&["rebase"][..], &["merge"][..]] {
        let out = runner
            .run_unchecked(repo.path(), args, CmdKind::Write)
            .await
            .unwrap();
        assert_ne!(out.code, 0);
        match explain(args, &out) {
            Step::Failed { message, .. } => {
                assert!(message.contains("HEAD is detached"), "{args:?}: {message}");
            }
            other => panic!("not a failure: {other:?}"),
        }
    }
}
