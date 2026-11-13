//! Integration tests for `Core::dispatch`: JSON in, JSON out, against real temp repositories
//! and a temp config dir.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gg_core::error::ErrorCode;
use gg_core::testutil::{RecordingSink, TestRepo};
use gg_core::{Core, CoreError};
use serde_json::{Value, json};
use tempfile::TempDir;

const TIMEOUT: Duration = Duration::from_secs(20);

struct Harness {
    core: Arc<Core>,
    sink: Arc<RecordingSink>,
    config: TempDir,
}

async fn harness_with(launch: Option<PathBuf>) -> Harness {
    let config = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    let core = Core::new(config.path().to_path_buf(), launch, sink.clone())
        .await
        .expect("core starts");
    Harness { core, sink, config }
}

async fn harness() -> Harness {
    harness_with(None).await
}

impl Harness {
    async fn call(&self, cmd: &str, args: Value) -> Value {
        self.core
            .dispatch(cmd, args)
            .await
            .unwrap_or_else(|e| panic!("{cmd} failed: {e:?}"))
    }

    async fn fail(&self, cmd: &str, args: Value) -> CoreError {
        match self.core.dispatch(cmd, args).await {
            Ok(v) => panic!("{cmd} succeeded with {v}"),
            Err(e) => e,
        }
    }

    /// Opens `path` and waits until its log is loaded; returns the snapshot JSON.
    async fn open_loaded(&self, path: &Path) -> Value {
        let snap = self.call("open_repo", json!({ "path": path })).await;
        let id = snap["repoId"].clone();
        let loaded = self
            .sink
            .wait_for(TIMEOUT, |name, v| {
                name == "repo-changed" && v["repoId"] == id && v["loading"] == false
            })
            .await;
        assert!(loaded.is_some(), "load finished");
        self.call("get_snapshot", json!({ "repoId": id })).await
    }
}

