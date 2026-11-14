//! Integration tests for R39: a confirmation, and a request on the branch checked out, act
//! only on the state the user saw. Each danger carries a token of that state, and a request
//! on the current branch carries HEAD as the UI showed it; between the confirmation and the
//! run (a queued operation, a terminal) the state can change, and then nothing runs: the core
//! asks again (another token) or refuses with a plain line (another HEAD).

use std::fs;

use gg_core::dto::{
    Confirmed, DangerKind, HeadInfo, MergeTargetKind, OpOutcome, OpRequest, OpStatus, ResetMode,
};
use gg_core::testutil::TestRepo;

mod common;
use common::*;

fn confirmed(out: &OpOutcome) -> Vec<Confirmed> {
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    out.dangers.iter().map(Confirmed::from).collect()
}

fn reset_hard(hash: &str, expect_head: HeadInfo) -> OpRequest {
    OpRequest::Reset {
        hash: hash.to_owned(),
        mode: ResetMode::Hard,
        expect_head,
    }
}

/// The reviewer's scenario (C2): Reset main to here (hard) is asked for and its confirmation
/// shown; meanwhile a queued checkout switches to `feature`, which has uncommitted changes.
/// The confirmation then arrives: it was for main, so nothing is reset, and feature's tip and
/// changes are as they were.
#[tokio::test]
async fn a_hard_reset_confirmed_for_main_does_not_run_on_the_branch_checked_out_since() {
    let repo = TestRepo::new();
    let first = repo.commit("one");
    repo.commit("two on main");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("feature.txt", "feature\n");
    let feature_tip = repo.commit_all("on feature");
    repo.checkout("main");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let on_main = head_at(repo.path());
    let asked = h.run(id, reset_hard(&first, on_main.clone())).await;
    assert_eq!(asked.title, format!("Reset main to {} (hard)", &first[..8]));
    let yes = confirmed(&asked);

    // The queued checkout runs; feature gets an uncommitted change.
    let out = h.run(id, checkout_branch("feature")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    repo.write("feature.txt", "work in progress\n");

    let mark = h.last_log_id();
    let out = h.run_with(id, reset_hard(&first, on_main), &yes).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert_eq!(
        out.message,
        "Nothing was done: it was asked for on main, but feature is checked out now. Look at \
         it again, and ask again if it still applies."
    );
    assert!(
        h.writes_since(mark).is_empty(),
        "{:?}",
        h.writes_since(mark)
    );
    assert_eq!(repo.git(&["rev-parse", "feature"]), feature_tip);
    assert_eq!(
        fs::read_to_string(repo.path().join("feature.txt")).unwrap(),
        "work in progress\n"
    );

    // With feature's HEAD (the UI shows it now) but main's confirmation: asked again, about
    // feature's changes.
    let out = h.run_with(id, reset_hard(&first, here()), &yes).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert!(
        out.dangers[0]
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("feature.txt")),
        "{out:?}"
    );
    assert!(h.writes_since(mark).is_empty());
    assert_eq!(repo.git(&["rev-parse", "feature"]), feature_tip);
}

/// The confirmation names the files a hard reset discards: one changed after it was shown
/// makes the core ask again, listing it, and nothing is reset until that is confirmed.
#[tokio::test]
async fn a_hard_reset_confirmed_before_more_files_changed_is_asked_again() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    let first = repo.commit_all("one");
    repo.commit("two");
    repo.write("a.txt", "edited\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let asked = h.run(id, reset_hard(&first, here())).await;
    let yes = confirmed(&asked);
    assert!(
        !asked.dangers[0]
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("b.txt")
    );

    repo.write("b.txt", "edited later\n");
    let mark = h.last_log_id();
    let out = h.run_with(id, reset_hard(&first, here()), &yes).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert!(
        out.dangers[0]
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("b.txt")),
        "{out:?}"
    );
    assert!(h.writes_since(mark).is_empty());
    assert_eq!(
        fs::read_to_string(repo.path().join("b.txt")).unwrap(),
        "edited later\n"
    );

    // Confirmed as it is now, it runs.
    let yes = confirmed(&out);
    let out = h.run_with(id, reset_hard(&first, here()), &yes).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), first);
}

/// A reset (any mode) asked for with main at one commit does not run once main has moved
/// (a commit made in a terminal): what it would undo is not what the user saw.
#[tokio::test]
async fn a_reset_does_not_run_once_the_branch_has_moved() {
    let repo = TestRepo::new();
    let first = repo.commit("one");
    repo.commit("two");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let seen = head_at(repo.path());
    let moved = repo.commit("made in a terminal");
    let out = h
        .run(
            id,
            OpRequest::Reset {
                hash: first.clone(),
                mode: ResetMode::Soft,
                expect_head: seen,
            },
        )
        .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message
            .starts_with("Nothing was done: main has moved since"),
        "{}",
        out.message
    );
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), moved);
}

