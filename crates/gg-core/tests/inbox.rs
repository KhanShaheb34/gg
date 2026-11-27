//! The Pull requests page across repositories, against the recording fake `gh` on the tool
//! `PATH` (`gg_core::testutil::FakeGh`): one GraphQL query per GitHub host, what gg keeps of
//! it, a pull request's details and the writes to it by repository name, which open
//! repositories have it as a remote, and cloning from a local folder (never the network).
#![cfg(unix)]

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::harness;
use gg_core::dto::{
    InboxActivity, InboxChecks, InboxGroup, InboxProblem, OpRequest, ReviewDecision,
};
use gg_core::error::ErrorCode;
use gg_core::inbox::InboxStore;
use gg_core::launch::External;
use gg_core::testutil::{FakeGh, TestRepo};
use serde_json::{Value, json};

const VIEW: &str = include_str!("fixtures/gh/pr22-view.json");
const THREADS: &str = include_str!("fixtures/gh/pr22-threads.json");

struct Setup {
    h: common::Harness,
    gh: FakeGh,
    config: tempfile::TempDir,
}

async fn setup() -> Setup {
    let gh = FakeGh::new();
    let h = harness().await;
    h.m.set_tool_path(Some(gh.path()));
    let config = tempfile::tempdir().unwrap();
    h.m.set_inbox_store(Arc::new(InboxStore::load(config.path())));
    h.m.set_inbox_every(Duration::ZERO);
    Setup { h, gh, config }
}

fn node(number: u32, repo: &str, author: &str, o: Value) -> Value {
    let (owner, name) = repo.split_once('/').unwrap();
    let mut n = json!({
        "number": number,
        "title": format!("Change {number}"),
        "url": format!("https://github.com/{repo}/pull/{number}"),
        "isDraft": false,
        "updatedAt": "2026-03-01T10:00:00Z",
        "headRefName": "fix/typo",
        "baseRefName": "main",
        "isCrossRepository": false,
        "repository": {"owner": {"login": owner}, "name": name},
        "headRepository": {"owner": {"login": owner}, "name": name},
        "author": {"login": author},
        "reviewDecision": "REVIEW_REQUIRED",
        "comments": {"totalCount": 1, "nodes": [{"author": {"login": "ada"}}]},
        "reviews": {"totalCount": 0, "nodes": []},
        "commits": {"nodes": [{"commit": {"oid": "a1", "statusCheckRollup": {"state": "FAILURE"}}}]}
    });
    if let (Value::Object(n), Value::Object(o)) = (&mut n, o) {
        n.extend(o);
    }
    n
}

fn answer(mine: Vec<Value>, review: Vec<Value>, mentioned: Vec<Value>) -> String {
    let search = |nodes: Vec<Value>| json!({"issueCount": nodes.len(), "nodes": nodes});
    json!({"data": {
        "viewer": {"login": "octo"},
        "mine": search(mine),
        "review": search(review),
        "mentioned": search(mentioned),
        "assigned": search(vec![]),
    }})
    .to_string()
}

