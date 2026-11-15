//! Security: which hosts `gh` is run for, and which tokens of the user's environment reach
//! it. A host is GitHub by name only as github.com or `*.ghe.com`; any other host only when
//! `gh` has credentials stored for exactly that host (`gh auth token --hostname <host>`, its
//! output never read), so a repository whose remote names some `github.example.net` cannot
//! make the app point `gh` (and an enterprise token from the environment) at it.
//!
//! Alone in its test binary: it sets the token variables in the process environment, which
//! every child would inherit, before anything starts.
#![cfg(unix)]

mod common;

use std::time::Duration;

use common::{TIMEOUT, harness};
use gg_core::dto::{PrStatus, PrUnavailable, RepoSnapshot};
use gg_core::testutil::{FakeGh, TestRepo};

const TOKENS: [&str; 4] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
];
const ENTERPRISE_TOKENS: [&str; 2] = ["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"];
/// What the fake `gh auth token` prints: the app must never see it.
const STORED_TOKEN: &str = "gho_STORED_TOKEN_TEXT_9f8e7d";

fn has(env: &[String], var: &str) -> bool {
    env.iter().any(|e| e.starts_with(&format!("{var}=")))
}

/// A repository whose `origin` is `url` (a local bare repository underneath, by insteadOf).
fn repo_on(url: &str) -> (TestRepo, TestRepo) {
    let repo = TestRepo::new();
    repo.commit("one");
    let origin = repo.clone_as_remote();
    repo.git(&["remote", "set-url", "origin", url]);
    let key = format!("url.{}.insteadOf", origin.path().display());
    repo.git(&["config", "--add", &key, url]);
    (repo, origin)
}

