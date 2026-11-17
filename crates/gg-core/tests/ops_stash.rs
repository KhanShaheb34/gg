//! Integration tests for the stash operations (`run_op`): save, apply, pop, drop and branch
//! from a stash, against real temp repositories. A request names its stash by commit hash;
//! the core finds where it is in the list in the operation's turn.

use std::fs;

use gg_core::dto::{DangerKind, OpRequest, OpStatus, RowKind};
use gg_core::error::ErrorCode;
use gg_core::git::runner::CmdKind;
use gg_core::testutil::TestRepo;

mod common;
use common::*;

fn save(message: Option<&str>, include_untracked: bool) -> OpRequest {
    OpRequest::StashSave {
        message: message.map(str::to_owned),
        include_untracked,
        expect_worktree: here_wt(),
    }
}

fn apply(stash: &str) -> OpRequest {
    OpRequest::StashApply {
        stash: stash.to_owned(),
        expect_head: here(),
        expect_worktree: here_wt(),
    }
}

fn pop(stash: &str) -> OpRequest {
    OpRequest::StashPop {
        stash: stash.to_owned(),
        expect_head: here(),
        expect_worktree: here_wt(),
    }
}

fn drop_stash(stash: &str) -> OpRequest {
    OpRequest::StashDrop {
        stash: stash.to_owned(),
        expect_worktree: here_wt(),
    }
}

fn branch(stash: &str, name: &str) -> OpRequest {
    OpRequest::StashBranch {
        stash: stash.to_owned(),
        name: name.to_owned(),
        expect_worktree: here_wt(),
    }
}

fn read(repo: &TestRepo, rel: &str) -> String {
    fs::read_to_string(repo.path().join(rel)).unwrap()
}

/// `(commit, subject)` of every stash, newest first.
fn stashes(repo: &TestRepo) -> Vec<(String, String)> {
    repo.git(&["stash", "list", "--format=%H %s"])
        .lines()
        .map(|l| {
            let (h, s) = l.split_once(' ').unwrap();
            (h.to_owned(), s.to_owned())
        })
        .collect()
}

fn write_args(h: &Harness, out: &gg_core::dto::OpOutcome) -> Vec<Vec<String>> {
    op_records(&h.log, out)
        .into_iter()
        .filter(|r| r.kind == CmdKind::Write)
        .map(|r| r.args)
        .collect()
}

/// A repository with one commit (`notes.txt` holds "base").
fn repo_with_notes() -> TestRepo {
    let repo = TestRepo::new();
    repo.write("notes.txt", "base\n");
    repo.commit_all("base");
    repo
}

