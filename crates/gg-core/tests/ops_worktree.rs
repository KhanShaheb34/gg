//! Worktree operations (`run_op`): adding a worktree, removing one (Review Focus 1: every
//! kind of work in it either refuses plainly or is listed before a confirmed force, and a
//! state changed since the dialog asks again), pruning stale entries, the default location
//! of a new worktree, and the `make_active` hint on refusals over a branch checked out in
//! another worktree.

use std::fs;
use std::path::{Path, PathBuf};

use gg_core::dto::{
    Confirmed, Danger, DangerKind, OpHint, OpOutcome, OpRequest, OpStatus, WorktreeBranch,
};
use gg_core::error::ErrorCode;
use gg_core::git::runner::CommandRecord;
use gg_core::testutil::{TestRepo, empty_git_config};

mod common;
use common::*;

/// Runs git in `dir` and returns trimmed stdout; panics if git fails.
fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", empty_git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn real(path: &Path) -> String {
    dunce::canonicalize(path)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn remove(path: &Path, delete_branch: bool) -> OpRequest {
    OpRequest::WorktreeRemove {
        path: path.to_string_lossy().into_owned(),
        delete_branch,
    }
}

fn add(path: &str, branch: WorktreeBranch) -> OpRequest {
    OpRequest::WorktreeAdd {
        path: path.to_owned(),
        branch,
    }
}

fn existing(name: &str) -> WorktreeBranch {
    WorktreeBranch::Existing {
        existing: name.to_owned(),
    }
}

/// The `worktree` commands the operation ran (args after `worktree`).
fn worktree_writes(h: &Harness, out: &OpOutcome) -> Vec<Vec<String>> {
    op_records(&h.log, out)
        .into_iter()
        .filter(|r: &CommandRecord| r.args.first().map(String::as_str) == Some("worktree"))
        .filter(|r| r.kind == gg_core::git::runner::CmdKind::Write)
        .map(|r| r.args)
        .collect()
}

fn listed(repo: &TestRepo) -> Vec<String> {
    repo.git(&["worktree", "list", "--porcelain"])
        .lines()
        .filter_map(|l| l.strip_prefix("worktree ").map(str::to_owned))
        .collect()
}

fn confirmed(dangers: &[Danger]) -> Vec<Confirmed> {
    dangers.iter().map(Confirmed::from).collect()
}

/// A repository with one commit pushed to `origin` and a linked worktree on `feature`,
/// pushed too. Returns the repository, its remote (keep it alive) and the worktree.
fn pushed_worktree() -> (TestRepo, TestRepo, PathBuf) {
    let repo = TestRepo::new();
    repo.write(".gitignore", "build/\n*.log\n");
    repo.write("a.txt", "a\n");
    repo.commit_all("one");
    repo.branch("feature");
    let origin = repo.clone_as_remote();
    let wt = repo.add_worktree("feature");
    (repo, origin, wt)
}

/// Asks to remove `wt` (nothing confirmed): the single danger, of `kind`.
async fn asked(h: &Harness, id: u32, wt: &Path, kind: DangerKind) -> Danger {
    let out = h.run(id, remove(wt, false)).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:#?}");
    assert_eq!(out.dangers.len(), 1, "{:#?}", out.dangers);
    assert_eq!(out.dangers[0].kind, kind);
    assert!(wt.is_dir(), "nothing removed yet");
    assert_eq!(out.log_from, 0, "nothing written");
    out.dangers[0].clone()
}

// ---- Removing: Review Focus 1, one test per case ----

#[tokio::test]
async fn a_clean_pushed_worktree_is_removed_without_a_danger() {
    let (repo, _origin, wt) = pushed_worktree();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, remove(&wt, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(out.dangers.is_empty());
    assert_eq!(
        out.message,
        format!("Removed the worktree at {}.", real_gone(&wt))
    );
    assert!(!wt.exists());
    assert_eq!(listed(&repo).len(), 1);
    assert_eq!(
        worktree_writes(&h, &out),
        [vec![
            "worktree".to_owned(),
            "remove".into(),
            "--".into(),
            real_gone(&wt)
        ]],
        "plain remove, no --force"
    );
    assert_eq!(out.snapshot.worktrees.len(), 1);
    assert!(
        repo.git_succeeds(&["rev-parse", "--verify", "-q", "refs/heads/feature"]),
        "its branch stays"
    );
}

/// The path git listed for a worktree that is gone now: its parent resolved.
fn real_gone(wt: &Path) -> String {
    let parent = dunce::canonicalize(wt.parent().unwrap()).unwrap();
    parent
        .join(wt.file_name().unwrap())
        .to_string_lossy()
        .into_owned()
}

#[tokio::test]
async fn uncommitted_changes_are_listed_and_removed_only_with_a_confirmed_force() {
    let (repo, _origin, wt) = pushed_worktree();
    let first = "a.txt";
    fs::write(wt.join(first), "edited\n").unwrap();
    fs::write(wt.join(".gitignore"), "build/\n").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeWithWork).await;
    assert!(
        danger.message.starts_with(&format!(
            "The worktree at {} has uncommitted changes to 2 files.",
            real(&wt)
        )),
        "{}",
        danger.message
    );
    assert!(
        danger
            .message
            .contains("deletes the uncommitted changes for good")
    );
    let detail = danger.detail.as_deref().unwrap();
    assert!(detail.starts_with("Uncommitted changes (2):\n"), "{detail}");
    assert!(detail.contains(&format!("  {first}")), "{detail}");
    assert!(detail.contains("  .gitignore"), "{detail}");

    let out = h
        .run_with(id, remove(&wt, false), &confirmed(&[danger]))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(!wt.exists());
    assert_eq!(
        worktree_writes(&h, &out),
        [vec![
            "worktree".to_owned(),
            "remove".into(),
            "--force".into(),
            "--".into(),
            real_gone(&wt)
        ]],
        "--force once, never twice"
    );
}

