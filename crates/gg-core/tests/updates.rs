//! Updating gg through `Core::dispatch`: the commands the UI calls, with a fake backend in
//! place of the app's updater (the network and the app's files are never touched).

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gg_core::error::ErrorCode;
use gg_core::testutil::{RecordingSink, TestRepo};
use gg_core::update::{
    Blocker, BoxFuture, Offer, Progress, UpdateBackend, UpdateFailure, is_newer,
};
use gg_core::{Core, CoreError};
use serde_json::{Value, json};

mod common;
use common::{create_branch, in_wt};

const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Default)]
struct Fake {
    offer: Mutex<Option<String>>,
    blocker: Mutex<Option<Blocker>>,
    checks: AtomicUsize,
    installs: AtomicUsize,
}

impl UpdateBackend for Fake {
    fn current_version(&self) -> String {
        "0.4.0".into()
    }
    fn blocker(&self) -> Option<Blocker> {
        self.blocker.lock().unwrap().clone()
    }
    fn check(&self) -> BoxFuture<'_, Result<Option<Offer>, UpdateFailure>> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        let offer = self
            .offer
            .lock()
            .unwrap()
            .clone()
            .map(|version| Offer { version });
        Box::pin(async move { Ok(offer) })
    }
    fn download<'a>(
        &'a self,
        _version: &'a str,
        mut progress: Progress<'a>,
    ) -> BoxFuture<'a, Result<(), UpdateFailure>> {
        Box::pin(async move {
            progress(10, Some(10));
            Ok(())
        })
    }
    fn install_and_restart(&self) -> Result<(), UpdateFailure> {
        self.installs.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct H {
    core: Arc<Core>,
    sink: Arc<RecordingSink>,
    _config: tempfile::TempDir,
}

async fn harness(fake: Option<Arc<Fake>>) -> H {
    let config = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    let core = Core::new(config.path().to_path_buf(), None, sink.clone())
        .await
        .unwrap();
    if let Some(fake) = fake {
        core.set_update_backend(fake);
    }
    H {
        core,
        sink,
        _config: config,
    }
}

impl H {
    async fn call(&self, cmd: &str, args: Value) -> Value {
        self.core
            .dispatch(cmd, args)
            .await
            .unwrap_or_else(|e| panic!("{cmd}: {e:?}"))
    }

    async fn fail(&self, cmd: &str) -> CoreError {
        self.core.dispatch(cmd, json!({})).await.unwrap_err()
    }

    async fn wait_state(&self, state: &str) -> Value {
        self.sink
            .wait_for(TIMEOUT, |n, v| n == "update-changed" && v["state"] == state)
            .await
            .unwrap_or_else(|| panic!("no {state}: {:?}", self.sink.events()))
            .1
    }
}

#[tokio::test]
async fn without_a_backend_updating_is_unavailable() {
    let h = harness(None).await;
    assert_eq!(
        h.call("update_status", json!({})).await,
        json!({"state": "unavailable"})
    );
    assert_eq!(
        h.call("start_update_checks", json!({})).await,
        json!({"state": "unavailable"})
    );
    h.call("check_for_updates", json!({})).await;
    assert_eq!(
        h.fail("restart_to_update").await.code,
        ErrorCode::BadRequest
    );
    // The commands take no arguments.
    let bad = h
        .core
        .dispatch("update_status", json!({"x": 1}))
        .await
        .unwrap_err();
    assert_eq!(bad.code, ErrorCode::BadRequest);
}

#[tokio::test]
async fn check_now_finds_downloads_and_restarts() {
    let fake = Arc::new(Fake::default());
    *fake.offer.lock().unwrap() = Some("0.4.1".into());
    let h = harness(Some(fake.clone())).await;
    assert_eq!(
        h.call("update_status", json!({})).await,
        json!({"state": "idle"})
    );
    h.call("check_for_updates", json!({})).await;
    let ready = h.wait_state("ready").await;
    assert_eq!(ready["version"], "0.4.1");
    assert_eq!(h.call("update_status", json!({})).await, ready);
    h.call("restart_to_update", json!({})).await;
    assert_eq!(fake.installs.load(Ordering::SeqCst), 1);
    assert_eq!(
        h.call("update_status", json!({})).await["state"],
        "installing"
    );
}

#[tokio::test]
async fn a_copy_that_cannot_update_itself_only_announces() {
    let fake = Arc::new(Fake::default());
    *fake.offer.lock().unwrap() = Some("0.5.0".into());
    *fake.blocker.lock().unwrap() = Some(Blocker::Translocated);
    let h = harness(Some(fake.clone())).await;
    h.call("check_for_updates", json!({})).await;
    let available = h.wait_state("available").await;
    assert_eq!(available["version"], "0.5.0");
    assert_eq!(available["reason"], Blocker::Translocated.reason());
    assert_eq!(
        h.fail("restart_to_update").await.code,
        ErrorCode::BadRequest
    );
    assert_eq!(fake.installs.load(Ordering::SeqCst), 0);
}

/// The setting turns the automatic checks off: started, nothing is asked for; "Check now"
/// still works.
#[tokio::test(start_paused = true)]
async fn automatic_checks_follow_the_setting() {
    let fake = Arc::new(Fake::default());
    let h = harness(Some(fake.clone())).await;
    let settings = h
        .call(
            "update_settings",
            json!({"patch": {"checkForUpdates": false}}),
        )
        .await;
    assert_eq!(settings["checkForUpdates"], false);
    h.call("start_update_checks", json!({})).await;
    tokio::time::sleep(Duration::from_secs(48 * 3600)).await;
    assert_eq!(fake.checks.load(Ordering::SeqCst), 0);
    h.call("check_for_updates", json!({})).await;
    h.wait_state("upToDate").await;
    assert_eq!(fake.checks.load(Ordering::SeqCst), 1);
    // Back to the defaults: on.
    let settings = h.call("restore_default_settings", json!({})).await;
    assert_eq!(settings["checkForUpdates"], true);
}

/// Restarting to update waits for git: refused, with the repository named, while an
/// operation runs there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_is_refused_while_an_operation_runs() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let fake = Arc::new(Fake::default());
    *fake.offer.lock().unwrap() = Some("0.4.1".into());
    let h = harness(Some(fake.clone())).await;
    h.call("check_for_updates", json!({})).await;
    h.wait_state("ready").await;

    let snap = h.call("open_repo", json!({"path": repo.path()})).await;
    let id = snap["repoId"].as_u64().unwrap() as u32;
    let m = h.core.manager().unwrap().clone();
    m.set_op_delay(Duration::from_millis(800));
    let op = tokio::spawn({
        let (m, wt) = (m.clone(), repo.path().to_path_buf());
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
    let refused = h.fail("restart_to_update").await;
    assert_eq!(refused.code, ErrorCode::Busy);
    let name = Path::new(repo.path())
        .file_name()
        .unwrap()
        .to_string_lossy();
    assert_eq!(
        refused.message,
        format!("gg cannot restart while git is working in {name}. Restart once it is done.")
    );
    assert_eq!(fake.installs.load(Ordering::SeqCst), 0);
    assert_eq!(h.call("update_status", json!({})).await["state"], "ready");
    op.await.unwrap().expect("op");
    h.call("restart_to_update", json!({})).await;
    assert_eq!(fake.installs.load(Ordering::SeqCst), 1);
}

#[test]
fn the_version_rule_is_exported() {
    assert!(is_newer("0.4.0", "0.4.1"));
    assert!(!is_newer("0.4.1", "0.4.1"));
}
