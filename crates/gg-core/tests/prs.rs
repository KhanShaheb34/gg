//! Pull requests through `gh` (Task 8, Review Focus 4), against a recording fake `gh` on the
//! tool `PATH` (`gg_core::testutil::FakeGh`): no test reaches a network host. The
//! repositories' remotes are GitHub addresses that `url.<bare>.insteadOf` sends to a local
//! bare repository, so fetches and pushes stay on disk while the app sees GitHub.
#![cfg(unix)]

mod common;

use std::time::Duration;

use common::{TIMEOUT, harness};
use gg_core::dto::{
    OpHint, OpRequest, OpStatus, PrStatus, PrUnavailable, PullRequestState, RefKind, RepoSnapshot,
};
use gg_core::testutil::{FakeGh, TestRepo};
use serde_json::{Value, json};

const FIELDS: &str =
    "number,title,url,isDraft,headRefName,baseRefName,headRepositoryOwner,isCrossRepository";
const GH_URL: &str = "https://github.com/octo/cat.git";

/// A repository whose `origin` is `GH_URL` (a local bare repository underneath), opened
/// with the fake `gh` on the tool `PATH`.
struct Setup {
    repo: TestRepo,
    _origin: TestRepo,
    h: common::Harness,
    id: u32,
    gh: FakeGh,
}

/// Sends `url` to the bare repository `bare` for git, which the app still sees as `url`.
fn behind(repo: &TestRepo, url: &str, bare: &TestRepo) {
    let key = format!("url.{}.insteadOf", bare.path().display());
    repo.git(&["config", "--add", &key, url]);
}

async fn setup_with(url: &str) -> Setup {
    let repo = TestRepo::new();
    repo.commit("one");
    let origin = repo.clone_as_remote();
    repo.git(&["remote", "set-url", "origin", url]);
    behind(&repo, url, &origin);
    let gh = FakeGh::new();
    let h = harness().await;
    h.m.set_tool_path(Some(gh.path()));
    let id = h.open(repo.path()).await.repo_id;
    Setup {
        repo,
        _origin: origin,
        h,
        id,
        gh,
    }
}

async fn setup() -> Setup {
    setup_with(GH_URL).await
}

fn pr(number: u32, head: &str, owner: &str, cross: bool, draft: bool) -> Value {
    json!({
        "number": number,
        "title": format!("PR {number}"),
        "url": format!("https://github.com/octo/cat/pull/{number}"),
        "isDraft": draft,
        "headRefName": head,
        "baseRefName": "main",
        "headRepositoryOwner": {"id": "X", "login": owner},
        "isCrossRepository": cross,
    })
}

fn list(prs: &[Value]) -> String {
    Value::Array(prs.to_vec()).to_string()
}

impl Setup {
    /// Loads the pull requests as a refresh the user asks for does, and waits for the load.
    async fn load(&self) -> RepoSnapshot {
        let before = self.h.m.prs_loads(self.id);
        self.h.m.refresh_by_user(self.id).await.unwrap();
        tokio::time::timeout(TIMEOUT, self.h.m.wait_prs_loads(self.id, before))
            .await
            .expect("the pull requests load");
        self.h.m.snapshot(self.id).await.unwrap()
    }

    /// The `repo-error`s about pull requests so far.
    fn notices(&self) -> Vec<Value> {
        self.h
            .sink
            .events()
            .into_iter()
            .filter(|(n, v)| n == "repo-error" && v.get("prs").is_some())
            .map(|(_, v)| v)
            .collect()
    }

    /// The labels of every row, flattened.
    async fn labels(&self) -> Vec<gg_core::dto::RefLabel> {
        for _ in 0..100 {
            tokio::time::timeout(TIMEOUT, self.h.m.wait_loaded(self.id))
                .await
                .unwrap();
            let snap = self.h.m.snapshot(self.id).await.unwrap();
            if let Ok(page) = self.h.m.rows(self.id, snap.generation, 0, 1000).await {
                return page.rows.into_iter().flat_map(|r| r.labels).collect();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("rows could not be read");
    }
}

/// Brief, Review Focus 4: open pull requests are listed with exactly `gh pr list --state
/// open --limit 1000 --json …`, in the main worktree, without a terminal, with prompts and
/// the update notice off, no colour, nothing on stdin, and no token set by the app; each
/// branch with one gets a marker (local and remote label, worktree), a draft as a draft.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_pull_requests_are_listed_and_marked_on_their_branches() {
    let s = setup().await;
    s.repo.branch("feature");
    s.repo.branch("wip");
    s.repo.git(&["push", "-q", "origin", "feature", "wip"]);
    s.repo.git(&["fetch", "-q", "origin"]);
    s.gh.reply(
        "pr-list",
        0,
        &list(&[
            pr(12, "feature", "octo", false, false),
            pr(13, "wip", "octo", false, true),
        ]),
        "",
    );
    let snap = s.load().await;
    assert_eq!(snap.prs.status, PrStatus::Ready, "{:?}", snap.prs);
    assert_eq!(snap.prs.reason, None);
    assert!(snap.prs.loaded_at_ms.is_some());
    assert!(!snap.prs.partial);

    let calls = s.gh.calls();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(
        call.argv,
        [
            "pr", "list", "--repo", "octo/cat", "--state", "open", "--limit", "1000", "--json",
            FIELDS
        ]
    );
    let root = dunce::canonicalize(s.repo.path()).unwrap();
    assert_eq!(
        dunce::canonicalize(&call.cwd).unwrap(),
        root,
        "runs in the main worktree"
    );
    assert!(!call.tty, "no terminal");
    assert!(call.stdin.is_empty(), "stdin null");
    for set in [
        "GH_PROMPT_DISABLED=1",
        "GH_NO_UPDATE_NOTIFIER=1",
        "NO_COLOR=1",
    ] {
        assert!(call.env.iter().any(|e| e == set), "{set}: {:?}", call.env);
    }
    assert!(
        !call.env.iter().any(|e| e.starts_with("GH_HOST=")),
        "github.com: no GH_HOST"
    );
    for token in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if std::env::var_os(token).is_none() {
            assert!(
                !call.env.iter().any(|e| e.starts_with(&format!("{token}="))),
                "{token} is never set by the app"
            );
        }
    }

