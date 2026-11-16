//! Worktrees as first-class state: what the snapshot says of each worktree, switching the
//! active worktree (where operations run, what HEAD, the status and the uncommitted row are),
//! `expectWorktree`, falling back to the main worktree when the active one is removed, the
//! background reads of the other worktrees, and the active worktree's file watch.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gg_core::dto::{
    InProgressKind, OpStatus, RepoSnapshot, RowKind, WorktreeInfo, WorktreeStatus, WorktreeUpstream,
};
use gg_core::error::ErrorCode;
use gg_core::git::runner::CommandRecord;
use gg_core::testutil::{TestRepo, empty_git_config};

mod common;
use common::*;

/// Runs git in `dir` (any worktree) and returns trimmed stdout; panics if git fails.
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

/// Whether git succeeds in `dir`.
fn git_ok(dir: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", empty_git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn real(path: &Path) -> String {
    dunce::canonicalize(path)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// The worktree of `snap` whose path ends with `name`.
fn wt<'a>(snap: &'a RepoSnapshot, name: &str) -> &'a WorktreeInfo {
    snap.worktrees
        .iter()
        .find(|w| w.path.ends_with(name))
        .unwrap_or_else(|| panic!("no worktree {name} in {:?}", snap.worktrees))
}

fn counts(changed: u32, untracked: u32, conflicted: u32, paths: &[&str]) -> WorktreeStatus {
    WorktreeStatus {
        changed,
        untracked,
        conflicted,
        conflicted_paths: paths.iter().map(|p| (*p).to_owned()).collect(),
    }
}

