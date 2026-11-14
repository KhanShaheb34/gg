//! Integration tests for what git stopped in: continuing and aborting it (`run_op`), the
//! operations refused meanwhile, and opening files in an editor, against real temp
//! repositories.

use std::fs;
use std::path::Path;

use gg_core::dto::{InProgressKind, MergeTargetKind, OpRequest, OpStatus};
use gg_core::error::ErrorCode;
use gg_core::git::runner::CmdKind;
use gg_core::testutil::TestRepo;

mod common;
use common::*;

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

fn args_of(h: &Harness, out: &gg_core::dto::OpOutcome) -> Vec<Vec<String>> {
    op_records(&h.log, out)
        .into_iter()
        .filter(|r| r.kind == CmdKind::Write)
        .map(|r| r.args)
        .collect()
}

fn cherry_pick(hash: &str) -> OpRequest {
    OpRequest::CherryPick {
        hash: hash.to_owned(),
        mainline: None,
        no_commit: false,
    }
}

fn revert(hash: &str) -> OpRequest {
    OpRequest::Revert {
        hash: hash.to_owned(),
        mainline: None,
    }
}

fn create_branch_and_checkout(name: &str, at: &str) -> OpRequest {
    OpRequest::CreateBranch {
        name: name.to_owned(),
        at: at.to_owned(),
        checkout: true,
    }
}

fn read(repo: &TestRepo, rel: &str) -> String {
    fs::read_to_string(repo.path().join(rel)).unwrap()
}

// ---- Continue ----

#[tokio::test]
async fn continue_stages_resolved_files_and_finishes_the_merge() {
    let repo = TestRepo::new();
    let names = ["clash.txt", "a b.txt", "[ab].txt", "gone.txt"];
    for name in names {
        repo.write(name, "base\n");
    }
    repo.write("notes.txt", "notes\n");
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
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, merge_branch("feature")).await;
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    let stopped = out
        .snapshot
        .in_progress
        .expect("the outcome shows the merge");
    assert_eq!(stopped.kind, InProgressKind::Merge);
    assert_eq!(
        stopped.conflicted,
        ["[ab].txt", "a b.txt", "clash.txt", "gone.txt"]
    );

    // Resolved by hand: three edited, the one deleted on main deleted. Beside them, an
    // untracked file a glob `[ab].txt` would match, and an unrelated change.
    for name in &names[..3] {
        repo.write(name, "resolved\n");
    }
    fs::remove_file(repo.path().join("gone.txt")).unwrap();
    repo.write("a.txt", "untracked\n");
    repo.write("notes.txt", "edited, not part of the merge\n");

    let out = h.run(id, OpRequest::Continue).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Continue the merge");
    assert_eq!(out.snapshot.in_progress, None);
    assert!(!in_git_dir(&repo, "MERGE_HEAD"));
    assert_eq!(
        args_of(&h, &out),
        [
            vec![
                "--literal-pathspecs",
                "add",
                "--",
                "[ab].txt",
                "a b.txt",
                "clash.txt",
                "gone.txt"
            ],
            vec!["merge", "--continue"],
        ]
    );
    assert_eq!(repo.git(&["rev-list", "--count", "--merges", "HEAD"]), "1");
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]),
        "Merge branch 'feature'"
    );
    assert_eq!(repo.git(&["show", "HEAD:clash.txt"]), "resolved");
    assert_eq!(
        repo.git(&["ls-tree", "--name-only", "HEAD", "--", "gone.txt"]),
        ""
    );
    assert_eq!(repo.git(&["show", "HEAD:notes.txt"]), "notes");
    assert_eq!(
        repo.git(&["status", "--porcelain"]),
        "M notes.txt\n?? a.txt",
        "the unrelated change and the untracked file are left alone"
    );
}

