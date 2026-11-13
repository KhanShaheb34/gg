//! Integration tests for `RepoManager`: opening repositories, streaming the log into rows,
//! labels, search, view options and refresh, against real temp repositories.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gg_core::dto::{CommitRow, FetchState, RefLabel, RepoSnapshot, RowKind, ViewOptions};
use gg_core::error::ErrorCode;
use gg_core::git::refs::RefKind;
use gg_core::git::runner::{CmdKind, CommandLog, GitRunner};
use gg_core::repo::RepoManager;
use gg_core::testutil::{RecordingSink, TestRepo};
use serde_json::Value;

/// Generous bound for anything asynchronous; tests never sleep for a fixed time.
const TIMEOUT: Duration = Duration::from_secs(20);

struct Harness {
    m: Arc<RepoManager>,
    sink: Arc<RecordingSink>,
    log: Arc<CommandLog>,
}

async fn harness() -> Harness {
    let log = CommandLog::new();
    let runner = GitRunner::detect(log.clone())
        .await
        .expect("git is installed");
    let sink = Arc::new(RecordingSink::default());
    let m = RepoManager::new(runner, sink.clone());
    Harness { m, sink, log }
}

impl Harness {
    async fn wait(&self, id: u32) {
        tokio::time::timeout(TIMEOUT, self.m.wait_loaded(id))
            .await
            .expect("load finishes");
    }

    /// Opens `path`, waits for the log to load and the status to be read, and returns the
    /// snapshot.
    async fn open(&self, path: &Path) -> RepoSnapshot {
        let snap = self.m.open(path).await.expect("open");
        self.wait(snap.repo_id).await;
        tokio::time::timeout(TIMEOUT, self.m.wait_status(snap.repo_id))
            .await
            .expect("status read");
        self.m.snapshot(snap.repo_id).await.unwrap()
    }

    async fn refresh(&self, id: u32) -> RepoSnapshot {
        self.m.refresh(id).await.expect("refresh");
        self.wait(id).await;
        tokio::time::timeout(TIMEOUT, self.m.wait_status(id))
            .await
            .expect("status read");
        self.m.snapshot(id).await.unwrap()
    }

    async fn view(&self, id: u32, o: ViewOptions) -> RepoSnapshot {
        self.m
            .set_view_options(id, o)
            .await
            .expect("set_view_options");
        self.wait(id).await;
        self.m.snapshot(id).await.unwrap()
    }

    /// Every row of the current generation.
    async fn rows(&self, id: u32) -> Vec<CommitRow> {
        let snap = self.m.snapshot(id).await.unwrap();
        let mut rows = Vec::new();
        while rows.len() < snap.total_rows as usize {
            let page = self
                .m
                .rows(id, snap.generation, rows.len() as u32, 1000)
                .await
                .unwrap();
            assert!(!page.rows.is_empty());
            rows.extend(page.rows);
        }
        rows
    }

    /// Log commands recorded after command id `since`.
    fn log_commands_since(&self, since: u64) -> usize {
        self.log
            .since(since)
            .iter()
            .filter(|r| r.args.first().map(String::as_str) == Some("log"))
            .count()
    }

    /// Waits for the `repo-changed` event that ends a load with `total_rows` rows (it can
    /// arrive just after `wait_loaded` returns).
    async fn final_event(&self, id: u32, total_rows: u32) {
        self.sink
            .wait_for(TIMEOUT, |name, v| {
                name == "repo-changed"
                    && v["repoId"] == id
                    && v["loading"] == false
                    && v["totalRows"] == total_rows
            })
            .await
            .expect("final snapshot");
    }

    fn last_command_id(&self) -> u64 {
        self.log.since(0).last().map_or(0, |r| r.id)
    }

    fn repo_changed(&self, id: u32) -> Vec<RepoSnapshot> {
        self.sink
            .events()
            .into_iter()
            .filter(|(name, v)| name == "repo-changed" && v["repoId"] == id)
            .map(|(_, v)| snapshot_from_json(v))
            .collect()
    }
}

fn snapshot_from_json(v: Value) -> RepoSnapshot {
    serde_json::from_value(v).expect("repo-changed payload is a RepoSnapshot")
}

fn canonical(p: &Path) -> String {
    fs::canonicalize(p).unwrap().to_string_lossy().into_owned()
}

fn all_refs() -> ViewOptions {
    ViewOptions {
        branches: None,
        show_remotes: true,
        show_tags: true,
        show_stashes: true,
    }
}

fn hashes(rows: &[CommitRow]) -> Vec<String> {
    rows.iter().map(|r| r.hash.clone()).collect()
}

fn git_lines(repo: &TestRepo, args: &[&str]) -> Vec<String> {
    repo.git(args).lines().map(str::to_owned).collect()
}

fn row<'a>(rows: &'a [CommitRow], hash: &str) -> &'a CommitRow {
    rows.iter()
        .find(|r| r.hash == hash)
        .unwrap_or_else(|| panic!("no row for {hash}"))
}

fn label(kind: RefKind, name: &str) -> RefLabel {
    RefLabel {
        kind,
        name: name.into(),
        remotes: vec![],
        current: false,
        worktree_path: None,
        upstream: None,
        ahead: 0,
        behind: 0,
    }
}

/// main: c1 - c2 ------- c3 - m
///              \- f1 - f2 -/      (branch `feature`, merged with --no-ff)
fn merge_repo() -> (TestRepo, Vec<String>) {
    let repo = TestRepo::new();
    let c1 = repo.commit("one");
    let c2 = repo.commit("two");
    repo.branch("feature");
    repo.checkout("feature");
    let f1 = repo.commit("feat one");
    let f2 = repo.commit("feat two");
    repo.checkout("main");
    let c3 = repo.commit("three");
    let m = repo.merge("feature");
    (repo, vec![c1, c2, f1, f2, c3, m])
}

// ---------------------------------------------------------------------------------------
// open

#[tokio::test]
async fn open_loads_all_rows() {
    let (repo, commits) = merge_repo();
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    assert_eq!(snap.total_rows as usize, commits.len());
    assert!(!snap.loading);
    assert_eq!(snap.generation, 1);

    let page =
        h.m.rows(snap.repo_id, snap.generation, 0, 100)
            .await
            .unwrap();
    assert_eq!(page.generation, snap.generation);
    assert_eq!(page.start, 0);
    let expected = git_lines(
        &repo,
        &[
            "log",
            "--date-order",
            "--branches",
            "--remotes",
            "--tags",
            "HEAD",
            "--format=%H",
        ],
    );
    assert_eq!(hashes(&page.rows), expected);
    for (i, r) in page.rows.iter().enumerate() {
        assert_eq!(r.index as usize, i);
        assert_eq!(r.kind, RowKind::Commit);
    }
    let merge = row(&page.rows, &commits[5]);
    assert!(merge.is_merge);
    assert_eq!(merge.parents, [commits[4].clone(), commits[3].clone()]);
    assert_eq!(merge.author, "Test");
    assert_eq!(merge.email, "test@example.com");
    assert!(merge.is_head);
    assert!(!row(&page.rows, &commits[0]).is_merge);
    assert!(snap.max_lane >= 1, "the feature branch needs a second lane");
}

