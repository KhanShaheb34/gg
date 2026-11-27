//! The Clone dialog against local folders and the recording fake `gh` (never the network):
//! a link cloned into the folder name chosen, the parent folders made; a folder that is not
//! empty refused before git runs; Stop taking away only what the clone made; the user's
//! repositories through `gh` and the states it can be in; the repositories gg already has.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::harness;
use gg_core::clone::CloneSource;
use gg_core::dto::InboxProblem;
use gg_core::error::ErrorCode;
use gg_core::inbox::InboxStore;
use gg_core::testutil::{FakeGh, TestRepo};

struct Setup {
    h: common::Harness,
    gh: FakeGh,
    _config: tempfile::TempDir,
    /// The clone source: `<source>/<owner>/<name>.git`.
    source: tempfile::TempDir,
    _repo: TestRepo,
}

/// A harness whose clones come from a local folder holding `octo/cat.git`.
async fn setup() -> Setup {
    let gh = FakeGh::new();
    let h = harness().await;
    h.m.set_tool_path(Some(gh.path()));
    let config = tempfile::tempdir().unwrap();
    h.m.set_inbox_store(Arc::new(InboxStore::load(config.path())));
    let repo = TestRepo::new();
    repo.commit("one");
    repo.commit("two");
    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(source.path().join("octo")).unwrap();
    let bare = source.path().join("octo/cat.git");
    repo.git(&["clone", "-q", "--bare", ".", bare.to_str().unwrap()]);
    h.m.set_clone_source(Some(source.path().to_owned()));
    Setup {
        h,
        gh,
        _config: config,
        source,
        _repo: repo,
    }
}

fn link(url: &str) -> CloneSource {
    gg_core::clone::read_link(url).unwrap().0
}

fn clones(s: &Setup) -> usize {
    s.h.log
        .since(0)
        .into_iter()
        .filter(|r| r.args.iter().any(|a| a == "clone"))
        .count()
}

/// A link (any of its forms) is cloned into the folder name given, in a parent folder made for
/// it; the repository's commits are there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_link_is_cloned_into_the_folder_named() {
    let s = setup().await;
    let root = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(root.path()).unwrap();
    let parent = root.join("Developer/team");
    let done =
        s.h.m
            .clone_repo("l1", &link("https://github.com/octo/cat"), &parent, "kitty")
            .await
            .unwrap();
    assert_eq!(done.status, "done", "{done:?}");
    let path = Path::new(done.path.as_deref().unwrap()).to_owned();
    assert_eq!(path, parent.join("kitty"));
    let log = std::process::Command::new("git")
        .args(["log", "--format=%s"])
        .current_dir(&path)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&log.stdout), "two\none\n");
    // The scp-like form of the same repository, into an empty folder already there.
    std::fs::create_dir(root.join("empty")).unwrap();
    let done =
        s.h.m
            .clone_repo("l2", &link("git@github.com:octo/cat.git"), &root, "empty")
            .await
            .unwrap();
    assert_eq!(done.status, "done", "{done:?}");
    assert!(root.join("empty/.git").is_dir());
}

/// Something not empty at the destination, a name that leaves the parent, a relative parent:
/// refused before git runs, and nothing there is touched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_taken_destination_is_refused_before_git() {
    let s = setup().await;
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("cat")).unwrap();
    std::fs::write(root.path().join("cat/notes.txt"), "mine").unwrap();
    let source = link("octo/cat");
    let e =
        s.h.m
            .clone_repo("t1", &source, root.path(), "cat")
            .await
            .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    assert!(e.message.contains("not empty"), "{}", e.message);
    for name in ["..", "../cat", "a/b", "-x"] {
        let e =
            s.h.m
                .clone_repo("t2", &source, root.path(), name)
                .await
                .unwrap_err();
        assert_eq!(e.code, ErrorCode::BadRequest, "{name}");
    }
    let e =
        s.h.m
            .clone_repo("t3", &source, Path::new("relative"), "cat")
            .await
            .unwrap_err();
    assert_eq!(e.code, ErrorCode::PathMissing);
    assert_eq!(clones(&s), 0, "git never ran");
    assert_eq!(
        std::fs::read_to_string(root.path().join("cat/notes.txt")).unwrap(),
        "mine"
    );
}

