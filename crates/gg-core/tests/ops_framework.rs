//! Integration tests for the operation framework (`run_op`): the queue, the refresh after an
//! operation, name checks, the write log, per-repository preferences, confirmation and the
//! outcome's snapshot, against real temp repositories.

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use gg_core::Core;
use gg_core::dto::{MergeTargetKind, OpOutcome, OpRequest, OpStatus, ResetMode};
use gg_core::error::ErrorCode;
use gg_core::git::runner::{CmdKind, CommandLog, GitRunner};
use gg_core::ops::{Step, explain};
use gg_core::store::Store;
use gg_core::testutil::{RecordingSink, TestRepo};
use serde_json::{Value, json};

mod common;
use common::*;

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

/// Every field that names a commit takes a full hash (40 or 64 lowercase hex digits) and
/// nothing else: a range, a ref name, a short or uppercase hash is refused before any git
/// runs, so it can never reach cherry-pick, reset or the others.
#[tokio::test]
async fn commit_fields_take_only_full_hashes() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    repo.git(&["tag", "t"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let sha256 = "a".repeat(64);
    let bad = [
        "main..t".to_owned(),
        "HEAD".to_owned(),
        "t".to_owned(),
        head[..7].to_owned(),
        head.to_uppercase(),
        format!("{head}^"),
        format!("{head}0"),
        "g".repeat(40),
        String::new(),
    ];
    for hash in &bad {
        let requests = [
            OpRequest::CheckoutCommit { hash: hash.clone() },
            create_branch("x", hash),
            merge(MergeTargetKind::Commit, hash, true, false, false),
            OpRequest::CherryPick {
                hash: hash.clone(),
                mainline: None,
                no_commit: false,
            },
            OpRequest::Revert {
                hash: hash.clone(),
                mainline: None,
            },
            reset(hash, ResetMode::Soft),
            OpRequest::CreateTag {
                name: "v1".into(),
                hash: hash.clone(),
                message: None,
            },
        ];
        for request in requests {
            let mark = h.last_log_id();
            let shown = format!("{request:?}");
            let err = h.m.run_op(id, request, &[]).await.expect_err(&shown);
            assert_eq!(err.code, ErrorCode::BadRequest, "{shown}: {err:?}");
            assert!(h.log.since(mark).is_empty(), "{shown} ran git");
        }
    }
    // A full SHA-256 hash passes the check (git then says it does not exist).
    let out = h.run(id, OpRequest::CheckoutCommit { hash: sha256 }).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    // And a full SHA-1 hash runs.
    let out = h.run(id, create_branch("x", &head)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
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
    tokio::time::timeout(TIMEOUT, async {
        while !repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/x"]) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the body makes the branch");
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

/// A request that needs confirmation runs nothing, and shows nothing either: no busy
/// state is published and no refresh runs.
#[tokio::test]
async fn needs_confirmation_publishes_and_refreshes_nothing() {
    let repo = TestRepo::new();
    let first = repo.commit("one");
    let second = repo.commit("two");
    let h = harness().await;
    let before = h.open(repo.path()).await;
    let id = before.repo_id;
    let seen = h.changes(id).len();
    let mark = h.last_log_id();

    let out = h.run(id, reset(&first, ResetMode::Hard)).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert_eq!(out.snapshot.op, None);
    assert_eq!(out.snapshot.generation, before.generation);
    assert_eq!(out.snapshot.head.hash.as_deref(), Some(second.as_str()));
    assert_eq!(h.changes(id).len(), seen, "{:#?}", &h.changes(id)[seen..]);
    // Only the danger check's reads ran: no refresh, no status read.
    let ran = h.log.since(mark);
    assert!(!ran.is_empty());
    assert!(
        ran.iter()
            .all(|r| r.kind == CmdKind::Read && r.op_id == Some(out.op_id)),
        "{ran:#?}"
    );
}

// ---- The outcome's snapshot and the working tree's status (ruling R4) ----

/// `main` ignores `build.log`, which is in the working tree; `plain` has no `.gitignore`, so
/// checking it out turns `build.log` into an untracked file.
fn ignored_file_becomes_untracked(repo: &TestRepo) {
    repo.commit("one");
    repo.branch("plain");
    repo.write(".gitignore", "build.log\n");
    repo.commit_all("ignore the build log");
    repo.write("build.log", "log\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outcome_snapshot_has_the_status_after_checkout() {
    let repo = TestRepo::new();
    ignored_file_becomes_untracked(&repo);
    let h = harness().await;
    let before = h.open(repo.path()).await;
    let id = before.repo_id;
    assert_eq!(before.status.untracked, 0);
    let seen = h.changes(id).len();
    // A status that takes a moment is waited for.
    h.m.set_status_delay(Duration::from_millis(300));

    let out = h.run(id, checkout_branch("plain")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(
        out.snapshot.status.untracked, 1,
        "{:?}",
        out.snapshot.status
    );
    // The end of the operation is published once, with the new status.
    let ended: Vec<Value> = h.changes(id)[seen..]
        .iter()
        .filter(|v| v["op"].is_null())
        .cloned()
        .collect();
    assert_eq!(
        ended.first().map(|v| v["status"]["untracked"].clone()),
        Some(json!(1)),
        "{ended:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outcome_does_not_wait_long_for_a_slow_status() {
    let repo = TestRepo::new();
    ignored_file_becomes_untracked(&repo);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_status_delay(Duration::from_millis(3500));

    let asked = std::time::Instant::now();
    let out = h.run(id, checkout_branch("plain")).await;
    let took = asked.elapsed();
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(took >= Duration::from_secs(2), "waited {took:?}");
    assert!(took < Duration::from_millis(3000), "waited {took:?}");
    assert_eq!(out.snapshot.status.untracked, 0, "not read yet");
    assert_eq!(out.snapshot.op, None);
    // It arrives later, by event.
    h.sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["status"]["untracked"] == 1
        })
        .await
        .expect("the status arrives");
}
