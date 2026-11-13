//! Integration tests for local operations (`run_op`): checkout, branches, merge, rebase,
//! cherry-pick, revert, reset and tags, against real temp repositories.

use std::fs;
use std::time::Duration;

use gg_core::dto::{DangerKind, MergeTargetKind, OpHint, OpRequest, OpStatus, ResetMode};
use gg_core::error::ErrorCode;
use gg_core::git::runner::{CmdKind, CommandLog, GitRunner};
use gg_core::ops::{Step, explain};
use gg_core::testutil::TestRepo;

mod common;
use common::*;

// ---- Local operations: checkout and branches ----

#[tokio::test]
async fn checkout_branch_switches_to_it() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("feature/x");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, checkout_branch("feature/x")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Check out feature/x");
    assert_eq!(current_branch(&repo).as_deref(), Some("feature/x"));
    assert_eq!(out.snapshot.head.branch.as_deref(), Some("feature/x"));
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["checkout", "feature/x", "--"]),
    );
}

#[tokio::test]
async fn checkout_remote_makes_a_tracking_branch() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("topic");
    repo.checkout("topic");
    let tip = repo.commit("on topic");
    repo.checkout("main");
    let _origin = repo.clone_as_remote();
    repo.git(&["branch", "-D", "topic"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let request = OpRequest::CheckoutRemote {
        remote_branch: "origin/topic".into(),
        local_name: "mine".into(),
    };
    let out = h.run(id, request).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(current_branch(&repo).as_deref(), Some("mine"));
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), tip);
    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "mine@{upstream}"]),
        "origin/topic"
    );
    assert!(out.snapshot.branches.contains(&"mine".to_owned()));
}

#[tokio::test]
async fn checkout_commit_detaches_head() {
    let repo = TestRepo::new();
    let first = repo.commit("one");
    repo.commit("two");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run(
            id,
            OpRequest::CheckoutCommit {
                hash: first.clone(),
            },
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(current_branch(&repo), None);
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), first);
    assert_eq!(out.snapshot.head.branch, None);
    assert_eq!(out.snapshot.head.hash.as_deref(), Some(first.as_str()));
}

#[tokio::test]
async fn create_branch_with_checkout_switches_to_it() {
    let repo = TestRepo::new();
    let first = repo.commit("one");
    repo.commit("two");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let request = OpRequest::CreateBranch {
        name: "fix".into(),
        at: first.clone(),
        checkout: true,
    };
    let out = h.run(id, request).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(current_branch(&repo).as_deref(), Some("fix"));
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), first);
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["checkout", "-b", "fix", first.as_str()]),
    );
}

#[tokio::test]
async fn rename_branch_renames_it_and_keeps_head_on_it() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    repo.branch("old");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let rename = |branch: &str, new_name: &str| OpRequest::RenameBranch {
        branch: branch.into(),
        new_name: new_name.into(),
    };
    let out = h.run(id, rename("old", "new")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/old"]));
    assert_eq!(repo.git(&["rev-parse", "refs/heads/new"]), head);
    assert_eq!(out.snapshot.branches, ["main", "new"]);

    let out = h.run(id, rename("main", "trunk")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(current_branch(&repo).as_deref(), Some("trunk"));

    let err = h.m.run_op(id, rename("new", "-x"), &[]).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::BadName);
}

/// "a branch named 'x' already exists", from rename and from a remote checkout.
#[tokio::test]
async fn an_existing_branch_name_explains() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("a");
    repo.branch("b");
    let _origin = repo.clone_as_remote();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let rename = OpRequest::RenameBranch {
        branch: "a".into(),
        new_name: "b".into(),
    };
    let remote = OpRequest::CheckoutRemote {
        remote_branch: "origin/a".into(),
        local_name: "b".into(),
    };
    for request in [rename, remote] {
        let out = h.run(id, request).await;
        assert_eq!(out.status, OpStatus::Failed, "{out:?}");
        assert_eq!(
            out.message,
            "A branch named \u{201c}b\u{201d} already exists."
        );
        assert!(out.detail.unwrap().contains("already exists"));
    }
    assert!(repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/a"]));
    assert_eq!(current_branch(&repo).as_deref(), Some("main"));
}