#[tokio::test]
async fn open_subdirectory_and_worktree_same_repo() {
    let repo = TestRepo::new();
    repo.write("sub/x.txt", "x");
    repo.commit_all("with sub");
    let wt = repo.add_worktree("wt-a");
    let root = canonical(repo.path());

    let h = harness().await;
    let from_sub = h.open(&repo.path().join("sub")).await;
    assert_eq!(from_sub.root_path, root);
    assert_eq!(from_sub.active_worktree, root);
    assert_eq!(from_sub.name, "repo");
    let from_wt = h.m.open(&wt).await.unwrap();
    assert_eq!(from_wt.repo_id, from_sub.repo_id);
    assert_eq!(h.m.open_paths(), [(from_sub.repo_id, PathBuf::from(&root))]);
    let (git_dir, git_root) = h.m.git_dirs(from_sub.repo_id).unwrap();
    assert_eq!(git_dir, PathBuf::from(canonical(&repo.path().join(".git"))));
    assert_eq!(git_root, PathBuf::from(&root));

    // A manager that opens the linked worktree first works in that worktree.
    let other = harness().await;
    let snap = other.open(&wt).await;
    assert_eq!(snap.active_worktree, canonical(&wt));
    assert_eq!(snap.root_path, root);
    assert_eq!(snap.name, "repo");
    assert_eq!(snap.head.branch.as_deref(), Some("wt-a"));
    // Paths are comparable with each other.
    assert_eq!(snap.worktrees[0].path, snap.root_path);
    assert_eq!(snap.worktrees[1].path, snap.active_worktree);
}

#[tokio::test]
async fn open_errors() {
    let h = harness().await;
    let dir = tempfile::tempdir().unwrap();

    let err = h.m.open(&dir.path().join("missing")).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::PathMissing);

    let err = h.m.open(dir.path()).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotARepo);
    assert_eq!(err.message, "This folder is not a git repository.");

    let file = dir.path().join("file.txt");
    fs::write(&file, "x").unwrap();
    assert_eq!(h.m.open(&file).await.unwrap_err().code, ErrorCode::NotARepo);

    let bare = dir.path().join("bare.git");
    fs::create_dir(&bare).unwrap();
    let status = std::process::Command::new("git")
        .args(["init", "-q", "--bare"])
        .current_dir(&bare)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(h.m.open(&bare).await.unwrap_err().code, ErrorCode::BareRepo);
    assert!(h.m.open_paths().is_empty());
}

#[tokio::test]
async fn empty_repo() {
    let repo = TestRepo::new();
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    assert_eq!(snap.total_rows, 0);
    assert!(!snap.loading);
    assert_eq!(snap.head.branch.as_deref(), Some("main"));
    assert_eq!(snap.head.hash, None);
    let page =
        h.m.rows(snap.repo_id, snap.generation, 0, 100)
            .await
            .unwrap();
    assert!(page.rows.is_empty());
    assert!(
        h.sink.events().iter().all(|(name, _)| name != "repo-error"),
        "{:?}",
        h.sink.events()
    );
}

#[tokio::test]
async fn snapshot_lists_refs_sorted_with_idle_fetch_state() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("zeta");
    repo.branch("alpha");
    let _origin = repo.clone_as_remote();
    repo.git(&["tag", "v1"]);
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    assert_eq!(snap.branches, ["alpha", "main", "zeta"]);
    assert_eq!(
        snap.remote_branches,
        ["origin/alpha", "origin/main", "origin/zeta"]
    );
    assert_eq!(snap.remotes, ["origin"]);
    assert_eq!(snap.status.changed + snap.status.untracked, 0);
    assert_eq!(snap.view_options, all_refs());
    assert_eq!(
        snap.fetch,
        FetchState {
            running: false,
            last_error: None,
            last_error_detail: None,
            last_at_ms: None,
        }
    );
    assert_eq!(snap.max_lane, 0);
    // The JSON shape crosses to the UI unchanged.
    let v = serde_json::to_value(&snap).unwrap();
    assert_eq!(
        v["fetch"],
        serde_json::json!({"running": false, "lastError": null, "lastErrorDetail": null, "lastAtMs": null})
    );
    assert_eq!(v["head"]["branch"], "main");
}

// ---------------------------------------------------------------------------------------
// labels

#[tokio::test]
async fn labels_combine_local_and_remote() {
    let repo = TestRepo::new();
    repo.commit("one");
    let c2 = repo.commit("two");
    let _origin = repo.clone_as_remote();
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let root = canonical(repo.path());

    let rows = h.rows(snap.repo_id).await;
    assert_eq!(
        row(&rows, &c2).labels,
        [RefLabel {
            remotes: vec!["origin".into()],
            current: true,
            worktree_path: Some(root.clone()),
            ..label(RefKind::Branch, "main")
        }]
    );

    let c3 = repo.commit("three");
    let snap = h.refresh(snap.repo_id).await;
    assert_eq!(snap.total_rows, 3);
    let rows = h.rows(snap.repo_id).await;
    assert_eq!(
        row(&rows, &c2).labels,
        [label(RefKind::Remote, "origin/main")]
    );
    assert_eq!(
        row(&rows, &c3).labels,
        [RefLabel {
            current: true,
            worktree_path: Some(root),
            ..label(RefKind::Branch, "main")
        }]
    );
}

#[tokio::test]
async fn labels_order_branches_remotes_tags() {
    let repo = TestRepo::new();
    let c1 = repo.commit("one");
    repo.branch("b-side");
    repo.branch("a-side");
    let _origin = repo.clone_as_remote();
    // A remote-tracking branch with no local branch of the same name.
    repo.git(&["update-ref", "refs/remotes/origin/only-remote", &c1]);
    repo.git(&["tag", "v2"]);
    repo.git(&["tag", "-a", "-m", "annotated", "v1"]);
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let rows = h.rows(snap.repo_id).await;
    let names: Vec<(RefKind, &str)> = row(&rows, &c1)
        .labels
        .iter()
        .map(|l| (l.kind, l.name.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            (RefKind::Branch, "main"),
            (RefKind::Branch, "a-side"),
            (RefKind::Branch, "b-side"),
            (RefKind::Remote, "origin/only-remote"),
            (RefKind::Tag, "v1"),
            (RefKind::Tag, "v2"),
        ]
    );
}

#[tokio::test]
async fn labels_respect_view_options() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.git(&["tag", "v1"]);
    repo.commit("two");
    let _origin = repo.clone_as_remote();
    repo.commit("three"); // origin/main is now a separate label
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let id = snap.repo_id;
    let all: Vec<RefLabel> = h
        .rows(id)
        .await
        .into_iter()
        .flat_map(|r| r.labels)
        .collect();
    assert!(all.iter().any(|l| l.kind == RefKind::Remote));
    assert!(all.iter().any(|l| l.kind == RefKind::Tag));

    h.view(
        id,
        ViewOptions {
            show_remotes: false,
            ..all_refs()
        },
    )
    .await;
    let labels: Vec<RefLabel> = h
        .rows(id)
        .await
        .into_iter()
        .flat_map(|r| r.labels)
        .collect();
    assert!(
        labels.iter().all(|l| l.kind != RefKind::Remote),
        "{labels:?}"
    );
    assert!(labels.iter().all(|l| l.remotes.is_empty()), "{labels:?}");
    assert!(labels.iter().any(|l| l.kind == RefKind::Tag));

    let snap = h
        .view(
            id,
            ViewOptions {
                show_tags: false,
                show_stashes: true,
                ..all_refs()
            },
        )
        .await;
    assert!(!snap.view_options.show_tags);
    let labels: Vec<RefLabel> = h
        .rows(id)
        .await
        .into_iter()
        .flat_map(|r| r.labels)
        .collect();
    assert!(labels.iter().all(|l| l.kind != RefKind::Tag), "{labels:?}");
    assert!(labels.iter().any(|l| l.kind == RefKind::Remote));
}