/// A clone that fails (no such repository) and one that is stopped halfway take away what they
/// made (the folder, the parent folders made for it) and nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_and_failure_take_away_only_what_the_clone_made() {
    let s = setup().await;
    let root = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(root.path()).unwrap();
    std::fs::write(root.join("keep.txt"), "mine").unwrap();

    let failed =
        s.h.m
            .clone_repo(
                "f1",
                &link("octo/nothing"),
                &root.join("new/deeper"),
                "nothing",
            )
            .await
            .unwrap();
    assert_eq!(failed.status, "failed", "{failed:?}");
    let notice = failed.notice.expect("a notice");
    assert!(!notice.title.is_empty());
    assert!(!root.join("new").exists(), "the parents made are gone");
    assert!(root.join("keep.txt").exists());

    // The source answers through a hook that waits: the clone hangs until stopped.
    let hook = s.source.path().join("slow-hook");
    std::fs::write(&hook, "#!/bin/sh\nsleep 30\nexec git upload-pack \"$@\"\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    s.h.m.set_clone_hook(Some(hook));
    let parent = root.join("Developer");
    let (m, p) = (s.h.m.clone(), parent.clone());
    let running = tokio::spawn(async move {
        m.clone_repo("s1", &link("https://github.com/octo/cat.git"), &p, "cat")
            .await
    });
    // git has made its folder and is waiting on the source.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !parent.join("cat/.git").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the clone never started"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(s.h.m.clone_cancel("s1"));
    let stopped = tokio::time::timeout(Duration::from_secs(15), running)
        .await
        .expect("Stop ended it")
        .unwrap()
        .unwrap();
    assert_eq!(stopped.status, "stopped", "{stopped:?}");
    assert!(!parent.exists(), "the folder and the parent made are gone");
    assert!(root.join("keep.txt").exists());

    // Into an empty folder that was there: emptied again, and kept.
    std::fs::create_dir(root.join("waiting")).unwrap();
    let (m, p) = (s.h.m.clone(), root.clone());
    let running =
        tokio::spawn(async move { m.clone_repo("s2", &link("octo/cat"), &p, "waiting").await });
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !root.join("waiting/.git").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the clone never started"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(s.h.m.clone_cancel("s2"));
    let stopped = running.await.unwrap().unwrap();
    assert_eq!(stopped.status, "stopped");
    assert!(root.join("waiting").is_dir());
    assert_eq!(std::fs::read_dir(root.join("waiting")).unwrap().count(), 0);
}

fn repos_answer() -> String {
    serde_json::json!({"data": {"viewer": {"login": "octo", "repositories": {
    "totalCount": 3,
    "nodes": [
        {"name": "cat", "owner": {"login": "octo"}, "isPrivate": true, "isArchived": false,
         "isFork": false, "description": "Purrs", "pushedAt": "2026-03-02T10:00:00Z"},
        {"name": "dog", "owner": {"login": "hub"}, "isPrivate": false, "isArchived": false,
         "isFork": true, "description": null, "pushedAt": "2026-03-01T10:00:00Z"},
        {"name": "owl", "owner": {"login": "octo"}, "isPrivate": false, "isArchived": true,
         "isFork": false, "description": "", "pushedAt": "2025-03-01T10:00:00Z"}
    ]}}}})
    .to_string()
}