/// Waits (up to `TIMEOUT`) until the snapshot of `id` satisfies `ok`; returns it.
async fn until(
    h: &Harness,
    id: u32,
    what: &str,
    ok: impl Fn(&RepoSnapshot) -> bool,
) -> RepoSnapshot {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let snap = h.m.snapshot(id).await.unwrap();
        if ok(&snap) {
            return snap;
        }
        assert!(Instant::now() < deadline, "{what}: {snap:#?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The `status` commands run after `mark`, in `cwd` if given.
fn status_reads(h: &Harness, mark: u64, cwd: Option<&Path>) -> Vec<CommandRecord> {
    h.log
        .since(mark)
        .into_iter()
        .filter(|r| r.args.first().map(String::as_str) == Some("status"))
        .filter(|r| cwd.is_none_or(|c| Path::new(&r.cwd) == Path::new(&real(c))))
        .collect()
}

fn repo_errors(h: &Harness, id: u32) -> Vec<serde_json::Value> {
    h.sink
        .events()
        .into_iter()
        .filter(|(n, v)| n == "repo-error" && v["repoId"] == id)
        .map(|(_, v)| v)
        .collect()
}

#[tokio::test]
async fn the_snapshot_says_how_each_worktree_stands() {
    let repo = TestRepo::new();
    repo.write("clash.txt", "base\n");
    repo.commit_all("base");
    repo.branch("other");
    repo.checkout("other");
    repo.write("clash.txt", "other\n");
    repo.commit_all("other side");
    repo.checkout("main");
    let _origin = repo.clone_as_remote();
    repo.git(&["branch", "--set-upstream-to=origin/main", "main"]);

    let dirty = repo.add_worktree("dirty");
    fs::write(dirty.join("clash.txt"), "edited\n").unwrap();
    let untracked = repo.add_worktree("untracked");
    fs::write(untracked.join("new.txt"), "new\n").unwrap();
    // Ahead 2, behind 1: one commit pushed and then dropped here, two made after.
    let ahead = repo.add_worktree("ahead");
    fs::write(ahead.join("r.txt"), "r\n").unwrap();
    git_in(&ahead, &["add", "r.txt"]);
    git_in(&ahead, &["commit", "-qm", "pushed"]);
    git_in(&ahead, &["push", "-q", "-u", "origin", "ahead"]);
    git_in(&ahead, &["reset", "-q", "--hard", "HEAD~"]);
    for n in ["l1", "l2"] {
        fs::write(ahead.join(n), n).unwrap();
        git_in(&ahead, &["add", n]);
        git_in(&ahead, &["commit", "-qm", n]);
    }
    let detached = repo.path().parent().unwrap().join("wt-detached");
    repo.git(&[
        "worktree",
        "add",
        "-q",
        "--detach",
        &detached.to_string_lossy(),
        "main",
    ]);
    let locked = repo.add_worktree("locked");
    repo.git(&[
        "worktree",
        "lock",
        "--reason",
        "on a usb disk",
        &locked.to_string_lossy(),
    ]);
    let gone = repo.add_worktree("gone");
    fs::remove_dir_all(&gone).unwrap();
    let merging = repo.add_worktree("merging");
    fs::write(merging.join("clash.txt"), "mine\n").unwrap();
    git_in(&merging, &["commit", "-qam", "mine"]);
    assert!(
        !git_ok(&merging, &["merge", "-q", "other"]),
        "the merge stops"
    );

    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let snap = until(&h, snap.repo_id, "every worktree read", |s| {
        s.worktrees
            .iter()
            .all(|w| w.unpushed.is_some() && (w.prunable || w.status.is_some()))
    })
    .await;
    assert_eq!(snap.worktrees.len(), 8, "{:#?}", snap.worktrees);

    let main = &snap.worktrees[0];
    assert!(main.is_main && main.active);
    assert_eq!(main.status, Some(WorktreeStatus::default()));
    assert_eq!(
        main.upstream,
        Some(WorktreeUpstream {
            name: "origin/main".into(),
            ahead: 0,
            behind: 0
        })
    );
    assert_eq!((main.unpushed, main.in_progress), (Some(0), None));
    assert_eq!(
        main.status,
        Some(snap.status.clone()),
        "the active one's own"
    );

    let w = wt(&snap, "wt-dirty");
    assert!(!w.active);
    assert_eq!(w.status, Some(counts(1, 0, 0, &[])));
    assert_eq!(
        (w.upstream.clone(), w.unpushed),
        (None, Some(0)),
        "no upstream"
    );

    let w = wt(&snap, "wt-untracked");
    assert_eq!(w.status, Some(counts(0, 1, 0, &[])));

    let w = wt(&snap, "wt-ahead");
    assert_eq!(
        w.upstream,
        Some(WorktreeUpstream {
            name: "origin/ahead".into(),
            ahead: 2,
            behind: 1
        })
    );
    assert_eq!(w.unpushed, Some(2));
    assert_eq!(w.status, Some(WorktreeStatus::default()));

    let w = wt(&snap, "wt-detached");
    assert_eq!((w.branch.clone(), w.upstream.clone()), (None, None));
    assert_eq!(
        (w.unpushed, w.status.clone()),
        (Some(0), Some(WorktreeStatus::default()))
    );

    let w = wt(&snap, "wt-locked");
    assert!(w.locked);
    assert_eq!(w.lock_reason.as_deref(), Some("on a usb disk"));

    let w = wt(&snap, "wt-gone");
    assert!(w.prunable);
    assert_eq!(
        (w.status.clone(), w.in_progress),
        (None, None),
        "nothing to read"
    );
    assert_eq!(w.unpushed, Some(0), "its commits are known all the same");

    let w = wt(&snap, "wt-merging");
    assert_eq!(w.in_progress, Some(InProgressKind::Merge));
    assert_eq!(w.status, Some(counts(1, 0, 1, &["clash.txt"])));
    assert_eq!(w.unpushed, Some(1));
    assert!(snap.worktrees.iter().all(|w| w.pr.is_none()));
}

#[tokio::test]
async fn switching_moves_head_status_in_progress_the_uncommitted_row_and_where_ops_run() {
    let repo = TestRepo::new();
    conflicting(&repo);
    repo.git(&["branch", "-m", "feature", "feature-side"]);
    let feature = repo.add_worktree("feature");
    fs::write(feature.join("clash.txt"), "edited\n").unwrap();
    assert!(
        !repo.git_succeeds(&["merge", "-q", "feature-side"]),
        "main stops in a merge"
    );
    let h = harness().await;
    let before = h.open(repo.path()).await;
    let id = before.repo_id;
    assert_eq!(before.active_worktree, real(repo.path()));
    assert_eq!(
        before.in_progress.as_ref().map(|p| p.kind),
        Some(InProgressKind::Merge)
    );

    let started = Instant::now();
    let snap = h.m.set_active_worktree(id, &feature).await.unwrap();
    let took = started.elapsed();
    assert_eq!(snap.active_worktree, real(&feature));
    assert!(snap.seq > before.seq);
    assert_eq!(snap.head.branch.as_deref(), Some("feature"));
    assert_eq!(snap.status, counts(1, 0, 0, &[]));
    assert_eq!(snap.in_progress, None, "the merge is main's");
    let actives: Vec<bool> = snap.worktrees.iter().map(|w| w.active).collect();
    assert_eq!(actives, [false, true]);
    assert_eq!(wt(&snap, "wt-feature").status, Some(snap.status.clone()));
    // The worktree left keeps its state, as one of the others.
    let main = &snap.worktrees[0];
    assert_eq!(main.in_progress, Some(InProgressKind::Merge));
    assert_eq!(main.status, Some(before.status.clone()));
    // The uncommitted row is the feature worktree's, on its HEAD.
    let rows = h.m.rows(id, snap.generation, 0, 2).await.unwrap().rows;
    assert_eq!(rows[0].kind, RowKind::Uncommitted);
    assert_eq!(rows[0].subject, "Uncommitted changes (1)");
    assert_eq!(rows[0].parents, [git_in(&feature, &["rev-parse", "HEAD"])]);
    eprintln!("switching took {took:?}");

    // The next operation runs there.
    let out = h
        .run(
            id,
            gg_core::dto::OpRequest::StashSave {
                message: None,
                include_untracked: false,
                expect_worktree: here_wt(),
            },
        )
        .await;
    assert_eq!(out.status, OpStatus::Done, "{out:?}");
    assert!(
        op_records(&h.log, &out)
            .iter()
            .all(|r| Path::new(&r.cwd) == Path::new(&real(&feature)))
    );
    assert_eq!(git_in(&feature, &["status", "--porcelain"]), "");
    assert!(in_git_dir(&repo, "MERGE_HEAD"), "main untouched");

    // And back: main's merge again.
    let back = h.m.set_active_worktree(id, repo.path()).await.unwrap();
    assert_eq!(back.active_worktree, real(repo.path()));
    assert_eq!(
        back.in_progress.as_ref().map(|p| p.kind),
        Some(InProgressKind::Merge)
    );
    assert_eq!(back.status.conflicted, 1);
    // The same one again changes nothing but the seq.
    let same = h.m.set_active_worktree(id, repo.path()).await.unwrap();
    assert_eq!(same.active_worktree, back.active_worktree);
    assert_eq!(same.generation, back.generation);
}

#[tokio::test]
async fn switching_is_refused_while_an_operation_runs_or_waits() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let feature = repo.add_worktree("feature");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_op_delay(Duration::from_millis(800));
    let running = tokio::spawn({
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
    let err = h.m.set_active_worktree(id, &feature).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Busy);
    // One waiting behind it too.
    let waiting = tokio::spawn({
        let (m, wt, head) = (h.m.clone(), repo.path().to_path_buf(), head.clone());
        async move {
            m.run_op(id, in_wt(&wt, create_branch("b", &head)), &[])
                .await
        }
    });
    h.sink
        .wait_for(TIMEOUT, |n, v| {
            n == "repo-changed" && v["repoId"] == id && v["op"]["queued"] == 1
        })
        .await
        .expect("the second one waits");
    let err = h.m.set_active_worktree(id, &feature).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Busy);
    for op in [running, waiting] {
        assert_eq!(op.await.unwrap().unwrap().status, OpStatus::Done);
    }
    h.m.set_op_delay(Duration::ZERO);
    // Both ran in main; now the switch goes through.
    let snap = h.m.set_active_worktree(id, &feature).await.unwrap();
    assert_eq!(snap.active_worktree, real(&feature));
}

#[tokio::test]
async fn only_a_worktree_with_its_folder_can_be_made_active() {
    let repo = TestRepo::new();
    repo.commit("one");
    let gone = repo.add_worktree("gone");
    fs::remove_dir_all(&gone).unwrap();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let err = h.m.set_active_worktree(id, &gone).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::PathMissing);
    assert!(err.message.contains("is gone"), "{}", err.message);
    let elsewhere = tempfile::tempdir().unwrap();
    let err =
        h.m.set_active_worktree(id, elsewhere.path())
            .await
            .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
    let err =
        h.m.set_active_worktree(id, &repo.path().join(".git"))
            .await
            .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
    assert_eq!(
        h.m.snapshot(id).await.unwrap().active_worktree,
        real(repo.path())
    );
}