    let labels = s.labels().await;
    let mark = |kind: RefKind, name: &str| {
        labels
            .iter()
            .find(|l| l.kind == kind && l.name == name)
            .and_then(|l| l.pr.as_ref())
            .map(|p| (p.number, p.state))
    };
    assert_eq!(
        mark(RefKind::Branch, "feature"),
        Some((12, PullRequestState::Open))
    );
    assert_eq!(
        mark(RefKind::Branch, "wip"),
        Some((13, PullRequestState::Draft))
    );
    assert_eq!(mark(RefKind::Branch, "main"), None);
    // The log is in the command log as gh's, masked like git's.
    let logged: Vec<_> =
        s.h.log
            .since(0)
            .into_iter()
            .filter(|r| r.program == "gh")
            .collect();
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].args[0], "pr");

    // A worktree on a branch with one shows it, in full.
    let wt = s.repo.add_worktree("feature");
    s.h.m.refresh(s.id).await.unwrap();
    let snap = s.h.m.snapshot(s.id).await.unwrap();
    let main = snap.worktrees.iter().find(|w| w.is_main).unwrap();
    assert_eq!(main.pr, None, "main has none");
    let root = |p: &std::path::Path| dunce::canonicalize(p).unwrap();
    let on_feature = snap
        .worktrees
        .iter()
        .find(|w| root(std::path::Path::new(&w.path)) == root(&wt))
        .expect("the worktree is listed");
    assert_eq!(on_feature.branch.as_deref(), Some("feature"));
    let shown = on_feature.pr.as_ref().expect("its pull request");
    assert_eq!(
        (
            shown.number,
            shown.title.as_str(),
            shown.url.as_str(),
            shown.state,
            shown.head.as_str(),
            shown.base.as_str()
        ),
        (
            12,
            "PR 12",
            "https://github.com/octo/cat/pull/12",
            PullRequestState::Open,
            "feature",
            "main"
        )
    );
}

/// Forks: a pull request's head is matched to a branch only by its name AND the owner of the
/// remote the branch is pushed to; another owner's branch of the same name is not ours.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forks_match_only_their_owners_branches() {
    let s = setup().await;
    let fork = s.repo.clone_as_remote_named("ada");
    s.repo
        .git(&["remote", "set-url", "ada", "git@github.com:ada/cat.git"]);
    behind(&s.repo, "git@github.com:ada/cat.git", &fork);
    for b in ["topic", "shared"] {
        s.repo.branch(b);
    }
    s.repo.git(&["config", "branch.topic.pushRemote", "ada"]);
    s.gh.reply(
        "pr-list",
        0,
        &list(&[
            pr(21, "topic", "ada", true, false),
            pr(22, "shared", "stranger", true, false),
            pr(23, "topic", "stranger", true, false),
        ]),
        "",
    );
    s.load().await;
    let labels = s.labels().await;
    let mark = |name: &str| {
        labels
            .iter()
            .find(|l| l.kind == RefKind::Branch && l.name == name)
            .and_then(|l| l.pr.as_ref())
            .map(|p| p.number)
    };
    assert_eq!(
        mark("topic"),
        Some(21),
        "pushed to ada's fork: ada's pull request"
    );
    assert_eq!(mark("shared"), None, "a stranger's branch of the same name");
}

/// Review Focus 4: a branch whose pull request was merged or closed shows none (only open
/// ones are listed), and Create pull request is offered again (`existing` is none).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_branch_whose_pull_request_was_merged_shows_none_and_is_offered_again() {
    let s = setup().await;
    s.repo.branch("done");
    s.gh.reply(
        "pr-list",
        0,
        &list(&[pr(5, "done", "octo", false, false)]),
        "",
    );
    s.load().await;
    assert!(
        s.labels()
            .await
            .iter()
            .any(|l| l.name == "done" && l.pr.is_some())
    );
    // Merged meanwhile: no longer open.
    s.h.m.set_prs_every(Duration::ZERO);
    s.gh.reply("pr-list", 0, "[]", "");
    s.gh.reply("pr-list-head", 0, "[]", "");
    s.load().await;
    assert!(s.labels().await.iter().all(|l| l.pr.is_none()));
    let suggested = s.h.m.pr_suggest(s.id, "done").await.unwrap();
    assert_eq!(suggested.existing, None);
    for call in s.gh.calls_of(&["pr", "list"]) {
        let state = call.argv.iter().position(|a| a == "--state").unwrap();
        assert_eq!(call.argv[state + 1], "open");
    }
}

/// Review Focus 4: `gh` not installed: unavailable (`no_gh`), said once per repository per
/// run of the app however often it loads; never a hang.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_missing_is_unavailable_and_said_once() {
    let s = setup().await;
    let empty = tempfile::tempdir().unwrap();
    s.h.m
        .set_tool_path(Some(empty.path().as_os_str().to_owned()));
    s.h.m.set_prs_every(Duration::ZERO);
    let snap = s.load().await;
    assert_eq!(snap.prs.status, PrStatus::Unavailable);
    assert_eq!(snap.prs.reason, Some(PrUnavailable::NoGh));
    s.load().await;
    s.load().await;
    let notices = s.notices();
    assert_eq!(notices.len(), 1, "{notices:#?}");
    assert_eq!(notices[0]["prs"], "no_gh");
    assert!(
        notices[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("GitHub CLI (gh)")
    );
}

