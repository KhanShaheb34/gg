//! Each repository's open pull requests (`snapshot.prs`, the branch labels' and worktrees'
//! markers), loaded through `gh` (see [`crate::gh`]) only alongside a fetch (scheduled or
//! the user's) or a refresh the user asks for, never from the watcher alone, and at most once
//! every 60 s per repository; a pull request just made reloads them at once. Why they cannot
//! be shown is in the snapshot always, and said once per repository per run of the app (a
//! `repo-error`). A reload that fails after one that worked keeps the list shown, marked as
//! of when it was loaded.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use crate::dto::{PrState, PrStatus, PrSuggestion, PrUnavailable, RepoSnapshot};
use crate::error::{CoreError, ErrorCode, Result};
use crate::gh::{
    Gh, GhFailure, LIST_LIMIT, PR_FIELDS, PrData, RemoteFacts, humanise, parse_default_branch,
    parse_list, ssh_hostname, tool_on, unavailable_message,
};
use crate::git::runner::{CmdKind, GitRunner, StdinPolicy};
use crate::hosts::{RemoteRepo, compare_url, is_known_github, is_plain_host};
use crate::repo::fetch::unix_ms;
use crate::repo::manager::{ErrorKind, Repo, RepoManager, lock};

/// Least time between two loads of one repository's pull requests (but for one just made).
const LOAD_EVERY: Duration = Duration::from_secs(60);
/// How long `ssh -G` may take to say what an alias stands for.
const SSH_TIMEOUT: Duration = Duration::from_secs(5);
/// Most commit subjects a suggested description lists.
const MAX_SUBJECTS: usize = 50;

/// What asked for the pull requests to be loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrTrigger {
    /// A fetch ended (scheduled or the user's).
    Fetch,
    /// The user asked for a refresh.
    User,
    /// A pull request was just made: loaded at once, whenever the last load was.
    Created,
}

/// Whether a repository's pull requests are being loaded, and when a load last started.
#[derive(Debug, Default)]
pub(crate) struct PrGate {
    running: bool,
    /// A load asked for while one ran, which must not be skipped (a pull request just made).
    again: bool,
    last: Option<Instant>,
}

/// The pull requests as last loaded.
#[derive(Debug, Clone, Default)]
pub(crate) struct PrSlot {
    pub(crate) state: PrState,
    /// The list and the remotes it was matched with; kept after a reload that failed.
    pub(crate) data: Option<Arc<PrData>>,
}

/// Why there is no list: the reason, `gh`'s failure, and whether it is worth saying (not for
/// a repository without remotes).
type Unloaded = (PrUnavailable, Option<GhFailure>, bool);
/// How a load ended: the list, or why there is none.
type Loaded = std::result::Result<PrData, Unloaded>;

/// A failure to list for error `e`.
fn failed(e: CoreError) -> Unloaded {
    let failure = GhFailure::Failed {
        message: e.message,
        detail: e.detail.unwrap_or_default(),
    };
    (PrUnavailable::Failed, Some(failure), true)
}

/// Lists the open pull requests with `gh`: `gh pr list --repo <repo> --state open --limit
/// 1000 --json …` in the main worktree.
async fn list_prs(gh: &Gh, facts: RemoteFacts) -> Loaded {
    let limit = LIST_LIMIT.to_string();
    let args = [
        "pr",
        "list",
        "--repo",
        gh.repo(),
        "--state",
        "open",
        "--limit",
        &limit,
        "--json",
        PR_FIELDS,
    ];
    match gh.run(&args, CmdKind::Read, StdinPolicy::Null).await {
        Ok(Ok(out)) => match parse_list(&out.stdout) {
            Ok(listed) => Ok(PrData { listed, facts }),
            Err(why) => Err(failed(
                CoreError::new(ErrorCode::Io, "gh's answer could not be read.").with_detail(why),
            )),
        },
        Ok(Err(failure)) => Err((failure.reason(), Some(failure), true)),
        Err(e) => Err(failed(e)),
    }
}