fn path_string(p: &Path) -> String {
    dunce::canonicalize(p)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

#[tokio::test]
async fn unknown_command_is_bad_request() {
    let h = harness().await;
    let err = h.fail("no_such_command", json!({})).await;
    assert_eq!(err.code, ErrorCode::BadRequest);
    assert!(err.message.contains("no_such_command"), "{}", err.message);
}

#[tokio::test]
async fn malformed_args_is_bad_request() {
    let h = harness().await;
    let err = h.fail("open_repo", json!({})).await;
    assert_eq!(err.code, ErrorCode::BadRequest);
    assert!(
        err.detail.unwrap().contains("path"),
        "serde message in detail"
    );

    // Unknown fields, wrong types and a non-object are all rejected.
    for (cmd, args) in [
        ("open_repo", json!({"path": "/x", "extra": 1})),
        (
            "get_rows",
            json!({"repoId": "1", "generation": 1, "start": 0, "count": 1}),
        ),
        (
            "get_rows",
            json!({"repoId": -1, "generation": 1, "start": 0, "count": 1}),
        ),
        ("get_app_state", json!({"x": 1})),
        ("get_app_state", json!([])),
        ("update_settings", json!({"patch": {"theme": "blue"}})),
        ("set_window_focused", json!({})),
    ] {
        let err = h.fail(cmd, args.clone()).await;
        assert_eq!(err.code, ErrorCode::BadRequest, "{cmd} {args}");
    }
}

#[tokio::test]
async fn open_then_rows_via_dispatch() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.commit("two");
    repo.commit("three");
    let h = harness().await;
    let first = h.call("open_repo", json!({ "path": repo.path() })).await;
    for key in [
        "repoId",
        "totalRows",
        "activeWorktree",
        "rootPath",
        "viewOptions",
    ] {
        assert!(first.get(key).is_some(), "{key} missing from {first}");
    }
    let snap = h.open_loaded(repo.path()).await;
    assert_eq!(snap["repoId"], first["repoId"], "same repository, same id");
    assert_eq!(snap["totalRows"], 3);
    assert_eq!(snap["activeWorktree"], path_string(repo.path()));

    let page = h
        .call(
            "get_rows",
            json!({"repoId": snap["repoId"], "generation": snap["generation"], "start": 0, "count": 10}),
        )
        .await;
    assert_eq!(page["generation"], snap["generation"]);
    let rows = page["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["subject"], "three");
    assert_eq!(rows[0]["isHead"], true);

    let stale = h
        .fail(
            "get_rows",
            json!({"repoId": snap["repoId"], "generation": 999, "start": 0, "count": 10}),
        )
        .await;
    assert_eq!(stale.code, ErrorCode::StaleGeneration);

    let hash = rows[2]["hash"].as_str().unwrap();
    let id = &snap["repoId"];
    assert_eq!(
        h.call("find_row", json!({"repoId": id, "hash": hash}))
            .await,
        2
    );
    assert_eq!(
        h.call("find_row", json!({"repoId": id, "hash": "0".repeat(40)}))
            .await,
        Value::Null
    );
    assert_eq!(
        h.call("search_commits", json!({"repoId": id, "query": "tw"}))
            .await,
        json!([1])
    );
    let details = h
        .call("commit_details", json!({"repoId": id, "hash": hash}))
        .await;
    assert_eq!(details["message"], "one");
    assert!(details["authorEmail"].is_string());

    let opts = json!({"branches": null, "showRemotes": false, "showTags": false});
    let changed = h
        .call("set_view_options", json!({"repoId": id, "options": opts}))
        .await;
    assert_eq!(changed["viewOptions"], opts);
    let refreshed = h.call("refresh", json!({"repoId": id})).await;
    assert_eq!(refreshed["repoId"], *id);
    // No remotes: nothing to fetch, still `null`.
    assert_eq!(h.call("fetch", json!({"repoId": id})).await, Value::Null);
    assert_eq!(
        h.call("set_window_focused", json!({"focused": false}))
            .await,
        Value::Null
    );

    let log = h.call("command_log", json!({"sinceId": 0})).await;
    let log = log.as_array().unwrap();
    assert!(!log.is_empty());
    for key in [
        "id",
        "kind",
        "cwd",
        "args",
        "startedAtMs",
        "durationMs",
        "exitCode",
    ] {
        assert!(log[0].get(key).is_some(), "{key} missing from {}", log[0]);
    }
    let last = log.last().unwrap()["id"].as_u64().unwrap();
    assert_eq!(
        h.call("command_log", json!({"sinceId": last})).await,
        json!([])
    );

    assert_eq!(
        h.call("close_repo", json!({"repoId": id})).await,
        Value::Null
    );
    let gone = h.fail("get_snapshot", json!({"repoId": id})).await;
    assert_eq!(gone.code, ErrorCode::UnknownRepo);
}

#[tokio::test]
async fn open_adds_recent() {
    let other = TestRepo::new_in_subdir("other repo");
    other.commit("x");
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    h.call("open_repo", json!({ "path": other.path() })).await;
    let snap = h.call("open_repo", json!({ "path": repo.path() })).await;
    let state = h.call("get_app_state", json!({})).await;
    let recents = state["recents"].as_array().unwrap();
    assert_eq!(recents.len(), 2);
    assert_eq!(recents[0]["path"], snap["rootPath"]);
    assert_eq!(recents[0]["name"], "repo");
    assert!(recents[0]["lastOpenedMs"].as_i64().unwrap() > 0);
    assert_eq!(recents[1]["name"], "other repo");

    // A failed open adds nothing.
    let not_repo = tempfile::tempdir().unwrap();
    let err = h
        .fail("open_repo", json!({ "path": not_repo.path() }))
        .await;
    assert_eq!(err.code, ErrorCode::NotARepo);
    let state = h.call("get_app_state", json!({})).await;
    assert_eq!(state["recents"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn open_missing_recent_path_errors_and_recent_is_removable() {
    let repo = TestRepo::new();
    repo.commit("one");
    let keep = TestRepo::new();
    keep.commit("one");
    let h = harness().await;
    h.call("open_repo", json!({ "path": keep.path() })).await;
    let snap = h.call("open_repo", json!({ "path": repo.path() })).await;
    let root = snap["rootPath"].as_str().unwrap().to_owned();
    h.call("close_repo", json!({ "repoId": snap["repoId"] }))
        .await;

    std::fs::remove_dir_all(repo.path()).unwrap();
    let err = h.fail("open_repo", json!({ "path": root })).await;
    assert_eq!(err.code, ErrorCode::PathMissing);

    let rest = h.call("remove_recent", json!({ "path": root })).await;
    let rest = rest.as_array().unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0]["path"], path_string(keep.path()));
    let state = h.call("get_app_state", json!({})).await;
    assert_eq!(state["recents"], Value::Array(rest.clone()));
}

/// The session's tabs and the launch path start opening (and loading) before the UI asks;
/// the UI's `open_repo` then finds them open.
#[tokio::test]
async fn startup_repos_open_before_the_ui_asks() {
    let session_repo = TestRepo::new();
    session_repo.commit("in the session");
    let launch_repo = TestRepo::new();
    launch_repo.commit("launched");
    let config = tempfile::tempdir().unwrap();
    let session = json!({"tabs": [session_repo.path()], "active": session_repo.path()});
    let first = Core::new(
        config.path().to_path_buf(),
        None,
        Arc::new(RecordingSink::default()),
    )
    .await
    .unwrap();
    first.dispatch("save_session", session).await.unwrap();

    let sink = Arc::new(RecordingSink::default());
    let core = Core::new(
        config.path().to_path_buf(),
        Some(launch_repo.path().to_path_buf()),
        sink.clone(),
    )
    .await
    .unwrap();
    core.open_startup_repos();
    for id in [1, 2] {
        let loaded = sink
            .wait_for(TIMEOUT, |name, v| {
                name == "repo-changed" && v["repoId"] == id && v["loading"] == false
            })
            .await;
        assert!(
            loaded.is_some(),
            "repository {id} was not opened and loaded"
        );
    }
    let mut ids = Vec::new();
    for path in [session_repo.path(), launch_repo.path()] {
        let snap = core
            .dispatch("open_repo", json!({ "path": path }))
            .await
            .unwrap();
        ids.push(snap["repoId"].as_u64().unwrap());
    }
    ids.sort_unstable();
    assert_eq!(ids, [1, 2], "open_repo opened a repository again");
}

#[tokio::test]
async fn launch_path_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let h = harness_with(Some(dir.path().to_path_buf())).await;
    let state = h.call("get_app_state", json!({})).await;
    assert_eq!(state["launchPath"], dir.path().to_string_lossy().as_ref());

    let none = harness().await;
    let state = none.call("get_app_state", json!({})).await;
    let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var_os(home_var)
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string_lossy().into_owned());
    assert_eq!(
        state,
        json!({
            "recents": [],
            "session": {"tabs": [], "active": null},
            "settings": {"theme": "system", "fetchIntervalSecs": 300},
            "launchPath": null,
            "homeDir": home
        })
    );
}