/// Review Focus 4: signed out (gh says so, or says to run `gh auth login`): `signed_out`;
/// another reason later in the same run is not said again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_out_is_unavailable_and_said_once() {
    let s = setup().await;
    s.h.m.set_prs_every(Duration::ZERO);
    s.gh.reply(
        "pr-list",
        4,
        "",
        "To get started with GitHub CLI, please run:  gh auth login\nAlternatively, populate the GH_TOKEN environment variable with a GitHub API authentication token.\n",
    );
    let snap = s.load().await;
    assert_eq!(snap.prs.status, PrStatus::Unavailable);
    assert_eq!(snap.prs.reason, Some(PrUnavailable::SignedOut));
    s.gh.reply("pr-list", 1, "", "not logged in to github.com\n");
    s.load().await;
    s.gh.reply("pr-list", 1, "", "HTTP 502: bad gateway\n");
    let snap = s.load().await;
    assert_eq!(snap.prs.reason, Some(PrUnavailable::Failed));
    assert_eq!(s.notices().len(), 1, "said once per repository per run");
    assert_eq!(s.notices()[0]["prs"], "signed_out");
}

/// Review Focus 4: a remote not on GitHub: `not_github`. A host not GitHub by name is asked
/// of `gh` once (`gh auth token --hostname <host>`: has it credentials stored for it?); with
/// none, no other `gh` command runs, and the compare page cannot be built either.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_not_on_github_never_runs_gh() {
    let s = setup_with("https://gitlab.com/octo/cat.git").await;
    s.gh.reply("auth-token", 1, "", "no oauth token found for gitlab.com\n");
    let snap = s.load().await;
    assert_eq!(snap.prs.status, PrStatus::Unavailable);
    assert_eq!(snap.prs.reason, Some(PrUnavailable::NotGithub));
    assert_eq!(s.notices().len(), 1);
    assert_eq!(s.notices()[0]["prs"], "not_github");
    let suggested = s.h.m.pr_suggest(s.id, "main").await.unwrap();
    assert_eq!(suggested.compare_url, None);
    let argv: Vec<Vec<String>> = s.gh.calls().into_iter().map(|c| c.argv).collect();
    assert_eq!(
        argv,
        [["auth", "token", "--hostname", "gitlab.com"]],
        "asked once for the run of the app, nothing else"
    );
}

/// Review Focus 4: slow (past the limit) or rate-limited after a load that worked: the
/// markers stay, marked stale (`ready` with reason `failed`, `loadedAtMs` of the list
/// shown); never a hang, and not said as unavailable. Before any load worked: unavailable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_or_rate_limited_reload_keeps_the_markers_marked_stale() {
    let s = setup().await;
    s.h.m.set_prs_every(Duration::ZERO);
    s.repo.branch("feature");
    s.gh.reply(
        "pr-list",
        0,
        &list(&[pr(3, "feature", "octo", false, false)]),
        "",
    );
    let first = s.load().await;
    let loaded_at = first.prs.loaded_at_ms;
    assert_eq!(first.prs.status, PrStatus::Ready);

    // A tight limit only once the first load worked (a loaded machine may be slow to start
    // the fake).
    s.h.m.set_gh_timeout(Duration::from_millis(800));
    s.gh.slow("pr-list", 30);
    let started = std::time::Instant::now();
    let slow = s.load().await;
    assert!(started.elapsed() < Duration::from_secs(10), "given up on");
    assert_eq!(slow.prs.status, PrStatus::Ready);
    assert_eq!(slow.prs.reason, Some(PrUnavailable::Failed));
    assert_eq!(
        slow.prs.loaded_at_ms, loaded_at,
        "stale: as of the last load"
    );
    assert!(
        s.labels()
            .await
            .iter()
            .any(|l| l.name == "feature" && l.pr.is_some())
    );

    s.gh.fast("pr-list");
    s.gh.reply(
        "pr-list",
        1,
        "",
        "GraphQL: API rate limit exceeded for user ID 1.\n",
    );
    let limited = s.load().await;
    assert_eq!(limited.prs.status, PrStatus::Ready);
    assert_eq!(limited.prs.reason, Some(PrUnavailable::Failed));
    assert_eq!(limited.prs.loaded_at_ms, loaded_at);
    assert!(s.notices().is_empty(), "a passing failure is not said");
    // The gh that hung was ended and logged as timed out.
    assert!(
        s.h.log
            .since(0)
            .iter()
            .any(|r| r.program == "gh" && r.stderr == "timed out")
    );

    // Working again: fresh.
    s.gh.reply("pr-list", 0, "[]", "");
    let fresh = s.load().await;
    assert_eq!(fresh.prs.reason, None);
    assert!(fresh.prs.loaded_at_ms >= loaded_at);

    // A first load that fails: unavailable, said.
    let t = setup().await;
    t.h.m.set_gh_timeout(Duration::from_millis(500));
    t.gh.slow("pr-list", 30);
    let snap = t.load().await;
    assert_eq!(snap.prs.status, PrStatus::Unavailable);
    assert_eq!(snap.prs.reason, Some(PrUnavailable::Failed));
    assert_eq!(t.notices().len(), 1);
}

/// Exactly 1000 listed (the limit asked for): `partial`, some markers may be missing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn as_many_as_the_limit_is_partial() {
    let s = setup().await;
    let many: Vec<Value> = (1..=1000)
        .map(|n| pr(n, &format!("b{n}"), "octo", false, false))
        .collect();
    s.gh.reply("pr-list", 0, &list(&many), "");
    let snap = s.load().await;
    assert_eq!(snap.prs.status, PrStatus::Ready);
    assert!(snap.prs.partial);
}

