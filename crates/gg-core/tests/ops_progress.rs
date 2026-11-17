//! Integration tests for what git stopped in: continuing and aborting it (`run_op`), the
//! operations refused meanwhile, and opening files in an editor, against real temp
//! repositories.

use std::fs;
use std::path::Path;

use gg_core::dto::{
    ConflictState, DangerKind, InProgressKind, MergeTarget, MergeTargetKind, OpHint, OpRequest,
    OpStatus,
};
use gg_core::error::ErrorCode;
use gg_core::git::runner::CmdKind;
use gg_core::testutil::TestRepo;

mod common;
use common::*;

/// `main` and `feature` both change `clash.txt` after `base`: `feature` in three commits,
/// "feat one" to "feat three", `main` in one. HEAD on `main`.
fn clashing(repo: &TestRepo) {
    repo.write("clash.txt", "base\n");
    repo.commit_all("base");
    repo.branch("feature");
    repo.checkout("feature");
    for (i, name) in ["one", "two", "three"].iter().enumerate() {
        repo.write("clash.txt", format!("feature {i}\n"));
        repo.commit_all(&format!("feat {name}"));
    }
    repo.checkout("main");
    repo.write("clash.txt", "main\n");
    repo.commit_all("main side");
}

fn args_of(h: &Harness, out: &gg_core::dto::OpOutcome) -> Vec<Vec<String>> {
    op_records(&h.log, out)
        .into_iter()
        .filter(|r| r.kind == CmdKind::Write)
        .map(|r| r.args)
        .collect()
}

fn cherry_pick(hash: &str) -> OpRequest {
    OpRequest::CherryPick {
        hash: hash.to_owned(),
        mainline: None,
        no_commit: false,
        expect_head: here(),
        expect_worktree: here_wt(),
    }
}

fn revert(hash: &str) -> OpRequest {
    OpRequest::Revert {
        hash: hash.to_owned(),
        mainline: None,
        expect_head: here(),
        expect_worktree: here_wt(),
    }
}

fn create_branch_and_checkout(name: &str, at: &str) -> OpRequest {
    OpRequest::CreateBranch {
        name: name.to_owned(),
        at: at.to_owned(),
        checkout: true,
        expect_worktree: here_wt(),
    }
}

fn read(repo: &TestRepo, rel: &str) -> String {
    fs::read_to_string(repo.path().join(rel)).unwrap()
}

// ---- Continue ----

#[tokio::test]
async fn continue_stages_resolved_files_and_finishes_the_merge() {
    let repo = TestRepo::new();
    let names = ["clash.txt", "a b.txt", "[ab].txt", "gone.txt"];
    for name in names {
        repo.write(name, "base\n");
    }
    repo.write("notes.txt", "notes\n");
    repo.commit_all("base");
    repo.branch("feature");
    repo.checkout("feature");
    for name in names {
        repo.write(name, "feature\n");
    }
    repo.commit_all("feature side");
    repo.checkout("main");
    for name in &names[..3] {
        repo.write(name, "main\n");
    }
    repo.git(&["rm", "-q", "--", "gone.txt"]);
    repo.commit_all("main side");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, merge_branch("feature")).await;
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    let stopped = out
        .snapshot
        .in_progress
        .expect("the outcome shows the merge");
    assert_eq!(stopped.kind, InProgressKind::Merge);
    assert_eq!(
        stopped.paths(),
        ["[ab].txt", "a b.txt", "clash.txt", "gone.txt"]
    );

    // Resolved by hand: three edited, the one deleted on main deleted. Beside them, an
    // untracked file a glob `[ab].txt` would match, and an unrelated change.
    for name in &names[..3] {
        repo.write(name, "resolved\n");
    }
    fs::remove_file(repo.path().join("gone.txt")).unwrap();
    repo.write("a.txt", "untracked\n");
    repo.write("notes.txt", "edited, not part of the merge\n");

    // The file deleted on one side is git's to keep one side of: only the user can say it
    // is resolved (they go ahead past the warning).
    let refused = h.run(id, continue_op(false)).await;
    assert_eq!(refused.status, OpStatus::Failed, "{refused:?}");
    assert!(refused.message.contains("gone.txt"), "{}", refused.message);
    let out = h.run(id, continue_op(true)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Continue the merge");
    assert_eq!(out.snapshot.in_progress, None);
    assert!(!in_git_dir(&repo, "MERGE_HEAD"));
    assert_eq!(
        args_of(&h, &out),
        [
            vec![
                "--literal-pathspecs",
                "add",
                "--",
                "[ab].txt",
                "a b.txt",
                "clash.txt",
                "gone.txt"
            ],
            vec!["merge", "--continue"],
        ]
    );
    assert_eq!(repo.git(&["rev-list", "--count", "--merges", "HEAD"]), "1");
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]),
        "Merge branch 'feature'"
    );
    assert_eq!(repo.git(&["show", "HEAD:clash.txt"]), "resolved");
    assert_eq!(
        repo.git(&["ls-tree", "--name-only", "HEAD", "--", "gone.txt"]),
        ""
    );
    assert_eq!(repo.git(&["show", "HEAD:notes.txt"]), "notes");
    assert_eq!(
        repo.git(&["status", "--porcelain"]),
        "M notes.txt\n?? a.txt",
        "the unrelated change and the untracked file are left alone"
    );
}

