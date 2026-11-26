//! A stale `index.lock`: an operation that fails on it names it; one older than ten
//! minutes, with no git of the app's running, is offered for removal, which is confirmed in
//! red with a token of the file as seen, removes exactly that file and nothing else, and
//! never a fresh lock or a symlink.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gg_core::dto::{Confirmed, DangerKind, OpHint, OpOutcome, OpRequest, OpStatus};
use gg_core::testutil::TestRepo;

mod common;
use common::*;

/// A checkout of a new branch: it needs the staging area, so its lock.
fn new_branch(at: &str) -> OpRequest {
    OpRequest::CreateBranch {
        name: "topic".into(),
        at: at.into(),
        checkout: true,
        expect_worktree: here_wt(),
    }
}

fn remove_lock() -> OpRequest {
    OpRequest::RemoveIndexLock {
        expect_worktree: here_wt(),
    }
}

/// Makes `index.lock` in `repo`'s git dir, last changed `age` ago.
fn lock(repo: &TestRepo, age: Duration) -> PathBuf {
    let path = repo.path().join(".git").join("index.lock");
    fs::write(&path, "").unwrap();
    let file = fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(SystemTime::now() - age).unwrap();
    path
}

fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap()
}

fn confirmed(out: &OpOutcome) -> Vec<Confirmed> {
    out.dangers.iter().map(Confirmed::from).collect()
}

#[tokio::test]
async fn a_stale_lock_is_named_and_removed_only_once_confirmed() {
    let repo = TestRepo::new();
    let one = repo.commit("one");
    let path = lock(&repo, Duration::from_secs(3 * 3600));
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    // The failure names the lock, its age, and offers to remove it.
    let out = h.run(id, new_branch(&one)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    let shown = canonical(&path);
    assert!(
        out.message.contains("index.lock") && out.message.contains("from 3 h ago"),
        "{}",
        out.message
    );
    assert!(out.message.contains("left behind"), "{}", out.message);
    let worktree = h.m.snapshot(id).await.unwrap().active_worktree;
    let offered = out.hints.iter().find_map(|h| match h {
        OpHint::RemoveIndexLock { path, worktree } => Some((path.clone(), worktree.clone())),
        _ => None,
    });
    let (offered_path, offered_worktree) = offered.expect("offered");
    assert_eq!(canonical(Path::new(&offered_path)), shown);
    assert_eq!(offered_worktree, worktree);
    assert!(path.exists(), "never removed silently");

    // Removing it asks first, in red, naming the file, its age and the risk.
    let out = h.run(id, remove_lock()).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:#?}");
    let [danger] = out.dangers.as_slice() else {
        panic!("{:?}", out.dangers)
    };
    assert_eq!(danger.kind, DangerKind::RemoveIndexLock);
    assert!(danger.message.contains("index.lock"), "{}", danger.message);
    assert!(danger.message.contains("3 h ago"), "{}", danger.message);
    assert!(
        danger.message.contains("another git process or an editor"),
        "{}",
        danger.message
    );
    assert!(path.exists());

    // The file changed since it was confirmed: asked again, nothing removed.
    let seen = confirmed(&out);
    fs::write(&path, "someone").unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(3 * 3600))
        .unwrap();
    let out =
        h.m.run_op(id, h.at_head(id, remove_lock()).await, &seen)
            .await
            .unwrap();
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:#?}");
    assert!(path.exists());

    // Confirmed as it is: exactly that file goes, nothing else.
    let index = repo.path().join(".git").join("index");
    assert!(index.exists());
    let out =
        h.m.run_op(id, h.at_head(id, remove_lock()).await, &confirmed(&out))
            .await
            .unwrap();
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(!path.exists());
    assert!(index.exists(), "the staging area itself stays");
    assert!(
        out.message.contains("Removed the lock file"),
        "{}",
        out.message
    );

    // And what failed works now.
    let out = h.run(id, new_branch(&one)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
}

#[tokio::test]
async fn a_fresh_lock_is_named_but_never_offered_nor_removed() {
    let repo = TestRepo::new();
    let one = repo.commit("one");
    let path = lock(&repo, Duration::from_secs(60));
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, new_branch(&one)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message.contains("Another git process is working") && out.message.contains("1 min ago"),
        "{}",
        out.message
    );
    assert!(
        !out.hints
            .iter()
            .any(|h| matches!(h, OpHint::RemoveIndexLock { .. })),
        "{:?}",
        out.hints
    );
    // Asked for anyway (an old notice): refused, kept.
    let out = h.run(id, remove_lock()).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message.contains("Nothing was removed"),
        "{}",
        out.message
    );
    assert!(path.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn a_lock_that_is_a_symlink_is_never_removed() {
    let repo = TestRepo::new();
    repo.commit("one");
    let elsewhere = tempfile::tempdir().unwrap();
    let target = elsewhere.path().join("precious");
    fs::write(&target, "keep me").unwrap();
    let link = repo.path().join(".git").join("index.lock");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, remove_lock()).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(out.message.contains("symbolic link"), "{}", out.message);
    assert!(fs::symlink_metadata(&link).is_ok());
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep me");
}