#[tokio::test]
async fn untracked_files_alone_are_work_and_listed_with_a_cap() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::create_dir(wt.join("notes")).unwrap();
    for i in 0..25 {
        fs::write(wt.join(format!("notes/n{i:02}.txt")), "x").unwrap();
    }
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeWithWork).await;
    assert!(
        danger.message.contains("has 25 untracked files."),
        "{}",
        danger.message
    );
    assert!(
        danger
            .message
            .contains("deletes the untracked files for good")
    );
    let detail = danger.detail.as_deref().unwrap();
    let lines: Vec<&str> = detail.lines().collect();
    assert_eq!(lines[0], "Untracked files (25):");
    assert_eq!(lines[1], "  notes/n00.txt");
    assert_eq!(lines.len(), 22, "{detail}");
    assert_eq!(lines[21], "  and 5 more");
    let out = h
        .run_with(id, remove(&wt, false), &confirmed(&[danger]))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(!wt.exists());
}

#[tokio::test]
async fn ignored_files_alone_need_the_ordinary_confirmation_that_names_them() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::create_dir_all(wt.join("build/deep")).unwrap();
    fs::write(wt.join("build/deep/out.o"), "x").unwrap();
    fs::write(wt.join("debug.log"), "x").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeIgnoredFiles).await;
    assert_eq!(
        danger.message,
        format!(
            "The worktree at {} has nothing uncommitted, but it holds 2 ignored files and \
             folders, which git deletes with it without asking (listed below).",
            real(&wt)
        )
    );
    assert_eq!(
        danger.detail.as_deref(),
        Some("Ignored, deleted with it (2):\n  build/\n  debug.log")
    );
    let out = h
        .run_with(id, remove(&wt, false), &confirmed(&[danger]))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(!wt.exists());
    assert_eq!(
        worktree_writes(&h, &out)[0],
        ["worktree", "remove", "--", &real_gone(&wt)],
        "git deletes ignored files without --force"
    );
}

#[tokio::test]
async fn ignored_files_beside_other_work_are_a_line_of_the_red_danger() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::create_dir(wt.join("build")).unwrap();
    fs::write(wt.join("build/out.o"), "x").unwrap();
    fs::write(wt.join("new.txt"), "x").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeWithWork).await;
    assert!(
        danger
            .message
            .ends_with("It also holds 1 ignored folder, which git deletes with it (listed below)."),
        "{}",
        danger.message
    );
    assert_eq!(
        danger.detail.as_deref(),
        Some("Untracked files (1):\n  new.txt\n\nIgnored, deleted with it (1):\n  build/")
    );
}

#[tokio::test]
async fn commits_on_no_remote_are_work_and_listed() {
    let (repo, _origin, wt) = pushed_worktree();
    git_in(&wt, &["commit", "-q", "--allow-empty", "-m", "local one"]);
    git_in(&wt, &["commit", "-q", "--allow-empty", "-m", "local two"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeWithWork).await;
    assert!(
        danger.message.contains("has 2 commits on no remote."),
        "{}",
        danger.message
    );
    assert!(
        danger
            .message
            .contains("Branch \u{201c}feature\u{201d} keeps them; they are not pushed anywhere."),
        "{}",
        danger.message
    );
    let detail = danger.detail.as_deref().unwrap();
    let lines: Vec<&str> = detail.lines().collect();
    assert_eq!(lines[0], "Commits on no remote (2):");
    assert!(lines[1].ends_with(" local two"), "{detail}");
    assert!(lines[2].ends_with(" local one"), "{detail}");
    let out = h
        .run_with(id, remove(&wt, false), &confirmed(&[danger]))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    // The branch keeps them.
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s", "feature"]),
        "local two"
    );
}