#[tokio::test]
async fn checkout_with_changes_in_the_way_fails_and_keeps_them() {
    let repo = TestRepo::new();
    repo.write("shared.txt", "base\n");
    repo.commit_all("base");
    repo.branch("other");
    repo.checkout("other");
    repo.write("shared.txt", "other\n");
    repo.commit_all("other");
    repo.checkout("main");
    repo.write("shared.txt", "my work\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, checkout_branch("other")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("shared.txt"), "{}", out.message);
    assert!(
        out.message.to_lowercase().contains("uncommitted changes"),
        "{}",
        out.message
    );
    assert!(
        out.detail
            .unwrap()
            .contains("would be overwritten by checkout")
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("shared.txt")).unwrap(),
        "my work\n"
    );
    assert_eq!(current_branch(&repo).as_deref(), Some("main"));
}

#[tokio::test]
async fn checkout_branch_held_by_another_worktree_explains() {
    let repo = TestRepo::new();
    repo.commit("one");
    let wt = repo.add_worktree("held");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, checkout_branch("held")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    let wt = dunce::canonicalize(&wt).unwrap();
    assert!(
        out.message.contains("\u{201c}held\u{201d}")
            && out.message.contains(wt.to_string_lossy().as_ref())
            && out.message.contains("another worktree"),
        "{}",
        out.message
    );
    assert_eq!(current_branch(&repo).as_deref(), Some("main"));
}

#[tokio::test]
async fn delete_merged_branch_runs_without_confirmation() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("done");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, delete_branch("done")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(out.dangers.is_empty());
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/done"]));
    assert_eq!(out.snapshot.branches, ["main"]);
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["branch", "-d", "--", "done"])
    );

    // Confirming the danger does not make a merged branch's delete a forced one.
    repo.branch("done");
    let confirmed = [DangerKind::DeleteUnmergedBranch];
    let out = h.run_confirmed(id, delete_branch("done"), &confirmed).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["branch", "-d", "--", "done"]),
        "{:?}",
        op_records(&h.log, &out)
    );
}

#[tokio::test]
async fn delete_unmerged_branch_needs_confirmation() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("wip");
    repo.checkout("wip");
    repo.commit("two");
    repo.commit("three");
    repo.checkout("main");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let mark = h.last_log_id();
    let out = h.run(id, delete_branch("wip")).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert_eq!(out.dangers.len(), 1);
    assert_eq!(out.dangers[0].kind, DangerKind::DeleteUnmergedBranch);
    assert!(
        out.dangers[0].message.contains("2 commits"),
        "{:?}",
        out.dangers
    );
    assert!(out.message.contains("2 commits"));
    assert_eq!((out.log_from, out.log_to), (0, 0));
    assert!(
        h.writes_since(mark).is_empty(),
        "{:?}",
        h.writes_since(mark)
    );
    assert!(repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/wip"]));

    let out = h
        .run_confirmed(
            id,
            delete_branch("wip"),
            &[DangerKind::DeleteUnmergedBranch],
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/wip"]));
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["branch", "-D", "--", "wip"])
    );
}

/// Merged into HEAD but not into its own upstream: git's `-d` refuses that, so it is the
/// danger, and the message says where the commits are. Confirmed, `-D` deletes it.
#[tokio::test]
async fn branch_not_merged_into_its_upstream_needs_confirmation() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("topic");
    let _origin = repo.clone_as_remote();
    repo.git(&["branch", "-u", "origin/topic", "topic"]);
    repo.checkout("topic");
    repo.commit("local only");
    repo.checkout("main");
    repo.git(&["merge", "-q", "--ff-only", "topic"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, delete_branch("topic")).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert_eq!(out.dangers.len(), 1);
    assert_eq!(out.dangers[0].kind, DangerKind::DeleteUnmergedBranch);
    let message = &out.dangers[0].message;
    assert!(message.contains("1 commit "), "{message}");
    assert!(message.contains("origin/topic"), "{message}");
    assert!(
        message.contains("its commits are in main, but not yet in origin/topic"),
        "{message}"
    );

    let confirmed = [DangerKind::DeleteUnmergedBranch];
    let out = h
        .run_confirmed(id, delete_branch("topic"), &confirmed)
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/topic"]));
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["branch", "-D", "--", "topic"])
    );
}

