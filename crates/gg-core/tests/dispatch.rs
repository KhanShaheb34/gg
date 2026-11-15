//! Integration tests for `Core::dispatch`: JSON in, JSON out, against real temp repositories
//! and a temp config dir.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gg_core::error::ErrorCode;
use gg_core::testutil::{RecordingSink, TestRepo, UNWATCHABLE};
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
        (
            "set_view_options",
            json!({"repoId": 1, "options": {"branches": null, "showRemotes": true, "showTags": true, "showNotes": true}}),
        ),
    ] {
        let err = h.fail(cmd, args.clone()).await;
        assert_eq!(err.code, ErrorCode::BadRequest, "{cmd} {args}");
    }
}

/// I7 / R42: opening a folder of another worktree of a repository already open answers with
/// that repository, whose tab acts on its own worktree, and says which worktree the folder
/// was in, so the UI can say which folder the tab acts on. Its own folders say nothing.
#[tokio::test]
async fn opening_another_worktree_of_an_open_repository_says_so() {
    let repo = TestRepo::new();
    repo.commit("one");
    let hotfix = repo.add_worktree("hotfix");
    let h = harness().await;
    let first = h.call("open_repo", json!({ "path": repo.path() })).await;
    assert_eq!(first["openedWorktree"], Value::Null);
    let main = first["activeWorktree"].clone();

    let again = h.call("open_repo", json!({ "path": hotfix })).await;
    assert_eq!(again["repoId"], first["repoId"]);
    assert_eq!(
        again["activeWorktree"], main,
        "the tab still acts on its own"
    );
    assert_eq!(again["openedWorktree"], json!(path_string(&hotfix)));
    let same = h.call("open_repo", json!({ "path": repo.path() })).await;
    assert_eq!(same["openedWorktree"], Value::Null);
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
    let mut shown = opts.clone();
    shown["showStashes"] = json!(true);
    assert_eq!(changed["viewOptions"], shown);
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

/// A core whose saved session holds `tabs` and whose launch path is `launch`.
async fn core_with_session(
    config: &Path,
    tabs: &[&Path],
    launch: Option<&Path>,
) -> (Arc<Core>, Arc<RecordingSink>) {
    let first = Core::new(
        config.to_path_buf(),
        None,
        Arc::new(RecordingSink::default()),
    )
    .await
    .unwrap();
    let session = json!({"tabs": tabs, "active": tabs.first()});
    first.dispatch("save_session", session).await.unwrap();
    let sink = Arc::new(RecordingSink::default());
    let core = Core::new(
        config.to_path_buf(),
        launch.map(Path::to_path_buf),
        sink.clone(),
    )
    .await
    .unwrap();
    (core, sink)
}

/// Git commands run so far whose arguments include `arg`.
async fn commands_with(core: &Arc<Core>, arg: &str) -> usize {
    let log = core
        .dispatch("command_log", json!({"sinceId": 0}))
        .await
        .unwrap();
    log.as_array()
        .unwrap()
        .iter()
        .filter(|r| r["args"].as_array().unwrap().iter().any(|a| a == arg))
        .count()
}

async fn wait_loaded_event(sink: &RecordingSink, id: u64) {
    let loaded = sink
        .wait_for(TIMEOUT, |name, v| {
            name == "repo-changed" && v["repoId"] == id && v["loading"] == false
        })
        .await;
    assert!(loaded.is_some(), "repository {id} did not finish loading");
}

fn repo_errors(sink: &RecordingSink) -> Vec<Value> {
    sink.events()
        .into_iter()
        .filter(|(name, _)| name == "repo-error")
        .map(|(_, v)| v)
        .collect()
}

/// An error found while a repository was opened before the UI asked for it (here: its git
/// dir cannot be watched) is held until the UI's `open_repo` adopts the repository, then
/// delivered once.
#[tokio::test]
async fn errors_of_a_pre_opened_repo_reach_the_ui_once() {
    let repo = TestRepo::new_in_subdir(UNWATCHABLE);
    repo.commit("one");
    let config = tempfile::tempdir().unwrap();
    let (core, sink) = core_with_session(config.path(), &[], Some(repo.path())).await;
    core.open_startup_repos();
    wait_loaded_event(&sink, 1).await;
    assert!(repo_errors(&sink).is_empty(), "nobody is listening yet");

    let snap = core
        .dispatch("open_repo", json!({ "path": repo.path() }))
        .await
        .unwrap();
    assert_eq!(snap["repoId"], 1);
    let errors = repo_errors(&sink);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0]["repoId"], 1);
    let message = errors[0]["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("Live updates are unavailable"),
        "{message}"
    );

    core.dispatch("open_repo", json!({ "path": repo.path() }))
        .await
        .unwrap();
    assert_eq!(repo_errors(&sink).len(), 1, "delivered again");
}

