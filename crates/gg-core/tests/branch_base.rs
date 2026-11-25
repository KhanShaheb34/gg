//! The base of a branch (`repo/base.rs`), each step of its order on real throwaway
//! repositories, and the pull request form's suggestion and "Committed on branch" agreeing on
//! it. The remote is a local bare repository (not GitHub): gh is never asked.

mod common;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Harness, TIMEOUT, harness};
use gg_core::testutil::TestRepo;

/// `main` (one file) and `staging` (two commits more), both on `origin`, whose HEAD names
/// `main`. Checked out: main.
fn with_staging() -> (TestRepo, TestRepo) {
    let repo = TestRepo::new();
    repo.write("app.txt", "app\n");
    repo.commit_all("start");
    repo.git(&["branch", "staging"]);
    repo.checkout("staging");
    repo.write("s1.txt", "1\n");
    repo.commit_all("staging one");
    repo.write("s2.txt", "2\n");
    repo.commit_all("staging two");
    repo.checkout("main");
    let origin = repo.clone_as_remote();
    repo.git(&["remote", "set-head", "origin", "main"]);
    (repo, origin)
}

/// One commit on the branch checked out, in `dir`.
fn work(dir: &Path, file: &str) {
    std::fs::write(dir.join(file), "w\n").unwrap();
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", gg_core::testutil::empty_git_config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["add", file]);
    run(&["commit", "-q", "-m", file]);
}

async fn settle(h: &Harness, id: u32) {
    h.m.refresh(id).await.expect("refresh");
    tokio::time::timeout(TIMEOUT, h.m.wait_loaded(id))
        .await
        .expect("loaded");
    tokio::time::timeout(TIMEOUT, h.m.wait_status(id))
        .await
        .expect("status");
    tokio::time::timeout(TIMEOUT, h.m.wait_background(id))
        .await
        .expect("background");
}

async fn base(h: &Harness, id: u32, branch: &str) -> (String, Option<&'static str>) {
    h.m.branch_base_for_tests(id, branch, "origin")
        .await
        .unwrap()
}

/// What the form suggests and what Committed on branch compares with, for the branch checked
/// out in `dir`.
async fn both(h: &Harness, id: u32, branch: &str, dir: &Path) -> (String, Option<String>) {
    let suggested = h.m.pr_suggest(id, branch).await.unwrap().base;
    let compared = h.m.branch_files(id, dir).await.unwrap().map(|f| f.base);
    (suggested, compared)
}

#[tokio::test]
async fn created_from_a_remote_branch() {
    let (repo, _origin) = with_staging();
    repo.git(&["branch", "topic", "origin/staging"]);
    repo.checkout("topic");
    work(repo.path(), "t.txt");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(
        base(&h, id, "topic").await,
        ("staging".into(), Some("created_from"))
    );
    assert_eq!(
        both(&h, id, "topic", repo.path()).await,
        ("staging".into(), Some("origin/staging".into()))
    );
}

#[tokio::test]
async fn switch_c_from_a_local_branch_by_heads_reflog() {
    let (repo, _origin) = with_staging();
    repo.checkout("staging");
    repo.git(&["switch", "-q", "-c", "topic"]);
    work(repo.path(), "t.txt");
    assert_eq!(
        repo.git(&["reflog", "show", "--format=%gs", "refs/heads/topic"])
            .lines()
            .last(),
        Some("branch: Created from HEAD")
    );
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(
        base(&h, id, "topic").await,
        ("staging".into(), Some("created_from"))
    );
    assert_eq!(
        both(&h, id, "topic", repo.path()).await,
        ("staging".into(), Some("origin/staging".into()))
    );
}

/// How gg itself makes a branch: `checkout -b <name> <commit>` (the picker's Create new
/// branch, at HEAD's commit): the branch's reflog names the commit only; HEAD's says where
/// it moved from.
#[tokio::test]
async fn a_branch_gg_made_at_heads_commit() {
    let (repo, _origin) = with_staging();
    repo.checkout("staging");
    let at = repo.git(&["rev-parse", "HEAD"]);
    repo.git(&["checkout", "-q", "-b", "topic", &at]);
    work(repo.path(), "t.txt");
    assert_eq!(
        repo.git(&["reflog", "show", "--format=%gs", "refs/heads/topic"])
            .lines()
            .last(),
        Some(format!("branch: Created from {at}").as_str())
    );
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(
        base(&h, id, "topic").await,
        ("staging".into(), Some("created_from"))
    );
}

#[tokio::test]
async fn made_in_a_linked_worktree_by_its_own_heads_reflog() {
    let (repo, _origin) = with_staging();
    let wt = repo.dir.path().join("wt-staging");
    repo.git(&["worktree", "add", "-q", wt.to_str().unwrap(), "staging"]);
    let run = |args: &[&str]| {
        let mut all = vec!["-C", wt.to_str().unwrap()];
        all.extend_from_slice(args);
        repo.git(&all)
    };
    run(&["switch", "-q", "-c", "topic"]);
    work(&wt, "t.txt");
    // The main worktree's HEAD never went there.
    assert!(
        !repo
            .git(&["reflog", "show", "--format=%gs", "HEAD"])
            .contains("to topic")
    );
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(
        base(&h, id, "topic").await,
        ("staging".into(), Some("created_from"))
    );
    assert_eq!(
        h.m.branch_files(id, &wt).await.unwrap().map(|f| f.base),
        Some("origin/staging".into())
    );
}