/// Loaded after a fetch or a refresh the user asks for, at most once per 60 s per
/// repository; a refresh without a fetch loads; the watcher alone (a change in the
/// repository) does not; a plain refresh (the window's focus) does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loads_go_with_fetches_and_user_refreshes_at_most_once_a_minute() {
    let s = setup().await;
    s.gh.reply("pr-list", 0, "[]", "");
    // Opening and a plain refresh load nothing.
    s.h.m.refresh(s.id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(s.gh.calls().is_empty(), "opened and refreshed: nothing");
    // The watcher alone: a commit made in a terminal.
    s.repo.commit("outside");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(s.gh.calls().is_empty(), "the watcher alone loads nothing");

    // A refresh the user asks for, without a fetch: loads.
    s.load().await;
    assert_eq!(s.gh.calls().len(), 1);
    // Again within the minute: not.
    s.h.m.refresh_by_user(s.id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(s.gh.calls().len(), 1, "at most once per 60 s");

    // A fetch: loads (once the minute has passed).
    s.h.m.set_prs_every(Duration::from_millis(200));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = s.h.m.prs_loads(s.id);
    s.h.m.fetch(s.id).await.unwrap();
    tokio::time::timeout(TIMEOUT, s.h.m.wait_prs_loads(s.id, before))
        .await
        .expect("loaded after the fetch");
    assert_eq!(s.gh.calls().len(), 2);
}

/// `pr_suggest`: one commit's subject as the title, or the branch name as words; the
/// subjects as the description; the base remembered, else gh's default branch, else the
/// remote's HEAD, else main; `existing` asked of gh for that branch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn suggestions_come_from_the_commits_and_the_base_from_what_is_known() {
    let s = setup().await;
    s.repo.git(&["switch", "-q", "-c", "fix/pr_loading-state"]);
    s.repo.commit("Load pull requests after a fetch");
    s.gh.reply(
        "repo-view",
        0,
        r#"{"defaultBranchRef":{"name":"main"}}"#,
        "",
    );
    let one =
        s.h.m
            .pr_suggest(s.id, "fix/pr_loading-state")
            .await
            .unwrap();
    assert_eq!(one.title, "Load pull requests after a fetch");
    assert_eq!(one.body, "- Load pull requests after a fetch");
    assert_eq!(one.base, "main");
    assert_eq!(one.existing, None);
    assert_eq!(
        one.compare_url.as_deref(),
        Some("https://github.com/octo/cat/compare/main...fix/pr_loading-state?expand=1")
    );
    let head = s.gh.calls_of(&["pr", "list"]);
    assert_eq!(
        head[0].argv,
        [
            "pr",
            "list",
            "--repo",
            "octo/cat",
            "--head",
            "fix/pr_loading-state",
            "--state",
            "open",
            "--limit",
            "1000",
            "--json",
            FIELDS
        ],
        "existing is asked for the branch itself, among as many as are listed"
    );
    assert_eq!(
        s.gh.calls_of(&["repo", "view"])[0].argv,
        ["repo", "view", "octo/cat", "--json", "defaultBranchRef"]
    );
    assert_eq!(one.base_remote.as_deref(), Some("origin"));

    s.repo.commit("Mark branches");
    s.gh.reply(
        "pr-list-head",
        0,
        &list(&[pr(9, "fix/pr_loading-state", "octo", false, false)]),
        "",
    );
    let two =
        s.h.m
            .pr_suggest(s.id, "fix/pr_loading-state")
            .await
            .unwrap();
    assert_eq!(two.title, "Pr loading state");
    assert_eq!(
        two.body,
        "- Load pull requests after a fetch\n- Mark branches"
    );
    assert_eq!(two.existing.map(|p| p.number), Some(9));

    // gh's default branch: develop (a branch on the remote).
    s.repo.git(&["branch", "develop", "main"]);
    s.repo.git(&["push", "-q", "origin", "develop"]);
    s.repo.git(&["fetch", "-q", "origin"]);
    s.gh.reply(
        "repo-view",
        0,
        r#"{"defaultBranchRef":{"name":"develop"}}"#,
        "",
    );
    assert_eq!(
        s.h.m
            .pr_suggest(s.id, "fix/pr_loading-state")
            .await
            .unwrap()
            .base,
        "develop"
    );
    // gh failing: the remote's HEAD.
    s.gh.reply("repo-view", 1, "", "HTTP 502\n");
    s.repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/develop",
    ]);
    assert_eq!(
        s.h.m
            .pr_suggest(s.id, "fix/pr_loading-state")
            .await
            .unwrap()
            .base,
        "develop"
    );
    // Nothing known: main.
    s.repo
        .git(&["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"]);
    assert_eq!(
        s.h.m
            .pr_suggest(s.id, "fix/pr_loading-state")
            .await
            .unwrap()
            .base,
        "main"
    );
    // Not a local branch: refused.
    assert!(s.h.m.pr_suggest(s.id, "nope").await.is_err());
    assert!(s.h.m.pr_suggest(s.id, "-x").await.is_err());
}

fn create(branch: &str, base: &str, title: &str, body: &str, draft: bool) -> OpRequest {
    OpRequest::PrCreate {
        branch: branch.into(),
        base: base.into(),
        base_remote: None,
        title: title.into(),
        body: body.into(),
        draft,
    }
}

/// Remembers the pull request base given, as the app's store does.
#[derive(Default)]
struct Memory(std::sync::Mutex<Option<String>>);

impl gg_core::repo::WorktreeMemory for Memory {
    fn remembered(&self, _root: &std::path::Path) -> Option<std::path::PathBuf> {
        None
    }
    fn remember(&self, _root: &std::path::Path, _worktree: Option<&std::path::Path>) {}
    fn pr_base(&self, _root: &std::path::Path) -> Option<String> {
        self.0.lock().unwrap().clone()
    }
    fn remember_pr_base(&self, _root: &std::path::Path, base: &str) {
        *self.0.lock().unwrap() = Some(base.to_owned());
    }
}

