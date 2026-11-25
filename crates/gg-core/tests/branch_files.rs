//! `branch_files`: the files a worktree's branch changes in its own commits, compared from
//! where it left its base, for the Commit tab's "Committed on branch". On real throwaway
//! repositories; the pull request case against the fake `gh` (no network host).

mod common;

use std::path::Path;

use common::{Harness, TIMEOUT, harness};
use gg_core::dto::{BranchFiles, FileChange, FileStatus, MAX_BRANCH_FILES};
use gg_core::error::ErrorCode;
use gg_core::git::runner::{CommandRecord, LogFilter};
use gg_core::testutil::TestRepo;

/// A repository with `main` (three files) pushed to `origin`, whose HEAD names `main`.
fn with_origin() -> (TestRepo, TestRepo) {
    let repo = TestRepo::new();
    repo.write("keep.txt", "keep\n");
    repo.write("old-name.txt", "one\ntwo\nthree\nfour\nfive\n");
    repo.write("doomed.txt", "bye\n");
    repo.commit_all("base");
    let origin = repo.clone_as_remote();
    repo.git(&["remote", "set-head", "origin", "main"]);
    (repo, origin)
}

/// The git commands run since record `since`.
fn since(h: &Harness, since: u64) -> Vec<CommandRecord> {
    h.log.query(LogFilter {
        since_id: since,
        repo_id: None,
        writes_only: false,
    })
}

fn last_id(h: &Harness) -> u64 {
    since(h, 0).last().map_or(0, |r| r.id)
}

fn merge_bases(records: &[CommandRecord]) -> usize {
    records
        .iter()
        .filter(|r| r.args.first().map(String::as_str) == Some("merge-base"))
        .count()
}

/// Reads the repository again and waits until the snapshot shows it (its log, status and
/// worktrees read).
async fn refresh(h: &Harness, id: u32) {
    h.m.refresh(id).await.expect("refresh");
    tokio::time::timeout(TIMEOUT, h.m.wait_loaded(id))
        .await
        .expect("load finishes");
    tokio::time::timeout(TIMEOUT, h.m.wait_status(id))
        .await
        .expect("status read");
    tokio::time::timeout(TIMEOUT, h.m.wait_background(id))
        .await
        .expect("background reads end");
}

async fn files_of(h: &Harness, id: u32, path: &Path) -> Option<BranchFiles> {
    tokio::time::timeout(TIMEOUT, h.m.branch_files(id, path))
        .await
        .expect("answers")
        .expect("no error")
}

fn change(
    path: &str,
    old: Option<&str>,
    status: FileStatus,
    add: Option<u32>,
    del: Option<u32>,
) -> FileChange {
    FileChange {
        path: path.into(),
        old_path: old.map(Into::into),
        status,
        additions: add,
        deletions: del,
    }
}