/// No reflog to go by (made with reflogs off, as a fresh clone or a fetched branch has
/// none): the nearest branch on the remote by history: staging, one commit away, not main,
/// three away.
#[tokio::test]
async fn no_record_the_nearest_by_history() {
    let (repo, _origin) = with_staging();
    repo.git(&[
        "-c",
        "core.logAllRefUpdates=false",
        "branch",
        "topic",
        "origin/staging",
    ]);
    repo.checkout("topic");
    work(repo.path(), "t.txt");
    assert!(
        !repo
            .git(&["reflog", "show", "--format=%gs", "refs/heads/topic"])
            .contains("Created from")
    );
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(
        base(&h, id, "topic").await,
        ("staging".into(), Some("nearest"))
    );
    assert_eq!(
        both(&h, id, "topic", repo.path()).await,
        ("staging".into(), Some("origin/staging".into()))
    );
    // Given no time to compare: the remote's HEAD.
    h.m.set_nearest_limit(Duration::ZERO);
    work(repo.path(), "u.txt");
    settle(&h, id).await;
    assert_eq!(base(&h, id, "topic").await, ("main".into(), None));
}

/// A tie by history goes to the default branch: `other` is at main's commit (and newer by
/// nothing), the branch one commit past both.
#[tokio::test]
async fn a_tie_goes_to_the_default_branch() {
    let (repo, _origin) = with_staging();
    repo.git(&["branch", "other", "main"]);
    repo.git(&["push", "-q", "origin", "other"]);
    repo.git(&["fetch", "-q", "origin"]);
    repo.git(&[
        "-c",
        "core.logAllRefUpdates=false",
        "branch",
        "topic",
        "main",
    ]);
    repo.checkout("topic");
    work(repo.path(), "t.txt");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(
        base(&h, id, "topic").await,
        ("main".into(), Some("nearest"))
    );
}

/// What the reflog names counts only when the remote has that branch: made from a branch
/// never pushed, the nearest by history decides.
#[tokio::test]
async fn a_record_naming_a_branch_not_on_the_remote_falls_through() {
    let (repo, _origin) = with_staging();
    repo.git(&["branch", "local-only", "origin/staging"]);
    repo.git(&["branch", "topic", "local-only"]);
    assert_eq!(
        repo.git(&["reflog", "show", "--format=%gs", "refs/heads/topic"])
            .lines()
            .last(),
        Some("branch: Created from local-only")
    );
    repo.checkout("topic");
    work(repo.path(), "t.txt");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(
        base(&h, id, "topic").await,
        ("staging".into(), Some("nearest"))
    );
    // Once pushed, it counts.
    repo.git(&["push", "-q", "origin", "local-only"]);
    repo.git(&["fetch", "-q", "origin"]);
    settle(&h, id).await;
    assert_eq!(
        base(&h, id, "topic").await,
        ("local-only".into(), Some("created_from"))
    );
}

/// The base remembered (the user's last pull request from it went there) is that branch's
/// only: another branch keeps its own.
#[derive(Default)]
struct Memory(Mutex<HashMap<String, String>>);

impl gg_core::repo::WorktreeMemory for Memory {
    fn remembered(&self, _root: &Path) -> Option<std::path::PathBuf> {
        None
    }
    fn remember(&self, _root: &Path, _worktree: Option<&Path>) {}
    fn pr_base(&self, _root: &Path, branch: &str) -> Option<String> {
        self.0.lock().unwrap().get(branch).cloned()
    }
}

#[tokio::test]
async fn a_remembered_base_is_its_branchs_own() {
    let (repo, _origin) = with_staging();
    for b in ["picked", "plain"] {
        repo.checkout("main");
        repo.git(&["switch", "-q", "-c", b]);
        work(repo.path(), &format!("{b}.txt"));
    }
    let memory = Arc::new(Memory::default());
    memory
        .0
        .lock()
        .unwrap()
        .insert("picked".into(), "staging".into());
    let h = harness().await;
    h.m.set_worktree_memory(memory.clone());
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(
        base(&h, id, "picked").await,
        ("staging".into(), Some("remembered"))
    );
    assert_eq!(
        base(&h, id, "plain").await,
        ("main".into(), Some("created_from"))
    );
    // Remembered, but no longer on the remote: the next step.
    memory
        .0
        .lock()
        .unwrap()
        .insert("picked".into(), "gone".into());
    assert_eq!(
        base(&h, id, "picked").await,
        ("main".into(), Some("created_from"))
    );
}

/// Read-only: working the base out writes nothing (reads only), and is not done again while
/// nothing it depends on moved.
#[tokio::test]
async fn read_only_and_kept_while_nothing_moved() {
    let (repo, _origin) = with_staging();
    repo.git(&[
        "-c",
        "core.logAllRefUpdates=false",
        "branch",
        "topic",
        "origin/staging",
    ]);
    repo.checkout("topic");
    work(repo.path(), "t.txt");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let since = h
        .log
        .query(gg_core::git::runner::LogFilter {
            since_id: 0,
            repo_id: None,
            writes_only: false,
        })
        .last()
        .map_or(0, |r| r.id);
    base(&h, id, "topic").await;
    let ran = h.log.query(gg_core::git::runner::LogFilter {
        since_id: since,
        repo_id: None,
        writes_only: false,
    });
    assert!(
        ran.iter()
            .any(|r| r.args.first().map(String::as_str) == Some("rev-list"))
    );
    assert!(
        ran.iter()
            .all(|r| r.kind == gg_core::git::runner::CmdKind::Read),
        "{ran:?}"
    );
    let last = ran.last().unwrap().id;
    base(&h, id, "topic").await;
    let again = h.log.query(gg_core::git::runner::LogFilter {
        since_id: last,
        repo_id: None,
        writes_only: false,
    });
    assert!(
        again
            .iter()
            .all(|r| r.args.first().map(String::as_str) != Some("rev-list")),
        "{again:?}"
    );
}
