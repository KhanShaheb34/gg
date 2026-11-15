//! The commit panel's core: the list of changed files (`worktree_files`) and the commit of
//! the ticked ones (`commit`). Review Focus 3, committing the wrong files: a ticked subset
//! beside changes staged elsewhere, renames, deletions, untracked files, odd paths, a file
//! changed between listing and commit, an empty message, a failing hook; exactly the ticked
//! paths are committed or nothing is, and the hook's output is shown. Remotes are local bare
//! repositories; no test contacts a network host.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gg_core::dto::{
    EntryStatus, FileEntry, HeadInfo, OpHint, OpOutcome, OpRequest, OpStatus, WorktreeFiles,
};
use gg_core::error::ErrorCode;
use gg_core::git::runner::CmdKind;
use gg_core::testutil::{TestRepo, empty_git_config};

mod common;
use common::*;

/// A commit of `paths` with `message`, asked for on HEAD and in the worktree as they are.
fn commit(paths: &[&str], message: &str, push: bool) -> OpRequest {
    OpRequest::Commit {
        paths: paths.iter().map(|p| (*p).to_owned()).collect(),
        message: message.to_owned(),
        push,
        expect_head: here(),
        expect_worktree: here_wt(),
    }
}

/// Runs git in `dir` and returns trimmed stdout; panics if git fails.
fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", empty_git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// The paths commit `c` changed compared with its first parent (or with nothing), without
/// rename detection: a rename is both its paths.
fn changed_by(dir: &Path, c: &str) -> BTreeSet<String> {
    let parent = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--verify", "-q", &format!("{c}^1")])
        .env("GIT_CONFIG_GLOBAL", empty_git_config())
        .output()
        .unwrap();
    let out = if parent.status.success() {
        let parent = String::from_utf8_lossy(&parent.stdout).trim().to_owned();
        git_raw(
            dir,
            &[
                "diff-tree",
                "-r",
                "--no-renames",
                "--name-only",
                "-z",
                &parent,
                c,
            ],
        )
    } else {
        git_raw(
            dir,
            &[
                "diff-tree",
                "--root",
                "--no-commit-id",
                "-r",
                "--no-renames",
                "--name-only",
                "-z",
                c,
            ],
        )
    };
    out.split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

/// git's stdout untrimmed (NUL-separated output).
fn git_raw(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", empty_git_config())
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn set(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|p| (*p).to_owned()).collect()
}

/// The user's index as git shows it: every entry, and what is staged.
fn index_state(dir: &Path) -> (String, String) {
    (
        git_raw(dir, &["ls-files", "-s", "-z"]),
        git_raw(dir, &["diff", "--cached", "--name-status", "-z"]),
    )
}

/// The temporary folders a commit made beside the index in `git_dir` (none once it ends).
fn scratch_left(git_dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(git_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("gg-commit-"))
        })
        .collect()
}