/// `pr_create`: a branch with no namesake on the remote is pushed first (R38, with its
/// upstream set), then `gh pr create --head <b> --base <base> --title <t> --body-file -`
/// (`--draft` when asked) with the description on its stdin, byte for byte; the outcome
/// carries the address and an `open_url` hint; the base is remembered; the list reloads at
/// once, whatever the minute; no working directory is needed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pull_request_is_made_after_the_push_with_the_body_on_stdin() {
    let s = setup().await;
    let memory = std::sync::Arc::new(Memory::default());
    s.h.m.set_worktree_memory(memory.clone());
    s.gh.reply("pr-list", 0, "[]", "");
    s.load().await;
    s.repo.git(&["switch", "-q", "-c", "feature/x"]);
    s.repo.commit("Feature");
    s.repo.git(&["switch", "-q", "main"]);
    s.gh.reply(
        "pr-create",
        0,
        "Warning: 2 uncommitted changes\nhttps://github.com/octo/cat/pull/42\n",
        "",
    );
    s.gh.reply(
        "pr-list",
        0,
        &list(&[pr(42, "feature/x", "octo", false, true)]),
        "",
    );
    let body = "Why:\n- `gh` and \"quotes\" $(not run) \u{e9}\n\nLast line, no newline";
    let loads = s.h.m.prs_loads(s.id);
    let out =
        s.h.run(s.id, create("feature/x", "main", "Add x", body, true))
            .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(
        out.message
            .starts_with("Pushed feature/x to origin/feature/x"),
        "{}",
        out.message
    );
    assert!(
        out.message
            .ends_with("Opened draft pull request #42 of feature/x into main: https://github.com/octo/cat/pull/42"),
        "{}",
        out.message
    );
    assert_eq!(
        out.hints,
        [OpHint::OpenUrl {
            url: "https://github.com/octo/cat/pull/42".into(),
            label: "Open pull request".into()
        }]
    );
    // Pushed to its namesake, upstream set.
    assert_eq!(
        s.repo.git(&["rev-parse", "refs/remotes/origin/feature/x"]),
        s.repo.git(&["rev-parse", "feature/x"])
    );
    assert_eq!(s.repo.git(&["config", "branch.feature/x.remote"]), "origin");
    // The steps are said, for the dialog waiting on it.
    let steps: Vec<String> =
        s.h.sink
            .events()
            .into_iter()
            .filter(|(n, v)| n == "op-progress" && v["opId"] == out.op_id)
            .map(|(_, v)| v["line"].as_str().unwrap_or_default().to_owned())
            .filter(|l| l.ends_with('…'))
            .collect();
    assert_eq!(
        steps,
        [
            "Pushing feature/x to origin/feature/x…",
            "Creating the pull request…"
        ]
    );
    let made = s.gh.calls_of(&["pr", "create"]);
    assert_eq!(made.len(), 1);
    assert_eq!(
        made[0].argv,
        [
            "pr",
            "create",
            "--repo",
            "octo/cat",
            "--head",
            "feature/x",
            "--base",
            "main",
            "--title",
            "Add x",
            "--body-file",
            "-",
            "--draft"
        ]
    );
    assert_eq!(
        made[0].stdin,
        body.as_bytes(),
        "the description, byte for byte"
    );
    assert!(!made[0].tty);
    // Remembered, and reloaded at once though the last load was just now.
    tokio::time::timeout(TIMEOUT, async {
        while memory.0.lock().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the base is remembered");
    assert_eq!(memory.0.lock().unwrap().as_deref(), Some("main"));
    tokio::time::timeout(TIMEOUT, s.h.m.wait_prs_loads(s.id, loads))
        .await
        .expect("reloaded at once");
    assert!(
        s.labels()
            .await
            .iter()
            .any(|l| l.name == "feature/x" && l.pr.as_ref().map(|p| p.number) == Some(42))
    );
    // The next suggestion offers the base used.
    s.gh.reply(
        "repo-view",
        0,
        r#"{"defaultBranchRef":{"name":"other"}}"#,
        "",
    );
    assert_eq!(
        s.h.m.pr_suggest(s.id, "feature/x").await.unwrap().base,
        "main"
    );

    // Up to date with its namesake: not pushed again; ready, not a draft.
    let pushes =
        s.h.log
            .since(0)
            .iter()
            .filter(|r| r.args.first().map(String::as_str) == Some("push"))
            .count();
    s.gh.reply("pr-create", 0, "https://github.com/octo/cat/pull/43\n", "");
    let out =
        s.h.run(s.id, create("feature/x", "main", "Again", "", false))
            .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(
        out.message.starts_with("Opened pull request #43"),
        "{}",
        out.message
    );
    let after =
        s.h.log
            .since(0)
            .iter()
            .filter(|r| r.args.first().map(String::as_str) == Some("push"))
            .count();
    assert_eq!(after, pushes, "nothing to push");
    assert!(
        !s.gh.calls_of(&["pr", "create"])[1]
            .argv
            .contains(&"--draft".to_owned())
    );
}

/// `gh` says one exists already: done, with an `open_url` hint to it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_existing_pull_request_is_offered_to_open() {
    let s = setup().await;
    s.repo.branch("feature");
    s.repo.git(&["push", "-q", "-u", "origin", "feature"]);
    s.gh.reply(
        "pr-create",
        1,
        "",
        "a pull request for branch \"feature\" into branch \"main\" already exists:\nhttps://github.com/octo/cat/pull/7\n",
    );
    let out =
        s.h.run(s.id, create("feature", "main", "T", "b", false))
            .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(out.message, "feature has a pull request open already #7.");
    assert_eq!(
        out.hints,
        [OpHint::OpenUrl {
            url: "https://github.com/octo/cat/pull/7".into(),
            label: "Open pull request".into()
        }]
    );
}