#[tokio::test]
async fn a_detached_head_with_commits_on_no_branch_lists_what_is_lost() {
    let (repo, _origin, wt) = pushed_worktree();
    git_in(&wt, &["checkout", "-q", "--detach"]);
    git_in(&wt, &["commit", "-q", "--allow-empty", "-m", "only here"]);
    let lost = git_in(&wt, &["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeWithWork).await;
    assert!(
        danger
            .message
            .contains("has 1 commit on no branch or remote."),
        "{}",
        danger.message
    );
    assert!(
        danger.message.contains("lost with it"),
        "{}",
        danger.message
    );
    assert_eq!(
        danger.detail.as_deref(),
        Some(
            format!(
                "Commits on no branch or remote (1):\n  {} only here",
                &lost[..8]
            )
            .as_str()
        )
    );
    // Commits that are on a branch (not pushed) are not lost with a detached worktree.
    let (repo2, _origin2, wt2) = pushed_worktree();
    repo2.git(&["branch", "keep", "feature"]);
    git_in(&wt2, &["checkout", "-q", "--detach"]);
    git_in(&wt2, &["commit", "-q", "--allow-empty", "-m", "kept"]);
    git_in(&wt2, &["branch", "-f", "keep", "HEAD"]);
    let id2 = h.open(repo2.path()).await.repo_id;
    let out = h.run(id2, remove(&wt2, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
}

#[tokio::test]
async fn something_in_progress_there_is_said() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::write(wt.join("clash.txt"), "feature\n").unwrap();
    git_in(&wt, &["add", "clash.txt"]);
    git_in(&wt, &["commit", "-q", "-m", "feature side"]);
    repo.write("clash.txt", "main\n");
    repo.commit_all("main side");
    assert!(
        !std::process::Command::new("git")
            .arg("-C")
            .arg(&wt)
            .args(["merge", "-q", "main"])
            .env("GIT_CONFIG_GLOBAL", empty_git_config())
            .output()
            .unwrap()
            .status
            .success(),
        "the merge stops"
    );
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeWithWork).await;
    assert!(
        danger
            .message
            .contains("A merge is in progress there; removing the worktree abandons it."),
        "{}",
        danger.message
    );
}

#[tokio::test]
async fn stashes_are_shared_and_not_warned_about() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::write(wt.join("stashed.txt"), "x").unwrap();
    git_in(&wt, &["stash", "push", "-q", "-u"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, remove(&wt, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(repo.git_succeeds(&["rev-parse", "--verify", "-q", "refs/stash"]));
}

#[tokio::test]
async fn a_confirmation_for_another_state_asks_again() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::write(wt.join("one.txt"), "x").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeWithWork).await;
    // A file appears between the dialog and the confirmation.
    fs::write(wt.join("two.txt"), "x").unwrap();
    let out = h
        .run_with(
            id,
            remove(&wt, false),
            &confirmed(std::slice::from_ref(&danger)),
        )
        .await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:#?}");
    assert_ne!(out.dangers[0].token, danger.token);
    assert!(out.dangers[0].message.contains("2 untracked files"));
    assert!(wt.join("two.txt").exists(), "nothing removed");
    // HEAD moving changes it too.
    let danger = out.dangers[0].clone();
    git_in(&wt, &["commit", "-q", "--allow-empty", "-m", "moved"]);
    let out = h
        .run_with(id, remove(&wt, false), &confirmed(&[danger]))
        .await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:#?}");
    // Confirmed as it is now, it goes.
    let out = h
        .run_with(id, remove(&wt, false), &confirmed(&out.dangers))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(!wt.exists());
}

#[tokio::test]
async fn an_ignored_file_that_appears_after_the_dialog_asks_again() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::write(wt.join("a.log"), "x").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeIgnoredFiles).await;
    fs::write(wt.join("b.log"), "x").unwrap();
    let out = h
        .run_with(id, remove(&wt, false), &confirmed(&[danger]))
        .await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:#?}");
    assert_eq!(
        out.dangers[0].detail.as_deref(),
        Some("Ignored, deleted with it (2):\n  a.log\n  b.log")
    );
    assert!(wt.join("b.log").exists());
    let _ = repo;
}

#[tokio::test]
async fn a_locked_worktree_is_refused_with_its_reason() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::write(wt.join("x.txt"), "x").unwrap();
    repo.git(&[
        "worktree",
        "lock",
        "--reason",
        "on a usb disk",
        &wt.to_string_lossy(),
    ]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let mark = h.last_log_id();
    // Even with every danger confirmed, as for an unlocked one.
    let out = h
        .run_confirmed(
            id,
            remove(&wt, false),
            &[DangerKind::RemoveWorktreeWithWork],
        )
        .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert_eq!(
        out.message,
        format!(
            "Nothing was removed: the worktree at {} is locked (on a usb disk). git keeps a \
             locked worktree; unlock it first (git worktree unlock) if it may go.",
            real(&wt)
        )
    );
    assert!(h.writes_since(mark).is_empty(), "nothing ran");
    assert!(wt.join("x.txt").exists());
}