/// Moves the loose object of commit `hash` out of `repo` (so `git log` fails on it) and
/// returns a closure that puts it back.
fn hide_object(repo: &TestRepo, hash: &str) -> impl FnOnce() {
    let from = repo
        .path()
        .join(".git/objects")
        .join(&hash[..2])
        .join(&hash[2..]);
    let to = repo.path().join(".git/hidden-object");
    std::fs::rename(&from, &to).expect("the commit is a loose object");
    move || std::fs::rename(&to, &from).unwrap()
}

/// Waits until `n` snapshots of repository `id` with `loading: false` have been emitted.
async fn wait_loaded_events(sink: &RecordingSink, id: u64, n: usize) {
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        let loaded = sink
            .events()
            .iter()
            .filter(|(name, v)| {
                name == "repo-changed" && v["repoId"] == id && v["loading"] == false
            })
            .count();
        if loaded >= n {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "repository {id} did not load {n} times"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A failed log load of a repository opened at start is held like any other error, and
/// delivered once to the UI's `open_repo`.
#[tokio::test]
async fn a_failed_load_of_a_pre_opened_repo_reaches_the_ui_once() {
    let repo = TestRepo::new();
    let first = repo.commit("one");
    repo.commit("two");
    let _restore = hide_object(&repo, &first);
    let config = tempfile::tempdir().unwrap();
    let (core, sink) = core_with_session(config.path(), &[], Some(repo.path())).await;
    core.open_startup_repos();
    wait_loaded_event(&sink, 1).await;
    assert!(repo_errors(&sink).is_empty(), "nobody is listening yet");

    core.dispatch("open_repo", json!({ "path": repo.path() }))
        .await
        .unwrap();
    let errors = repo_errors(&sink);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0]["repoId"], 1);
    assert_eq!(errors[0]["error"]["code"], "git_failed");
}

/// A failed load that a later reload makes good is not reported when the UI finally asks.
#[tokio::test]
async fn a_held_load_error_is_dropped_when_a_reload_succeeds() {
    let repo = TestRepo::new();
    let first = repo.commit("one");
    repo.commit("two");
    let restore = hide_object(&repo, &first);
    let config = tempfile::tempdir().unwrap();
    let (core, sink) = core_with_session(config.path(), &[], Some(repo.path())).await;
    core.open_startup_repos();
    wait_loaded_events(&sink, 1, 1).await;

    restore();
    core.dispatch("refresh", json!({ "repoId": 1 }))
        .await
        .unwrap();
    wait_loaded_events(&sink, 1, 2).await;
    let snap = core
        .dispatch("open_repo", json!({ "path": repo.path() }))
        .await
        .unwrap();
    assert_eq!(snap["totalRows"], 2, "the reload read the whole history");
    assert!(repo_errors(&sink).is_empty(), "{:?}", repo_errors(&sink));
}

/// The same failure for a repository the UI opens itself is reported at once, as before.
#[tokio::test]
async fn errors_of_a_repo_the_ui_opens_are_reported_at_once() {
    let repo = TestRepo::new_in_subdir(UNWATCHABLE);
    repo.commit("one");
    let h = harness().await;
    h.call("open_repo", json!({ "path": repo.path() })).await;
    assert_eq!(repo_errors(&h.sink).len(), 1);
}