async fn load(h: &common::Harness, id: u32) -> RepoSnapshot {
    let before = h.m.prs_loads(id);
    h.m.refresh_by_user(id).await.unwrap();
    tokio::time::timeout(TIMEOUT, h.m.wait_prs_loads(id, before))
        .await
        .expect("the pull requests load");
    h.m.snapshot(id).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tokens_reach_only_hosts_gh_has_credentials_stored_for() {
    for var in TOKENS {
        // SAFETY: the only test of this binary, and nothing has started yet that reads the
        // environment or starts a child (the runtime's workers are idle).
        unsafe { std::env::set_var(var, format!("env-value-of-{var}")) };
    }
    // A repository and host gh would otherwise take from the environment (Task 8 review:
    // every command names the repository the app chose; GH_HOST only as the app sets it).
    for (var, value) in [("GH_REPO", "evil/repo"), ("GH_HOST", "evil.example")] {
        // SAFETY: as above.
        unsafe { std::env::set_var(var, value) };
    }
    let gh = FakeGh::new();
    let h = harness().await;
    h.m.set_tool_path(Some(gh.path()));
    h.m.set_prs_every(Duration::ZERO);

    // A host named like GitHub, for which gh has no credentials stored: not GitHub. Only the
    // check ran, and it saw none of the token variables.
    gh.reply(
        "auth-token",
        1,
        "",
        "no oauth token found for github.example.net\n",
    );
    let (net, _net_origin) = repo_on("https://github.example.net/team/app.git");
    let net_id = h.open(net.path()).await.repo_id;
    let snap = load(&h, net_id).await;
    assert_eq!(snap.prs.status, PrStatus::Unavailable);
    assert_eq!(snap.prs.reason, Some(PrUnavailable::NotGithub));
    let suggested = h.m.pr_suggest(net_id, "main").await.unwrap();
    assert_eq!(suggested.compare_url, None, "no compare page for it");
    load(&h, net_id).await;
    let calls = gh.calls();
    let argv: Vec<&[String]> = calls.iter().map(|c| c.argv.as_slice()).collect();
    assert_eq!(
        argv,
        [["auth", "token", "--hostname", "github.example.net"]],
        "asked once per host per run; nothing else ran"
    );
    for var in TOKENS {
        assert!(!has(&calls[0].env, var), "{var} reached the check");
    }
    assert!(!has(&calls[0].env, "GH_HOST"));

    // A host gh has credentials stored for: GitHub; GH_HOST names it, and no enterprise
    // token of the environment goes with any command.
    gh.reply("auth-token", 0, &format!("{STORED_TOKEN}\n"), "");
    gh.reply("pr-list", 0, "[]", "");
    gh.reply(
        "repo-view",
        0,
        r#"{"defaultBranchRef":{"name":"main"}}"#,
        "",
    );
    let (org, _org_origin) = repo_on("https://github.example.org/team/app.git");
    let org_id = h.open(org.path()).await.repo_id;
    let snap = load(&h, org_id).await;
    assert_eq!(snap.prs.status, PrStatus::Ready, "{:?}", snap.prs);
    let suggested = h.m.pr_suggest(org_id, "main").await.unwrap();
    assert!(
        suggested
            .compare_url
            .as_deref()
            .is_some_and(|u| u.starts_with("https://github.example.org/team/app/compare/"))
    );
    let checks = gh.calls_of(&["auth", "token"]);
    assert_eq!(checks.len(), 2, "one per host");
    assert_eq!(checks[1].argv[3], "github.example.org");
    for var in TOKENS {
        assert!(!has(&checks[1].env, var), "{var} reached the check");
    }
    let org_calls: Vec<_> = gh
        .calls()
        .into_iter()
        .filter(|c| c.argv[0] != "auth")
        .collect();
    assert!(org_calls.len() >= 2, "listed, and asked for the suggestion");
    for call in &org_calls {
        assert!(
            call.env.iter().any(|e| e == "GH_HOST=github.example.org"),
            "{:?}",
            call.argv
        );
        for var in ENTERPRISE_TOKENS {
            assert!(!has(&call.env, var), "{var} went to {:?}", call.argv);
        }
    }

    // github.com: no check, no GH_HOST, and the user's own GH_TOKEN left alone.
    let (com, _com_origin) = repo_on("https://github.com/octo/cat.git");
    let com_id = h.open(com.path()).await.repo_id;
    let before = gh.calls().len();
    load(&h, com_id).await;
    let com_calls = &gh.calls()[before..];
    assert_eq!(com_calls.len(), 1);
    assert_eq!(com_calls[0].argv[..2], ["pr", "list"]);
    assert!(!has(&com_calls[0].env, "GH_HOST"));
    assert!(
        com_calls[0]
            .env
            .iter()
            .any(|e| e == "GH_TOKEN=env-value-of-GH_TOKEN")
    );

    // No command took GH_REPO from the environment, nor GH_HOST but as the app set it.
    for call in gh.calls() {
        assert!(!has(&call.env, "GH_REPO"), "{:?}", call.argv);
        assert!(
            !call.env.iter().any(|e| e == "GH_HOST=evil.example"),
            "{:?}",
            call.argv
        );
        if call.argv[0] != "auth" {
            assert!(
                call.argv
                    .iter()
                    .any(|a| a == "--repo" || a.ends_with("team/app"))
            );
        }
    }

    // The token gh printed for the check reached nothing of the app's.
    let log = h.log.since(0);
    let check_records: Vec<_> = log
        .iter()
        .filter(|r| r.program == "gh" && r.args.first().is_some_and(|a| a == "auth"))
        .collect();
    assert_eq!(check_records.len(), 2, "the checks are logged");
    for r in &check_records {
        assert_eq!(r.args, ["auth", "token", "--hostname", &r.args[3]]);
        assert!(r.stdout.is_empty() && r.stderr.is_empty(), "{r:?}");
    }
    let everything = format!(
        "{}\n{}\n{}\n{}",
        serde_json::to_string(&log).unwrap(),
        serde_json::to_string(&h.m.snapshot(org_id).await.unwrap()).unwrap(),
        serde_json::to_string(&h.m.snapshot(net_id).await.unwrap()).unwrap(),
        serde_json::to_string(&h.sink.events()).unwrap(),
    );
    assert!(!everything.contains(STORED_TOKEN));
}
