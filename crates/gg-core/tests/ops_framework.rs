//! Integration tests for the operation framework (`run_op`): the queue, the refresh after an
//! operation, name checks, the write log, per-repository preferences, confirmation and the
//! outcome's snapshot, against real temp repositories.

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use gg_core::Core;
use gg_core::dto::{MergeTarget, MergeTargetKind, OpOutcome, OpRequest, OpStatus, ResetMode};
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
        h.m.run_op(
            id,
            in_wt(repo.path(), create_branch("feature/x", &head)),
            &[],
        )
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
        let (m, wt, head) = (h.m.clone(), repo.path().to_path_buf(), head.clone());
        async move {
            m.run_op(id, in_wt(&wt, create_branch("a", &head)), &[])
                .await
        }
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
        let (m, wt, head) = (h.m.clone(), repo.path().to_path_buf(), head.clone());
        async move {
            m.run_op(id, in_wt(&wt, create_branch("b", &head)), &[])
                .await
        }
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
        h.m.run_op(id, in_wt(repo.path(), create_branch("x", &head)), &[])
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

/// The explanation of a held `index.lock` is checked on git's real output for a command run
/// against one.
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
                    .contains("another git process is working in this repository"),
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
            h.m.run_op(id, in_wt(repo.path(), create_branch(name, &head)), &[])
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
        h.m.run_op(id, in_wt(repo.path(), create_branch(name, &head)), &[])
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
        head[..8].to_owned(),
        head.to_uppercase(),
        format!("{head}^"),
        format!("{head}0"),
        "g".repeat(40),
        String::new(),
    ];
    for hash in &bad {
        let requests = [
            OpRequest::CheckoutCommit {
                hash: hash.clone(),
                expect_worktree: here_wt(),
            },
            create_branch("x", hash),
            merge(MergeTargetKind::Commit, hash, true, false, false),
            OpRequest::CherryPick {
                hash: hash.clone(),
                mainline: None,
                no_commit: false,
                expect_head: here(),
                expect_worktree: here_wt(),
            },
            OpRequest::Revert {
                hash: hash.clone(),
                mainline: None,
                expect_head: here(),
                expect_worktree: here_wt(),
            },
            reset(hash, ResetMode::Soft),
            OpRequest::CreateTag {
                name: "v1".into(),
                hash: hash.clone(),
                message: None,
                expect_worktree: here_wt(),
            },
        ];
        for request in requests {
            let mark = h.last_log_id();
            let shown = format!("{request:?}");
            let err =
                h.m.run_op(id, in_wt(repo.path(), request), &[])
                    .await
                    .expect_err(&shown);
            assert_eq!(err.code, ErrorCode::BadRequest, "{shown}: {err:?}");
            assert!(h.log.since(mark).is_empty(), "{shown} ran git");
        }
    }
    // A full SHA-256 hash passes the check (git then says it does not exist).
    let out = h
        .run(
            id,
            OpRequest::CheckoutCommit {
                hash: sha256,
                expect_worktree: here_wt(),
            },
        )
        .await;
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
            "request": {"op": "create_branch", "name": "kept", "at": head, "checkout": false,
                        "expectWorktree": repo.path()},
            "confirmed": [],
            "clientId": "ui",
            "clientSeq": 1
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
            && r["opTitle"] == "Create branch kept"
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
        assert_eq!(
            prefs,
            json!({ "mergeNoFf": true, "activeWorktree": null, "sidePanel": true, "prBase": null })
        );
        let set = core
            .dispatch(
                "set_repo_prefs",
                json!({ "repoId": id, "patch": { "mergeNoFf": false, "sidePanel": false } }),
            )
            .await
            .unwrap();
        assert_eq!(
            set,
            json!({ "mergeNoFf": false, "activeWorktree": null, "sidePanel": false, "prBase": null })
        );
        // An empty patch changes nothing.
        let same = core
            .dispatch("set_repo_prefs", json!({ "repoId": id, "patch": {} }))
            .await
            .unwrap();
        assert_eq!(
            same,
            json!({ "mergeNoFf": false, "activeWorktree": null, "sidePanel": false, "prBase": null })
        );
        let theirs = core
            .dispatch("get_repo_prefs", json!({ "repoId": other_id }))
            .await
            .unwrap();
        assert_eq!(
            theirs,
            json!({ "mergeNoFf": true, "activeWorktree": null, "sidePanel": true, "prBase": null }),
            "per repository"
        );
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
    assert_eq!(
        prefs,
        json!({ "mergeNoFf": false, "activeWorktree": null, "sidePanel": false, "prBase": null })
    );

    let root = dunce::canonicalize(repo.path()).unwrap();
    let store = Store::load(config.path());
    assert!(!store.repo_prefs(&root).merge_no_ff);
    assert!(store.repo_prefs(other.path()).merge_no_ff);
    let saved: Value =
        serde_json::from_str(&fs::read_to_string(config.path().join("state.json")).unwrap())
            .unwrap();
    assert_eq!(
        saved["repoPrefs"][root.to_string_lossy().as_ref()],
        json!({ "mergeNoFf": false, "activeWorktree": null, "sidePanel": false, "prBase": null })
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
        h.m.run_op(id, in_wt(repo.path(), create_branch("a", &head)), &[])
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
        let (m, wt, head) = (h.m.clone(), repo.path().to_path_buf(), head.clone());
        async move {
            m.run_op(id, in_wt(&wt, create_branch("b", &head)), &[])
                .await
        }
    });
    let running = h
        .sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"]["title"] == "Create branch b"
        })
        .await
        .expect("published");
    let next = tokio::spawn({
        let (m, wt, head) = (h.m.clone(), repo.path().to_path_buf(), head.clone());
        async move {
            m.run_op(id, in_wt(&wt, create_branch("c", &head)), &[])
                .await
        }
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
        let (m, wt, head) = (h.m.clone(), repo.path().to_path_buf(), head.clone());
        async move {
            m.run_op(id, in_wt(&wt, create_branch("a", &head)), &[])
                .await
        }
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
        let (m, wt, head) = (h.m.clone(), repo.path().to_path_buf(), head.clone());
        async move {
            m.run_op(id, in_wt(&wt, create_branch("b", &head)), &[])
                .await
        }
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

/// A title that names the current branch is made in the operation's turn: a merge queued
/// behind a checkout names the branch the checkout switched to. It was asked for on main,
/// though, so it does not run there: nothing is merged, and the notice says why.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queued_operation_is_titled_with_the_branch_it_runs_on() {
    let repo = TestRepo::new();
    repo.commit("base");
    repo.branch("feature");
    repo.branch("other");
    repo.checkout("other");
    repo.commit("on other");
    repo.checkout("main");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_op_delay(Duration::from_millis(300));
    // HEAD as the UI shows it when the merge is asked for: main, before the checkout runs.
    let on_main = head_at(repo.path());

    let first = tokio::spawn({
        let (m, wt) = (h.m.clone(), repo.path().to_path_buf());
        async move {
            m.run_op(id, in_wt(&wt, checkout_branch("feature")), &[])
                .await
        }
    });
    h.sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"].is_object()
        })
        .await
        .expect("the checkout runs");
    // Asked for while main is checked out, as the UI shows it.
    let asked_on_main = OpRequest::Merge {
        target: MergeTarget {
            kind: MergeTargetKind::Branch,
            name: "other".into(),
        },
        no_ff: true,
        squash: false,
        no_commit: false,
        expect_head: on_main,
        expect_worktree: here_wt(),
    };
    let feature = repo.git(&["rev-parse", "feature"]);
    let merge = h.run(id, asked_on_main).await;
    first.await.unwrap().expect("checkout");
    assert_eq!(merge.title, "Merge other into feature", "{merge:?}");
    assert_eq!(merge.status, OpStatus::Failed, "{merge:?}");
    assert_eq!(
        merge.message,
        "Nothing was done: it was asked for on main, but feature is checked out now. Look at \
         it again, and ask again if it still applies."
    );
    assert_eq!((merge.log_from, merge.log_to), (0, 0));
    assert_eq!(
        repo.git(&["rev-parse", "feature"]),
        feature,
        "nothing merged"
    );
}