/// Merged into its upstream but not into HEAD: git's `-d` deletes it (its commits are on
/// the remote), so there is no danger.
#[tokio::test]
async fn branch_merged_into_its_upstream_only_deletes_without_confirmation() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("topic");
    repo.checkout("topic");
    repo.commit("pushed");
    repo.checkout("main");
    let _origin = repo.clone_as_remote();
    repo.git(&["branch", "-u", "origin/topic", "topic"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, delete_branch("topic")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/heads/topic"]));
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["branch", "-d", "--", "topic"])
    );
}

// ---- Local operations: merge and rebase ----

/// `main` and `feature` both change `clash.txt` after `base`. HEAD on `main`.
fn conflicting(repo: &TestRepo) {
    repo.write("clash.txt", "base\n");
    repo.commit_all("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("clash.txt", "feature\n");
    repo.commit_all("feature side");
    repo.checkout("main");
    repo.write("clash.txt", "main\n");
    repo.commit_all("main side");
}

/// Whether `name` (`MERGE_HEAD`, `rebase-merge`, …) exists in the repository's git dir.
fn in_git_dir(repo: &TestRepo, name: &str) -> bool {
    repo.path().join(".git").join(name).exists()
}

fn parents(repo: &TestRepo, rev: &str) -> usize {
    repo.git(&["rev-list", "--parents", "-n", "1", rev])
        .split(' ')
        .count()
        - 1
}

#[tokio::test]
async fn merge_no_ff_creates_a_merge_commit_and_ff_does_not() {
    let repo = TestRepo::new();
    repo.commit("base");
    repo.branch("a");
    repo.branch("b");
    repo.checkout("a");
    let a_tip = repo.commit("on a");
    repo.checkout("b");
    let b_tip = repo.commit("on b");
    repo.checkout("main");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run(id, merge(MergeTargetKind::Branch, "a", true, false, false))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Merge a into main");
    assert_eq!(parents(&repo, "HEAD"), 2);
    assert_eq!(repo.git(&["rev-parse", "HEAD^2"]), a_tip);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "Merge branch 'a'");
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["merge", "--no-edit", "--no-ff", "a"])
    );
    // No remote: nothing to push to.
    assert!(out.hints.is_empty(), "{:?}", out.hints);

    repo.git(&["reset", "-q", "--hard", "HEAD~1"]);
    let out = h
        .run(id, merge(MergeTargetKind::Branch, "b", false, false, false))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), b_tip, "fast-forwarded");
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["merge", "--no-edit", "--ff", "b"])
    );
}

#[tokio::test]
async fn merge_of_a_remote_branch_and_of_a_commit() {
    let repo = TestRepo::new();
    let tip = diverged(&repo);
    let _origin = repo.clone_as_remote();
    let main_tip = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run(
            id,
            merge(
                MergeTargetKind::Remote,
                "origin/feature",
                true,
                false,
                false,
            ),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD^2"]), tip);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]),
        "Merge remote-tracking branch 'origin/feature'"
    );

    repo.git(&["reset", "-q", "--hard", &main_tip]);
    let out = h
        .run(id, merge(MergeTargetKind::Commit, &tip, true, false, false))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD^2"]), tip);
}

/// A tag named like the branch does not take its place.
#[tokio::test]
async fn merge_of_a_branch_named_like_a_tag_merges_the_branch() {
    let repo = TestRepo::new();
    repo.commit("base");
    let old = repo.git(&["rev-parse", "HEAD"]);
    let tip = diverged(&repo);
    repo.git(&["tag", "feature", &old]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, merge_branch("feature")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD^2"]), tip);
}

#[tokio::test]
async fn merge_squash_leaves_changes_staged() {
    let repo = TestRepo::new();
    diverged(&repo);
    let before = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    // noFf is ignored with squash (git refuses the two together).
    let out = h
        .run(
            id,
            merge(MergeTargetKind::Branch, "feature", true, true, false),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(
        repo.git(&["rev-parse", "HEAD"]),
        before,
        "nothing committed"
    );
    assert!(!repo.git(&["diff", "--cached", "--name-only"]).is_empty());
    assert!(!in_git_dir(&repo, "MERGE_HEAD"));
    assert!(out.hints.is_empty());
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["merge", "--no-edit", "--ff", "--squash", "feature"])
    );
}