#[tokio::test]
async fn a_request_asked_for_in_another_worktree_is_refused() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let feature = repo.add_worktree("feature");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    // Asked for while main was active (as the UI showed it), sent after the switch.
    let request = in_wt(repo.path(), create_branch("late", &head));
    h.m.set_active_worktree(id, &feature).await.unwrap();
    let mark = h.last_log_id();
    let out = h.run(id, request).await;
    assert_eq!(out.status, OpStatus::Failed);
    assert_eq!(
        out.message,
        format!(
            "That was asked for in {}, but {} is the active worktree now.",
            repo.path().display(),
            real(&feature)
        )
    );
    assert!(h.writes_since(mark).is_empty(), "nothing ran");
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "-q", "refs/heads/late"]));
    // Asked for in the active worktree (by any spelling of its path), it runs.
    let request = in_wt(&feature.join("."), create_branch("now", &head));
    assert_eq!(h.run(id, request).await.status, OpStatus::Done);
}

/// Where names ignore case (macOS by default, Windows), the active worktree's path spelled
/// in another case is the active worktree; where they do not, it is another folder.
#[tokio::test]
async fn the_active_worktree_in_another_case_is_itself_where_case_is_ignored() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let feature = repo.add_worktree("feature");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_active_worktree(id, &feature).await.unwrap();
    let name = feature
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_uppercase();
    let shouted = feature.with_file_name(&name);
    let ignores_case = shouted.exists();
    let out = h
        .run(id, in_wt(&shouted, create_branch("cased", &head)))
        .await;
    let expected = if ignores_case {
        OpStatus::Done
    } else {
        OpStatus::Failed
    };
    assert_eq!(out.status, expected, "{out:?}");
}

