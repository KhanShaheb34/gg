//! Integration tests for network operations (`run_op`): push, pull, fetch into a local
//! branch, remote deletes, tag push, checkout with fast-forward, and how fetches share the
//! operation queue. Remotes are local bare repositories (file paths); "someone else pushed"
//! is a second clone that pushes. No test contacts a real network host: the unreachable
//! remote is a closed port on 127.0.0.1, and the one that asks for credentials is a tiny
//! HTTP server on 127.0.0.1 that answers every request with 401.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gg_core::dto::{Confirmed, DangerKind, OpHint, OpOutcome, OpRequest, OpStatus, PullMode};
use gg_core::git::runner::{CmdKind, CommandRecord};
use gg_core::testutil::TestRepo;

mod common;
use common::*;

fn push(branch: &str, remote: &str, force: bool) -> OpRequest {
    OpRequest::Push {
        branch: branch.to_owned(),
        remote: remote.to_owned(),
        to: None,
        force,
    }
}

/// A push of `branch` to branch `to` on `remote` (its upstream there, picked in the dialog).
fn push_to(branch: &str, remote: &str, to: &str) -> OpRequest {
    OpRequest::Push {
        branch: branch.to_owned(),
        remote: remote.to_owned(),
        to: Some(to.to_owned()),
        force: false,
    }
}

/// A force push of `branch` to `remote` (the branch of the same name there).
fn force_push(branch: &str, remote: &str) -> OpRequest {
    push(branch, remote, true)
}

/// A force push confirmed with `token` as the `force_push` danger's (what the UI sends back
/// once the user confirmed the danger it was shown).
fn with_lease(token: Option<&str>) -> [Confirmed; 1] {
    [Confirmed {
        kind: DangerKind::ForcePush,
        token: token.map(str::to_owned),
    }]
}

fn pull(branch: &str, mode: PullMode) -> OpRequest {
    OpRequest::Pull {
        branch: branch.to_owned(),
        mode,
        expect_head: here(),
        expect_worktree: here_wt(),
    }
}

fn fetch_into_local(remote_branch: &str, local_name: &str) -> OpRequest {
    OpRequest::FetchIntoLocal {
        remote_branch: remote_branch.to_owned(),
        local_name: local_name.to_owned(),
    }
}

fn delete_remote_branch(remote: &str, branch: &str) -> OpRequest {
    OpRequest::DeleteRemoteBranch {
        remote: remote.to_owned(),
        branch: branch.to_owned(),
    }
}

/// The write commands operation `out` ran, as argument lists.
fn op_args(h: &Harness, out: &OpOutcome) -> Vec<Vec<String>> {
    op_records(&h.log, out)
        .into_iter()
        .filter(|r| r.kind == CmdKind::Write)
        .map(|r| r.args)
        .collect()
}

/// Whether operation `out` ran exactly `args`.
fn ran(h: &Harness, out: &OpOutcome, args: &[&str]) -> bool {
    op_args(h, out).iter().any(|a| a == args)
}

/// The commit `rev` names in `repo`.
fn rev(repo: &TestRepo, rev: &str) -> String {
    repo.git(&["rev-parse", rev])
}

