//! Worktree operations (`run_op`): adding a worktree, removing one (Review Focus 1: every
//! kind of work in it either refuses plainly or is listed before a confirmed force, and a
//! state changed since the dialog asks again), pruning stale entries, the default location
//! of a new worktree, and the `make_active` hint on refusals over a branch checked out in
//! another worktree.

use std::fs;
use std::path::{Path, PathBuf};

use gg_core::dto::{
    Confirmed, Danger, DangerKind, InputField, OpHint, OpOutcome, OpRequest, OpStatus,
    WorktreeBranch,
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
        .filter_map(|l| l.strip_prefix("worktree ").map(native))
        .collect()
}

/// `path` written with `/` (as git for Windows writes paths, or as a test does), spelled as
/// the platform does.
fn native(path: &str) -> String {
    if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.to_owned()
    }
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
        format!("Deleted the worktree at {}.", real_gone(&wt))
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
            .contains("loses the uncommitted changes for good")
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
            .contains("loses the untracked files for good")
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

/// Ignored files inside a folder git does not ignore whole are listed by that folder, with
/// a count, not one by one (Task 4 review).
#[tokio::test]
async fn ignored_files_are_listed_by_their_top_level_folder() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::create_dir_all(wt.join("newdir/sub")).unwrap();
    fs::write(wt.join("newdir/i.log"), "x").unwrap();
    fs::write(wt.join("newdir/sub/j.log"), "x").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeIgnoredFiles).await;
    assert!(
        danger.message.contains("it holds 1 ignored folder,"),
        "{}",
        danger.message
    );
    assert_eq!(
        danger.detail.as_deref(),
        Some("Ignored, deleted with it (1):\n  newdir/ (2 ignored inside)")
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

/// Removing a worktree never deletes its branch: commits on no remote stay on it, so they
/// are no danger, only a line in a dialog shown for something else (Task 4 review).
#[tokio::test]
async fn unpushed_commits_on_a_branch_are_kept_by_it_and_need_no_confirmation() {
    let (repo, _origin, wt) = pushed_worktree();
    git_in(&wt, &["commit", "-q", "--allow-empty", "-m", "local one"]);
    git_in(&wt, &["commit", "-q", "--allow-empty", "-m", "local two"]);
    // With something else to confirm, they are a line of that dialog.
    fs::write(wt.join("new.txt"), "x").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeWithWork).await;
    assert!(
        danger
            .message
            .ends_with(" 2 commits are not pushed; branch feature keeps them."),
        "{}",
        danger.message
    );
    assert!(
        !danger.message.contains("on no remote"),
        "{}",
        danger.message
    );
    assert_eq!(
        danger.detail.as_deref(),
        Some("Untracked files (1):\n  new.txt"),
        "commits are not listed as lost"
    );
    fs::remove_file(wt.join("new.txt")).unwrap();
    // Alone, they ask nothing.
    let out = h.run(id, remove(&wt, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(!wt.exists());
    assert_eq!(
        worktree_writes(&h, &out)[0],
        ["worktree", "remove", "--", &real_gone(&wt)]
    );
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s", "feature"]),
        "local two",
        "the branch keeps them"
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
            .contains("has 1 commit on no branch, tag or remote."),
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
                "Commits on no branch, tag or remote (1):\n  {} only here",
                &lost[..8]
            )
            .as_str()
        )
    );
    // Confirmed, it goes with a plain remove: git needs no --force for commits.
    let out = h
        .run_with(id, remove(&wt, false), &confirmed(&[danger]))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(
        worktree_writes(&h, &out)[0],
        ["worktree", "remove", "--", &real_gone(&wt)]
    );
    // Commits that are on a branch or a tag (not pushed) are not lost with a detached
    // worktree.
    for keep in ["branch", "tag"] {
        let (repo2, _origin2, wt2) = pushed_worktree();
        git_in(&wt2, &["checkout", "-q", "--detach"]);
        git_in(&wt2, &["commit", "-q", "--allow-empty", "-m", "kept"]);
        git_in(&wt2, &[keep, "keep", "HEAD"]);
        let id2 = h.open(repo2.path()).await.repo_id;
        let out = h.run(id2, remove(&wt2, false)).await;
        assert_eq!(out.status, OpStatus::Done, "{keep}: {out:#?}");
    }
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
            .contains("A merge is in progress there; deleting the worktree abandons it."),
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

