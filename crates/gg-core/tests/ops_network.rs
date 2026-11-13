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

use gg_core::dto::{DangerKind, OpHint, OpOutcome, OpRequest, OpStatus, PullMode};
use gg_core::git::runner::{CmdKind, CommandRecord};
use gg_core::testutil::TestRepo;

mod common;
use common::*;

fn push(branch: &str, remote: &str, set_upstream: bool, force: bool) -> OpRequest {
    OpRequest::Push {
        branch: branch.to_owned(),
        remote: remote.to_owned(),
        set_upstream,
        force,
        lease: None,
    }
}

/// A force push of `branch` to `remote`, with `lease` (the token of the `force_push` danger
/// the user confirmed).
fn force_push(branch: &str, remote: &str, lease: Option<&str>) -> OpRequest {
    OpRequest::Push {
        branch: branch.to_owned(),
        remote: remote.to_owned(),
        set_upstream: false,
        force: true,
        lease: lease.map(str::to_owned),
    }
}

fn pull(branch: &str, mode: PullMode) -> OpRequest {
    OpRequest::Pull {
        branch: branch.to_owned(),
        mode,
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
    let out = h.run(id, push("topic", "origin", false, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Push topic to origin");
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
    let out = h.run(id, push("topic", "origin", false, false)).await;
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
    let out = h.run(id, push("topic", "origin", false, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(out.message.contains("up to date"), "{}", out.message);

    // A remote that does not exist is refused before git runs.
    let out = h.run(id, push("topic", "nope", false, false)).await;
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
        },
    ];
    // Not fetched yet ("fetch first"), then fetched ("non-fast-forward"): the same.
    for fetched in [false, true] {
        if fetched {
            repo.git(&["fetch", "-q", "origin"]);
        }
        let out = h.run(id, push("main", "origin", false, false)).await;
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
    let out = h.run(id, push("main", "origin", false, true)).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert_eq!(out.title, "Force push main to origin");
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
    // The token is the remote-tracking commit the count was made from.
    let token = rev(&repo, "refs/remotes/origin/main");
    assert_eq!(out.dangers[0].token.as_deref(), Some(token.as_str()));

    // Confirmed without the token: asked again, nothing runs.
    let mark = h.last_log_id();
    let out = h
        .run_confirmed(
            id,
            push("main", "origin", false, true),
            &[DangerKind::ForcePush],
        )
        .await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert!(h.writes_since(mark).is_empty());

    let out = h
        .run_confirmed(
            id,
            force_push("main", "origin", Some(&token)),
            &[DangerKind::ForcePush],
        )
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

    let token = rev(&repo, "refs/remotes/origin/main");
    let out = h
        .run_confirmed(
            id,
            force_push("main", "origin", Some(&token)),
            &[DangerKind::ForcePush],
        )
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

    let out = h.run(id, force_push("main", "origin", None)).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation);
    let seen = out.dangers[0].token.clone().expect("a token");
    assert!(out.dangers[0].message.contains("1 commit "));

    let unseen = other.commit("unseen");
    other.git(&["push", "-q", "origin", "main"]);
    h.m.fetch(id).await.expect("the background fetch");
    assert_eq!(rev(&repo, "refs/remotes/origin/main"), unseen);

    let mark = h.last_log_id();
    let out = h
        .run_confirmed(
            id,
            force_push("main", "origin", Some(&seen)),
            &[DangerKind::ForcePush],
        )
        .await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert!(h.writes_since(mark).is_empty(), "nothing ran");
    assert_eq!(rev(&origin, "refs/heads/main"), unseen, "nothing discarded");
    assert_eq!(out.dangers[0].token.as_deref(), Some(unseen.as_str()));
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
        .run_confirmed(
            id,
            force_push("main", "origin", Some(&unseen)),
            &[DangerKind::ForcePush],
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(rev(&origin, "refs/heads/main"), rev(&repo, "main"));
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
    let out = h.run(id, force_push("fresh", "origin", None)).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation);
    assert_eq!(out.dangers[0].token, None);
    let out = h
        .run_confirmed(
            id,
            force_push("fresh", "origin", None),
            &[DangerKind::ForcePush],
        )
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
        .run_confirmed(
            id,
            force_push("theirs", "origin", None),
            &[DangerKind::ForcePush],
        )
        .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert_eq!(
        rev(&origin, "refs/heads/theirs"),
        theirs,
        "nothing replaced"
    );
}

/// R12: a branch with an upstream on the remote pushed to pushes to that upstream branch,
/// whatever its name; the danger and the lease use it too.
#[tokio::test]
async fn push_goes_to_the_upstream_branch() {
    let (repo, origin, other) = with_remote();
    other.git(&["checkout", "-q", "-b", "feature"]);
    other.commit("feature base");
    other.git(&["push", "-q", "origin", "feature"]);
    repo.git(&["fetch", "-q", "origin"]);
    repo.git(&["branch", "-q", "--track", "feat", "origin/feature"]);
    repo.checkout("feat");
    let mine = repo.commit("on feat");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, push("feat", "origin", false, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(ran(
        &h,
        &out,
        &[
            "push",
            "--progress",
            "origin",
            "refs/heads/feat:refs/heads/feature"
        ]
    ));
    assert_eq!(rev(&origin, "refs/heads/feature"), mine);
    assert!(!origin.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/feat"]));

    // Diverge, then force: the danger names origin/feature, the lease refs/heads/feature.
    other.git(&["pull", "-q", "--ff-only", "origin", "feature"]);
    other.commit("theirs on feature");
    other.git(&["push", "-q", "origin", "feature"]);
    repo.git(&["fetch", "-q", "origin"]);
    repo.git(&["commit", "-q", "--amend", "-m", "on feat, amended"]);
    let out = h.run(id, force_push("feat", "origin", None)).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation);
    assert!(out.dangers[0].message.contains("origin/feature"), "{out:?}");
    let token = out.dangers[0].token.clone().unwrap();
    assert_eq!(token, rev(&repo, "refs/remotes/origin/feature"));
    let out = h
        .run_confirmed(
            id,
            force_push("feat", "origin", Some(&token)),
            &[DangerKind::ForcePush],
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    let lease = format!("--force-with-lease=refs/heads/feature:{token}");
    assert!(op_args(&h, &out).iter().any(|a| a.contains(&lease)));
    assert_eq!(rev(&origin, "refs/heads/feature"), rev(&repo, "feat"));
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
        assert_eq!(progress.conflicted, ["clash.txt"], "{mode:?}");
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
                .contains("Applying autostash resulted in conflicts"),
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
        h.m.run_op(id, fetch_into_local("origin/feature", "-x"), &[])
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
    assert!(ran(
        &h,
        &out,
        &[
            "push",
            "--progress",
            "origin",
            "--delete",
            "refs/heads/topic"
        ]
    ));
    assert!(!origin.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/topic"]));
    assert!(
        !out.snapshot
            .remote_branches
            .contains(&"origin/topic".to_owned())
    );
    // The local branch is untouched.
    assert!(repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/topic"]));
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
        let m = h.m.clone();
        async move { m.run_op(id, push("main", "dead", false, false), &[]).await }
    });
    // Asked for once the push has started: it waits for the push, then runs.
    h.sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"]["title"] == "Push main to dead"
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
    let out = h.run(id, push("main", "nocreds", false, false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("username"), "{}", out.message);
    assert!(out.detail.unwrap().contains("could not read Username"));

    let out = h.run(id, push("main", "wrongcreds", false, false)).await;
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

    let out = h.run(id, push("big", "origin", false, false)).await;
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
        set_upstream: false,
    };
    assert_eq!(out.hints, std::slice::from_ref(&hint));

    // The hint is a push that works as offered.
    let OpHint::Push {
        branch,
        remote,
        set_upstream,
    } = hint
    else {
        unreachable!()
    };
    let out = h.run(id, push(&branch, &remote, set_upstream, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(rev(&origin, "refs/heads/main"), rev(&repo, "main"));
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
        push("main", "origin", false, false),
        pull("main", PullMode::Default),
        fetch_into_local("origin/main", "x"),
        OpRequest::PushTag {
            name: "v1".into(),
            remote: "origin".into(),
        },
        OpRequest::CheckoutBranch {
            branch: "main".into(),
            fast_forward: true,
        },
    ];
    for request in requests {
        let shown = format!("{request:?}");
        match tokio::time::timeout(TIMEOUT, h.m.run_op(id, request, &[])).await {
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
        let (m, head) = (h.m.clone(), head.clone());
        async move { m.run_op(id, create_branch("a", &head), &[]).await }
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
                h.m.run_op(id, create_branch("b", &head), &[]),
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
            push("main", remote, false, false),
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
