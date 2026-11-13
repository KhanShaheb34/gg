//! Integration tests for `GitRunner` and `CommandLog` against real temp repositories.

use std::fs;
use std::time::Duration;

use gg_core::error::ErrorCode;
use gg_core::git::runner::{CmdKind, CommandLog, GitRunner};
use gg_core::testutil::TestRepo;

async fn runner() -> GitRunner {
    GitRunner::detect(CommandLog::new())
        .await
        .expect("git is installed")
}

#[tokio::test]
async fn run_returns_stdout() {
    let repo = TestRepo::new();
    let git = runner().await;
    let out = git
        .run(
            repo.path(),
            &["rev-parse", "--is-inside-work-tree"],
            CmdKind::Read,
        )
        .await
        .unwrap();
    assert_eq!(out.text().trim(), "true");
    assert_eq!(out.code, 0);
}

#[tokio::test]
async fn nonzero_exit_is_git_failed() {
    let repo = TestRepo::new();
    repo.commit("initial");
    let git = runner().await;
    let err = git
        .run(repo.path(), &["checkout", "nope"], CmdKind::Write)
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::GitFailed);
    assert!(!err.message.is_empty());
    assert!(
        !err.message.contains('\n'),
        "message is one line: {:?}",
        err.message
    );
    let detail = err.detail.expect("full stderr in detail");
    assert!(detail.contains(&err.message));
}

#[tokio::test]
async fn run_unchecked_returns_code() {
    let repo = TestRepo::new();
    repo.commit("initial");
    let git = runner().await;
    let out = git
        .run_unchecked(repo.path(), &["checkout", "nope"], CmdKind::Write)
        .await
        .unwrap();
    assert_ne!(out.code, 0);
    assert!(!out.stderr.is_empty());
}

#[tokio::test]
async fn env_is_applied() {
    let repo = TestRepo::new();
    let git = runner().await;
    let editor = git
        .run(repo.path(), &["var", "GIT_EDITOR"], CmdKind::Read)
        .await
        .unwrap();
    assert_eq!(editor.text().trim(), "true");
    let quote = git
        .run(
            repo.path(),
            &["config", "--get", "core.quotePath"],
            CmdKind::Read,
        )
        .await
        .unwrap();
    assert_eq!(quote.text().trim(), "false");
    let color = git
        .run(repo.path(), &["config", "--get", "color.ui"], CmdKind::Read)
        .await
        .unwrap();
    assert_eq!(color.text().trim(), "false");
    let signature = git
        .run(
            repo.path(),
            &["config", "--get", "log.showSignature"],
            CmdKind::Read,
        )
        .await
        .unwrap();
    assert_eq!(signature.text().trim(), "false");
}

#[tokio::test]
async fn never_waits_for_input() {
    let repo = TestRepo::new();
    repo.commit("initial");
    fs::write(repo.path().join("staged.txt"), "staged\n").unwrap();
    repo.git(&["add", "staged.txt"]);
    let git = runner().await;
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        git.run_unchecked(repo.path(), &["commit"], CmdKind::Write),
    )
    .await
    .expect("git commit must not block on an editor or stdin");
    // With GIT_EDITOR=true the message stays empty, so git aborts the commit.
    let out = result.unwrap();
    assert_ne!(out.code, 0);
}

#[tokio::test]
async fn log_records_commands() {
    let repo = TestRepo::new();
    let log = CommandLog::new();
    let git = GitRunner::detect(log.clone()).await.unwrap();
    git.run(repo.path(), &["rev-parse", "--git-dir"], CmdKind::Read)
        .await
        .unwrap();
    git.run(repo.path(), &["branch", "feature"], CmdKind::Write)
        .await
        .unwrap_err();

    let records = log.since(0);
    assert_eq!(records.len(), 2);
    assert!(std::ptr::eq(git.log().as_ref(), log.as_ref()));

    let read = &records[0];
    assert_eq!(read.args, ["rev-parse", "--git-dir"]);
    assert_eq!(read.kind, CmdKind::Read);
    assert_eq!(read.exit_code, 0);
    assert!(read.stdout.is_empty(), "Read stdout is not kept");
    assert!(read.started_at_ms > 0);
    assert!(!read.cwd.is_empty());

    let write = &records[1];
    // `branch` with no commits fails: stderr is kept, and so is the (empty) stdout.
    assert_eq!(write.args, ["branch", "feature"]);
    assert_eq!(write.kind, CmdKind::Write);
    assert_ne!(write.exit_code, 0);
    assert!(!write.stderr.is_empty());
    assert!(write.id > read.id);

    assert_eq!(log.since(read.id).len(), 1);
    assert!(log.since(write.id).is_empty());
}