/// One `gh api graphql` with the page's query, in a folder of no repository, for github.com
/// (no `GH_HOST`): the groups come back each once, newest first; asked again inside the
/// floor, gh is not asked again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_page_is_one_query_per_host_read_into_groups() {
    let s = setup().await;
    // Nothing kept yet: an empty page, gh not asked.
    let kept = s.h.m.pr_inbox(false).await.unwrap();
    assert!(kept.prs.is_empty() && !kept.fresh);
    assert!(s.gh.calls().is_empty());

    s.gh.reply(
        "gql-inbox",
        0,
        &answer(
            vec![node(1, "octo/cat", "octo", json!({"isDraft": true}))],
            vec![node(2, "hub/dog", "ada", json!({}))],
            vec![
                node(2, "hub/dog", "ada", json!({})),
                node(3, "hub/owl", "grace", json!({})),
            ],
        ),
        "",
    );
    let page = s.h.m.pr_inbox(true).await.unwrap();
    assert!(page.fresh);
    let got: Vec<(u32, InboxGroup, bool)> = page
        .prs
        .iter()
        .map(|p| (p.number, p.group, p.mentioned))
        .collect();
    assert_eq!(
        got,
        [
            (1, InboxGroup::Mine, false),
            (2, InboxGroup::Review, true),
            (3, InboxGroup::Involved, true),
        ]
    );
    let first = &page.prs[0];
    assert!(first.draft);
    assert_eq!(first.checks, InboxChecks::Failing);
    assert_eq!(first.review, ReviewDecision::ReviewRequired);
    assert_eq!((first.owner.as_str(), first.name.as_str()), ("octo", "cat"));
    assert_eq!(page.hosts.len(), 1);
    assert_eq!(page.hosts[0].login.as_deref(), Some("octo"));
    assert_eq!(page.hosts[0].problem, None);

    let calls = s.gh.calls_of(&["api", "graphql"]);
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.argv.len(), 8, "{:?}", call.argv);
    // The host named, whatever gh's own default is.
    assert_eq!(call.argv[2..4], ["--hostname", "github.com"]);
    assert!(call.argv[5].starts_with("query=query GgInbox($first:Int!)"));
    assert_eq!(call.argv[6..], ["-F", "first=100"]);
    assert!(!call.env.iter().any(|e| e.starts_with("GH_HOST=")));
    assert!(!call.env.iter().any(|e| e.starts_with("GH_REPO=")));
    assert!(call.env.iter().any(|e| e == "GH_PROMPT_DISABLED=1"));
    assert!(!Path::new(call.cwd.trim()).join(".git").exists());
    assert!(!call.tty);

    // Kept between runs, in gg's config folder only.
    let again = InboxStore::load(s.config.path());
    drop(again);
    let kept = s.h.m.pr_inbox(false).await.unwrap();
    assert_eq!(kept.prs.len(), 3);
    assert!(!kept.fresh);
    assert!(s.config.path().join("pr-inbox.json").is_file());

    // Inside the floor: the last answer, gh not asked again.
    s.h.m.set_inbox_every(Duration::from_secs(60));
    s.h.m.pr_inbox(true).await.unwrap();
    assert_eq!(s.gh.calls_of(&["api", "graphql"]).len(), 1);
}

/// gh missing, signed out, rate limited, offline: each said for the host; a list loaded
/// before stays, as of when it was loaded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_failure_is_said_and_the_list_kept() {
    let s = setup().await;
    let cases = [
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
    ];
    for (said, want) in cases {
        s.gh.reply("gql-inbox", 1, "", said);
        let page = s.h.m.pr_inbox(true).await.unwrap();
        assert_eq!(page.hosts[0].problem, Some(want), "{said}");
        assert!(page.hosts[0].message.is_some());
        assert!(page.prs.is_empty());
    }
    // An answer of errors only (gh exits 0 for some).
    s.gh.reply(
        "gql-inbox",
        0,
        r#"{"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded"}]}"#,
        "",
    );
    let page = s.h.m.pr_inbox(true).await.unwrap();
    assert_eq!(page.hosts[0].problem, Some(InboxProblem::RateLimited));

    s.gh.reply(
        "gql-inbox",
        0,
        &answer(vec![], vec![node(5, "octo/cat", "ada", json!({}))], vec![]),
        "",
    );
    let page = s.h.m.pr_inbox(true).await.unwrap();
    let loaded = page.hosts[0].loaded_at_ms;
    assert!(loaded.is_some());
    s.gh.reply("gql-inbox", 1, "", "error connecting to api.github.com");
    let page = s.h.m.pr_inbox(true).await.unwrap();
    assert_eq!(page.prs.len(), 1, "the list stays");
    assert_eq!(page.hosts[0].problem, Some(InboxProblem::Offline));
    assert_eq!(page.hosts[0].loaded_at_ms, loaded);

    // No gh at all.
    let empty = tempfile::tempdir().unwrap();
    s.h.m
        .set_tool_path(Some(empty.path().as_os_str().to_owned()));
    let page = s.h.m.pr_inbox(true).await.unwrap();
    assert_eq!(page.hosts[0].problem, Some(InboxProblem::NoGh));
}