#[tokio::test]
async fn the_main_worktree_is_refused() {
    let (repo, _origin, _wt) = pushed_worktree();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let mark = h.last_log_id();
    for path in [
        repo.path().to_path_buf(),
        repo.path().join("."),
        repo.path().join("../repo"),
    ] {
        let out = h.run(id, remove(&path, true)).await;
        assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
        assert!(
            out.message.contains("is the main worktree"),
            "{}",
            out.message
        );
    }
    assert!(h.writes_since(mark).is_empty());
    assert!(repo.path().join(".git").is_dir());
}

#[tokio::test]
async fn a_path_git_does_not_list_is_never_removed() {
    let (repo, _origin, wt) = pushed_worktree();
    let elsewhere = tempfile::tempdir().unwrap();
    fs::write(elsewhere.path().join("keep.txt"), "x").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let mark = h.last_log_id();
    for path in [
        elsewhere.path().to_path_buf(),
        wt.join("sub"),
        repo.path().join(".git"),
        repo.path().join(".git/worktrees/wt-feature"),
    ] {
        let out = h.run(id, remove(&path, false)).await;
        assert_eq!(out.status, OpStatus::Failed, "{path:?}: {out:#?}");
        assert!(
            out.message.contains("is not a worktree of this repository"),
            "{}",
            out.message
        );
    }
    assert!(h.writes_since(mark).is_empty());
    assert!(elsewhere.path().join("keep.txt").exists());
    assert!(wt.is_dir());
}

