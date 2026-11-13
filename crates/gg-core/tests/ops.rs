//! Integration tests for operations (`run_op`): the queue, the refresh after an operation,
//! name checks, failure explanations, the write log and per-repository preferences, against
//! real temp repositories.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gg_core::Core;
use gg_core::dto::{OpOutcome, OpRequest, OpStatus, RepoSnapshot};
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
    // Long enough for a watcher refresh that was not folded to have shown up (quiet time
    // 300 ms); nothing can be waited for when the point is that nothing comes.
    tokio::time::sleep(Duration::from_millis(1000)).await;

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