impl RepoManager {
    /// Loads `repo`'s pull requests in the background for `trigger` (see the module
    /// comment); nothing when they were loaded less than 60 s ago, or are being loaded.
    pub(crate) fn spawn_load_prs(self: &Arc<Self>, repo: &Arc<Repo>, trigger: PrTrigger) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let (m, repo) = (self.clone(), repo.clone());
            runtime.spawn(async move { m.load_prs(&repo, trigger).await });
        }
    }

    /// Re-reads the repository as [`RepoManager::refresh`] does, for the user: also loads
    /// the pull requests (in the background, at most once every 60 s).
    pub async fn refresh_by_user(self: &Arc<Self>, repo_id: u32) -> Result<RepoSnapshot> {
        // A "no" is asked again: the user may have signed in to the
        // host meanwhile, or fixed the ssh config.
        lock(&self.gh_hosts).retain(|_, stored| *stored);
        lock(&self.ssh_hosts).retain(|_, real| real.is_some());
        let snapshot = self.refresh(repo_id).await?;
        if let Ok(repo) = self.get(repo_id) {
            self.spawn_load_prs(&repo, PrTrigger::User);
        }
        Ok(snapshot)
    }

    async fn load_prs(self: &Arc<Self>, repo: &Arc<Repo>, trigger: PrTrigger) {
        // After "no gh" or "signed out", the user's own refresh asks again at once (the
        // pull-request dialog does it when it opens): they may have installed gh or signed
        // in a moment ago.
        let asked_again = trigger == PrTrigger::User && repo.prs_unavailable().await.is_some();
        {
            let mut gate = lock(&repo.prs_gate);
            if gate.running {
                gate.again |= trigger == PrTrigger::Created;
                return;
            }
            let every = self.prs_every();
            if trigger != PrTrigger::Created
                && !asked_again
                && gate.last.is_some_and(|t| t.elapsed() < every)
            {
                return;
            }
            gate.running = true;
            gate.last = Some(Instant::now());
        }
        loop {
            self.load_prs_once(repo).await;
            let mut gate = lock(&repo.prs_gate);
            if gate.again {
                gate.again = false;
                gate.last = Some(Instant::now());
                continue;
            }
            gate.running = false;
            break;
        }
        repo.prs_loads.send_modify(|n| *n += 1);
    }

    async fn load_prs_once(self: &Arc<Self>, repo: &Arc<Repo>) {
        // Whether gh can be asked at all is known first: a repository not on GitHub, or
        // without gh, goes to unavailable without being shown as loading on the way.
        let ready = self.gh_for_list(repo).await;
        // Shown as loading while nothing is shown yet.
        let loading = if ready.is_err() {
            None
        } else {
            let mut slot = repo.slot.write().await;
            if slot.closed {
                return;
            }
            matches!(
                slot.prs.state.status,
                PrStatus::Unknown | PrStatus::Unavailable
            )
            .then(|| {
                slot.prs.state = PrState {
                    status: PrStatus::Loading,
                    ..PrState::default()
                };
                slot.snapshot_to_emit(&repo.identity, repo.is_loading())
            })
        };
        if let Some(stamped) = loading {
            repo.publish(self.sink.as_ref(), stamped);
        }
        let loaded = match ready {
            Ok((gh, facts)) => list_prs(&gh, facts).await,
            Err(why) => Err(why),
        };
        let say = {
            let mut slot = repo.slot.write().await;
            if slot.closed {
                return;
            }
            let prs = &mut slot.prs;
            match loaded {
                Ok(data) => {
                    prs.state = PrState {
                        status: PrStatus::Ready,
                        reason: None,
                        loaded_at_ms: Some(unix_ms()),
                        partial: data.listed.len() >= LIST_LIMIT,
                    };
                    prs.data = Some(Arc::new(data));
                    None
                }
                // A reload that failed after one that worked: the list stays, as of then.
                Err((PrUnavailable::Failed, _, _)) if prs.data.is_some() => {
                    prs.state.status = PrStatus::Ready;
                    prs.state.reason = Some(PrUnavailable::Failed);
                    None
                }
                Err((reason, failure, worth_saying)) => {
                    prs.state = PrState {
                        status: PrStatus::Unavailable,
                        reason: Some(reason),
                        loaded_at_ms: None,
                        partial: false,
                    };
                    prs.data = None;
                    worth_saying.then_some((reason, failure))
                }
            }
        };
        if let Some((reason, failure)) = say {
            self.say_prs_unavailable(repo, reason, failure.as_ref());
        }
        // Labels and worktrees take the markers, and the snapshot the state.
        let _serial = repo.refresh_lock.lock().await;
        let info = repo.slot.read().await.target().info().clone();
        let _ = self.apply_with(repo, info, None, None, true, false).await;
    }

    /// Says (`repo-error`, with `prs`: the reason) why `repo`'s pull requests are not shown,
    /// unless it was said for this repository already in this run of the app.
    fn say_prs_unavailable(&self, repo: &Repo, reason: PrUnavailable, failure: Option<&GhFailure>) {
        if !lock(&self.prs_said).insert(repo.root.clone()) {
            return;
        }
        let mut error = CoreError::new(ErrorCode::Io, unavailable_message(reason, failure));
        error.detail = failure.map(GhFailure::detail).filter(|d| !d.is_empty());
        let payload = serde_json::json!({
            "repoId": repo.identity.repo_id,
            "error": error,
            "prs": reason,
        });
        repo.report_error(self.sink.as_ref(), ErrorKind::Prs, payload);
    }

    /// `gh` for listing `repo`'s open pull requests, and the remote facts they are matched
    /// with; or why they cannot be listed: the repository's remote is not on GitHub, or `gh`
    /// is not installed.
    async fn gh_for_list(&self, repo: &Repo) -> std::result::Result<(Gh, RemoteFacts), Unloaded> {
        let facts = self
            .remote_facts(repo, &repo.runner, &repo.root)
            .await
            .map_err(failed)?;
        let Some(remote) = facts.base_remote() else {
            return Err((PrUnavailable::NotGithub, None, false));
        };
        let Some(on) = self.on_github(repo, &repo.runner, &facts, remote).await else {
            // A host that only gh could vouch for, and gh is not there to ask: gh is what is
            // missing.
            let unknown_host = facts
                .repo_of(remote)
                .is_some_and(|on| is_plain_host(&on.host) && !is_known_github(&on.host));
            if unknown_host && self.gh_program(repo).is_none() {
                return Err((PrUnavailable::NoGh, Some(GhFailure::NoGh), true));
            }
            return Err((PrUnavailable::NotGithub, None, true));
        };
        let Some(gh) = self.gh(repo, &repo.runner, &on) else {
            return Err((PrUnavailable::NoGh, Some(GhFailure::NoGh), true));
        };
        Ok((gh, facts))
    }

    /// `gh` for `repo`, on the repository's host `on` (one [`RepoManager::on_github`] gave),
    /// logged through `runner` (an operation's, for `pr_create`), if it is installed.
    pub(crate) fn gh(&self, repo: &Repo, runner: &GitRunner, on: &RemoteRepo) -> Option<Gh> {
        let (program, tool_path) = self.gh_program(repo)?;
        Gh::new(runner, program, tool_path, &repo.root, on, self.gh_limit())
    }

    /// `gh` as found for `repo`, and the `PATH` to run it with (`None`: git's).
    fn gh_program(&self, repo: &Repo) -> Option<(PathBuf, Option<OsString>)> {
        self.tool_program(repo, if cfg!(windows) { "gh.exe" } else { "gh" })
    }

    /// Program `name` (`gh`, `ssh`) as found for `repo` (see [`tool_on`]: never inside the
    /// repository), and the `PATH` to run it with (`None`: git's). With a tool `PATH` set
    /// (tests, the devserver) only from its first folder, in every build: never the real
    /// `gh` or `ssh`; else on the login shell's. A test build without a tool `PATH` runs
    /// none.
    fn tool_program(&self, repo: &Repo, name: &str) -> Option<(PathBuf, Option<OsString>)> {
        let tool_path = lock(&self.tool_path).clone();
        #[cfg(any(test, feature = "testutil"))]
        tool_path.as_ref()?;
        let program = tool_on(
            name,
            tool_path.as_ref(),
            self.login_path().as_ref(),
            &repo.root,
        )?;
        Some((program, tool_path))
    }

    /// The repository's remote facts ([`RemoteFacts::read`] in `cwd` through `runner`), with
    /// the ssh aliases their addresses use resolved to the real host names as `gh` does it:
    /// `ssh -G <alias>`'s `hostname` (once per alias per run of the app; the alias itself
    /// when ssh cannot tell).
    pub(crate) async fn remote_facts(
        &self,
        repo: &Repo,
        runner: &GitRunner,
        cwd: &std::path::Path,
    ) -> Result<RemoteFacts> {
        let mut facts = RemoteFacts::read(runner, cwd).await?;
        for alias in facts.ssh_hosts() {
            let known = lock(&self.ssh_hosts).get(&alias).cloned();
            let real = match known {
                Some(answer) => answer,
                None => {
                    let answer = self.ask_ssh(repo, runner, &alias).await;
                    lock(&self.ssh_hosts).insert(alias.clone(), answer.clone());
                    answer
                }
            }
            // ssh could not tell: the host as written (asked again on a user refresh).
            .unwrap_or_else(|| alias.clone());
            if real != alias {
                facts.set_real_host(alias, real);
            }
        }
        Ok(facts)
    }

    /// The real host name ssh address host `alias` stands for: `ssh -G -- <alias>`'s
    /// `hostname` line (no connection is made), asked only for a plain host name and taken
    /// only when it is one ([`is_plain_host`]). Run as git's children are (no terminal, stdin
    /// null), within 5 s, logged as a read (its output, the user's ssh settings, is not kept).
    async fn ask_ssh(&self, repo: &Repo, runner: &GitRunner, alias: &str) -> Option<String> {
        // ssh puts the host into commands of the user's ssh config: a plain name only.
        if !is_plain_host(alias) {
            return None;
        }
        let (program, tool_path) =
            self.tool_program(repo, if cfg!(windows) { "ssh.exe" } else { "ssh" })?;
        let mut ssh = runner.tool(program, "ssh");
        if tool_path.is_some() {
            ssh = ssh.with_path(tool_path);
        }
        let out = ssh
            .run(
                &repo.root,
                &["-G", "--", alias],
                CmdKind::Read,
                StdinPolicy::Null,
                crate::git::runner::TimeLimit::total(SSH_TIMEOUT),
            )
            .await
            .ok()?
            .ok()?;
        (out.code == 0).then(|| ssh_hostname(&out.text())).flatten()
    }

    fn gh_limit(&self) -> Duration {
        Duration::from_millis(
            self.gh_timeout_ms
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// The web hosts (`host[:port]`) the browser may be sent to for repository `repo`:
    /// github.com, and the GitHub host of each of its remotes on GitHub, as
    /// [`RepoManager::on_github`] resolves it.
    pub(crate) async fn github_web_hosts(&self, repo: &Repo) -> Vec<String> {
        let mut hosts = vec!["github.com".to_owned()];
        let Ok(facts) = self.remote_facts(repo, &repo.runner, &repo.root).await else {
            return hosts;
        };
        let names: Vec<String> = facts.remote_names().map(str::to_owned).collect();
        for name in names {
            if let Some(on) = self.on_github(repo, &repo.runner, &facts, &name).await {
                let web = on.web.to_ascii_lowercase();
                if !hosts.contains(&web) {
                    hosts.push(web);
                }
            }
        }
        hosts
    }

    /// The repository `remote` names, when its host is GitHub: github.com and `*.ghe.com` by
    /// name ([`is_known_github`]); any other host only when `gh` has credentials stored for
    /// exactly that host ([`Gh::has_stored_credentials`], asked once per host per run of the
    /// app, through `runner`). Otherwise `None`: no `gh` command runs for it, no `GH_HOST`
    /// names it, and no compare page is made for it.
    pub(crate) async fn on_github(
        &self,
        repo: &Repo,
        runner: &GitRunner,
        facts: &RemoteFacts,
        remote: &str,
    ) -> Option<RemoteRepo> {
        let on = facts.repo_of(remote)?;
        // Only a plain host name is ever GitHub, asked about or named to `gh`.
        if !is_plain_host(&on.host) {
            return None;
        }
        if is_known_github(&on.host) {
            return Some(on);
        }
        if let Some(known) = lock(&self.gh_hosts).get(&on.host).copied() {
            return known.then_some(on);
        }
        let (program, tool_path) = self.gh_program(repo)?;
        let stored = Gh::has_stored_credentials(
            runner,
            program,
            tool_path,
            &repo.root,
            &on.host,
            self.gh_limit(),
        )
        .await?;
        lock(&self.gh_hosts).insert(on.host.clone(), stored);
        stored.then_some(on)
    }

    /// What to offer for a new pull request of local branch `branch` (see [`PrSuggestion`]):
    /// the title is the commit's subject when the branch has one commit not on the base, else
    /// the branch's name as words; the description lists the subjects; the base is the one
    /// last used here (when it is still a branch on the remote), else the repository's
    /// default branch (`gh repo view`), else the remote's HEAD, else `main`. `existing` is asked
    /// of `gh` for this branch (`gh pr list --head <branch>`), never taken from the list shown,
    /// which may not hold it.
    pub async fn pr_suggest(&self, repo_id: u32, branch: &str) -> Result<PrSuggestion> {
        // On the heap: its future is large (it asks git and gh several things).
        Box::pin(self.suggest(repo_id, branch)).await
    }

    async fn suggest(&self, repo_id: u32, branch: &str) -> Result<PrSuggestion> {
        let repo = self.get(repo_id)?;
        crate::git::names::check_branch_name(branch)
            .map_err(|e| CoreError::new(ErrorCode::BadRequest, e.message))?;
        let root = &repo.root;
        let run = |args: Vec<String>| {
            let runner = repo.runner.clone();
            let root = root.clone();
            async move {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                runner.run_unchecked(&root, &args, CmdKind::Read).await
            }
        };
        let exists = |full: String| {
            let run = &run;
            async move {
                run(vec![
                    "rev-parse".into(),
                    "--verify".into(),
                    "--quiet".into(),
                    full,
                ])
                .await
                .is_ok_and(|o| o.code == 0)
            }
        };
        if !exists(format!("refs/heads/{branch}")).await {
            return Err(CoreError::new(
                ErrorCode::BadRequest,
                format!("There is no local branch named \u{201c}{branch}\u{201d}."),
            ));
        }
        let facts = self.remote_facts(&repo, &repo.runner, root).await?;
        let base_remote = facts.base_remote().map(str::to_owned);
        let on = match base_remote.as_deref() {
            Some(r) => self.on_github(&repo, &repo.runner, &facts, r).await,
            None => None,
        };
        let gh = on.as_ref().and_then(|on| self.gh(&repo, &repo.runner, on));

        // The base: the one remembered, if still on the remote; gh's default branch; the
        // remote's HEAD; main.
        let on_remote = |name: &str| {
            base_remote
                .as_ref()
                .map(|r| format!("refs/remotes/{r}/{name}"))
        };
        let remembered = self
            .memory()
            .and_then(|m| m.pr_base(root))
            .filter(|b| crate::git::names::check_branch_name(b).is_ok());
        let mut base = None;
        if let Some(b) = remembered
            && let Some(full) = on_remote(&b)
            && exists(full).await
        {
            base = Some(b);
        }
        // gh is asked two things at once, under one time limit: the default branch (when no
        // base is remembered) and the branch's open pull request (`gh pr list --head`, never
        // the list shown, which may not hold it).
        let (default_branch, existing) = match &gh {
            Some(gh) => {
                let need_view = base.is_none();
                let view = async {
                    if !need_view {
                        return None;
                    }
                    let args = ["repo", "view", gh.repo(), "--json", "defaultBranchRef"];
                    match gh.run(&args, CmdKind::Read, StdinPolicy::Null).await {
                        Ok(Ok(out)) => parse_default_branch(&out.stdout),
                        _ => None,
                    }
                };
                let limit = LIST_LIMIT.to_string();
                let open = async {
                    let args = [
                        "pr",
                        "list",
                        "--repo",
                        gh.repo(),
                        "--head",
                        branch,
                        "--state",
                        "open",
                        "--limit",
                        &limit,
                        "--json",
                        PR_FIELDS,
                    ];
                    let Ok(Ok(out)) = gh.run(&args, CmdKind::Read, StdinPolicy::Null).await else {
                        return None;
                    };
                    let data = PrData {
                        listed: parse_list(&out.stdout).ok()?,
                        facts: facts.clone(),
                    };
                    data.of_branch(branch).cloned()
                };
                tokio::time::timeout(
                    self.gh_limit(),
                    Box::pin(async { tokio::join!(view, open) }),
                )
                .await
                .unwrap_or((None, None))
            }
            None => (None, None),
        };
        if base.is_none() {
            base = default_branch;
        }
        if base.is_none()
            && let Some(r) = &base_remote
        {
            let head = format!("refs/remotes/{r}/HEAD");
            if let Ok(out) = run(vec!["symbolic-ref".into(), "--quiet".into(), head]).await
                && out.code == 0
            {
                base = out
                    .text()
                    .trim()
                    .strip_prefix(&format!("refs/remotes/{r}/"))
                    .map(str::to_owned);
            }
        }
        let base = base.unwrap_or_else(|| "main".to_owned());

        // The branch's commits not on the base (on the remote, else here).
        let mut since = None;
        if let Some(full) = on_remote(&base)
            && exists(full.clone()).await
        {
            since = Some(full);
        } else if exists(format!("refs/heads/{base}")).await {
            since = Some(format!("refs/heads/{base}"));
        }
        let mut args = vec![
            "log".to_owned(),
            "--no-merges".to_owned(),
            "--format=%s".to_owned(),
            format!("--max-count={MAX_SUBJECTS}"),
            format!("refs/heads/{branch}"),
        ];
        if let Some(since) = since {
            args.push(format!("^{since}"));
        }
        args.push("--".to_owned());
        let subjects: Vec<String> = match run(args).await {
            Ok(out) if out.code == 0 => {
                let mut s: Vec<String> = out.text().lines().map(str::to_owned).collect();
                s.reverse();
                s
            }
            _ => Vec::new(),
        };
        let title = match subjects.as_slice() {
            [only] => only.clone(),
            _ => humanise(branch),
        };
        let body = subjects
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n");

        let push = push_plan(&repo.runner, root, &facts, branch).await?;
        let repo_name = base_remote
            .as_deref()
            .and_then(|r| on.clone().or_else(|| facts.repo_of(r)))
            .map(|r| format!("{}/{}", r.owner, r.name));
        let compare = on.as_ref().map(|on| {
            let pushed = facts.push_remote(branch).and_then(|r| facts.repo_of(r));
            let fork = pushed
                .filter(|p| !p.owner.eq_ignore_ascii_case(&on.owner))
                .map(|p| p.owner);
            compare_url(on, &base, branch, fork.as_deref())
        });
        Ok(PrSuggestion {
            title,
            body,
            base,
            base_remote,
            existing,
            compare_url: compare,
            repo: repo_name,
            push_to: push.as_ref().map(|(r, _)| format!("{r}/{branch}")),
            needs_push: push.as_ref().is_some_and(|(_, needs)| *needs),
            push_remote: push.map(|(r, _)| r),
        })
    }

    /// Least time between two loads of a repository's pull requests (see
    /// [`RepoManager::set_prs_every`]).
    fn prs_every(&self) -> Duration {
        #[cfg(any(test, feature = "testutil"))]
        if let Some(every) = *lock(&self.prs_every) {
            return every;
        }
        LOAD_EVERY
    }

    /// Sets the least time between two loads of a repository's pull requests (tests only;
    /// normally 60 s).
    #[cfg(any(test, feature = "testutil"))]
    pub fn set_prs_every(&self, every: Duration) {
        *lock(&self.prs_every) = Some(every);
    }

    /// Sets the `PATH` `gh` and `ssh` are run with, instead of the login shell's, found in
    /// its first folder only: for tests and the devserver only (a fake `gh` and `ssh`; the
    /// `tool-path` feature, never in the app).
    #[cfg(feature = "tool-path")]
    pub fn set_tool_path(&self, path: Option<std::ffi::OsString>) {
        *lock(&self.tool_path) = path;
    }

    /// Sets how long a `gh` command may run (tests only; normally 60 s).
    #[cfg(any(test, feature = "testutil"))]
    pub fn set_gh_timeout(&self, timeout: Duration) {
        self.gh_timeout_ms.store(
            crate::repo::manager::duration_ms(timeout),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// How many loads of repository `repo_id`'s pull requests have ended (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn prs_loads(&self, repo_id: u32) -> u64 {
        self.get(repo_id).map_or(0, |r| *r.prs_loads.borrow())
    }

    /// Resolves once more than `n` loads of repository `repo_id`'s pull requests have ended
    /// (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub async fn wait_prs_loads(&self, repo_id: u32, n: u64) {
        let Ok(repo) = self.get(repo_id) else {
            return;
        };
        let mut loads = repo.prs_loads.subscribe();
        let _ = loads.wait_for(|done| *done > n).await;
    }
}

/// Where `pr_create` pushes local `branch` before opening its pull request, and whether it
/// must: the branch's push remote ([`RemoteFacts::push_remote`]) and whether its namesake
/// there is missing or behind it (as last fetched). `None` when the remote is not clear.
/// The suggestion and `pr_create` both ask this, so what the dialog says is what is done.
pub(crate) async fn push_plan(
    runner: &GitRunner,
    cwd: &std::path::Path,
    facts: &RemoteFacts,
    branch: &str,
) -> Result<Option<(String, bool)>> {
    let Some(remote) = facts.push_remote(branch) else {
        return Ok(None);
    };
    let tracking = format!("refs/remotes/{remote}/{branch}");
    let there = runner
        .run_unchecked(
            cwd,
            &["rev-parse", "--verify", "--quiet", &tracking],
            CmdKind::Read,
        )
        .await?;
    let needs = if there.code != 0 {
        true
    } else {
        let range = format!("{tracking}..refs/heads/{branch}");
        let out = runner
            .run_unchecked(cwd, &["rev-list", "--count", &range, "--"], CmdKind::Read)
            .await?;
        out.code != 0 || out.text().trim() != "0"
    };
    Ok(Some((remote.to_owned(), needs)))
}