#[tokio::test]
async fn save_with_message_and_untracked() {
    let repo = repo_with_notes();
    repo.write("notes.txt", "edited\n");
    repo.write("loose.txt", "untracked\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    // A message that looks like an option stays the message.
    let out = h.run(id, save(Some("--all: ünï"), true)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(
        write_args(&h, &out),
        [vec![
            "stash",
            "push",
            "--include-untracked",
            "--message=--all: ünï",
            "--"
        ]]
    );
    let list = stashes(&repo);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].1, "On main: --all: ünï");
    assert_eq!(read(&repo, "notes.txt"), "base\n");
    assert!(
        !repo.path().join("loose.txt").exists(),
        "untracked file stashed"
    );
    // The outcome shows the stash's row.
    assert_eq!(
        out.snapshot.status.changed + out.snapshot.status.untracked,
        0
    );
    let page = h.m.rows(id, out.snapshot.generation, 0, 10).await.unwrap();
    assert_eq!(page.rows[0].hash, list[0].0);
    assert_eq!(page.rows[0].kind, RowKind::Stash);
    assert_eq!(page.rows[0].labels[0].name, "stash@{0}");

    // Without untracked files (and a blank message is no message): they stay.
    repo.write("notes.txt", "edited again\n");
    repo.write("loose.txt", "untracked\n");
    let out = h.run(id, save(Some("  "), false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(write_args(&h, &out), [vec!["stash", "push", "--"]]);
    assert!(stashes(&repo)[0].1.starts_with("WIP on main: "));
    assert!(repo.path().join("loose.txt").exists());

    // Nothing to stash: only an untracked file, left out.
    let err =
        h.m.run_op(id, in_wt(repo.path(), save(None, false)), &[])
            .await
            .unwrap_err();
    assert_eq!(err.code, ErrorCode::NothingToDo);
    assert!(err.message.contains("untracked"), "{}", err.message);
    assert_eq!(stashes(&repo).len(), 2);
}

#[tokio::test]
async fn apply_keeps_and_pop_removes() {
    let repo = repo_with_notes();
    repo.write("notes.txt", "stashed\n");
    let w = repo.stash(&[]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, apply(&w)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(write_args(&h, &out), [vec!["stash", "apply", "stash@{0}"]]);
    assert_eq!(read(&repo, "notes.txt"), "stashed\n");
    assert_eq!(stashes(&repo).len(), 1, "apply keeps the stash");

    repo.git(&["checkout", "-q", "--", "notes.txt"]);
    let out = h.run(id, pop(&w)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(write_args(&h, &out), [vec!["stash", "pop", "stash@{0}"]]);
    assert_eq!(read(&repo, "notes.txt"), "stashed\n");
    assert!(stashes(&repo).is_empty(), "pop drops it");
    let page = h.m.rows(id, out.snapshot.generation, 0, 10).await.unwrap();
    assert!(page.rows.iter().all(|r| r.kind != RowKind::Stash));

    // Changes in the way: nothing applied, and said plainly.
    repo.write("notes.txt", "stashed again\n");
    let w = repo.stash(&[]);
    repo.write("notes.txt", "in the way\n");
    h.m.refresh(id).await.unwrap();
    for request in [apply(&w), pop(&w)] {
        let out = h.run(id, request).await;
        assert_eq!(out.status, OpStatus::Failed, "{out:?}");
        assert!(out.message.contains("notes.txt"), "{}", out.message);
        assert!(
            out.message.contains("would be overwritten"),
            "{}",
            out.message
        );
        assert!(out.message.contains("stash is kept"), "{}", out.message);
        assert_eq!(read(&repo, "notes.txt"), "in the way\n");
        assert_eq!(stashes(&repo).len(), 1);
    }
}

/// Apply and pop put the stash's changes into the branch checked out: asked for on one
/// branch, they never land on another that was checked out meanwhile (in a terminal, say),
/// nor in another worktree made active since; nothing runs and the stash stays.
#[tokio::test]
async fn apply_and_pop_run_only_on_the_branch_and_worktree_they_were_asked_for() {
    let repo = repo_with_notes();
    repo.write("notes.txt", "stashed\n");
    let w = repo.stash(&[]);
    repo.branch("other");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let shown = head_at(repo.path());
    let worktree = h.m.snapshot(id).await.unwrap().active_worktree;
    repo.checkout("other");
    for request in [
        OpRequest::StashApply {
            stash: w.clone(),
            expect_head: shown.clone(),
            expect_worktree: worktree.clone(),
        },
        OpRequest::StashPop {
            stash: w.clone(),
            expect_head: shown.clone(),
            expect_worktree: worktree.clone(),
        },
    ] {
        let out = h.m.run_op(id, request, &[]).await.unwrap();
        assert_eq!(out.status, OpStatus::Failed, "{out:?}");
        assert!(
            out.message.contains("main") && out.message.contains("other"),
            "{}",
            out.message
        );
        assert!(write_args(&h, &out).is_empty(), "nothing ran");
        assert_eq!(read(&repo, "notes.txt"), "base\n");
        assert_eq!(stashes(&repo).len(), 1);
    }
    // Another commit on the same branch is no reason to refuse: still that branch.
    repo.checkout("main");
    repo.write("more.txt", "more\n");
    repo.commit_all("more");
    let out =
        h.m.run_op(
            id,
            OpRequest::StashApply {
                stash: w.clone(),
                expect_head: shown.clone(),
                expect_worktree: worktree.clone(),
            },
            &[],
        )
        .await
        .unwrap();
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    // Another worktree active by its turn: refused.
    let out =
        h.m.run_op(
            id,
            OpRequest::StashPop {
                stash: w.clone(),
                expect_head: head_at(repo.path()),
                expect_worktree: "/elsewhere".into(),
            },
            &[],
        )
        .await
        .unwrap();
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(write_args(&h, &out).is_empty(), "nothing ran");
    assert_eq!(stashes(&repo).len(), 1);
}

#[tokio::test]
async fn drop_needs_confirmation() {
    let repo = repo_with_notes();
    repo.write("notes.txt", "stashed\n");
    let w = repo.stash(&["--message=half done"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let mark = h.last_log_id();

    let out = h.run(id, drop_stash(&w)).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert_eq!(out.dangers.len(), 1);
    let danger = &out.dangers[0];
    assert_eq!(danger.kind, DangerKind::DropStash);
    assert!(danger.message.contains("stash@{0}"), "{}", danger.message);
    assert!(
        danger.message.contains("On main: half done"),
        "{}",
        danger.message
    );
    assert_eq!(danger.token.as_deref(), Some(w.as_str()));
    assert!(h.writes_since(mark).is_empty(), "nothing ran");
    assert_eq!(stashes(&repo).len(), 1);

    let out = h
        .run_confirmed(id, drop_stash(&w), &[DangerKind::DropStash])
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(write_args(&h, &out), [vec!["stash", "drop", "stash@{0}"]]);
    assert!(stashes(&repo).is_empty());
    assert_eq!(read(&repo, "notes.txt"), "base\n");
}

/// The request names stash B by its commit; a newer stash saved since moved B to
/// `stash@{1}`, and the drop still takes B, not what is `stash@{0}` now.
#[tokio::test]
async fn drop_targets_the_right_stash_after_the_list_shifted() {
    let repo = repo_with_notes();
    repo.write("notes.txt", "a\n");
    let a = repo.stash(&["--message=A"]);
    repo.write("notes.txt", "b\n");
    let b = repo.stash(&["--message=B"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    // The UI saw B as stash@{0}; then C is saved outside the app.
    repo.write("notes.txt", "c\n");
    let c = repo.stash(&["--message=C"]);

    let out = h.run(id, drop_stash(&b)).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation);
    assert!(
        out.dangers[0].message.contains("stash@{1}"),
        "{}",
        out.dangers[0].message
    );
    assert!(
        out.dangers[0].message.contains("On main: B"),
        "{}",
        out.dangers[0].message
    );
    let out = h
        .run_confirmed(id, drop_stash(&b), &[DangerKind::DropStash])
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(write_args(&h, &out), [vec!["stash", "drop", "stash@{1}"]]);
    let left: Vec<String> = stashes(&repo).into_iter().map(|(h, _)| h).collect();
    assert_eq!(left, [c, a]);
}

#[tokio::test]
async fn stash_gone_fails_plainly() {
    let repo = repo_with_notes();
    repo.write("notes.txt", "stashed\n");
    let w = repo.stash(&[]);
    repo.write("notes.txt", "kept\n");
    let other = repo.stash(&[]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    repo.git(&["stash", "drop", "-q", "stash@{1}"]);
    let mark = h.last_log_id();

    for (request, confirmed) in [
        (apply(&w), &[][..]),
        (pop(&w), &[]),
        (drop_stash(&w), &[DangerKind::DropStash]),
        (branch(&w, "rescue"), &[]),
    ] {
        let out = h.run_confirmed(id, request.clone(), confirmed).await;
        assert_eq!(out.status, OpStatus::Failed, "{request:?}: {out:?}");
        assert!(out.message.contains("no longer exists"), "{}", out.message);
        assert_eq!((out.log_from, out.log_to), (0, 0));
        assert_eq!(out.detail, None);
    }
    assert!(h.writes_since(mark).is_empty(), "nothing ran");
    assert_eq!(stashes(&repo)[0].0, other, "the other stash is untouched");

    // Something that is not a commit hash never reaches git.
    for bad in ["stash@{0}", "-x", "abc"] {
        let err =
            h.m.run_op(id, in_wt(repo.path(), apply(bad)), &[])
                .await
                .unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest, "{bad}");
    }
}

#[tokio::test]
async fn branch_from_stash() {
    let repo = repo_with_notes();
    let base = repo.git(&["rev-parse", "HEAD"]);
    repo.write("notes.txt", "stashed\n");
    let w = repo.stash(&[]);
    repo.write("notes.txt", "moved on\n");
    repo.commit_all("later");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    for bad in ["-x", "a..b", ""] {
        let err =
            h.m.run_op(id, in_wt(repo.path(), branch(&w, bad)), &[])
                .await
                .unwrap_err();
        assert_eq!(err.code, ErrorCode::BadName, "{bad:?}");
    }
    let err =
        h.m.run_op(id, in_wt(repo.path(), branch(&w, "main")), &[])
            .await
            .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadName);
    assert!(err.message.contains("already exists"), "{}", err.message);

    let out = h.run(id, branch(&w, "rescue")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(
        write_args(&h, &out),
        [vec!["stash", "branch", "rescue", "stash@{0}"]]
    );
    assert_eq!(current_branch(&repo).as_deref(), Some("rescue"));
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), base);
    assert_eq!(read(&repo, "notes.txt"), "stashed\n");
    assert!(stashes(&repo).is_empty());
    assert_eq!(out.snapshot.head.branch.as_deref(), Some("rescue"));
}

/// git has no in-progress state for a stash that conflicts: the outcome is `failed`, says
/// that the stash was kept, and the conflicts are left to resolve.
#[tokio::test]
async fn pop_with_conflict_keeps_the_stash_and_explains() {
    let repo = repo_with_notes();
    repo.write("notes.txt", "stashed\n");
    let w = repo.stash(&[]);
    repo.write("notes.txt", "committed\n");
    repo.commit_all("clash");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, pop(&w)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.contains("conflicts in notes.txt"),
        "{}",
        out.message
    );
    assert!(out.message.contains("kept"), "{}", out.message);
    assert!(
        out.detail.as_deref().unwrap_or("").contains("CONFLICT"),
        "{:?}",
        out.detail
    );
    assert_eq!(stashes(&repo)[0].0, w, "the stash is kept");
    assert!(out.snapshot.in_progress.is_none());
    assert_eq!(out.snapshot.status.conflicted, 1);
    assert!(read(&repo, "notes.txt").contains("<<<<<<<"));
}

/// R17 (revised): while git is stopped in a merge, applying, popping or saving a stash is
/// refused (saving would take the merge's state with it); dropping one runs.
#[tokio::test]
async fn apply_and_pop_are_refused_while_git_is_stopped() {
    let repo = TestRepo::new();
    conflicting(&repo);
    repo.write("other.txt", "stashed\n");
    repo.git(&["add", "other.txt"]);
    let w = repo.stash(&[]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(
        h.run(id, merge_branch("feature")).await.status,
        OpStatus::Stopped
    );
    let mark = h.last_log_id();
    for request in [apply(&w), pop(&w), branch(&w, "rescue")] {
        let out = h.run(id, request.clone()).await;
        assert_eq!(out.status, OpStatus::Failed, "{request:?}: {out:?}");
        assert!(
            out.message.contains("A merge is in progress"),
            "{}",
            out.message
        );
        assert_eq!((out.log_from, out.log_to), (0, 0));
    }
    assert!(h.writes_since(mark).is_empty());

    let out = h
        .run_confirmed(id, drop_stash(&w), &[DangerKind::DropStash])
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(stashes(&repo).is_empty());
    // Saving is refused before git runs.
    let mark = h.last_log_id();
    let out = h.run(id, save(None, false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.contains("A merge is in progress"),
        "{}",
        out.message
    );
    assert!(h.writes_since(mark).is_empty());
}

/// R17 (revised): with the conflicts resolved and staged, git would stash the merge's
/// result and remove MERGE_HEAD, silently ending the merge; the core refuses instead.
#[tokio::test]
async fn saving_a_stash_does_not_end_a_stopped_merge() {
    let repo = TestRepo::new();
    conflicting(&repo);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(
        h.run(id, merge_branch("feature")).await.status,
        OpStatus::Stopped
    );
    repo.write("clash.txt", "resolved\n");
    repo.git(&["add", "clash.txt"]);

    let out = h.run(id, save(None, false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.contains("A merge is in progress"),
        "{}",
        out.message
    );
    assert!(in_git_dir(&repo, "MERGE_HEAD"));
    assert!(stashes(&repo).is_empty());
}

/// The branch is made and checked out, but the stash does not apply
/// (uncommitted changes in the way): the outcome says the branch exists and is checked out,
/// and that the stash is kept.
#[tokio::test]
async fn branch_from_stash_that_does_not_apply_says_the_branch_is_there() {
    let repo = repo_with_notes();
    repo.write("notes.txt", "stashed\n");
    let w = repo.stash(&[]);
    repo.write("notes.txt", "in the way\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, branch(&w, "rescue")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert_eq!(current_branch(&repo).as_deref(), Some("rescue"));
    assert_eq!(stashes(&repo)[0].0, w, "the stash is kept");
    assert!(
        out.message.starts_with(
            "Created branch rescue and checked it out, but the stash could not be applied"
        ),
        "{}",
        out.message
    );
    assert!(out.message.contains("The stash is kept"), "{}", out.message);
    assert!(!out.message.contains("Popping"), "{}", out.message);
}

/// Item 7: the checkout of the stash's base is blocked: nothing was made; the message is
/// git's meaning (changes in the way of the checkout), not a pop's.
#[tokio::test]
async fn branch_from_stash_blocked_by_the_checkout_says_so() {
    let repo = repo_with_notes();
    repo.write("other.txt", "stashed\n");
    repo.git(&["add", "other.txt"]);
    let w = repo.stash(&[]);
    repo.write("notes.txt", "moved on\n");
    repo.commit_all("later");
    repo.write("notes.txt", "uncommitted\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, branch(&w, "rescue")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert_eq!(current_branch(&repo).as_deref(), Some("main"));
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "-q", "refs/heads/rescue"]));
    assert!(out.message.contains("notes.txt"), "{}", out.message);
    assert!(out.message.contains("checkout"), "{}", out.message);
    assert!(!out.message.contains("Popping"), "{}", out.message);
    assert!(!out.message.contains("by the stash"), "{}", out.message);
}