/// Continue never stages a file that still has conflict markers: it is refused, and nothing
/// is staged.
#[tokio::test]
async fn continue_is_refused_while_a_file_has_conflict_markers() {
    let repo = TestRepo::new();
    for name in ["clash.txt", "other.txt"] {
        repo.write(name, "base\n");
    }
    repo.commit_all("base");
    repo.branch("feature");
    repo.checkout("feature");
    for name in ["clash.txt", "other.txt"] {
        repo.write(name, "feature\n");
    }
    repo.commit_all("feature side");
    repo.checkout("main");
    for name in ["clash.txt", "other.txt"] {
        repo.write(name, "main\n");
    }
    repo.commit_all("main side");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(
        h.run(id, merge_branch("feature")).await.status,
        OpStatus::Stopped
    );
    repo.write("clash.txt", "resolved\n");

    let out = h.run(id, OpRequest::Continue).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message.contains("other.txt") && out.message.contains("conflict markers"),
        "{}",
        out.message
    );
    assert!(args_of(&h, &out).is_empty(), "nothing ran");
    assert!(in_git_dir(&repo, "MERGE_HEAD"), "still merging");
    assert_eq!(
        repo.git(&["diff", "--name-only", "--diff-filter=U"]),
        "clash.txt\nother.txt",
        "nothing staged"
    );
}

#[tokio::test]
async fn continue_commits_a_merge_stopped_without_conflicts() {
    let repo = TestRepo::new();
    repo.commit("base");
    repo.branch("side");
    repo.checkout("side");
    repo.commit("on side");
    repo.checkout("main");
    repo.commit("on main");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h
        .run(
            id,
            merge(MergeTargetKind::Branch, "side", true, false, true),
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    let progress = out.snapshot.in_progress.expect("merge in progress");
    assert_eq!(progress.kind, InProgressKind::Merge);
    assert!(progress.conflicted.is_empty());

    let out = h.run(id, OpRequest::Continue).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(args_of(&h, &out), [vec!["merge", "--continue"]]);
    assert_eq!(out.snapshot.in_progress, None);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]),
        "Merge branch 'side'"
    );
}

#[tokio::test]
async fn continue_rebase_that_stops_again_returns_stopped() {
    let repo = TestRepo::new();
    clashing(&repo);
    repo.checkout("feature");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, rebase("main")).await;
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    let p = out.snapshot.in_progress.expect("rebase in progress");
    assert_eq!(
        (p.kind, p.step, p.total),
        (InProgressKind::Rebase, Some(1), Some(3))
    );

    repo.write("clash.txt", "resolved one\n");
    let out = h.run(id, OpRequest::Continue).await;
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    assert_eq!(out.title, "Continue the rebase");
    assert!(out.message.contains("conflicts"), "{}", out.message);
    assert!(
        out.detail.as_deref().unwrap_or("").contains("CONFLICT"),
        "{out:?}"
    );
    let p = out.snapshot.in_progress.clone().expect("still in progress");
    assert_eq!(p.kind, InProgressKind::Rebase);
    assert_eq!((p.step, p.total), (Some(2), Some(3)));
    assert_eq!(p.conflicted, ["clash.txt"]);
    assert_eq!(p.subject.as_deref(), Some("feat two"));
    assert!(
        args_of(&h, &out)
            .iter()
            .any(|a| a == &["rebase", "--continue"]),
        "{:?}",
        args_of(&h, &out)
    );

    repo.write("clash.txt", "resolved two\n");
    assert_eq!(
        h.run(id, OpRequest::Continue).await.status,
        OpStatus::Stopped
    );
    repo.write("clash.txt", "resolved three\n");
    let out = h.run(id, OpRequest::Continue).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.snapshot.in_progress, None);
    assert_eq!(current_branch(&repo).as_deref(), Some("feature"));
    assert_eq!(read(&repo, "clash.txt"), "resolved three\n");
    assert_eq!(repo.git(&["rev-list", "--count", "main..feature"]), "3");
}

