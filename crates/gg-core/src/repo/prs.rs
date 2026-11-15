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
    Gh, GhFailure, LIST_LIMIT, PR_FIELDS, PrData, RemoteFacts, find_gh, humanise,
    parse_default_branch, parse_list, unavailable_message,
};
use crate::git::runner::{CmdKind, GitRunner, StdinPolicy};
use crate::hosts::{RemoteRepo, compare_url, is_known_github};
use crate::repo::fetch::unix_ms;
use crate::repo::manager::{ErrorKind, Repo, RepoManager, lock};

/// Least time between two loads of one repository's pull requests (but for one just made).
const LOAD_EVERY: Duration = Duration::from_secs(60);
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

/// How a load ended: the list, or why there is none (with `gh`'s failure, and whether it is
/// worth saying: not for a repository without remotes).
type Loaded = std::result::Result<PrData, (PrUnavailable, Option<GhFailure>, bool)>;

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
        let snapshot = self.refresh(repo_id).await?;
        if let Ok(repo) = self.get(repo_id) {
            self.spawn_load_prs(&repo, PrTrigger::User);
        }
        Ok(snapshot)
    }

    async fn load_prs(self: &Arc<Self>, repo: &Arc<Repo>, trigger: PrTrigger) {
        {
            let mut gate = lock(&repo.prs_gate);
            if gate.running {
                gate.again |= trigger == PrTrigger::Created;
                return;
            }
            let every = self.prs_every();
            if trigger != PrTrigger::Created && gate.last.is_some_and(|t| t.elapsed() < every) {
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
        // Shown as loading while nothing is shown yet.
        let loading = {
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
        let loaded = self.list_prs(repo).await;
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

    /// Lists `repo`'s open pull requests: `gh pr list --state open --limit 1000 --json …` in
    /// the main worktree, unless the repository's remote is not on GitHub or `gh` is not
    /// installed.
    async fn list_prs(&self, repo: &Repo) -> Loaded {
        let failed = |e: CoreError| {
            let failure = GhFailure::Failed {
                message: e.message,
                detail: e.detail.unwrap_or_default(),
            };
            (PrUnavailable::Failed, Some(failure), true)
        };
        let facts = RemoteFacts::read(&repo.runner, &repo.root)
            .await
            .map_err(failed)?;
        let Some(remote) = facts.base_remote() else {
            return Err((PrUnavailable::NotGithub, None, false));
        };
        let Some(on) = self.on_github(repo, &repo.runner, &facts, remote).await else {
            return Err((PrUnavailable::NotGithub, None, true));
        };
        let Some(gh) = self.gh(repo, &repo.runner, &on) else {
            return Err((PrUnavailable::NoGh, Some(GhFailure::NoGh), true));
        };
        let limit = LIST_LIMIT.to_string();
        let args = [
            "pr", "list", "--state", "open", "--limit", &limit, "--json", PR_FIELDS,
        ];
        match gh.run(&args, CmdKind::Read, StdinPolicy::Null).await {
            Ok(Ok(out)) => match parse_list(&out.stdout) {
                Ok(listed) => Ok(PrData { listed, facts }),
                Err(why) => Err(failed(
                    CoreError::new(ErrorCode::Io, "gh's answer could not be read.")
                        .with_detail(why),
                )),
            },
            Ok(Err(failure)) => Err((failure.reason(), Some(failure), true)),
            Err(e) => Err(failed(e)),
        }
    }

    /// `gh` for `repo`, on the repository's host `on` (one [`RepoManager::on_github`] gave),
    /// logged through `runner` (an operation's, for `pr_create`), if it is installed.
    pub(crate) fn gh(&self, repo: &Repo, runner: &GitRunner, on: &RemoteRepo) -> Option<Gh> {
        let (program, tool_path) = self.gh_program(repo)?;
        Some(Gh::new(
            runner,
            program,
            tool_path,
            &repo.root,
            &on.host,
            self.gh_limit(),
        ))
    }

    /// `gh` as found for `repo`, and the `PATH` to run it with (`None`: git's).
    fn gh_program(&self, repo: &Repo) -> Option<(PathBuf, Option<OsString>)> {
        #[cfg(any(test, feature = "testutil"))]
        let tool_path = lock(&self.tool_path).clone();
        #[cfg(not(any(test, feature = "testutil")))]
        let tool_path: Option<OsString> = None;
        let path = tool_path.clone().or_else(|| self.login_path());
        let program = find_gh(path.as_ref(), &repo.root)?;
        Some((program, tool_path))
    }

    fn gh_limit(&self) -> Duration {
        Duration::from_millis(
            self.gh_timeout_ms
                .load(std::sync::atomic::Ordering::Relaxed),
        )
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
        let facts = RemoteFacts::read(&repo.runner, root).await?;
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
        if base.is_none()
            && let Some(gh) = &gh
            && let Ok(Ok(out)) = gh
                .run(
                    &["repo", "view", "--json", "defaultBranchRef"],
                    CmdKind::Read,
                    StdinPolicy::Null,
                )
                .await
        {
            base = parse_default_branch(&out.stdout);
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

        // Its open pull request, asked for this branch.
        let mut existing = None;
        if let Some(gh) = &gh
            && let Ok(Ok(out)) = gh
                .run(
                    &[
                        "pr", "list", "--head", branch, "--state", "open", "--json", PR_FIELDS,
                    ],
                    CmdKind::Read,
                    StdinPolicy::Null,
                )
                .await
            && let Ok(listed) = parse_list(&out.stdout)
        {
            let data = PrData {
                listed,
                facts: facts.clone(),
            };
            existing = data.of_branch(branch).cloned();
        }

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
            existing,
            compare_url: compare,
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

    /// Sets the `PATH` `gh` is found on and run with (tests only: a fake `gh`).
    #[cfg(any(test, feature = "testutil"))]
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