#[tokio::test]
async fn worktree_marks() {
    let repo = TestRepo::new();
    let c1 = repo.commit("one");
    let c2 = repo.commit("two");
    let wt = repo.add_worktree("wt-a");
    let detached = repo.dir.path().join("detached wt");
    repo.git(&[
        "worktree",
        "add",
        "-q",
        "--detach",
        detached.to_str().unwrap(),
        &c1,
    ]);
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    assert_eq!(snap.worktrees.len(), 3);
    assert_eq!(snap.worktrees[0].path, snap.root_path);
    let rows = h.rows(snap.repo_id).await;

    let labels = &row(&rows, &c2).labels;
    assert_eq!(labels.len(), 2, "{labels:?}");
    assert_eq!(labels[0].name, "main");
    assert!(labels[0].current);
    assert_eq!(
        labels[0].worktree_path.as_deref(),
        Some(snap.root_path.as_str())
    );
    assert_eq!(labels[1].name, "wt-a");
    assert!(!labels[1].current);
    assert_eq!(labels[1].worktree_path, Some(canonical(&wt)));
    assert!(row(&rows, &c2).detached_worktrees.is_empty());

    assert_eq!(row(&rows, &c1).detached_worktrees, [canonical(&detached)]);
}

#[tokio::test]
async fn detached_head() {
    let repo = TestRepo::new();
    repo.commit("one");
    let c2 = repo.commit("two");
    repo.commit("three");
    repo.git(&["checkout", "-q", "--detach", "HEAD~1"]);
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    assert_eq!(snap.head.branch, None);
    assert_eq!(snap.head.hash.as_deref(), Some(c2.as_str()));
    let rows = h.rows(snap.repo_id).await;
    assert_eq!(rows.len(), 3);
    for r in &rows {
        assert_eq!(r.is_head, r.hash == c2, "{}", r.hash);
    }
    // The main worktree is the one detached at c2.
    assert_eq!(row(&rows, &c2).detached_worktrees, [snap.root_path]);
}

#[tokio::test]
async fn shallow_clone_loads() {
    let src = TestRepo::new();
    for i in 0..5 {
        src.commit(&format!("c{i}"));
    }
    let dest = src.dir.path().join("shallow");
    let url = format!("file://{}", src.path().display());
    src.git(&["clone", "-q", "--depth", "2", &url, dest.to_str().unwrap()]);

    let h = harness().await;
    let snap = h.open(&dest).await;
    assert_eq!(snap.total_rows, 2);
    assert!(!snap.loading);
    let rows = h.rows(snap.repo_id).await;
    assert_eq!(rows.len(), 2);
    // git lists the shallow boundary commit without its (missing) parent.
    assert!(rows[1].parents.is_empty(), "{:?}", rows[1].parents);
    assert!(
        h.sink.events().iter().all(|(name, _)| name != "repo-error"),
        "{:?}",
        h.sink.events()
    );
}

// ---------------------------------------------------------------------------------------
// the uncommitted row

#[tokio::test]
async fn uncommitted_row() {
    let repo = TestRepo::new();
    repo.commit("one");
    let c2 = repo.commit("two");
    repo.write("file-0.txt", "changed\n");
    repo.write("new.txt", "untracked\n");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    assert_eq!(snap.total_rows, 3);
    assert_eq!(snap.status.changed, 1);
    assert_eq!(snap.status.untracked, 1);

    let rows = h.rows(snap.repo_id).await;
    let dirty = &rows[0];
    assert_eq!(dirty.kind, RowKind::Uncommitted);
    assert_eq!(dirty.hash, "UNCOMMITTED");
    assert_eq!(dirty.subject, "Uncommitted changes (2)");
    assert_eq!(dirty.parents, std::slice::from_ref(&c2));
    assert_eq!(dirty.index, 0);
    assert!(!dirty.is_head);
    assert!(dirty.labels.is_empty());
    assert_eq!(rows[1].hash, c2);
    assert_eq!(rows[1].index, 1);
    assert!(rows[1].is_head);
    // The HEAD commit continues the uncommitted row's lane.
    assert_eq!(rows[1].lane, dirty.lane);
    assert_eq!(serde_json::to_value(dirty).unwrap()["kind"], "uncommitted");
    assert_eq!(h.m.find_row(snap.repo_id, &c2).await.unwrap(), Some(1));
}

#[tokio::test]
async fn uncommitted_row_disappears_without_reload() {
    let repo = TestRepo::new();
    repo.commit("one");
    let c2 = repo.commit("two");
    repo.write("new.txt", "untracked\n");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    assert_eq!(snap.total_rows, 3);

    fs::remove_file(repo.path().join("new.txt")).unwrap();
    let before = h.last_command_id();
    // Status is read after the refresh returns; wait for it.
    let after = h.refresh(snap.repo_id).await;
    assert_eq!(h.log_commands_since(before), 0, "commits are reused");
    assert!(after.generation > snap.generation, "row indices changed");
    assert_eq!(after.total_rows, 2);
    let rows = h.rows(snap.repo_id).await;
    assert_eq!(rows[0].hash, c2);
    assert_eq!(rows[0].kind, RowKind::Commit);
}

// ---------------------------------------------------------------------------------------
// generations and refresh

#[tokio::test]
async fn stale_generation() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let old = snap.generation;
    repo.commit("two");
    let fresh = h.refresh(snap.repo_id).await;
    let err = h.m.rows(snap.repo_id, old, 0, 10).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::StaleGeneration);
    assert!(fresh.generation > old);
    assert!(
        h.m.rows(snap.repo_id, fresh.generation, 0, 10)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn refresh_without_history_change_keeps_commits() {
    let repo = TestRepo::new();
    repo.commit("one");
    let c2 = repo.commit("two");
    repo.commit("three");
    let h = harness().await;
    let snap = h.open(repo.path()).await;

    repo.branch("side");
    repo.git(&["branch", "-f", "side", &c2]);
    let before = h.last_command_id();
    let after = h.m.refresh(snap.repo_id).await.unwrap();
    assert_eq!(h.log_commands_since(before), 0, "no log reload");
    assert!(!after.loading);
    assert!(after.generation > snap.generation, "labels changed");
    assert_eq!(after.total_rows, 3);
    assert!(after.branches.contains(&"side".to_owned()));
    let rows = h.rows(snap.repo_id).await;
    assert_eq!(row(&rows, &c2).labels, [label(RefKind::Branch, "side")]);
    let changed = h.repo_changed(snap.repo_id);
    assert_eq!(changed.last().unwrap().generation, after.generation);
}

#[tokio::test]
async fn refresh_with_nothing_changed_keeps_generation() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let events = h.sink.events().len();
    let before = h.last_command_id();
    let after = h.m.refresh(snap.repo_id).await.unwrap();
    assert_eq!(after.generation, snap.generation);
    assert_eq!(h.log_commands_since(before), 0);
    assert_eq!(h.sink.events().len(), events, "nothing to report");
}

#[tokio::test]
async fn refresh_with_new_commit_reloads() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let c2 = repo.commit("two");
    let before = h.last_command_id();
    let after = h.refresh(snap.repo_id).await;
    assert_eq!(h.log_commands_since(before), 1);
    assert_eq!(after.total_rows, 2);
    assert_eq!(after.head.hash.as_deref(), Some(c2.as_str()));
    let rows = h.rows(snap.repo_id).await;
    assert_eq!(rows[0].hash, c2);
    assert!(rows[0].is_head);
}