#[tokio::test]
async fn merge_no_commit_leaves_merge_in_progress_without_conflicts() {
    let repo = TestRepo::new();
    diverged(&repo);
    let before = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run(
            id,
            merge(MergeTargetKind::Branch, "feature", true, false, true),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    assert!(in_git_dir(&repo, "MERGE_HEAD"));
    assert!(out.message.contains("commit"), "{}", out.message);
}

/// Without committing, even a merge that could fast-forward stays a merge in progress:
/// `--no-ff` is sent, so nothing is committed and no push is offered.
#[tokio::test]
async fn merge_no_commit_never_fast_forwards() {
    let repo = TestRepo::new();
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.commit("ahead");
    repo.checkout("main");
    let before = repo.git(&["rev-parse", "HEAD"]);
    let _origin = repo.clone_as_remote();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run(
            id,
            merge(MergeTargetKind::Branch, "feature", false, false, true),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    assert!(in_git_dir(&repo, "MERGE_HEAD"));
    assert!(out.hints.is_empty(), "{:?}", out.hints);
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args.first().is_some_and(|a| a == "merge")
                && r.args.iter().any(|a| a == "--no-ff")),
        "{:?}",
        op_records(&h.log, &out)
    );
}

#[tokio::test]
async fn merge_with_conflicts_stops() {
    let repo = TestRepo::new();
    conflicting(&repo);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let started = std::time::Instant::now();
    let out = h.run(id, merge_branch("feature")).await;
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    assert!(out.message.contains("conflict"), "{}", out.message);
    assert!(in_git_dir(&repo, "MERGE_HEAD"));
    // git's own output comes along on a stop too.
    let detail = out.detail.as_deref().unwrap_or_default();
    assert!(detail.contains("CONFLICT"), "{detail}");
}

/// Writes an executable hook `name` that prints "hook says no" and fails.
#[cfg(unix)]
fn failing_hook(repo: &TestRepo, name: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = repo.path().join(".git/hooks").join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "#!/bin/sh\necho 'hook says no' >&2\nexit 1\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A hook that refuses the merge commit leaves the merge staged, with no conflict: that
/// is a failure with git's (the hook's) output, not a stop on conflicts.
#[cfg(unix)]
#[tokio::test]
async fn merge_commit_refused_by_a_hook_fails_with_its_output() {
    let repo = TestRepo::new();
    diverged(&repo);
    failing_hook(&repo, "pre-merge-commit");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, merge_branch("feature")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert_eq!(
        out.message,
        "A hook stopped the merge commit; the merge is staged and not committed."
    );
    let detail = out.detail.as_deref().unwrap_or_default();
    assert!(detail.contains("hook says no"), "{detail}");
    assert!(in_git_dir(&repo, "MERGE_HEAD"));
}

/// A pre-rebase hook that refuses: the rebase never starts, and git's output is shown.
#[cfg(unix)]
#[tokio::test]
async fn rebase_refused_by_a_hook_fails_with_its_output() {
    let repo = TestRepo::new();
    diverged(&repo);
    failing_hook(&repo, "pre-rebase");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, rebase("feature")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    let detail = out.detail.as_deref().unwrap_or_default();
    assert!(detail.contains("hook says no"), "{detail}");
    assert!(!in_git_dir(&repo, "rebase-merge") && !in_git_dir(&repo, "rebase-apply"));
}

#[tokio::test]
async fn merge_already_up_to_date_is_done() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("same");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, merge_branch("same")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(
        out.message.starts_with("Already up to date"),
        "{}",
        out.message
    );
    assert!(out.hints.is_empty());
}

/// After a merge, push is offered: to the upstream, or, without one, to the only remote
/// (setting it as upstream).
#[tokio::test]
async fn merge_hints_push_when_there_is_somewhere_to_push() {
    let repo = TestRepo::new();
    diverged(&repo);
    let main_tip = repo.git(&["rev-parse", "HEAD"]);
    let _origin = repo.clone_as_remote();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, merge_branch("feature")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(
        out.hints,
        [OpHint::Push {
            branch: "main".into(),
            remote: "origin".into(),
            set_upstream: true,
        }]
    );

    repo.git(&["reset", "-q", "--hard", &main_tip]);
    repo.git(&["branch", "-q", "-u", "origin/main", "main"]);
    let out = h.run(id, merge_branch("feature")).await;
    assert_eq!(
        out.hints,
        [OpHint::Push {
            branch: "main".into(),
            remote: "origin".into(),
            set_upstream: false,
        }]
    );
}