#[tokio::test]
async fn the_active_worktree_is_removed_after_main_becomes_active() {
    let (repo, _origin, wt) = pushed_worktree();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_active_worktree(id, &wt).await.unwrap();
    let out = h.run(id, remove(&wt, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(
        out.message,
        format!(
            "Removed the worktree at {}. The main worktree, {}, is active now.",
            real_gone(&wt),
            real(repo.path())
        )
    );
    assert_eq!(out.snapshot.active_worktree, real(repo.path()));
    assert_eq!(out.snapshot.head.branch.as_deref(), Some("main"));
    assert!(!wt.exists());
    // Main is active when the folder goes: it was never the folder commands ran in then.
    let records = op_records(&h.log, &out);
    let removal = records
        .iter()
        .position(|r| {
            r.args.first().is_some_and(|a| a == "worktree")
                && r.args.get(1).is_some_and(|a| a == "remove")
        })
        .unwrap();
    assert!(
        records[removal..]
            .iter()
            .all(|r| r.cwd == real(repo.path()))
    );
    // No "gone" notice: the app moved away itself.
    let gone: Vec<_> = h
        .sink
        .events()
        .into_iter()
        .filter(|(n, v)| n == "repo-error" && v["repoId"] == id)
        .collect();
    assert!(gone.is_empty(), "{gone:?}");
}

#[tokio::test]
async fn an_active_worktree_that_git_will_not_remove_says_main_is_active_now() {
    // A submodule checked out there: git refuses even a clean one.
    let sub = TestRepo::new();
    sub.commit("sub one");
    let (repo, _origin, wt) = pushed_worktree();
    git_in(
        &wt,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &sub.path().to_string_lossy(),
            "lib",
        ],
    );
    git_in(&wt, &["commit", "-q", "-m", "add the submodule"]);
    git_in(&wt, &["push", "-q", "origin", "feature"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_active_worktree(id, &wt).await.unwrap();
    let out = h.run(id, remove(&wt, false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message
            .starts_with("git does not remove a worktree that contains submodules"),
        "{}",
        out.message
    );
    assert!(
        out.message.ends_with(&format!(
            "The main worktree, {}, is active now.",
            real(repo.path())
        )),
        "{}",
        out.message
    );
    assert!(
        out.detail
            .as_deref()
            .unwrap()
            .contains("working trees containing submodules cannot be moved or removed")
    );
    assert_eq!(out.snapshot.active_worktree, real(repo.path()));
    assert!(wt.join("lib/.git").exists(), "nothing removed");
    assert!(
        worktree_writes(&h, &out)
            .iter()
            .all(|args| !args.contains(&"--force".to_owned())),
        "never forced"
    );
}

#[tokio::test]
async fn a_missing_folder_suggests_pruning() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::remove_dir_all(&wt).unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let mark = h.last_log_id();
    let out = h.run(id, remove(&wt, false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(out.message.contains("is missing"), "{}", out.message);
    assert!(
        out.message.contains("Prune stale entries"),
        "{}",
        out.message
    );
    assert!(h.writes_since(mark).is_empty());
    assert_eq!(listed(&repo).len(), 2, "still listed");
}

/// Asks to remove `wt` (everything confirmed that is asked about) and checks it is refused
/// with a message containing `says`, with nothing written and `inner` still there.
async fn refused_for(h: &Harness, id: u32, wt: &Path, inner: &Path, says: &str) {
    let mark = h.last_log_id();
    let out = h
        .run_confirmed(
            id,
            remove(wt, false),
            &[
                DangerKind::RemoveWorktreeWithWork,
                DangerKind::RemoveWorktreeIgnoredFiles,
            ],
        )
        .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(out.message.contains(says), "{}", out.message);
    assert!(h.writes_since(mark).is_empty(), "nothing ran");
    assert!(wt.is_dir());
    assert!(inner.join(".git").exists(), "{} is kept", inner.display());
}

/// Critical (Task 4 review): `git worktree remove` deletes whatever is inside the folder,
/// another worktree included; one in an ignored folder showed only as `! build/`.
#[tokio::test]
async fn a_worktree_inside_an_ignored_folder_is_never_removed_with_it() {
    let (repo, _origin, wt) = pushed_worktree();
    let inner = wt.join("build/nested");
    repo.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "inner",
        &inner.to_string_lossy(),
    ]);
    fs::write(inner.join("work.txt"), "uncommitted\n").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let says = format!(
        "{} contains the worktree at {}. Remove that one first.",
        real(&wt),
        real(&inner)
    );
    refused_for(&h, id, &wt, &inner, &says).await;
    assert!(inner.join("work.txt").exists());
}

#[tokio::test]
async fn a_worktree_inside_in_an_untracked_folder_is_never_removed_with_it() {
    let (repo, _origin, wt) = pushed_worktree();
    let inner = wt.join("nested");
    repo.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "inner",
        &inner.to_string_lossy(),
    ]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let says = format!("contains the worktree at {}", real(&inner));
    refused_for(&h, id, &wt, &inner, &says).await;
}

#[tokio::test]
async fn another_repository_inside_is_never_removed_with_it() {
    let (repo, _origin, wt) = pushed_worktree();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    // One level inside an ignored folder (git shows only `! build/`).
    let deep = wt.join("build/other");
    fs::create_dir_all(&deep).unwrap();
    git_in(&deep, &["init", "-q"]);
    fs::write(deep.join("mine.txt"), "x").unwrap();
    let says = format!(
        "{} contains another repository at {}, which git would delete with it. Move or \
         remove it first.",
        real(&wt),
        real(&deep)
    );
    refused_for(&h, id, &wt, &deep, &says).await;
    fs::remove_dir_all(wt.join("build")).unwrap();
    // An ignored folder that is a repository itself.
    fs::write(wt.join(".gitignore"), "build/\n*.log\nvendored/\n").unwrap();
    git_in(&wt, &["commit", "-q", "-am", "ignore vendored"]);
    git_in(&wt, &["push", "-q", "origin", "feature"]);
    let vendored = wt.join("vendored");
    fs::create_dir(&vendored).unwrap();
    git_in(&vendored, &["init", "-q"]);
    refused_for(&h, id, &wt, &vendored, "contains another repository at").await;
    fs::remove_dir_all(&vendored).unwrap();
    // An untracked folder that is a repository.
    let plain = wt.join("plain");
    fs::create_dir(&plain).unwrap();
    git_in(&plain, &["init", "-q"]);
    refused_for(&h, id, &wt, &plain, "contains another repository at").await;
}

// ---- Removing with the branch ----

#[tokio::test]
async fn deleting_a_merged_branch_with_its_worktree_needs_no_confirmation() {
    let (repo, _origin, wt) = pushed_worktree();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, remove(&wt, true)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(
        out.message.ends_with("Deleted branch feature."),
        "{}",
        out.message
    );
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "-q", "refs/heads/feature"]));
    let branch_deletes: Vec<_> = op_records(&h.log, &out)
        .into_iter()
        .filter(|r| r.args.first().is_some_and(|a| a == "branch"))
        .map(|r| r.args)
        .collect();
    assert_eq!(branch_deletes, [vec!["branch", "-d", "--", "feature"]]);
}

