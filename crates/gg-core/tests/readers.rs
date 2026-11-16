//! Integration tests for the git readers (log, refs, worktrees, status, commit details)
//! against real temp repositories.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use gg_core::dto::{
    ConflictFile, ConflictState, FileChange, FileStatus, InProgress, InProgressKind, ViewOptions,
    WorktreeStatus,
};
use gg_core::error::ErrorCode;
use gg_core::git::details::read_details;
use gg_core::git::log::{Commit, LogParser, log_args, read_log};
use gg_core::git::progress::read_in_progress;
use gg_core::git::refs::{RefKind, read_head, read_refs, read_remotes};
use gg_core::git::runner::{CmdKind, CommandLog, GitRunner};
use gg_core::git::status::read_status;
use gg_core::git::worktrees::read_worktrees;
use gg_core::testutil::TestRepo;

async fn runner() -> GitRunner {
    GitRunner::detect(CommandLog::new())
        .await
        .expect("git is installed")
}

fn all_refs() -> ViewOptions {
    ViewOptions {
        branches: None,
        show_remotes: true,
        show_tags: true,
        show_stashes: true,
    }
}

fn canonical(p: &Path) -> String {
    dunce::canonicalize(p)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// Runs `read_log` and returns the batches it delivered.
async fn log_batches(git: &GitRunner, cwd: &Path, args: &[String]) -> Vec<Vec<Commit>> {
    let mut batches = Vec::new();
    read_log(git, cwd, args, |b| batches.push(b))
        .await
        .expect("read_log");
    batches
}

fn hashes(commits: &[Commit]) -> Vec<&str> {
    commits.iter().map(|c| c.hash.as_str()).collect()
}

/// main: c1 - c2 ------ c3 - m
///                \- f -------/   (branch `feature`, merged with --no-ff)
struct MergeHistory {
    repo: TestRepo,
    c1: String,
    c2: String,
    f: String,
    c3: String,
    m: String,
}

fn merge_history() -> MergeHistory {
    let repo = TestRepo::new();
    let c1 = repo.commit("one");
    let c2 = repo.commit("two");
    repo.branch("feature");
    repo.checkout("feature");
    let f = repo.commit("feat");
    repo.checkout("main");
    let c3 = repo.commit("three");
    let m = repo.merge("feature");
    MergeHistory {
        repo,
        c1,
        c2,
        f,
        c3,
        m,
    }
}

// ---------------------------------------------------------------------------------------
// log

#[tokio::test]
async fn log_reads_real_repo() {
    use tokio::io::AsyncReadExt;

    let h = merge_history();
    let git = runner().await;
    let args = log_args(&all_refs(), &[]);
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = git.spawn_stream(h.repo.path(), &argv).unwrap();
    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .await
        .unwrap();
    assert!(child.wait().await.unwrap().success());

    let mut parser = LogParser::new();
    let mut commits = Vec::new();
    parser.feed(&stdout, &mut commits);
    parser.finish(&mut commits);

    assert_eq!(hashes(&commits), [&h.m, &h.c3, &h.f, &h.c2, &h.c1]);
    let merge = &commits[0];
    assert_eq!(merge.parents, [h.c3.clone(), h.f.clone()]);
    assert_eq!(merge.subject, "Merge branch 'feature'");
    assert!(commits.iter().all(|c| c.author == "Test"));
    assert!(commits.iter().all(|c| c.email == "test@example.com"));
    let root = commits.last().unwrap();
    assert!(root.parents.is_empty());
    assert_eq!(root.subject, "one");
    assert_eq!(root.time, 1_700_000_000);
}

#[tokio::test]
async fn read_log_matches_parsed_output_and_logs_the_command() {
    let h = merge_history();
    let log = CommandLog::new();
    let git = GitRunner::detect(log.clone()).await.unwrap();
    let args = log_args(&all_refs(), &[]);
    let batches = log_batches(&git, h.repo.path(), &args).await;
    assert!(batches.iter().all(|b| !b.is_empty()), "no empty batches");
    let commits: Vec<Commit> = batches.concat();
    assert_eq!(hashes(&commits), [&h.m, &h.c3, &h.f, &h.c2, &h.c1]);

    let records = log.since(0);
    let rec = records.last().expect("log command recorded");
    assert_eq!(rec.args, args);
    assert_eq!(rec.exit_code, 0);
}

#[tokio::test]
async fn read_log_delivers_batches_as_output_arrives() {
    // ~400 KiB of log output cannot arrive in one pipe read.
    let repo = TestRepo::new();
    let long = "s".repeat(20_000);
    for i in 0..20 {
        repo.commit(&format!("{i}{long}"));
    }
    let git = runner().await;
    let batches = log_batches(&git, repo.path(), &log_args(&all_refs(), &[])).await;
    assert!(batches.len() > 1, "got {} batch(es)", batches.len());
    let commits = batches.concat();
    assert_eq!(commits.len(), 20);
    assert_eq!(commits[0].subject, format!("19{long}"));
    assert_eq!(commits[19].subject, format!("0{long}"));
}

#[tokio::test]
async fn read_log_empty_repo_is_ok_without_batches() {
    let repo = TestRepo::new();
    let git = runner().await;
    for opts in [
        all_refs(),
        ViewOptions {
            branches: Some(vec!["main".into()]),
            show_remotes: false,
            show_tags: false,
            show_stashes: true,
        },
    ] {
        let batches = log_batches(&git, repo.path(), &log_args(&opts, &[])).await;
        assert!(batches.is_empty(), "{opts:?}: {batches:?}");
    }
}

#[tokio::test]
async fn read_log_with_unborn_head_shows_other_branches() {
    let repo = TestRepo::new();
    let c1 = repo.commit("one");
    repo.git(&["checkout", "-q", "--orphan", "fresh"]);
    let git = runner().await;
    let commits = log_batches(&git, repo.path(), &log_args(&all_refs(), &[]))
        .await
        .concat();
    assert_eq!(hashes(&commits), [&c1]);
}

#[tokio::test]
async fn read_log_with_files_named_like_revisions() {
    // Without a final `--`, git cannot tell the revision `HEAD` from the file `HEAD`.
    let repo = TestRepo::new();
    repo.write("HEAD", "a file, not a revision\n");
    repo.write("main", "also a file\n");
    let c1 = repo.commit_all("files named HEAD and main");
    let c2 = repo.commit("two");
    let git = runner().await;
    let commits = log_batches(&git, repo.path(), &log_args(&all_refs(), &[]))
        .await
        .concat();
    assert_eq!(hashes(&commits), [&c2, &c1]);

    let opts = ViewOptions {
        branches: Some(vec!["main".into()]),
        show_remotes: false,
        show_tags: false,
        show_stashes: true,
    };
    let commits = log_batches(&git, repo.path(), &log_args(&opts, &[]))
        .await
        .concat();
    assert_eq!(hashes(&commits), [&c2, &c1]);
}

#[tokio::test]
async fn read_log_ignores_show_signature_config() {
    let repo = TestRepo::new();
    let c1 = repo.commit("one");
    let c2 = repo.commit("two");
    repo.git(&["config", "log.showSignature", "true"]);
    let git = runner().await;
    let effective = git
        .run(
            repo.path(),
            &["config", "--get", "log.showSignature"],
            CmdKind::Read,
        )
        .await
        .unwrap();
    assert_eq!(effective.text().trim(), "false");
    let commits = log_batches(&git, repo.path(), &log_args(&all_refs(), &[]))
        .await
        .concat();
    assert_eq!(hashes(&commits), [&c2, &c1]);
}

#[tokio::test]
async fn read_log_unknown_branch_is_git_failed() {
    let repo = TestRepo::new();
    repo.commit("one");
    let git = runner().await;
    let opts = ViewOptions {
        branches: Some(vec!["nope".into()]),
        show_remotes: false,
        show_tags: false,
        show_stashes: true,
    };
    let err = read_log(&git, repo.path(), &log_args(&opts, &[]), |_| {
        panic!("no batches expected")
    })
    .await
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::GitFailed);
}