#[tokio::test]
async fn the_active_worktree_removed_in_a_terminal_falls_back_to_main_once() {
    let repo = TestRepo::new();
    repo.commit("one");
    let feature = repo.add_worktree("feature");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_active_worktree(id, &feature).await.unwrap();
    tokio::time::timeout(TIMEOUT, h.m.wait_background(id))
        .await
        .unwrap();
    assert!(repo_errors(&h, id).is_empty());

    repo.git(&["worktree", "remove", "--force", &feature.to_string_lossy()]);
    let snap = until(&h, id, "back in main", |s| {
        s.active_worktree == real(repo.path())
    })
    .await;
    assert_eq!(snap.head.branch.as_deref(), Some("main"));
    assert_eq!(snap.worktrees.len(), 1);
    // Everything that noticed has noticed (the watchers, the status read).
    tokio::time::sleep(Duration::from_millis(1500)).await;
    h.m.refresh(id).await.unwrap();
    let errors = repo_errors(&h, id);
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_eq!(errors[0]["error"]["code"], "path_missing");
    let message = errors[0]["error"]["message"].as_str().unwrap();
    assert!(message.starts_with("The active worktree "), "{message}");
    assert!(message.contains("is gone"), "{message}");
    // Operations run in main again.
    let head = repo.git(&["rev-parse", "HEAD"]);
    assert_eq!(
        h.run(id, create_branch("x", &head)).await.status,
        OpStatus::Done
    );
}

#[tokio::test]
async fn other_worktrees_are_read_in_the_background_four_at_a_time() {
    let repo = TestRepo::new();
    repo.commit("one");
    let worktrees: Vec<PathBuf> = (0..8)
        .map(|n| repo.add_worktree(&format!("w{n}")))
        .collect();
    for wt in &worktrees {
        fs::write(wt.join("new.txt"), "x").unwrap();
    }
    let h = harness().await;
    // Each read takes 1.5 s more: longer than the open itself, even where git is slow.
    h.m.set_worktree_read_delay(Duration::from_millis(1500));
    // Opening does not wait for them.
    let started = Instant::now();
    let snap = h.m.open(repo.path()).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
    assert!(snap.worktrees[1..].iter().all(|w| w.status.is_none()));
    let id = snap.repo_id;
    let snap = until(&h, id, "all read", |s| {
        s.worktrees[1..].iter().all(|w| w.status.is_some())
    })
    .await;
    assert!(
        snap.worktrees[1..]
            .iter()
            .all(|w| w.status == Some(counts(0, 1, 0, &[])))
    );
    assert_eq!(h.m.most_worktree_reads_at_once(), 4);
}