/// The lock removed is the active worktree's own (`git rev-parse --git-path index.lock`):
/// a linked worktree's is in its own git dir, and the main one's is left alone.
#[tokio::test]
async fn the_lock_is_the_active_worktrees_own() {
    let repo = TestRepo::new();
    repo.commit("one");
    let linked = repo.add_worktree("side");
    let main_lock = lock(&repo, Duration::from_secs(3600));
    let git_dir = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "--absolute-git-dir"])
            .current_dir(&linked)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let linked_lock = Path::new(git_dir.trim()).join("index.lock");
    fs::write(&linked_lock, "").unwrap();
    fs::File::options()
        .write(true)
        .open(&linked_lock)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_active_worktree(id, &linked).await.unwrap();

    let out = h.run(id, remove_lock()).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:#?}");
    let out =
        h.m.run_op(id, h.at_head(id, remove_lock()).await, &confirmed(&out))
            .await
            .unwrap();
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(!linked_lock.exists());
    assert!(
        main_lock.exists(),
        "the main worktree's lock is not this one"
    );
}

/// A `git commit` waiting in its editor (in a terminal, say) holds `index.lock` for as long
/// as the editor is open, however old the lock gets: it is never offered for removal, and a
/// removal asked for anyway is refused, naming the git process.
/// Where gg can look for running git processes (macOS, Linux).
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn a_lock_held_by_a_running_git_is_never_offered_nor_removed() {
    let repo = TestRepo::new();
    let one = repo.commit("one");
    repo.write("staged.txt", "x\n");
    repo.git(&["add", "staged.txt"]);
    // `git commit -a` in the repository, as from a terminal, waiting in its editor (with
    // `-a` or paths, git holds the staging area's lock until the message is written).
    let mut editor = std::process::Command::new("git")
        .args(["commit", "-a", "-q"])
        .current_dir(repo.path())
        .env("GIT_CONFIG_GLOBAL", gg_core::testutil::empty_git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // git runs it as `<editor> "$@"`: the file is given to `:`.
        .env("GIT_EDITOR", "sleep 30; :")
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let path = repo.path().join(".git").join("index.lock");
    let started = std::time::Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "git took the lock"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // As old as a stale one.
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let pid = editor.id();

    let out = h.run(id, new_branch(&one)).await;
    let offered = out
        .hints
        .iter()
        .any(|h| matches!(h, OpHint::RemoveIndexLock { .. }));
    let asked = h.run(id, remove_lock()).await;
    let _ = editor.kill();
    let _ = editor.wait();

    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(!offered, "{:?}", out.hints);
    assert!(
        out.message.contains(&format!("pid {pid}")),
        "{}",
        out.message
    );
    assert_eq!(asked.status, OpStatus::Failed, "{asked:#?}");
    assert!(
        asked.message.contains(&format!(
            "A git process (pid {pid}) is using this repository; the lock is not stale."
        )),
        "{}",
        asked.message
    );
}