/// A `refresh` asked for while an operation runs (the UI sends one when the
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
        let (m, wt, head) = (h.m.clone(), repo.path().to_path_buf(), head.clone());
        async move {
            m.run_op(id, in_wt(&wt, create_branch("x", &head)), &[])
                .await
        }
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

// ---- The outcome's snapshot and the working tree's status ----

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

/// Every snapshot carries `seq`: it grows by one per repository with each snapshot that
/// changes anything (emitted or returned as an operation's outcome), so the UI can drop
/// one older than what it has. A plain read repeats the latest.
#[tokio::test]
async fn snapshot_seq_increases_with_every_change() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let id = snap.repo_id;
    let seqs = |h: &Harness| -> Vec<u64> {
        h.changes(id)
            .iter()
            .map(|v| v["seq"].as_u64().expect("seq"))
            .collect()
    };
    assert_eq!(
        snap.seq,
        *seqs(&h).last().unwrap(),
        "a read repeats the latest"
    );
    let mut last = snap.seq;

    // A refresh after a commit made outside.
    repo.commit("two");
    let refreshed = h.m.refresh(id).await.unwrap();
    assert!(refreshed.seq > last, "{} > {last}", refreshed.seq);
    last = refreshed.seq;
    // The status arriving on its own.
    repo.write("dirty.txt", "x");
    h.m.refresh(id).await.unwrap();
    tokio::time::timeout(TIMEOUT, h.m.wait_status(id))
        .await
        .unwrap();
    let dirty = h.m.snapshot(id).await.unwrap();
    assert_eq!(dirty.status.untracked, 1);
    assert!(dirty.seq > last);
    last = dirty.seq;
    // An operation's end, and an outcome where nothing changed.
    let out = h.run(id, create_branch("b", &head)).await;
    assert!(out.snapshot.seq > last);
    last = out.snapshot.seq;
    let out = h.run(id, reset(&head, ResetMode::Hard)).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation);
    assert!(out.snapshot.seq > last, "every outcome gets its own seq");
    last = out.snapshot.seq;
    // A view-option change.
    let mut view = h.m.snapshot(id).await.unwrap().view_options;
    view.show_tags = false;
    let changed = h.m.set_view_options(id, view).await.unwrap();
    assert!(changed.seq > last);
    tokio::time::timeout(TIMEOUT, h.m.wait_loaded(id))
        .await
        .unwrap();

    let all = seqs(&h);
    assert!(
        all.windows(2).all(|w| w[0] < w[1]),
        "emitted in increasing order: {all:?}"
    );
    assert_eq!(h.m.snapshot(id).await.unwrap().seq, *all.last().unwrap());
}