#[tokio::test]
async fn session_and_settings_persist() {
    let h = harness().await;
    let saved = h
        .call(
            "save_session",
            json!({"tabs": ["/a", "/b"], "active": "/b"}),
        )
        .await;
    assert_eq!(saved, Value::Null);
    let settings = h
        .call(
            "update_settings",
            json!({"patch": {"theme": "dark", "fetchIntervalSecs": 0}}),
        )
        .await;
    assert_eq!(settings, json!({"theme": "dark", "fetchIntervalSecs": 0}));
    let err = h
        .fail(
            "update_settings",
            json!({"patch": {"fetchIntervalSecs": 5}}),
        )
        .await;
    assert_eq!(err.code, ErrorCode::BadRequest);

    // A new core on the same config dir starts from what was saved.
    let again = Core::new(
        h.config.path().to_path_buf(),
        None,
        Arc::new(RecordingSink::default()),
    )
    .await
    .unwrap();
    let state = again.dispatch("get_app_state", json!({})).await.unwrap();
    assert_eq!(
        state["session"],
        json!({"tabs": ["/a", "/b"], "active": "/b"})
    );
    assert_eq!(
        state["settings"],
        json!({"theme": "dark", "fetchIntervalSecs": 0})
    );
}

#[tokio::test]
async fn perf_marks_are_kept_and_logged() {
    let h = harness().await;
    let log = h.config.path().join("perf.log");
    h.core.set_perf_log(Some(log.clone()));
    let before = unix_ms();
    assert_eq!(
        h.call("perf_mark", json!({"name": "first-rows", "ms": 412.5}))
            .await,
        Value::Null
    );
    h.call("perf_mark", json!({"name": "fully-loaded", "ms": 900}))
        .await;
    let after = unix_ms();
    assert_eq!(
        h.core.perf_marks(),
        vec![
            ("first-rows".to_owned(), 412.5),
            ("fully-loaded".to_owned(), 900.0)
        ]
    );
    // `name<TAB>ms<TAB>unix epoch ms when the mark was recorded`.
    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<Vec<&str>> = text.lines().map(|l| l.split('\t').collect()).collect();
    assert_eq!(lines.len(), 2, "{text:?}");
    assert_eq!(lines[0][..2], ["first-rows", "412.5"]);
    assert_eq!(lines[1][..2], ["fully-loaded", "900"]);
    for line in &lines {
        assert_eq!(line.len(), 3, "{text:?}");
        let at: u64 = line[2].parse().expect("epoch ms is an integer");
        assert!(
            (before..=after).contains(&at),
            "{at} not in {before}..={after}"
        );
    }

    // Without a log path, marks are only kept in memory.
    h.core.set_perf_log(None);
    h.call("perf_mark", json!({"name": "later", "ms": 1})).await;
    assert_eq!(h.core.perf_marks().len(), 3);
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 2);
}