#[tokio::test]
async fn refresh_after_branch_deleted_reloads() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("side");
    repo.checkout("side");
    let s = repo.commit("side only");
    repo.checkout("main");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    assert_eq!(snap.total_rows, 2);
    repo.git(&["branch", "-D", "side"]);
    let after = h.refresh(snap.repo_id).await;
    assert_eq!(after.total_rows, 1);
    assert_eq!(h.m.find_row(snap.repo_id, &s).await.unwrap(), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_refreshes_coalesce() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let id = snap.repo_id;
    let before = h.last_command_id();
    let (a, b, c, d, e) = tokio::join!(
        h.m.refresh(id),
        h.m.refresh(id),
        h.m.refresh(id),
        h.m.refresh(id),
        h.m.refresh(id)
    );
    for r in [a, b, c, d, e] {
        r.unwrap();
    }
    let status_runs = h
        .log
        .since(before)
        .iter()
        .filter(|r| r.args.first().map(String::as_str) == Some("status"))
        .count();
    assert!(
        (1..=2).contains(&status_runs),
        "5 overlapping refreshes ran {status_runs} times"
    );
}

// ---------------------------------------------------------------------------------------
// view options

/// main: c1 - c2 - c3;  feature: c1 - f1 - f2
fn diverged_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.branch("feature");
    repo.checkout("feature");
    repo.commit("feat one");
    repo.commit("feat two");
    repo.checkout("main");
    repo.commit("two");
    repo.commit("three");
    repo
}

#[tokio::test]
async fn view_options_filter() {
    let repo = diverged_repo();
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    assert_eq!(snap.total_rows, 5);
    let snap = h
        .view(
            snap.repo_id,
            ViewOptions {
                branches: Some(vec!["feature".into()]),
                ..all_refs()
            },
        )
        .await;
    assert_eq!(snap.view_options.branches, Some(vec!["feature".into()]));
    let rows = h.rows(snap.repo_id).await;
    assert_eq!(
        hashes(&rows),
        git_lines(
            &repo,
            &["log", "--date-order", "refs/heads/feature", "--format=%H"]
        )
    );
}

#[tokio::test]
async fn view_options_normalize_branch_selection() {
    let repo = diverged_repo();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let select = |names: &[&str]| ViewOptions {
        branches: Some(names.iter().map(|n| (*n).to_owned()).collect()),
        ..all_refs()
    };

    let snap = h.view(id, select(&[])).await;
    assert_eq!(
        snap.view_options.branches, None,
        "empty selection shows all"
    );
    assert_eq!(snap.total_rows, 5);

    let snap = h.view(id, select(&["feature", "gone"])).await;
    assert_eq!(snap.view_options.branches, Some(vec!["feature".into()]));
    assert_eq!(snap.total_rows, 3);

    let snap = h.view(id, select(&["gone"])).await;
    assert_eq!(snap.view_options.branches, None);
    assert_eq!(snap.total_rows, 5);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_is_superseded_by_a_newer_one() {
    let repo = TestRepo::new();
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    let f = repo.commit("feature only");
    repo.checkout("main");
    repo.commit_many(1500);
    let h = harness().await;
    let snap = h.m.open(repo.path()).await.unwrap();
    // Replace the initial load before it can finish.
    h.m.set_view_options(
        snap.repo_id,
        ViewOptions {
            branches: Some(vec!["feature".into()]),
            ..all_refs()
        },
    )
    .await
    .unwrap();
    h.wait(snap.repo_id).await;
    let rows = h.rows(snap.repo_id).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].hash, f);
    let id = snap.repo_id;
    h.final_event(id, 2).await;

    // However far the initial load got, it stopped publishing once superseded: after the
    // first event with the new view, rows are either the frozen old ones or the new ones.
    let events = h.repo_changed(id);
    let feature = Some(vec!["feature".to_owned()]);
    let k = events
        .iter()
        .position(|e| e.view_options.branches == feature)
        .expect("an event with the new view");
    let frozen = events[k].total_rows;
    for e in &events[k..] {
        assert_eq!(e.view_options.branches, feature);
        assert!(
            e.total_rows == frozen || e.total_rows <= 2,
            "{} rows after the view changed (frozen at {frozen})",
            e.total_rows
        );
    }
    let last = events.last().unwrap();
    assert!(!last.loading);
    assert_eq!(last.total_rows, 2);
}

/// A dirty worktree whose HEAD (on `main`) is not in the view shows no uncommitted row,
/// and its rows are laid out exactly as with a clean worktree.
#[tokio::test]
async fn uncommitted_row_follows_the_view() {
    let repo = diverged_repo();
    repo.write("file-0.txt", "changed\n");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let feature = ViewOptions {
        branches: Some(vec!["feature".into()]),
        ..all_refs()
    };
    let dirty = h.view(id, feature.clone()).await;
    assert_eq!(dirty.status.changed, 1);
    assert_eq!(dirty.total_rows, 3);
    assert_eq!(dirty.max_lane, 0);
    let dirty_rows = h.rows(id).await;
    assert!(dirty_rows.iter().all(|r| r.kind == RowKind::Commit));

    repo.git(&["checkout", "--", "file-0.txt"]);
    let clean = h.refresh(id).await;
    assert_eq!(clean.status.changed, 0);
    assert_eq!(clean.max_lane, 0);
    assert_eq!(h.rows(id).await, dirty_rows);

    // HEAD detached at a commit the selected branch reaches (not its tip): row 0 shows.
    let base = repo.git(&["rev-parse", "feature~2"]);
    repo.git(&["checkout", "-q", "--detach", &base]);
    repo.write("new.txt", "untracked\n");
    let snap = h.refresh(id).await;
    assert_eq!(snap.total_rows, 4);
    let rows = h.rows(id).await;
    assert_eq!(rows[0].kind, RowKind::Uncommitted);
    assert_eq!(rows[0].parents, [base]);
}

#[tokio::test]
async fn head_move_outside_the_selection_does_not_reload() {
    let repo = diverged_repo();
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let snap = h
        .view(
            id,
            ViewOptions {
                branches: Some(vec!["feature".into()]),
                ..all_refs()
            },
        )
        .await;
    assert_eq!(snap.total_rows, 3);
    // A new commit on main moves HEAD to a commit the feature view does not contain.
    let c = repo.commit("main moves on");
    let before = h.last_command_id();
    let after = h.refresh(id).await;
    assert_eq!(h.log_commands_since(before), 0, "no log reload");
    assert_eq!(after.head.hash, Some(c));
    assert_eq!(after.total_rows, 3);
}

/// 1,200 commits on main plus a `small` branch of 2 commits.
fn big_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.commit("base");
    repo.branch("small");
    repo.checkout("small");
    repo.commit("small only");
    repo.checkout("main");
    repo.commit_many(1200);
    repo
}