#[tokio::test]
async fn a_branch_lists_what_its_commits_change_from_where_it_left_main() {
    let (repo, _origin) = with_origin();
    repo.git(&["checkout", "-q", "-b", "feature"]);
    // A rename with an edit, a delete, a binary file, an added and a modified one.
    repo.git(&["mv", "old-name.txt", "new-name.txt"]);
    repo.write("new-name.txt", "one\ntwo\nthree\nfour\nfive\nsix\n");
    repo.git(&["rm", "-q", "doomed.txt"]);
    repo.write("logo.bin", [0u8, 159, 146, 150, 0, 1, 2]);
    repo.write("src/added.rs", "fn a() {}\nfn b() {}\n");
    repo.commit_all("work");
    repo.write("keep.txt", "keep\nmore\n");
    repo.commit_all("more work");
    // main moves on after the branch left it: none of that is the branch's.
    repo.checkout("main");
    repo.write("main-only.txt", "main\n");
    repo.commit_all("main moves");
    repo.git(&["push", "-q", "origin", "main"]);
    repo.checkout("feature");
    // Uncommitted work is not the branch's committed work either.
    repo.write("keep.txt", "keep\nmore\nuncommitted\n");
    repo.write("untracked.txt", "u\n");

    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let mark = last_id(&h);
    let got = files_of(&h, snap.repo_id, repo.path())
        .await
        .expect("a list");
    assert_eq!(got.branch, "feature");
    assert_eq!(got.base, "origin/main");
    assert_eq!(got.base_commit, repo.git(&["rev-parse", "origin/main"]));
    assert_eq!(
        got.merge_base,
        repo.git(&["merge-base", "origin/main", "HEAD"])
    );
    assert_eq!(got.head, repo.git(&["rev-parse", "HEAD"]));
    assert_eq!(
        got.files,
        [
            change("doomed.txt", None, FileStatus::D, Some(0), Some(1)),
            change("keep.txt", None, FileStatus::M, Some(1), Some(0)),
            change("logo.bin", None, FileStatus::A, None, None),
            change(
                "new-name.txt",
                Some("old-name.txt"),
                FileStatus::R,
                Some(1),
                Some(0)
            ),
            change("src/added.rs", None, FileStatus::A, Some(2), Some(0)),
        ]
    );
    assert_eq!(got.total, 5);
    // Read-only: reads, never a write.
    let ran = since(&h, mark);
    assert_eq!(merge_bases(&ran), 1);
    assert!(
        ran.iter()
            .all(|r| r.kind == gg_core::git::runner::CmdKind::Read),
        "{ran:?}"
    );
    assert!(
        h.log
            .query(LogFilter {
                since_id: mark,
                repo_id: None,
                writes_only: true
            })
            .is_empty()
    );
}

#[tokio::test]
async fn the_same_range_is_read_once_and_again_when_head_or_the_base_moves() {
    let (repo, _origin) = with_origin();
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit("one");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let id = snap.repo_id;
    let mark = last_id(&h);
    // Asked at once, twice: one read, both answered.
    let (a, b) = tokio::join!(files_of(&h, id, repo.path()), files_of(&h, id, repo.path()));
    assert_eq!(a, b);
    assert_eq!(a.as_ref().unwrap().files.len(), 1);
    // Asked again with nothing moved: the same answer, no git.
    assert_eq!(files_of(&h, id, repo.path()).await, a);
    assert_eq!(merge_bases(&since(&h, mark)), 1);

    // A commit on the branch: read again.
    repo.commit("two");
    refresh(&h, id).await;
    let c = files_of(&h, id, repo.path()).await.unwrap();
    assert_eq!(c.files.len(), 2);
    assert_eq!(merge_bases(&since(&h, mark)), 2);

    // The base moves (main gets the branch's first commit): read again, from there.
    repo.git(&["push", "-q", "origin", "HEAD~1:main"]);
    repo.git(&["fetch", "-q", "origin"]);
    refresh(&h, id).await;
    let d = files_of(&h, id, repo.path()).await.unwrap();
    assert_eq!(d.files.len(), 1);
    assert_eq!(d.base_commit, repo.git(&["rev-parse", "HEAD~1"]));
    assert_eq!(merge_bases(&since(&h, mark)), 3);
}

#[tokio::test]
async fn none_when_detached_on_the_default_branch_with_no_remote_or_nothing_changed() {
    let (repo, _origin) = with_origin();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    // The default branch itself.
    assert_eq!(files_of(&h, id, repo.path()).await, None);
    // A branch with no commits of its own.
    repo.git(&["checkout", "-q", "-b", "feature"]);
    refresh(&h, id).await;
    assert_eq!(files_of(&h, id, repo.path()).await, None);
    // Detached.
    repo.commit("one");
    repo.git(&["checkout", "-q", "--detach"]);
    refresh(&h, id).await;
    assert_eq!(files_of(&h, id, repo.path()).await, None);
    // The remote's HEAD unknown: no base.
    repo.checkout("feature");
    repo.git(&["remote", "set-head", "origin", "-d"]);
    refresh(&h, id).await;
    assert_eq!(files_of(&h, id, repo.path()).await, None);

    // No remote at all.
    let alone = TestRepo::new();
    alone.commit("base");
    alone.git(&["checkout", "-q", "-b", "feature"]);
    alone.commit("work");
    let id = h.open(alone.path()).await.repo_id;
    assert_eq!(files_of(&h, id, alone.path()).await, None);
}

