//! The Pull request tab's writes (`pr_reply`, `pr_resolve`, `pr_comment`) through `gh`, against
//! the recording fake `gh` on the tool `PATH` (`gg_core::testutil::FakeGh`): no test reaches a
//! network host, and the real `gh` never runs. Each write names the repository `pr_details`
//! reads (a fork's pull request too), carries the user's text to `gh` as data on its stdin,
//! says gh's failures, and is refused without `gh` when its request is bad.
#![cfg(unix)]

mod common;

use std::time::Duration;

use common::harness;
use gg_core::dto::{OpOutcome, OpRequest, OpStatus};
use gg_core::error::ErrorCode;
use gg_core::git::runner::CmdKind;
use gg_core::testutil::{FakeGh, FakeGhCall, TestRepo};
use serde_json::{Value, json};

const VIEW: &str = include_str!("fixtures/gh/pr22-view.json");
const THREADS: &str = include_str!("fixtures/gh/pr22-threads.json");

/// A body that would do harm if it were ever taken as anything but text: quotes, backticks,
/// a command substitution, newlines, a leading `@` (which `gh api -F` reads as a file) and
/// a leading `-` on a line (an option, were it an argument).
const NASTY: &str = "@/etc/passwd\n-f query=mutation{x}\n\"quoted\" 'single' `id` $(touch /tmp/gg-pwned) ${HOME}\n\\n not a newline\n\u{1F600} done";

struct Setup {
    repo: TestRepo,
    h: common::Harness,
    id: u32,
    gh: FakeGh,
}

/// A repository whose remotes are `remotes` (never fetched from), opened with the fake `gh`.
async fn setup_with(remotes: &[(&str, &str)]) -> Setup {
    let repo = TestRepo::new();
    repo.commit("one");
    for (name, url) in remotes {
        repo.git(&["remote", "add", name, url]);
    }
    let gh = FakeGh::new();
    let h = harness().await;
    h.m.set_tool_path(Some(gh.path()));
    let id = h.open(repo.path()).await.repo_id;
    Setup { repo, h, id, gh }
}

async fn setup() -> Setup {
    setup_with(&[("origin", "https://github.com/octo/cat.git")]).await
}

fn reply(number: u32, thread: &str, body: &str) -> OpRequest {
    OpRequest::PrReply {
        number,
        thread_id: thread.into(),
        body: body.into(),
    }
}

fn resolve(number: u32, thread: &str, resolved: bool) -> OpRequest {
    OpRequest::PrResolve {
        number,
        thread_id: thread.into(),
        resolved,
    }
}

fn comment(number: u32, body: &str) -> OpRequest {
    OpRequest::PrComment {
        number,
        body: body.into(),
    }
}

/// What every `gh` command of the app gets, and nothing typed by the user in its arguments.
fn assert_as_every_gh(call: &FakeGhCall) {
    assert!(!call.tty, "no terminal");
    for set in [
        "GH_PROMPT_DISABLED=1",
        "GH_NO_UPDATE_NOTIFIER=1",
        "NO_COLOR=1",
    ] {
        assert!(call.env.iter().any(|e| e == set), "{set}");
    }
    assert!(!call.env.iter().any(|e| e.starts_with("GH_REPO=")));
    for arg in &call.argv {
        assert!(!arg.contains("passwd") && !arg.contains("$("), "{arg}");
    }
}

fn the_one(gh: &FakeGh, prefix: &[&str]) -> FakeGhCall {
    let calls = gh.calls_of(prefix);
    assert_eq!(calls.len(), 1, "{:?}", gh.calls());
    calls.into_iter().next().unwrap()
}

/// The `gh` commands an operation's outcome logged, as writes of that operation.
fn logged_gh(s: &Setup, out: &OpOutcome) -> Vec<Vec<String>> {
    s.h.log
        .since(0)
        .into_iter()
        .filter(|r| r.program == "gh" && r.op_id == Some(out.op_id))
        .inspect(|r| assert_eq!(r.kind, CmdKind::Write, "{:?}", r.args))
        .map(|r| r.args)
        .collect()
}