/// The UI's `open_repo` arriving while the pre-open is still reading the repository waits
/// for it instead of opening the repository a second time.
#[tokio::test]
async fn open_repo_during_the_pre_open_opens_once() {
    let repo = TestRepo::new();
    repo.commit("one");
    let config = tempfile::tempdir().unwrap();
    let (core, sink) = core_with_session(config.path(), &[], Some(repo.path())).await;
    core.open_startup_repos();
    let snap = core
        .dispatch("open_repo", json!({ "path": repo.path() }))
        .await
        .unwrap();
    assert_eq!(snap["repoId"], 1);
    wait_loaded_event(&sink, 1).await;
    assert_eq!(
        commands_with(&core, "for-each-ref").await,
        1,
        "refs read twice"
    );
    assert_eq!(commands_with(&core, "log").await, 1, "log loaded twice");
}

/// A repository that is both a session tab and the launch path is opened once: one
/// repository, one read, one watcher.
#[tokio::test]
async fn session_tab_that_is_also_the_launch_path_opens_once() {
    let repo = TestRepo::new();
    repo.commit("one");
    let config = tempfile::tempdir().unwrap();
    let (core, sink) = core_with_session(config.path(), &[repo.path()], Some(repo.path())).await;
    core.open_startup_repos();
    wait_loaded_event(&sink, 1).await;
    let snap = core
        .dispatch("open_repo", json!({ "path": repo.path() }))
        .await
        .unwrap();
    assert_eq!(snap["repoId"], 1);
    assert_eq!(
        commands_with(&core, "for-each-ref").await,
        1,
        "refs read twice"
    );
    assert_eq!(commands_with(&core, "log").await, 1, "log loaded twice");
    let opened = sink
        .events()
        .iter()
        .filter(|(name, v)| name == "repo-changed" && v["repoId"] != 1)
        .count();
    assert_eq!(opened, 0, "a second repository was opened");
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

/// Folders the operating system asks the app to open (Finder's Open With, a folder dropped
/// on the Dock icon) wait in a queue the UI takes once; each arrival is announced with
/// `paths-opened`, and the repositories start opening at once.
#[tokio::test]
async fn folders_opened_by_the_system_are_queued_announced_and_taken_once() {
    let h = harness().await;
    let repo = TestRepo::new();
    repo.commit("one");
    let path = repo.path().to_path_buf();
    assert_eq!(h.call("take_opened_paths", json!({})).await, json!([]));
    h.core.open_later(vec![path.clone()]);
    let announced = h
        .sink
        .wait_for(TIMEOUT, |name, _| name == "paths-opened")
        .await;
    assert!(announced.is_some());
    // Opened ahead, like the launch path.
    let opened = h
        .sink
        .wait_for(TIMEOUT, |name, _| name == "repo-changed")
        .await;
    assert!(opened.is_some(), "the repository started opening");
    let taken = h.call("take_opened_paths", json!({})).await;
    assert_eq!(taken, json!([path.to_string_lossy()]));
    assert_eq!(h.call("take_opened_paths", json!({})).await, json!([]));
}

#[tokio::test]
async fn launch_path_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let h = harness_with(Some(dir.path().to_path_buf())).await;
    let state = h.call("get_app_state", json!({})).await;
    assert_eq!(state["launchPath"], dir.path().to_string_lossy().as_ref());

    let none = harness().await;
    let mut state = none.call("get_app_state", json!({})).await;
    // The `gg` command as this machine has it (see the_gg_command_installs_and_uninstalls);
    // no shell is asked unless `start_cli` says which.
    let cli = state.as_object_mut().unwrap().remove("cli").unwrap();
    assert_eq!(cli["shadowedBy"], Value::Null);
    assert!(cli["candidates"].is_array());
    let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var_os(home_var)
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string_lossy().into_owned());
    assert_eq!(
        state,
        json!({
            "recents": [],
            "session": {"tabs": [], "active": null},
            "settings": {"theme": "system", "fetchIntervalSecs": 300, "editorCommand": null},
            "launchPath": null,
            "homeDir": home,
            "settingsProblem": null,
            // The git installed here is new enough (M3).
            "gitProblem": null
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
    assert_eq!(
        settings,
        json!({"theme": "dark", "fetchIntervalSecs": 0, "editorCommand": null})
    );
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
        json!({"theme": "dark", "fetchIntervalSecs": 0, "editorCommand": null})
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

/// A settings file that cannot be read is reported once, through `get_app_state`, never
/// written over; changes apply for the session; `reset_settings` moves it aside and saving
/// resumes.
#[cfg(unix)]
#[tokio::test]
async fn unreadable_settings_are_reported_kept_and_can_be_reset() {
    use std::os::unix::fs::PermissionsExt;
    let config = tempfile::tempdir().unwrap();
    let path = config.path().join("state.json");
    std::fs::write(&path, "{}").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let core = Core::new(
        config.path().to_path_buf(),
        None,
        Arc::new(RecordingSink::default()),
    )
    .await
    .unwrap();
    let state = core.dispatch("get_app_state", json!({})).await.unwrap();
    let problem = state["settingsProblem"]
        .as_str()
        .expect("reported")
        .to_owned();
    assert!(problem.contains("could not be read"), "{problem}");

    for i in 0..10 {
        let tab = format!("/t{i}");
        core.dispatch("save_session", json!({"tabs": [tab], "active": tab}))
            .await
            .expect("a session change applies even though it is not saved");
    }
    let settings = core
        .dispatch(
            "update_settings",
            json!({"patch": {"fetchIntervalSecs": 60}}),
        )
        .await
        .unwrap();
    assert_eq!(settings["fetchIntervalSecs"], 60);
    let state = core.dispatch("get_app_state", json!({})).await.unwrap();
    assert_eq!(state["session"]["tabs"], json!(["/t9"]));
    assert_eq!(state["settings"]["fetchIntervalSecs"], 60);

    core.dispatch("reset_settings", json!({})).await.unwrap();
    let state = core.dispatch("get_app_state", json!({})).await.unwrap();
    assert_eq!(state["settingsProblem"], Value::Null);
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["settings"]["fetchIntervalSecs"], 60);
    for entry in std::fs::read_dir(config.path()).unwrap() {
        let _ = std::fs::set_permissions(
            entry.unwrap().path(),
            std::fs::Permissions::from_mode(0o644),
        );
    }
}

/// Settings changes land in the order they were asked for, however quickly they come.
#[tokio::test]
async fn settings_changes_land_in_call_order() {
    let h = harness().await;
    let mut tasks = tokio::task::JoinSet::new();
    let themes = ["dark", "light"];
    for i in 0..40 {
        let core = h.core.clone();
        let theme = themes[i % 2];
        tasks.spawn(async move {
            core.dispatch("update_settings", json!({"patch": {"theme": theme}}))
                .await
                .unwrap();
        });
        // Each call reaches the core before the next is made, as calls from the UI do.
        tokio::task::yield_now().await;
    }
    tasks.join_all().await;
    let state = h.call("get_app_state", json!({})).await;
    assert_eq!(state["settings"]["theme"], "light");
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(h.config.path().join("state.json")).unwrap())
            .unwrap();
    assert_eq!(saved["settings"]["theme"], "light");
}

/// Marks kept in memory are bounded: the oldest go once there are 1,000.
#[tokio::test]
async fn perf_marks_in_memory_are_bounded() {
    let h = harness().await;
    for i in 0..1_200 {
        h.call("perf_mark", json!({"name": format!("m{i}"), "ms": i}))
            .await;
    }
    let marks = h.core.perf_marks();
    assert_eq!(marks.len(), 1_000);
    assert_eq!(marks[0].0, "m200");
    assert_eq!(marks[999].0, "m1199");
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

/// `open_in_editor` refuses paths outside the worktree and runs the editor command from
/// the settings (a program that does not exist here, so nothing is opened).
#[tokio::test]
async fn open_in_editor_uses_the_editor_command_setting() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let snap = h.open_loaded(repo.path()).await;
    let id = snap["repoId"].clone();

    let err = h
        .fail("open_in_editor", json!({"repoId": id, "path": "../x"}))
        .await;
    assert_eq!(err.code, ErrorCode::BadRequest, "{err:?}");
    let patch = json!({"patch": {"editorCommand": "gg-no-such-editor --wait"}});
    h.call("update_settings", patch).await;
    let err = h
        .fail("open_in_editor", json!({"repoId": id, "path": null}))
        .await;
    assert_eq!(err.code, ErrorCode::Io, "{err:?}");
    assert!(err.message.contains("gg-no-such-editor"), "{err:?}");
    let err = h
        .fail("open_in_editor", json!({"repoId": 999, "path": null}))
        .await;
    assert_eq!(err.code, ErrorCode::UnknownRepo, "{err:?}");
}

/// With launches recorded to a file (the dev server's `--record-editor-launches`), nothing is
/// started: each launch is a line of JSON, the program and its arguments.
#[tokio::test]
async fn editor_launches_can_be_recorded_to_a_file() {
    let repo = TestRepo::new();
    repo.write("a b.txt", "x\n");
    repo.commit_all("one");
    let h = harness().await;
    let record = tempfile::tempdir().unwrap();
    let file = record.path().join("launches.jsonl");
    h.core.record_editor_launches_to(Some(file.clone()));
    let snap = h.open_loaded(repo.path()).await;
    let id = snap["repoId"].clone();
    let patch = json!({"patch": {"editorCommand": "gg-no-such-editor --wait"}});
    h.call("update_settings", patch).await;

    let note = h
        .call("open_in_editor", json!({"repoId": id, "path": "a b.txt"}))
        .await;
    assert_eq!(note, Value::Null);
    h.call("open_in_editor", json!({"repoId": id, "path": null}))
        .await;
    let lines: Vec<Value> = std::fs::read_to_string(&file)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let root = dunce::canonicalize(repo.path()).unwrap();
    assert_eq!(
        lines,
        [
            json!(["gg-no-such-editor", "--wait", root.join("a b.txt")]),
            json!(["gg-no-such-editor", "--wait", root]),
        ]
    );
}

/// R36: a credential in a URL (the `user:password@` part) never reaches the UI, wherever git
/// or the config put it: a failed operation's message and output, the fetch state, errors.
/// git hides credentials in the URLs it prints itself; a hook or a helper does not, so a
/// pre-push hook and an upload-pack print one here.
#[cfg(unix)]
#[tokio::test]
async fn credentials_in_urls_never_reach_the_ui() {
    use std::os::unix::fs::PermissionsExt;
    const SECRET: &str = "s3cr3t-t0ken";
    // The line stays git's latest for a moment, so it goes out as a progress event (those are
    // the latest line, at most ten a second).
    let say = format!(
        "echo \"could not reach https://user:{SECRET}@example.invalid/x.git\" >&2; sleep 0.3; exit 1"
    );
    let repo = TestRepo::new();
    repo.commit("one");
    let _origin = repo.clone_as_remote();
    let script = |path: &Path| {
        std::fs::write(path, format!("#!/bin/sh\ncat >/dev/null &\n{say}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    script(&repo.path().join(".git/hooks/pre-push"));
    let upload_pack = repo.dir.path().join("failing-upload-pack");
    script(&upload_pack);
    repo.git(&[
        "config",
        "remote.origin.uploadpack",
        upload_pack.to_str().unwrap(),
    ]);

    let h = harness().await;
    let snap = h.open_loaded(repo.path()).await;
    let id = snap["repoId"].clone();

    // A failed push: git's message and output come back in the outcome.
    let outcome = h
        .call(
            "run_op",
            json!({"repoId": id, "clientId": "c", "clientSeq": 1, "confirmed": [],
                   "request": {"op": "push", "branch": "main", "remote": "origin",
                               "to": null, "force": false}}),
        )
        .await;
    assert_eq!(outcome["status"], "failed", "{outcome}");
    let detail = outcome["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("https://***@example.invalid"), "{outcome}");

    // A failed fetch: its one-line reason and git's output are in the fetch state.
    h.call("fetch", json!({"repoId": id})).await;
    let failed = h
        .sink
        .wait_for(TIMEOUT, |name, v| {
            name == "repo-changed" && v["repoId"] == id && v["fetch"]["lastError"].is_string()
        })
        .await
        .expect("the fetch failed");
    assert!(
        failed.1["fetch"]["lastErrorDetail"]
            .as_str()
            .unwrap_or_default()
            .contains("https://***@example.invalid"),
        "{}",
        failed.1["fetch"]
    );
    let refreshed = h.call("refresh", json!({"repoId": id})).await;

    // An error: a folder that does not exist, named in the error.
    let missing = repo
        .dir
        .path()
        .join(format!("https://user:{SECRET}@host/gone"));
    let err = h.fail("open_repo", json!({"path": missing})).await;
    assert!(err.message.contains("https://***@host/gone"), "{err:?}");
    let err = serde_json::to_string(&err).unwrap();

    // The push's progress lines (git's stderr as it comes) are events too.
    assert!(
        h.sink.events().iter().any(|(n, v)| n == "op-progress"
            && v["line"]
                .as_str()
                .is_some_and(|l| l.contains("https://***@example.invalid"))),
        "a progress line carried the masked URL"
    );

    let mut seen = vec![outcome.to_string(), refreshed.to_string(), err];
    seen.extend(h.sink.events().iter().map(|(n, v)| format!("{n} {v}")));
    for text in &seen {
        assert!(
            !text.contains(SECRET),
            "the credential reached the UI: {text}"
        );
    }
}

/// The `gg` command through `install_cli` / `uninstall_cli` and `get_app_state`, with a
/// stand-in login shell and a `PATH` and home folder of temp folders only.
#[cfg(unix)]
#[tokio::test]
async fn the_gg_command_installs_and_uninstalls() {
    use std::os::unix::fs::PermissionsExt;

    let h = harness().await;
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let local = home.join(".local/bin");
    let bin = home.join("bin");
    let outside = root.path().join("outside");
    for dir in [&local, &bin, &outside] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let shell = root.path().join("fake-shell");
    std::fs::write(&shell, "#!/bin/sh\nalias gg='git log'\neval \"$2\"\n").unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
    h.core.start_cli(gg_core::cli::CliSetup {
        shell: Some(shell),
        search_path: Some(std::env::join_paths([&outside, &bin, &local]).unwrap()),
        home: Some(home.clone()),
    });

    // The shell's answer arrives in the background.
    let deadline = std::time::Instant::now() + TIMEOUT;
    let cli = loop {
        let cli = h.call("get_app_state", json!({})).await["cli"].clone();
        if !cli["shadowedBy"].is_null() || std::time::Instant::now() > deadline {
            break cli;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let expected = json!({
        "installed": null,
        "shadowedBy": "an alias for git log",
        "candidates": [s(&local), s(&bin)],
    });
    assert_eq!(cli, expected);
    // Announced too, for a UI that read the state before the answer came.
    let announced = h
        .sink
        .wait_for(TIMEOUT, |name, _| name == "cli-changed")
        .await;
    assert_eq!(announced.map(|(_, v)| v), Some(expected));

    // Only a candidate folder.
    let err = h.fail("install_cli", json!({ "dir": outside })).await;
    assert_eq!(err.code, ErrorCode::BadRequest);
    assert!(!outside.join("gg").exists());

    let cli = h.call("install_cli", json!({ "dir": bin })).await;
    assert_eq!(cli["installed"], s(&bin.join("gg")));
    assert_eq!(cli["shadowedBy"], "an alias for git log");
    assert!(gg_core::cli::has_marker(&bin.join("gg")));
    assert_eq!(
        h.call("get_app_state", json!({})).await["cli"]["installed"],
        s(&bin.join("gg"))
    );

    let cli = h.call("uninstall_cli", json!({})).await;
    assert_eq!(cli["installed"], Value::Null);
    assert_eq!(cli["removed"], json!([s(&bin.join("gg"))]));
    assert!(!bin.join("gg").exists());
    // Again: nothing to remove, and it says so.
    let cli = h.call("uninstall_cli", json!({})).await;
    assert_eq!(cli["removed"], json!([]));

    let err = h.fail("uninstall_cli", json!({ "dir": bin })).await;
    assert_eq!(err.code, ErrorCode::BadRequest);
    let err = h.fail("install_cli", json!({})).await;
    assert_eq!(err.code, ErrorCode::BadRequest);
}

/// Without a login shell to ask, nothing is said to shadow the command.
#[tokio::test]
async fn the_gg_command_without_a_shell_check() {
    let h = harness().await;
    let root = tempfile::tempdir().unwrap();
    h.core.start_cli(gg_core::cli::CliSetup {
        shell: None,
        search_path: Some(root.path().as_os_str().to_owned()),
        home: Some(root.path().join("home")),
    });
    let cli = h.call("get_app_state", json!({})).await["cli"].clone();
    assert_eq!(
        cli,
        json!({ "installed": null, "shadowedBy": null, "candidates": [] })
    );
}