#[tokio::test]
async fn continue_finishes_a_cherry_pick_and_a_revert() {
    let repo = TestRepo::new();
    clashing(&repo);
    let one = repo.git(&["rev-parse", "feature~2"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;

    let out = h.run(id, cherry_pick(&one)).await;
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    repo.write("clash.txt", "picked\n");
    let out = h.run(id, OpRequest::Continue).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.title, "Continue the cherry-pick");
    assert_eq!(out.snapshot.in_progress, None);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "feat one");
    assert!(args_of(&h, &out).contains(&vec![
        "cherry-pick".to_owned(),
        "--continue".to_owned(),
        "--no-edit".to_owned()
    ]));

    let out = h.run(id, revert(&repo.git(&["rev-parse", "HEAD~1"]))).await;
    assert_eq!(out.status, OpStatus::Stopped, "{out:?}");
    repo.write("clash.txt", "reverted\n");
    let out = h.run(id, OpRequest::Continue).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.snapshot.in_progress, None);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]),
        "Revert \"main side\""
    );
}

/// A resolution that leaves the commit with no changes: git will not continue it, and the
/// core says so plainly, with git's text.
#[tokio::test]
async fn continue_that_comes_out_empty_explains() {
    let repo = TestRepo::new();
    clashing(&repo);
    let one = repo.git(&["rev-parse", "feature~2"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(h.run(id, cherry_pick(&one)).await.status, OpStatus::Stopped);
    repo.write("clash.txt", "main\n");

    let out = h.run(id, OpRequest::Continue).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(out.message.contains("empty"), "{}", out.message);
    assert!(out.message.contains("abort"), "{}", out.message);
    assert!(
        out.detail.as_deref().unwrap_or("").contains("now empty"),
        "{out:?}"
    );
    let p = out.snapshot.in_progress.expect("still in progress");
    assert_eq!(p.kind, InProgressKind::CherryPick);
    assert!(p.conflicted.is_empty());
}

#[tokio::test]
async fn continue_with_nothing_in_progress_is_nothing_to_do() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let mark = h.last_log_id();
    let events = h.changes(id).len();

    for request in [OpRequest::Continue, OpRequest::Abort] {
        let err =
            h.m.run_op(id, request.clone(), &[])
                .await
                .expect_err("nothing to continue or abort");
        assert_eq!(err.code, ErrorCode::NothingToDo, "{request:?}");
        assert!(err.message.contains("in progress"), "{}", err.message);
    }
    assert!(h.writes_since(mark).is_empty());
    assert_eq!(h.changes(id).len(), events, "nothing was published");
}

// ---- Abort ----

#[tokio::test]
async fn abort_restores_the_branch() {
    for kind in ["merge", "rebase", "cherry-pick", "revert"] {
        let repo = TestRepo::new();
        clashing(&repo);
        if kind == "rebase" || kind == "revert" {
            repo.checkout("feature");
        }
        let branch = current_branch(&repo).unwrap();
        let tip = repo.git(&["rev-parse", "HEAD"]);
        let h = harness().await;
        let id = h.open(repo.path()).await.repo_id;
        let request = match kind {
            "merge" => merge_branch("feature"),
            "rebase" => rebase("main"),
            "cherry-pick" => cherry_pick(&repo.git(&["rev-parse", "feature~2"])),
            _ => revert(&repo.git(&["rev-parse", "feature~2"])),
        };
        let out = h.run(id, request).await;
        assert_eq!(out.status, OpStatus::Stopped, "{kind}: {out:?}");

        let out = h.run(id, OpRequest::Abort).await;
        assert_eq!(out.status, OpStatus::Done, "{kind}: {out:?}");
        assert_eq!(out.title, format!("Abort the {kind}"));
        assert!(out.message.contains(&branch), "{kind}: {}", out.message);
        assert_eq!(args_of(&h, &out), [vec![kind, "--abort"]], "{kind}");
        assert_eq!(out.snapshot.in_progress, None, "{kind}");
        assert_eq!(current_branch(&repo).as_deref(), Some(branch.as_str()));
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), tip, "{kind}");
        assert_eq!(repo.git(&["status", "--porcelain"]), "", "{kind}");
    }
}

// ---- Other operations meanwhile ----

