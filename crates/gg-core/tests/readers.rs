//! Integration tests for the git readers (log, refs, worktrees, status, commit details)
//! against real temp repositories.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use gg_core::dto::{FileChange, FileStatus, ViewOptions, WorktreeStatus};
use gg_core::error::ErrorCode;
use gg_core::git::details::read_details;
use gg_core::git::log::{Commit, LogParser, log_args, read_log};
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
    }
}

fn canonical(p: &Path) -> String {
    fs::canonicalize(p).unwrap().to_string_lossy().into_owned()
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
            conflicted: 0
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
            conflicted: 0
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
            conflicted: 1
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