/// An edit to a file already listed, after the dialog, asks again (Task 4 review): the
/// token has the files' sizes and times. So does an ignored entry that changes.
#[tokio::test]
async fn a_listed_file_edited_after_the_dialog_asks_again() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::write(wt.join("notes.txt"), "first\n").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeWithWork).await;
    fs::write(wt.join("notes.txt"), "first, and much more written later\n").unwrap();
    let out = h
        .run_with(id, remove(&wt, false), &confirmed(&[danger]))
        .await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:#?}");
    assert!(wt.join("notes.txt").exists());
    fs::remove_file(wt.join("notes.txt")).unwrap();
    fs::write(wt.join("x.log"), "1").unwrap();
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeIgnoredFiles).await;
    fs::write(wt.join("x.log"), "12345").unwrap();
    let out = h
        .run_with(id, remove(&wt, false), &confirmed(&[danger]))
        .await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:#?}");
    let _ = repo;
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

/// Runs the removal of the active worktree `wt` (with `confirmed`) while the switch to main
/// is slowed, runs `during` once the removal has started, and returns its outcome.
async fn remove_during_switch(
    h: &Harness,
    id: u32,
    wt: &Path,
    confirmed: Vec<Confirmed>,
    during: impl FnOnce(),
) -> OpOutcome {
    h.m.set_active_worktree(id, wt).await.unwrap();
    h.m.set_status_delay(std::time::Duration::from_millis(1500));
    let run = tokio::spawn({
        let (m, request) = (h.m.clone(), remove(wt, false));
        async move { m.run_op(id, request, &confirmed).await }
    });
    h.sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"].is_object()
        })
        .await
        .expect("the removal runs");
    during();
    let out = tokio::time::timeout(TIMEOUT, run)
        .await
        .expect("ends")
        .unwrap()
        .unwrap();
    h.m.set_status_delay(std::time::Duration::ZERO);
    out
}

/// Final review M1: the re-read before removing covers the whole of what was confirmed, not
/// only the files. A commit made in a clean detached worktree during the switch would be
/// lost with it: the removal stops.
#[tokio::test]
async fn a_commit_made_in_a_clean_detached_worktree_during_the_switch_stops_the_removal() {
    let (repo, _origin, wt) = pushed_worktree();
    git_in(&wt, &["checkout", "-q", "--detach"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = remove_during_switch(&h, id, &wt, Vec::new(), || {
        git_in(&wt, &["commit", "-q", "--allow-empty", "-m", "wip"]);
    })
    .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message
            .starts_with("It changed while gg was getting ready; nothing was deleted."),
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
    assert!(worktree_writes(&h, &out).is_empty(), "no remove ran");
    assert!(wt.is_dir());
}

/// Final review M1, confirmed: HEAD moving during the switch (a commit, an amend, a reset)
/// is not what was confirmed.
#[tokio::test]
async fn head_moving_during_the_switch_after_a_confirmation_stops_the_removal() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::write(wt.join("notes.txt"), "x").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeWithWork).await;
    let out = remove_during_switch(&h, id, &wt, confirmed(&[danger]), || {
        git_in(&wt, &["commit", "-q", "--allow-empty", "-m", "moved"]);
    })
    .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message
            .starts_with("It changed since you confirmed; nothing was deleted."),
        "{}",
        out.message
    );
    assert!(worktree_writes(&h, &out).is_empty(), "no remove ran");
    assert!(wt.join("notes.txt").exists());
}

/// A remote-tracking ref to an object that is not there: `rev-list … --not --remotes` fails.
fn break_rev_list(repo: &TestRepo) {
    let common = repo.git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]);
    let refs = Path::new(&common).join("refs/remotes/origin");
    fs::create_dir_all(&refs).unwrap();
    fs::write(
        refs.join("broken"),
        "0123456789012345678901234567890123456789\n",
    )
    .unwrap();
}