/// A cherry-pick asked for on main does not land on the branch checked out since (I2);
/// several asked for one after another on the same branch all run.
#[tokio::test]
async fn a_cherry_pick_runs_on_the_branch_it_was_asked_for() {
    let repo = TestRepo::new();
    repo.commit("base");
    repo.branch("feature");
    repo.branch("source");
    repo.checkout("source");
    repo.write("one.txt", "1\n");
    let one = repo.commit_all("one");
    repo.write("two.txt", "2\n");
    let two = repo.commit_all("two");
    repo.checkout("main");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let on_main = head_at(repo.path());
    let pick = |hash: &str| OpRequest::CherryPick {
        hash: hash.to_owned(),
        mainline: None,
        no_commit: false,
        expect_head: on_main.clone(),
    };
    // Both asked for while main was at the same commit: the second runs after the first.
    assert_eq!(h.run(id, pick(&one)).await.status, OpStatus::Done);
    assert_eq!(h.run(id, pick(&two)).await.status, OpStatus::Done);
    assert_eq!(repo.git(&["log", "-1", "--format=%s", "main"]), "two");

    repo.checkout("feature");
    let feature = repo.git(&["rev-parse", "feature"]);
    let out = h.run(id, pick(&one)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message
            .contains("asked for on main, but feature is checked out now")
    );
    assert_eq!(repo.git(&["rev-parse", "feature"]), feature);
}

/// A merge asked for on a detached HEAD does not run once HEAD has moved.
#[tokio::test]
async fn a_merge_on_a_detached_head_runs_only_where_it_was_asked_for() {
    let repo = TestRepo::new();
    diverged(&repo);
    repo.git(&["checkout", "-q", "--detach", "main"]);
    let seen = head_at(repo.path());
    assert_eq!(seen.branch, None);
    repo.git(&["checkout", "-q", "--detach", "main~1"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let mut request = merge(MergeTargetKind::Branch, "feature", true, false, false);
    if let OpRequest::Merge { expect_head, .. } = &mut request {
        *expect_head = seen;
    }
    let before = repo.git(&["rev-parse", "HEAD"]);
    let out = h.run(id, request).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.contains("HEAD has moved since"),
        "{}",
        out.message
    );
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
}

/// Deleting an unmerged branch confirmed while it had one commit: it got another since, so
/// the core asks again, counting both, and the branch stays.
#[tokio::test]
async fn deleting_a_branch_that_grew_since_its_confirmation_is_asked_again() {
    let repo = TestRepo::new();
    repo.commit("base");
    repo.branch("topic");
    repo.checkout("topic");
    repo.commit("one on topic");
    repo.checkout("main");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let asked = h.run(id, delete_branch("topic")).await;
    let yes = confirmed(&asked);
    assert!(asked.dangers[0].message.contains("1 commit"), "{asked:?}");

    repo.checkout("topic");
    let grown = repo.commit("two on topic");
    repo.checkout("main");
    let out = h.run_with(id, delete_branch("topic"), &yes).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert!(out.dangers[0].message.contains("2 commits"), "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "topic"]), grown);
}

/// Aborting is confirmed for what is stopped: a merge aborted and a cherry-pick stopped in
/// a terminal meanwhile (HEAD where it was) is not aborted on the merge's confirmation.
#[tokio::test]
async fn an_abort_confirmed_for_a_merge_does_not_abort_a_cherry_pick() {
    let repo = TestRepo::new();
    conflicting(&repo);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(
        h.run(id, merge_branch("feature")).await.status,
        OpStatus::Stopped
    );

    let asked = h.run(id, abort()).await;
    assert_eq!(asked.dangers[0].kind, DangerKind::Abort);
    assert!(
        asked.dangers[0].message.contains("merge"),
        "{:?}",
        asked.dangers
    );
    let yes = confirmed(&asked);

    repo.git(&["merge", "--abort"]);
    assert!(!repo.git_succeeds(&["cherry-pick", "feature"]));
    let mark = h.last_log_id();
    let out = h.run_with(id, abort(), &yes).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert!(
        out.dangers[0].message.contains("cherry-pick"),
        "{:?}",
        out.dangers
    );
    assert!(h.writes_since(mark).is_empty());
    assert!(in_git_dir(&repo, "CHERRY_PICK_HEAD"));
}
