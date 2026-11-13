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

    // A remote that does not exist is explained.
    let out = h.run(id, push("topic", "nope", false, false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        !out.message.starts_with("fatal:") && out.message.contains("\u{201c}nope\u{201d}"),
        "{}",
        out.message
    );
    assert!(
        out.detail
            .unwrap()
            .contains("does not appear to be a git repository")
    );
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

    let out = h
        .run_confirmed(
            id,
            push("main", "origin", false, true),
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
    assert!(pushes[0].iter().any(|a| a == "--force-with-lease"));
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

    let out = h
        .run_confirmed(
            id,
            push("main", "origin", false, true),
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