/// `main` (HEAD) and `feature` after `base`: `feature` changes `clash.txt`, `main` adds
/// another file, and `clash.txt` has an uncommitted change on `main`, which conflicts with
/// `feature`'s once git puts it back after a merge or rebase with auto-stash.
fn dirty_against_feature(repo: &TestRepo) {
    repo.write("clash.txt", "base\n");
    repo.commit_all("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("clash.txt", "feature\n");
    repo.commit_all("feature side");
    repo.checkout("main");
    repo.commit("main side");
    repo.write("clash.txt", "dirty\n");
}

/// With auto-stash, git exits 0 even when the stashed changes conflict as they are put
/// back: done, saying that they are kept in the stash, with git's text as the detail.
#[tokio::test]
async fn autostash_that_cannot_be_reapplied_is_said() {
    for (config, request) in [
        ("rebase.autoStash", rebase("feature")),
        ("merge.autoStash", merge_branch("feature")),
    ] {
        let repo = TestRepo::new();
        dirty_against_feature(&repo);
        repo.git(&["config", config, "true"]);
        let h = harness().await;
        let id = h.open(repo.path()).await.repo_id;

        let out = h.run(id, request).await;
        assert_eq!(out.status, OpStatus::Done, "{config}: {out:?}");
        assert!(
            out.message.contains("kept in the stash"),
            "{config}: {}",
            out.message
        );
        let detail = out.detail.as_deref().unwrap_or_default();
        assert!(
            detail.contains("Applying autostash resulted in conflicts"),
            "{config}: {detail}"
        );
        assert!(repo.git(&["stash", "list"]).contains("autostash"));
    }
}

#[tokio::test]
async fn rebase_replays_the_branch_onto_another() {
    let repo = TestRepo::new();
    let feature_tip = diverged(&repo);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, rebase("feature")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Rebase main onto feature");
    assert_eq!(repo.git(&["rev-parse", "HEAD~1"]), feature_tip);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "on main");
    assert_eq!(current_branch(&repo).as_deref(), Some("main"));

    let out = h.run(id, rebase("feature")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(
        out.message.starts_with("Already up to date"),
        "{}",
        out.message
    );
}

#[tokio::test]
async fn rebase_onto_a_remote_branch() {
    let repo = TestRepo::new();
    let feature_tip = diverged(&repo);
    let _origin = repo.clone_as_remote();
    repo.git(&["branch", "-D", "feature"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, rebase("origin/feature")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD~1"]), feature_tip);
}

#[tokio::test]
async fn rebase_with_conflicts_stops() {
    let repo = TestRepo::new();
    conflicting(&repo);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let started = std::time::Instant::now();
    let out = h.run(id, rebase("feature")).await;
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    assert!(in_git_dir(&repo, "rebase-merge") || in_git_dir(&repo, "rebase-apply"));
}

#[tokio::test]
async fn operations_on_detached_head() {
    let repo = TestRepo::new();
    let feature_tip = diverged(&repo);
    repo.git(&["checkout", "-q", "--detach"]);
    let detached = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, merge_branch("feature")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Merge feature into detached HEAD");
    assert_eq!(current_branch(&repo), None);
    assert_eq!(repo.git(&["rev-parse", "HEAD^2"]), feature_tip);
    assert!(out.hints.is_empty());

    repo.git(&["checkout", "-q", "--detach", &detached]);
    let mark = h.last_log_id();
    let out = h.run(id, rebase("feature")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("HEAD is detached"), "{}", out.message);
    assert!(h.writes_since(mark).is_empty());
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), detached);
}

/// The detached-HEAD explanation, on git's own words for it.
#[tokio::test]
async fn no_current_branch_explains() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.git(&["checkout", "-q", "--detach"]);
    let runner = GitRunner::detect(CommandLog::new()).await.unwrap();
    for args in [&["rebase"][..], &["merge"][..]] {
        let out = runner
            .run_unchecked(repo.path(), args, CmdKind::Write)
            .await
            .unwrap();
        assert_ne!(out.code, 0);
        match explain(args, &out) {
            Step::Failed { message, .. } => {
                assert!(message.contains("HEAD is detached"), "{args:?}: {message}");
            }
            other => panic!("not a failure: {other:?}"),
        }
    }
}

// ---- Local operations: cherry-pick, revert and reset ----