/// Final review M3: when git cannot list the commits a detached worktree would lose, that
/// is not "none": the delete is red and says gg could not tell.
#[tokio::test]
async fn commits_that_cannot_be_counted_on_delete_are_red() {
    let (repo, _origin, wt) = pushed_worktree();
    git_in(&wt, &["checkout", "-q", "--detach"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    // After opening: the app's own ref reads refuse such a repository.
    break_rev_list(&repo);
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeWithWork).await;
    assert!(
        danger
            .message
            .contains("gg could not tell whether commits on no branch would be lost with it"),
        "{}",
        danger.message
    );
}

/// Final review M3, prune: an entry whose lost commits cannot be counted is red too.
#[tokio::test]
async fn commits_that_cannot_be_counted_on_prune_are_red() {
    let (repo, _origin, wt) = pushed_worktree();
    git_in(&wt, &["checkout", "-q", "--detach"]);
    fs::remove_dir_all(&wt).unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    // After opening: the app's own ref reads refuse such a repository.
    break_rev_list(&repo);
    let out = h.run(id, OpRequest::WorktreePrune {}).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:#?}");
    assert_eq!(out.dangers[0].kind, DangerKind::PruneWorktreesLosesCommits);
    assert!(
        out.dangers[0]
            .message
            .contains("gg could not tell whether commits on no branch would be lost"),
        "{}",
        out.dangers[0].message
    );
}

/// Final review M9: a worktree's danger is bound to the worktree: its token names the path,
/// so one worktree's confirmation is never taken for another's in the same state.
#[tokio::test]
async fn a_worktree_danger_names_its_worktree() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::write(wt.join("one.txt"), "x").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeWithWork).await;
    let token = danger.token.unwrap();
    assert_eq!(token.lines().next(), Some(real(&wt).as_str()), "{token}");
}

/// Final review C1: git's `status` (and git's own check before a plain `worktree remove`)
/// skips files marked assume-unchanged or skip-worktree, so edits to them were deleted
/// without a word. They are uncommitted changes: red, listed with why git hides them, and
/// removed only with a confirmed `--force`.
#[tokio::test]
async fn edits_to_files_marked_assume_unchanged_or_skip_worktree_are_work() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::write(wt.join("a.txt"), "a, edited locally\n").unwrap();
    git_in(&wt, &["update-index", "--assume-unchanged", "a.txt"]);
    fs::write(wt.join(".gitignore"), "build/\n*.log\n# mine\n").unwrap();
    git_in(&wt, &["update-index", "--skip-worktree", ".gitignore"]);
    assert_eq!(
        git_in(&wt, &["status", "--porcelain"]),
        "",
        "git shows nothing"
    );
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeWithWork).await;
    assert!(
        danger
            .message
            .contains("has uncommitted changes to 2 files."),
        "{}",
        danger.message
    );
    let detail = danger.detail.as_deref().unwrap();
    assert!(detail.starts_with("Uncommitted changes (2):\n"), "{detail}");
    assert!(
        detail.contains("  a.txt (marked assume-unchanged: git status does not show it)"),
        "{detail}"
    );
    assert!(
        detail.contains("  .gitignore (marked skip-worktree: git status does not show it)"),
        "{detail}"
    );
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
        ]]
    );
}

/// A marked file edited after the dialog changes the token (its size and time are in it).
#[tokio::test]
async fn a_marked_file_edited_after_the_dialog_asks_again() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::write(wt.join("a.txt"), "local\n").unwrap();
    git_in(&wt, &["update-index", "--skip-worktree", "a.txt"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeWithWork).await;
    fs::write(
        wt.join("a.txt"),
        "local, and more written after the dialog\n",
    )
    .unwrap();
    let out = h
        .run_with(id, remove(&wt, false), &confirmed(&[danger]))
        .await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:#?}");
    assert!(wt.join("a.txt").exists());
}

