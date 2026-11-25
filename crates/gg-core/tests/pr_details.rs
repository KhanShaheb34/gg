//! A pull request's checks and comments (`pr_details`) through `gh`, against the recording
//! fake `gh` on the tool `PATH` (`gg_core::testutil::FakeGh`), answering with the captured
//! answers in `tests/fixtures/gh`: no test reaches a network host. Also: the links of
//! comments and checks open in the browser only as `https://` addresses.
#![cfg(unix)]

mod common;

use std::time::{Duration, Instant};

use common::harness;
use gg_core::dto::{CheckState, CommentKind, MergeStatus, PrLifecycle};
use gg_core::error::ErrorCode;
use gg_core::launch::{External, MAX_LINK_LEN, Programs};
use gg_core::testutil::{FakeGh, TestRepo};

const VIEW: &str = include_str!("fixtures/gh/pr22-view.json");
const VIEW_PENDING: &str = include_str!("fixtures/gh/pr25-view-pending.json");
const THREADS: &str = include_str!("fixtures/gh/pr22-threads.json");
const VIEW_FIELDS: &str = "number,state,isDraft,mergeable,mergeStateStatus,reviewDecision,\
                           statusCheckRollup,comments,reviews,url,updatedAt";

struct Setup {
    _repo: TestRepo,
    h: common::Harness,
    id: u32,
    gh: FakeGh,
}

/// A repository whose `origin` is `url` (never fetched from), opened with the fake `gh`.
async fn setup_with(url: &str) -> Setup {
    let repo = TestRepo::new();
    repo.commit("one");
    repo.git(&["remote", "add", "origin", url]);
    let gh = FakeGh::new();
    let h = harness().await;
    h.m.set_tool_path(Some(gh.path()));
    let id = h.open(repo.path()).await.repo_id;
    Setup {
        _repo: repo,
        h,
        id,
        gh,
    }
}

async fn setup() -> Setup {
    let s = setup_with("https://github.com/octo/cat.git").await;
    s.gh.reply("pr-view", 0, VIEW, "");
    s.gh.reply("api-graphql", 0, THREADS, "");
    s
}

/// `gh pr view <n> --repo <repo> --json …` and `gh api graphql` with the threads' query, the
/// owner, name and number as its variables (strings with `-f`, which never reads a file; the
/// number alone typed): each as every `gh` command runs, both at once; their answers make
/// one list of comments.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn details_come_from_two_gh_commands_at_once() {
    let s = setup().await;
    s.gh.slow("pr-view", 1);
    s.gh.slow("api-graphql", 1);
    let d = s.h.m.pr_details(s.id, 22).await.unwrap();
    assert_eq!(d.number, 22);
    assert_eq!(d.state, PrLifecycle::Merged);
    assert_eq!(d.checks.len(), 4);
    assert_eq!(d.checks_summary.passing, 3);
    let kinds: Vec<CommentKind> = d.comments.iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        [
            CommentKind::Comment,
            CommentKind::Thread,
            CommentKind::Review
        ]
    );
    assert!(d.comments.iter().all(|c| c.is_bot));
    assert!(d.loaded_at_ms > 0);

    let calls = s.gh.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    let view = &s.gh.calls_of(&["pr", "view"])[0];
    assert_eq!(
        view.argv,
        [
            "pr",
            "view",
            "22",
            "--repo",
            "octo/cat",
            "--json",
            VIEW_FIELDS
        ]
    );
    let api = &s.gh.calls_of(&["api", "graphql"])[0];
    assert_eq!(api.argv.len(), 10, "{:?}", api.argv);
    assert_eq!(api.argv[2], "-f");
    assert!(api.argv[3].starts_with("query=query($o:String!,$r:String!,$n:Int!)"));
    assert!(api.argv[3].contains("reviewThreads(first:50)"));
    assert_eq!(api.argv[4..], ["-f", "o=octo", "-f", "r=cat", "-F", "n=22"]);
    for call in [view, api] {
        assert!(!call.tty);
        assert!(call.stdin.is_empty());
        for set in [
            "GH_PROMPT_DISABLED=1",
            "GH_NO_UPDATE_NOTIFIER=1",
            "NO_COLOR=1",
        ] {
            assert!(call.env.iter().any(|e| e == set), "{set}");
        }
        assert!(!call.env.iter().any(|e| e.starts_with("GH_REPO=")));
        assert!(!call.env.iter().any(|e| e.starts_with("GH_HOST=")));
    }
    // Each started before the other had answered.
    assert!(
        view.started.unwrap() < api.done.unwrap() && api.started.unwrap() < view.done.unwrap(),
        "one after the other"
    );
    // In the command log, as gh's reads.
    let logged: Vec<Vec<String>> =
        s.h.log
            .since(0)
            .into_iter()
            .filter(|r| r.program == "gh")
            .map(|r| r.args)
            .collect();
    assert_eq!(logged.len(), 2, "{logged:?}");
    assert!(logged.iter().any(|a| a[..2] == ["pr", "view"]));
    assert!(logged.iter().any(|a| a[..2] == ["api", "graphql"]));
}