fn cherry_pick(hash: &str, mainline: Option<u32>, no_commit: bool) -> OpRequest {
    OpRequest::CherryPick {
        hash: hash.to_owned(),
        mainline,
        no_commit,
    }
}

fn revert(hash: &str, mainline: Option<u32>) -> OpRequest {
    OpRequest::Revert {
        hash: hash.to_owned(),
        mainline,
    }
}

/// `main` with a merge of `side` on top; returns the merge commit.
fn with_merge(repo: &TestRepo) -> String {
    repo.commit("base");
    repo.branch("side");
    repo.checkout("side");
    repo.commit("on side");
    repo.checkout("main");
    repo.commit("on main");
    repo.merge("side")
}

#[tokio::test]
async fn cherry_pick_applies_the_commit() {
    let repo = TestRepo::new();
    let picked = diverged(&repo);
    let before = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, cherry_pick(&picked, None, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, format!("Cherry-pick {}", &picked[..7]));
    assert_eq!(repo.git(&["rev-parse", "HEAD~1"]), before);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "on feature");
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["cherry-pick", picked.as_str()])
    );
}

#[tokio::test]
async fn cherry_pick_without_committing_stages_the_changes() {
    let repo = TestRepo::new();
    let picked = diverged(&repo);
    let before = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, cherry_pick(&picked, None, true)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    assert!(!repo.git(&["diff", "--cached", "--name-only"]).is_empty());
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["cherry-pick", "--no-commit", picked.as_str()])
    );
}

#[tokio::test]
async fn cherry_pick_with_conflicts_stops() {
    let repo = TestRepo::new();
    conflicting(&repo);
    let picked = repo.git(&["rev-parse", "feature"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let started = std::time::Instant::now();
    let out = h.run(id, cherry_pick(&picked, None, false)).await;
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    assert!(in_git_dir(&repo, "CHERRY_PICK_HEAD"));
}

/// With `--no-commit` git leaves no cherry-pick to continue: the conflicts are explained
/// as a failure.
#[tokio::test]
async fn cherry_pick_without_committing_with_conflicts_explains() {
    let repo = TestRepo::new();
    conflicting(&repo);
    let picked = repo.git(&["rev-parse", "feature"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, cherry_pick(&picked, None, true)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("clash.txt"), "{}", out.message);
    assert!(out.message.contains("conflicts"), "{}", out.message);
    assert!(!in_git_dir(&repo, "CHERRY_PICK_HEAD"));
}

#[tokio::test]
async fn cherry_pick_merge_commit_without_mainline_explains() {
    let repo = TestRepo::new();
    let merge_commit = with_merge(&repo);
    repo.git(&["checkout", "-q", "-b", "elsewhere", "HEAD~1"]);
    let before = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, cherry_pick(&merge_commit, None, false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.contains("is a merge") && out.message.contains("mainline"),
        "{}",
        out.message
    );
    assert!(out.detail.unwrap().contains("no -m option was given"));
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);

    let out = h.run(id, cherry_pick(&merge_commit, Some(1), false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(parents(&repo, "HEAD"), 1);
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["cherry-pick", "-m", "1", merge_commit.as_str()])
    );

    let err =
        h.m.run_op(id, cherry_pick(&merge_commit, Some(0), false), &[])
            .await
            .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
}

/// Changes already there: nothing to commit, which is done, and no cherry-pick is left in
/// progress.
#[tokio::test]
async fn cherry_pick_of_changes_already_here_is_done() {
    let repo = TestRepo::new();
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    let picked = repo.commit("on feature");
    repo.checkout("main");
    repo.git(&["merge", "-q", "--ff-only", "feature"]);
    let before = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, cherry_pick(&picked, None, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(
        out.message.starts_with("Nothing to commit"),
        "{}",
        out.message
    );
    assert!(!in_git_dir(&repo, "CHERRY_PICK_HEAD"));
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
}

#[tokio::test]
async fn revert_undoes_the_commit() {
    let repo = TestRepo::new();
    repo.commit("base");
    let undone = repo.commit("mistake");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, revert(&undone, None)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD~1"]), undone);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]),
        "Revert \"mistake\""
    );
    assert!(repo.git(&["diff", "HEAD~2", "HEAD"]).is_empty());
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["revert", "--no-edit", undone.as_str()])
    );
}