/// A marked file that is as the index has it is no work, and neither is a skip-worktree
/// file that is not on disk at all (a sparse checkout leaves them out): nothing is asked,
/// and the remove is plain.
#[tokio::test]
async fn marked_files_unchanged_or_left_out_by_a_sparse_checkout_are_no_danger() {
    let (repo, _origin, wt) = pushed_worktree();
    git_in(&wt, &["update-index", "--assume-unchanged", "a.txt"]);
    git_in(&wt, &["update-index", "--skip-worktree", ".gitignore"]);
    fs::remove_file(wt.join(".gitignore")).unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, remove(&wt, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(out.dangers.is_empty());
    assert!(!wt.exists());
    assert_eq!(
        worktree_writes(&h, &out),
        [vec![
            "worktree".to_owned(),
            "remove".into(),
            "--".into(),
            real_gone(&wt)
        ]],
        "plain remove"
    );
}

/// A clean worktree whose marked file is edited while the main worktree becomes active is
/// not removed: the body's re-read sees marked files too.
#[tokio::test]
async fn a_marked_file_edited_during_the_switch_stops_the_removal() {
    let (repo, _origin, wt) = pushed_worktree();
    git_in(&wt, &["update-index", "--assume-unchanged", "a.txt"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_active_worktree(id, &wt).await.unwrap();
    h.m.set_status_delay(std::time::Duration::from_millis(1500));
    let run = tokio::spawn({
        let (m, request) = (h.m.clone(), remove(&wt, false));
        async move { m.run_op(id, request, &[]).await }
    });
    h.sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"].is_object()
        })
        .await
        .expect("the removal runs");
    fs::write(wt.join("a.txt"), "edited during the switch\n").unwrap();
    let out = tokio::time::timeout(TIMEOUT, run)
        .await
        .expect("ends")
        .unwrap()
        .unwrap();
    h.m.set_status_delay(std::time::Duration::ZERO);
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message
            .starts_with("It changed while gg was getting ready; nothing was deleted."),
        "{}",
        out.message
    );
    assert!(worktree_writes(&h, &out).is_empty(), "no remove ran");
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
            "Nothing was deleted: the worktree at {} is locked (on a usb disk). git keeps a \
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
    let mark = h.last_log_id();
    let out = h.run(id, remove(&wt, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    // Main became active (its status was read for the switch) before the folder went.
    let all = h.log.since(mark);
    let is_remove = |r: &CommandRecord| {
        r.args.first().is_some_and(|a| a == "worktree")
            && r.args.get(1).is_some_and(|a| a == "remove")
    };
    let removed_at = all.iter().position(is_remove).expect("removed");
    let main_status_at = all
        .iter()
        .position(|r| {
            r.args.first().is_some_and(|a| a == "status")
                && r.args.iter().any(|a| a == "--ignore-submodules=all")
                && r.cwd == real(repo.path())
        })
        .expect("main's status read for the switch");
    assert!(main_status_at < removed_at, "{all:#?}");
    assert_eq!(
        out.message,
        format!(
            "Deleted the worktree at {}. The main worktree, {}, is active now.",
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

/// Task 4 review: removing the active worktree switches to main first, which takes a moment;
/// a file written meanwhile is not deleted unconfirmed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_written_while_main_becomes_active_stops_the_removal() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::write(wt.join("seen.txt"), "x").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_active_worktree(id, &wt).await.unwrap();
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeWithWork).await;
    // The switch waits for the main worktree's status, slow here.
    h.m.set_status_delay(std::time::Duration::from_millis(1500));
    let run = tokio::spawn({
        let (m, request) = (h.m.clone(), remove(&wt, false));
        async move { m.run_op(id, request, &confirmed(&[danger])).await }
    });
    h.sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"].is_object()
        })
        .await
        .expect("the removal runs");
    fs::write(wt.join("late.txt"), "written during the switch").unwrap();
    let out = tokio::time::timeout(TIMEOUT, run)
        .await
        .expect("ends")
        .unwrap()
        .unwrap();
    h.m.set_status_delay(std::time::Duration::ZERO);
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message
            .starts_with("It changed since you confirmed; nothing was deleted."),
        "{}",
        out.message
    );
    assert!(wt.join("late.txt").exists());
    assert!(worktree_writes(&h, &out).is_empty(), "no remove ran");
    assert_eq!(out.snapshot.active_worktree, real(repo.path()));
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
    // Work that would otherwise need --force: still never offered, never forced.
    fs::write(wt.join("untracked.txt"), "x").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_active_worktree(id, &wt).await.unwrap();
    let out = h.run(id, remove(&wt, false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(out.dangers.is_empty(), "no confirmation offered");
    assert!(
        out.message
            .starts_with("git does not delete a worktree that contains submodules"),
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
    assert!(wt.join("untracked.txt").exists());
    assert_eq!(
        worktree_writes(&h, &out),
        [vec![
            "worktree".to_owned(),
            "remove".into(),
            "--".into(),
            real(&wt)
        ]],
        "a plain remove ran (git refused it)"
    );
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
        "{} contains the worktree at {}. Delete that one first.",
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
         delete it first.",
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

/// Final review I1 (P3-R12): git lists an ignored folder as one entry, so a repository two
/// or more levels inside it (`tmp/refs/nested`) was deleted under the ordinary dialog for
/// ignored files. Found at any depth now, in an ignored folder and in an untracked one.
#[tokio::test]
async fn a_repository_at_any_depth_inside_an_ignored_folder_is_never_removed_with_it() {
    let (repo, _origin, wt) = pushed_worktree();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    for depth in ["build/one", "build/refs/nested", "build/a/b/c/nested"] {
        let inner = wt.join(depth);
        fs::create_dir_all(&inner).unwrap();
        git_in(&inner, &["init", "-q"]);
        fs::write(inner.join("mine.txt"), "x").unwrap();
        // Something beside it, so the folder is not only the repository.
        fs::write(wt.join("build/out.o"), "built").unwrap();
        let says = format!(
            "{} contains another repository at {}, which git would delete with it.",
            real(&wt),
            real(&inner)
        );
        refused_for(&h, id, &wt, &inner, &says).await;
        fs::remove_dir_all(wt.join("build")).unwrap();
    }
    // Ignored inside an untracked folder (git lists `! new/cache/`), two levels down.
    let inner = wt.join("new/cache/x/nested");
    fs::create_dir_all(&inner).unwrap();
    fs::write(wt.join("new/keep.txt"), "x").unwrap();
    fs::write(wt.join("new/.gitignore"), "cache/\n").unwrap();
    git_in(&inner, &["init", "-q"]);
    refused_for(&h, id, &wt, &inner, "contains another repository at").await;
    // An untracked folder, four levels down (git itself stops at the repository).
    fs::remove_dir_all(wt.join("new")).unwrap();
    let inner = wt.join("plain/a/b/c");
    fs::create_dir_all(&inner).unwrap();
    git_in(&inner, &["init", "-q"]);
    refused_for(&h, id, &wt, &inner, "contains another repository at").await;
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
        branch.message.ends_with(
            "1 of its commits is not pushed anywhere (it is on no other branch, tag or remote)."
        ),
        "{}",
        branch.message
    );
    assert_eq!(
        branch.token.as_deref(),
        Some(
            format!(
                "refs/heads/feature\n{}",
                git_in(&wt, &["rev-parse", "HEAD"])
            )
            .as_str()
        ),
        "R9: the branch's tip, and (final review M9) the branch it is"
    );
    assert!(
        !out.dangers[0].message.contains("not pushed"),
        "the branch's danger says it: {}",
        out.dangers[0].message
    );
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

/// The branch's "not pushed anywhere" counts only what deleting it loses: commits on no
/// other branch, tag or remote.
#[tokio::test]
async fn commits_another_branch_has_are_not_counted_as_lost_with_the_branch() {
    let (repo, _origin, wt) = pushed_worktree();
    git_in(&wt, &["commit", "-q", "--allow-empty", "-m", "local"]);
    repo.git(&["branch", "copy", "feature"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, remove(&wt, true)).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:#?}");
    assert_eq!(out.dangers.len(), 1, "{:#?}", out.dangers);
    assert_eq!(
        out.dangers[0].kind,
        DangerKind::DeleteUnmergedBranchWithWorktree
    );
    assert!(
        !out.dangers[0].message.contains("not pushed anywhere"),
        "{}",
        out.dangers[0].message
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
            "Deleted the worktree at {}. Branch \u{201c}feature\u{201d} was not deleted: it is \
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

/// The worktree went, the branch could not: the outcome is done, saying so.
#[tokio::test]
async fn a_branch_that_cannot_be_deleted_after_its_worktree_is_said_in_a_done_outcome() {
    let (repo, _origin, wt) = pushed_worktree();
    // A lock file git leaves behind holds the branch.
    fs::write(repo.path().join(".git/refs/heads/feature.lock"), "").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, remove(&wt, true)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(
        out.message.starts_with(&format!(
            "Deleted the worktree at {}. Branch \u{201c}feature\u{201d} was not deleted: ",
            real_gone(&wt)
        )),
        "{}",
        out.message
    );
    assert!(out.detail.is_some());
    assert!(!wt.exists());
    assert!(repo.git_succeeds(&["rev-parse", "--verify", "-q", "refs/heads/feature"]));
}

/// The path git keeps for a worktree must be its own folder (Task 4 review): not a link to
/// another worktree's, nor one of two entries leading to the same folder.
#[cfg(unix)]
#[tokio::test]
async fn a_worktree_path_that_is_a_link_is_refused() {
    let repo = TestRepo::new();
    repo.commit("one");
    let one = repo.add_worktree("one");
    let two = repo.add_worktree("two");
    let moved = repo.dir.path().join("moved");
    fs::rename(&two, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &two).unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let mark = h.last_log_id();
    let out = h.run(id, remove(&two, false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message
            .ends_with("is a link to another folder; delete the worktree from a terminal."),
        "{}",
        out.message
    );
    // Two entries leading to one folder: `two` made a link to `one`.
    fs::remove_file(&two).unwrap();
    std::os::unix::fs::symlink(&one, &two).unwrap();
    let out = h.run(id, remove(&one, false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message.contains("is a link to another folder")
            || out.message.contains("entries that lead to"),
        "{}",
        out.message
    );
    assert!(h.writes_since(mark).is_empty());
    assert!(one.is_dir() && moved.is_dir());
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
    // Asked first (Task 4 review): an ordinary confirmation naming what git forgets.
    let mark = h.last_log_id();
    let asked = h.run(id, OpRequest::WorktreePrune {}).await;
    assert_eq!(asked.status, OpStatus::NeedsConfirmation, "{asked:#?}");
    assert!(h.writes_since(mark).is_empty());
    let danger = &asked.dangers[0];
    assert_eq!(asked.dangers.len(), 1);
    assert_eq!(danger.kind, DangerKind::PruneWorktrees);
    assert_eq!(
        danger.message,
        "Pruning makes git forget 1 worktree entry whose folder is gone (listed below). If it \
         is on a disconnected drive, reconnect it or lock the worktree instead."
    );
    assert_eq!(
        danger.detail.as_deref(),
        Some(format!("Entries forgotten (1):\n  {gone_listed} (branch gone)").as_str())
    );
    let out = h
        .run_confirmed(
            id,
            OpRequest::WorktreePrune {},
            &[DangerKind::PruneWorktrees],
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(out.title, "Prune stale worktree entries");
    assert_eq!(
        out.message,
        format!(
            "Pruned 1 stale worktree entry, whose folder was gone: {gone_listed}. 1 missing \
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

/// A stale detached entry whose commits are on no branch, tag or remote: pruning loses
/// them, so the confirmation is the red one and lists them.
#[tokio::test]
async fn pruning_a_detached_entry_with_commits_of_its_own_is_red_and_lists_them() {
    let repo = TestRepo::new();
    repo.commit("one");
    let wt = repo.add_worktree("tmp");
    git_in(&wt, &["checkout", "-q", "--detach"]);
    git_in(&wt, &["commit", "-q", "--allow-empty", "-m", "only there"]);
    let lost = git_in(&wt, &["rev-parse", "HEAD"]);
    repo.git(&["branch", "-D", "tmp"]);
    let listed_path = real(&wt);
    fs::remove_dir_all(&wt).unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let asked = h.run(id, OpRequest::WorktreePrune {}).await;
    assert_eq!(asked.status, OpStatus::NeedsConfirmation, "{asked:#?}");
    let danger = &asked.dangers[0];
    assert_eq!(danger.kind, DangerKind::PruneWorktreesLosesCommits);
    assert!(
        danger
            .message
            .contains("and with it 1 commit on no branch, tag or remote, which is lost."),
        "{}",
        danger.message
    );
    assert_eq!(
        danger.detail.as_deref(),
        Some(
            format!(
                "Entries forgotten (1):\n  {listed_path} (detached at {})\n\nCommits on no \
                 branch, tag or remote (1):\n  {} only there",
                &lost[..8],
                &lost[..8]
            )
            .as_str()
        )
    );
    // A confirmation for the ordinary kind does not do.
    let ordinary = [Confirmed {
        kind: DangerKind::PruneWorktrees,
        token: danger.token.clone(),
    }];
    let again = h.run_with(id, OpRequest::WorktreePrune {}, &ordinary).await;
    assert_eq!(again.status, OpStatus::NeedsConfirmation);
    let out = h
        .run_with(id, OpRequest::WorktreePrune {}, &confirmed(&asked.dangers))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(listed(&repo).len(), 1);
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

/// The outcome names the worktree the request made, as the snapshot lists it, so the UI can
/// make exactly that one active: never one another program added meanwhile. A request that
/// made none names none.
#[tokio::test]
async fn the_outcome_names_the_worktree_it_made() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("mine");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    // An agent adds one from a terminal while the dialog is open.
    let theirs = repo.add_worktree("agent");
    let out = h
        .run(id, add("../repo.worktrees/mine", existing("mine")))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    let mine = real(&repo.dir.path().join("repo.worktrees/mine"));
    assert_eq!(out.created.as_deref(), Some(mine.as_str()));
    let listed: Vec<&str> = out
        .snapshot
        .worktrees
        .iter()
        .map(|w| w.path.as_str())
        .collect();
    assert!(listed.contains(&mine.as_str()), "{listed:?}");
    assert!(listed.contains(&real(&theirs).as_str()), "{listed:?}");

    // Refused (the folder is a worktree already): nothing made, nothing named.
    let out = h
        .run(id, add("../repo.worktrees/mine", existing("main")))
        .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert_eq!(out.created, None);
    let out = h.run(id, OpRequest::WorktreePrune {}).await;
    assert_eq!(out.created, None);
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
        // The dialog shows it under the location, told so by the core, not by the wording.
        assert_eq!(out.field, Some(InputField::Location), "{path:?}");
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
    assert_eq!(out.field, None);
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
    assert_eq!(out.field, Some(InputField::Name));
    assert!(h.writes_since(mark).is_empty(), "nothing ran");
    assert!(!base.join("x").exists() && !base.join("y").exists());
    // Bad names and arguments never reach the queue.
    for (request, code, field) in [
        (
            add(
                &base.join("z").to_string_lossy(),
                WorktreeBranch::Create {
                    create: "bad name".into(),
                    at: head.clone(),
                },
            ),
            ErrorCode::BadName,
            Some(InputField::Name),
        ),
        (
            add(&base.join("z").to_string_lossy(), existing("-x")),
            ErrorCode::BadRequest,
            None,
        ),
        (
            add(
                &base.join("z").to_string_lossy(),
                WorktreeBranch::Detach {
                    detach: "HEAD".into(),
                },
            ),
            ErrorCode::BadRequest,
            None,
        ),
        (
            add(" ", existing("free")),
            ErrorCode::BadRequest,
            Some(InputField::Location),
        ),
    ] {
        let err = h.m.run_op(id, request, &[]).await.unwrap_err();
        assert_eq!(err.code, code, "{}", err.message);
        assert_eq!(err.field, field, "{}", err.message);
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(
            json.get("field").cloned(),
            field.map(|f| serde_json::to_value(f).unwrap()),
            "absent when about no field"
        );
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
    assert_eq!(
        path.path,
        native(&format!("{base}/repo.worktrees/feature-xy"))
    );
    assert!(!path.exists);
    // A non-empty folder there: -2. An empty one is usable and said to exist.
    fs::create_dir_all(format!("{base}/repo.worktrees/feature-xy")).unwrap();
    fs::write(format!("{base}/repo.worktrees/feature-xy/f"), "x").unwrap();
    fs::create_dir_all(format!("{base}/repo.worktrees/feature-xy-2")).unwrap();
    let path =
        h.m.worktree_default_path(id, "feature/x:y", pattern)
            .await
            .unwrap();
    assert_eq!(
        path.path,
        native(&format!("{base}/repo.worktrees/feature-xy-2"))
    );
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
    assert_eq!(path.path, native(&format!("{base}/repo.worktrees/main2-2")));
    // Absolute patterns, and through dispatch-free settings: an odd branch.
    let path =
        h.m.worktree_default_path(id, "@@", &format!("{base}/flat/{{repo}}-{{branch}}"))
            .await
            .unwrap();
    assert_eq!(path.path, native(&format!("{base}/flat/repo-worktree")));
    // One that lands inside the main worktree, a linked one or the git dir: the default
    // pattern instead (Task 4 review).
    let other = repo.add_worktree("other");
    for inside in [
        format!("{base}/repo/inside/{{branch}}"),
        format!("{}/sub/{{branch}}", real(&other)),
        format!("{base}/repo/.git/{{branch}}"),
    ] {
        let path = h.m.worktree_default_path(id, "x", &inside).await.unwrap();
        assert_eq!(
            path.path,
            native(&format!("{base}/repo.worktrees/x")),
            "{inside}"
        );
    }
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

// ---- Task 5 review fixes ----

/// A clean worktree needs no confirmation, but what is removed must still be what was read:
/// files that appear while the main worktree becomes active (a build's `build/`) stop it.
#[tokio::test]
async fn a_clean_worktree_that_gains_files_during_the_switch_is_not_removed() {
    let (repo, _origin, wt) = pushed_worktree();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_active_worktree(id, &wt).await.unwrap();
    h.m.set_status_delay(std::time::Duration::from_millis(1500));
    let run = tokio::spawn({
        let (m, request) = (h.m.clone(), remove(&wt, false));
        async move { m.run_op(id, request, &[]).await }
    });
    h.sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"].is_object()
        })
        .await
        .expect("the removal runs");
    fs::create_dir_all(wt.join("build")).unwrap();
    fs::write(wt.join("build/out.o"), "built during the switch").unwrap();
    let out = tokio::time::timeout(TIMEOUT, run)
        .await
        .expect("ends")
        .unwrap()
        .unwrap();
    h.m.set_status_delay(std::time::Duration::ZERO);
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message
            .starts_with("It changed while gg was getting ready; nothing was deleted."),
        "{}",
        out.message
    );
    assert!(wt.join("build/out.o").exists());
    assert!(worktree_writes(&h, &out).is_empty(), "no remove ran");
}

/// A worktree listed inside the one being removed whose own folder is gone: prune it first.
#[tokio::test]
async fn a_stale_worktree_listed_inside_says_to_prune() {
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
    let shown_inner = real(&inner);
    fs::remove_dir_all(&inner).unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let mark = h.last_log_id();
    let out = h.run(id, remove(&wt, false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert_eq!(
        out.message,
        format!(
            "{} lists the worktree at {shown_inner} inside it, whose folder is gone. Prune \
             stale entries first.",
            real(&wt)
        )
    );
    assert!(h.writes_since(mark).is_empty(), "nothing ran");
    assert!(wt.is_dir());
}

/// A worktree whose path only starts like another's (`wt-feature`, `wt-feature2`) is not
/// inside it.
#[tokio::test]
async fn a_sibling_whose_name_starts_alike_is_not_inside() {
    let (repo, _origin, wt) = pushed_worktree();
    repo.branch("feature2");
    let sibling = repo.add_worktree("feature2");
    assert!(
        sibling
            .to_string_lossy()
            .starts_with(&*wt.to_string_lossy())
    );
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, remove(&wt, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(!wt.exists());
    assert!(sibling.is_dir());
}

/// The status read that sees ignored files runs once in the removal's turn (the refusal
/// checks and the dangers share it), and once more in its body, after the switch.
#[tokio::test]
async fn the_files_are_read_once_in_the_turn_and_once_before_removing() {
    let (repo, _origin, wt) = pushed_worktree();
    fs::write(wt.join("work.txt"), "untracked\n").unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let reads = |since: u64| {
        h.log
            .since(since)
            .into_iter()
            .filter(|r| r.args.iter().any(|a| a == "--ignored=matching"))
            .count()
    };
    let mark = h.last_log_id();
    let danger = asked(&h, id, &wt, DangerKind::RemoveWorktreeWithWork).await;
    assert_eq!(reads(mark), 1, "asking reads once");
    let mark = h.last_log_id();
    let out = h
        .run_with(id, remove(&wt, false), &confirmed(&[danger]))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(reads(mark), 2, "the turn once, the body once");
}