/// During a reload the old rows stay served until the new load has as many, and the swap
/// is announced at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_keeps_old_rows_until_caught_up() {
    let repo = big_repo();
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let id = snap.repo_id;
    let old_rows = snap.total_rows;
    assert_eq!(old_rows, 1202);
    h.final_event(id, old_rows).await;
    let seen = h.repo_changed(id).len();

    repo.commit("one more");
    let after = h.refresh(id).await;
    assert_eq!(after.total_rows, old_rows + 1);
    h.final_event(id, old_rows + 1).await;
    let events = h.repo_changed(id).split_off(seen);
    assert!(!events.is_empty());
    for e in &events {
        assert!(
            e.total_rows >= old_rows,
            "the reload showed {} rows (generation {}); the old view had {old_rows}",
            e.total_rows,
            e.generation
        );
    }
    // The swap itself was announced while the load was still running.
    assert!(
        events
            .iter()
            .any(|e| e.generation == after.generation && e.loading),
        "no event for the swap: {:?}",
        events
            .iter()
            .map(|e| (e.generation, e.total_rows, e.loading))
            .collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_to_a_smaller_view_completes() {
    let repo = big_repo();
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let id = snap.repo_id;
    h.final_event(id, 1202).await;
    let small = ViewOptions {
        branches: Some(vec!["small".into()]),
        ..all_refs()
    };
    let returned = h.m.set_view_options(id, small.clone()).await.unwrap();
    assert_eq!(
        returned.view_options, small,
        "the new view, even while loading"
    );
    h.wait(id).await;
    h.final_event(id, 2).await;
    let snap = h.m.snapshot(id).await.unwrap();
    assert_eq!(snap.total_rows, 2);
    assert!(snap.generation > 1);
    assert_eq!(
        hashes(&h.rows(id).await),
        git_lines(
            &repo,
            &["log", "--date-order", "refs/heads/small", "--format=%H"]
        )
    );
}

// ---------------------------------------------------------------------------------------
// status, read beside the rows

/// Rows do not wait for `git status` (slow on a big working tree): open returns, and the log
/// loads, before status is known; the uncommitted row comes in later, in a new generation.
#[tokio::test]
async fn open_and_rows_do_not_wait_for_status() {
    let repo = TestRepo::new();
    for i in 0..5 {
        repo.commit(&format!("c{i}"));
    }
    repo.write("untracked.txt", "x");
    let h = harness().await;
    h.m.set_status_delay(Duration::from_millis(1500));
    let started = std::time::Instant::now();
    let snap = h.m.open(repo.path()).await.unwrap();
    let id = snap.repo_id;
    h.wait(id).await;
    let loaded = h.m.snapshot(id).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "open and load waited for status ({:?})",
        started.elapsed()
    );
    assert_eq!(
        loaded.total_rows, 5,
        "no uncommitted row before status is known"
    );
    assert_eq!(loaded.status.untracked, 0);

    let with_status = h
        .sink
        .wait_for(TIMEOUT, |name, v| {
            name == "repo-changed" && v["repoId"] == id && v["status"]["untracked"] == 1
        })
        .await
        .map(|(_, v)| snapshot_from_json(v))
        .expect("status arrives");
    assert_eq!(with_status.total_rows, 6);
    assert!(with_status.generation > loaded.generation);
    let rows = h.rows(id).await;
    assert_eq!(rows[0].kind, RowKind::Uncommitted);
    assert_eq!(rows[1].subject, "c4");
}

/// A status that takes too long (or fails) is a notice, once; the rows stay.
#[tokio::test]
async fn a_status_that_times_out_is_a_notice_and_rows_stay() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.write("untracked.txt", "x");
    let h = harness().await;
    h.m.set_status_delay(Duration::from_millis(800));
    h.m.set_status_timeout(Duration::from_millis(300));
    let snap = h.open(repo.path()).await;
    let error = h
        .sink
        .wait_for(TIMEOUT, |name, v| {
            name == "repo-error" && v["repoId"] == snap.repo_id
        })
        .await
        .expect("a notice")
        .1;
    let message = error["error"]["message"].as_str().unwrap().to_owned();
    assert!(message.contains("uncommitted changes"), "{message}");
    assert_eq!(h.m.snapshot(snap.repo_id).await.unwrap().total_rows, 1);
    // Said once, not on every refresh that fails the same way.
    h.m.refresh(snap.repo_id).await.unwrap();
    let errors = h
        .sink
        .events()
        .iter()
        .filter(|(n, _)| n == "repo-error")
        .count();
    assert_eq!(errors, 1);
}

/// A slow status holds up nothing else: view-option changes and refreshes run while it is
/// read, the late status still lands, and an older read never overwrites a newer one.
#[tokio::test]
async fn a_slow_status_blocks_neither_view_changes_nor_refreshes() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.git(&["branch", "side"]);
    let h = harness().await;
    h.m.set_status_delay(Duration::from_millis(1500));
    let snap = h.m.open(repo.path()).await.unwrap();
    let id = snap.repo_id;
    h.wait(id).await;
    // While the first read sleeps: a working-tree change.
    repo.write("untracked.txt", "x");

    let started = std::time::Instant::now();
    let side = ViewOptions {
        branches: Some(vec!["side".into()]),
        ..all_refs()
    };
    h.m.set_view_options(id, side).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(700),
        "set_view_options waited for status ({:?})",
        started.elapsed()
    );
    h.m.set_view_options(id, all_refs()).await.unwrap();
    repo.commit("two");
    let started = std::time::Instant::now();
    h.m.refresh(id).await.unwrap();
    h.wait(id).await;
    assert!(
        started.elapsed() < Duration::from_millis(700),
        "refresh waited for status ({:?})",
        started.elapsed()
    );
    let refreshed = h.m.snapshot(id).await.unwrap();
    assert_eq!(refreshed.total_rows, 2, "the new commit is there at once");

    // The untracked file shows once status has been read.
    h.sink
        .wait_for(TIMEOUT, |name, v| {
            name == "repo-changed" && v["repoId"] == id && v["status"]["untracked"] == 1
        })
        .await
        .expect("the late status lands");
    // Gone again: the read the refresh asked for (or a later one) says so, and nothing
    // older brings it back.
    std::fs::remove_file(repo.path().join("untracked.txt")).unwrap();
    h.m.refresh(id).await.unwrap();
    h.sink
        .wait_for(TIMEOUT, |name, v| {
            name == "repo-changed"
                && v["repoId"] == id
                && v["status"]["untracked"] == 0
                && v["totalRows"] == 2
        })
        .await
        .expect("the newer status lands");
    tokio::time::sleep(Duration::from_millis(3500)).await;
    let now = h.m.snapshot(id).await.unwrap();
    assert_eq!(
        now.status.untracked, 0,
        "an older read was applied over a newer one"
    );
    assert_eq!(now.total_rows, 2);
}

/// A refresh shows a new commit without waiting for status.
#[tokio::test]
async fn refresh_shows_new_commits_before_status() {
    let repo = TestRepo::new();
    repo.commit("one");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let id = snap.repo_id;
    h.m.set_status_delay(Duration::from_millis(1500));
    repo.commit("two");
    let m = h.m.clone();
    let started = std::time::Instant::now();
    let refresh = tokio::spawn(async move { m.refresh(id).await });
    let shown = h
        .sink
        .wait_for(TIMEOUT, |name, v| {
            name == "repo-changed" && v["repoId"] == id && v["totalRows"] == 2
        })
        .await;
    assert!(shown.is_some());
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "the new commit waited for status ({:?})",
        started.elapsed()
    );
    refresh.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------------------
// queries