#[tokio::test]
async fn read_log_selected_branches() {
    let h = merge_history();
    let git = runner().await;
    let opts = ViewOptions {
        branches: Some(vec!["feature".into()]),
        show_remotes: false,
        show_tags: false,
        show_stashes: true,
    };
    let commits = log_batches(&git, h.repo.path(), &log_args(&opts, &[]))
        .await
        .concat();
    assert_eq!(hashes(&commits), [&h.f, &h.c2, &h.c1]);
}

// ---------------------------------------------------------------------------------------
// refs and HEAD

#[tokio::test]
async fn refs_branches_tags_remotes() {
    let repo = TestRepo::new();
    let c1 = repo.commit("one");
    repo.branch("feature/ünï-✨");
    repo.git(&["tag", "-a", "v1", "-m", "release one"]);
    let _origin = repo.clone_as_remote();
    repo.git(&["branch", "-q", "--set-upstream-to=origin/main", "main"]);
    repo.git(&["remote", "set-head", "origin", "main"]);
    let c2 = repo.commit("two");

    let git = runner().await;
    let refs = read_refs(&git, repo.path()).await.unwrap();
    let names: BTreeSet<(RefKind, &str)> = refs.iter().map(|r| (r.kind, r.name.as_str())).collect();
    let expected: BTreeSet<(RefKind, &str)> = [
        (RefKind::Branch, "main"),
        (RefKind::Branch, "feature/ünï-✨"),
        (RefKind::Remote, "origin/main"),
        (RefKind::Remote, "origin/feature/ünï-✨"),
        (RefKind::Tag, "v1"),
    ]
    .into_iter()
    .collect();
    assert_eq!(names, expected);
    assert_eq!(refs.len(), expected.len(), "no duplicates: {refs:?}");
    assert!(refs.iter().all(|r| r.name != "origin/HEAD"));

    let find = |kind: RefKind, name: &str| {
        refs.iter()
            .find(|r| r.kind == kind && r.name == name)
            .unwrap()
    };
    let tag = find(RefKind::Tag, "v1");
    assert_eq!(tag.hash, c1, "annotated tag is peeled to its commit");
    let main = find(RefKind::Branch, "main");
    assert_eq!(main.hash, c2);
    assert_eq!(main.upstream.as_deref(), Some("origin/main"));
    assert_eq!((main.ahead, main.behind), (1, 0));
    let feature = find(RefKind::Branch, "feature/ünï-✨");
    assert_eq!(feature.hash, c1);
    assert_eq!(feature.upstream, None);
    assert_eq!((feature.ahead, feature.behind), (0, 0));
    assert_eq!(find(RefKind::Remote, "origin/main").hash, c1);

    assert_eq!(read_remotes(&git, repo.path()).await.unwrap(), ["origin"]);
}

#[tokio::test]
async fn refs_upstream_gone_keeps_name_with_zero_counts() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("topic");
    let _origin = repo.clone_as_remote();
    repo.git(&["branch", "-q", "--set-upstream-to=origin/topic", "topic"]);
    repo.git(&["update-ref", "-d", "refs/remotes/origin/topic"]);

    let git = runner().await;
    let refs = read_refs(&git, repo.path()).await.unwrap();
    let topic = refs
        .iter()
        .find(|r| r.kind == RefKind::Branch && r.name == "topic")
        .unwrap();
    assert_eq!(topic.upstream.as_deref(), Some("origin/topic"));
    assert_eq!((topic.ahead, topic.behind), (0, 0));
}

#[tokio::test]
async fn refs_behind_upstream() {
    let repo = TestRepo::new();
    repo.commit("one");
    let _origin = repo.clone_as_remote();
    repo.git(&["branch", "-q", "--set-upstream-to=origin/main", "main"]);
    repo.commit("two");
    repo.commit("three");
    repo.git(&["push", "-q", "origin", "main"]);
    repo.git(&["reset", "-q", "--hard", "HEAD~2"]);
    repo.commit("local");

    let git = runner().await;
    let refs = read_refs(&git, repo.path()).await.unwrap();
    let main = refs
        .iter()
        .find(|r| r.kind == RefKind::Branch && r.name == "main")
        .unwrap();
    assert_eq!((main.ahead, main.behind), (1, 2));
}

#[tokio::test]
async fn head_on_branch() {
    let repo = TestRepo::new();
    let c1 = repo.commit("one");
    let git = runner().await;
    let head = read_head(&git, repo.path()).await.unwrap();
    assert_eq!(head.branch.as_deref(), Some("main"));
    assert_eq!(head.hash, Some(c1));
}