/// A reply: `gh api graphql --input -`, its query and variables (the thread's id, the body
/// exactly as typed) a JSON request on stdin; nothing of the body in an argument or the
/// command log. Run from the main worktree, logged as the operation's write.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_goes_to_gh_as_data_on_stdin() {
    let s = setup().await;
    s.gh.reply(
        "api-graphql",
        0,
        r#"{"data":{"addPullRequestReviewThreadReply":{"comment":{"id":"PRRC_1","url":"https://github.com/octo/cat/pull/22#discussion_r1"}}}}"#,
        "",
    );
    let out =
        s.h.run(s.id, reply(22, "PRRT_kwDOVsODgM67XTTJ", NASTY))
            .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(out.message, "Replied on pull request #22");
    assert_eq!(out.title, "Reply on pull request #22");

    let call = the_one(&s.gh, &["api", "graphql"]);
    assert_eq!(call.argv, ["api", "graphql", "--input", "-"]);
    assert_as_every_gh(&call);
    assert_eq!(
        std::fs::canonicalize(&call.cwd).unwrap(),
        std::fs::canonicalize(s.repo.path()).unwrap()
    );
    let sent: Value = serde_json::from_slice(&call.stdin).expect("a JSON request");
    let query = sent["query"].as_str().unwrap();
    assert!(
        query.starts_with("mutation($thread:ID!,$body:String!){addPullRequestReviewThreadReply("),
        "{query}"
    );
    assert!(query.contains("pullRequestReviewThreadId:$thread,body:$body"));
    assert!(!query.contains("passwd"), "the body is never in the query");
    assert_eq!(
        sent["variables"],
        json!({"thread": "PRRT_kwDOVsODgM67XTTJ", "body": NASTY})
    );
    assert!(!std::path::Path::new("/tmp/gg-pwned").exists());

    let logged = logged_gh(&s, &out);
    assert_eq!(logged, [["api", "graphql", "--input", "-"]]);
}

/// Resolve and unresolve: `gh api graphql -f query=<mutation> -f thread=<id>` (strings taken as
/// they are), nothing on stdin.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resolving_and_unresolving_name_the_thread_as_a_string() {
    let s = setup().await;
    s.gh.reply(
        "api-graphql",
        0,
        r#"{"data":{"resolveReviewThread":{"thread":{"id":"PRRT_a","isResolved":true}}}}"#,
        "",
    );
    let out = s.h.run(s.id, resolve(22, "PRRT_a", true)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(out.message, "Resolved a conversation on pull request #22");
    let out = s.h.run(s.id, resolve(22, "PRRT_a", false)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(out.message, "Unresolved a conversation on pull request #22");
    assert_eq!(out.title, "Unresolve a conversation on pull request #22");

    let calls = s.gh.calls_of(&["api", "graphql"]);
    assert_eq!(calls.len(), 2);
    let want = |mutation: &str| {
        vec![
            "api".to_owned(),
            "graphql".to_owned(),
            "-f".to_owned(),
            format!(
                "query=mutation($thread:ID!){{{mutation}(input:{{threadId:$thread}}){{thread{{id isResolved}}}}}}"
            ),
            "-f".to_owned(),
            "thread=PRRT_a".to_owned(),
        ]
    };
    assert_eq!(calls[0].argv, want("resolveReviewThread"));
    assert_eq!(calls[1].argv, want("unresolveReviewThread"));
    for call in &calls {
        assert_as_every_gh(call);
        assert!(call.stdin.is_empty());
    }
}

/// A comment: `gh pr comment <n> --repo <repo> --body-file -`, the body on stdin byte for byte.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_comment_goes_to_gh_pr_comment_on_stdin() {
    let s = setup().await;
    s.gh.reply(
        "pr-comment",
        0,
        "https://github.com/octo/cat/pull/22#issuecomment-1\n",
        "",
    );
    let out = s.h.run(s.id, comment(22, NASTY)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(out.message, "Commented on pull request #22");
    let call = the_one(&s.gh, &["pr", "comment"]);
    assert_eq!(
        call.argv,
        [
            "pr",
            "comment",
            "22",
            "--repo",
            "octo/cat",
            "--body-file",
            "-"
        ]
    );
    assert_eq!(call.stdin, NASTY.as_bytes());
    assert_as_every_gh(&call);
    assert_eq!(
        logged_gh(&s, &out),
        [[
            "pr",
            "comment",
            "22",
            "--repo",
            "octo/cat",
            "--body-file",
            "-"
        ]]
    );
}

/// A body that starts with `@` or `-` reaches gh as it is: in the JSON request, and as the
/// comment's stdin, never as an argument gh could read a file from or take as an option.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_starting_with_at_or_dash_is_sent_literally() {
    let s = setup().await;
    for body in ["@README.md", "-F body=@/etc/passwd", "--help"] {
        let out = s.h.run(s.id, reply(22, "PRRT_a", body)).await;
        assert_eq!(out.status, OpStatus::Done, "{body}: {out:#?}");
        let out = s.h.run(s.id, comment(22, body)).await;
        assert_eq!(out.status, OpStatus::Done, "{body}: {out:#?}");
    }
    let replies = s.gh.calls_of(&["api", "graphql"]);
    let comments = s.gh.calls_of(&["pr", "comment"]);
    for (i, body) in ["@README.md", "-F body=@/etc/passwd", "--help"]
        .iter()
        .enumerate()
    {
        let sent: Value = serde_json::from_slice(&replies[i].stdin).unwrap();
        assert_eq!(sent["variables"]["body"], *body);
        assert_eq!(replies[i].argv, ["api", "graphql", "--input", "-"]);
        assert_eq!(comments[i].stdin, body.as_bytes());
        assert_eq!(comments[i].argv.last().unwrap(), "-");
        assert!(!comments[i].argv.iter().any(|a| a == body));
    }
}