#[tokio::test]
async fn revert_with_conflicts_stops() {
    let repo = TestRepo::new();
    repo.write("clash.txt", "one\n");
    repo.commit_all("one");
    repo.write("clash.txt", "two\n");
    let undone = repo.commit_all("two");
    repo.write("clash.txt", "three\n");
    repo.commit_all("three");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let started = std::time::Instant::now();
    let out = h.run(id, revert(&undone, None)).await;
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    assert!(in_git_dir(&repo, "REVERT_HEAD"));
}

#[tokio::test]
async fn revert_merge_commit_needs_a_mainline() {
    let repo = TestRepo::new();
    let merge_commit = with_merge(&repo);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, revert(&merge_commit, None)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.contains("to revert it") && out.message.contains("mainline"),
        "{}",
        out.message
    );

    let out = h.run(id, revert(&merge_commit, Some(1))).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD~1"]), merge_commit);
}

#[tokio::test]
async fn revert_of_changes_already_undone_is_done() {
    let repo = TestRepo::new();
    repo.commit("base");
    let undone = repo.commit("mistake");
    repo.git(&["revert", "--no-edit", &undone]);
    let before = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, revert(&undone, None)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(
        out.message.starts_with("Nothing to commit"),
        "{}",
        out.message
    );
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    assert!(!in_git_dir(&repo, "REVERT_HEAD"));
}

#[tokio::test]
async fn soft_and_mixed_reset_run_without_confirmation() {
    let repo = TestRepo::new();
    let first = repo.commit("one");
    let second = repo.commit("two");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, reset(&first, ResetMode::Soft)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), first);
    assert!(
        !repo.git(&["diff", "--cached", "--name-only"]).is_empty(),
        "staged"
    );
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["reset", "--soft", first.as_str()])
    );

    repo.git(&["reset", "-q", "--hard", &second]);
    let out = h.run(id, reset(&first, ResetMode::Mixed)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), first);
    assert!(
        repo.git(&["diff", "--cached", "--name-only"]).is_empty(),
        "not staged"
    );
    assert!(
        !repo.git(&["status", "--porcelain"]).is_empty(),
        "kept in the working tree"
    );
}

#[tokio::test]
async fn hard_reset_needs_confirmation() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    let first = repo.commit_all("one");
    repo.commit("two");
    repo.write("a.txt", "changed\n");
    repo.write("b.txt", "changed\n");
    repo.write("new.txt", "untracked\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let mark = h.last_log_id();
    let out = h.run(id, reset(&first, ResetMode::Hard)).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    assert_eq!(out.dangers.len(), 1);
    let danger = &out.dangers[0];
    assert_eq!(danger.kind, DangerKind::HardReset);
    assert!(danger.message.contains("2 files"), "{}", danger.message);
    assert!(
        danger.message.contains("1 untracked file is kept"),
        "{}",
        danger.message
    );
    assert!(h.writes_since(mark).is_empty());
    assert_eq!(
        fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "changed\n"
    );

    let out = h
        .run_confirmed(id, reset(&first, ResetMode::Hard), &[DangerKind::HardReset])
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), first);
    assert_eq!(
        fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "a\n"
    );
    assert!(
        repo.path().join("new.txt").exists(),
        "untracked files are kept"
    );
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["reset", "--hard", first.as_str()])
    );
}

// ---- Local operations: tags ----

fn create_tag(name: &str, hash: &str, message: Option<&str>) -> OpRequest {
    OpRequest::CreateTag {
        name: name.to_owned(),
        hash: hash.to_owned(),
        message: message.map(str::to_owned),
    }
}