#[tokio::test]
async fn another_worktree_is_read_again_when_its_git_dir_changes_or_the_window_gains_focus() {
    let repo = TestRepo::new();
    repo.commit("one");
    let b = repo.add_worktree("b");
    let c = repo.add_worktree("c");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    until(&h, id, "read", |s| {
        s.worktrees.iter().all(|w| w.status.is_some())
    })
    .await;

    // Staged in b (its index, in its git dir, changes): b is read again, c is not.
    let mark = h.last_log_id();
    fs::write(b.join("staged.txt"), "x").unwrap();
    git_in(&b, &["add", "staged.txt"]);
    until(&h, id, "b read again", |s| {
        wt(s, "wt-b").status == Some(counts(1, 0, 0, &[]))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        status_reads(&h, mark, Some(&c)).is_empty(),
        "c was not read"
    );

    // An edit (no git dir change) shows when the window gains focus.
    fs::write(c.join("edit.txt"), "x").unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        wt(&h.m.snapshot(id).await.unwrap(), "wt-c").status,
        Some(WorktreeStatus::default())
    );
    h.m.set_window_focused(true);
    until(&h, id, "c read on focus", |s| {
        wt(s, "wt-c").status == Some(counts(0, 1, 0, &[]))
    })
    .await;
}

#[tokio::test]
async fn editing_a_file_reads_the_status_without_a_new_generation() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.commit_all("two files");
    repo.write("a.txt", "edited\n");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let id = snap.repo_id;
    assert_eq!(snap.status.changed, 1);
    let mark = h.last_log_id();

    let started = Instant::now();
    repo.write("b.txt", "edited too\n");
    let now = until(&h, id, "the edit shows", |s| s.status.changed == 2).await;
    let took = started.elapsed();
    assert!(took < Duration::from_secs(2), "took {took:?}");
    assert_eq!(now.generation, snap.generation, "no row moved");
    assert!(now.seq > snap.seq);
    let reloads: Vec<_> = h
        .log
        .since(mark)
        .into_iter()
        .filter(|r| r.args.first().map(String::as_str) == Some("log"))
        .collect();
    assert!(
        reloads.is_empty(),
        "the history was not read again: {reloads:?}"
    );
    // Rows read now carry the new count.
    let row = &h.m.rows(id, now.generation, 0, 1).await.unwrap().rows[0];
    assert_eq!(row.subject, "Uncommitted changes (2)");
}