#[tokio::test]
async fn other_operations_explain_while_in_progress() {
    let repo = TestRepo::new();
    clashing(&repo);
    let tip = repo.git(&["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(
        h.run(id, merge_branch("feature")).await.status,
        OpStatus::Stopped
    );
    let mark = h.last_log_id();
    let events = h.changes(id).len();

    let one = repo.git(&["rev-parse", "feature~2"]);
    for request in [
        checkout_branch("feature"),
        create_branch_and_checkout("later", &tip),
        merge_branch("feature"),
        rebase("feature"),
        cherry_pick(&one),
        revert(&tip),
        OpRequest::Pull {
            branch: "main".into(),
            mode: gg_core::dto::PullMode::Default,
        },
    ] {
        let out = h.run(id, request.clone()).await;
        assert_eq!(out.status, OpStatus::Failed, "{request:?}: {out:?}");
        assert!(
            out.message.contains("A merge is in progress"),
            "{}",
            out.message
        );
        assert!(
            out.message.contains("continue it or abort it"),
            "{}",
            out.message
        );
        assert_eq!((out.log_from, out.log_to), (0, 0));
    }
    assert!(h.writes_since(mark).is_empty(), "nothing ran");
    assert_eq!(h.changes(id).len(), events, "nothing was published");
    assert_eq!(current_branch(&repo).as_deref(), Some("main"));
    assert!(in_git_dir(&repo, "MERGE_HEAD"));

    // What leaves HEAD and the working tree alone still runs.
    let out = h.run(id, create_branch("later", &tip)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(out.snapshot.in_progress.is_some());

    // So does a reset (a way out of a merge), a hard one still only once confirmed.
    let hard = reset(&tip, gg_core::dto::ResetMode::Hard);
    let out = h.run(id, hard.clone()).await;
    assert_eq!(out.status, OpStatus::NeedsConfirmation, "{out:?}");
    let out = h
        .run_confirmed(id, hard, &[gg_core::dto::DangerKind::HardReset])
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert_eq!(out.snapshot.in_progress, None);
    assert!(!in_git_dir(&repo, "MERGE_HEAD"));
}

// ---- Open in editor ----

#[tokio::test]
async fn open_in_editor_rejects_paths_outside_the_worktree() {
    let repo = TestRepo::new();
    repo.write("a b.txt", "x\n");
    repo.write("dir/inner.txt", "x\n");
    repo.commit_all("files");
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.txt"), "x").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), repo.path().join("link")).unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.record_editor_launches();

    let escaping = outside.path().join("secret.txt");
    let escaping = escaping.to_str().unwrap();
    for path in [
        "../secret.txt",
        "dir/../../secret.txt",
        escaping,
        "",
        #[cfg(unix)]
        "link/secret.txt",
    ] {
        let err =
            h.m.open_in_editor(id, Some(path), None)
                .await
                .expect_err(path);
        assert_eq!(err.code, ErrorCode::BadRequest, "{path}: {err:?}");
    }
    let err =
        h.m.open_in_editor(id, Some("missing.txt"), None)
            .await
            .expect_err("missing");
    assert_eq!(err.code, ErrorCode::PathMissing, "{err:?}");
    assert!(h.m.editor_launches().is_empty(), "nothing was opened");

    let root = dunce::canonicalize(repo.path()).unwrap();
    let editor = Some("my-editor");
    h.m.open_in_editor(id, Some("a b.txt"), editor)
        .await
        .unwrap();
    h.m.open_in_editor(id, Some("dir/./inner.txt"), editor)
        .await
        .unwrap();
    h.m.open_in_editor(id, None, editor).await.unwrap();
    let opened: Vec<String> =
        h.m.editor_launches()
            .into_iter()
            .map(|argv| argv.last().cloned().unwrap())
            .collect();
    let at = |rel: &Path| root.join(rel).to_string_lossy().into_owned();
    assert_eq!(
        opened,
        [
            at(Path::new("a b.txt")),
            at(&Path::new("dir").join("inner.txt")),
            root.to_string_lossy().into_owned(),
        ]
    );
}

#[tokio::test]
async fn editor_command_is_split_without_a_shell() {
    let repo = TestRepo::new();
    repo.write("a.txt", "x\n");
    repo.commit_all("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.record_editor_launches();
    let file = dunce::canonicalize(repo.path().join("a.txt"))
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let command = r#"my-editor  --wait 'two words' "$(touch pwned)" ; rm -rf x"#;
    h.m.open_in_editor(id, Some("a.txt"), Some(command))
        .await
        .unwrap();
    // Without a command: as text on macOS; elsewhere the worktree folder, with a note
    // (the file itself could be a program the default application runs).
    let note = h.m.open_in_editor(id, Some("a.txt"), None).await.unwrap();
    let root = dunce::canonicalize(repo.path()).unwrap();
    let root = root.to_string_lossy().into_owned();
    let expected_opener: Vec<String> = if cfg!(target_os = "macos") {
        assert_eq!(note, None);
        vec!["/usr/bin/open".into(), "-t".into(), file.clone()]
    } else {
        assert!(
            note.as_deref().unwrap_or("").contains("Settings"),
            "{note:?}"
        );
        let opener = if cfg!(windows) {
            let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
            format!(r"{windows}\explorer.exe")
        } else {
            "xdg-open".to_owned()
        };
        vec![opener, root]
    };
    assert_eq!(
        h.m.editor_launches(),
        [
            vec![
                "my-editor".to_owned(),
                "--wait".to_owned(),
                "two words".to_owned(),
                "$(touch pwned)".to_owned(),
                ";".to_owned(),
                "rm".to_owned(),
                "-rf".to_owned(),
                "x".to_owned(),
                file,
            ],
            expected_opener,
        ]
    );

    let err =
        h.m.open_in_editor(id, Some("a.txt"), Some("code 'unclosed"))
            .await
            .expect_err("unclosed quote");
    assert_eq!(err.code, ErrorCode::BadRequest, "{err:?}");
}

/// For real: the program runs directly, so `$(…)` and `;` reach it as arguments (here
/// `touch` makes files of those names), and the call returns without waiting for it.
#[cfg(unix)]
#[tokio::test]
async fn editor_runs_detached_without_a_shell() {
    let repo = TestRepo::new();
    repo.write("a.txt", "x\n");
    repo.commit_all("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.open_in_editor(id, Some("a.txt"), Some("touch '$(echo hi)' ;"))
        .await
        .unwrap();
    let made = |name: &str| repo.path().join(name).exists();
    let deadline = std::time::Instant::now() + TIMEOUT;
    while !(made("$(echo hi)") && made(";")) {
        assert!(std::time::Instant::now() < deadline, "touch ran");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(!made("hi"));

    let err =
        h.m.open_in_editor(id, Some("a.txt"), Some("gg-no-such-editor --wait"))
            .await
            .expect_err("no such program");
    assert!(err.message.contains("gg-no-such-editor"), "{err:?}");
    assert!(err.message.contains("Settings"), "{err:?}");
}

/// A cherry-pick sequence whose current commit the user committed by hand: continue goes
/// on with the rest; abort ends it, and says that git keeps the commit made by hand.
#[tokio::test]
async fn a_sequence_committed_by_hand_continues_and_aborts() {
    for abort in [false, true] {
        let repo = TestRepo::new();
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
        let before = repo.commit_all("main side");
        assert!(!repo.git_succeeds(&["cherry-pick", "feature~1", "feature"]));
        repo.write("clash.txt", "resolved\n");
        repo.git(&["add", "clash.txt"]);
        repo.git(&["commit", "-q", "--no-edit"]);
        let h = harness().await;
        let snap = h.open(repo.path()).await;
        let id = snap.repo_id;
        assert_eq!(
            snap.in_progress.map(|p| p.kind),
            Some(InProgressKind::CherryPick)
        );
        let out = h
            .run(
                id,
                if abort {
                    OpRequest::Abort
                } else {
                    OpRequest::Continue
                },
            )
            .await;
        assert_eq!(out.status, OpStatus::Done, "{out:?}");
        assert!(out.snapshot.in_progress.is_none());
        assert!(!in_git_dir(&repo, "sequencer"));
        if abort {
            // git does not rewind over a commit made by hand, and the message says so.
            assert_eq!(repo.git(&["rev-parse", "HEAD~1"]), before);
            assert!(
                out.message.contains("without going back"),
                "{}",
                out.message
            );
        } else {
            assert_eq!(read(&repo, "other.txt"), "other\n");
            assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "feat other");
        }
    }
}