/// A fork's pull request is written where it is read: the base repository (`upstream`, as gh
/// and the list take it), not the fork the branch is pushed to.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forks_pull_request_is_written_to_the_base_repository() {
    let s = setup_with(&[
        ("origin", "https://github.com/ada/cat.git"),
        ("upstream", "https://github.com/octo/cat.git"),
    ])
    .await;
    s.gh.reply("pr-view", 0, VIEW, "");
    s.gh.reply("api-graphql", 0, THREADS, "");
    s.h.m.pr_details(s.id, 22).await.unwrap();
    let read = the_one(&s.gh, &["pr", "view"]);
    assert_eq!(read.argv[3..5], ["--repo", "octo/cat"]);
    let out = s.h.run(s.id, comment(22, "Thanks!")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(
        the_one(&s.gh, &["pr", "comment"]).argv[3..5],
        ["--repo", "octo/cat"]
    );

    // On another GitHub host: gh is told the host for the write as for the read, and given
    // no enterprise token from the environment.
    let ghe = setup_with(&[("origin", "https://ghe.example.ghe.com/octo/cat.git")]).await;
    let out = ghe.h.run(ghe.id, reply(3, "PRRT_b", "Fixed.")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    let out = ghe.h.run(ghe.id, comment(3, "Fixed.")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(
        the_one(&ghe.gh, &["pr", "comment"]).argv[3..5],
        ["--repo", "ghe.example.ghe.com/octo/cat"]
    );
    for call in ghe.gh.calls() {
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

/// Done, the pull request's details are forgotten: the next `pr_details` asks gh at once,
/// however long the floor between loads; failed, they are not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_done_makes_the_next_load_ask_gh() {
    let s = setup().await;
    s.h.m.set_pr_details_every(Duration::from_secs(3600));
    s.gh.reply("pr-view", 0, VIEW, "");
    s.gh.reply("api-graphql", 0, THREADS, "");
    s.h.m.pr_details(s.id, 22).await.unwrap();
    s.h.m.pr_details(s.id, 22).await.unwrap();
    assert_eq!(s.gh.calls_of(&["pr", "view"]).len(), 1, "within the floor");

    s.gh.reply("pr-comment", 1, "", "GraphQL: Something went wrong.\n");
    let out = s.h.run(s.id, comment(22, "Hi")).await;
    assert_eq!(out.status, OpStatus::Failed);
    s.h.m.pr_details(s.id, 22).await.unwrap();
    assert_eq!(
        s.gh.calls_of(&["pr", "view"]).len(),
        1,
        "not after a failure"
    );

    s.gh.reply("pr-comment", 0, "", "");
    let out = s.h.run(s.id, comment(22, "Hi")).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    s.h.m.pr_details(s.id, 22).await.unwrap();
    assert_eq!(s.gh.calls_of(&["pr", "view"]).len(), 2, "asked again");
    let out = s.h.run(s.id, resolve(22, "PRRT_a", true)).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    s.h.m.pr_details(s.id, 22).await.unwrap();
    assert_eq!(s.gh.calls_of(&["pr", "view"]).len(), 3, "asked again");
    // Another pull request's are kept.
    s.h.m.pr_details(s.id, 25).await.unwrap();
    let out = s.h.run(s.id, comment(22, "Hi")).await;
    assert_eq!(out.status, OpStatus::Done);
    s.h.m.pr_details(s.id, 25).await.unwrap();
    assert_eq!(s.gh.calls_of(&["pr", "view", "25"]).len(), 1);
}

/// gh's failures are said plainly, with its output as the detail: an error it exits with, a
/// GraphQL error in an answer it exits 0 with, signed out, not installed, no answer in time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failures_are_said() {
    let s = setup().await;
    s.gh.reply(
        "api-graphql",
        1,
        "",
        "gh: Could not resolve to a node with the global id of 'PRRT_gone'.\n",
    );
    let out = s.h.run(s.id, reply(22, "PRRT_gone", "Hi")).await;
    assert_eq!(out.status, OpStatus::Failed);
    assert_eq!(
        out.message,
        "gh could not post the reply. gh: Could not resolve to a node with the global id of \
         'PRRT_gone'."
    );
    assert!(out.detail.unwrap().contains("PRRT_gone"));
    assert!(out.log_from > 0, "the command is in the log");

    s.gh.reply(
        "api-graphql",
        0,
        r#"{"data":{"resolveReviewThread":null},"errors":[{"type":"FORBIDDEN","message":"Resource not accessible by integration"}]}"#,
        "",
    );
    let out = s.h.run(s.id, resolve(22, "PRRT_a", true)).await;
    assert_eq!(out.status, OpStatus::Failed);
    assert_eq!(
        out.message,
        "gh could not resolve the conversation. Resource not accessible by integration."
    );

    s.gh.reply(
        "pr-comment",
        4,
        "",
        "To get started with GitHub CLI, please run:  gh auth login\n",
    );
    let out = s.h.run(s.id, comment(22, "Hi")).await;
    assert_eq!(out.status, OpStatus::Failed);
    assert!(
        out.message
            .starts_with("Nothing was sent. The GitHub CLI (gh) is not signed in."),
        "{}",
        out.message
    );

    // No answer in time: given up on, said so.
    s.h.m.set_gh_timeout(Duration::from_secs(1));
    s.gh.reply("pr-comment", 0, "", "");
    s.gh.slow("pr-comment", 5);
    let out = s.h.run(s.id, comment(22, "Hi")).await;
    assert_eq!(out.status, OpStatus::Failed);
    assert_eq!(
        out.message,
        "gh could not post the comment. gh did not answer within 1 s."
    );
    s.gh.fast("pr-comment");

    // Not installed: nothing runs.
    let empty = tempfile::tempdir().unwrap();
    s.h.m
        .set_tool_path(Some(empty.path().as_os_str().to_owned()));
    let before = s.gh.calls().len();
    let out = s.h.run(s.id, comment(22, "Hi")).await;
    assert_eq!(out.status, OpStatus::Failed);
    assert!(
        out.message.contains("GitHub CLI (gh), which was not found"),
        "{}",
        out.message
    );
    assert_eq!(s.gh.calls().len(), before);
}

/// Not on GitHub: refused, gh never asked to write.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn not_on_github_nothing_is_sent() {
    let s = setup_with(&[("origin", "https://git.example.com/octo/cat.git")]).await;
    // gh has no credentials for that host: it is not GitHub.
    s.gh.reply("auth-token", 1, "", "");
    let out = s.h.run(s.id, reply(3, "PRRT_a", "Hi")).await;
    assert_eq!(out.status, OpStatus::Failed);
    assert!(
        out.message.starts_with("Nothing was sent. ")
            && out.message.contains("GitHub repositories only"),
        "{}",
        out.message
    );
    assert!(s.gh.calls_of(&["api"]).is_empty());
}

/// A bad request is refused before it is queued, gh never run: a blank body, pull request 0, a
/// thread id GitHub would never give (option-like, with spaces or quotes, too long).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bad_requests_are_refused_without_gh() {
    let s = setup().await;
    let refused = |request: OpRequest| {
        let m = s.h.m.clone();
        let id = s.id;
        async move { m.run_op(id, request, &[]).await.unwrap_err() }
    };
    for body in ["", "   ", "\n\t\n"] {
        let e = refused(reply(22, "PRRT_a", body)).await;
        assert_eq!(e.code, ErrorCode::BadRequest);
        assert_eq!(e.message, "Nothing was sent: the comment is empty.");
        assert_eq!(refused(comment(22, body)).await.code, ErrorCode::BadRequest);
    }
    assert_eq!(refused(comment(0, "Hi")).await.code, ErrorCode::BadRequest);
    let long = "A".repeat(201);
    for thread in [
        "",
        "-f",
        "PRRT a",
        "PRRT\"}",
        "$(id)",
        "PRRT_\n",
        long.as_str(),
    ] {
        let e = refused(resolve(22, thread, true)).await;
        assert_eq!(e.code, ErrorCode::BadRequest, "{thread:?}");
        assert_eq!(
            refused(reply(22, thread, "Hi")).await.code,
            ErrorCode::BadRequest
        );
    }
    assert!(s.gh.calls().is_empty(), "{:?}", s.gh.calls());
}