/// `fetch` returns as soon as the fetch has started; progress arrives as events.
#[cfg(unix)]
#[tokio::test]
async fn fetch_returns_before_git_finishes() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TestRepo::new();
    repo.commit("one");
    let _origin = repo.clone_as_remote();
    // An upload-pack that never answers, so the fetch hangs until it is killed.
    let hung = repo.dir.path().join("hung-upload-pack");
    std::fs::write(&hung, "#!/bin/sh\ncat >/dev/null\n").unwrap();
    std::fs::set_permissions(&hung, std::fs::Permissions::from_mode(0o755)).unwrap();
    repo.git(&["config", "remote.origin.uploadpack", hung.to_str().unwrap()]);

    let h = harness().await;
    let snap = h.open_loaded(repo.path()).await;
    let id = &snap["repoId"];
    let returned = tokio::time::timeout(
        Duration::from_secs(5),
        h.core.dispatch("fetch", json!({ "repoId": id })),
    )
    .await
    .expect("fetch returned while git still runs");
    assert_eq!(returned.unwrap(), Value::Null);
    let running = h
        .sink
        .wait_for(TIMEOUT, |name, v| {
            name == "repo-changed" && v["repoId"] == *id && v["fetch"]["running"] == true
        })
        .await;
    assert!(running.is_some(), "fetch.running was emitted");

    let unknown = h.fail("fetch", json!({"repoId": 999})).await;
    assert_eq!(unknown.code, ErrorCode::UnknownRepo);
}

fn unix_ms() -> u64 {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    u64::try_from(since.as_millis()).unwrap()
}
