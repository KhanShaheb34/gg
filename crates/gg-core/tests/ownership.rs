//! A repository git refuses to open because another user owns it ("dubious ownership",
//! `safe.directory`). Its own test binary: it sets an environment variable for the whole
//! process, which git's children inherit.

use gg_core::error::ErrorCode;
use gg_core::git::runner::{CommandLog, GitRunner};
use gg_core::repo::RepoManager;
use gg_core::testutil::{RecordingSink, TestRepo};
use std::sync::Arc;

#[tokio::test]
async fn dubious_ownership_is_not_called_not_a_repository() {
    let repo = TestRepo::new();
    repo.commit("one");
    let runner = GitRunner::detect(CommandLog::new()).await.unwrap();
    let m = RepoManager::new(runner, Arc::new(RecordingSink::default()));
    // git's own test switch: every repository counts as owned by someone else.
    // SAFETY: the only test in this binary; no other thread reads the environment meanwhile.
    unsafe { std::env::set_var("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1") };

    let err = m.open(repo.path()).await.unwrap_err();
    assert_ne!(err.code, ErrorCode::NotARepo, "{err:?}");
    assert!(err.message.contains("another user"), "{}", err.message);
    assert!(err.message.contains("safe.directory"), "{}", err.message);
    let detail = err.detail.expect("git's text is the detail");
    assert!(detail.contains("dubious ownership"), "{detail}");
    assert!(
        detail.contains("git config --global --add safe.directory"),
        "{detail}"
    );
}