#[cfg(unix)]
fn hook(repo: &TestRepo, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = repo.path().join(".git/hooks").join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// `git status --porcelain` lines, sorted.
fn porcelain(dir: &Path) -> Vec<String> {
    let mut lines: Vec<String> = git_raw(dir, &["status", "--porcelain", "--untracked-files=all"])
        .lines()
        .map(str::to_owned)
        .collect();
    lines.sort();
    lines
}

/// The write commands of operation `out`, as argument lists.
fn writes(h: &Harness, out: &OpOutcome) -> Vec<Vec<String>> {
    op_records(&h.log, out)
        .into_iter()
        .filter(|r| r.kind == CmdKind::Write)
        .map(|r| r.args)
        .collect()
}

/// `a.txt`, `b.txt`, `c.txt` committed.
fn three_files() -> TestRepo {
    let repo = TestRepo::new();
    for f in ["a.txt", "b.txt", "c.txt"] {
        repo.write(f, format!("{f}\n"));
    }
    repo.commit_all("three");
    repo
}

async fn run_err(h: &Harness, id: u32, request: OpRequest) -> gg_core::error::CoreError {
    let request = h.at_head(id, request).await;
    tokio::time::timeout(TIMEOUT, h.m.run_op(id, request, &[]))
        .await
        .expect("ends")
        .expect_err("refused")
}

// ---- Review Focus 3: exactly the ticked paths, or nothing ----

/// A ticked subset beside a change staged elsewhere: the commit is built in a fresh index
/// from HEAD, never a copy of the user's, so the staged but unticked file is not in it and
/// stays staged; the ticked files no longer show as changed.
#[tokio::test]
async fn only_the_ticked_files_are_committed_and_staged_unticked_ones_stay_staged() {
    let repo = three_files();
    repo.write("a.txt", "a edited\n");
    repo.write("b.txt", "b staged\n");
    repo.git(&["add", "b.txt"]);
    repo.write("c.txt", "c staged\n");
    repo.git(&["add", "c.txt"]);
    repo.write("c.txt", "c staged, then edited\n");
    repo.write("d.txt", "new\n");
    let before = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h
        .run(
            id,
            commit(&["a.txt", "c.txt", "d.txt"], "three of four", false),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    let new = repo.git(&["rev-parse", "HEAD"]);
    assert_eq!(repo.git(&["rev-parse", "HEAD^"]), before);
    assert_eq!(
        out.message,
        format!("Committed 3 files to main ({}).", &new[..8])
    );
    assert_eq!(
        changed_by(repo.path(), &new),
        set(&["a.txt", "c.txt", "d.txt"])
    );
    // As they are in the working tree, not as staged.
    assert_eq!(repo.git(&["show", "HEAD:c.txt"]), "c staged, then edited");
    assert_eq!(repo.git(&["show", "HEAD:b.txt"]), "b.txt", "b is not in it");
    // b stays staged; the committed ones are clean now.
    assert_eq!(porcelain(repo.path()), ["M  b.txt"]);
    assert!(scratch_left(&repo.path().join(".git")).is_empty());
    // The temporary index: HEAD with the user's index's stat data, then exactly the ticked
    // paths by name; after the commit, the user's index gets exactly what it changed.
    let w = writes(&h, &out);
    assert_eq!(w[0][..2], ["read-tree", "--reset"], "{w:?}");
    assert!(
        w[0][2].starts_with("--index-output=") && w[0][2].ends_with("/index"),
        "{w:?}"
    );
    assert_eq!(w[0][3], before);
    assert_eq!(
        w[1],
        [
            "update-index",
            "--add",
            "--remove",
            "--",
            "a.txt",
            "c.txt",
            "d.txt"
        ]
    );
    assert_eq!(w[2][..2], ["commit", "--cleanup=whitespace"], "{w:?}");
    assert_eq!(w[3][..3], ["update-index", "--add", "--replace"], "{w:?}");
    assert_eq!(
        w[3].iter().filter(|a| *a == "--cacheinfo").count(),
        3,
        "{w:?}"
    );
    assert_eq!(w[4], ["update-index", "-q", "--refresh"], "{w:?}");
    assert_eq!(w.len(), 5, "{w:?}");
}

/// A rename is one change: ticking either side commits both paths.
#[tokio::test]
async fn ticking_either_side_of_a_rename_commits_both() {
    for side in ["new name.txt", "a.txt"] {
        let repo = three_files();
        repo.git(&["mv", "a.txt", "new name.txt"]);
        repo.write("b.txt", "b edited\n");
        let h = harness().await;
        let id = h.open(repo.path()).await.repo_id;
        let out = h.run(id, commit(&[side], "rename", false)).await;
        assert_eq!(out.status, OpStatus::Done, "{side}: {out:#?}");
        assert_eq!(
            out.message[..24],
            *"Committed 1 file to main",
            "{}",
            out.message
        );
        let new = repo.git(&["rev-parse", "HEAD"]);
        assert_eq!(
            changed_by(repo.path(), &new),
            set(&["a.txt", "new name.txt"])
        );
        assert_eq!(porcelain(repo.path()), [" M b.txt"]);
    }
}

/// A deletion is ticked by its path though the file is gone, staged or not; an unticked
/// deletion stays as it is.
#[tokio::test]
async fn deletions_are_committed_by_their_path() {
    let repo = three_files();
    fs::remove_file(repo.path().join("a.txt")).unwrap();
    repo.git(&["rm", "-q", "b.txt"]);
    fs::remove_file(repo.path().join("c.txt")).unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h
        .run(id, commit(&["a.txt", "b.txt"], "two gone", false))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    let new = repo.git(&["rev-parse", "HEAD"]);
    assert_eq!(changed_by(repo.path(), &new), set(&["a.txt", "b.txt"]));
    assert!(!repo.git_succeeds(&["cat-file", "-e", "HEAD:a.txt"]));
    assert!(repo.git_succeeds(&["cat-file", "-e", "HEAD:c.txt"]));
    assert_eq!(porcelain(repo.path()), [" D c.txt"]);
}

/// Paths are literal: spaces, non-ASCII, a leading `-`, glob characters and (on Unix) a
/// newline name exactly those files; `star*.txt` does not take `starry.txt` with it.
#[tokio::test]
async fn odd_paths_are_committed_literally() {
    let repo = three_files();
    let mut ticked = vec![
        "with space.txt",
        "\u{fc}n\u{ef}c\u{f6}d\u{e9}.txt",
        "-leading.txt",
        "star*.txt",
        "dir/[x].txt",
        ":(glob)magic.txt",
    ];
    if cfg!(unix) {
        ticked.push("new\nline.txt");
    }
    for p in &ticked {
        repo.write(p, "x\n");
    }
    repo.write("starry.txt", "not ticked\n");
    repo.write("dir/x.txt", "not ticked\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, commit(&ticked, "odd names", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    let new = repo.git(&["rev-parse", "HEAD"]);
    assert_eq!(changed_by(repo.path(), &new), set(&ticked));
    assert_eq!(porcelain(repo.path()), ["?? dir/x.txt", "?? starry.txt"]);
}

/// The commit takes each ticked file as it is when its turn comes, as `git commit <paths>`
/// does: an edit made after the list was read is in it.
#[tokio::test]
async fn a_file_changed_after_listing_is_committed_as_it_is_in_its_turn() {
    let repo = three_files();
    repo.write("a.txt", "as listed\n");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let listed =
        h.m.worktree_files(snap.repo_id, Path::new(&snap.active_worktree))
            .await
            .unwrap();
    assert_eq!(listed.files.len(), 1);
    repo.write("a.txt", "edited after the list was read\n");
    let out = h
        .run(snap.repo_id, commit(&["a.txt"], "late edit", false))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(
        repo.git(&["show", "HEAD:a.txt"]),
        "edited after the list was read"
    );
}

/// No file ticked, or a blank message, or a path outside the worktree: refused before git
/// runs.
#[tokio::test]
async fn nothing_ticked_or_a_blank_message_is_refused_before_git_runs() {
    let repo = three_files();
    repo.write("a.txt", "edited\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let mark = h.last_log_id();
    let err = run_err(&h, id, commit(&[], "message", false)).await;
    assert_eq!(err.code, ErrorCode::NothingToDo);
    for message in ["", "  \n\t "] {
        let err = run_err(&h, id, commit(&["a.txt"], message, false)).await;
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(err.message, "A commit needs a message.");
    }
    for path in ["../outside", "/etc/hosts", "", "./a.txt"] {
        let err = run_err(&h, id, commit(&[path], "m", false)).await;
        assert_eq!(err.code, ErrorCode::BadRequest, "{path:?}");
    }
    assert_eq!(h.last_log_id(), mark, "git never ran");
}

/// Ticked files with no change now: nothing to commit, no hook runs, no commit is made.
#[cfg(unix)]
#[tokio::test]
async fn ticked_files_without_changes_commit_nothing() {
    let repo = three_files();
    repo.write("b.txt", "b edited\n");
    let marker = repo.dir.path().join("hook-ran");
    hook(
        &repo,
        "pre-commit",
        &format!("touch '{}'", marker.display()),
    );
    let before = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let err = run_err(&h, id, commit(&["a.txt", "gone.txt"], "nothing", false)).await;
    assert_eq!(err.code, ErrorCode::NothingToDo);
    assert_eq!(
        err.message,
        "Nothing to commit: the ticked files have no changes now."
    );
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    assert!(!marker.exists(), "no hook ran");
    assert!(scratch_left(&repo.path().join(".git")).is_empty());
}

/// A failing pre-commit or commit-msg hook: nothing is committed, the hook's output is the
/// detail, the user's index is as it was (a file staged but not ticked stays staged) and
/// the temporary files are gone.
#[cfg(unix)]
#[tokio::test]
async fn a_failing_hook_commits_nothing_and_its_output_is_shown() {
    for name in ["pre-commit", "commit-msg"] {
        let repo = three_files();
        repo.write("a.txt", "edited\n");
        repo.write("b.txt", "staged\n");
        repo.git(&["add", "b.txt"]);
        hook(
            &repo,
            name,
            "echo \"lint: a.txt is not formatted\" >&2\nexit 1",
        );
        let before = repo.git(&["rev-parse", "HEAD"]);
        let index = index_state(repo.path());
        let h = harness().await;
        let id = h.open(repo.path()).await.repo_id;
        let out = h.run(id, commit(&["a.txt"], "refused", false)).await;
        assert_eq!(out.status, OpStatus::Failed, "{name}: {out:#?}");
        assert!(
            out.message
                .starts_with("Nothing was committed: git or one of the repository's hooks"),
            "{}",
            out.message
        );
        assert!(
            out.detail
                .as_deref()
                .unwrap()
                .contains("lint: a.txt is not formatted"),
            "{out:#?}"
        );
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
        assert_eq!(
            index_state(repo.path()),
            index,
            "the user's index is untouched"
        );
        assert_eq!(porcelain(repo.path()), [" M a.txt", "M  b.txt"]);
        assert!(scratch_left(&repo.path().join(".git")).is_empty());
    }
}

/// Hooks run, and see the temporary index (`GIT_INDEX_FILE`), which holds HEAD and the
/// ticked files only, as with `git commit <paths>`; post-commit runs after.
#[cfg(unix)]
#[tokio::test]
async fn hooks_run_and_see_the_temporary_index() {
    let repo = three_files();
    repo.write("a.txt", "edited\n");
    repo.write("b.txt", "staged, not ticked\n");
    repo.git(&["add", "b.txt"]);
    let seen = repo.dir.path().join("seen");
    let post = repo.dir.path().join("post");
    hook(
        &repo,
        "pre-commit",
        &format!(
            "echo \"$GIT_INDEX_FILE\" > '{0}'\ngit diff --cached --name-only >> '{0}'",
            seen.display()
        ),
    );
    hook(&repo, "post-commit", &format!("touch '{}'", post.display()));
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, commit(&["a.txt"], "with hooks", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    let seen = fs::read_to_string(&seen).unwrap();
    let mut lines = seen.lines();
    let index = lines.next().unwrap();
    assert!(index.contains("/gg-commit-"), "{index}");
    assert!(index.ends_with("/index"), "{index}");
    assert_eq!(
        lines.collect::<Vec<_>>(),
        ["a.txt"],
        "only the ticked file is staged there"
    );
    assert!(post.exists(), "post-commit ran");
    assert!(!Path::new(index).exists(), "removed after");
}

/// The message is taken as written (a leading `-` is no option), trailing blank lines
/// trimmed; a line starting with `#` ("#123 fixes …") is kept (P3-R17).
#[tokio::test]
async fn the_message_is_taken_as_written() {
    let repo = three_files();
    repo.write("a.txt", "edited\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let message = "-v not an option\n\nbody line\n#123 is kept\n\n\n";
    let out = h.run(id, commit(&["a.txt"], message, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(
        git_raw(repo.path(), &["log", "-1", "--format=%B"]),
        "-v not an option\n\nbody line\n#123 is kept\n\n"
    );
}

/// git does not know who the user is: a plain explanation, nothing committed.
#[tokio::test]
async fn a_missing_identity_is_explained() {
    let repo = three_files();
    repo.git(&["config", "--unset", "user.name"]);
    repo.git(&["config", "--unset", "user.email"]);
    repo.git(&["config", "user.useConfigOnly", "true"]);
    repo.write("a.txt", "edited\n");
    let before = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, commit(&["a.txt"], "who", false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message
            .starts_with("Nothing was committed: git does not know who you are."),
        "{}",
        out.message
    );
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    assert_eq!(porcelain(repo.path()), [" M a.txt"]);
}

/// A signing program that wants a passphrase on the terminal fails at once (R41: git runs
/// with no terminal), said with the README pointer.
#[cfg(unix)]
#[tokio::test]
async fn a_signing_prompt_fails_fast_with_the_readme_pointer() {
    use std::os::unix::fs::PermissionsExt;
    let repo = three_files();
    let gpg = repo.dir.path().join("fake-gpg");
    // It reads the passphrase from the terminal, else waits a long time.
    fs::write(
        &gpg,
        "#!/bin/sh\ncat /dev/tty > /dev/null || exit 2\nsleep 60\n",
    )
    .unwrap();
    fs::set_permissions(&gpg, fs::Permissions::from_mode(0o755)).unwrap();
    repo.git(&["config", "commit.gpgsign", "true"]);
    repo.git(&["config", "user.signingkey", "TESTKEY"]);
    repo.git(&["config", "gpg.program", &gpg.to_string_lossy()]);
    repo.write("a.txt", "edited\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let started = Instant::now();
    let out = h.run(id, commit(&["a.txt"], "signed", false)).await;
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "not the 60 s wait"
    );
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message.contains("could not be signed") && out.message.contains("README"),
        "{}",
        out.message
    );
    assert_eq!(porcelain(repo.path()), [" M a.txt"]);
    assert!(scratch_left(&repo.path().join(".git")).is_empty());
}

/// The first commit of a branch: no HEAD to start from, so the temporary index starts
/// empty; the files not ticked stay untracked.
#[tokio::test]
async fn the_first_commit_takes_only_the_ticked_files() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, commit(&["a.txt"], "first", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    let new = repo.git(&["rev-parse", "HEAD"]);
    assert_eq!(
        out.message,
        format!("Committed 1 file to main ({}).", &new[..8])
    );
    assert_eq!(changed_by(repo.path(), &new), set(&["a.txt"]));
    assert!(writes(&h, &out).iter().all(|w| w[0] != "read-tree"));
    assert_eq!(porcelain(repo.path()), ["?? b.txt"]);
}

/// A file with conflicts cannot be ticked (here from a stash pop, nothing in progress);
/// other files still can.
#[tokio::test]
async fn a_conflicted_file_cannot_be_committed() {
    let repo = TestRepo::new();
    repo.write("notes", "one\n");
    repo.write("other", "x\n");
    repo.commit_all("notes");
    repo.write("notes", "stashed\n");
    repo.git(&["stash", "push", "-q"]);
    repo.write("notes", "two\n");
    repo.commit_all("notes two");
    assert!(!repo.git_succeeds(&["stash", "pop", "-q"]));
    repo.write("other", "edited\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let mark = h.last_log_id();
    let out = h
        .run(id, commit(&["notes", "other"], "with a conflict", false))
        .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert_eq!(
        out.message,
        "Nothing was committed: notes still has conflicts. Resolve them first: a file with \
         conflicts cannot be committed."
    );
    assert!(h.writes_since(mark).is_empty());
    let out = h.run(id, commit(&["other"], "the other one", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(
        changed_by(repo.path(), &repo.git(&["rev-parse", "HEAD"])),
        set(&["other"])
    );
}

// ---- What git is stopped in ----

/// A merge with no conflicts left is concluded with all its changed files (git allows no
/// partial commit then); with conflicts left, or a tracked change not ticked, it is refused.
#[tokio::test]
async fn a_merge_is_concluded_only_whole_and_without_conflicts() {
    let repo = TestRepo::new();
    conflicting(&repo);
    repo.write("unrelated.txt", "tracked\n");
    repo.commit_all("unrelated, tracked");
    assert!(!repo.git_succeeds(&["merge", "feature"]));
    repo.write("unrelated.txt", "edited during the merge\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let mark = h.last_log_id();
    let out = h
        .run(id, commit(&["clash.txt", "unrelated.txt"], "merge", false))
        .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert_eq!(
        out.message,
        "The merge still has conflicts in clash.txt: resolve them, then commit."
    );
    repo.write("clash.txt", "both\n");
    repo.git(&["add", "clash.txt"]);
    let out = h.run(id, commit(&["clash.txt"], "merge", false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert_eq!(
        out.message,
        "A merge is concluded with all its changed files (new files git does not track yet \
         may stay out)."
    );
    assert!(h.writes_since(mark).is_empty(), "nothing ran");
    assert!(in_git_dir(&repo, "MERGE_HEAD"));
    let out = h
        .run(
            id,
            commit(&["clash.txt", "unrelated.txt"], "Merge feature", false),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(
        out.message
            .starts_with("Concluded the merge: committed 2 files to main ("),
        "{}",
        out.message
    );
    assert!(!in_git_dir(&repo, "MERGE_HEAD"));
    assert_eq!(
        repo.git(&["rev-list", "--parents", "-n1", "HEAD"])
            .split(' ')
            .count(),
        3
    );
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "Merge feature");
    assert!(porcelain(repo.path()).is_empty());
}

/// While a rebase (or cherry-pick, revert) is stopped, a commit is refused: Continue makes
/// its commit (R17').
#[tokio::test]
async fn a_commit_is_refused_while_a_rebase_is_stopped() {
    let repo = TestRepo::new();
    conflicting(&repo);
    assert!(!repo.git_succeeds(&["rebase", "feature"]));
    repo.write("clash.txt", "resolved\n");
    repo.git(&["add", "clash.txt"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let mark = h.last_log_id();
    let out = h.run(id, commit(&["clash.txt"], "mine", false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message
            .starts_with("A rebase is in progress: Continue commits its changes and goes on"),
        "{}",
        out.message
    );
    assert!(h.writes_since(mark).is_empty());
}

// ---- Bound to what the user saw ----

/// A commit runs only on the HEAD and in the worktree it was asked for (R39).
#[tokio::test]
async fn a_commit_runs_only_on_the_head_and_worktree_it_was_asked_for() {
    let repo = three_files();
    let shown = repo.git(&["rev-parse", "HEAD"]);
    repo.commit("made in a terminal");
    repo.write("a.txt", "edited\n");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let id = snap.repo_id;
    let mark = h.last_log_id();
    let mut stale = commit(&["a.txt"], "m", false);
    if let OpRequest::Commit { expect_head, .. } = &mut stale {
        *expect_head = HeadInfo {
            branch: Some("main".into()),
            hash: Some(shown),
        };
    }
    let out = h.run(id, stale).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message.contains("main has moved since"),
        "{}",
        out.message
    );
    let mut elsewhere = commit(&["a.txt"], "m", false);
    if let OpRequest::Commit {
        expect_worktree, ..
    } = &mut elsewhere
    {
        *expect_worktree = repo.dir.path().join("other").to_string_lossy().into_owned();
    }
    let out = h.run(id, elsewhere).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message.contains("is the active worktree now"),
        "{}",
        out.message
    );
    assert!(h.writes_since(mark).is_empty());
    assert_eq!(porcelain(repo.path()), [" M a.txt"]);
}

/// In a linked worktree the temporary index sits beside that worktree's own index (its git
/// dir); the main worktree's index is not touched.
#[tokio::test]
async fn a_linked_worktree_commits_with_an_index_beside_its_own() {
    let repo = three_files();
    repo.write("b.txt", "staged in main\n");
    repo.git(&["add", "b.txt"]);
    let wt = repo.add_worktree("feature");
    fs::write(wt.join("a.txt"), "in the worktree\n").unwrap();
    let main_index = index_state(repo.path());
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_active_worktree(id, &wt).await.unwrap();
    let out = h.run(id, commit(&["a.txt"], "in feature", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(out.message.contains("to feature"), "{}", out.message);
    assert_eq!(git_in(&wt, &["show", "HEAD:a.txt"]), "in the worktree");
    assert_eq!(repo.git(&["show", "main:a.txt"]), "a.txt");
    assert_eq!(index_state(repo.path()), main_index);
    assert!(porcelain(&wt).is_empty());
    let git_dir = repo.path().join(".git/worktrees/wt-feature");
    assert!(git_dir.is_dir());
    assert!(scratch_left(&git_dir).is_empty());
}

/// Stopped while a hook hangs (R41): nothing committed, the user's index as it was, the
/// temporary files gone.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stopped_commit_leaves_no_temporary_files_and_the_index_as_it_was() {
    let repo = three_files();
    repo.write("a.txt", "edited\n");
    repo.write("b.txt", "staged\n");
    repo.git(&["add", "b.txt"]);
    let marker = repo.dir.path().join("hook-started");
    hook(
        &repo,
        "pre-commit",
        &format!("touch '{}'\nsleep 60", marker.display()),
    );
    let before = repo.git(&["rev-parse", "HEAD"]);
    let index = index_state(repo.path());
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let request = h.at_head(id, commit(&["a.txt"], "slow", false)).await;
    let run = tokio::spawn({
        let m = h.m.clone();
        async move { m.run_op(id, request, &[]).await }
    });
    let running = h
        .sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"].is_object()
        })
        .await
        .expect("the commit runs");
    let op_id = running.1["op"]["id"].as_u64().unwrap();
    let started = Instant::now();
    while !marker.exists() {
        assert!(started.elapsed() < TIMEOUT, "the hook ran");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !scratch_left(&repo.path().join(".git")).is_empty(),
        "in use"
    );
    assert!(h.m.stop_op(id, op_id).unwrap());
    let out = tokio::time::timeout(TIMEOUT, run)
        .await
        .expect("ends once stopped")
        .unwrap()
        .unwrap();
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert_eq!(out.message, "Stopped.");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    assert_eq!(index_state(repo.path()), index);
    assert!(scratch_left(&repo.path().join(".git")).is_empty());
}

// ---- Commit and push ----

/// With `push`, the branch goes to the branch of the same name on its remote after the
/// commit (R38), getting it as its upstream when it has none.
#[tokio::test]
async fn commit_and_push_sends_the_branch_to_its_namesake() {
    let repo = three_files();
    let origin = repo.clone_as_remote();
    repo.git(&["checkout", "-q", "-b", "topic"]);
    repo.write("a.txt", "edited\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, commit(&["a.txt"], "and push", true)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(out.title, "Commit 1 file on topic and push");
    let new = repo.git(&["rev-parse", "HEAD"]);
    assert_eq!(
        out.message,
        format!(
            "Committed 1 file to topic ({}). Pushed topic to origin/topic, and set it as its \
             upstream.",
            &new[..8]
        )
    );
    assert_eq!(origin.git(&["rev-parse", "refs/heads/topic"]), new);
    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "topic@{upstream}"]),
        "origin/topic"
    );
}

/// The commit is made; the push fails: said apart, with a push to try again.
#[tokio::test]
async fn a_failed_push_is_said_after_the_commit() {
    let repo = three_files();
    let origin = repo.clone_as_remote();
    let other = TestRepo::clone_of(&origin);
    other.commit("theirs");
    other.git(&["push", "-q", "origin", "main"]);
    repo.write("a.txt", "edited\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let before = repo.git(&["rev-parse", "HEAD"]);
    let out = h.run(id, commit(&["a.txt"], "mine", true)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message
            .starts_with("Committed. The push failed: The remote has commits"),
        "{}",
        out.message
    );
    assert_eq!(
        out.hints[0],
        OpHint::Push {
            branch: "main".into(),
            remote: "origin".into()
        }
    );
    assert_eq!(repo.git(&["rev-parse", "HEAD^"]), before, "committed");
    assert!(
        out.detail
            .as_deref()
            .unwrap()
            .starts_with("Committed 1 file to main (")
    );
}

/// A push nobody can send (no remote, a detached HEAD) is refused before anything is
/// committed.
#[tokio::test]
async fn a_push_that_cannot_go_anywhere_is_refused_before_committing() {
    let repo = three_files();
    repo.write("a.txt", "edited\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, commit(&["a.txt"], "m", true)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert_eq!(
        out.message,
        "Nothing was committed: this repository has no remote to push to. Commit without \
         pushing."
    );
    repo.git(&["checkout", "-q", "--detach"]);
    let out = h.run(id, commit(&["a.txt"], "m", true)).await;
    assert!(out.message.contains("HEAD is detached"), "{}", out.message);
    assert_eq!(porcelain(repo.path()), [" M a.txt"]);
}

// ---- The file list ----

fn entry(path: &str, status: EntryStatus, staged: bool, unstaged: bool) -> FileEntry {
    FileEntry {
        path: path.into(),
        orig_path: None,
        status,
        staged,
        unstaged,
        committable: true,
        reason: None,
    }
}

async fn files_of(h: &Harness, id: u32, wt: &Path) -> WorktreeFiles {
    h.m.worktree_files(id, wt).await.unwrap()
}

/// Every kind of change, with odd paths; submodules are left out, as in the counts.
#[tokio::test]
async fn the_file_list_names_every_kind_of_change() {
    let sub = TestRepo::new();
    sub.commit("sub one");
    let repo = TestRepo::new();
    for f in [
        "modified",
        "deleted",
        "both",
        "renamed from",
        "staged deletion",
        "typed",
    ] {
        repo.write(f, format!("{f}\n"));
    }
    repo.write("notes", "one\n");
    repo.commit_all("base");
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        &sub.path().to_string_lossy(),
        "lib",
    ]);
    repo.commit_all("with a submodule");
    fs::write(repo.path().join("lib/inside.txt"), "x").unwrap();
    // A conflict (from a stash pop).
    repo.write("notes", "stashed\n");
    repo.git(&["stash", "push", "-q", "--", "notes"]);
    repo.write("notes", "two\n");
    repo.commit_all("notes two");
    assert!(!repo.git_succeeds(&["stash", "pop", "-q"]));
    repo.write("modified", "edited\n");
    fs::remove_file(repo.path().join("deleted")).unwrap();
    repo.write("both", "staged\n");
    repo.git(&["add", "both"]);
    repo.write("both", "and edited\n");
    repo.git(&["mv", "renamed from", "renamed to"]);
    repo.git(&["rm", "-q", "staged deletion"]);
    repo.write("added", "new\n");
    repo.git(&["add", "added"]);
    #[cfg(unix)]
    {
        fs::remove_file(repo.path().join("typed")).unwrap();
        std::os::unix::fs::symlink("modified", repo.path().join("typed")).unwrap();
    }
    let odd = ["with space", "\u{fc}n\u{ef}", "-dash", "new\nline"];
    for p in odd {
        if cfg!(unix) || !p.contains('\n') {
            repo.write(p, "x\n");
        }
    }
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let list = files_of(&h, snap.repo_id, Path::new(&snap.active_worktree)).await;
    assert!(!list.truncated);
    let mut want = vec![
        entry("added", EntryStatus::A, true, false),
        entry("both", EntryStatus::M, true, true),
        entry("deleted", EntryStatus::D, false, true),
        entry("modified", EntryStatus::M, false, true),
        entry("notes", EntryStatus::U, false, true),
        FileEntry {
            orig_path: Some("renamed from".into()),
            ..entry("renamed to", EntryStatus::R, true, false)
        },
        entry("staged deletion", EntryStatus::D, true, false),
    ];
    if cfg!(unix) {
        want.push(entry("typed", EntryStatus::T, false, true));
    }
    for p in odd {
        if cfg!(unix) || !p.contains('\n') {
            want.push(entry(p, EntryStatus::Untracked, false, true));
        }
    }
    want.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(list.files, want);
}

/// At most 5000 files are listed, and the list says when there are more.
#[tokio::test]
async fn the_file_list_stops_at_5000() {
    let repo = three_files();
    for i in 0..5001 {
        repo.write(&format!("many/f{i:05}"), "");
    }
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let wt = PathBuf::from(&snap.active_worktree);
    let list = files_of(&h, snap.repo_id, &wt).await;
    assert_eq!(list.files.len(), 5000);
    assert!(list.truncated);
    assert_eq!(list.files[0].path, "many/f00000");
    fs::remove_file(repo.path().join("many/f05000")).unwrap();
    let list = files_of(&h, snap.repo_id, &wt).await;
    assert_eq!(list.files.len(), 5000);
    assert!(!list.truncated);
}

/// Any worktree of the repository can be listed, not only the active one; a folder that
/// is not one of its worktrees is refused, and one that is gone is said to be.
#[tokio::test]
async fn the_file_list_is_for_the_repositorys_worktrees_only() {
    let repo = three_files();
    let wt = repo.add_worktree("feature");
    fs::write(wt.join("there.txt"), "x").unwrap();
    let gone = repo.add_worktree("gone");
    fs::remove_dir_all(&gone).unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let list = files_of(&h, id, &wt).await;
    assert_eq!(
        list.files,
        [entry("there.txt", EntryStatus::Untracked, false, true)]
    );
    assert!(files_of(&h, id, repo.path()).await.files.is_empty());
    let err = h.m.worktree_files(id, repo.dir.path()).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
    let err = h.m.worktree_files(id, &wt.join("sub")).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
    let err = h.m.worktree_files(id, &gone).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::PathMissing);
}

// ---- Task 5 review fixes ----

/// The temporary index starts from the user's index's stat data (`read-tree --reset
/// --index-output`), so git hashes only what is staged there: a clean filter (LFS, say) runs
/// for the ticked file, not for every tracked one (once to stage it, once when the user's
/// index records the committed file's stat data).
#[cfg(unix)]
#[tokio::test]
async fn a_clean_filter_runs_for_the_ticked_files_only() {
    let repo = TestRepo::new();
    let count = repo.dir.path().join("filter-runs");
    repo.write(".gitattributes", "*.txt filter=count\n");
    repo.git(&[
        "config",
        "filter.count.clean",
        &format!(
            "sh -c 'echo \"${{GIT_INDEX_FILE:-the user index}}\" >> \"{}\"; cat'",
            count.display()
        ),
    ]);
    for i in 0..300 {
        repo.write(&format!("f{i}.txt"), format!("{i}\n"));
    }
    repo.commit_all("three hundred");
    // Files written in the same second as the index are "racily clean": git hashes them
    // again on any read. A refresh a second later records them as clean for good.
    std::thread::sleep(Duration::from_millis(1100));
    repo.git(&["update-index", "-q", "--really-refresh"]);
    repo.git(&["status", "--porcelain"]);
    repo.write("f7.txt", "edited\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    fs::write(&count, "").unwrap();
    let out = h.run(id, commit(&["f7.txt"], "one file", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    let runs = fs::read_to_string(&count).unwrap();
    // Building the commit: for the ticked file only (once; twice when git reads it again
    // because it was staged in the same second as the index was written: "racily clean").
    let building = runs.lines().filter(|l| l.contains("/gg-commit-")).count();
    assert!((1..=2).contains(&building), "{runs}");
    // In all (the user's index taking the commit, git's own reads after): a few, never 300.
    assert!(runs.lines().count() <= 5, "{runs}");
    assert_eq!(changed_by(repo.path(), "HEAD"), set(&["f7.txt"]));
}

/// Exact paths, no pathspec matching: the deletion of the file `dir`, now a folder with an
/// untracked and a staged file in it, commits the deletion only; the staging under `dir/`
/// stays as it was.
#[tokio::test]
async fn a_deleted_file_that_is_now_a_folder_commits_only_its_deletion() {
    let repo = three_files();
    repo.write("dir", "a file\n");
    repo.commit_all("dir is a file");
    fs::remove_file(repo.path().join("dir")).unwrap();
    repo.write("dir/x.txt", "untracked\n");
    repo.write("dir/y.txt", "staged, not ticked\n");
    repo.git(&["add", "dir/y.txt"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, commit(&["dir"], "dir goes", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(changed_by(repo.path(), "HEAD"), set(&["dir"]));
    assert_eq!(porcelain(repo.path()), ["?? dir/x.txt", "A  dir/y.txt"]);
}

/// A folder replaced by a file: committing the file also removes the folder's files, so
/// they must be ticked too; then all go in one commit.
#[tokio::test]
async fn a_folder_replaced_by_a_file_needs_its_files_ticked_too() {
    let repo = three_files();
    repo.write("sub/a.txt", "a\n");
    repo.write("sub/b.txt", "b\n");
    repo.commit_all("sub is a folder");
    fs::remove_dir_all(repo.path().join("sub")).unwrap();
    repo.write("sub", "now a file\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let mark = h.last_log_id();
    let out = h.run(id, commit(&["sub"], "sub", false)).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert_eq!(
        out.message,
        "Committing sub also removes 2 files under it; tick them too."
    );
    assert!(h.writes_since(mark).is_empty(), "nothing ran");
    let out = h
        .run(id, commit(&["sub", "sub/a.txt", "sub/b.txt"], "sub", false))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(
        changed_by(repo.path(), "HEAD"),
        set(&["sub", "sub/a.txt", "sub/b.txt"])
    );
    assert!(porcelain(repo.path()).is_empty());
}

/// A file a hook stages goes into the commit; the user's index follows exactly what the
/// commit changed, that file included, so nothing shows as changed after.
#[cfg(unix)]
#[tokio::test]
async fn a_file_a_hook_adds_is_committed_and_the_index_follows() {
    let repo = three_files();
    repo.write("a.txt", "edited\n");
    hook(
        &repo,
        "pre-commit",
        "echo generated > generated.txt\ngit add generated.txt",
    );
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, commit(&["a.txt"], "with generated", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(
        changed_by(repo.path(), "HEAD"),
        set(&["a.txt", "generated.txt"])
    );
    assert!(
        porcelain(repo.path()).is_empty(),
        "{:?}",
        porcelain(repo.path())
    );
}

/// Whether the file system of `dir` ignores case in names.
fn ignores_case(dir: &Path) -> bool {
    let probe = dir.join("Case-Probe");
    fs::write(&probe, "").unwrap();
    let folded = dir.join("case-probe").exists();
    fs::remove_file(&probe).unwrap();
    folded
}

/// A rename that changes only the case of a name, on a file system that ignores case: the
/// old name leaves the commit's index by name, so the rename is committed.
#[tokio::test]
async fn a_rename_of_case_only_is_committed() {
    let repo = three_files();
    if !ignores_case(repo.path()) {
        eprintln!("skipped: this file system tells names apart by case");
        return;
    }
    repo.write("Readme.md", "read me\n");
    repo.commit_all("readme");
    repo.git(&["mv", "Readme.md", "README.md"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, commit(&["README.md"], "case", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(
        changed_by(repo.path(), "HEAD"),
        set(&["README.md", "Readme.md"])
    );
    assert!(
        porcelain(repo.path()).is_empty(),
        "{:?}",
        porcelain(repo.path())
    );
}

/// A file added to the index and deleted from the working tree is listed; ticking it records
/// nothing (HEAD never had it) but drops the stale index entry, and says so.
#[tokio::test]
async fn a_staged_file_deleted_since_is_listed_and_its_entry_dropped() {
    let repo = three_files();
    repo.write("ghost.txt", "x\n");
    repo.git(&["add", "ghost.txt"]);
    fs::remove_file(repo.path().join("ghost.txt")).unwrap();
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let listed = files_of(&h, snap.repo_id, repo.path()).await;
    let ghost = listed
        .files
        .iter()
        .find(|f| f.path == "ghost.txt")
        .expect("listed");
    assert_eq!(ghost.status, EntryStatus::D);
    let before = repo.git(&["rev-parse", "HEAD"]);
    let out = h
        .run(snap.repo_id, commit(&["ghost.txt"], "m", false))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(
        out.message,
        "Nothing to commit; removed the staged entry for a deleted file."
    );
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    assert!(
        porcelain(repo.path()).is_empty(),
        "{:?}",
        porcelain(repo.path())
    );
}

/// A folder that became a link to another folder: the deletion of a file that was in it is
/// committed (no "beyond a symbolic link" from git).
#[cfg(unix)]
#[tokio::test]
async fn a_file_below_a_folder_that_became_a_link_is_committed_as_deleted() {
    let repo = three_files();
    repo.write("docs/a.md", "a\n");
    repo.commit_all("docs");
    let elsewhere = repo.dir.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("a.md"), "a\n").unwrap();
    fs::remove_dir_all(repo.path().join("docs")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, repo.path().join("docs")).unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h
        .run(id, commit(&["docs/a.md"], "docs/a.md goes", false))
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(changed_by(repo.path(), "HEAD"), set(&["docs/a.md"]));
    let out = h.run(id, commit(&["docs"], "docs is a link", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(changed_by(repo.path(), "HEAD"), set(&["docs"]));
    assert!(
        porcelain(repo.path()).is_empty(),
        "{:?}",
        porcelain(repo.path())
    );
}

/// Runs `request` on `id` and stops it once `marker` exists (a hook made it); its outcome.
#[cfg(unix)]
async fn stopped_once(h: &Harness, id: u32, request: OpRequest, marker: &Path) -> OpOutcome {
    let request = h.at_head(id, request).await;
    let run = tokio::spawn({
        let m = h.m.clone();
        async move { m.run_op(id, request, &[]).await }
    });
    let running = h
        .sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"].is_object()
        })
        .await
        .expect("it runs");
    let op_id = running.1["op"]["id"].as_u64().unwrap();
    let started = Instant::now();
    while !marker.exists() {
        assert!(started.elapsed() < TIMEOUT, "the hook ran");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(h.m.stop_op(id, op_id).unwrap());
    tokio::time::timeout(TIMEOUT, run)
        .await
        .expect("ends once stopped")
        .unwrap()
        .unwrap()
}

/// Stopped while the post-commit hook hangs: git has made the commit, so the outcome says
/// so, and the user's index follows it (the committed file no longer shows as changed; a
/// file staged but not ticked stays staged).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_after_the_commit_is_made_says_it_was_made() {
    let repo = three_files();
    repo.write("a.txt", "edited\n");
    repo.write("b.txt", "staged\n");
    repo.git(&["add", "b.txt"]);
    let marker = repo.dir.path().join("post-commit-started");
    hook(
        &repo,
        "post-commit",
        &format!("touch '{}'\nsleep 60", marker.display()),
    );
    let before = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = stopped_once(&h, id, commit(&["a.txt"], "slow after", false), &marker).await;
    let new = repo.git(&["rev-parse", "HEAD"]);
    assert_ne!(new, before, "committed");
    assert!(
        out.message
            .starts_with(&format!("Committed 1 file to main ({}).", &new[..8])),
        "{out:#?}"
    );
    assert!(out.message.contains("Stopped"), "{}", out.message);
    assert_eq!(porcelain(repo.path()), ["M  b.txt"]);
    assert!(scratch_left(&repo.path().join(".git")).is_empty());
}

/// Stopped during the push of a commit-and-push: the commit stands and is said, with a push
/// to try again.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_during_the_push_keeps_the_commit_and_offers_the_push() {
    let repo = three_files();
    let _origin = repo.clone_as_remote();
    repo.write("a.txt", "edited\n");
    let marker = repo.dir.path().join("pre-push-started");
    hook(
        &repo,
        "pre-push",
        &format!("touch '{}'\nsleep 60", marker.display()),
    );
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = stopped_once(&h, id, commit(&["a.txt"], "and push", true), &marker).await;
    let new = repo.git(&["rev-parse", "HEAD"]);
    assert!(
        out.message
            .starts_with(&format!("Committed 1 file to main ({}).", &new[..8])),
        "{out:#?}"
    );
    assert!(
        out.message.contains("Stopped before the push"),
        "{}",
        out.message
    );
    assert_eq!(
        out.hints.first(),
        Some(&OpHint::Push {
            branch: "main".into(),
            remote: "origin".into()
        }),
        "{out:#?}"
    );
    assert!(porcelain(repo.path()).is_empty());
}

/// Concluding a merge needs every tracked changed file; untracked ones may stay out
/// (P3-R17). git's own comment lines in the message (`# Conflicts:`) are removed then.
#[tokio::test]
async fn a_merge_is_concluded_without_the_untracked_files_and_without_comment_lines() {
    let repo = TestRepo::new();
    conflicting(&repo);
    assert!(!repo.git_succeeds(&["merge", "feature"]));
    repo.write("clash.txt", "both\n");
    repo.git(&["add", "clash.txt"]);
    repo.write("notes.txt", "untracked, not ticked\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let message = "Merge feature\n\n# Conflicts:\n#\tclash.txt\n";
    let out = h.run(id, commit(&["clash.txt"], message, false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(!in_git_dir(&repo, "MERGE_HEAD"));
    assert_eq!(
        git_raw(repo.path(), &["log", "-1", "--format=%B"]),
        "Merge feature\n\n"
    );
    assert_eq!(porcelain(repo.path()), ["?? notes.txt"]);
}

/// A name that is not valid UTF-8 is listed, marked as not committable with the reason, and
/// a commit that ticks it is refused (P3-R17).
#[cfg(unix)]
#[tokio::test]
async fn a_name_that_is_not_utf8_is_listed_but_not_committable() {
    use std::os::unix::ffi::OsStrExt;
    let repo = three_files();
    let odd = std::ffi::OsStr::from_bytes(b"caf\xe9.txt");
    if fs::write(repo.path().join(odd), "x\n").is_err() {
        eprintln!("skipped: this file system takes only UTF-8 names");
        return;
    }
    repo.write("a.txt", "edited\n");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let listed = files_of(&h, snap.repo_id, repo.path()).await;
    let bad = listed
        .files
        .iter()
        .find(|f| f.path.starts_with("caf"))
        .expect("listed");
    assert!(!bad.committable);
    assert!(
        bad.reason.as_deref().unwrap().contains("not valid UTF-8"),
        "{bad:?}"
    );
    assert!(
        listed
            .files
            .iter()
            .find(|f| f.path == "a.txt")
            .unwrap()
            .committable
    );
    let mark = h.last_log_id();
    let out = h
        .run(
            snap.repo_id,
            commit(&[bad.path.as_str(), "a.txt"], "m", false),
        )
        .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(
        out.message.starts_with("Nothing was committed:"),
        "{}",
        out.message
    );
    assert!(out.message.contains("not valid UTF-8"), "{}", out.message);
    assert!(h.writes_since(mark).is_empty());
}

/// Temporary folders left by a gg that crashed are removed at the next commit, once their
/// process is gone; those of a process still running are left alone (P3-R17).
#[cfg(unix)]
#[tokio::test]
async fn temporary_folders_of_a_gone_process_are_removed_at_the_next_commit() {
    let repo = three_files();
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let gone = child.id();
    child.wait().unwrap();
    let git_dir = repo.path().join(".git");
    let old = git_dir.join(format!("gg-commit-{gone}-7"));
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join("index"), "left over").unwrap();
    let alive = git_dir.join(format!("gg-commit-{}-999999", std::process::id()));
    fs::create_dir_all(&alive).unwrap();
    repo.write("a.txt", "edited\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let out = h.run(id, commit(&["a.txt"], "m", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(!old.exists(), "a gone process's folder is removed");
    assert!(alive.exists(), "a running process's folder is kept");
    fs::remove_dir_all(&alive).unwrap();
}