#[tokio::test]
async fn an_unmerged_unpushed_branch_and_the_work_are_asked_about_together() {
    let (repo, _origin, wt) = pushed_worktree();
    git_in(&wt, &["commit", "-q", "--allow-empty", "-m", "local"]);
    fs::write(wt.join("new.txt"), "x").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, remove(&wt, true)).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:#?}");
    let kinds: Vec<DangerKind> = out.dangers.iter().map(|d| d.kind).collect();
    assert_eq!(
        kinds,
        [
            DangerKind::RemoveWorktreeWithWork,
            DangerKind::DeleteUnmergedBranchWithWorktree
        ],
        "both in one round"
    );
    let branch = &out.dangers[1];
    assert!(
        branch
            .message
            .starts_with("Branch \u{201c}feature\u{201d} has 1 commit"),
        "{}",
        branch.message
    );
    assert!(
        branch
            .message
            .ends_with("1 of its commits is not pushed anywhere (it is on no remote)."),
        "{}",
        branch.message
    );
    assert_eq!(
        branch.token.as_deref(),
        Some(git_in(&wt, &["rev-parse", "HEAD"]).as_str()),
        "R9: the branch's tip"
    );
    assert!(out.dangers[0].message.contains("asked about separately"));
    // Only one of the two confirmed: asked again, nothing removed.
    let half = h
        .run_with(id, remove(&wt, true), &confirmed(&out.dangers[..1]))
        .await;
    assert_eq!(half.status, OpStatus::NeedsConfirmation);
    assert!(wt.is_dir());
    let done = h
        .run_with(id, remove(&wt, true), &confirmed(&out.dangers))
        .await;
    assert_eq!(done.status, OpStatus::Done, "{done:#?}");
    assert!(!wt.exists());
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "-q", "refs/heads/feature"]));
    let writes: Vec<Vec<String>> = op_records(&h.log, &done)
        .into_iter()
        .filter(|r| r.kind == gg_core::git::runner::CmdKind::Write)
        .map(|r| r.args)
        .collect();
    assert_eq!(writes.len(), 2, "{writes:?}");
    assert_eq!(writes[0][..3], ["worktree", "remove", "--force"]);
    assert_eq!(
        writes[1],
        ["branch", "-D", "--", "feature"],
        "after the removal"
    );
}