/// Unavailable (no gh, signed out): failed with the plain reason and, the branch pushed,
/// the compare page to open instead; a rejected push fails with the push's hints, and gh
/// is not run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_gh_the_compare_page_is_offered_and_a_rejected_push_stops_it() {
    let s = setup().await;
    s.repo.branch("feature");
    let empty = tempfile::tempdir().unwrap();
    s.h.m
        .set_tool_path(Some(empty.path().as_os_str().to_owned()));
    let out =
        s.h.run(s.id, create("feature", "main", "T", "b", false))
            .await;
    // Without gh (Task 8 review): pushed all the same (R38), and done: finished on GitHub.
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(
        out.message,
        "Pushed feature to origin/feature. Finish on GitHub:"
    );
    assert_eq!(
        out.hints,
        [OpHint::OpenUrl {
            url: "https://github.com/octo/cat/compare/main...feature?expand=1".into(),
            label: "Open the compare page".into()
        }]
    );
    assert!(
        s.repo
            .git_succeeds(&["rev-parse", "--verify", "refs/remotes/origin/feature"]),
        "pushed, so the compare page shows it"
    );
    // Nothing more to push: said so.
    let out =
        s.h.run(s.id, create("feature", "main", "T", "b", false))
            .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(
        out.message,
        "feature is on origin/feature already. Finish on GitHub:"
    );

    // Signed out.
    s.h.m.set_tool_path(Some(s.gh.path()));
    s.gh.reply(
        "pr-create",
        4,
        "",
        "To get started with GitHub CLI, please run:  gh auth login\n",
    );
    let out =
        s.h.run(s.id, create("feature", "main", "T", "b", false))
            .await;
    assert_eq!(out.status, OpStatus::Failed);
    assert!(out.message.contains("not signed in"), "{}", out.message);
    assert!(matches!(&out.hints[..], [OpHint::OpenUrl { url, .. }] if url.contains("/compare/")));

    // The remote has moved on: the push is rejected, nothing is made.
    let other = TestRepo::clone_of(&s._origin);
    other.git(&["switch", "-q", "feature"]);
    other.commit("theirs");
    other.git(&["push", "-q", "origin", "feature"]);
    s.repo.git(&["switch", "-q", "feature"]);
    s.repo.commit("mine");
    s.repo.git(&["fetch", "-q", "origin"]);
    let made = s.gh.calls_of(&["pr", "create"]).len();
    let out =
        s.h.run(s.id, create("feature", "main", "T", "b", false))
            .await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert!(out.hints.contains(&OpHint::Fetch), "{:?}", out.hints);
    assert_eq!(s.gh.calls_of(&["pr", "create"]).len(), made, "gh not run");
}

/// Bad requests are refused before anything runs: an empty or multi-line title, a base
/// that is option-like or not on the remote.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bad_pull_request_requests_are_refused_before_anything_runs() {
    let s = setup().await;
    s.repo.branch("feature");
    for request in [
        create("feature", "main", "  ", "b", false),
        create("feature", "main", "two\nlines", "b", false),
        create("feature", "-x", "T", "b", false),
        create("-f", "main", "T", "b", false),
    ] {
        let err = s.h.m.run_op(s.id, request, &[]).await.unwrap_err();
        assert!(
            matches!(
                err.code,
                gg_core::error::ErrorCode::BadRequest | gg_core::error::ErrorCode::BadName
            ),
            "{err:?}"
        );
    }
    let mark = s.h.last_log_id();
    let out =
        s.h.run(s.id, create("feature", "nowhere", "T", "b", false))
            .await;
    assert_eq!(out.status, OpStatus::Failed);
    assert_eq!(
        out.message,
        "Nothing was opened: nowhere is not a branch on origin. Choose another base, or fetch first."
    );
    assert!(s.h.writes_since(mark).is_empty(), "nothing pushed");
    assert!(s.gh.calls().is_empty());
}

/// An Enterprise host `gh` has credentials stored for: `GH_HOST` names it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_enterprise_host_is_named_with_gh_host() {
    let s = setup_with("https://github.example.com/team/app.git").await;
    s.gh.reply("auth-token", 0, "", "");
    s.gh.reply("pr-list", 0, "[]", "");
    let snap = s.load().await;
    assert_eq!(snap.prs.status, PrStatus::Ready);
    let call = &s.gh.calls_of(&["pr", "list"])[0];
    assert!(
        call.env.iter().any(|e| e == "GH_HOST=github.example.com"),
        "{:?}",
        call.env
    );
}

/// Security: `gh` runs `git` itself in the repository. Its git gets the protection the app's
/// own git children have, through the environment git honours in every child: no prompt,
/// editor or pager, no optional locks, none of an enclosing git's repository variables, and
/// the app's settings as command-line config (`core.fsmonitor=false`, …) plus no hooks.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_git_gh_runs_is_protected_as_the_apps_own() {
    let s = setup().await;
    s.repo
        .git(&["config", "core.fsmonitor", "touch fsmonitor-ran"]);
    s.repo.git(&["config", "core.hooksPath", ".githooks"]);
    s.gh.reply("pr-list", 0, "[]", "");
    s.gh.run_on(
        "pr-list",
        "git config --get core.fsmonitor; git config --get core.hooksPath; git status --short >/dev/null",
    );
    s.load().await;
    let call = &s.gh.calls_of(&["pr", "list"])[0];
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let git_env: Vec<&String> = call.env.iter().filter(|e| e.starts_with("GIT_")).collect();
    assert_eq!(call.ran, format!("false\n{null}\n"), "{git_env:?}");
    assert!(!s.repo.path().join("fsmonitor-ran").exists());
    for set in [
        "GIT_TERMINAL_PROMPT=0",
        "GIT_OPTIONAL_LOCKS=0",
        "GIT_EDITOR=true",
        "GIT_SEQUENCE_EDITOR=true",
        "GIT_MERGE_AUTOEDIT=no",
        "GIT_PAGER=cat",
        "GIT_CONFIG_COUNT=5",
        "GIT_CONFIG_KEY_0=core.quotePath",
        "GIT_CONFIG_VALUE_0=false",
        "GIT_CONFIG_KEY_1=color.ui",
        "GIT_CONFIG_VALUE_1=false",
        "GIT_CONFIG_KEY_2=log.showSignature",
        "GIT_CONFIG_VALUE_2=false",
        "GIT_CONFIG_KEY_3=core.fsmonitor",
        "GIT_CONFIG_VALUE_3=false",
        "GIT_CONFIG_KEY_4=core.hooksPath",
    ] {
        assert!(call.env.iter().any(|e| e == set), "{set}: {git_env:?}");
    }
    assert!(
        call.env
            .iter()
            .any(|e| *e == format!("GIT_CONFIG_VALUE_4={null}"))
    );
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
    ] {
        assert!(
            !call.env.iter().any(|e| e.starts_with(&format!("{var}="))),
            "{var}: {git_env:?}"
        );
    }
}