/// Opening a pull request in gg records what it has; what comes after shows as activity.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn activity_since_it_was_last_opened() {
    let s = setup().await;
    let with = |comments: u32, last: &str| json!({"comments": {"totalCount": comments, "nodes": [{"author": {"login": last}}]}});
    s.gh.reply(
        "gql-inbox",
        0,
        &answer(
            vec![],
            vec![node(5, "octo/cat", "ada", with(1, "ada"))],
            vec![],
        ),
        "",
    );
    let page = s.h.m.pr_inbox(true).await.unwrap();
    assert_eq!(
        page.prs[0].activity,
        InboxActivity::None,
        "the first load is the start"
    );
    s.gh.reply(
        "gql-inbox",
        0,
        &answer(
            vec![],
            vec![
                node(5, "octo/cat", "ada", with(2, "ada")),
                node(6, "octo/cat", "ada", json!({})),
            ],
            vec![],
        ),
        "",
    );
    let page = s.h.m.pr_inbox(true).await.unwrap();
    let of = |n: u32| page.prs.iter().find(|p| p.number == n).unwrap().activity;
    assert_eq!(of(5), InboxActivity::Updated);
    assert_eq!(of(6), InboxActivity::New);
    s.h.m.pr_inbox_seen("github.com/octo/cat#5").await.unwrap();
    s.h.m.pr_inbox_seen("github.com/octo/cat#6").await.unwrap();
    let page = s.h.m.pr_inbox(false).await.unwrap();
    assert!(page.prs.iter().all(|p| p.activity == InboxActivity::None));
    // The user's own reply is no news.
    s.gh.reply(
        "gql-inbox",
        0,
        &answer(
            vec![],
            vec![node(5, "octo/cat", "ada", with(3, "octo"))],
            vec![],
        ),
        "",
    );
    let page = s.h.m.pr_inbox(true).await.unwrap();
    assert_eq!(page.prs[0].activity, InboxActivity::None);
}

/// An open repository whose remote is on an enterprise host gh has credentials for makes
/// the page ask that host too, with GH_HOST and without the enterprise token variables.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enterprise_hosts_of_open_repositories_are_asked_too() {
    let s = setup().await;
    let repo = TestRepo::new();
    repo.commit("one");
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://ghe.example/team/app.git",
    ]);
    s.gh.reply("auth-token", 0, "", "");
    s.h.open(repo.path()).await;
    // Someone else's on each host, so the check holds whichever answers first.
    s.gh.reply(
        "gql-inbox",
        0,
        &answer(vec![], vec![node(9, "octo/cat", "ada", json!({}))], vec![]),
        "",
    );
    let mut ghe = node(3, "team/app", "ada", json!({}));
    ghe["url"] = "https://ghe.example/team/app/pull/3".into();
    s.gh.reply(
        "gql-inbox-ghe_example",
        0,
        &answer(vec![], vec![ghe], vec![]),
        "",
    );
    let page = s.h.m.pr_inbox(true).await.unwrap();
    let hosts: Vec<&str> = page.hosts.iter().map(|h| h.host.as_str()).collect();
    assert_eq!(hosts, ["github.com", "ghe.example"]);
    assert_eq!(page.prs.len(), 2);
    assert!(page.prs.iter().any(|p| p.key == "ghe.example/team/app#3"));
    // Both answered on the page's first load: nothing is new on either host.
    assert!(page.prs.iter().all(|p| p.activity == InboxActivity::None));
    let ghe_call =
        s.gh.calls_of(&["api", "graphql"])
            .into_iter()
            .find(|c| c.env.iter().any(|e| e == "GH_HOST=ghe.example"))
            .expect("asked with GH_HOST");
    assert!(
        !ghe_call
            .env
            .iter()
            .any(|e| e.starts_with("GH_ENTERPRISE_TOKEN="))
    );
}