#[tokio::test]
async fn a_branch_checked_out_in_yet_another_worktree_is_kept_and_said_so() {
    let (repo, _origin, wt) = pushed_worktree();
    let other = repo.dir.path().join("other");
    repo.git(&[
        "worktree",
        "add",
        "-q",
        "--force",
        &other.to_string_lossy(),
        "feature",
    ]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, remove(&wt, true)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(
        out.message,
        format!(
            "Removed the worktree at {}. Branch \u{201c}feature\u{201d} was not deleted: it is \
             checked out in the worktree at {} too.",
            real_gone(&wt),
            real(&other)
        )
    );
    assert_eq!(
        out.hints,
        [OpHint::MakeActive {
            worktree: real(&other)
        }]
    );
    assert!(repo.git_succeeds(&["rev-parse", "--verify", "-q", "refs/heads/feature"]));
}

#[tokio::test]
async fn deleting_the_branch_of_a_detached_worktree_is_refused() {
    let (repo, _origin, wt) = pushed_worktree();
    git_in(&wt, &["checkout", "-q", "--detach"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, remove(&wt, true)).await;
    assert_eq!(out.status, OpStatus::Failed);
    assert!(
        out.message.contains("no branch to delete"),
        "{}",
        out.message
    );
    assert!(wt.is_dir());
}

// ---- Pruning ----

#[tokio::test]
async fn prune_removes_only_what_git_calls_prunable_and_names_it() {
    let repo = TestRepo::new();
    repo.commit("one");
    let gone = repo.add_worktree("gone");
    let locked = repo.add_worktree("locked");
    let kept = repo.add_worktree("kept");
    repo.git(&["worktree", "lock", &locked.to_string_lossy()]);
    let gone_listed = real(&gone);
    let locked_listed = real(&locked);
    fs::remove_dir_all(&gone).unwrap();
    fs::remove_dir_all(&locked).unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, OpRequest::WorktreePrune {}).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(out.title, "Prune stale worktree entries");
    assert_eq!(
        out.message,
        format!(
            "Pruned 1 stale worktree entry, whose folders were gone: {gone_listed}. 1 missing \
             worktree is locked, so git keeps it."
        )
    );
    let now = listed(&repo);
    assert!(!now.contains(&gone_listed));
    assert!(now.contains(&locked_listed));
    assert!(now.contains(&real(&kept)));
    assert!(kept.is_dir());
    assert_eq!(out.snapshot.worktrees.len(), 3);
    // Nothing left to prune.
    let again = h.run(id, OpRequest::WorktreePrune {}).await;
    assert!(
        again.message.starts_with("Nothing to prune"),
        "{}",
        again.message
    );
}

// ---- Adding ----

#[tokio::test]
async fn a_worktree_is_added_for_an_existing_branch_and_not_made_active() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("feature/x");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    // Relative to the main worktree; missing parents are made.
    let out = h
        .run(
            id,
            add("../repo.worktrees/deep/feature-x", existing("feature/x")),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    let at = real(&repo.dir.path().join("repo.worktrees/deep/feature-x"));
    assert_eq!(
        out.message,
        format!("Created a worktree at {at}, on branch feature/x.")
    );
    assert_eq!(
        git_in(Path::new(&at), &["branch", "--show-current"]),
        "feature/x"
    );
    assert_eq!(
        out.snapshot.active_worktree,
        real(repo.path()),
        "not made active"
    );
    assert!(
        out.snapshot
            .worktrees
            .iter()
            .any(|w| w.path == at && !w.active)
    );
    let writes = worktree_writes(&h, &out);
    assert_eq!(writes, [vec!["worktree", "add", "--", &at, "feature/x"]]);
    assert!(
        op_records(&h.log, &out)
            .iter()
            .all(|r| r.cwd == real(repo.path())),
        "run from the main worktree"
    );
}

#[tokio::test]
async fn a_worktree_is_added_with_a_new_branch_or_detached() {
    let repo = TestRepo::new();
    let first = repo.commit("one");
    repo.commit("two");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let base = repo.dir.path().to_string_lossy().into_owned();
    let out = h
        .run(
            id,
            add(
                &format!("{base}/new"),
                WorktreeBranch::Create {
                    create: "topic".into(),
                    at: first.clone(),
                },
            ),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(
        out.message.contains("on a new branch topic, at"),
        "{}",
        out.message
    );
    assert_eq!(repo.git(&["rev-parse", "topic"]), first);
    let out = h
        .run(
            id,
            add(
                &format!("{base}/detached"),
                WorktreeBranch::Detach {
                    detach: first.clone(),
                },
            ),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(out.message.contains("detached"), "{}", out.message);
    let detached = Path::new(&base).join("detached");
    assert_eq!(git_in(&detached, &["rev-parse", "HEAD"]), first);
    assert_eq!(git_in(&detached, &["branch", "--show-current"]), "");
    // An empty folder is fine.
    fs::create_dir(Path::new(&base).join("empty")).unwrap();
    let out = h
        .run(id, add(&format!("{base}/empty"), existing("main")))
        .await;
    assert_eq!(
        out.status,
        OpStatus::Failed,
        "main is checked out in the main worktree"
    );
    let out = h
        .run(
            id,
            add(
                &format!("{base}/empty"),
                WorktreeBranch::Create {
                    create: "side".into(),
                    at: first,
                },
            ),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
}

#[tokio::test]
async fn adding_refuses_a_bad_place_or_name() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    repo.branch("free");
    let linked = repo.add_worktree("linked");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let base = repo.dir.path();
    let full = fs::create_dir(base.join("full")).and_then(|()| fs::write(base.join("full/x"), "x"));
    full.unwrap();
    let mark = h.last_log_id();
    let refused = [
        (base.join("full"), "is not empty"),
        (repo.path().join("inside"), "is inside the worktree at"),
        (linked.join("deeper/still"), "is inside the worktree at"),
        (linked.clone(), "there is a worktree at"),
        (base.join("full/x"), "is not a folder"),
    ];
    for (path, why) in refused {
        let out = h
            .run(id, add(&path.to_string_lossy(), existing("free")))
            .await;
        assert_eq!(out.status, OpStatus::Failed, "{path:?}: {out:#?}");
        assert!(out.message.contains(why), "{path:?}: {}", out.message);
        assert!(
            out.message.starts_with("Nothing was created"),
            "{}",
            out.message
        );
    }
    // Inside the git dir (outside the main worktree only for a separate git dir; here it
    // is inside main too, and refused as such or as the git folder).
    let out = h
        .run(
            id,
            add(
                &repo.path().join(".git/wt").to_string_lossy(),
                existing("free"),
            ),
        )
        .await;
    assert_eq!(out.status, OpStatus::Failed);
    let out = h
        .run(id, add(&base.join("x").to_string_lossy(), existing("nope")))
        .await;
    assert!(
        out.message.contains("no local branch named"),
        "{}",
        out.message
    );
    let out = h
        .run(
            id,
            add(
                &base.join("y").to_string_lossy(),
                WorktreeBranch::Create {
                    create: "free".into(),
                    at: head.clone(),
                },
            ),
        )
        .await;
    assert!(out.message.contains("already exists"), "{}", out.message);
    assert!(h.writes_since(mark).is_empty(), "nothing ran");
    assert!(!base.join("x").exists() && !base.join("y").exists());
    // Bad names and arguments never reach the queue.
    for (request, code) in [
        (
            add(
                &base.join("z").to_string_lossy(),
                WorktreeBranch::Create {
                    create: "bad name".into(),
                    at: head.clone(),
                },
            ),
            ErrorCode::BadName,
        ),
        (
            add(&base.join("z").to_string_lossy(), existing("-x")),
            ErrorCode::BadRequest,
        ),
        (
            add(
                &base.join("z").to_string_lossy(),
                WorktreeBranch::Detach {
                    detach: "HEAD".into(),
                },
            ),
            ErrorCode::BadRequest,
        ),
        (add(" ", existing("free")), ErrorCode::BadRequest),
    ] {
        let err = h.m.run_op(id, request, &[]).await.unwrap_err();
        assert_eq!(err.code, code, "{}", err.message);
    }
}

#[tokio::test]
async fn adding_a_branch_checked_out_elsewhere_says_where_and_offers_to_make_it_active() {
    let repo = TestRepo::new();
    repo.commit("one");
    let feature = repo.add_worktree("feature");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let target = repo.dir.path().join("second");
    let out = h
        .run(id, add(&target.to_string_lossy(), existing("feature")))
        .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message.contains(&format!(
            "checked out in another worktree, at {}",
            real(&feature)
        )),
        "{}",
        out.message
    );
    assert_eq!(
        out.hints,
        [OpHint::MakeActive {
            worktree: real(&feature)
        }]
    );
    assert!(!target.exists(), "nothing half-made left");
    // Checked out in the active worktree: no hint to make it active.
    let out = h
        .run(id, add(&target.to_string_lossy(), existing("main")))
        .await;
    assert_eq!(out.status, OpStatus::Failed);
    assert!(out.hints.is_empty(), "{:?}", out.hints);
}

#[tokio::test]
async fn a_failing_post_checkout_hook_is_reported_with_the_worktree_it_left() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("feature");
    let hooks = repo.path().join(".git/hooks");
    fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("post-checkout");
    fs::write(&hook, "#!/bin/sh\necho hook ran in \"$PWD\" >&2\nexit 3\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let target = repo.dir.path().join("hooked");
    let out = h
        .run(id, add(&target.to_string_lossy(), existing("feature")))
        .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message
            .starts_with(&format!("The worktree at {} was created", real(&target))),
        "{}",
        out.message
    );
    assert!(out.detail.as_deref().unwrap().contains("hook ran in"));
    assert!(target.join(".git").exists(), "left as git made it");
}

// ---- The default location ----

#[tokio::test]
async fn the_default_location_follows_the_pattern_and_skips_taken_folders() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let pattern = "../{repo}.worktrees/{branch}";
    let base = real(repo.dir.path());
    let path =
        h.m.worktree_default_path(id, "feature/x:y", pattern)
            .await
            .unwrap();
    assert_eq!(path.path, format!("{base}/repo.worktrees/feature-xy"));
    assert!(!path.exists);
    // A non-empty folder there: -2. An empty one is usable and said to exist.
    fs::create_dir_all(format!("{base}/repo.worktrees/feature-xy")).unwrap();
    fs::write(format!("{base}/repo.worktrees/feature-xy/f"), "x").unwrap();
    fs::create_dir_all(format!("{base}/repo.worktrees/feature-xy-2")).unwrap();
    let path =
        h.m.worktree_default_path(id, "feature/x:y", pattern)
            .await
            .unwrap();
    assert_eq!(path.path, format!("{base}/repo.worktrees/feature-xy-2"));
    assert!(path.exists);
    // A worktree git lists (its folder gone) is taken too.
    let wt = repo.add_worktree("main2");
    let listed_at = format!("{base}/repo.worktrees/main2");
    repo.git(&["worktree", "move", &wt.to_string_lossy(), &listed_at]);
    fs::remove_dir_all(&listed_at).unwrap();
    let path =
        h.m.worktree_default_path(id, "main2", pattern)
            .await
            .unwrap();
    assert_eq!(path.path, format!("{base}/repo.worktrees/main2-2"));
    // Absolute patterns, and through dispatch-free settings: an odd branch.
    let path =
        h.m.worktree_default_path(id, "@@", &format!("{base}/flat/{{repo}}-{{branch}}"))
            .await
            .unwrap();
    assert_eq!(path.path, format!("{base}/flat/repo-worktree"));
}

// ---- Branches checked out elsewhere offer that worktree ----

#[tokio::test]
async fn refusals_over_a_branch_checked_out_elsewhere_offer_to_make_it_active() {
    let repo = TestRepo::new();
    repo.commit("one");
    let feature = repo.add_worktree("feature");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let hint = [OpHint::MakeActive {
        worktree: real(&feature),
    }];
    let out = h.run(id, checkout_branch("feature")).await;
    assert_eq!(out.status, OpStatus::Failed);
    assert_eq!(out.hints, hint, "{}", out.message);
    let out = h.run(id, delete_branch("feature")).await;
    assert_eq!(out.status, OpStatus::Failed);
    assert!(
        out.message.contains("checked out in the worktree at"),
        "{}",
        out.message
    );
    assert_eq!(out.hints, hint);
}