/// One choice of repository (Task 8 review): every `gh` command names the repository the
/// app chose (`--repo`), never left to `gh`. In a fork workflow, remotes `fork` (ada's, the
/// branch's upstream) and `main-repo` (octo's), with the base on `main-repo`: the pull
/// request goes to octo/cat from `ada:topic`; when `gh` cannot make it, the compare page is
/// main-repo's, comparing ada's branch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_workflow_names_the_base_repository_and_the_forks_branch() {
    let s = setup().await;
    s.repo.git(&["remote", "rename", "origin", "main-repo"]);
    let fork = s.repo.clone_as_remote_named("fork");
    s.repo
        .git(&["remote", "set-url", "fork", "git@github.com:ada/cat.git"]);
    behind(&s.repo, "git@github.com:ada/cat.git", &fork);
    s.repo.branch("topic");
    s.repo.git(&["push", "-q", "-u", "fork", "topic"]);
    s.gh.reply("pr-list", 0, "[]", "");
    s.load().await;
    let suggested = s.h.m.pr_suggest(s.id, "topic").await.unwrap();
    assert_eq!(
        suggested.base_remote.as_deref(),
        Some("fork"),
        "no upstream/github/origin: the branch's push remote"
    );

    s.gh.reply("pr-create", 0, "https://github.com/octo/cat/pull/5\n", "");
    let mut request = create("topic", "main", "Topic", "", false);
    if let OpRequest::PrCreate { base_remote, .. } = &mut request {
        *base_remote = Some("main-repo".into());
    }
    let out = s.h.run(s.id, request.clone()).await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    let made = &s.gh.calls_of(&["pr", "create"])[0];
    assert_eq!(
        made.argv[..8],
        [
            "pr",
            "create",
            "--repo",
            "octo/cat",
            "--head",
            "ada:topic",
            "--base",
            "main"
        ]
    );

    // gh cannot make it: said as such, with the compare page of the base repository and
    // the fork's branch.
    s.gh.reply("pr-create", 1, "", "HTTP 502: bad gateway\n");
    let out = s.h.run(s.id, request).await;
    assert_eq!(out.status, OpStatus::Failed, "{out:#?}");
    assert_eq!(
        out.message,
        "gh could not open the pull request: HTTP 502: bad gateway"
    );
    assert_eq!(
        out.hints,
        [OpHint::OpenUrl {
            url: "https://github.com/octo/cat/compare/main...ada:topic?expand=1".into(),
            label: "Open the compare page".into()
        }]
    );

    // A remote that is not one: refused, nothing run.
    let before = s.gh.calls().len();
    let mut bad = create("topic", "main", "Topic", "", false);
    if let OpRequest::PrCreate { base_remote, .. } = &mut bad {
        *base_remote = Some("nope".into());
    }
    let out = s.h.run(s.id, bad).await;
    assert_eq!(out.status, OpStatus::Failed);
    assert_eq!(s.gh.calls().len(), before);

    // gh never ran without naming the repository.
    for call in s.gh.calls() {
        let named = call.argv.iter().any(|a| a == "--repo")
            || call.argv[..2] == ["repo", "view"] && call.argv.len() > 2;
        assert!(named, "{:?}", call.argv);
        assert!(!call.env.iter().any(|e| e.starts_with("GH_REPO=")));
        assert!(!call.env.iter().any(|e| e.starts_with("GH_HOST=")));
    }
}

/// ssh aliases (Task 8 review): `git@github.com-work:me/r.git` and `git@gh-work:me/r.git`
/// stand for github.com in the user's ssh config; the real host is asked of `ssh -G` (once
/// per alias per run), and the GitHub check, `--repo` and the compare page use it. When ssh
/// cannot tell, the host as written is kept (not GitHub, no credentials stored for it).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_aliases_are_resolved_to_the_real_host() {
    for alias in ["github.com-work", "gh-work"] {
        let url = format!("git@{alias}:me/cat.git");
        let s = setup_with(&url).await;
        s.gh.ssh_alias(alias, "github.com");
        s.gh.reply("pr-list", 0, "[]", "");
        s.h.m.set_prs_every(Duration::ZERO);
        let snap = s.load().await;
        assert_eq!(snap.prs.status, PrStatus::Ready, "{alias}: {:?}", snap.prs);
        let listed = &s.gh.calls_of(&["pr", "list"])[0];
        assert_eq!(listed.argv[2..4], ["--repo", "me/cat"], "{alias}");
        assert!(
            !listed.env.iter().any(|e| e.starts_with("GH_HOST=")),
            "{alias}"
        );
        assert!(
            s.gh.calls_of(&["auth", "token"]).is_empty(),
            "{alias}: GitHub by name"
        );
        let suggested = s.h.m.pr_suggest(s.id, "main").await.unwrap();
        assert_eq!(
            suggested.compare_url.as_deref(),
            Some("https://github.com/me/cat/compare/main...main?expand=1"),
            "{alias}"
        );
        s.load().await;
        assert_eq!(
            s.gh.ssh_calls(),
            [["-G", "--", alias]],
            "{alias}: asked once for the run of the app"
        );
    }

    // ssh cannot tell: the host as written, which gh has no credentials for.
    let s = setup_with("git@gh-other:me/cat.git").await;
    s.gh.ssh_fails(Some(255));
    s.gh.reply("auth-token", 1, "", "");
    let snap = s.load().await;
    assert_eq!(snap.prs.reason, Some(PrUnavailable::NotGithub));
    assert_eq!(
        s.gh.calls_of(&["auth", "token"])[0].argv,
        ["auth", "token", "--hostname", "gh-other"]
    );
}