/// A pull request's details by its repository's name: `gh pr view --repo octo/cat` and the
/// threads' query, at most once in the floor; a reply sent through gh's stdin, after which
/// they are asked again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn details_and_writes_by_repository_name() {
    let s = setup().await;
    s.h.m.set_pr_details_every(Duration::from_secs(60));
    s.gh.reply("pr-view", 0, VIEW, "");
    s.gh.reply("api-graphql", 0, THREADS, "");
    let d =
        s.h.m
            .inbox_pr_details("github.com", "octo", "cat", 22)
            .await
            .unwrap();
    assert_eq!(d.number, 22);
    assert_eq!(d.checks.len(), 4);
    let view = &s.gh.calls_of(&["pr", "view"])[0];
    assert_eq!(view.argv[..5], ["pr", "view", "22", "--repo", "octo/cat"]);
    s.h.m
        .inbox_pr_details("github.com", "octo", "cat", 22)
        .await
        .unwrap();
    assert_eq!(s.gh.calls_of(&["pr", "view"]).len(), 1, "inside the floor");

    let reply = OpRequest::PrReply {
        number: 22,
        thread_id: "PRRT_kwDOabc".into(),
        body: "Done in `abc`.".into(),
    };
    let wrote =
        s.h.m
            .inbox_pr_write("github.com", "octo", "cat", &reply)
            .await
            .unwrap();
    assert!(wrote.done, "{wrote:?}");
    let sent =
        s.gh.calls_of(&["api", "graphql", "--input", "-"])
            .pop()
            .expect("the reply");
    let body: Value = serde_json::from_slice(&sent.stdin).unwrap();
    assert_eq!(body["variables"]["body"], "Done in `abc`.");
    assert!(!sent.argv.iter().any(|a| a.contains("Done in")));
    // Forgotten: asked again at once.
    s.h.m
        .inbox_pr_details("github.com", "octo", "cat", 22)
        .await
        .unwrap();
    assert_eq!(s.gh.calls_of(&["pr", "view"]).len(), 2);

    let comment = OpRequest::PrComment {
        number: 22,
        body: "Thanks".into(),
    };
    s.gh.reply("pr-comment", 1, "", "GraphQL: Could not add the comment");
    let wrote =
        s.h.m
            .inbox_pr_write("github.com", "octo", "cat", &comment)
            .await
            .unwrap();
    assert!(!wrote.done);
    assert!(
        wrote.message.contains("Could not add the comment"),
        "{wrote:?}"
    );

    // Only the Pull request tab's writes, on a plain repository name.
    let other = OpRequest::PrComment {
        number: 22,
        body: "x".into(),
    };
    let e =
        s.h.m
            .inbox_pr_write("github.com", "-octo", "cat", &other)
            .await
            .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    let e =
        s.h.m
            .inbox_pr_details("evil host", "octo", "cat", 1)
            .await
            .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    let e =
        s.h.m
            .inbox_pr_details("github.com", "octo", "cat", 0)
            .await
            .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
}

/// The open repositories with the pull request's repository as a remote, by its address
/// (any case, ssh or https), with the remote its head is on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_repositories_are_matched_by_their_remotes() {
    let s = setup().await;
    let repo = TestRepo::new();
    repo.commit("one");
    repo.git(&["remote", "add", "origin", "git@github.com:Octo/Cat.git"]);
    repo.git(&["remote", "add", "fork", "https://github.com/ada/cat.git"]);
    let other = TestRepo::new();
    other.commit("one");
    other.git(&["remote", "add", "origin", "https://github.com/hub/dog.git"]);
    let id = s.h.open(repo.path()).await.repo_id;
    s.h.open(other.path()).await;

    let found =
        s.h.m
            .inbox_repo_match("github.com", "octo", "cat", Some("octo/cat"))
            .await
            .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].repo_id, id);
    assert_eq!(found[0].head_remote.as_deref(), Some("origin"));
    let fork =
        s.h.m
            .inbox_repo_match("github.com", "octo", "cat", Some("ada/cat"))
            .await
            .unwrap();
    assert_eq!(fork[0].head_remote.as_deref(), Some("fork"));
    let stranger =
        s.h.m
            .inbox_repo_match("github.com", "octo", "cat", Some("grace/cat"))
            .await
            .unwrap();
    assert_eq!(stranger[0].head_remote, None);
    assert!(
        s.h.m
            .inbox_repo_match("ghe.example", "octo", "cat", None)
            .await
            .unwrap()
            .is_empty()
    );
}