#[tokio::test]
async fn head_detached() {
    let repo = TestRepo::new();
    let c1 = repo.commit("one");
    repo.commit("two");
    repo.git(&["checkout", "-q", "--detach", &c1]);
    let git = runner().await;
    let head = read_head(&git, repo.path()).await.unwrap();
    assert_eq!(head.branch, None);
    assert_eq!(head.hash, Some(c1));
}

#[tokio::test]
async fn head_empty_repo() {
    let repo = TestRepo::new();
    let git = runner().await;
    let head = read_head(&git, repo.path()).await.unwrap();
    assert_eq!(head.branch.as_deref(), Some("main"));
    assert_eq!(head.hash, None);
    assert!(read_refs(&git, repo.path()).await.unwrap().is_empty());
    assert!(read_remotes(&git, repo.path()).await.unwrap().is_empty());
}

#[tokio::test]
async fn readers_outside_a_repo_fail() {
    let dir = tempfile::tempdir().unwrap();
    let git = runner().await;
    let err = read_head(&git, dir.path()).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::GitFailed);
    let err = read_log(&git, dir.path(), &log_args(&all_refs(), &[]), |_| {})
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::GitFailed);
}

// ---------------------------------------------------------------------------------------
// worktrees

#[tokio::test]
async fn worktrees_lists_main_and_linked() {
    let repo = TestRepo::new();
    let c1 = repo.commit("one");
    let wt = repo.add_worktree("wt-a");
    let git = runner().await;
    let list = read_worktrees(&git, repo.path()).await.unwrap();
    assert_eq!(list.len(), 2, "{list:?}");

    assert!(list[0].is_main);
    assert_eq!(list[0].path, canonical(repo.path()));
    assert_eq!(list[0].branch.as_deref(), Some("main"));
    assert_eq!(list[0].head.as_deref(), Some(c1.as_str()));

    assert!(!list[1].is_main);
    assert_eq!(list[1].path, canonical(&wt));
    assert_eq!(list[1].branch.as_deref(), Some("wt-a"));
    assert_eq!(list[1].head.as_deref(), Some(c1.as_str()));
    assert!(list.iter().all(|w| !w.locked && !w.prunable));

    // Asked from inside the linked worktree, the main one is still first.
    let from_linked = read_worktrees(&git, &wt).await.unwrap();
    assert_eq!(from_linked, list);
}

#[tokio::test]
async fn worktrees_locked_prunable_and_detached() {
    let repo = TestRepo::new();
    let c1 = repo.commit("one");
    let locked = repo.add_worktree("locked");
    let gone = repo.add_worktree("gone");
    repo.git(&[
        "worktree",
        "lock",
        "--reason",
        "on a usb disk",
        locked.to_str().unwrap(),
    ]);
    fs::remove_dir_all(&gone).unwrap();
    repo.git(&["checkout", "-q", "--detach"]);

    let git = runner().await;
    let list = read_worktrees(&git, repo.path()).await.unwrap();
    assert_eq!(list.len(), 3, "{list:?}");
    assert_eq!(list[0].branch, None, "main worktree is detached");
    assert_eq!(list[0].head.as_deref(), Some(c1.as_str()));
    let by_branch = |b: &str| {
        list.iter()
            .find(|w| w.branch.as_deref() == Some(b))
            .unwrap()
    };
    assert!(by_branch("locked").locked);
    assert!(!by_branch("locked").prunable);
    assert!(by_branch("gone").prunable);
}

#[tokio::test]
async fn worktrees_in_empty_repo_have_no_head() {
    let repo = TestRepo::new_in_subdir("dir with space ü");
    let git = runner().await;
    let list = read_worktrees(&git, repo.path()).await.unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].is_main);
    assert_eq!(list[0].path, canonical(repo.path()));
    assert_eq!(list[0].head, None);
    assert_eq!(list[0].branch.as_deref(), Some("main"));
}

// ---------------------------------------------------------------------------------------
// status

#[tokio::test]
async fn status_counts() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "one\n");
    repo.write("move me.txt", "content that moves\n");
    repo.commit_all("base");
    let git = runner().await;
    assert_eq!(
        read_status(&git, repo.path()).await.unwrap(),
        WorktreeStatus {
            changed: 0,
            untracked: 0,
            conflicted: 0,
            conflicted_paths: vec![]
        }
    );

    repo.write("tracked.txt", "two\n");
    repo.write("new 1.txt", "u\n");
    repo.write("sub dir/ü 2.txt", "u\n");
    let status = read_status(&git, repo.path()).await.unwrap();
    assert_eq!(
        status,
        WorktreeStatus {
            changed: 1,
            untracked: 2,
            conflicted: 0,
            conflicted_paths: vec![]
        }
    );

    // A staged rename is one change (porcelain v2 `2` entry with an extra path field).
    repo.git(&["mv", "move me.txt", "moved ü.txt"]);
    let status = read_status(&git, repo.path()).await.unwrap();
    assert_eq!((status.changed, status.untracked), (2, 2));
}

#[tokio::test]
async fn status_counts_conflicts() {
    let repo = TestRepo::new();
    repo.write("c.txt", "base\n");
    repo.commit_all("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("c.txt", "theirs\n");
    repo.commit_all("theirs");
    repo.checkout("main");
    repo.write("c.txt", "ours\n");
    repo.commit_all("ours");
    assert!(!repo.git_succeeds(&["merge", "--no-edit", "feature"]));

    let git = runner().await;
    let status = read_status(&git, repo.path()).await.unwrap();
    assert_eq!(
        status,
        WorktreeStatus {
            changed: 1,
            untracked: 0,
            conflicted: 1,
            conflicted_paths: vec!["c.txt".to_owned()]
        }
    );
}

#[tokio::test]
async fn status_ignores_submodules() {
    let sub = TestRepo::new();
    sub.commit("sub one");
    let repo = TestRepo::new();
    repo.commit("one");
    let sub_path = sub.path().to_str().unwrap();
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        sub_path,
        "sub",
    ]);
    repo.commit_all("add submodule");
    // New commit, modified and untracked content inside the submodule checkout.
    let checkout = repo.path().join("sub");
    fs::write(checkout.join("untracked.txt"), "x").unwrap();
    fs::write(checkout.join("file-0.txt"), "changed\n").unwrap();
    assert!(
        !repo.git(&["status", "--porcelain"]).is_empty(),
        "plain git status reports the submodule"
    );

    let git = runner().await;
    assert_eq!(
        read_status(&git, repo.path()).await.unwrap(),
        WorktreeStatus::default()
    );
}