/// Requests the UI numbers reach the queue in their numbers' order, whatever order they
/// arrive in: one whose predecessor has not arrived waits for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn numbered_requests_arriving_out_of_order_run_in_order() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    // Number 2 arrives first, then number 1.
    let second = tokio::spawn({
        let (m, wt, head) = (h.m.clone(), repo.path().to_path_buf(), head.clone());
        async move {
            m.run_op_in_order(id, "ui", 2, in_wt(&wt, create_branch("b", &head)), &[])
                .await
        }
    });
    // Number 2 is waiting before number 1 is sent (no sleep: the core says so).
    tokio::time::timeout(TIMEOUT, async {
        while h.m.numbered_waiting(id, "ui") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("number 2 waits");
    let first = tokio::spawn({
        let (m, wt, head) = (h.m.clone(), repo.path().to_path_buf(), head.clone());
        async move {
            m.run_op_in_order(id, "ui", 1, in_wt(&wt, create_branch("a", &head)), &[])
                .await
        }
    });
    // (That number 1 waits for nothing is order.rs's paused-clock test; timed here, it
    // failed under the full suite's load.)
    let first = first.await.unwrap().expect("first");
    let second = second.await.unwrap().expect("second");
    assert_eq!(first.status, OpStatus::Done, "{first:?}");
    assert_eq!(second.status, OpStatus::Done, "{second:?}");
    assert!(
        first.op_id < second.op_id,
        "{} {}",
        first.op_id,
        second.op_id
    );
    assert!(second.log_from > first.log_to);
    // Numbers go on from there.
    let third =
        h.m.run_op_in_order(
            id,
            "ui",
            3,
            in_wt(repo.path(), create_branch("c", &head)),
            &[],
        )
        .await
        .expect("third");
    assert!(third.op_id > second.op_id);
}

/// A numbered request that waited less than this for its turn waited for no earlier number
/// (a missing one is waited for 500 ms). The turn wait is recorded by the order itself, so
/// git's own time, long and uneven on a slow machine, does not count.
const AT_ONCE: Duration = Duration::from_millis(100);

/// A request whose predecessor never arrives waits for it 500 ms, then runs; the gap
/// is not waited for again, and the late predecessor runs at once when it comes. Numbers
/// start again when the repository is opened again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gap_in_the_numbers_is_waited_for_then_skipped() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    // How long each request waited for its turn (git's own time aside, which on a slow
    // machine is longer than the gap wait itself).
    let turn_wait = |id: u32, n: u64, name: &'static str| {
        let (m, wt, head) = (h.m.clone(), repo.path().to_path_buf(), head.clone());
        async move {
            let out = m
                .run_op_in_order(id, "ui", n, in_wt(&wt, create_branch(name, &head)), &[])
                .await
                .expect("runs");
            assert_eq!(out.status, OpStatus::Done, "{out:?}");
            m.last_turn_wait(id, "ui").expect("open")
        }
    };
    let waited = turn_wait(id, 2, "b").await;
    assert!(
        waited >= Duration::from_millis(480),
        "waited {waited:?} for number 1"
    );
    assert!(waited < Duration::from_millis(3000), "waited {waited:?}");
    let third = turn_wait(id, 3, "c").await;
    assert!(
        third < AT_ONCE,
        "the gap is not waited for again: {third:?}"
    );
    let late = turn_wait(id, 1, "a").await;
    assert!(late < AT_ONCE, "late: {late:?}");

    h.m.close(id).await.expect("closes");
    let id2 = h.open(repo.path()).await.repo_id;
    let again = turn_wait(id2, 1, "d").await;
    assert!(again < AT_ONCE, "numbers start again: {again:?}");
}