#[tokio::test]
async fn log_keeps_write_stdout() {
    let repo = TestRepo::new();
    let log = CommandLog::new();
    let git = GitRunner::detect(log.clone()).await.unwrap();
    git.run(
        repo.path(),
        &["rev-parse", "--is-bare-repository"],
        CmdKind::Write,
    )
    .await
    .unwrap();
    let records = log.since(0);
    assert_eq!(records[0].stdout.trim(), "false");
}

#[tokio::test]
async fn log_is_capped() {
    let repo = TestRepo::new();
    let log = CommandLog::new();
    let git = GitRunner::detect(log.clone()).await.unwrap();
    // 600 runs, 20 at a time to keep the suite fast without exhausting file descriptors.
    for _ in 0..30 {
        let mut batch = tokio::task::JoinSet::new();
        for _ in 0..20 {
            let (git, path) = (git.clone(), repo.path().to_path_buf());
            batch.spawn(async move {
                git.run(&path, &["rev-parse", "--git-dir"], CmdKind::Read)
                    .await
                    .unwrap();
            });
        }
        batch.join_all().await;
    }
    let records = log.since(0);
    assert_eq!(records.len(), 500);
    assert_eq!(records[0].id, 101);
    assert_eq!(records[499].id, 600);
}

#[tokio::test]
async fn path_with_spaces_and_unicode() {
    let repo = TestRepo::new_in_subdir("my répo ✨");
    assert!(repo.path().ends_with("my répo ✨"));
    repo.commit("initial");
    fs::write(repo.path().join("naïve file ✨.txt"), "x\n").unwrap();
    let git = runner().await;
    let top = git
        .run(
            repo.path(),
            &["rev-parse", "--show-toplevel"],
            CmdKind::Read,
        )
        .await
        .unwrap();
    assert_eq!(
        fs::canonicalize(top.text().trim()).unwrap(),
        fs::canonicalize(repo.path()).unwrap()
    );
    // core.quotePath=false keeps non-ASCII file names unescaped.
    let status = git
        .run(repo.path(), &["status", "--porcelain", "-z"], CmdKind::Read)
        .await
        .unwrap();
    assert_eq!(status.text(), "?? naïve file ✨.txt\0");
}

#[tokio::test]
async fn missing_cwd_is_path_missing() {
    let repo = TestRepo::new();
    let git = runner().await;
    let err = git
        .run(
            &repo.path().join("does-not-exist"),
            &["status"],
            CmdKind::Read,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::PathMissing);
}

#[tokio::test]
async fn spawn_stream_pipes_stdout_and_logs_on_done() {
    use tokio::io::AsyncReadExt;

    let repo = TestRepo::new();
    let hash = repo.commit("initial");
    let log = CommandLog::new();
    let git = GitRunner::detect(log.clone()).await.unwrap();
    let args = ["log", "--format=%H"];
    let started = std::time::Instant::now();
    let mut child = git.spawn_stream(repo.path(), &args).unwrap();
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .await
        .unwrap();
    let status = child.wait().await.unwrap();
    assert_eq!(stdout.trim(), hash);
    assert!(log.since(0).is_empty(), "not logged until reported done");

    git.log_stream_done(repo.path(), &args, started, status.code().unwrap(), "");
    let records = log.since(0);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, CmdKind::Read);
    assert_eq!(records[0].args, args);
    assert_eq!(records[0].exit_code, 0);
}