/// Cloning from the local clone source (never the network): into a new folder of the folder
/// chosen, progress as events, remembered for next time; a folder already there is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clone_into_a_new_folder() {
    let s = setup().await;
    let src = TestRepo::new();
    src.commit("one");
    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(source.path().join("octo")).unwrap();
    let bare = source.path().join("octo/cat.git");
    src.git(&["clone", "-q", "--bare", ".", bare.to_str().unwrap()]);
    s.h.m.set_clone_source(Some(source.path().to_owned()));
    let parent = tempfile::tempdir().unwrap();
    let parent_path = dunce::canonicalize(parent.path()).unwrap();

    let done =
        s.h.m
            .clone_repo("c1", "github.com", "octo", "cat", &parent_path)
            .await
            .unwrap();
    assert_eq!(done.status, "done", "{done:?}");
    let path = done.path.unwrap();
    assert!(Path::new(&path).join(".git").is_dir());
    assert_eq!(s.h.m.clone_parent().as_deref(), parent_path.to_str());
    let logged: Vec<_> =
        s.h.log
            .since(0)
            .into_iter()
            .filter(|r| r.args.first().is_some_and(|a| a == "clone"))
            .collect();
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].args[..3], ["clone", "--progress", "--"]);

    let e =
        s.h.m
            .clone_repo("c2", "github.com", "octo", "cat", &parent_path)
            .await
            .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    assert!(e.message.contains("already"));
    let missing =
        s.h.m
            .clone_repo("c3", "github.com", "hub", "dog", &parent_path)
            .await
            .unwrap();
    assert_eq!(missing.status, "failed");
    assert!(!parent_path.join("dog").exists());
    let e =
        s.h.m
            .clone_repo("c4", "github.com", "octo", "cat", Path::new("relative"))
            .await
            .unwrap_err();
    assert_eq!(e.code, ErrorCode::PathMissing);
    assert!(!s.h.m.clone_cancel("nothing-running"));
    // A Stop that reaches the core before its clone: that clone never starts git.
    assert!(!s.h.m.clone_cancel("c6"));
    let early =
        s.h.m
            .clone_repo(
                "c6",
                "github.com",
                "octo",
                "cat",
                &other_parent(&parent_path),
            )
            .await
            .unwrap();
    assert_eq!(early.status, "stopped");
    assert!(!parent_path.join("early/cat").exists());
    let clones = |log: &gg_core::git::runner::CommandLog| {
        log.since(0)
            .into_iter()
            .filter(|r| r.args.first().is_some_and(|a| a == "clone"))
            .count()
    };
    assert_eq!(clones(&s.h.log), 2, "no git for the stopped one");
    // One id, one clone: a second with the id of one running is refused at once.
    s.gh.reply("auth-token", 0, "", "");
    s.gh.slow("auth-token", 2);
    let (m, dir) = (s.h.m.clone(), other_parent(&parent_path));
    let first =
        tokio::spawn(async move { m.clone_repo("c7", "ghe.example", "octo", "cat", &dir).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    let e =
        s.h.m
            .clone_repo(
                "c7",
                "ghe.example",
                "octo",
                "cat",
                &other_parent(&parent_path),
            )
            .await
            .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    assert!(e.message.contains("runs already"), "{}", e.message);
    // While it runs, gg does not restart.
    assert_eq!(s.h.m.begin_restart().unwrap_err().code, ErrorCode::Busy);
    let _ = first.await.unwrap();
    s.gh.fast("auth-token");
    // While gg restarts to update, no clone starts.
    let other = parent_path.join("other");
    std::fs::create_dir(&other).unwrap();
    s.h.m.begin_restart().unwrap();
    let e =
        s.h.m
            .clone_repo("c5", "github.com", "octo", "cat", &other)
            .await
            .unwrap_err();
    assert_eq!(e.code, ErrorCode::Busy);
    assert!(!other.join("cat").exists());
    s.h.m.end_restart();
}

/// A pull request's page opens only on a GitHub host the page knows; a link anywhere, https
/// only.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn web_pages_open_only_where_they_should() {
    let s = setup().await;
    s.h.m.record_launches();
    let open = |target, url: &str| s.h.m.open_web(target, url, &["ghe.example".into()]);
    assert!(open(External::Url, "https://github.com/octo/cat/pull/1").is_ok());
    assert!(open(External::Url, "https://ghe.example/team/app/pull/3").is_ok());
    assert!(open(External::Url, "https://evil.example/x").is_err());
    assert!(open(External::Link, "https://evil.example/x").is_ok());
    assert!(open(External::Link, "http://evil.example/x").is_err());
    assert!(open(External::Editor, "https://github.com/").is_err());
}

/// A folder `early` beside the clones (made once), for the clones that must not happen.
fn other_parent(parent: &Path) -> std::path::PathBuf {
    let dir = parent.join("early");
    let _ = std::fs::create_dir(&dir);
    dir
}