/// A request refused before it is queued (a bad name) still lets the next number in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_numbered_request_lets_the_next_one_in() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let err =
        h.m.run_op_in_order(
            id,
            "ui",
            1,
            in_wt(repo.path(), create_branch("-bad", &head)),
            &[],
        )
        .await
        .expect_err("bad name");
    assert_eq!(err.code, ErrorCode::BadName);
    h.m.run_op_in_order(
        id,
        "ui",
        2,
        in_wt(repo.path(), create_branch("good", &head)),
        &[],
    )
    .await
    .expect("runs");
    let waited = h.m.last_turn_wait(id, "ui").expect("open");
    assert!(waited < AT_ONCE, "waited {waited:?}");
}

/// Review item 3a: each page load (a reload, another window) numbers from 1 under a client id
/// of its own; one does not wait for another's numbers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_client_numbers_its_own_requests() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    for n in 1..=3 {
        h.m.run_op_in_order(
            id,
            "before reload",
            n,
            in_wt(repo.path(), create_branch(&format!("a{n}"), &head)),
            &[],
        )
        .await
        .expect("runs");
    }
    h.m.run_op_in_order(
        id,
        "after reload",
        1,
        in_wt(repo.path(), create_branch("b", &head)),
        &[],
    )
    .await
    .expect("runs");
    let waited = h.m.last_turn_wait(id, "after reload").expect("open");
    assert!(waited < AT_ONCE, "waited {waited:?}");
}