#[test]
fn test_repo_builds_history() {
    let repo = TestRepo::new();
    let a = repo.commit("a");
    assert_eq!(a.len(), 40);
    repo.branch("feature");
    repo.checkout("feature");
    let b = repo.commit_as("b", "Alice", "alice@example.com");
    repo.checkout("main");
    let c = repo.commit("c");
    let m = repo.merge("feature");
    assert_eq!(repo.git(&["rev-parse", "HEAD^1"]), c);
    assert_eq!(repo.git(&["rev-parse", "HEAD^2"]), b);
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), m);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%an <%ae>", &b]),
        "Alice <alice@example.com>"
    );
    assert_eq!(
        repo.git(&["log", "-1", "--format=%an <%ae>", &a]),
        "Test <test@example.com>"
    );
    // Fixed, increasing dates: 1700000000 + 60 s per commit.
    assert_eq!(
        repo.git(&["log", "-1", "--format=%at %ct", &a]),
        "1700000000 1700000000"
    );
    assert_eq!(repo.git(&["log", "-1", "--format=%at", &b]), "1700000060");
    assert_eq!(repo.git(&["log", "-1", "--format=%ct", &m]), "1700000180");
}

#[test]
fn test_repo_is_deterministic() {
    let one = TestRepo::new();
    let two = TestRepo::new();
    assert_eq!(one.commit("same"), two.commit("same"));
}

#[test]
fn test_repo_worktree_and_remote() {
    let repo = TestRepo::new();
    repo.commit("a");
    repo.branch("wt");
    let wt = repo.add_worktree("wt");
    assert!(wt.is_dir());
    let list = repo.git(&["worktree", "list", "--porcelain"]);
    assert!(list.contains("branch refs/heads/wt"), "{list}");
    let created = repo.add_worktree("new/branch");
    assert!(created.is_dir());
    assert_eq!(
        repo.git(&["rev-parse", "new/branch"]),
        repo.git(&["rev-parse", "main"])
    );

    let origin = repo.clone_as_remote();
    assert_eq!(origin.git(&["rev-parse", "--is-bare-repository"]), "true");
    assert_eq!(
        origin.git(&["rev-parse", "main"]),
        repo.git(&["rev-parse", "main"])
    );
    assert_eq!(
        repo.git(&["rev-parse", "refs/remotes/origin/main"]),
        repo.git(&["rev-parse", "main"])
    );
    assert_eq!(
        repo.git(&["rev-parse", "refs/remotes/origin/wt"]),
        repo.git(&["rev-parse", "wt"])
    );
}

#[tokio::test]
async fn extra_config_applies_to_every_command_but_stays_out_of_the_log() {
    let repo = TestRepo::new();
    repo.git(&["config", "core.fsmonitor", "/bin/false"]);
    let log = CommandLog::new();
    let base = GitRunner::detect(log.clone()).await.unwrap();
    let guarded = base.with_extra_config(&[("core.fsmonitor", "false"), ("gg.test", "yes")]);
    let args = ["config", "--get", "core.fsmonitor"];
    let get = |git: &GitRunner, key: &'static str| {
        let git = git.clone();
        let cwd = repo.path().to_path_buf();
        async move {
            git.run(&cwd, &["config", "--get", key], CmdKind::Read)
                .await
                .unwrap()
                .text()
                .trim()
                .to_owned()
        }
    };
    assert_eq!(get(&guarded, "core.fsmonitor").await, "false");
    assert_eq!(get(&guarded, "gg.test").await, "yes");
    // The base runner and other clones are unaffected; the default options are kept.
    assert_eq!(get(&base, "core.fsmonitor").await, "/bin/false");
    assert_eq!(get(&guarded, "core.quotePath").await, "false");
    // Extra options compose.
    let twice = guarded.with_extra_config(&[("gg.other", "1")]);
    assert_eq!(get(&twice, "gg.test").await, "yes");
    assert_eq!(get(&twice, "gg.other").await, "1");

    let records = log.since(0);
    assert!(records.iter().all(|r| !r.args.iter().any(|a| a == "-c")));
    assert_eq!(records[0].args, args);
    assert!(std::ptr::eq(guarded.log().as_ref(), log.as_ref()));
}