#[tokio::test]
async fn writing_into_ignored_folders_reads_no_status() {
    let repo = TestRepo::new();
    repo.write(".gitignore", "build/\ndist/\n");
    repo.commit_all("ignore the build");
    repo.write("build/old.o", "x");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    // Let the open's own events pass.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let mark = h.last_log_id();

    // 10,000 files in a folder ignored from the start.
    for d in 0..100 {
        let dir = repo.path().join(format!("build/obj/{d}"));
        fs::create_dir_all(&dir).unwrap();
        for f in 0..100 {
            fs::write(dir.join(format!("{f}.o")), "x").unwrap();
        }
    }
    // A folder that appears now, ignored by a pattern.
    for d in 0..5 {
        let dir = repo.path().join(format!("dist/{d}"));
        fs::create_dir_all(&dir).unwrap();
        for f in 0..20 {
            fs::write(dir.join(format!("{f}.js")), "x").unwrap();
        }
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    let reads = status_reads(&h, mark, None);
    assert!(reads.is_empty(), "no status read: {reads:?}");
    assert_eq!(
        h.m.snapshot(id).await.unwrap().status,
        WorktreeStatus::default()
    );

    // A file that is not ignored still is seen.
    repo.write("src.txt", "x");
    until(&h, id, "the new file shows", |s| s.status.untracked == 1).await;
}

/// Measurement (run by hand, `--ignored --nocapture`): opening a repository with 20
/// worktrees, each with changes, against the same repository with none: the time to the
/// first snapshot and to the fully loaded log, and when the other worktrees' state is in.
#[tokio::test]
#[ignore = "a measurement, run by hand"]
async fn measure_opening_with_twenty_worktrees() {
    let repo = TestRepo::new();
    repo.commit_many(20_000);
    let mut times = Vec::new();
    for worktrees in [0, 20] {
        for n in 0..worktrees {
            let wt = repo.add_worktree(&format!("m{worktrees}-{n}"));
            fs::write(wt.join("new.txt"), "x").unwrap();
        }
        let h = harness().await;
        let started = Instant::now();
        let snap = h.m.open(repo.path()).await.unwrap();
        let first = started.elapsed();
        h.m.wait_loaded(snap.repo_id).await;
        let loaded = started.elapsed();
        until(&h, snap.repo_id, "all read", |s| {
            s.worktrees.iter().all(|w| w.status.is_some())
        })
        .await;
        let all = started.elapsed();
        times.push((worktrees, first, loaded, all));
    }
    for (n, first, loaded, all) in times {
        eprintln!(
            "{n:>2} other worktrees: first snapshot {first:?}, log loaded {loaded:?}, every worktree's state {all:?}"
        );
    }
}

/// A view change while a switch of worktree is underway (Task 3 review, item 1): no snapshot
/// shows the new active worktree with the old one's HEAD, and the switch still lands with
/// the new worktree's status.
#[tokio::test]
async fn a_view_change_racing_a_switch_never_mixes_the_two_worktrees() {
    let repo = TestRepo::new();
    repo.commit("one");
    let feature = repo.add_worktree("feature");
    git_in(
        &feature,
        &["commit", "-q", "--allow-empty", "-m", "on feature"],
    );
    fs::write(feature.join("new.txt"), "x").unwrap();
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let id = snap.repo_id;
    // The switch waits for the new worktree's status: long enough to change the view meanwhile.
    h.m.set_status_delay(Duration::from_millis(700));
    let switching = tokio::spawn({
        let (m, feature) = (h.m.clone(), feature.clone());
        async move { m.set_active_worktree(id, &feature).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut view = snap.view_options.clone();
    view.show_tags = false;
    let changed = h.m.set_view_options(id, view).await.unwrap();
    let switched = switching.await.unwrap().unwrap();
    h.m.set_status_delay(Duration::ZERO);

    let feature_path = real(&feature);
    let check = |s: &serde_json::Value| {
        let branch = s["head"]["branch"].as_str();
        if s["activeWorktree"] == feature_path.as_str() {
            assert_eq!(branch, Some("feature"), "mixed: {s:#}");
        } else {
            assert_eq!(branch, Some("main"), "mixed: {s:#}");
        }
    };
    check(&serde_json::to_value(&changed).unwrap());
    for s in h.changes(id) {
        check(&s);
    }
    assert_eq!(switched.active_worktree, feature_path);
    assert!(!switched.view_options.show_tags, "the view change is kept");
    assert_eq!(switched.status.untracked, 1);
    assert_eq!(
        wt(&switched, "wt-feature").status,
        Some(counts(0, 1, 0, &[]))
    );
}

/// The commits on no remote of another worktree are counted again only when its HEAD or a
/// remote-tracking branch moved, and the window gaining focus reads a worktree at most once
/// every 30 s (Task 3 review, item 2).
#[tokio::test]
async fn focus_reads_other_worktrees_sparingly() {
    let repo = TestRepo::new();
    repo.commit("one");
    let b = repo.add_worktree("b");
    git_in(&b, &["commit", "-q", "--allow-empty", "-m", "on b"]);
    let b_head = git_in(&b, &["rev-parse", "HEAD"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    until(&h, id, "b read", |s| wt(s, "wt-b").unpushed == Some(2)).await;
    let mark = h.last_log_id();
    for _ in 0..2 {
        h.m.set_window_focused(false);
        h.m.set_window_focused(true);
        tokio::time::sleep(Duration::from_millis(300)).await;
        tokio::time::timeout(TIMEOUT, h.m.wait_background(id))
            .await
            .unwrap();
    }
    let counts_of_b = |since: u64| {
        h.log
            .since(since)
            .into_iter()
            .filter(|r| r.args.first().map(String::as_str) == Some("rev-list"))
            .filter(|r| r.args.contains(&b_head))
            .count()
    };
    assert_eq!(counts_of_b(0), 1, "counted once, when opened");
    assert_eq!(status_reads(&h, mark, Some(&b)).len(), 1, "one focus read");
    // Its HEAD moves: counted again.
    git_in(&b, &["commit", "-q", "--allow-empty", "-m", "again"]);
    until(&h, id, "b counted again", |s| {
        wt(s, "wt-b").unpushed == Some(3)
    })
    .await;
}

/// The active worktree removed while an operation runs: at the operation's end the main
/// worktree is active, and its files are the ones watched (Task 3 review, item 3).
#[tokio::test]
async fn the_fallback_at_an_operations_end_watches_main() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let feature = repo.add_worktree("feature");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_active_worktree(id, &feature).await.unwrap();
    tokio::time::timeout(TIMEOUT, h.m.wait_background(id))
        .await
        .unwrap();
    h.m.set_op_delay(Duration::from_millis(800));
    let running = tokio::spawn({
        let (m, wt, head) = (h.m.clone(), feature.clone(), head.clone());
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
    // Its git commands have run (the delay comes after them).
    while !repo.git_succeeds(&["rev-parse", "--verify", "-q", "refs/heads/a"]) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    repo.git(&["worktree", "remove", "--force", &feature.to_string_lossy()]);
    let out = running.await.unwrap();
    h.m.set_op_delay(Duration::ZERO);
    let out = out.unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(out.snapshot.active_worktree, real(repo.path()));
    tokio::time::timeout(TIMEOUT, h.m.wait_background(id))
        .await
        .unwrap();
    // An edit in main shows without anything else asking for it: main is watched.
    let started = Instant::now();
    repo.write("main-edit.txt", "x");
    until(&h, id, "the edit in main shows", |s| {
        s.status.untracked == 1
    })
    .await;
    assert!(started.elapsed() < Duration::from_secs(4));
}

/// A bare main repository: it cannot be made active, and when the active worktree goes,
/// the first linked worktree with its folder stands in for the main one; with none left,
/// that is said once (Task 3 review, item 5).
#[tokio::test]
async fn a_bare_main_repository_is_never_made_active() {
    let source = TestRepo::new();
    source.commit("one");
    source.branch("other");
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("bare.git");
    git_in(
        dir.path(),
        &[
            "clone",
            "-q",
            "--bare",
            &source.path().to_string_lossy(),
            "bare.git",
        ],
    );
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    git_in(
        &bare,
        &["worktree", "add", "-q", &a.to_string_lossy(), "main"],
    );
    git_in(
        &bare,
        &["worktree", "add", "-q", &b.to_string_lossy(), "other"],
    );
    let (a_real, b_real) = (real(&a), real(&b));
    let h = harness().await;
    let snap = h.open(&a).await;
    let id = snap.repo_id;
    assert_eq!(snap.active_worktree, a_real);
    let err = h.m.set_active_worktree(id, &bare).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
    assert!(err.message.contains("bare"), "{}", err.message);

    git_in(
        &bare,
        &["worktree", "remove", "--force", &a.to_string_lossy()],
    );
    let snap = until(&h, id, "b stands in", |s| s.active_worktree == b_real).await;
    assert_eq!(snap.head.branch.as_deref(), Some("other"));
    let errors = repo_errors(&h, id);
    assert_eq!(errors.len(), 1, "{errors:#?}");
    let message = errors[0]["error"]["message"].as_str().unwrap();
    assert!(message.contains("works in the worktree"), "{message}");

    git_in(
        &bare,
        &["worktree", "remove", "--force", &b.to_string_lossy()],
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let _ = h.m.refresh(id).await;
    let _ = h.m.refresh(id).await;
    let errors = repo_errors(&h, id);
    let none_left: Vec<_> = errors
        .iter()
        .filter(|e| e["error"]["code"] == "bare_repo")
        .collect();
    assert_eq!(none_left.len(), 1, "said once: {errors:#?}");
    assert_eq!(
        h.m.snapshot(id).await.unwrap().active_worktree,
        b_real,
        "never the bare repository"
    );
}

/// The active worktree's folder replaced by a plain folder, outside the app: a request
/// asked for there is refused before anything is read there (Task 3 review, item 6).
#[tokio::test]
async fn a_request_for_a_folder_that_is_no_longer_a_worktree_is_refused() {
    let repo = TestRepo::new();
    let head = repo.commit("one");
    let feature = repo.add_worktree("feature");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_active_worktree(id, &feature).await.unwrap();
    tokio::time::timeout(TIMEOUT, h.m.wait_background(id))
        .await
        .unwrap();
    h.m.set_op_delay(Duration::from_millis(600));
    let first = tokio::spawn({
        let (m, wt, head) = (h.m.clone(), feature.clone(), head.clone());
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
    while !repo.git_succeeds(&["rev-parse", "--verify", "-q", "refs/heads/a"]) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let second = tokio::spawn({
        let (m, wt, head) = (h.m.clone(), feature.clone(), head.clone());
        async move {
            m.run_op(id, in_wt(&wt, create_branch("b", &head)), &[])
                .await
        }
    });
    fs::remove_dir_all(&feature).unwrap();
    fs::create_dir(&feature).unwrap();
    first.await.unwrap().unwrap();
    h.m.set_op_delay(Duration::ZERO);
    let out = second.await.unwrap().unwrap();
    assert_eq!(out.status, OpStatus::Failed, "{out:?}");
    assert!(
        out.message
            == format!(
                "That was asked for in {}, but {} is no longer a worktree.",
                feature.display(),
                real(&feature)
            )
            || out.message.contains("is the active worktree now"),
        "{}",
        out.message
    );
    // Nothing but the check ran for it.
    let ran: Vec<_> = op_records(&h.log, &out)
        .into_iter()
        .map(|r| r.args.join(" "))
        .collect();
    assert!(
        ran.iter().all(|a| a == "rev-parse --show-toplevel"),
        "{ran:?}"
    );
    assert!(!repo.git_succeeds(&["rev-parse", "--verify", "-q", "refs/heads/b"]));
}

/// Two switches in quick succession: the last one is what is remembered, however the
/// writes are scheduled (Task 3 review, item 7).
#[tokio::test]
async fn the_last_of_two_quick_switches_is_remembered() {
    use std::sync::{Arc, Mutex};
    struct Slow {
        slow: PathBuf,
        stored: Mutex<Option<PathBuf>>,
    }
    impl gg_core::repo::WorktreeMemory for Slow {
        fn remembered(&self, _root: &Path) -> Option<PathBuf> {
            None
        }
        fn remember(&self, _root: &Path, worktree: Option<&Path>) {
            if worktree == Some(self.slow.as_path()) {
                std::thread::sleep(Duration::from_millis(400));
            }
            *self.stored.lock().unwrap() = worktree.map(Path::to_path_buf);
        }
    }
    let repo = TestRepo::new();
    repo.commit("one");
    let a = repo.add_worktree("a");
    let b = repo.add_worktree("b");
    let memory = Arc::new(Slow {
        slow: PathBuf::from(real(&a)),
        stored: Mutex::new(None),
    });
    let h = harness().await;
    h.m.set_worktree_memory(memory.clone());
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_active_worktree(id, &a).await.unwrap();
    h.m.set_active_worktree(id, &b).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(
        memory.stored.lock().unwrap().as_deref(),
        Some(Path::new(&real(&b)))
    );
}

/// More changes at once than a batch keeps by name (5,000 files in a folder that appears
/// now, ignored by a pattern): judged by their folders against what git ignores, read again
/// first, they read no status (Task 3 review, item 4).
#[tokio::test]
async fn thousands_of_files_in_a_new_ignored_folder_read_no_status() {
    let repo = TestRepo::new();
    repo.write(".gitignore", "gen/\n");
    repo.commit_all("ignore generated files");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let mark = h.last_log_id();
    for d in 0..10 {
        let dir = repo.path().join(format!("gen/{d}"));
        fs::create_dir_all(&dir).unwrap();
        for f in 0..500 {
            fs::write(dir.join(format!("{f}.out")), "x").unwrap();
        }
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    let reads = status_reads(&h, mark, None);
    // On failure, every git command since, to tell what asked for the read.
    let since: Vec<String> = h
        .log
        .since(mark)
        .iter()
        .map(|r| format!("{} {:?} (+{} ms)", r.id, r.args, r.started_at_ms))
        .collect();
    assert!(
        reads.is_empty(),
        "no status read: {} reads\n{}",
        reads.len(),
        since.join("\n")
    );
    repo.write("seen.txt", "x");
    until(&h, id, "a file that is not ignored shows", |s| {
        s.status.untracked == 1
    })
    .await;
}