/// Review item 3b: a `run_op` whose arguments cannot be read still gives up its number, so
/// the next one does not wait for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_op_that_cannot_be_read_lets_the_next_one_in() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let config = tempfile::tempdir().unwrap();
    let core = Core::new(
        config.path().to_path_buf(),
        None,
        Arc::new(RecordingSink::default()),
    )
    .await
    .unwrap();
    let opened = core
        .dispatch("open_repo", json!({ "path": repo.path() }))
        .await
        .unwrap();
    let id = opened["repoId"].clone();
    let err = core
        .dispatch(
            "run_op",
            json!({"repoId": id, "clientId": "c", "clientSeq": 1, "confirmed": [],
                   "request": {"op": "no_such_op"}}),
        )
        .await
        .expect_err("refused");
    assert_eq!(err.code, ErrorCode::BadRequest);
    let out = core
        .dispatch(
            "run_op",
            json!({"repoId": id, "clientId": "c", "clientSeq": 2, "confirmed": [],
                   "request": {"op": "create_branch", "name": "x", "at": head, "checkout": false,
                               "expectWorktree": repo.path()}}),
        )
        .await
        .expect("runs");
    assert_eq!(out["status"], "done", "{out}");
    let repo_id = u32::try_from(id.as_u64().expect("a number")).expect("small");
    let waited = core
        .manager()
        .expect("git")
        .last_turn_wait(repo_id, "c")
        .expect("open");
    assert!(waited < AT_ONCE, "waited {waited:?}");
}

/// A hook that never ends (here, one that sleeps) holds the repository only until the
/// user stops it: Stop ends git and the hook (its process group), the operation fails as
/// "Stopped.", the repository is refreshed once and free for the next operation.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hook_that_hangs_is_stopped() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("other");
    let marker = repo.path().join(".git").join("hook-pid");
    let hook = repo.path().join(".git/hooks/post-checkout");
    std::fs::write(
        &hook,
        format!("#!/bin/sh\necho $$ > '{}'\nsleep 60\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let started = std::time::Instant::now();
    let run = tokio::spawn({
        let (m, wt) = (h.m.clone(), repo.path().to_path_buf());
        async move {
            m.run_op(id, in_wt(&wt, checkout_branch("other")), &[])
                .await
        }
    });
    let running = h
        .sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"].is_object()
        })
        .await
        .expect("the checkout runs");
    let op_id = running.1["op"]["id"].as_u64().unwrap();
    let pid = loop {
        if let Some(pid) = std::fs::read_to_string(&marker)
            .ok()
            .and_then(|s| s.trim().parse::<libc::pid_t>().ok())
        {
            break pid;
        }
        assert!(started.elapsed() < TIMEOUT, "the hook ran");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    assert!(!h.m.stop_op(id, op_id + 1).unwrap(), "not that one");
    assert!(h.m.stop_op(id, op_id).unwrap());
    let out = tokio::time::timeout(TIMEOUT, run)
        .await
        .expect("ends once stopped")
        .unwrap()
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "not the hook's 60 s"
    );
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert_eq!(out.message, "Stopped.");
    assert_eq!(out.op_id, op_id);
    assert_eq!(out.snapshot.op, None);
    // The hook went with git (its process group).
    let gone = loop {
        // SAFETY: kill with signal 0 only checks that the process exists.
        if unsafe { libc::kill(pid, 0) } != 0 {
            break true;
        }
        if started.elapsed() > TIMEOUT {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(gone, "the hook was ended");
    // The checkout itself had happened (the hook runs after it); the log shows the stop.
    let records = op_records(&h.log, &out);
    assert!(
        records
            .iter()
            .any(|r| r.args.first().map(String::as_str) == Some("checkout")
                && r.exit_code == -1
                && r.stderr.contains("Stopped.")),
        "{records:?}"
    );
    // Free for the next one.
    std::fs::remove_file(&hook).unwrap();
    let next = h.run(id, checkout_branch("main")).await;
    assert_eq!(next.status, OpStatus::Done, "{next:?}");
    // Nothing to stop once it has ended.
    assert!(!h.m.stop_op(id, op_id).unwrap());
}