#[tokio::test]
async fn status_of_linked_worktree_is_its_own() {
    let repo = TestRepo::new();
    repo.commit("one");
    let wt = repo.add_worktree("wt-b");
    fs::write(wt.join("only-here.txt"), "x").unwrap();
    let git = runner().await;
    assert_eq!(read_status(&git, &wt).await.unwrap().untracked, 1);
    assert_eq!(read_status(&git, repo.path()).await.unwrap().untracked, 0);
}

// ---------------------------------------------------------------------------------------
// commit details

fn file<'a>(files: &'a [FileChange], path: &str) -> &'a FileChange {
    files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path} not in {files:?}"))
}

#[tokio::test]
async fn details_lists_files() {
    let repo = TestRepo::new();
    repo.write("b.txt", "one\ntwo\n");
    let numbers: String = (1..=50).map(|i| format!("line {i}\n")).collect();
    repo.write("c.txt", &numbers);
    let base = repo.commit_all("base");

    repo.write("a.txt", "x\ny\n");
    repo.write("b.txt", "one\nTWO\n");
    repo.git(&["mv", "c.txt", "d.txt"]);
    repo.write("image.bin", [0u8, 159, 146, 150, 0, 1, 2, 255]);
    repo.write("dir with space/ünï ✨.txt", "hi\n");
    let hash = repo.commit_all("add, modify, rename\n\nbody text here");

    let git = runner().await;
    let d = read_details(&git, repo.path(), &hash).await.unwrap();
    assert_eq!(d.hash, hash);
    assert_eq!(d.parents, [base]);
    assert_eq!(d.author, "Test");
    assert_eq!(d.author_email, "test@example.com");
    assert_eq!(d.committer, "Test");
    assert_eq!(d.committer_email, "test@example.com");
    assert_eq!(d.author_time, 1_700_000_060);
    assert_eq!(d.commit_time, 1_700_000_060);
    assert!(
        d.message.starts_with("add, modify, rename"),
        "{:?}",
        d.message
    );
    assert!(d.message.contains("body text here"), "{:?}", d.message);
    assert_eq!(d.files.len(), 5, "{:?}", d.files);

    let a = file(&d.files, "a.txt");
    assert_eq!((a.status, a.old_path.as_deref()), (FileStatus::A, None));
    assert_eq!((a.additions, a.deletions), (Some(2), Some(0)));
    let b = file(&d.files, "b.txt");
    assert_eq!(b.status, FileStatus::M);
    assert_eq!((b.additions, b.deletions), (Some(1), Some(1)));
    let renamed = file(&d.files, "d.txt");
    assert_eq!(renamed.status, FileStatus::R);
    assert_eq!(renamed.old_path.as_deref(), Some("c.txt"));
    assert_eq!((renamed.additions, renamed.deletions), (Some(0), Some(0)));
    let bin = file(&d.files, "image.bin");
    assert_eq!(bin.status, FileStatus::A);
    assert_eq!((bin.additions, bin.deletions), (None, None));
    let uni = file(&d.files, "dir with space/ünï ✨.txt");
    assert_eq!(uni.status, FileStatus::A);
    assert_eq!(uni.additions, Some(1));
}

#[tokio::test]
async fn details_deleted_file() {
    let repo = TestRepo::new();
    repo.write("gone.txt", "a\nb\nc\n");
    repo.commit_all("base");
    fs::remove_file(repo.path().join("gone.txt")).unwrap();
    let hash = repo.commit_all("delete");
    let git = runner().await;
    let d = read_details(&git, repo.path(), &hash).await.unwrap();
    assert_eq!(
        d.files,
        [FileChange {
            path: "gone.txt".into(),
            old_path: None,
            status: FileStatus::D,
            additions: Some(0),
            deletions: Some(3),
        }]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn details_type_change() {
    let repo = TestRepo::new();
    repo.write("target.txt", "t\n");
    repo.write("link", "a regular file\n");
    repo.commit_all("base");
    fs::remove_file(repo.path().join("link")).unwrap();
    std::os::unix::fs::symlink("target.txt", repo.path().join("link")).unwrap();
    let hash = repo.commit_all("file becomes symlink");
    let git = runner().await;
    let d = read_details(&git, repo.path(), &hash).await.unwrap();
    assert_eq!(d.files.len(), 1, "{:?}", d.files);
    assert_eq!(d.files[0].path, "link");
    assert_eq!(d.files[0].status, FileStatus::T);
}

#[tokio::test]
async fn details_root_commit() {
    let repo = TestRepo::new();
    repo.write("x.txt", "1\n2\n3\n");
    repo.write("y.txt", "1\n");
    let root = repo.commit_all("root");
    let git = runner().await;
    let d = read_details(&git, repo.path(), &root).await.unwrap();
    assert!(d.parents.is_empty());
    assert_eq!(d.message, "root");
    let paths: Vec<&str> = d.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["x.txt", "y.txt"]);
    assert!(d.files.iter().all(|f| f.status == FileStatus::A));
    assert_eq!(file(&d.files, "x.txt").additions, Some(3));
}

/// In a shallow clone, the oldest commit fetched has parents that are not in the repository:
/// it is not a root commit, and the details say so. A branch named "grafted" does not count.
#[tokio::test]
async fn details_shallow_boundary_is_not_a_root_commit() {
    let src = TestRepo::new();
    let root = src.commit("one");
    src.commit("two");
    let tip = src.commit("three");
    let shallow = TestRepo::new();
    let url = format!("file://{}", src.path().display());
    shallow.git(&["remote", "add", "origin", &url]);
    shallow.git(&["fetch", "-q", "--depth", "2", "origin", "main"]);
    shallow.git(&["reset", "-q", "--hard", "origin/main"]);
    shallow.git(&["branch", "grafted"]);
    let boundary = shallow.git(&["rev-parse", "HEAD~1"]);

    let git = runner().await;
    let d = read_details(&git, shallow.path(), &boundary).await.unwrap();
    assert!(d.parents.is_empty());
    assert!(d.shallow_boundary, "the boundary of a shallow clone");
    let d = read_details(&git, shallow.path(), &tip).await.unwrap();
    assert!(!d.shallow_boundary);
    // A real root commit, with a branch named "grafted" on it.
    src.git(&["branch", "grafted", &root]);
    let d = read_details(&git, src.path(), &root).await.unwrap();
    assert!(d.parents.is_empty());
    assert!(!d.shallow_boundary, "a root commit");
}

#[tokio::test]
async fn details_merge_commit() {
    let h = merge_history();
    let git = runner().await;
    let d = read_details(&git, h.repo.path(), &h.m).await.unwrap();
    assert_eq!(d.parents, [h.c3.clone(), h.f.clone()]);
    // Against the first parent (c3), the merge brings in only the feature branch's file.
    let paths: Vec<&str> = d.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["file-2.txt"]);
    assert_eq!(d.files[0].status, FileStatus::A);
}