/// The user's repositories: one `gh api graphql` for github.com, in a folder of no repository,
/// the most recently pushed first, at most 100; each state gh can be in is told apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_users_repositories_through_gh() {
    let s = setup().await;
    s.gh.reply("gql-repos", 0, &repos_answer(), "");
    let list = s.h.m.clone_repos().await;
    assert_eq!(list.problem, None, "{list:?}");
    assert_eq!(list.login.as_deref(), Some("octo"));
    assert_eq!(list.total, 3);
    let names: Vec<String> = list
        .repos
        .iter()
        .map(|r| format!("{}/{}", r.owner, r.name))
        .collect();
    assert_eq!(names, ["octo/cat", "hub/dog", "octo/owl"]);
    assert!(list.repos[0].private && list.repos[1].fork && list.repos[2].archived);
    let calls = s.gh.calls_of(&["api", "graphql"]);
    assert_eq!(calls.len(), 1);
    let argv = &calls[0].argv;
    assert_eq!(argv[2..4], ["--hostname", "github.com"]);
    assert!(argv.iter().any(|a| a.contains("query GgRepos")));
    assert!(argv.iter().any(|a| a.contains("ORGANIZATION_MEMBER")));
    assert!(argv.iter().any(|a| a == "first=100"));
    assert_eq!(
        dunce::canonicalize(&calls[0].cwd).unwrap(),
        dunce::canonicalize(std::env::temp_dir()).unwrap()
    );

    for (said, want) in [
        (
            "You are not logged into any GitHub hosts. To log in, run: gh auth login",
            InboxProblem::SignedOut,
        ),
        (
            "GraphQL: API rate limit exceeded for user ID 1.",
            InboxProblem::RateLimited,
        ),
        (
            "error connecting to api.github.com\ncheck your internet connection",
            InboxProblem::Offline,
        ),
        ("HTTP 502: Bad gateway", InboxProblem::Failed),
    ] {
        s.gh.reply("gql-repos", 1, "", said);
        let list = s.h.m.clone_repos().await;
        assert_eq!(list.problem, Some(want), "{said}");
        assert!(list.message.is_some());
        assert!(list.repos.is_empty());
    }
    s.gh.reply(
        "gql-repos",
        0,
        r#"{"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded"}]}"#,
        "",
    );
    assert_eq!(
        s.h.m.clone_repos().await.problem,
        Some(InboxProblem::RateLimited)
    );
    let empty = tempfile::tempdir().unwrap();
    s.h.m
        .set_tool_path(Some(empty.path().as_os_str().to_owned()));
    let list = s.h.m.clone_repos().await;
    assert_eq!(list.problem, Some(InboxProblem::NoGh));
    // Without gh, a link still clones with git alone.
    let root = tempfile::tempdir().unwrap();
    let done =
        s.h.m
            .clone_repo("n1", &link("octo/cat"), root.path(), "cat")
            .await
            .unwrap();
    assert_eq!(done.status, "done", "{done:?}");
}

/// The repositories gg already has: the open ones by their remotes (with their tab), and
/// recent ones not open by the remotes git has for them; a recent folder gone is left out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repositories_gg_has_are_known_by_their_remotes() {
    let s = setup().await;
    let open = TestRepo::new();
    open.commit("one");
    open.git(&["remote", "add", "origin", "git@github.com:octo/cat.git"]);
    let id = s.h.open(open.path()).await.repo_id;
    let recent = TestRepo::new();
    recent.commit("one");
    recent.git(&["remote", "add", "origin", "https://github.com/hub/dog.git"]);
    recent.git(&["remote", "add", "mirror", "/srv/not-a-host"]);
    let gone = recent.path().join("gone");
    let open_root = dunce::canonicalize(open.path()).unwrap();
    let known =
        s.h.m
            .clone_known(vec![
                open_root.to_string_lossy().into_owned(),
                recent.path().to_string_lossy().into_owned(),
                gone.to_string_lossy().into_owned(),
            ])
            .await;
    let found: Vec<(String, Option<u32>)> = known
        .iter()
        .map(|k| (format!("{}/{}/{}", k.host, k.owner, k.name), k.repo_id))
        .collect();
    assert_eq!(
        found,
        [
            ("github.com/octo/cat".to_owned(), Some(id)),
            ("github.com/hub/dog".to_owned(), None),
        ]
    );
    assert_eq!(Path::new(&known[1].path), recent.path());
}