/// The details carry each thread's id and what the user may do to it (GitHub's
/// `viewerCanReply`, `viewerCanResolve`, `viewerCanUnresolve`, asked for in the threads'
/// query), and whether a comment can be added (not when archived; when locked, only with write
/// access).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_details_say_what_the_user_may_do() {
    let s = setup().await;
    s.h.m.set_pr_details_every(Duration::ZERO);
    let threads = |locked: bool, permission: &str, archived: bool| {
        json!({"data": {"repository": {"isArchived": archived, "viewerPermission": permission,
            "pullRequest": {"locked": locked, "reviewThreads": {"pageInfo": {"hasNextPage": false},
            "nodes": [
                {"id": "PRRT_open", "isResolved": false, "path": "a.rs", "line": 3,
                 "viewerCanReply": true, "viewerCanResolve": true, "viewerCanUnresolve": false,
                 "comments": {"nodes": [{"id": "c1", "body": "Why?", "createdAt": "2026-11-24T12:00:00Z",
                    "author": {"login": "ada", "__typename": "User"}}]}},
                {"id": "PRRT_done", "isResolved": true, "path": "b.rs", "line": 9,
                 "viewerCanReply": true, "viewerCanResolve": false, "viewerCanUnresolve": true,
                 "comments": {"nodes": [{"id": "c2", "body": "Typo.", "createdAt": "2026-11-24T12:01:00Z",
                    "author": {"login": "ada", "__typename": "User"}}]}},
                {"id": "PRRT_theirs", "isResolved": false, "path": "c.rs", "line": 1,
                 "viewerCanReply": false, "viewerCanResolve": false, "viewerCanUnresolve": false,
                 "comments": {"nodes": [{"id": "c3", "body": "Hm.", "createdAt": "2026-11-24T12:02:00Z",
                    "author": {"login": "ada", "__typename": "User"}}]}},
                {"id": "not an id", "isResolved": false, "path": "d.rs", "line": 1,
                 "viewerCanReply": true, "viewerCanResolve": true,
                 "comments": {"nodes": [{"id": "c4", "body": "Odd.", "createdAt": "2026-11-24T12:03:00Z",
                    "author": {"login": "ada", "__typename": "User"}}]}}
            ]}}}}})
        .to_string()
    };
    s.gh.reply("pr-view", 0, VIEW, "");
    s.gh.reply("api-graphql", 0, &threads(false, "READ", false), "");
    let d = s.h.m.pr_details(s.id, 22).await.unwrap();
    let api = the_one(&s.gh, &["api", "graphql"]);
    for field in [
        "viewerCanReply",
        "viewerCanResolve",
        "viewerCanUnresolve",
        "locked",
        "isArchived",
        "viewerPermission",
    ] {
        assert!(api.argv[3].contains(field), "{field}");
    }
    let t: Vec<_> = d
        .comments
        .iter()
        .filter_map(|c| c.thread.as_ref())
        .map(|t| (t.id.as_str(), t.can_reply, t.can_resolve, t.can_unresolve))
        .collect();
    assert_eq!(
        t,
        [
            ("PRRT_open", true, true, false),
            ("PRRT_done", true, false, true),
            ("PRRT_theirs", false, false, false),
            ("", false, false, false),
        ]
    );
    assert!(d.can_comment);
    let v = serde_json::to_value(&d).unwrap();
    let first = v["comments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == "thread")
        .unwrap();
    assert_eq!(first["thread"]["id"], "PRRT_open");
    assert_eq!(first["thread"]["canReply"], true);
    assert_eq!(first["thread"]["canResolve"], true);
    assert_eq!(first["thread"]["canUnresolve"], false);
    assert_eq!(v["canComment"], true);

    for (locked, permission, archived, can) in [
        (true, "READ", false, false),
        (true, "TRIAGE", false, false),
        (true, "WRITE", false, true),
        (true, "ADMIN", false, true),
        (false, "WRITE", true, false),
    ] {
        s.gh.reply("api-graphql", 0, &threads(locked, permission, archived), "");
        let d = s.h.m.pr_details(s.id, 22).await.unwrap();
        assert_eq!(d.can_comment, can, "{locked} {permission} {archived}");
    }
    // The threads not read: a comment is offered (it says why, if it cannot be made).
    s.gh.reply("api-graphql", 1, "", "gh: rate limited\n");
    let d = s.h.m.pr_details(s.id, 22).await.unwrap();
    assert!(d.can_comment);
}