#[tokio::test]
async fn details_rejects_non_hash_arguments() {
    let repo = TestRepo::new();
    repo.commit("one");
    let git = runner().await;
    let full = repo.git(&["rev-parse", "HEAD"]);
    let short = &full[..12];
    for bad in [
        "",
        "--output=/tmp/x",
        "HEAD",
        "abc",
        "zzzzzzzz",
        "abcd ef01",
        short,
    ] {
        let err = read_details(&git, repo.path(), bad).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest, "{bad:?}");
    }
    assert_eq!(
        read_details(&git, repo.path(), &full).await.unwrap().hash,
        full
    );
    let err = read_details(&git, repo.path(), &"0".repeat(40))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::GitFailed);
}

#[cfg(unix)]
#[tokio::test]
async fn details_never_runs_external_diff_or_textconv() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TestRepo::new();
    let tools = tempfile::tempdir().unwrap();
    let marker = tools.path().join("ran");
    let script = tools.path().join("tool.sh");
    fs::write(
        &script,
        format!("#!/bin/sh\ntouch '{}'\ncat \"$1\"\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let script = script.to_str().unwrap();
    repo.git(&["config", "diff.external", script]);
    repo.git(&["config", "diff.conv.textconv", script]);
    repo.write(".gitattributes", "*.txt diff=conv\n");
    repo.write("a.txt", "one\n");
    repo.commit_all("base");
    repo.write("a.txt", "one\ntwo\nthree\n");
    let hash = repo.commit_all("grow a.txt");

    let log = CommandLog::new();
    let git = GitRunner::detect(log.clone()).await.unwrap();
    let d = read_details(&git, repo.path(), &hash).await.unwrap();
    assert!(!marker.exists(), "an external diff or textconv program ran");
    assert_eq!(
        d.files,
        [FileChange {
            path: "a.txt".into(),
            old_path: None,
            status: FileStatus::M,
            additions: Some(2),
            deletions: Some(0),
        }]
    );
    // Both commands opt out explicitly instead of relying on plumbing defaults.
    let records = log.since(0);
    assert_eq!(records.len(), 2);
    for rec in &records {
        assert!(
            rec.args.iter().any(|a| a == "--no-ext-diff")
                && rec.args.iter().any(|a| a == "--no-textconv"),
            "{:?}",
            rec.args
        );
    }
}

// ---- In-progress states ----

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

async fn in_progress(repo: &TestRepo) -> Option<InProgress> {
    read_in_progress(&runner().await, repo.path())
        .await
        .expect("read_in_progress")
}

fn stopped(
    kind: InProgressKind,
    conflicted: &[&str],
    step: Option<(u32, u32)>,
    subject: &str,
) -> Option<InProgress> {
    Some(InProgress {
        kind,
        // Nothing resolved yet: each still has git's conflict markers.
        files: conflicted
            .iter()
            .map(|p| ConflictFile {
                path: (*p).to_owned(),
                state: ConflictState::Markers,
            })
            .collect(),
        file_count: conflicted.len() as u32,
        step: step.map(|(s, _)| s),
        total: step.map(|(_, t)| t),
        subject: Some(subject.to_owned()),
        incoming: None,
        rebasing: None,
        commit: None,
        empty: false,
    })
}

/// `state` with what is coming in (`incoming`) and, for a rebase, the branch being rebased.
fn coming(state: Option<InProgress>, incoming: &str, rebasing: Option<&str>) -> Option<InProgress> {
    state.map(|s| InProgress {
        incoming: Some(incoming.to_owned()),
        rebasing: rebasing.map(str::to_owned),
        ..s
    })
}

/// `state`, stopped on commit `commit` (R40).
fn on(state: Option<InProgress>, commit: &str) -> Option<InProgress> {
    state.map(|s| InProgress {
        commit: Some(commit.to_owned()),
        ..s
    })
}

#[tokio::test]
async fn no_state_is_none() {
    let repo = TestRepo::new();
    assert_eq!(in_progress(&repo).await, None, "empty repository");
    clashing(&repo);
    assert_eq!(in_progress(&repo).await, None);
    // Conflicts that leave nothing to continue are not a state git is stopped in.
    assert!(!repo.git_succeeds(&["cherry-pick", "--no-commit", "feature"]));
    assert_eq!(in_progress(&repo).await, None);
}

#[tokio::test]
async fn merge_in_progress_is_read() {
    let repo = TestRepo::new();
    clashing(&repo);
    assert!(!repo.git_succeeds(&["merge", "feature"]));
    assert_eq!(
        in_progress(&repo).await,
        coming(
            stopped(
                InProgressKind::Merge,
                &["clash.txt"],
                None,
                "Merge branch 'feature'"
            ),
            "feature",
            None
        )
    );

    // Stopped before committing, without conflicts: in progress, nothing conflicted.
    repo.git(&["merge", "--abort"]);
    repo.branch("side");
    repo.checkout("side");
    repo.commit("on side");
    repo.checkout("main");
    repo.git(&["merge", "--no-commit", "--no-ff", "side"]);
    assert_eq!(
        in_progress(&repo).await,
        coming(
            stopped(InProgressKind::Merge, &[], None, "Merge branch 'side'"),
            "side",
            None
        )
    );
}

#[tokio::test]
async fn rebase_in_progress_is_read() {
    let repo = TestRepo::new();
    clashing(&repo);
    repo.checkout("feature");
    let one = repo.git(&["rev-parse", "feature~2"]);
    assert!(!repo.git_succeeds(&["rebase", "main"]));
    assert_eq!(
        in_progress(&repo).await,
        on(
            coming(
                stopped(
                    InProgressKind::Rebase,
                    &["clash.txt"],
                    Some((1, 3)),
                    "feat one"
                ),
                "main",
                Some("feature")
            ),
            &one
        )
    );
}

#[tokio::test]
async fn rebase_reports_step_and_total() {
    let repo = TestRepo::new();
    clashing(&repo);
    repo.checkout("feature");
    let (one, two) = (
        repo.git(&["rev-parse", "feature~2"]),
        repo.git(&["rev-parse", "feature~1"]),
    );
    // The merge backend (rebase-merge/msgnum, end).
    assert!(!repo.git_succeeds(&["rebase", "main"]));
    repo.write("clash.txt", "resolved\n");
    repo.git(&["add", "clash.txt"]);
    assert!(!repo.git_succeeds(&["rebase", "--continue"]));
    assert_eq!(
        in_progress(&repo).await,
        on(
            coming(
                stopped(
                    InProgressKind::Rebase,
                    &["clash.txt"],
                    Some((2, 3)),
                    "feat two"
                ),
                "main",
                Some("feature")
            ),
            &two
        )
    );
    repo.git(&["rebase", "--abort"]);

    // The apply backend (rebase-apply/next, last), which leaves no MERGE_MSG: the subject
    // is the stopped commit's.
    assert!(!repo.git_succeeds(&["rebase", "--apply", "main"]));
    assert_eq!(
        in_progress(&repo).await,
        on(
            coming(
                stopped(
                    InProgressKind::Rebase,
                    &["clash.txt"],
                    Some((1, 3)),
                    "feat one"
                ),
                "main",
                Some("feature")
            ),
            &one
        )
    );
    repo.write("clash.txt", "resolved\n");
    repo.git(&["add", "clash.txt"]);
    assert!(!repo.git_succeeds(&["rebase", "--continue"]));
    assert_eq!(
        in_progress(&repo).await.and_then(|p| p.step.zip(p.total)),
        Some((2, 3))
    );
}

#[tokio::test]
async fn cherry_pick_in_progress_is_read() {
    let repo = TestRepo::new();
    clashing(&repo);
    assert!(!repo.git_succeeds(&["cherry-pick", "feature~2"]));
    let picked = repo.git(&["rev-parse", "feature~2"]);
    assert_eq!(
        in_progress(&repo).await,
        on(
            coming(
                stopped(InProgressKind::CherryPick, &["clash.txt"], None, "feat one"),
                &picked[..8],
                None
            ),
            &picked
        )
    );
}

#[tokio::test]
async fn revert_in_progress_is_read() {
    let repo = TestRepo::new();
    clashing(&repo);
    repo.checkout("feature");
    assert!(!repo.git_succeeds(&["revert", "--no-edit", "feature~2"]));
    let reverted = repo.git(&["rev-parse", "feature~2"]);
    assert_eq!(
        in_progress(&repo).await,
        on(
            coming(
                stopped(
                    InProgressKind::Revert,
                    &["clash.txt"],
                    None,
                    "Revert \"feat one\""
                ),
                &reverted[..8],
                None
            ),
            &reverted
        )
    );
}

/// R32: what a merge brings in is named by the refs at that commit, a local branch before a
/// remote one before a tag, else its 8-character hash. Started outside the app.
#[tokio::test]
async fn a_merge_names_what_comes_in() {
    let repo = TestRepo::new();
    clashing(&repo);
    let feature = repo.git(&["rev-parse", "feature"]);
    let incoming = |repo: &TestRepo| {
        let repo = repo.path().to_path_buf();
        async move {
            read_in_progress(&runner().await, &repo)
                .await
                .expect("read_in_progress")
                .and_then(|p| p.incoming)
        }
    };
    repo.git(&["update-ref", "refs/remotes/origin/feature", &feature]);
    repo.git(&["tag", "v1", &feature]);
    repo.git(&["tag", "-a", "-m", "annotated", "v2", &feature]);
    assert!(!repo.git_succeeds(&["merge", &feature]));
    assert_eq!(incoming(&repo).await.as_deref(), Some("feature"));
    repo.git(&["merge", "--abort"]);

    repo.git(&["branch", "-D", "feature"]);
    assert!(!repo.git_succeeds(&["merge", &feature]));
    assert_eq!(incoming(&repo).await.as_deref(), Some("origin/feature"));
    repo.git(&["merge", "--abort"]);

    repo.git(&["update-ref", "-d", "refs/remotes/origin/feature"]);
    assert!(!repo.git_succeeds(&["merge", &feature]));
    assert_eq!(incoming(&repo).await.as_deref(), Some("v1"));
    repo.git(&["merge", "--abort"]);

    repo.git(&["tag", "-d", "v1"]);
    assert!(!repo.git_succeeds(&["merge", &feature]));
    assert_eq!(
        incoming(&repo).await.as_deref(),
        Some("v2"),
        "an annotated tag"
    );
    repo.git(&["merge", "--abort"]);

    repo.git(&["tag", "-d", "v2"]);
    assert!(!repo.git_succeeds(&["merge", &feature]));
    assert_eq!(incoming(&repo).await.as_deref(), Some(&feature[..8]));
}

#[tokio::test]
async fn conflicted_paths_with_spaces_and_unicode() {
    let repo = TestRepo::new();
    let names = [
        "a b.txt",
        "\u{fc}/\u{f1} \u{e9}.txt",
        "[ab].txt",
        "gone.txt",
    ];
    for name in names {
        repo.write(name, "base\n");
    }
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
    assert!(!repo.git_succeeds(&["merge", "feature"]));
    let state = in_progress(&repo).await.expect("a merge");
    // Sorted, relative to the worktree, unquoted; the file deleted on one side included.
    assert_eq!(
        state.paths(),
        [
            "[ab].txt",
            "a b.txt",
            "gone.txt",
            "\u{fc}/\u{f1} \u{e9}.txt"
        ]
    );
}

/// The state of each file of the merge in `repo` (path, state), in path order.
async fn states(repo: &TestRepo) -> Vec<(String, ConflictState)> {
    in_progress(repo)
        .await
        .expect("a merge")
        .files
        .into_iter()
        .map(|f| (f.path, f.state))
        .collect()
}

fn st(path: &str, state: ConflictState) -> (String, ConflictState) {
    (path.to_owned(), state)
}

/// `main` and `feature` both change each of `names` after `base` (each from `base\n` to
/// "<side>\n"); the merge of `feature` stopped.
fn both_changed(repo: &TestRepo, names: &[&str]) {
    for name in names {
        repo.write(name, "base\n");
    }
    repo.commit_all("base");
    repo.branch("feature");
    repo.checkout("feature");
    for name in names {
        repo.write(name, "feature\n");
    }
    repo.commit_all("feature side");
    repo.checkout("main");
    for name in names {
        repo.write(name, "main\n");
    }
    repo.commit_all("main side");
    assert!(!repo.git_succeeds(&["merge", "feature"]));
}

/// A conflicted text file whose conflict markers are gone is resolved (still unmerged until
/// it is staged); one that still has a `<<<<<<<` or `>>>>>>>` line is not. A Markdown
/// underline of `=======` is no marker. Names git would quote (a double quote, a tab) and
/// one that looks like a report line are read as they are.
#[tokio::test]
async fn resolved_files_are_told_from_ones_with_markers() {
    use ConflictState::{Markers, Resolved};
    let repo = TestRepo::new();
    let names = ["clash.txt", "a: 1: b.txt", "say \"hi\"\tthere.md"];
    both_changed(&repo, &names);
    let all_marked = vec![
        st("a: 1: b.txt", Markers),
        st("clash.txt", Markers),
        st("say \"hi\"\tthere.md", Markers),
    ];
    assert_eq!(states(&repo).await, all_marked);

    repo.write("clash.txt", "resolved\n");
    repo.write("say \"hi\"\tthere.md", "Title\n=======\n\nText.\n");
    assert_eq!(
        states(&repo).await,
        [
            st("a: 1: b.txt", Markers),
            st("clash.txt", Resolved),
            st("say \"hi\"\tthere.md", Resolved),
        ]
    );

    // Only the closing marker left: still not resolved. Eight `<` are no marker.
    repo.write("a: 1: b.txt", "<<<<<<<< not one\nkept\n>>>>>>> feature\n");
    assert_eq!(states(&repo).await[0], st("a: 1: b.txt", Markers));
    repo.write("a: 1: b.txt", "<<<<<<<< not one\n");
    let state = in_progress(&repo).await.expect("a merge");
    assert_eq!(state.file_count, 3, "unmerged until staged");
    assert!(state.files.iter().all(|f| f.state == Resolved), "{state:?}");
}

/// What `git diff --check` cannot see: a binary conflict (git keeps one side, no markers)
/// and a file deleted on one side are undecided; a text file marked `-diff` (a lock file)
/// with markers in it has markers; `conflict-marker-size` is honoured.
#[tokio::test]
async fn binary_deleted_and_undiffed_conflicts_are_not_taken_for_resolved() {
    use ConflictState::{Markers, Resolved, Undecided};
    let repo = TestRepo::new();
    repo.write(
        ".gitattributes",
        "*.lock -diff\nwide.txt conflict-marker-size=10\n",
    );
    repo.write("image.bin", "base\0\x01");
    repo.write("gone.txt", "base\n");
    for name in ["deps.lock", "wide.txt"] {
        repo.write(name, "base\n");
    }
    repo.commit_all("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("image.bin", "feature\0\x02");
    repo.write("gone.txt", "feature\n");
    for name in ["deps.lock", "wide.txt"] {
        repo.write(name, "feature\n");
    }
    repo.commit_all("feature side");
    repo.checkout("main");
    repo.write("image.bin", "main\0\x03");
    repo.git(&["rm", "-q", "--", "gone.txt"]);
    for name in ["deps.lock", "wide.txt"] {
        repo.write(name, "main\n");
    }
    repo.commit_all("main side");
    assert!(!repo.git_succeeds(&["merge", "feature"]));
    assert_eq!(
        states(&repo).await,
        [
            st("deps.lock", Markers),
            st("gone.txt", Undecided),
            st("image.bin", Undecided),
            st("wide.txt", Markers),
        ]
    );
    // A run of 7 is no marker where the size is 10.
    repo.write("wide.txt", "<<<<<<< x\n>>>>>>> y\n");
    repo.write("deps.lock", "resolved\n");
    assert_eq!(
        states(&repo).await,
        [
            st("deps.lock", Resolved),
            st("gone.txt", Undecided),
            st("image.bin", Undecided),
            st("wide.txt", Resolved),
        ]
    );
}

/// Conflicts nothing in progress explains (a stash pop's) are named in the status, at most
/// 200 of them.
#[tokio::test]
async fn status_names_conflicted_files() {
    let repo = TestRepo::new();
    for i in 0..205 {
        repo.write(&format!("f{i:03}.txt"), "base\n");
    }
    repo.commit_all("base");
    for i in 0..205 {
        repo.write(&format!("f{i:03}.txt"), "stashed\n");
    }
    repo.git(&["stash", "-q"]);
    for i in 0..205 {
        repo.write(&format!("f{i:03}.txt"), "committed\n");
    }
    repo.commit_all("moved on");
    assert!(!repo.git_succeeds(&["stash", "pop"]));
    let status = read_status(&runner().await, repo.path()).await.unwrap();
    assert_eq!(status.conflicted, 205);
    assert_eq!(status.conflicted_paths.len(), 200);
    assert_eq!(status.conflicted_paths[0], "f000.txt");
    assert_eq!(status.conflicted_paths[199], "f199.txt");
}

/// A merge started in a terminal shows in the snapshot after the next refresh, with the
/// working tree's status.
#[tokio::test]
async fn state_started_outside_the_app_is_detected() {
    use gg_core::repo::RepoManager;
    use gg_core::testutil::RecordingSink;
    use std::sync::Arc;
    use std::time::Duration;

    let repo = TestRepo::new();
    clashing(&repo);
    let sink = Arc::new(RecordingSink::default());
    let m = RepoManager::new(runner().await, sink.clone());
    let id = m.open(repo.path()).await.unwrap().repo_id;
    let wait = Duration::from_secs(20);
    tokio::time::timeout(wait, m.wait_status(id)).await.unwrap();
    assert_eq!(m.snapshot(id).await.unwrap().in_progress, None);

    assert!(!repo.git_succeeds(&["merge", "feature"]));
    m.refresh(id).await.unwrap();
    let changed = sink
        .wait_for(wait, |name, v| {
            name == "repo-changed" && v["repoId"] == id && !v["inProgress"].is_null()
        })
        .await
        .expect("a repo-changed with the merge in progress");
    assert_eq!(changed.1["inProgress"]["kind"], "merge");
    assert_eq!(
        changed.1["inProgress"]["files"][0],
        serde_json::json!({"path": "clash.txt", "state": "markers"})
    );
    assert_eq!(changed.1["inProgress"]["fileCount"], 1);
    assert_eq!(changed.1["status"]["conflicted"], 1);
    let snap = m.snapshot(id).await.unwrap();
    assert_eq!(
        snap.in_progress,
        coming(
            stopped(
                InProgressKind::Merge,
                &["clash.txt"],
                None,
                "Merge branch 'feature'"
            ),
            "feature",
            None
        )
    );

    repo.git(&["merge", "--abort"]);
    m.refresh(id).await.unwrap();
    sink.wait_for(wait, |name, v| {
        name == "repo-changed"
            && v["repoId"] == id
            && v["inProgress"].is_null()
            && v["status"]["conflicted"] == 0
    })
    .await
    .expect("a repo-changed without it");
}

// ---------------------------------------------------------------------------------------
// stashes

/// Subjects keep whatever git stores: unicode, a unit separator (the field separator) and
/// what git made of bytes that are not UTF-8 (the parser's lossy decoding of raw bytes is
/// unit-tested in `git::stash`). Parents, selectors, author and time come through, and a
/// stash with untracked files has its third parent.
#[cfg(unix)]
#[tokio::test]
async fn stash_list_parses_subjects_with_odd_bytes() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    use gg_core::git::stash::read_stashes;

    let repo = TestRepo::new();
    let git = runner().await;
    assert!(
        read_stashes(&git, repo.path()).await.unwrap().is_empty(),
        "no commit, no stash"
    );
    let base = repo.commit("one");
    assert!(read_stashes(&git, repo.path()).await.unwrap().is_empty());

    repo.write("file-0.txt", "changed\n");
    let odd: &[u8] = b"m\xc3\xbcsli \x1f split \xff\xfe end";
    let mut message = b"--message=".to_vec();
    message.extend_from_slice(odd);
    let plain = repo.stash_os(&[OsStr::from_bytes(&message)]);
    repo.write("file-0.txt", "changed again\n");
    repo.write("new.txt", "untracked\n");
    let with_untracked = repo.stash(&["--include-untracked"]);

    let stashes = read_stashes(&git, repo.path()).await.unwrap();
    assert_eq!(stashes.len(), 2, "{stashes:?}");
    let (newest, older) = (&stashes[0], &stashes[1]);

    assert_eq!(newest.hash, with_untracked);
    assert_eq!(newest.selector, "stash@{0}");
    assert_eq!(newest.base, base);
    let parents = repo.git(&["rev-list", "--parents", "-n", "1", &with_untracked]);
    let parents: Vec<&str> = parents.split(' ').skip(1).collect();
    assert_eq!(parents.len(), 3, "{parents:?}");
    assert_eq!(newest.index_parent.as_deref(), Some(parents[1]));
    assert_eq!(newest.untracked_parent.as_deref(), Some(parents[2]));
    assert_eq!(newest.author, "Test");
    assert_eq!(newest.email, "test@example.com");
    assert_eq!(newest.time, 1_700_000_000 + 2 * 60);
    assert!(
        newest.subject.starts_with("WIP on main: "),
        "{}",
        newest.subject
    );

    assert_eq!(older.hash, plain);
    assert_eq!(older.selector, "stash@{1}");
    assert_eq!(older.base, base);
    assert!(older.index_parent.is_some());
    assert_eq!(older.untracked_parent, None);
    assert_eq!(older.time, 1_700_000_000 + 60);
    // git stores a message that is not UTF-8 as if it were Latin-1 (`\xff\xfe` is "ÿþ"):
    // the subject is what git stores, the separator inside it included.
    assert_eq!(older.subject, "On main: müsli \u{1f} split ÿþ end");
    let stored = repo.git(&["log", "-1", "--format=%s", &plain]);
    assert_eq!(older.subject, stored);
}

/// `main` and `feature` change `clash.txt`; `feature` then adds `other.txt` in a second
/// commit. A cherry-pick of both stops on the first; the user resolves it and commits by
/// hand, which ends the pick of that commit (no `CHERRY_PICK_HEAD`) but not the sequence.
fn sequence_committed_by_hand(repo: &TestRepo, verb: &str) {
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
    repo.commit_all("main side");
    if verb == "cherry-pick" {
        assert!(!repo.git_succeeds(&["cherry-pick", "feature~1", "feature"]));
    } else {
        // Reverting feature's commits on feature: the older one clashes with the newer.
        repo.checkout("feature");
        repo.write("clash.txt", "feature again\n");
        repo.commit_all("feat again");
        assert!(!repo.git_succeeds(&["revert", "--no-edit", "HEAD~2", "HEAD~1"]));
    }
    repo.write("clash.txt", "resolved\n");
    repo.git(&["add", "clash.txt"]);
    repo.git(&["commit", "-q", "--no-edit"]);
    assert!(!in_git_dir_has(repo, "CHERRY_PICK_HEAD") && !in_git_dir_has(repo, "REVERT_HEAD"));
    assert!(in_git_dir_has(repo, "sequencer"));
}

fn in_git_dir_has(repo: &TestRepo, name: &str) -> bool {
    repo.path().join(".git").join(name).exists()
}

/// A cherry-pick or revert sequence whose current commit was committed by hand is still
/// in progress (git status says so): its kind comes from the sequencer's next command. Its
/// subject is unknown: the sequencer's current line names the commit already made by hand
/// (Task 5 review, item 9).
#[tokio::test]
async fn sequence_committed_by_hand_is_still_in_progress() {
    for (verb, kind) in [
        ("cherry-pick", InProgressKind::CherryPick),
        ("revert", InProgressKind::Revert),
    ] {
        let repo = TestRepo::new();
        sequence_committed_by_hand(&repo, verb);
        let state = in_progress(&repo).await.expect(verb);
        assert_eq!(state.kind, kind);
        assert!(state.files.is_empty());
        assert_eq!(state.subject, None, "{verb}: {state:?}");
    }
}

/// `git am` stopped on a patch is not something the app continues or aborts: not reported.
#[tokio::test]
async fn am_in_progress_is_not_reported() {
    let repo = TestRepo::new();
    clashing(&repo);
    let patches = repo.dir.path().join("patches");
    let patches = patches.to_str().unwrap();
    repo.git(&["format-patch", "-q", "-1", "feature~2", "-o", patches]);
    let patch = fs::read_dir(patches)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(!repo.git_succeeds(&["am", patch.to_str().unwrap()]));
    assert!(in_git_dir_has(&repo, "rebase-apply"));
    assert_eq!(in_progress(&repo).await, None);
}