/// A repository with one commit on `main`, its bare remote `origin` (with `main` as the
/// upstream of `main`) and another clone of that remote, as someone else's.
fn with_remote() -> (TestRepo, TestRepo, TestRepo) {
    let repo = TestRepo::new();
    repo.commit("one");
    let origin = repo.clone_as_remote();
    repo.git(&["branch", "-q", "-u", "origin/main", "main"]);
    let other = TestRepo::clone_of(&origin);
    (repo, origin, other)
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

// ---- push ----

#[tokio::test]
async fn push_sets_upstream_when_missing() {
    let (repo, origin, _other) = with_remote();
    repo.branch("topic");
    repo.checkout("topic");
    repo.commit("on topic");
    repo.checkout("main");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    // No upstream yet: the core sets it, though the request did not ask.
    let out = h.run(id, push("topic", "origin", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Push topic to origin/topic");
    assert!(
        ran(
            &h,
            &out,
            &[
                "push",
                "--progress",
                "--set-upstream",
                "origin",
                "refs/heads/topic:refs/heads/topic"
            ]
        ),
        "{:?}",
        op_args(&h, &out)
    );
    assert_eq!(rev(&origin, "refs/heads/topic"), rev(&repo, "topic"));
    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "topic@{upstream}"]),
        "origin/topic"
    );
    assert!(
        out.snapshot
            .remote_branches
            .contains(&"origin/topic".to_owned())
    );

    // With an upstream, a push leaves it alone.
    repo.checkout("topic");
    repo.commit("more");
    let out = h.run(id, push("topic", "origin", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(
        ran(
            &h,
            &out,
            &[
                "push",
                "--progress",
                "origin",
                "refs/heads/topic:refs/heads/topic"
            ]
        ),
        "{:?}",
        op_args(&h, &out)
    );
    assert_eq!(rev(&origin, "refs/heads/topic"), rev(&repo, "topic"));

    // Nothing new: done, saying so.
    let out = h.run(id, push("topic", "origin", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(out.message.contains("up to date"), "{}", out.message);

    // A remote that does not exist is refused before git runs.
    let out = h.run(id, push("topic", "nope", false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message
            .contains("\u{201c}nope\u{201d} is not a remote of this repository"),
        "{}",
        out.message
    );
    assert_eq!((out.log_from, out.log_to), (0, 0));
}

#[tokio::test]
async fn push_rejected_explains_and_hints() {
    let (repo, _origin, other) = with_remote();
    other.commit("theirs");
    other.git(&["push", "-q", "origin", "main"]);
    repo.commit("mine");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let hints = [
        OpHint::Fetch,
        OpHint::Pull {
            branch: "main".into(),
        },
        OpHint::ForcePush {
            branch: "main".into(),
            remote: "origin".into(),
            to: None,
        },
    ];
    // Not fetched yet ("fetch first"), then fetched ("non-fast-forward"): the same.
    for fetched in [false, true] {
        if fetched {
            repo.git(&["fetch", "-q", "origin"]);
        }
        let out = h.run(id, push("main", "origin", false)).await;
        assert_eq!(out.status, OpStatus::Failed, "{out:?}");
        assert!(
            out.message
                .starts_with("The remote has commits you don't have"),
            "{}",
            out.message
        );
        assert_eq!(out.hints, hints);
        assert!(out.detail.unwrap().contains("[rejected]"));
    }
}

/// `main` and `origin/main` (fetched) each with a commit of their own; returns the remote's.
fn diverged_from_remote(repo: &TestRepo, other: &TestRepo) -> String {
    let theirs = other.commit("theirs");
    other.git(&["push", "-q", "origin", "main"]);
    repo.git(&["fetch", "-q", "origin"]);
    repo.commit("mine");
    theirs
}

#[tokio::test]
async fn force_push_needs_confirmation_and_uses_lease() {
    let (repo, origin, other) = with_remote();
    diverged_from_remote(&repo, &other);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let mark = h.last_log_id();
    let out = h.run(id, push("main", "origin", true)).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert_eq!(out.title, "Force push main to origin/main");
    assert_eq!(out.dangers.len(), 1);
    assert_eq!(out.dangers[0].kind, DangerKind::ForcePush);
    let message = &out.dangers[0].message;
    assert!(message.contains("origin/main"), "{message}");
    assert!(message.contains("1 commit "), "{message}");
    assert!(
        out.dangers[0]
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("theirs")),
        "{:?}",
        out.dangers[0]
    );
    assert!(
        h.writes_since(mark).is_empty(),
        "{:?}",
        h.writes_since(mark)
    );
    assert_ne!(rev(&origin, "refs/heads/main"), rev(&repo, "main"));
    // The token is the remote-tracking commit the count was made from and the local tip
    // (M1): the count and the list name those commits, not the refs, which could move
    // between the reads.
    let token = rev(&repo, "refs/remotes/origin/main");
    let local = rev(&repo, "main");
    assert_eq!(
        out.dangers[0].token.as_deref(),
        Some(format!("{token}\n{local}").as_str())
    );
    let reads: Vec<Vec<String>> = h
        .log
        .since(mark)
        .into_iter()
        .filter(|r| r.op_id == Some(out.op_id) && matches!(r.args[0].as_str(), "rev-list" | "log"))
        .map(|r| r.args)
        .collect();
    assert_eq!(reads.len(), 2, "{reads:?}");
    for args in &reads {
        assert!(args.contains(&token), "{args:?}");
        assert!(
            !args.iter().any(|a| a == "refs/remotes/origin/main"),
            "{args:?}"
        );
    }

    // Confirmed without the token: asked again, nothing runs.
    let mark = h.last_log_id();
    let out = h
        .run_with(id, push("main", "origin", true), &with_lease(None))
        .await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert!(h.writes_since(mark).is_empty());

    let out = h
        .run_confirmed(id, force_push("main", "origin"), &[DangerKind::ForcePush])
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(rev(&origin, "refs/heads/main"), rev(&repo, "main"));
    let pushes: Vec<Vec<String>> = op_args(&h, &out)
        .into_iter()
        .filter(|a| a.first().is_some_and(|c| c == "push"))
        .collect();
    assert_eq!(pushes.len(), 1, "{pushes:?}");
    let lease = format!("--force-with-lease=refs/heads/main:{token}");
    assert!(pushes[0].contains(&lease), "{pushes:?}");
    // What is pushed is the tip that was confirmed.
    assert!(
        pushes[0].contains(&format!("{local}:refs/heads/main")),
        "{pushes:?}"
    );
    assert!(
        pushes[0]
            .iter()
            .all(|a| a != "--force" && a != "-f" && !a.starts_with('+')),
        "{pushes:?}"
    );
}

#[tokio::test]
async fn force_with_lease_fails_when_remote_moved_since_fetch() {
    let (repo, origin, other) = with_remote();
    diverged_from_remote(&repo, &other);
    // The remote moves again after the fetch.
    let newer = other.commit("newer");
    other.git(&["push", "-q", "origin", "main"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    // Confirmed as fetched, which is no longer where the remote is.
    let out = h
        .run_confirmed(id, force_push("main", "origin"), &[DangerKind::ForcePush])
        .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.contains("since your last fetch"),
        "{}",
        out.message
    );
    assert_eq!(out.hints, [OpHint::Fetch]);
    assert!(out.detail.unwrap().contains("stale info"));
    assert_eq!(rev(&origin, "refs/heads/main"), newer);
}

/// The reviewer's scenario: the user sees the danger, someone pushes, a background fetch
/// moves the tracking branch, and the user confirms what they saw. A bare lease would
/// pass (it compares with the tracking branch as now fetched) and discard the commit
/// nobody saw; the lease token makes the core ask again, with the new count.
#[tokio::test]
async fn confirming_with_an_old_lease_asks_again() {
    let (repo, origin, other) = with_remote();
    diverged_from_remote(&repo, &other);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, force_push("main", "origin")).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation);
    let seen = out.dangers[0].token.clone().expect("a token");
    assert!(out.dangers[0].message.contains("1 commit "));

    let unseen = other.commit("unseen");
    other.git(&["push", "-q", "origin", "main"]);
    h.m.fetch(id).await.expect("the background fetch");
    assert_eq!(rev(&repo, "refs/remotes/origin/main"), unseen);

    let mark = h.last_log_id();
    let out = h
        .run_with(id, force_push("main", "origin"), &with_lease(Some(&seen)))
        .await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert!(h.writes_since(mark).is_empty(), "nothing ran");
    assert_eq!(rev(&origin, "refs/heads/main"), unseen, "nothing discarded");
    let now = out.dangers[0].token.clone().expect("a token");
    assert_eq!(now.lines().next(), Some(unseen.as_str()));
    assert!(out.dangers[0].message.contains("2 commits"), "{out:?}");
    assert!(
        out.dangers[0]
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("unseen"),
        "{out:?}"
    );

    let out = h
        .run_with(id, force_push("main", "origin"), &with_lease(Some(&now)))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(rev(&origin, "refs/heads/main"), rev(&repo, "main"));
}

/// M1: the confirmation counted what the remote would lose against the local tip it saw. If
/// the local branch moves before the confirmation arrives (a queued reset, a terminal), the
/// core asks again with the new count, and nothing is pushed.
#[tokio::test]
async fn a_force_push_confirmed_before_the_local_branch_moved_is_asked_again() {
    let (repo, origin, other) = with_remote();
    diverged_from_remote(&repo, &other);
    repo.commit("mine two");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let asked = h.run(id, force_push("main", "origin")).await;
    assert_eq!(asked.status, OpStatus::NeedsConfirmation);
    let token = asked.dangers[0].token.clone().unwrap();

    // The local branch goes back a commit before the confirmation arrives.
    repo.git(&["reset", "-q", "--hard", "HEAD~1"]);
    let there = rev(&origin, "refs/heads/main");
    let mark = h.last_log_id();
    let out = h
        .run_with(id, force_push("main", "origin"), &with_lease(Some(&token)))
        .await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert!(h.writes_since(mark).is_empty(), "nothing pushed");
    assert_eq!(rev(&origin, "refs/heads/main"), there);
    assert_ne!(out.dangers[0].token.as_deref(), Some(token.as_str()));
}

/// Without a remote-tracking branch the token is null and the lease is "the branch must
/// not exist there": a push that would create it runs, one that would replace a branch
/// never fetched is refused.
#[tokio::test]
async fn force_push_without_a_tracking_branch() {
    let (repo, origin, other) = with_remote();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    repo.git(&["branch", "fresh"]);
    let out = h.run(id, force_push("fresh", "origin")).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation);
    // Nothing fetched: no remote commit in the token, only the local tip.
    assert_eq!(
        out.dangers[0].token.as_deref(),
        Some(format!("\n{}", rev(&repo, "fresh")).as_str())
    );
    let out = h
        .run_confirmed(id, force_push("fresh", "origin"), &[DangerKind::ForcePush])
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(ran(
        &h,
        &out,
        &[
            "push",
            "--progress",
            "--set-upstream",
            "--force-with-lease=refs/heads/fresh:",
            "origin",
            "refs/heads/fresh:refs/heads/fresh"
        ]
    ));
    assert_eq!(rev(&origin, "refs/heads/fresh"), rev(&repo, "fresh"));

    // Someone else made `theirs` there; it was never fetched here.
    other.git(&["checkout", "-q", "-b", "theirs"]);
    other.commit("their branch");
    other.git(&["push", "-q", "origin", "theirs"]);
    let theirs = rev(&origin, "refs/heads/theirs");
    repo.git(&["branch", "theirs"]);
    let out = h
        .run_confirmed(id, force_push("theirs", "origin"), &[DangerKind::ForcePush])
        .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert_eq!(
        rev(&origin, "refs/heads/theirs"),
        theirs,
        "nothing replaced"
    );
}

/// C1 / R38: a branch whose upstream has another name (`git switch -c feature origin/main`)
/// is pushed to the branch of its own name: the remote's `main` and the branch's upstream
/// stay as they are. Only a push that names the upstream (`to`, picked in the dialog) goes
/// there, and `to` can name nothing else.
#[tokio::test]
async fn push_goes_to_the_same_named_branch_whatever_the_upstream() {
    let (repo, origin, _other) = with_remote();
    repo.git(&["switch", "-q", "-c", "feature", "--track", "origin/main"]);
    let mine = repo.commit("on feature");
    let main_there = rev(&origin, "refs/heads/main");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, push("feature", "origin", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Push feature to origin/feature");
    assert_eq!(out.message, "Pushed feature to origin/feature.");
    assert!(
        ran(
            &h,
            &out,
            &[
                "push",
                "--progress",
                "origin",
                "refs/heads/feature:refs/heads/feature"
            ]
        ),
        "{:?}",
        op_args(&h, &out)
    );
    assert_eq!(rev(&origin, "refs/heads/feature"), mine);
    assert_eq!(
        rev(&origin, "refs/heads/main"),
        main_there,
        "main unchanged"
    );
    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "feature@{upstream}"]),
        "origin/main",
        "the upstream is not changed"
    );

    // Named in the request, the upstream is where it goes.
    let more = repo.commit("more on feature");
    let out = h.run(id, push_to("feature", "origin", "main")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Push feature to origin/main");
    assert_eq!(out.message, "Pushed feature to origin/main.");
    assert!(ran(
        &h,
        &out,
        &[
            "push",
            "--progress",
            "origin",
            "refs/heads/feature:refs/heads/main"
        ]
    ));
    assert_eq!(rev(&origin, "refs/heads/main"), more);

    // `to` names only the upstream: anything else is refused before git runs.
    let mark = h.last_log_id();
    let out = h.run(id, push_to("feature", "origin", "release")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("origin/release"), "{}", out.message);
    assert!(h.writes_since(mark).is_empty());
    assert!(!origin.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/release"]));
}

/// A force push to the upstream of another name (picked in the dialog): the danger names
/// it, and the lease is on it.
#[tokio::test]
async fn force_push_to_the_upstream_picked_in_the_dialog() {
    let (repo, origin, other) = with_remote();
    other.git(&["checkout", "-q", "-b", "feature"]);
    other.commit("feature base");
    other.git(&["push", "-q", "origin", "feature"]);
    repo.git(&["fetch", "-q", "origin"]);
    repo.git(&["branch", "-q", "--track", "feat", "origin/feature"]);
    repo.checkout("feat");
    repo.commit("on feat");
    other.commit("theirs on feature");
    other.git(&["push", "-q", "origin", "feature"]);
    repo.git(&["fetch", "-q", "origin"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let force = OpRequest::Push {
        branch: "feat".into(),
        remote: "origin".into(),
        to: Some("feature".into()),
        force: true,
    };
    let out = h.run(id, force.clone()).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation);
    assert_eq!(out.title, "Force push feat to origin/feature");
    assert!(out.dangers[0].message.contains("origin/feature"), "{out:?}");
    let token = out.dangers[0].token.clone().unwrap();
    let seen = rev(&repo, "refs/remotes/origin/feature");
    assert_eq!(token, format!("{seen}\n{}", rev(&repo, "feat")));
    let out = h.run_with(id, force, &with_lease(Some(&token))).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    let lease = format!("--force-with-lease=refs/heads/feature:{seen}");
    assert!(op_args(&h, &out).iter().any(|a| a.contains(&lease)));
    assert_eq!(rev(&origin, "refs/heads/feature"), rev(&repo, "feat"));
    assert!(!origin.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/feat"]));
}

/// A rejected push offers a force push to where it went: the upstream it named.
#[tokio::test]
async fn a_rejected_push_to_the_upstream_offers_a_force_push_there() {
    let (repo, _origin, other) = with_remote();
    repo.git(&["switch", "-q", "-c", "feature", "--track", "origin/main"]);
    repo.commit("mine");
    other.commit("theirs");
    other.git(&["push", "-q", "origin", "main"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, push_to("feature", "origin", "main")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.hints.contains(&OpHint::ForcePush {
        branch: "feature".into(),
        remote: "origin".into(),
        to: Some("main".into()),
    }));
}

// ---- pull ----

#[tokio::test]
async fn pull_current_fast_forwards() {
    let (repo, _origin, other) = with_remote();
    let theirs = other.commit("theirs");
    other.git(&["push", "-q", "origin", "main"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, pull("main", PullMode::Default)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Pull main");
    assert_eq!(rev(&repo, "HEAD"), theirs);
    assert!(
        ran(&h, &out, &["pull", "--progress", "--no-edit"]),
        "{:?}",
        op_args(&h, &out)
    );

    let out = h.run(id, pull("main", PullMode::Default)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(out.message.contains("up to date"), "{}", out.message);

    // Without an upstream there is nothing to pull from: explained.
    repo.git(&["branch", "-q", "--unset-upstream", "main"]);
    let out = h.run(id, pull("main", PullMode::Default)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("no upstream"), "{}", out.message);
}

/// Diverged with no pull mode configured: git asks for one, so the outcome offers the
/// choice; merge and rebase then each work.
#[tokio::test]
async fn pull_diverged_asks_for_a_mode() {
    let (repo, _origin, other) = with_remote();
    let theirs = other.commit("theirs");
    other.git(&["push", "-q", "origin", "main"]);
    let mine = repo.commit("mine");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, pull("main", PullMode::Default)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert_eq!(
        out.hints,
        [OpHint::ChoosePullMode {
            branch: "main".into()
        }]
    );
    assert!(out.message.contains("diverged"), "{}", out.message);
    assert_eq!(rev(&repo, "HEAD"), mine);

    let out = h.run(id, pull("main", PullMode::Merge)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(ran(
        &h,
        &out,
        &["pull", "--progress", "--no-edit", "--no-rebase"]
    ));
    assert_eq!(
        repo.git(&["rev-list", "--parents", "-n", "1", "HEAD"])
            .split(' ')
            .count(),
        3,
        "a merge commit"
    );

    repo.git(&["reset", "-q", "--hard", &mine]);
    let out = h.run(id, pull("main", PullMode::Rebase)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(ran(
        &h,
        &out,
        &["pull", "--progress", "--no-edit", "--rebase"]
    ));
    assert_eq!(rev(&repo, "HEAD~1"), theirs);
}

/// Diverged with `pull.ff=only` configured: git refuses ("Not possible to fast-forward"),
/// and the outcome offers the same merge-or-rebase choice, each of which then works: a
/// merge commit (`--ff` lifts the fast-forward-only rule for that pull only) or a rebase.
#[tokio::test]
async fn pull_diverged_with_fast_forward_only_offers_merge_or_rebase() {
    let (repo, _origin, other) = with_remote();
    let theirs = other.commit("theirs");
    other.git(&["push", "-q", "origin", "main"]);
    let mine = repo.commit("mine");
    repo.git(&["config", "pull.ff", "only"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, pull("main", PullMode::Default)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert_eq!(
        out.hints,
        [OpHint::ChoosePullMode {
            branch: "main".into()
        }]
    );
    assert!(out.message.contains("diverged"), "{}", out.message);
    assert!(out.message.contains("fast-forward"), "{}", out.message);
    assert_eq!(rev(&repo, "HEAD"), mine);

    let out = h.run(id, pull("main", PullMode::Merge)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(
        ran(
            &h,
            &out,
            &["pull", "--progress", "--no-edit", "--no-rebase", "--ff"]
        ),
        "{:?}",
        op_args(&h, &out)
    );
    assert_eq!(
        repo.git(&["rev-list", "--parents", "-n", "1", "HEAD"])
            .split(' ')
            .count(),
        3,
        "a merge commit"
    );
    assert_eq!(
        repo.git(&["config", "pull.ff"]),
        "only",
        "the setting stays"
    );

    repo.git(&["reset", "-q", "--hard", &mine]);
    let out = h.run(id, pull("main", PullMode::Rebase)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(rev(&repo, "HEAD~1"), theirs);
}

/// `main` here and on `origin` both change `clash.txt` after a shared `base`: the other
/// clone pushed its change, this one committed its own. HEAD on `main`.
fn clashing_with_remote() -> (TestRepo, TestRepo) {
    let (repo, origin, other) = with_remote();
    repo.write("clash.txt", "base\n");
    repo.commit_all("base");
    repo.git(&["push", "-q", "origin", "main"]);
    other.git(&["pull", "-q", "--ff-only", "origin", "main"]);
    other.write("clash.txt", "theirs\n");
    other.commit_all("their change");
    other.git(&["push", "-q", "origin", "main"]);
    repo.write("clash.txt", "mine\n");
    repo.commit_all("my change");
    (repo, origin)
}

#[tokio::test]
async fn pull_that_conflicts_stops() {
    for mode in [PullMode::Merge, PullMode::Rebase] {
        let (repo, _origin) = clashing_with_remote();
        let h = harness().await;
        let id = h.open(repo.path()).await.repo_id;
        let out = h.run(id, pull("main", mode)).await;
        assert_eq!(out.status, OpStatus::Stopped, "{mode:?}: {out:?}");
        assert!(out.message.contains("conflicts"), "{}", out.message);
        let progress = out.snapshot.in_progress.expect("in progress");
        assert_eq!(progress.paths(), ["clash.txt"], "{mode:?}");
    }
}

#[tokio::test]
async fn pull_with_changes_in_the_way_fails_and_keeps_them() {
    let (repo, _origin) = clashing_with_remote();
    repo.git(&["reset", "-q", "--hard", "HEAD~1"]);
    repo.write("clash.txt", "uncommitted\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, pull("main", PullMode::Merge)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("clash.txt"), "{}", out.message);
    assert!(out.message.contains("Commit or stash"), "{}", out.message);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("clash.txt")).unwrap(),
        "uncommitted\n"
    );
    assert_eq!(out.snapshot.in_progress, None);
}

/// With auto-stash, a pull whose stashed changes conflict as git puts them back exits 0:
/// done, saying they are kept in the stash, with git's text as the detail.
#[tokio::test]
async fn pull_whose_autostash_conflicts_says_so() {
    for (config, mode) in [
        ("rebase.autoStash", PullMode::Rebase),
        ("merge.autoStash", PullMode::Merge),
    ] {
        let (repo, _origin) = clashing_with_remote();
        // Resolve the clash in history so the pull itself is clean; then a dirty
        // `clash.txt` conflicts only with the auto-stash.
        repo.git(&["reset", "-q", "--hard", "HEAD~1"]);
        repo.commit("unrelated");
        repo.write("clash.txt", "dirty\n");
        repo.git(&["config", config, "true"]);
        let h = harness().await;
        let id = h.open(repo.path()).await.repo_id;
        let out = h.run(id, pull("main", mode)).await;
        assert_eq!(out.status, OpStatus::Done, "{config}: {out:?}");
        assert!(
            out.message.contains("kept in the stash"),
            "{config}: {}",
            out.message
        );
        assert!(
            out.detail
                .as_deref()
                .unwrap_or_default()
                .contains("resulted in conflicts"),
            "{config}: {out:?}"
        );
        assert!(repo.git(&["stash", "list"]).contains("autostash"));
    }
}

#[tokio::test]
async fn pull_other_branch_updates_it_without_checkout() {
    let (repo, origin, other) = with_remote();
    repo.branch("topic");
    repo.git(&["push", "-q", "-u", "origin", "topic"]);
    other.git(&["fetch", "-q", "origin"]);
    other.git(&["checkout", "-q", "-b", "topic", "origin/topic"]);
    let theirs = other.commit("theirs on topic");
    other.git(&["push", "-q", "origin", "topic"]);
    let head = rev(&repo, "HEAD");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, pull("topic", PullMode::Default)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(
        ran(
            &h,
            &out,
            &[
                "fetch",
                "--progress",
                "origin",
                "refs/heads/topic:refs/heads/topic"
            ]
        ),
        "{:?}",
        op_args(&h, &out)
    );
    assert_eq!(rev(&repo, "topic"), theirs);
    assert_eq!(current_branch(&repo).as_deref(), Some("main"));
    assert_eq!(rev(&repo, "HEAD"), head);

    // Not a fast-forward any more: explained, and the branch stays.
    repo.checkout("topic");
    let mine = repo.commit("mine on topic");
    repo.checkout("main");
    other.commit("more theirs");
    other.git(&["push", "-q", "origin", "topic"]);
    let out = h.run(id, pull("topic", PullMode::Default)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.contains("\u{201c}topic\u{201d}") && out.message.contains("fast-forward"),
        "{}",
        out.message
    );
    assert_eq!(rev(&repo, "topic"), mine);
    let _ = origin;

    // A branch without an upstream: explained.
    repo.branch("lonely");
    let out = h.run(id, pull("lonely", PullMode::Default)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("no upstream"), "{}", out.message);
}

// ---- fetch into a local branch ----

#[tokio::test]
async fn fetch_into_local_creates_or_fast_forwards() {
    let (repo, _origin, other) = with_remote();
    other.git(&["checkout", "-q", "-b", "feature"]);
    let first = other.commit("feature 1");
    other.git(&["push", "-q", "origin", "feature"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run(id, fetch_into_local("origin/feature", "feature"))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Fetch origin/feature into feature");
    assert!(
        ran(
            &h,
            &out,
            &[
                "fetch",
                "--progress",
                "origin",
                "refs/heads/feature:refs/heads/feature"
            ]
        ),
        "{:?}",
        op_args(&h, &out)
    );
    assert_eq!(rev(&repo, "refs/heads/feature"), first);
    // New commits: the rows reload, and the new branch is served once they have.
    tokio::time::timeout(TIMEOUT, h.m.wait_loaded(id))
        .await
        .expect("reloaded");
    let snapshot = h.m.snapshot(id).await.unwrap();
    assert!(snapshot.branches.contains(&"feature".to_owned()));
    assert_eq!(current_branch(&repo).as_deref(), Some("main"));

    let second = other.commit("feature 2");
    other.git(&["push", "-q", "origin", "feature"]);
    let out = h
        .run(id, fetch_into_local("origin/feature", "feature"))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(rev(&repo, "refs/heads/feature"), second);

    // Into another name too.
    let out = h.run(id, fetch_into_local("origin/feature", "copy")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(rev(&repo, "refs/heads/copy"), second);

    // Not a fast-forward: explained.
    repo.checkout("copy");
    repo.commit("mine");
    repo.checkout("main");
    other.commit("feature 3");
    other.git(&["push", "-q", "origin", "feature"]);
    let out = h.run(id, fetch_into_local("origin/feature", "copy")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("fast-forward"), "{}", out.message);

    // Into the checked-out branch: git will not; explained.
    let out = h.run(id, fetch_into_local("origin/feature", "main")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.contains("\u{201c}main\u{201d}") && out.message.contains("checked out"),
        "{}",
        out.message
    );

    // A bad local name never reaches git.
    let err =
        h.m.run_op(
            id,
            in_wt(repo.path(), fetch_into_local("origin/feature", "-x")),
            &[],
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, gg_core::error::ErrorCode::BadName);
}

// ---- deletes and tags on the remote ----

#[tokio::test]
async fn delete_remote_branch_needs_confirmation() {
    let (repo, origin, _other) = with_remote();
    repo.branch("topic");
    repo.git(&["push", "-q", "origin", "topic"]);
    repo.git(&["fetch", "-q", "origin"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let mark = h.last_log_id();
    let out = h.run(id, delete_remote_branch("origin", "topic")).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert_eq!(out.title, "Delete branch topic on origin");
    assert_eq!(out.dangers.len(), 1);
    assert_eq!(out.dangers[0].kind, DangerKind::DeleteRemoteBranch);
    assert!(
        out.dangers[0].message.contains("origin"),
        "{:?}",
        out.dangers
    );
    assert!(h.writes_since(mark).is_empty());
    assert!(origin.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/topic"]));

    let out = h
        .run_confirmed(
            id,
            delete_remote_branch("origin", "topic"),
            &[DangerKind::DeleteRemoteBranch],
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    // A lease on the commit the confirmation counted from (I1).
    let lease = format!(
        "--force-with-lease=refs/heads/topic:{}",
        rev(&repo, "topic")
    );
    assert!(
        ran(
            &h,
            &out,
            &[
                "push",
                "--progress",
                &lease,
                "origin",
                "--delete",
                "refs/heads/topic"
            ]
        ),
        "{:?}",
        op_args(&h, &out)
    );
    assert!(!origin.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/topic"]));
    assert!(
        !out.snapshot
            .remote_branches
            .contains(&"origin/topic".to_owned())
    );
    // The local branch is untouched.
    assert!(repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/topic"]));
}

/// I1: someone pushed to the branch after the last fetch here. Deleting it, confirmed with
/// what was fetched, is refused by git (the lease), with a plain line; their commit stays.
#[tokio::test]
async fn deleting_a_remote_branch_that_moved_since_the_fetch_is_refused() {
    let (repo, origin, other) = with_remote();
    repo.branch("topic");
    repo.git(&["push", "-q", "origin", "topic"]);
    repo.git(&["fetch", "-q", "origin"]);
    other.git(&["fetch", "-q", "origin"]);
    other.git(&["checkout", "-q", "-b", "topic", "origin/topic"]);
    let theirs = other.commit("pushed after our fetch");
    other.git(&["push", "-q", "origin", "topic"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run_confirmed(
            id,
            delete_remote_branch("origin", "topic"),
            &[DangerKind::DeleteRemoteBranch],
        )
        .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert_eq!(
        out.message,
        "Branch \u{201c}topic\u{201d} on origin has changed since your last fetch, so it was \
         not deleted (that would have deleted commits you have not seen). Fetch, look at what \
         is new, then decide again."
    );
    assert!(out.detail.unwrap_or_default().contains("stale info"));
    assert_eq!(out.hints, [OpHint::Fetch]);
    assert_eq!(rev(&origin, "refs/heads/topic"), theirs, "nothing deleted");
}

#[tokio::test]
async fn push_and_delete_tag_on_remote() {
    let (repo, origin, _other) = with_remote();
    repo.git(&["tag", "v1"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let request = OpRequest::PushTag {
        name: "v1".into(),
        remote: "origin".into(),
    };
    let out = h.run(id, request).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Push tag v1 to origin");
    assert!(ran(
        &h,
        &out,
        &["push", "--progress", "origin", "refs/tags/v1"]
    ));
    assert_eq!(rev(&origin, "refs/tags/v1"), rev(&repo, "v1"));

    // A tag the remote has elsewhere: explained.
    repo.commit("two");
    repo.git(&["tag", "-f", "v1"]);
    let request = OpRequest::PushTag {
        name: "v1".into(),
        remote: "origin".into(),
    };
    let out = h.run(id, request).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("already has a tag"), "{}", out.message);

    let request = OpRequest::DeleteTag {
        name: "v1".into(),
        remote: Some("origin".into()),
    };
    let out = h.run(id, request).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(ran(
        &h,
        &out,
        &["push", "--progress", "origin", "--delete", "refs/tags/v1"]
    ));
    assert!(ran(&h, &out, &["tag", "-d", "--", "v1"]));
    assert!(!origin.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/tags/v1"]));
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/tags/v1"]));
}

#[tokio::test]
async fn delete_tag_on_a_remote_that_lacks_it_deletes_it_here() {
    let (repo, origin, _other) = with_remote();
    repo.git(&["tag", "v2"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let request = OpRequest::DeleteTag {
        name: "v2".into(),
        remote: Some("origin".into()),
    };
    let out = h.run(id, request).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(out.message.contains("did not have it"), "{}", out.message);
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/tags/v2"]));
    assert!(!origin.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/tags/v2"]));
}

// ---- checkout with fast-forward ----

#[tokio::test]
async fn checkout_with_fast_forward() {
    let (repo, _origin, other) = with_remote();
    repo.branch("topic");
    repo.git(&["push", "-q", "-u", "origin", "topic"]);
    other.git(&["fetch", "-q", "origin"]);
    other.git(&["checkout", "-q", "-b", "topic", "origin/topic"]);
    let theirs = other.commit("theirs on topic");
    other.git(&["push", "-q", "origin", "topic"]);
    repo.git(&["fetch", "-q", "origin"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let checkout_ff = |branch: &str| OpRequest::CheckoutBranch {
        branch: branch.to_owned(),
        fast_forward: true,
        expect_worktree: here_wt(),
    };

    let out = h.run(id, checkout_ff("topic")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(current_branch(&repo).as_deref(), Some("topic"));
    assert_eq!(rev(&repo, "HEAD"), theirs);
    assert!(out.message.contains("fast-forwarded"), "{}", out.message);
    assert!(ran(&h, &out, &["merge", "--ff-only", "@{upstream}"]));

    // Diverged: checked out all the same, and the message says it could not move.
    repo.checkout("main");
    repo.checkout("topic");
    let mine = repo.commit("mine on topic");
    repo.checkout("main");
    other.commit("more theirs");
    other.git(&["push", "-q", "origin", "topic"]);
    repo.git(&["fetch", "-q", "origin"]);
    let out = h.run(id, checkout_ff("topic")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(current_branch(&repo).as_deref(), Some("topic"));
    assert_eq!(rev(&repo, "HEAD"), mine);
    assert!(
        out.message.contains("could not be fast-forwarded"),
        "{}",
        out.message
    );

    // No upstream: checked out, nothing to fast-forward to.
    repo.branch("lonely");
    let out = h.run(id, checkout_ff("lonely")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(current_branch(&repo).as_deref(), Some("lonely"));
    assert!(out.message.contains("no upstream"), "{}", out.message);
}

/// Checking out with a fast-forward when an uncommitted change is in the fast-forward's way:
/// checked out, not moved, the change kept, and the message says why.
#[tokio::test]
async fn checkout_with_fast_forward_and_a_change_in_the_way_loses_nothing() {
    let (repo, _origin, other) = with_remote();
    repo.write("shared.txt", "base\n");
    repo.commit_all("shared");
    repo.git(&["push", "-q", "origin", "main"]);
    repo.branch("topic");
    repo.git(&["push", "-q", "-u", "origin", "topic"]);
    other.git(&["fetch", "-q", "origin"]);
    other.git(&["checkout", "-q", "-b", "topic", "origin/topic"]);
    other.write("shared.txt", "theirs\n");
    other.commit_all("theirs on topic");
    other.git(&["push", "-q", "origin", "topic"]);
    repo.git(&["fetch", "-q", "origin"]);
    let before = rev(&repo, "topic");
    // On main, the same file as on topic before theirs: switching keeps the change.
    repo.write("shared.txt", "my unsaved work\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run(
            id,
            OpRequest::CheckoutBranch {
                branch: "topic".into(),
                fast_forward: true,
                expect_worktree: here_wt(),
            },
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(current_branch(&repo).as_deref(), Some("topic"));
    assert_eq!(rev(&repo, "HEAD"), before, "not moved");
    assert!(
        out.message.contains("could not be fast-forwarded"),
        "{}",
        out.message
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("shared.txt")).unwrap(),
        "my unsaved work\n"
    );
    assert!(!in_git_dir(&repo, "MERGE_HEAD"));
}

// ---- failures of the network itself ----

/// A port on 127.0.0.1 that nothing listens on (bound, then closed).
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().expect("address").port()
}

#[tokio::test]
async fn push_to_unreachable_remote_fails_fast_and_queue_continues() {
    let (repo, _origin, _other) = with_remote();
    let url = format!("http://127.0.0.1:{}/repo.git", closed_port());
    repo.git(&["remote", "add", "dead", &url]);
    let head = rev(&repo, "HEAD");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let started = Instant::now();
    let pushing = tokio::spawn({
        let (m, wt) = (h.m.clone(), repo.path().to_path_buf());
        async move {
            m.run_op(id, in_wt(&wt, push("main", "dead", false)), &[])
                .await
        }
    });
    // Asked for once the push has started: it waits for the push, then runs.
    h.sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"]["title"] == "Push main to dead/main"
        })
        .await
        .expect("the push starts");
    let after = h.run(id, create_branch("after", &head)).await;
    let out = pushing.await.unwrap().expect("push");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.starts_with("The remote could not be reached"),
        "{}",
        out.message
    );
    assert!(out.detail.unwrap().contains("127.0.0.1"));
    assert_eq!(after.status, OpStatus::Done, "{after:?}");
    assert!(after.op_id > out.op_id);
    assert!(repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/after"]));
}

/// Answers every HTTP request on 127.0.0.1 with "401 Unauthorized" until dropped.
struct AuthServer {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AuthServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("non-blocking");
        let port = listener.local_addr().expect("address").port();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let stop = stop.clone();
            move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                            let mut request = Vec::new();
                            let mut buf = [0u8; 4096];
                            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                                match stream.read(&mut buf) {
                                    Ok(0) | Err(_) => break,
                                    Ok(n) => request.extend_from_slice(&buf[..n]),
                                }
                            }
                            let _ = stream.write_all(
                                b"HTTP/1.1 401 Unauthorized\r\n\
                                  WWW-Authenticate: Basic realm=\"test\"\r\n\
                                  Content-Length: 0\r\nConnection: close\r\n\r\n",
                            );
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
            }
        });
        Self {
            port,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for AuthServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Real git against a server that refuses: without stored credentials git cannot ask
/// (prompts are off), and with wrong ones it is refused. One explanation each, promptly.
#[tokio::test]
async fn authentication_failures_explain() {
    let (repo, _origin, _other) = with_remote();
    let server = AuthServer::start();
    // No credential helper from the system or user config takes part.
    repo.git(&["config", "credential.helper", ""]);
    let port = server.port;
    repo.git(&[
        "remote",
        "add",
        "nocreds",
        &format!("http://127.0.0.1:{port}/repo.git"),
    ]);
    repo.git(&[
        "remote",
        "add",
        "wrongcreds",
        &format!("http://user:wrong@127.0.0.1:{port}/repo.git"),
    ]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let started = Instant::now();
    let out = h.run(id, push("main", "nocreds", false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("username"), "{}", out.message);
    assert!(out.detail.unwrap().contains("could not read Username"));

    let out = h.run(id, push("main", "wrongcreds", false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("refused the"), "{}", out.message);
    assert!(out.detail.unwrap().contains("Authentication failed"));
    assert!(started.elapsed() < Duration::from_secs(10));
    drop(server);
}

// ---- progress, hints after merge, empty repository ----

#[tokio::test]
async fn network_op_reports_progress_events() {
    let (repo, _origin, _other) = with_remote();
    repo.branch("big");
    repo.checkout("big");
    for i in 0..200 {
        repo.write(&format!("data/{i}.txt"), format!("{i}\n").repeat(200));
    }
    repo.commit_all("lots of files");
    repo.checkout("main");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, push("big", "origin", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    let progress: Vec<serde_json::Value> = h
        .sink
        .events()
        .into_iter()
        .filter(|(n, v)| n == "op-progress" && v["opId"] == out.op_id)
        .map(|(_, v)| v)
        .collect();
    assert!(!progress.is_empty(), "no progress events");
    for p in &progress {
        assert_eq!(p["repoId"], id);
        assert!(!p["line"].as_str().unwrap_or("").is_empty(), "{p}");
    }
}

#[tokio::test]
async fn merge_outcome_hints_push() {
    let (repo, origin, _other) = with_remote();
    repo.branch("feature");
    repo.checkout("feature");
    repo.commit("on feature");
    repo.checkout("main");
    repo.commit("on main");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, merge_branch("feature")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    let hint = OpHint::Push {
        branch: "main".into(),
        remote: "origin".into(),
    };
    assert_eq!(out.hints, std::slice::from_ref(&hint));

    // The hint is a push that works as offered.
    let OpHint::Push { branch, remote } = hint else {
        unreachable!()
    };
    let out = h.run(id, push(&branch, &remote, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(rev(&origin, "refs/heads/main"), rev(&repo, "main"));
}

/// A squash or a no-commit merge commits nothing: nothing new to push, so no push is offered.
#[tokio::test]
async fn squash_and_no_commit_merges_offer_no_push() {
    let (repo, _origin, _other) = with_remote();
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("feature.txt", "feature\n");
    repo.commit_all("on feature");
    repo.checkout("main");
    repo.commit("on main");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run(
            id,
            merge(
                gg_core::dto::MergeTargetKind::Branch,
                "feature",
                true,
                true,
                false,
            ),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(out.hints.is_empty(), "squash: {:?}", out.hints);
    repo.git(&["reset", "-q", "--hard"]);

    let out = h
        .run(
            id,
            merge(
                gg_core::dto::MergeTargetKind::Branch,
                "feature",
                true,
                false,
                true,
            ),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(out.hints.is_empty(), "no commit: {:?}", out.hints);
    assert!(in_git_dir(&repo, "MERGE_HEAD"));
}

/// Before the first commit nothing can be pushed or pulled: plain failures, no hang.
#[tokio::test]
async fn network_operations_fail_plainly_on_an_empty_repository() {
    let repo = TestRepo::new();
    let origin = TestRepo::new();
    origin.git(&["init", "-q", "--bare"]);
    repo.git(&["remote", "add", "origin", origin.path().to_str().unwrap()]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let requests = vec![
        push("main", "origin", false),
        pull("main", PullMode::Default),
        fetch_into_local("origin/main", "x"),
        OpRequest::PushTag {
            name: "v1".into(),
            remote: "origin".into(),
        },
        OpRequest::CheckoutBranch {
            branch: "main".into(),
            fast_forward: true,
            expect_worktree: here_wt(),
        },
    ];
    for request in requests {
        let shown = format!("{request:?}");
        match tokio::time::timeout(TIMEOUT, h.m.run_op(id, in_wt(repo.path(), request), &[])).await
        {
            Err(_) => panic!("{shown}: did not end"),
            Ok(Err(e)) => assert!(!e.message.is_empty(), "{shown}: {e:?}"),
            Ok(Ok(out)) => {
                assert_eq!(out.status, OpStatus::Failed, "{shown}: {out:?}");
                assert!(!out.message.is_empty(), "{shown}");
            }
        }
    }
}

// ---- fetches and the operation queue (ruling R5) ----

/// Records of `git fetch` (scheduled or asked for) logged so far.
fn fetch_records(h: &Harness) -> Vec<CommandRecord> {
    h.log
        .since(0)
        .into_iter()
        .filter(|r| r.op_id.is_none() && r.args.iter().any(|a| a == "fetch"))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduled_fetch_is_skipped_while_an_operation_runs() {
    let (repo, _origin, _other) = with_remote();
    let head = rev(&repo, "HEAD");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_op_delay(Duration::from_millis(2500));
    h.m.set_fetch_interval(1);

    let out = h.run(id, create_branch("slow", &head)).await;
    let ended = unix_ms();
    let started = h
        .changes(id)
        .iter()
        .find(|v| v["op"]["id"] == out.op_id)
        .and_then(|v| v["op"]["startedAtMs"].as_i64())
        .expect("published");
    h.m.set_op_delay(Duration::ZERO);
    // The schedule resumes once the operation is over.
    let resumed = tokio::time::timeout(Duration::from_secs(10), async {
        while !fetch_records(&h).iter().any(|r| r.started_at_ms >= ended) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    h.m.set_fetch_interval(0);
    assert!(resumed.is_ok(), "no scheduled fetch after the operation");
    let fetches = fetch_records(&h);
    let during: Vec<&CommandRecord> = fetches
        .iter()
        .filter(|r| r.started_at_ms >= started && r.started_at_ms < ended)
        .collect();
    assert!(
        during.is_empty(),
        "fetched while the operation ran: {during:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn user_fetch_waits_for_the_running_operation() {
    const DELAY_MS: i64 = 800;
    let (repo, _origin, _other) = with_remote();
    let head = rev(&repo, "HEAD");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_op_delay(Duration::from_millis(DELAY_MS as u64));

    let op = tokio::spawn({
        let (m, wt, head) = (h.m.clone(), repo.path().to_path_buf(), head.clone());
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
        .expect("the operation runs");
    let fetch = tokio::spawn({
        let m = h.m.clone();
        async move { m.fetch(id).await }
    });
    // The fetch waits in the queue, counted with the operation's waiting requests.
    h.sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"]["queued"] == 1
        })
        .await
        .expect("the fetch is counted as waiting");
    let out = op.await.unwrap().expect("operation");
    tokio::time::timeout(TIMEOUT, fetch)
        .await
        .expect("the fetch ends")
        .unwrap()
        .expect("fetch");

    let write = op_records(&h.log, &out)
        .into_iter()
        .find(|r| r.kind == CmdKind::Write)
        .expect("the operation's write");
    let body_end = write.started_at_ms + i64::from(write.duration_ms) + DELAY_MS;
    let fetches = fetch_records(&h);
    assert_eq!(fetches.len(), 1, "{fetches:?}");
    assert!(
        fetches[0].started_at_ms >= body_end - 1,
        "fetch started at {}, the operation's body ended at {body_end}",
        fetches[0].started_at_ms
    );
    assert!(!h.m.snapshot(id).await.unwrap().fetch.running);
}

/// A fetch the user asks for just as a scheduled one has shown itself running, but before
/// git has started, is not taken as covered by it: when it returns, it has fetched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_user_fetch_just_after_a_scheduled_one_starts_still_fetches() {
    let (repo, _origin, other) = with_remote();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let pushed = other.commit("new on origin");
    other.git(&["push", "-q", "origin", "main"]);

    h.m.set_scheduled_fetch_gap(Duration::from_millis(1500));
    h.m.set_fetch_interval(1);
    h.sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["fetch"]["running"] == true
        })
        .await
        .expect("a scheduled fetch shows as running");
    h.m.set_fetch_interval(0);
    tokio::time::timeout(TIMEOUT, h.m.fetch(id))
        .await
        .expect("the fetch ends")
        .expect("fetch");
    assert_eq!(
        rev(&repo, "refs/remotes/origin/main"),
        pushed,
        "fetched by now"
    );
    let snap = h.m.snapshot(id).await.unwrap();
    assert!(!snap.fetch.running);
    assert!(snap.fetch.last_error.is_none());
}

/// Task 5 review, item 2: a scheduled fetch passes its "nothing waits" check, then a user's
/// fetch takes its turn and finds no scheduled fetch to cancel yet; the scheduled one then
/// shows as running. The user's fetch must not take that `running` as covering it: it
/// cancels the scheduled fetch, waits for it and fetches. The interleaving is forced with
/// pause points.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_user_fetch_is_not_covered_by_a_scheduled_one_that_started_behind_its_back() {
    let (repo, _origin, other) = with_remote();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let pushed = other.commit("new on origin");
    other.git(&["push", "-q", "origin", "main"]);

    // The scheduled fetch stops just after its check; it shows as running for a while
    // before it would start git.
    let scheduled = h.m.pause_at("scheduled-fetch-checked");
    h.m.set_scheduled_fetch_gap(Duration::from_millis(800));
    h.m.set_fetch_interval(1);
    tokio::time::timeout(TIMEOUT, scheduled.reached.notified())
        .await
        .expect("a scheduled fetch is due");
    h.m.set_fetch_interval(0);

    // The user's fetch takes its turn and cancels (nothing yet), then stops.
    let user = h.m.pause_at("user-fetch-cancelled");
    let fetch = tokio::spawn({
        let m = h.m.clone();
        async move { m.fetch(id).await }
    });
    tokio::time::timeout(TIMEOUT, user.reached.notified())
        .await
        .expect("the user's fetch has its turn");

    // Now the scheduled fetch shows itself running.
    scheduled.release.notify_one();
    h.sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["fetch"]["running"] == true
        })
        .await
        .expect("the scheduled fetch shows as running");
    user.release.notify_one();

    tokio::time::timeout(TIMEOUT, fetch)
        .await
        .expect("the fetch ends")
        .unwrap()
        .expect("fetch");
    assert_eq!(
        rev(&repo, "refs/remotes/origin/main"),
        pushed,
        "the user's fetch fetched"
    );
    let snap = h.m.snapshot(id).await.unwrap();
    assert!(!snap.fetch.running);
    assert!(snap.fetch.last_error.is_none());
}

/// R13: a scheduled fetch that is running when an operation or a fetch the user asked for
/// takes its turn is cancelled: its git and what git started are stopped, no error is
/// shown, and the operation does not wait for it.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_scheduled_fetch_is_cancelled_when_an_operation_takes_its_turn() {
    use std::os::unix::fs::PermissionsExt;

    for by_user_fetch in [false, true] {
        let (repo, _origin, _other) = with_remote();
        let head = rev(&repo, "HEAD");
        let pidfile = repo.path().join(".git/hung.pid");
        let script = repo.path().join(".git/hung-upload-pack");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nexec sleep 30\n",
                pidfile.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        repo.git(&[
            "config",
            "remote.origin.uploadpack",
            script.to_str().unwrap(),
        ]);
        let h = harness().await;
        let id = h.open(repo.path()).await.repo_id;

        h.m.set_fetch_interval(1);
        h.sink
            .wait_for(TIMEOUT, |n, v| {
                n == "repo-changed" && v["repoId"] == id && v["fetch"]["running"] == true
            })
            .await
            .expect("a scheduled fetch starts");
        h.m.set_fetch_interval(0);
        let deadline = Instant::now() + TIMEOUT;
        while !pidfile.exists() {
            assert!(Instant::now() < deadline, "upload-pack started");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let pid = std::fs::read_to_string(&pidfile).unwrap().trim().to_owned();

        let started = Instant::now();
        if by_user_fetch {
            repo.git(&["config", "--unset", "remote.origin.uploadpack"]);
            tokio::time::timeout(Duration::from_secs(10), h.m.fetch(id))
                .await
                .expect("the user's fetch is not held up")
                .expect("fetch");
        } else {
            let out = tokio::time::timeout(
                Duration::from_secs(10),
                h.m.run_op(id, in_wt(repo.path(), create_branch("b", &head)), &[]),
            )
            .await
            .expect("the operation is not held up")
            .expect("run_op");
            assert_eq!(out.status, OpStatus::Done, "{out:?}");
        }
        assert!(started.elapsed() < Duration::from_secs(10));
        let alive = || {
            std::process::Command::new("kill")
                .args(["-0", &pid])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive() {
            assert!(
                Instant::now() < deadline,
                "the hung upload-pack was stopped"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let snap = h.m.snapshot(id).await.unwrap();
        assert!(!snap.fetch.running);
        assert_eq!(
            snap.fetch.last_error, None,
            "no error shown: {:?}",
            snap.fetch
        );
    }
}

/// If the task of a fetch the user asked for panics, the next fetch asked for still runs
/// (the "one is waiting or running" flag is cleared on every way out).
#[tokio::test]
async fn a_user_fetch_that_panics_does_not_block_the_next() {
    let (repo, _origin, other) = with_remote();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.panic_next_fetch();
    h.m.fetch(id)
        .await
        .expect("a panicked fetch is Ok, like a dropped one");
    let theirs = other.commit("theirs");
    other.git(&["push", "-q", "origin", "main"]);
    tokio::time::timeout(TIMEOUT, h.m.fetch(id))
        .await
        .expect("the fetch ends")
        .expect("fetch");
    assert_eq!(rev(&repo, "refs/remotes/origin/main"), theirs);
}

/// Closing a repository while a fetch the user asked for runs is refused, saying so (not
/// that an operation runs).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_during_a_user_fetch_says_a_fetch_runs() {
    use std::os::unix::fs::PermissionsExt;

    let (repo, _origin, _other) = with_remote();
    let script = repo.path().join(".git/slow-upload-pack");
    std::fs::write(&script, "#!/bin/sh\nexec sleep 2\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    repo.git(&[
        "config",
        "remote.origin.uploadpack",
        script.to_str().unwrap(),
    ]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let fetch = tokio::spawn({
        let m = h.m.clone();
        async move { m.fetch(id).await }
    });
    h.sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["fetch"]["running"] == true
        })
        .await
        .expect("the fetch runs");
    let err = h.m.close(id).await.expect_err("busy");
    assert_eq!(err.code, gg_core::error::ErrorCode::Busy);
    assert!(err.message.contains("fetch"), "{}", err.message);
    assert!(!err.message.contains("operation"), "{}", err.message);
    let _ = tokio::time::timeout(TIMEOUT, fetch).await.expect("ends");
    h.m.close(id).await.expect("closes once the fetch is done");
}

/// A remote is named by one of the repository's remotes: a URL, a path or `x:y` in its
/// place is refused with a plain line, before git runs.
#[tokio::test]
async fn only_the_repositorys_remotes_are_pushed_to() {
    let (repo, origin, _other) = with_remote();
    repo.git(&["tag", "v1"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let mark = h.last_log_id();
    let path = origin.path().to_str().unwrap().to_owned();
    for remote in [
        path.as_str(),
        "https://example.invalid/r.git",
        "origin:main",
        "nope",
    ] {
        for request in [
            push("main", remote, false),
            OpRequest::PushTag {
                name: "v1".into(),
                remote: remote.into(),
            },
            OpRequest::DeleteTag {
                name: "v1".into(),
                remote: Some(remote.into()),
            },
            delete_remote_branch(remote, "main"),
        ] {
            let out = h
                .run_confirmed(id, request.clone(), &[DangerKind::DeleteRemoteBranch])
                .await;
            assert_eq!(out.status, OpStatus::Failed, "{request:?}: {out:?}");
            assert!(
                out.message.contains("is not a remote of this repository"),
                "{}",
                out.message
            );
            assert!(
                out.message.contains("origin"),
                "names the remotes: {}",
                out.message
            );
        }
    }
    assert!(
        h.writes_since(mark).is_empty(),
        "{:?}",
        h.writes_since(mark)
    );
    assert!(repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/tags/v1"]));
}

/// Deleting the remote's default branch (what `<remote>/HEAD` points at) says so.
#[tokio::test]
async fn deleting_the_remotes_default_branch_says_so() {
    let (repo, _origin, other) = with_remote();
    other.git(&["push", "-q", "origin", "main:side"]);
    repo.git(&["fetch", "-q", "origin"]);
    repo.git(&["remote", "set-head", "origin", "main"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, delete_remote_branch("origin", "main")).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation);
    assert!(out.dangers[0].message.contains("default branch"), "{out:?}");
    let out = h.run(id, delete_remote_branch("origin", "side")).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation);
    assert!(
        !out.dangers[0].message.contains("default branch"),
        "{out:?}"
    );
}

/// Pulling the branch that a rebase stopped on, in this same worktree: git counts it as
/// checked out here and refuses; the message says what is in progress here, not to switch
/// to another worktree.
#[tokio::test]
async fn pulling_the_branch_being_rebased_here_says_a_rebase_is_in_progress() {
    let (repo, _origin, other) = with_remote();
    repo.write("clash.txt", "base\n");
    repo.commit_all("base");
    repo.git(&["push", "-q", "origin", "main"]);
    repo.git(&["fetch", "-q", "origin"]);
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("clash.txt", "feature\n");
    repo.commit_all("feature side");
    repo.checkout("main");
    repo.write("clash.txt", "main\n");
    repo.commit_all("main side");
    other.git(&["pull", "-q"]);
    other.commit("new on origin");
    other.git(&["push", "-q", "origin", "main"]);
    assert!(!repo.git_succeeds(&["rebase", "feature"]));
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run(
            id,
            OpRequest::Pull {
                branch: "main".into(),
                mode: PullMode::Default,
                expect_head: here(),
                expect_worktree: here_wt(),
            },
        )
        .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.contains("rebase is in progress"),
        "{}",
        out.message
    );
    assert!(out.message.contains("this worktree"), "{}", out.message);
    assert!(
        !out.message.contains("Switch to that worktree"),
        "{}",
        out.message
    );
    assert!(
        !out.message.contains("switch to that worktree"),
        "{}",
        out.message
    );
}

/// Task 5 review, item 6: fetching into the branch checked out here, with nothing in
/// progress, says what git's refusal means (no talk of an operation in progress).
#[tokio::test]
async fn fetching_into_the_branch_checked_out_here_with_nothing_stopped_says_why_plainly() {
    let (repo, _origin, other) = with_remote();
    other.commit("new on origin");
    other.git(&["push", "-q", "origin", "main"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, fetch_into_local("origin/main", "main")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    // Checked out in this very worktree: not "switch to that worktree", but to pull it.
    assert_eq!(
        out.message,
        "Branch \u{201c}main\u{201d} is checked out here, and git does not move a \
         checked-out branch by fetching into it: use Pull to update it."
    );
}

/// Item 6, the other half: with a rebase stopped on the branch, fetching into it says the
/// rebase is in progress, and to fetch (not pull) once it has ended.
#[tokio::test]
async fn fetching_into_the_branch_being_rebased_here_says_to_fetch_afterwards() {
    let (repo, _origin, other) = with_remote();
    repo.write("clash.txt", "base\n");
    repo.commit_all("base");
    repo.git(&["push", "-q", "origin", "main"]);
    repo.git(&["fetch", "-q", "origin"]);
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("clash.txt", "feature\n");
    repo.commit_all("feature side");
    repo.checkout("main");
    repo.write("clash.txt", "main\n");
    repo.commit_all("main side");
    other.git(&["pull", "-q"]);
    other.commit("new on origin");
    other.git(&["push", "-q", "origin", "main"]);
    assert!(!repo.git_succeeds(&["rebase", "feature"]));
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, fetch_into_local("origin/main", "main")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.contains("A rebase is in progress"),
        "{}",
        out.message
    );
    assert!(
        out.message
            .ends_with("Continue or abort the rebase first, then fetch."),
        "{}",
        out.message
    );
}

/// M4: a remote branch never fetched here cannot be deleted from the app (its lease is "not
/// there"); the refusal says it has not been fetched, not that it changed.
#[tokio::test]
async fn deleting_a_remote_branch_never_fetched_says_so() {
    let (repo, origin, other) = with_remote();
    other.git(&["checkout", "-q", "-b", "theirs"]);
    let theirs = other.commit("their branch");
    other.git(&["push", "-q", "origin", "theirs"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run_confirmed(
            id,
            delete_remote_branch("origin", "theirs"),
            &[DangerKind::DeleteRemoteBranch],
        )
        .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert_eq!(
        out.message,
        "Branch \u{201c}theirs\u{201d} on origin has not been fetched here, so it was not \
         deleted (that would delete commits you have not seen). Fetch, look at what is on it, \
         then decide again."
    );
    assert_eq!(out.hints, [OpHint::Fetch]);
    assert_eq!(rev(&origin, "refs/heads/theirs"), theirs, "nothing deleted");
}