#[tokio::test]
async fn search_matches() {
    let repo = TestRepo::new();
    let a = repo.commit("Fix Parser bug");
    let b = repo.commit_as("add feature", "Zed Writer", "zed@example.com");
    let c = repo.commit("fix parser again");
    repo.commit("unrelated ünïcode");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let id = snap.repo_id;
    let rows = h.rows(id).await;
    let index = |hash: &str| rows.iter().position(|r| r.hash == hash).unwrap() as u32;
    let (ia, ib, ic) = (index(&a), index(&b), index(&c));

    let mut both = vec![ia, ic];
    both.sort_unstable();
    assert_eq!(h.m.search(id, "PARSER").await.unwrap(), both);
    assert_eq!(h.m.search(id, "  parser  ").await.unwrap(), both);
    assert_eq!(h.m.search(id, "zed").await.unwrap(), [ib]);
    assert_eq!(h.m.search(id, &b[..6]).await.unwrap(), [ib]);
    assert_eq!(h.m.search(id, &b[..6].to_uppercase()).await.unwrap(), [ib]);
    assert_eq!(h.m.search(id, "ÜNÏ").await.unwrap().len(), 1);
    assert!(h.m.search(id, "").await.unwrap().is_empty());
    assert!(h.m.search(id, "   ").await.unwrap().is_empty());
    assert!(h.m.search(id, "no such thing").await.unwrap().is_empty());
}

/// Ref names match at once, with the rest of the in-memory search.
#[tokio::test]
async fn search_matches_ref_names() {
    let repo = TestRepo::new();
    let a = repo.commit("one");
    repo.commit("two");
    repo.git(&["branch", "release/Spring", &a]);
    repo.git(&["tag", "v9-candidate", &a]);
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let id = snap.repo_id;
    let row = h.m.find_row(id, &a).await.unwrap().unwrap();
    assert_eq!(h.m.search(id, "spring").await.unwrap(), [row]);
    assert_eq!(h.m.search(id, "V9-CAND").await.unwrap(), [row]);
}

/// Message bodies are searched by git, case-insensitively and as plain text (a pattern
/// starting with "-" or holding regex characters included), over the commits of the view.
#[tokio::test]
async fn body_search_finds_commits_by_their_whole_message() {
    let repo = TestRepo::new();
    let a = repo.commit("Subject one\n\nThe Body mentions frobnication here.");
    let b = repo.commit("Subject two\n\n--dry-run [x] is literal");
    repo.commit("Frobnication in the subject");
    let side = repo.git(&["rev-parse", "HEAD"]);
    repo.git(&["switch", "-q", "-c", "hidden", &a]);
    let hidden = repo.commit("hidden\n\nfrobnication on another branch");
    repo.git(&["switch", "-q", "main"]);
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let id = snap.repo_id;
    let row = |hash: String| {
        let m = h.m.clone();
        async move { m.find_row(id, &hash).await.unwrap().unwrap() }
    };
    let (ra, rb, rs, rh) = (
        row(a.clone()).await,
        row(b).await,
        row(side).await,
        row(hidden).await,
    );

    let found = h.m.search_bodies(id, "FROBNICATION").await.unwrap();
    assert_eq!(found.generation, snap.generation);
    let mut want = vec![ra, rs, rh];
    want.sort_unstable();
    assert_eq!(found.rows, want);
    assert_eq!(
        h.m.search_bodies(id, "--dry-run [x]").await.unwrap().rows,
        [rb]
    );
    assert!(h.m.search_bodies(id, "  ").await.unwrap().rows.is_empty());

    // Only the commits the view shows.
    let main_only = ViewOptions {
        branches: Some(vec!["main".into()]),
        ..all_refs()
    };
    let snap = h.m.set_view_options(id, main_only).await.unwrap();
    h.wait(id).await;
    let found = h.m.search_bodies(id, "frobnication").await.unwrap();
    assert_eq!(found.rows.len(), 2, "{found:?} in {snap:?}");
}