/// On a GitHub host other than github.com, gh is told the host (`GH_HOST`) for both commands,
/// the repository with its host, and no enterprise token from the environment.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_github_host_is_named_to_both_commands() {
    let s = setup_with("https://ghe.example.ghe.com/octo/cat.git").await;
    s.gh.reply("pr-view", 0, VIEW_PENDING, "");
    s.gh.reply("api-graphql", 0, THREADS, "");
    let d = s.h.m.pr_details(s.id, 25).await.unwrap();
    assert_eq!(d.merge, MergeStatus::Unstable);
    assert!(d.checks.iter().all(|c| c.state == CheckState::Pending));
    let view = &s.gh.calls_of(&["pr", "view"])[0];
    assert_eq!(view.argv[4], "ghe.example.ghe.com/octo/cat");
    for call in s.gh.calls() {
        assert!(
            call.env.iter().any(|e| e == "GH_HOST=ghe.example.ghe.com"),
            "{:?}",
            call.argv
        );
        assert!(
            !call
                .env
                .iter()
                .any(|e| e.starts_with("GH_ENTERPRISE_TOKEN=")
                    || e.starts_with("GITHUB_ENTERPRISE_TOKEN="))
        );
    }
}

/// Asked again within the floor, the last answer is given again (gh is not asked); after
/// it, gh is asked again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loads_are_at_least_the_floor_apart() {
    let s = setup().await;
    s.h.m.set_pr_details_every(Duration::from_millis(600));
    let first = s.h.m.pr_details(s.id, 22).await.unwrap();
    let again = s.h.m.pr_details(s.id, 22).await.unwrap();
    assert_eq!(first, again, "the same answer, as of the same time");
    assert_eq!(s.gh.calls_of(&["pr", "view"]).len(), 1);
    // Another pull request has its own floor.
    s.h.m.pr_details(s.id, 25).await.unwrap();
    assert_eq!(s.gh.calls_of(&["pr", "view"]).len(), 2);
    tokio::time::sleep(Duration::from_millis(700)).await;
    let later = s.h.m.pr_details(s.id, 22).await.unwrap();
    assert!(later.loaded_at_ms > first.loaded_at_ms);
    assert_eq!(s.gh.calls_of(&["pr", "view"]).len(), 3);
    assert_eq!(s.h.m.pr_details_loads(), 3);
}

/// Requests while a load runs wait for its answer: one gh command each, however many asked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_during_a_load_share_it() {
    let s = setup().await;
    s.gh.slow("pr-view", 1);
    let m = s.h.m.clone();
    let id = s.id;
    let asks: Vec<_> = (0..4)
        .map(|_| {
            let m = m.clone();
            tokio::spawn(async move { m.pr_details(id, 22).await })
        })
        .collect();
    let mut answers = Vec::new();
    for ask in asks {
        answers.push(ask.await.unwrap().unwrap());
    }
    assert!(answers.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(s.gh.calls_of(&["pr", "view"]).len(), 1);
    assert_eq!(s.gh.calls_of(&["api", "graphql"]).len(), 1);
}