/// `main` and `feature` both change `clash.txt` and `other.txt`; the merge of `feature`
/// stopped on both. Returns the harness and the repository's id.
async fn merge_stopped_on_two_files(repo: &TestRepo) -> (Harness, u32) {
    for name in ["clash.txt", "other.txt"] {
        repo.write(name, "base\n");
    }
    repo.commit_all("base");
    repo.branch("feature");
    repo.checkout("feature");
    for name in ["clash.txt", "other.txt"] {
        repo.write(name, "feature\n");
    }
    repo.commit_all("feature side");
    repo.checkout("main");
    for name in ["clash.txt", "other.txt"] {
        repo.write(name, "main\n");
    }
    repo.commit_all("main side");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(
        h.run(id, merge_branch("feature")).await.status,
        OpStatus::Stopped
    );
    (h, id)
}

/// Continue never stages a file that still has conflict markers unless told to: it is
/// refused, and nothing is staged. The refusal comes with the repository read again:
/// the file resolved meanwhile (nothing git watches changed) shows as resolved.
#[tokio::test]
async fn continue_is_refused_while_a_file_has_conflict_markers() {
    let repo = TestRepo::new();
    let (h, id) = merge_stopped_on_two_files(&repo).await;
    repo.write("clash.txt", "resolved\n");

    let out = h.run(id, continue_op(false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.contains("other.txt") && out.message.contains("conflict markers"),
        "{}",
        out.message
    );
    assert!(args_of(&h, &out).is_empty(), "nothing ran");
    assert!(in_git_dir(&repo, "MERGE_HEAD"), "still merging");
    assert_eq!(
        repo.git(&["diff", "--name-only", "--diff-filter=U"]),
        "clash.txt\nother.txt",
        "nothing staged"
    );
    let now = out.snapshot.in_progress.expect("still merging");
    assert_eq!(
        now.files.iter().map(|f| f.state).collect::<Vec<_>>(),
        [ConflictState::Resolved, ConflictState::Markers],
        "read again: {now:?}"
    );
}

/// A file git could not merge and kept one side of (binary, or deleted on one side) is not
/// taken for resolved: continue is refused without the user's go-ahead, saying which, and
/// with it stages what is in the folder.
#[tokio::test]
async fn continue_is_refused_while_git_kept_one_side_of_a_file() {
    let repo = TestRepo::new();
    repo.write("image.bin", "base\0");
    repo.write("gone.txt", "base\n");
    repo.commit_all("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("image.bin", "feature\0");
    repo.write("gone.txt", "feature\n");
    repo.commit_all("feature side");
    repo.checkout("main");
    repo.write("image.bin", "main\0");
    repo.git(&["rm", "-q", "--", "gone.txt"]);
    repo.commit_all("main side");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, merge_branch("feature")).await;
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    let files = out.snapshot.in_progress.expect("merging").files;
    assert!(
        files.iter().all(|f| f.state == ConflictState::Undecided),
        "{files:?}"
    );

    let out = h.run(id, continue_op(false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.contains("gone.txt") && out.message.contains("image.bin"),
        "{}",
        out.message
    );
    assert!(args_of(&h, &out).is_empty(), "nothing ran");

    let out = h.run(id, continue_op(true)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["show", "HEAD:image.bin"]), "main\0".trim_end());
    assert_eq!(repo.git(&["show", "HEAD:gone.txt"]), "feature");
}

/// Told to, continue stages files with conflict markers as they are (a line that only
/// looks like a marker must never trap the user).
#[tokio::test]
async fn continue_even_with_markers_stages_them_as_they_are() {
    let repo = TestRepo::new();
    let (h, id) = merge_stopped_on_two_files(&repo).await;
    repo.write("clash.txt", "resolved\n");
    repo.write("other.txt", "kept\n=======\nas written\n");

    let out = h.run(id, continue_op(true)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(!in_git_dir(&repo, "MERGE_HEAD"), "the merge is committed");
    assert_eq!(out.snapshot.in_progress, None);
    assert_eq!(
        repo.git(&["show", "HEAD:other.txt"]),
        "kept\n=======\nas written"
    );
    assert_eq!(repo.git(&["show", "HEAD:clash.txt"]), "resolved");
}

#[tokio::test]
async fn continue_commits_a_merge_stopped_without_conflicts() {
    let repo = TestRepo::new();
    repo.commit("base");
    repo.branch("side");
    repo.checkout("side");
    repo.commit("on side");
    repo.checkout("main");
    repo.commit("on main");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run(
            id,
            merge(MergeTargetKind::Branch, "side", true, false, true),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    let progress = out.snapshot.in_progress.expect("merge in progress");
    assert_eq!(progress.kind, InProgressKind::Merge);
    assert!(progress.files.is_empty());

    let out = h.run(id, continue_op(false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(args_of(&h, &out), [vec!["merge", "--continue"]]);
    assert_eq!(out.snapshot.in_progress, None);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]),
        "Merge branch 'side'"
    );
}

#[tokio::test]
async fn continue_rebase_that_stops_again_returns_stopped() {
    let repo = TestRepo::new();
    clashing(&repo);
    repo.checkout("feature");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, rebase("main")).await;
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    let p = out.snapshot.in_progress.expect("rebase in progress");
    assert_eq!(
        (p.kind, p.step, p.total),
        (InProgressKind::Rebase, Some(1), Some(3))
    );

    repo.write("clash.txt", "resolved one\n");
    let out = h.run(id, continue_op(false)).await;
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    assert_eq!(out.title, "Continue the rebase");
    assert!(out.message.contains("conflicts"), "{}", out.message);
    assert!(
        out.detail.as_deref().unwrap_or("").contains("CONFLICT"),
        "{out:?}"
    );
    let p = out.snapshot.in_progress.clone().expect("still in progress");
    assert_eq!(p.kind, InProgressKind::Rebase);
    assert_eq!((p.step, p.total), (Some(2), Some(3)));
    assert_eq!(p.paths(), ["clash.txt"]);
    assert_eq!(p.subject.as_deref(), Some("feat two"));
    assert!(
        args_of(&h, &out)
            .iter()
            .any(|a| a == &["rebase", "--continue"]),
        "{:?}",
        args_of(&h, &out)
    );

    repo.write("clash.txt", "resolved two\n");
    assert_eq!(
        h.run(id, continue_op(false)).await.status,
        OpStatus::Stopped
    );
    repo.write("clash.txt", "resolved three\n");
    let out = h.run(id, continue_op(false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.snapshot.in_progress, None);
    assert_eq!(current_branch(&repo).as_deref(), Some("feature"));
    assert_eq!(read(&repo, "clash.txt"), "resolved three\n");
    assert_eq!(repo.git(&["rev-list", "--count", "main..feature"]), "3");
}

#[tokio::test]
async fn continue_finishes_a_cherry_pick_and_a_revert() {
    let repo = TestRepo::new();
    clashing(&repo);
    let one = repo.git(&["rev-parse", "feature~2"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, cherry_pick(&one)).await;
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    repo.write("clash.txt", "picked\n");
    let out = h.run(id, continue_op(false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Continue the cherry-pick");
    assert_eq!(out.snapshot.in_progress, None);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "feat one");
    assert!(args_of(&h, &out).contains(&vec![
        "cherry-pick".to_owned(),
        "--continue".to_owned(),
        "--no-edit".to_owned()
    ]));

    let out = h.run(id, revert(&repo.git(&["rev-parse", "HEAD~1"]))).await;
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    repo.write("clash.txt", "reverted\n");
    let out = h.run(id, continue_op(false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.snapshot.in_progress, None);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]),
        "Revert \"main side\""
    );
}

/// A resolution that leaves the commit with no changes: git will not continue it, and the
/// core says so plainly, with git's text.
#[tokio::test]
async fn continue_that_comes_out_empty_explains() {
    let repo = TestRepo::new();
    clashing(&repo);
    let one = repo.git(&["rev-parse", "feature~2"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(h.run(id, cherry_pick(&one)).await.status, OpStatus::Stopped);
    repo.write("clash.txt", "main\n");

    let out = h.run(id, continue_op(false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("empty"), "{}", out.message);
    assert!(out.message.contains("abort"), "{}", out.message);
    assert!(
        out.detail.as_deref().unwrap_or("").contains("now empty"),
        "{out:?}"
    );
    let p = out.snapshot.in_progress.expect("still in progress");
    assert_eq!(p.kind, InProgressKind::CherryPick);
    assert!(p.files.is_empty());
    // The core says the step is empty, and which commit it is.
    assert!(p.empty, "{p:?}");
    assert_eq!(p.commit.as_deref(), Some(one.as_str()));
    // The notice offers the way past it: skipping that commit.
    assert_eq!(
        out.hints,
        vec![OpHint::Skip {
            commit: one.clone()
        }]
    );

    let head = repo.git(&["rev-parse", "HEAD"]);
    let skipped = h.run(id, skip(&one)).await;
    assert_eq!(skipped.status, OpStatus::Done, "{skipped:?}");
    assert_eq!(skipped.title, "Skip a commit of the cherry-pick");
    assert!(skipped.snapshot.in_progress.is_none(), "{skipped:?}");
    assert_eq!(
        repo.git(&["rev-parse", "HEAD"]),
        head,
        "nothing was committed"
    );
    assert_eq!(read(&repo, "clash.txt"), "main\n");
}

/// Skip goes on with a rebase past the commit it stopped on; a merge has nothing to skip.
#[tokio::test]
async fn skip_moves_a_rebase_on_and_is_refused_for_a_merge() {
    let repo = TestRepo::new();
    clashing(&repo);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let merge = OpRequest::Merge {
        target: MergeTarget {
            kind: MergeTargetKind::Branch,
            name: "feature".into(),
        },
        no_ff: false,
        squash: false,
        no_commit: false,
        expect_head: here(),
        expect_worktree: here_wt(),
    };
    assert_eq!(h.run(id, merge).await.status, OpStatus::Stopped);
    let any = repo.git(&["rev-parse", "feature"]);
    let refused = h.run(id, skip(&any)).await;
    assert_eq!(refused.status, OpStatus::Failed, "{refused:?}");
    assert!(refused.message.contains("merge"), "{}", refused.message);
    assert!(refused.snapshot.in_progress.is_some(), "the merge stays");
    assert_eq!(
        h.run_confirmed(id, abort(), &[DangerKind::Abort])
            .await
            .status,
        OpStatus::Done
    );

    let rebase = OpRequest::Rebase {
        onto: "feature".into(),
        expect_head: here(),
        expect_worktree: here_wt(),
    };
    let stopped = h.run(id, rebase).await;
    assert_eq!(stopped.status, OpStatus::Stopped);
    // Stopped on main's own commit; nothing in the tree beyond its conflict.
    let main_side = repo.git(&["rev-parse", "main"]);
    let p = stopped.snapshot.in_progress.expect("stopped");
    assert_eq!(p.commit.as_deref(), Some(main_side.as_str()));
    assert!(!p.empty, "it has a conflict");
    let out = h.run(id, skip(&main_side)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Skip a commit of the rebase");
    // Its only commit left out, main is where feature is, and nothing is in progress.
    assert!(out.snapshot.in_progress.is_none(), "{out:?}");
    assert_eq!(
        repo.git(&["rev-parse", "main"]),
        repo.git(&["rev-parse", "feature"])
    );
    assert_eq!(current_branch(&repo).as_deref(), Some("main"));
    assert_eq!(read(&repo, "clash.txt"), "feature 2\n");
}

/// A skip is for the commit git was stopped on when it was offered. Once git has moved
/// on (here, the user skipped it in a terminal and git stopped on the next), the skip is
/// refused and nothing runs.
#[tokio::test]
async fn a_skip_for_another_commit_is_refused() {
    let repo = TestRepo::new();
    clashing(&repo);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    repo.checkout("feature");
    repo.git(&["branch", "-q", "-f", "target", "main"]);
    let rebase = OpRequest::Rebase {
        onto: "target".into(),
        expect_head: here(),
        expect_worktree: here_wt(),
    };
    let stopped = h.run(id, rebase).await;
    assert_eq!(stopped.status, OpStatus::Stopped, "{stopped:?}");
    let first = stopped.snapshot.in_progress.unwrap().commit.unwrap();
    assert_eq!(first, repo.git(&["rev-parse", "feature~2"]));

    // Skipped in a terminal: git stops on the next commit.
    assert!(!repo.git_succeeds(&["rebase", "--skip"]));
    let now = repo.git(&["rev-parse", "REBASE_HEAD"]);
    assert_ne!(now, first);
    let mark = h.last_log_id();
    let out = h.run(id, skip(&first)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message
            .starts_with("Nothing was skipped: git has moved on"),
        "{}",
        out.message
    );
    assert!(h.writes_since(mark).is_empty());
    assert_eq!(repo.git(&["rev-parse", "REBASE_HEAD"]), now);
}

/// The repository looked at again (as the UI asks when the user comes back to it), once its
/// working tree has been read.
async fn looked_again(h: &Harness, id: u32) -> gg_core::dto::RepoSnapshot {
    h.m.refresh(id).await.unwrap();
    tokio::time::timeout(TIMEOUT, h.m.wait_status(id))
        .await
        .expect("status read");
    h.m.snapshot(id).await.unwrap()
}

/// Skipping a rebase's commit puts every tracked file back as HEAD has it. An edit
/// made after the step came out empty survives unless the user confirms that it goes; the
/// core reports the empty step (read-only), and the skip is done once confirmed.
#[tokio::test]
async fn a_rebase_skip_that_would_discard_other_changes_asks_first() {
    let repo = TestRepo::new();
    repo.write("notes.txt", "notes\n");
    clashing(&repo);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let rebase = OpRequest::Rebase {
        onto: "feature".into(),
        expect_head: here(),
        expect_worktree: here_wt(),
    };
    assert_eq!(h.run(id, rebase).await.status, OpStatus::Stopped);
    let main_side = repo.git(&["rev-parse", "main"]);
    // Resolved to what HEAD has: the step has no changes left.
    repo.write("clash.txt", "feature 2\n");
    repo.git(&["add", "clash.txt"]);
    let snap = looked_again(&h, id).await;
    let p = snap.in_progress.expect("stopped");
    assert!(p.empty, "{p:?}");
    assert_eq!(p.commit.as_deref(), Some(main_side.as_str()));

    // An edit made afterwards, to a file the step does not touch.
    repo.write("notes.txt", "my own edit\n");
    let snap = looked_again(&h, id).await;
    assert!(!snap.in_progress.unwrap().empty);
    let mark = h.last_log_id();
    let asked = h.run(id, skip(&main_side)).await;
    assert_eq!(asked.status, OpStatus::NeedsConfirmation, "{asked:?}");
    assert_eq!(asked.dangers[0].kind, DangerKind::SkipDiscards);
    assert_eq!(asked.dangers[0].detail.as_deref(), Some("notes.txt"));
    assert!(h.writes_since(mark).is_empty());
    assert_eq!(read(&repo, "notes.txt"), "my own edit\n");

    let out = h
        .run_confirmed(id, skip(&main_side), &[DangerKind::SkipDiscards])
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(out.snapshot.in_progress.is_none());
    assert_eq!(
        repo.git(&["rev-parse", "main"]),
        repo.git(&["rev-parse", "feature"])
    );
    assert_eq!(read(&repo, "notes.txt"), "notes\n");
}

/// A cherry-pick's skip keeps changes it does not touch (git's own), so it asks nothing.
#[tokio::test]
async fn a_cherry_pick_skip_keeps_other_changes_without_asking() {
    let repo = TestRepo::new();
    repo.write("notes.txt", "notes\n");
    clashing(&repo);
    let one = repo.git(&["rev-parse", "feature~2"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(h.run(id, cherry_pick(&one)).await.status, OpStatus::Stopped);
    repo.write("clash.txt", "main\n");
    repo.git(&["add", "clash.txt"]);
    repo.write("notes.txt", "my own edit\n");
    let out = h.run(id, skip(&one)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(read(&repo, "notes.txt"), "my own edit\n");
}

#[tokio::test]
async fn continue_with_nothing_in_progress_is_nothing_to_do() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let mark = h.last_log_id();
    let events = h.changes(id).len();

    for request in [continue_op(false), abort()] {
        let err =
            h.m.run_op(id, in_wt(repo.path(), request.clone()), &[])
                .await
                .expect_err("nothing to continue or abort");
        assert_eq!(err.code, ErrorCode::NothingToDo, "{request:?}");
        assert!(err.message.contains("in progress"), "{}", err.message);
    }
    assert!(h.writes_since(mark).is_empty());
    assert_eq!(h.changes(id).len(), events, "nothing was published");
}

// ---- What comes in ----

/// A merge or rebase the app started names what comes in as the user asked for it, even
/// where another ref points at the same commit (`feature` sorts before `feature-copy`).
#[tokio::test]
async fn what_comes_in_is_named_as_the_app_asked_for_it() {
    let repo = TestRepo::new();
    clashing(&repo);
    repo.git(&["branch", "feature-copy", "feature"]);
    repo.git(&["branch", "main-copy", "main"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, merge_branch("feature-copy")).await;
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    let merging = out.snapshot.in_progress.expect("merging");
    assert_eq!(merging.incoming.as_deref(), Some("feature-copy"));
    assert_eq!(merging.rebasing, None);
    assert_eq!(
        h.run_confirmed(id, abort(), &[DangerKind::Abort])
            .await
            .status,
        OpStatus::Done
    );

    repo.checkout("feature");
    h.m.refresh(id).await.unwrap();
    let out = h
        .run(
            id,
            OpRequest::Rebase {
                onto: "main-copy".into(),
                expect_head: here(),
                expect_worktree: here_wt(),
            },
        )
        .await;
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    let rebasing = out.snapshot.in_progress.expect("rebasing");
    assert_eq!(rebasing.incoming.as_deref(), Some("main-copy"));
    assert_eq!(rebasing.rebasing.as_deref(), Some("feature"));

    // Once it has ended, what the app asked for is forgotten: started outside the app, the
    // same commit is named by the first ref at it.
    assert_eq!(
        h.run_confirmed(id, abort(), &[DangerKind::Abort])
            .await
            .status,
        OpStatus::Done
    );
    assert!(!repo.git_succeeds(&["rebase", "main-copy"]));
    h.m.refresh(id).await.unwrap();
    tokio::time::timeout(TIMEOUT, h.m.wait_status(id))
        .await
        .unwrap();
    let snap = h.m.snapshot(id).await.unwrap();
    let outside = snap.in_progress.expect("rebasing");
    assert_eq!(outside.incoming.as_deref(), Some("main"));
}

// ---- Abort ----

#[tokio::test]
async fn abort_restores_the_branch() {
    for kind in ["merge", "rebase", "cherry-pick", "revert"] {
        let repo = TestRepo::new();
        clashing(&repo);
        if kind == "rebase" || kind == "revert" {
            repo.checkout("feature");
        }
        let branch = current_branch(&repo).unwrap();
        let tip = repo.git(&["rev-parse", "HEAD"]);
        let h = harness().await;
        let id = h.open(repo.path()).await.repo_id;
        let request = match kind {
            "merge" => merge_branch("feature"),
            "rebase" => rebase("main"),
            "cherry-pick" => cherry_pick(&repo.git(&["rev-parse", "feature~2"])),
            _ => revert(&repo.git(&["rev-parse", "feature~2"])),
        };
        let out = h.run(id, request).await;
        assert_eq!(out.status, OpStatus::Stopped, "{kind}: {out:?}");

        let out = h.run_confirmed(id, abort(), &[DangerKind::Abort]).await;
        assert_eq!(out.status, OpStatus::Done, "{kind}: {out:?}");
        assert_eq!(out.title, format!("Abort the {kind}"));
        assert!(out.message.contains(&branch), "{kind}: {}", out.message);
        assert_eq!(args_of(&h, &out), [vec![kind, "--abort"]], "{kind}");
        assert_eq!(out.snapshot.in_progress, None, "{kind}");
        assert_eq!(current_branch(&repo).as_deref(), Some(branch.as_str()));
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), tip, "{kind}");
        assert_eq!(repo.git(&["status", "--porcelain"]), "", "{kind}");
    }
}

/// `main` and `feature` both change `clash.txt` after `base`, and `feature` changes
/// `other.txt`; on `main`, `other.txt` and `notes.txt` are changed and not committed. HEAD on
/// `main`.
fn clashing_with_changes(repo: &TestRepo) {
    repo.write("clash.txt", "base\n");
    repo.write("other.txt", "base\n");
    repo.write("notes.txt", "base\n");
    repo.commit_all("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("clash.txt", "feature\n");
    repo.write("other.txt", "feature\n");
    repo.commit_all("feature side");
    repo.checkout("main");
    repo.write("clash.txt", "main\n");
    repo.commit_all("main side");
    repo.write("other.txt", "mine\n");
    repo.write("notes.txt", "mine\n");
}

/// A rebase with `rebase.autoStash` stops on a conflict; once it is continued or the commit
/// skipped, git puts the auto-stashed changes back, they conflict with the result (`other.txt`),
/// and git keeps them in the stash and exits 0: the outcome says so. With
/// git's words ignored, the stash's state alone tells it.
#[tokio::test]
async fn continue_and_skip_say_when_auto_stashed_changes_stay_in_the_stash() {
    for (how, by_state) in [("continue", false), ("skip", false), ("continue", true)] {
        let repo = TestRepo::new();
        clashing_with_changes(&repo);
        repo.git(&["config", "rebase.autoStash", "true"]);
        let h = harness().await;
        let id = h.open(repo.path()).await.repo_id;
        let main_side = repo.git(&["rev-parse", "main"]);
        let out = h.run(id, rebase("feature")).await;
        assert_eq!(out.status, OpStatus::Stopped, "{how}: {out:?}");

        gg_core::testutil::AUTOSTASH_BY_STATE_ONLY
            .store(by_state, std::sync::atomic::Ordering::Relaxed);
        let out = if how == "continue" {
            repo.write("clash.txt", "resolved\n");
            h.run(id, continue_op(false)).await
        } else {
            h.run(id, skip(&main_side)).await
        };
        gg_core::testutil::AUTOSTASH_BY_STATE_ONLY
            .store(false, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(out.status, OpStatus::Done, "{how}: {out:?}");
        assert!(out.snapshot.in_progress.is_none(), "{how}");
        assert!(
            out.message.contains("kept in the stash"),
            "{how} ({by_state}): {}",
            out.message
        );
        assert!(repo.git(&["stash", "list"]).contains("autostash"), "{how}");
    }
}

/// `merge --abort` puts auto-stashed changes back too: when a file they change was edited
/// meanwhile, git cannot, keeps them in the stash, and the abort says so instead of "back as
/// it was". An abort whose auto-stash comes back cleanly says nothing of it.
#[tokio::test]
async fn abort_says_when_auto_stashed_changes_stay_in_the_stash() {
    for edited_meanwhile in [true, false] {
        let repo = TestRepo::new();
        clashing_with_changes(&repo);
        repo.git(&["config", "merge.autoStash", "true"]);
        let h = harness().await;
        let id = h.open(repo.path()).await.repo_id;
        let out = h.run(id, merge_branch("feature")).await;
        assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
        if edited_meanwhile {
            repo.write("notes.txt", "edited while stopped\n");
        }
        let out = h.run_confirmed(id, abort(), &[DangerKind::Abort]).await;
        assert_eq!(out.status, OpStatus::Done, "{edited_meanwhile}: {out:?}");
        assert_eq!(
            out.message.contains("kept in the stash"),
            edited_meanwhile,
            "{edited_meanwhile}: {}",
            out.message
        );
        assert_eq!(
            repo.git(&["stash", "list"]).contains("autostash"),
            edited_meanwhile
        );
    }
}

// ---- Other operations meanwhile ----

#[tokio::test]
async fn other_operations_explain_while_in_progress() {
    let repo = TestRepo::new();
    clashing(&repo);
    let tip = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(
        h.run(id, merge_branch("feature")).await.status,
        OpStatus::Stopped
    );
    let mark = h.last_log_id();
    let events = h.changes(id).len();

    let one = repo.git(&["rev-parse", "feature~2"]);
    for request in [
        checkout_branch("feature"),
        create_branch_and_checkout("later", &tip),
        merge_branch("feature"),
        rebase("feature"),
        cherry_pick(&one),
        revert(&tip),
        OpRequest::Pull {
            branch: "main".into(),
            mode: gg_core::dto::PullMode::Default,
            expect_head: here(),
            expect_worktree: here_wt(),
        },
    ] {
        let out = h.run(id, request.clone()).await;
        assert_eq!(out.status, OpStatus::Failed, "{request:?}: {out:?}");
        assert!(
            out.message.contains("A merge is in progress"),
            "{}",
            out.message
        );
        assert!(
            out.message.contains("continue it or abort it"),
            "{}",
            out.message
        );
        assert_eq!((out.log_from, out.log_to), (0, 0));
    }
    assert!(h.writes_since(mark).is_empty(), "nothing ran");
    assert_eq!(h.changes(id).len(), events, "nothing was published");
    assert_eq!(current_branch(&repo).as_deref(), Some("main"));
    assert!(in_git_dir(&repo, "MERGE_HEAD"));

    // What leaves HEAD and the working tree alone still runs.
    let out = h.run(id, create_branch("later", &tip)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(out.snapshot.in_progress.is_some());

    // So does a reset (a way out of a merge), a hard one still only once confirmed.
    let hard = reset(&tip, gg_core::dto::ResetMode::Hard);
    let out = h.run(id, hard.clone()).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    let out = h
        .run_confirmed(id, hard, &[gg_core::dto::DangerKind::HardReset])
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.snapshot.in_progress, None);
    assert!(!in_git_dir(&repo, "MERGE_HEAD"));
}

// ---- Open in editor ----

#[tokio::test]
async fn open_in_editor_rejects_paths_outside_the_worktree() {
    let repo = TestRepo::new();
    repo.write("a b.txt", "x\n");
    repo.write("dir/inner.txt", "x\n");
    repo.commit_all("files");
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.txt"), "x").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), repo.path().join("link")).unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.record_launches();

    let escaping = outside.path().join("secret.txt");
    let escaping = escaping.to_str().unwrap();
    for path in [
        "../secret.txt",
        "dir/../../secret.txt",
        escaping,
        "",
        #[cfg(unix)]
        "link/secret.txt",
    ] {
        let err =
            h.m.open_in_editor(id, Some(path), None)
                .await
                .expect_err(path);
        assert_eq!(err.code, ErrorCode::BadRequest, "{path}: {err:?}");
    }
    let err =
        h.m.open_in_editor(id, Some("missing.txt"), None)
            .await
            .expect_err("missing");
    assert_eq!(err.code, ErrorCode::PathMissing, "{err:?}");
    assert!(h.m.launches().is_empty(), "nothing was opened");

    let root = dunce::canonicalize(repo.path()).unwrap();
    let editor = Some("my-editor");
    h.m.open_in_editor(id, Some("a b.txt"), editor)
        .await
        .unwrap();
    h.m.open_in_editor(id, Some("dir/./inner.txt"), editor)
        .await
        .unwrap();
    h.m.open_in_editor(id, None, editor).await.unwrap();
    let opened: Vec<String> =
        h.m.launches()
            .into_iter()
            .map(|argv| argv.last().cloned().unwrap())
            .collect();
    let at = |rel: &Path| root.join(rel).to_string_lossy().into_owned();
    assert_eq!(
        opened,
        [
            at(Path::new("a b.txt")),
            at(&Path::new("dir").join("inner.txt")),
            root.to_string_lossy().into_owned(),
        ]
    );
}

#[tokio::test]
async fn editor_command_is_split_without_a_shell() {
    let repo = TestRepo::new();
    repo.write("a.txt", "x\n");
    repo.commit_all("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.record_launches();
    let file = dunce::canonicalize(repo.path().join("a.txt"))
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let command = r#"my-editor  --wait 'two words' "$(touch pwned)" ; rm -rf x"#;
    h.m.open_in_editor(id, Some("a.txt"), Some(command))
        .await
        .unwrap();
    // Without a command: as text on macOS; elsewhere the worktree folder, with a note
    // (the file itself could be a program the default application runs).
    let note = h.m.open_in_editor(id, Some("a.txt"), None).await.unwrap();
    let root = dunce::canonicalize(repo.path()).unwrap();
    let root = root.to_string_lossy().into_owned();
    let expected_opener: Vec<String> = if cfg!(target_os = "macos") {
        assert_eq!(note, None);
        vec!["/usr/bin/open".into(), "-t".into(), file.clone()]
    } else {
        assert!(
            note.as_deref().unwrap_or("").contains("Settings"),
            "{note:?}"
        );
        if cfg!(windows) {
            // Explorer reads its own command line: the folder is given quoted, as written.
            let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
            vec![format!(r"{windows}\explorer.exe"), format!("\"{root}\"")]
        } else {
            vec!["xdg-open".to_owned(), root]
        }
    };
    assert_eq!(
        h.m.launches(),
        [
            vec![
                "my-editor".to_owned(),
                "--wait".to_owned(),
                "two words".to_owned(),
                "$(touch pwned)".to_owned(),
                ";".to_owned(),
                "rm".to_owned(),
                "-rf".to_owned(),
                "x".to_owned(),
                file,
            ],
            expected_opener,
        ]
    );

    let err =
        h.m.open_in_editor(id, Some("a.txt"), Some("code 'unclosed"))
            .await
            .expect_err("unclosed quote");
    assert_eq!(err.code, ErrorCode::BadRequest, "{err:?}");
}

/// For real: the program runs directly, so `$(…)` and `;` reach it as arguments (here
/// `touch` makes files of those names), and the call returns without waiting for it.
#[cfg(unix)]
#[tokio::test]
async fn editor_runs_detached_without_a_shell() {
    let repo = TestRepo::new();
    repo.write("a.txt", "x\n");
    repo.commit_all("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.open_in_editor(id, Some("a.txt"), Some("touch '$(echo hi)' ;"))
        .await
        .unwrap();
    let made = |name: &str| repo.path().join(name).exists();
    let deadline = std::time::Instant::now() + TIMEOUT;
    while !(made("$(echo hi)") && made(";")) {
        assert!(std::time::Instant::now() < deadline, "touch ran");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(!made("hi"));

    let err =
        h.m.open_in_editor(id, Some("a.txt"), Some("gg-no-such-editor --wait"))
            .await
            .expect_err("no such program");
    assert!(err.message.contains("gg-no-such-editor"), "{err:?}");
    assert!(err.message.contains("Settings"), "{err:?}");
}

/// A cherry-pick sequence whose current commit the user committed by hand: continue goes
/// on with the rest; abort ends it, and says that git keeps the commit made by hand.
#[tokio::test]
async fn a_sequence_committed_by_hand_continues_and_aborts() {
    for aborting in [false, true] {
        let repo = TestRepo::new();
        repo.write("clash.txt", "base\n");
        repo.commit_all("base");
        repo.branch("feature");
        repo.checkout("feature");
        repo.write("clash.txt", "feature\n");
        repo.commit_all("feat clash");
        repo.write("other.txt", "other\n");
        repo.commit_all("feat other");
        repo.checkout("main");
        repo.write("clash.txt", "main\n");
        let before = repo.commit_all("main side");
        assert!(!repo.git_succeeds(&["cherry-pick", "feature~1", "feature"]));
        repo.write("clash.txt", "resolved\n");
        repo.git(&["add", "clash.txt"]);
        repo.git(&["commit", "-q", "--no-edit"]);
        let h = harness().await;
        let snap = h.open(repo.path()).await;
        let id = snap.repo_id;
        assert_eq!(
            snap.in_progress.map(|p| p.kind),
            Some(InProgressKind::CherryPick)
        );
        let out = h
            .run_confirmed(
                id,
                if aborting {
                    abort()
                } else {
                    continue_op(false)
                },
                &[DangerKind::Abort],
            )
            .await;
        assert_eq!(out.status, OpStatus::Done, "{out:?}");
        assert!(out.snapshot.in_progress.is_none());
        assert!(!in_git_dir(&repo, "sequencer"));
        if aborting {
            // git does not rewind over a commit made by hand, and the message says so.
            assert_eq!(repo.git(&["rev-parse", "HEAD~1"]), before);
            assert!(
                out.message.contains("without going back"),
                "{}",
                out.message
            );
        } else {
            assert_eq!(read(&repo, "other.txt"), "other\n");
            assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "feat other");
        }
    }
}