/// One body search at a time per repository: a newer one stops the one before (its git is
/// killed), and one that takes too long gives up.
#[tokio::test]
async fn a_newer_body_search_stops_the_older_and_slow_ones_give_up() {
    let repo = TestRepo::new();
    repo.commit("one\n\nfrobnication");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    h.m.set_body_search_delay(Duration::from_millis(1000));
    let m = h.m.clone();
    let older = tokio::spawn(async move { m.search_bodies(id, "frob").await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = std::time::Instant::now();
    let m = h.m.clone();
    let newer = tokio::spawn(async move { m.search_bodies(id, "frobnication").await });
    let older = older.await.unwrap();
    assert!(
        older.is_err(),
        "the older search was not stopped: {older:?}"
    );
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(newer.await.unwrap().unwrap().rows.len(), 1);

    h.m.set_body_search_timeout(Duration::from_millis(300));
    let err = h.m.search_bodies(id, "frob").await.unwrap_err();
    assert!(err.message.contains("longer than"), "{}", err.message);
}

#[tokio::test]
async fn find_row_works() {
    let repo = TestRepo::new();
    let c1 = repo.commit("one");
    let c2 = repo.commit("two");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(h.m.find_row(id, &c2).await.unwrap(), Some(0));
    assert_eq!(h.m.find_row(id, &c1).await.unwrap(), Some(1));
    assert_eq!(h.m.find_row(id, &"0".repeat(40)).await.unwrap(), None);
    assert_eq!(h.m.find_row(id, "nonsense").await.unwrap(), None);
}

#[tokio::test]
async fn details_of_a_row() {
    let repo = TestRepo::new();
    let c1 = repo.commit("one");
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let d = h.m.details(id, &c1).await.unwrap();
    assert_eq!(d.hash, c1);
    assert_eq!(d.message, "one");
    assert_eq!(d.files.len(), 1);
    let err = h.m.details(id, "UNCOMMITTED").await.unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
}

#[tokio::test]
async fn rows_are_clamped() {
    let repo = TestRepo::new();
    repo.commit_many(1200);
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let (id, generation) = (snap.repo_id, snap.generation);
    assert_eq!(snap.total_rows, 1200);

    let page = h.m.rows(id, generation, 0, 5000).await.unwrap();
    assert_eq!(page.rows.len(), 1000, "count is capped");
    let page = h.m.rows(id, generation, 1190, 50).await.unwrap();
    assert_eq!((page.start, page.rows.len()), (1190, 10));
    assert_eq!(page.rows[0].index, 1190);
    let page = h.m.rows(id, generation, 5000, 10).await.unwrap();
    assert!(page.rows.is_empty());
    let page = h.m.rows(id, generation, u32::MAX, u32::MAX).await.unwrap();
    assert!(page.rows.is_empty());
}

// ---------------------------------------------------------------------------------------
// events, close, unknown repositories

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn events_emitted() {
    let repo = TestRepo::new();
    repo.commit_many(1200);
    let h = harness().await;
    let snap = h.m.open(repo.path()).await.unwrap();
    let id = snap.repo_id;
    h.sink
        .wait_for(TIMEOUT, |name, v| {
            name == "repo-changed" && v["repoId"] == id && v["loading"] == false
        })
        .await
        .expect("a final repo-changed event");
    let changed = h.repo_changed(id);
    assert!(changed.len() >= 2, "{} events", changed.len());
    let last = changed.last().unwrap();
    assert!(!last.loading);
    assert_eq!(last.total_rows, 1200);
    assert!(changed[..changed.len() - 1].iter().all(|s| s.loading));
    assert!(changed[0].total_rows >= 500);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_and_unknown_repo() {
    let repo = TestRepo::new();
    repo.commit_many(1500);
    let h = harness().await;
    let id = h.m.open(repo.path()).await.unwrap().repo_id;
    // Closing while the log is still loading stops the load.
    h.m.close(id).await.unwrap();
    tokio::time::timeout(TIMEOUT, h.m.wait_loaded(id))
        .await
        .expect("wait_loaded returns for a closed repository");
    assert!(h.m.open_paths().is_empty());
    assert!(h.m.git_dirs(id).is_none());

    let unknown = |e: gg_core::CoreError| assert_eq!(e.code, ErrorCode::UnknownRepo);
    unknown(h.m.close(id).await.unwrap_err());
    unknown(h.m.snapshot(id).await.unwrap_err());
    unknown(h.m.rows(id, 1, 0, 10).await.unwrap_err());
    unknown(h.m.find_row(id, "abc").await.unwrap_err());
    unknown(h.m.search(id, "abc").await.unwrap_err());
    unknown(h.m.details(id, &"0".repeat(40)).await.unwrap_err());
    unknown(h.m.set_view_options(id, all_refs()).await.unwrap_err());
    unknown(h.m.refresh(id).await.unwrap_err());

    // Reopening gives a new id.
    let again = h.open(repo.path()).await;
    assert_ne!(again.repo_id, id);
    assert_eq!(again.total_rows, 1500);
}

// ---------------------------------------------------------------------------------------
// fsmonitor guard

/// A script that creates `marker` when run, for use as an fsmonitor hook.
#[cfg(unix)]
fn marker_hook(repo: &TestRepo, name: &str) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let marker = repo.dir.path().join(format!("{name}-ran"));
    let hook = repo.dir.path().join(format!("{name}.sh"));
    fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    (hook, marker)
}

#[cfg(unix)]
#[tokio::test]
async fn fsmonitor_hook_is_never_run() {
    let repo = TestRepo::new();
    repo.commit("one");
    let (hook, marker) = marker_hook(&repo, "fsmonitor-hook");
    repo.git(&["config", "core.fsmonitor", hook.to_str().unwrap()]);
    // Plain git does run the hook: the guard is what keeps it from running.
    repo.git(&["status", "--porcelain"]);
    assert!(marker.exists(), "git runs the configured fsmonitor hook");
    fs::remove_file(&marker).unwrap();

    repo.write("new.txt", "untracked\n");
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    h.refresh(snap.repo_id).await;
    assert!(
        !marker.exists(),
        "the app ran the repository's fsmonitor hook"
    );
    assert_eq!(snap.status.untracked, 1, "status still works");
}

#[cfg(unix)]
#[tokio::test]
async fn submodule_fsmonitor_hook_is_never_run() {
    let sub = TestRepo::new();
    sub.commit("sub one");
    let repo = TestRepo::new();
    repo.commit("one");
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        sub.path().to_str().unwrap(),
        "sub",
    ]);
    repo.commit_all("add submodule");
    let (hook, marker) = marker_hook(&repo, "submodule-hook");
    let checkout = repo.path().join("sub");
    let status = std::process::Command::new("git")
        .args(["config", "core.fsmonitor", hook.to_str().unwrap()])
        .current_dir(&checkout)
        .env("GIT_CONFIG_GLOBAL", gg_core::testutil::empty_git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .unwrap();
    assert!(status.success());
    // Plain git status in the parent runs the submodule's hook.
    repo.git(&["status", "--porcelain"]);
    assert!(marker.exists(), "git runs the submodule's fsmonitor hook");
    fs::remove_file(&marker).unwrap();

    let h = harness().await;
    let snap = h.open(repo.path()).await;
    h.refresh(snap.repo_id).await;
    assert!(
        !marker.exists(),
        "the app ran the submodule's fsmonitor hook"
    );
}

#[tokio::test]
async fn fsmonitor_is_false_even_when_configured_true() {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.git(&["config", "core.fsmonitor", "true"]);
    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let git = h.m.runner(snap.repo_id).unwrap();
    let value = git
        .run(
            repo.path(),
            &["config", "--get", "core.fsmonitor"],
            CmdKind::Read,
        )
        .await
        .unwrap();
    assert_eq!(value.text().trim(), "false");
}

// ---------------------------------------------------------------------------------------
// stashes

fn stash_rows(rows: &[CommitRow]) -> Vec<&CommitRow> {
    rows.iter().filter(|r| r.kind == RowKind::Stash).collect()
}

/// Every parent of commit `hash`, as git has them.
fn parents_of(repo: &TestRepo, hash: &str) -> Vec<String> {
    let line = repo.git(&["rev-list", "--parents", "-n", "1", hash]);
    line.split(' ').skip(1).map(str::to_owned).collect()
}

/// Each stash is one row, with its base as its only parent and its label; its index and
/// untracked commits are no rows. A stash whose base no branch has keeps that base (and its
/// history) in the graph. Saving a stash on a loaded commit adds its row without reading
/// the log again; dropping the stash that alone leads to a commit reads it again.
#[tokio::test]
async fn stash_rows_appear_once_with_one_parent() {
    let repo = TestRepo::new();
    let c1 = repo.commit("one");
    let c2 = repo.commit("two");
    repo.write("file-0.txt", "changed\n");
    repo.write("loose.txt", "untracked\n");
    let with_untracked = repo.stash(&["--include-untracked", "--message=keep this"]);
    repo.git(&["checkout", "-q", "-b", "tmp"]);
    let lost = repo.commit("on a branch that goes");
    repo.write("file-1.txt", "changed\n");
    let on_lost = repo.stash(&[]);
    repo.checkout("main");
    repo.git(&["branch", "-q", "-D", "tmp"]);

    let h = harness().await;
    let snap = h.open(repo.path()).await;
    let id = snap.repo_id;
    let rows = h.rows(id).await;
    assert_eq!(
        hashes(&rows),
        [
            on_lost.clone(),
            lost.clone(),
            with_untracked.clone(),
            c2.clone(),
            c1.clone()
        ]
    );
    let unique: std::collections::BTreeSet<&str> = rows.iter().map(|r| r.hash.as_str()).collect();
    assert_eq!(unique.len(), rows.len(), "each commit once");
    let stashes = stash_rows(&rows);
    assert_eq!(stashes.len(), 2);
    for (w, base, n) in [(&on_lost, &lost, 0), (&with_untracked, &c2, 1)] {
        let r = row(&rows, w);
        assert_eq!(r.kind, RowKind::Stash);
        assert_eq!(r.parents, std::slice::from_ref(base));
        assert!(!r.is_merge);
        assert_eq!(r.labels, [label(RefKind::Stash, &format!("stash@{{{n}}}"))]);
        let helpers = &parents_of(&repo, w)[1..];
        assert!(!helpers.is_empty());
        for helper in helpers {
            assert!(
                !unique.contains(helper.as_str()),
                "helper {helper} is a row"
            );
        }
    }
    assert_eq!(
        parents_of(&repo, &with_untracked).len(),
        3,
        "it has an untracked commit"
    );
    assert_eq!(row(&rows, &with_untracked).subject, "On main: keep this");
    assert!(row(&rows, &on_lost).subject.starts_with("WIP on tmp: "));
    // Its label and subject are searched like a commit's.
    assert_eq!(h.m.search(id, "stash@{1}").await.unwrap(), [2]);
    assert_eq!(h.m.search(id, "keep this").await.unwrap(), [2]);

    // A new stash on HEAD: in place, without the log.
    let mark = h.last_command_id();
    repo.write("file-0.txt", "changed again\n");
    let newest = repo.stash(&[]);
    let snap = h.refresh(id).await;
    assert_eq!(
        h.log_commands_since(mark),
        0,
        "no log read for a stash on a loaded commit"
    );
    assert_eq!(snap.total_rows, 6);
    let rows = h.rows(id).await;
    assert_eq!(rows[0].hash, newest);
    assert_eq!(rows[0].kind, RowKind::Stash);
    assert_eq!(
        row(&rows, &on_lost).labels,
        [label(RefKind::Stash, "stash@{1}")]
    );

    // Dropping the stash that alone leads to `lost`: the log is read, and `lost` goes.
    let mark = h.last_command_id();
    repo.git(&["stash", "drop", "-q", "stash@{1}"]);
    h.refresh(id).await;
    assert_eq!(h.log_commands_since(mark), 1);
    let rows = h.rows(id).await;
    assert_eq!(hashes(&rows), [newest, with_untracked, c2, c1]);
}

/// With `showStashes` off there are no stash rows or labels, and the log does not list
/// them; turning it off and on again needs no log read when the stashes' bases are loaded.
#[tokio::test]
async fn stashes_hidden_by_view_option() {
    let repo = TestRepo::new();
    let c1 = repo.commit("one");
    repo.write("file-0.txt", "changed\n");
    repo.write("loose.txt", "untracked\n");
    let w = repo.stash(&["--include-untracked"]);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    assert_eq!(hashes(&h.rows(id).await), [w.clone(), c1.clone()]);

    let mark = h.last_command_id();
    let hidden = ViewOptions {
        show_stashes: false,
        ..all_refs()
    };
    let snap = h.view(id, hidden.clone()).await;
    assert!(!snap.view_options.show_stashes);
    let rows = h.rows(id).await;
    assert_eq!(hashes(&rows), std::slice::from_ref(&c1));
    assert!(
        rows.iter()
            .all(|r| r.labels.iter().all(|l| l.kind != RefKind::Stash))
    );
    assert!(h.m.search(id, "stash@{0}").await.unwrap().is_empty());
    assert_eq!(h.log_commands_since(mark), 0);
    // A refresh keeps them hidden, and a log read for another reason leaves them out.
    repo.commit("two");
    h.refresh(id).await;
    let rows = h.rows(id).await;
    assert!(stash_rows(&rows).is_empty());
    assert!(!rows.iter().any(|r| r.hash == w));
    let log = h
        .log
        .since(mark)
        .into_iter()
        .rfind(|r| r.args[0] == "log")
        .unwrap();
    assert!(!log.args.contains(&w));

    let mark = h.last_command_id();
    h.view(id, all_refs()).await;
    assert_eq!(h.log_commands_since(mark), 0);
    let rows = h.rows(id).await;
    assert_eq!(stash_rows(&rows).len(), 1);
    assert_eq!(row(&rows, &w).labels, [label(RefKind::Stash, "stash@{0}")]);
}

/// Task 5 review, item 5: stashes put in memory sit where `git log --date-order` puts them,
/// which goes by committer time: after a commit amended later than the stash, though it was
/// authored before it. Hiding and showing them gives the rows of a fresh load.
#[tokio::test]
async fn stashes_shown_again_sit_where_a_fresh_load_puts_them() {
    let repo = TestRepo::new();
    let c1 = repo.commit("one");
    repo.write("file-0.txt", "changed\n");
    let w = repo.stash(&[]);
    // Authored long before the stash, committed now (long after it).
    repo.write("amended.txt", "amended\n");
    repo.git(&["add", "amended.txt"]);
    repo.git(&["commit", "-q", "-m", "amended", "--date=@1000000000 +0000"]);
    let x = repo.git(&["rev-parse", "HEAD"]);

    let fresh = harness().await;
    let id = fresh.open(repo.path()).await.repo_id;
    let expected = hashes(&fresh.rows(id).await);
    assert_eq!(expected, [x, w, c1]);

    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let mark = h.last_command_id();
    h.view(
        id,
        ViewOptions {
            show_stashes: false,
            ..all_refs()
        },
    )
    .await;
    h.view(id, all_refs()).await;
    assert_eq!(h.log_commands_since(mark), 0, "in memory");
    assert_eq!(hashes(&h.rows(id).await), expected);
}

/// `main` (c1, c2) and `side` (from c1: s1), a stash on each tip: `on_main` on c2,
/// `on_side` on s1. HEAD on `main`.
fn stashes_on_two_branches(repo: &TestRepo) -> [String; 5] {
    let c1 = repo.commit("one");
    repo.branch("side");
    let c2 = repo.commit("two");
    repo.checkout("side");
    let s1 = repo.commit("side one");
    repo.write("file-0.txt", "on side\n");
    let on_side = repo.stash(&[]);
    repo.checkout("main");
    repo.write("file-0.txt", "on main\n");
    let on_main = repo.stash(&[]);
    [c1, c2, s1, on_main, on_side]
}

/// R21: with a branch selection, the stashes whose base is a commit of the selection are
/// shown (and only those), without the stashes becoming tips of the log; stashes can be
/// hidden and shown in that mode too; back to all branches, the rows are those of a fresh
/// load.
#[tokio::test]
async fn a_branch_selection_shows_the_stashes_on_its_commits() {
    let repo = TestRepo::new();
    let [c1, c2, s1, on_main, on_side] = stashes_on_two_branches(&repo);
    let h = harness().await;
    let id = h.open(repo.path()).await.repo_id;
    let main_only = ViewOptions {
        branches: Some(vec!["main".into()]),
        ..all_refs()
    };

    let mark = h.last_command_id();
    let snap = h.view(id, main_only.clone()).await;
    assert!(snap.view_options.show_stashes);
    let rows = h.rows(id).await;
    assert_eq!(hashes(&rows), [on_main.clone(), c2.clone(), c1.clone()]);
    assert_eq!(row(&rows, &on_main).kind, RowKind::Stash);
    assert_eq!(
        row(&rows, &on_main).labels,
        [label(RefKind::Stash, "stash@{0}")]
    );
    let log = h
        .log
        .since(mark)
        .into_iter()
        .rfind(|r| r.args[0] == "log")
        .expect("the selection's log was read");
    assert!(!log.args.contains(&on_main) && !log.args.contains(&on_side));

    // The side branch alone: its stash, not main's.
    let side_only = ViewOptions {
        branches: Some(vec!["side".into()]),
        ..all_refs()
    };
    h.view(id, side_only).await;
    assert_eq!(
        hashes(&h.rows(id).await),
        [on_side.clone(), s1.clone(), c1.clone()]
    );

    // Hidden and shown again within the selection, in memory.
    h.view(id, main_only.clone()).await;
    let mark = h.last_command_id();
    h.view(
        id,
        ViewOptions {
            show_stashes: false,
            ..main_only.clone()
        },
    )
    .await;
    assert_eq!(hashes(&h.rows(id).await), [c2.clone(), c1.clone()]);
    h.view(id, main_only).await;
    assert_eq!(hashes(&h.rows(id).await), [on_main, c2, c1]);
    assert_eq!(h.log_commands_since(mark), 0, "in memory");

    // Back to all branches: as a fresh load.
    h.view(id, all_refs()).await;
    let fresh = harness().await;
    let fresh_id = fresh.open(repo.path()).await.repo_id;
    assert_eq!(
        hashes(&h.rows(id).await),
        hashes(&fresh.rows(fresh_id).await)
    );
}