/// A test build never runs the real `gh` or `ssh`: only those in the first folder of a tool
/// `PATH` that was set.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_test_build_runs_only_the_fake_tools() {
    let s = setup_with("git@gh-work:me/cat.git").await;
    s.gh.ssh_alias("gh-work", "github.com");
    let empty = tempfile::tempdir().unwrap();
    let mut path = empty.path().as_os_str().to_owned();
    path.push(":");
    path.push(s.gh.path());
    s.h.m.set_tool_path(Some(path));
    s.h.m.set_prs_every(Duration::ZERO);
    let snap = s.load().await;
    assert!(s.gh.calls().is_empty() && s.gh.ssh_calls().is_empty());
    assert_eq!(snap.prs.status, PrStatus::Unavailable);
    s.h.m.set_tool_path(None);
    let snap = s.load().await;
    assert!(s.gh.calls().is_empty() && s.gh.ssh_calls().is_empty());
    assert_eq!(snap.prs.status, PrStatus::Unavailable);
}

/// `gh pr create` made it but printed no address the app takes (Task 8 review): done, said
/// so, the list reloaded at once. An address on the host with another port is taken.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pull_request_made_without_an_address_printed_is_done() {
    let s = setup().await;
    s.gh.reply("pr-list", 0, "[]", "");
    s.load().await;
    s.repo.branch("feature");
    s.repo.git(&["push", "-q", "-u", "origin", "feature"]);
    s.gh.reply("pr-create", 0, "Creating pull request...\n", "");
    let loads = s.h.m.prs_loads(s.id);
    let out =
        s.h.run(s.id, create("feature", "main", "T", "", false))
            .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert_eq!(
        out.message,
        "The pull request was created, but gh did not print its address."
    );
    tokio::time::timeout(TIMEOUT, s.h.m.wait_prs_loads(s.id, loads))
        .await
        .expect("reloaded at once");

    s.gh.reply(
        "pr-create",
        0,
        "https://github.com:443/octo/cat/pull/8\n",
        "",
    );
    let out =
        s.h.run(s.id, create("feature", "main", "T", "", false))
            .await;
    assert_eq!(out.status, OpStatus::Done, "{out:#?}");
    assert!(
        out.message
            .ends_with("https://github.com:443/octo/cat/pull/8"),
        "{}",
        out.message
    );
}

/// Security: ssh puts the host into `Match exec` and `ProxyCommand` strings of the user's
/// ssh config (`%h`, `%n`), which a shell runs; `ssh -G` is run only for a plain host name
/// (letters, digits, `.`, `_`, `-`; no leading or trailing dot or hyphen; at most 253), any
/// other host is kept as written (and is not GitHub).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_is_never_asked_about_a_host_that_is_not_a_plain_name() {
    for host in [
        "$(touch x)",
        "a`id`b",
        "a;b",
        "%h",
        "a'b",
        "a\"b",
        ".lead",
        "trail-",
        "a|b",
    ] {
        let s = setup_with(&format!("git@{host}:me/cat.git")).await;
        s.gh.reply("auth-token", 1, "", "");
        let snap = s.load().await;
        assert_eq!(snap.prs.reason, Some(PrUnavailable::NotGithub), "{host}");
        assert!(s.gh.ssh_calls().is_empty(), "{host}: ssh was run");
    }
    // ssh's answer is taken only when it is a plain name too.
    let s = setup_with("git@gh-work:me/cat.git").await;
    s.gh.ssh_alias("gh-work", "github.com;id");
    s.gh.reply("auth-token", 1, "", "");
    let snap = s.load().await;
    assert_eq!(snap.prs.reason, Some(PrUnavailable::NotGithub));
    assert_eq!(s.gh.ssh_calls().len(), 1);
}

/// No flicker (Task 8 review): a repository known not to be on GitHub, or without `gh`, is
/// never shown as loading on its way to unavailable; one that can load is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loading_is_shown_only_when_gh_will_be_asked() {
    let loading = |s: &Setup| {
        s.h.sink
            .events()
            .into_iter()
            .filter(|(n, v)| n == "repo-changed" && v["prs"]["status"] == "loading")
            .count()
    };
    let s = setup_with("https://gitlab.com/octo/cat.git").await;
    s.gh.reply("auth-token", 1, "", "");
    s.h.m.set_prs_every(Duration::ZERO);
    s.load().await;
    s.load().await;
    assert_eq!(loading(&s), 0, "not GitHub");

    let s = setup().await;
    let empty = tempfile::tempdir().unwrap();
    s.h.m
        .set_tool_path(Some(empty.path().as_os_str().to_owned()));
    s.h.m.set_prs_every(Duration::ZERO);
    s.load().await;
    s.load().await;
    assert_eq!(loading(&s), 0, "no gh");

    let s = setup().await;
    s.gh.reply("pr-list", 0, "[]", "");
    s.load().await;
    assert_eq!(loading(&s), 1, "gh asked");
}

/// `pr_suggest` asks gh for the default branch and for the branch's pull request at once
/// (Task 8 review), under one time limit: two slow answers take the time of one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_suggestion_asks_gh_both_questions_at_once() {
    let s = setup().await;
    s.repo.branch("feature");
    s.gh.reply(
        "repo-view",
        0,
        r#"{"defaultBranchRef":{"name":"main"}}"#,
        "",
    );
    s.gh.reply(
        "pr-list-head",
        0,
        &list(&[pr(3, "feature", "octo", false, false)]),
        "",
    );
    s.gh.slow("repo-view", 2);
    s.gh.slow("pr-list-head", 2);
    let started = std::time::Instant::now();
    let suggested = s.h.m.pr_suggest(s.id, "feature").await.unwrap();
    let took = started.elapsed();
    assert_eq!(suggested.base, "main");
    assert_eq!(suggested.existing.map(|p| p.number), Some(3));
    assert!(
        took < Duration::from_millis(3500),
        "{took:?}: one after the other"
    );
}
