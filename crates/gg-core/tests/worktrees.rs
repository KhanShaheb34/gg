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
    h.m.set_worktree_read_delay(Duration::from_millis(400));
    // Opening does not wait for them.
    let started = Instant::now();
    let snap = h.m.open(repo.path()).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(400),
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