#[tokio::test]
async fn create_tag_lightweight_and_annotated() {
    let repo = TestRepo::new();
    let first = repo.commit("one");
    let second = repo.commit("two");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, create_tag("v1", &first, None)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Create tag v1");
    assert_eq!(repo.git(&["cat-file", "-t", "refs/tags/v1"]), "commit");
    assert_eq!(repo.git(&["rev-parse", "refs/tags/v1"]), first);
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["tag", "--", "v1", first.as_str()])
    );

    // A message that looks like an option is still a message.
    let out = h
        .run(id, create_tag("v2", &second, Some("-release two")))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(repo.git(&["cat-file", "-t", "refs/tags/v2"]), "tag");
    assert_eq!(repo.git(&["rev-parse", "refs/tags/v2^{commit}"]), second);
    assert_eq!(
        repo.git(&["tag", "-l", "--format=%(contents:subject)", "v2"]),
        "-release two"
    );

    let out = h.run(id, create_tag("v1", &second, None)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert_eq!(
        out.message,
        "A tag named \u{201c}v1\u{201d} already exists."
    );
    assert_eq!(repo.git(&["rev-parse", "refs/tags/v1"]), first);

    for name in ["-v3", "a..b", "HEAD"] {
        let err =
            h.m.run_op(id, create_tag(name, &first, None), &[])
                .await
                .unwrap_err();
        assert_eq!(err.code, ErrorCode::BadName, "{name}");
    }
    let err =
        h.m.run_op(id, create_tag("v4", &first, Some("  ")), &[])
            .await
            .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
}

#[tokio::test]
async fn delete_tag_deletes_it_locally() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    repo.git(&["tag", "v1", &head]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let request = |remote: Option<&str>| OpRequest::DeleteTag {
        name: "v1".into(),
        remote: remote.map(str::to_owned),
    };
    // On a remote too: the remote goes first, so when it fails the tag stays here.
    let out = h.run(id, request(Some("origin"))).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.contains("\u{201c}origin\u{201d}"),
        "{}",
        out.message
    );
    assert!(repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/tags/v1"]));

    let out = h.run(id, request(None)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Delete tag v1");
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "refs/tags/v1"]));
    assert!(
        op_records(&h.log, &out)
            .iter()
            .any(|r| r.args == ["tag", "-d", "--", "v1"])
    );

    let out = h.run(id, request(None)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("not found"), "{}", out.message);
}

/// Before the first commit nothing can be checked out, merged or tagged: every request
/// ends in a plain failure (an outcome or an error with a message), none panics or hangs.
#[tokio::test]
async fn empty_repository() {
    let repo = TestRepo::new();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let hash = "0123456789abcdef0123456789abcdef01234567";

    let requests = vec![
        checkout_branch("main"),
        OpRequest::CheckoutRemote {
            remote_branch: "origin/main".into(),
            local_name: "x".into(),
        },
        OpRequest::CheckoutCommit { hash: hash.into() },
        create_branch("x", hash),
        OpRequest::CreateBranch {
            name: "x".into(),
            at: hash.into(),
            checkout: true,
        },
        // (Renaming the unborn branch itself works.)
        OpRequest::RenameBranch {
            branch: "nope".into(),
            new_name: "trunk".into(),
        },
        delete_branch("main"),
        merge_branch("main"),
        merge(MergeTargetKind::Commit, hash, false, true, true),
        rebase("main"),
        cherry_pick(hash, None, false),
        revert(hash, Some(1)),
        reset(hash, ResetMode::Soft),
        reset(hash, ResetMode::Hard),
        create_tag("v1", hash, None),
        OpRequest::DeleteTag {
            name: "v1".into(),
            remote: None,
        },
    ];
    for request in requests {
        let shown = format!("{request:?}");
        let confirmed = [DangerKind::HardReset, DangerKind::DeleteUnmergedBranch];
        match tokio::time::timeout(TIMEOUT, h.m.run_op(id, request, &confirmed)).await {
            Err(_) => panic!("{shown}: did not end"),
            Ok(Err(e)) => assert!(!e.message.is_empty(), "{shown}: {e:?}"),
            Ok(Ok(out)) => {
                assert_eq!(out.status, OpStatus::Failed, "{shown}: {out:?}");
                assert!(!out.message.is_empty(), "{shown}");
                assert_eq!(out.snapshot.op, None);
            }
        }
    }
    // Still empty, and still usable.
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "--quiet", "HEAD"]));
    assert_eq!(h.m.snapshot(id).await.unwrap().op, None);
}

#[tokio::test]
async fn checkout_with_untracked_files_in_the_way_explains() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("other");
    repo.checkout("other");
    repo.write("made.txt", "tracked on other\n");
    repo.commit_all("add made.txt");
    repo.checkout("main");
    repo.write("made.txt", "mine, untracked\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, checkout_branch("other")).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message
            .starts_with("Untracked files would be overwritten by checkout: made.txt."),
        "{}",
        out.message
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("made.txt")).unwrap(),
        "mine, untracked\n"
    );
}