/// The threads failing alone leaves the rest, with why they are missing; the view failing
/// is the request failing, plainly, and asked again within the floor the next load waits
/// for the floor rather than giving the failure again or asking gh at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failures_are_said_and_retried_after_the_floor() {
    let s = setup().await;
    s.h.m.set_pr_details_every(Duration::from_millis(800));
    s.gh.reply(
        "api-graphql",
        1,
        "",
        "gh: API rate limit exceeded for user ID 1.\n",
    );
    let d = s.h.m.pr_details(s.id, 22).await.unwrap();
    assert_eq!(
        d.threads_error.as_deref(),
        Some("gh: API rate limit exceeded for user ID 1.")
    );
    assert_eq!(d.comments.len(), 2, "the comment and the review summary");
    assert_eq!(d.checks.len(), 4);

    s.gh.reply(
        "pr-view",
        1,
        "",
        "GraphQL: Could not resolve to a PullRequest with the number of 99.\n",
    );
    let err = s.h.m.pr_details(s.id, 99).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Io);
    assert_eq!(
        err.message,
        "Could not load the checks and comments: GraphQL: Could not resolve to a PullRequest \
         with the number of 99."
    );
    s.gh.reply("pr-view", 0, VIEW, "");
    let asked = Instant::now();
    let d = s.h.m.pr_details(s.id, 99).await.unwrap();
    assert!(
        asked.elapsed() >= Duration::from_millis(500),
        "waited for the floor: {:?}",
        asked.elapsed()
    );
    assert_eq!(d.number, 22);
    assert_eq!(s.gh.calls_of(&["pr", "view", "99"]).len(), 2);

    // gh's answer that is not JSON: said plainly, with what it was in the detail.
    s.gh.reply("pr-view", 0, "<html>rate limited</html>", "");
    let err = s.h.m.pr_details(s.id, 7).await.unwrap_err();
    assert_eq!(
        err.message,
        "Could not load the checks and comments: gh's answer could not be read."
    );
    assert!(err.detail.unwrap().contains("could not be read"));
    assert_eq!(
        s.h.m.pr_details(s.id, 0).await.unwrap_err().code,
        ErrorCode::BadRequest
    );
}

/// Without gh, signed out, or not on GitHub: said as for the list, gh never run for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_gh_the_reason_is_said() {
    let s = setup().await;
    s.gh.reply(
        "pr-view",
        4,
        "",
        "To get started with GitHub CLI, please run:  gh auth login\n",
    );
    let err = s.h.m.pr_details(s.id, 22).await.unwrap_err();
    assert!(err.message.contains("not signed in"), "{}", err.message);
    let elsewhere = setup_with("https://git.example.com/octo/cat.git").await;
    // gh has no credentials for that host: it is not GitHub.
    elsewhere.gh.reply("auth-token", 1, "", "");
    let err = elsewhere.h.m.pr_details(elsewhere.id, 3).await.unwrap_err();
    assert!(
        err.message.contains("GitHub repositories only"),
        "{}",
        err.message
    );
    assert!(elsewhere.gh.calls_of(&["pr"]).is_empty());
}

/// A link of a comment or check opens anywhere, but only as an `https://` address with no
/// user name or password, spaces or control characters, and not longer than the limit;
/// recorded as every launch is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn links_open_only_as_https_addresses() {
    let s = setup().await;
    s.h.m.record_launches();
    let programs = Programs {
        editor: None,
        terminal: None,
    };
    let open = |url: String| {
        let m = &s.h.m;
        async move {
            m.open_external(s.id, External::Link, None, Some(&url), programs)
                .await
        }
    };
    for url in [
        "https://docs.coderabbit.ai/management/rate-limits",
        "https://github.com/octo/cat/pull/22#discussion_r1",
    ] {
        open(url.to_owned())
            .await
            .unwrap_or_else(|e| panic!("{url}: {e:?}"));
    }
    let longest = format!("https://example.com/{}", "a".repeat(MAX_LINK_LEN - 20));
    assert_eq!(longest.len(), MAX_LINK_LEN);
    open(longest).await.unwrap();
    for url in [
        "javascript:alert(1)",
        "JAVASCRIPT:alert(1)",
        "data:text/html,<script>alert(1)</script>",
        "http://example.com/",
        "file:///etc/passwd",
        "vscode://open?x",
        "//example.com/x",
        "/relative/path",
        "#anchor",
        "https://user:pw@example.com/",
        "https://example.com/a b",
        "https://example.com/\u{0}",
        "",
    ] {
        let err = open(url.to_owned()).await.expect_err(url);
        assert_eq!(err.code, ErrorCode::BadRequest, "{url}");
        assert!(
            err.message.starts_with("Not opened:"),
            "{url}: {}",
            err.message
        );
    }
    let long = format!("https://example.com/{}", "a".repeat(MAX_LINK_LEN));
    let err = open(long).await.unwrap_err();
    assert_eq!(err.message, "Not opened: the web address is too long.");
    let launches = s.h.m.launches();
    assert_eq!(launches.len(), 3);
    assert_eq!(
        launches[0].last().unwrap(),
        "https://docs.coderabbit.ai/management/rate-limits"
    );
    // A path is not a link.
    let err =
        s.h.m
            .open_external(
                s.id,
                External::Link,
                Some(std::path::Path::new("/tmp")),
                Some("https://example.com/"),
                programs,
            )
            .await
            .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
}