#[tokio::test]
async fn another_worktrees_branch_and_a_path_that_is_none() {
    let (repo, _origin) = with_origin();
    let wt = repo.add_worktree("topic");
    std::fs::write(wt.join("topic.txt"), "t\n").unwrap();
    repo.git(&["-C", wt.to_str().unwrap(), "add", "topic.txt"]);
    repo.git(&["-C", wt.to_str().unwrap(), "commit", "-q", "-m", "topic"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    refresh(&h, id).await;
    let got = files_of(&h, id, &wt).await.expect("the topic worktree's");
    assert_eq!(got.branch, "topic");
    assert_eq!(
        got.files,
        [change("topic.txt", None, FileStatus::A, Some(1), Some(0))]
    );
    // The main worktree is on main: none.
    assert_eq!(files_of(&h, id, repo.path()).await, None);
    let err =
        h.m.branch_files(id, &repo.path().join("nowhere"))
            .await
            .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
}

#[tokio::test]
async fn at_most_500_files_with_how_many_in_all() {
    let (repo, _origin) = with_origin();
    repo.git(&["checkout", "-q", "-b", "many"]);
    for i in 0..(MAX_BRANCH_FILES + 20) {
        repo.write(&format!("f/{i:04}.txt"), format!("{i}\n"));
    }
    repo.commit_all("many");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let got = files_of(&h, id, repo.path()).await.unwrap();
    assert_eq!(got.files.len(), MAX_BRANCH_FILES);
    assert_eq!(got.total as usize, MAX_BRANCH_FILES + 20);
    assert_eq!(got.files[0].path, "f/0000.txt");
}

/// The base of a branch with an open pull request is the pull request's base (here
/// `develop`), not the remote's default branch.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_base_is_the_pull_requests_when_it_has_one() {
    use gg_core::testutil::FakeGh;

    const GH_URL: &str = "https://github.com/octo/cat.git";
    let (repo, origin) = with_origin();
    repo.git(&["checkout", "-q", "-b", "develop"]);
    repo.write("develop.txt", "d\n");
    repo.commit_all("develop");
    repo.git(&["push", "-q", "origin", "develop"]);
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("feature.txt", "f\n");
    repo.commit_all("feature");
    repo.git(&["push", "-q", "-u", "origin", "feature"]);
    // The app sees GitHub; git goes to the local bare repository.
    repo.git(&["remote", "set-url", "origin", GH_URL]);
    let key = format!("url.{}.insteadOf", origin.path().display());
    repo.git(&["config", "--add", &key, GH_URL]);

    let gh = FakeGh::new();
    let h = harness().await;
    h.m.set_tool_path(Some(gh.path()));
    let id = h.open(repo.path()).await.repo_id;
    // Without the pull request known: the remote's default, main.
    let got = files_of(&h, id, repo.path()).await.unwrap();
    assert_eq!(got.base, "origin/main");
    assert_eq!(got.files.len(), 2);

    gh.reply(
        "pr-list",
        0,
        &serde_json::json!([{
            "number": 5,
            "title": "Feature",
            "url": "https://github.com/octo/cat/pull/5",
            "isDraft": false,
            "headRefName": "feature",
            "baseRefName": "develop",
            "headRepositoryOwner": {"id": "X", "login": "octo"},
            "isCrossRepository": false,
        }])
        .to_string(),
        "",
    );
    let before = h.m.prs_loads(id);
    h.m.refresh_by_user(id).await.unwrap();
    tokio::time::timeout(TIMEOUT, h.m.wait_prs_loads(id, before))
        .await
        .expect("the pull requests load");
    assert_eq!(
        h.m.snapshot(id).await.unwrap().branch_prs["feature"].base,
        "develop"
    );
    let got = files_of(&h, id, repo.path()).await.unwrap();
    assert_eq!(got.base, "origin/develop");
    assert_eq!(
        got.files,
        [change("feature.txt", None, FileStatus::A, Some(1), Some(0))]
    );
}
