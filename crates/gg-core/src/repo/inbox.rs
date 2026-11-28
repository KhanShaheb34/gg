//! The Pull requests page's work (see [`crate::inbox`] for what is asked and how it is read):
//!
//! - `pr_inbox`: the page as gg kept it (at once, no `gh`), or loaded again: one `gh api
//!   graphql` per GitHub host (github.com, and the GitHub hosts of the open repositories'
//!   remotes and of the page's last load, each as `RepoManager::on_github` tells them), all at
//!   once, at most once every 10 s (a request meanwhile gets the last answer; one during a load
//!   waits for it). `gh` runs as for the Pull request tab, in a folder of no repository.
//! - A pull request's details and the writes to it (`inbox_pr_details`, `inbox_pr_write`),
//!   as the Pull request tab reads and writes them, for a repository named by its host, owner
//!   and name rather than an open one.
//! - Which open repositories have its repository as a remote (`inbox_repo_match`).
//! - Cloning a repository (`clone_repo`): `git clone --progress` through the runner, into a
//!   new folder of a folder the user chose, its progress as `clone-progress` events, Cancel
//!   (`clone_cancel`) ending git as a fetch's Stop does.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;

use crate::clone::{CloneSource, check_dest, clone_args, clone_failure, make_dest};
use crate::dto::{
    InboxProblem, InboxRepoMatch, InboxWritten, Notice, OpRequest, PrDetails, PrInbox,
    PrUnavailable,
};
use crate::error::{CoreError, ErrorCode, Result};
use crate::gh::{Gh, GhFailure};
use crate::git::runner::{CmdKind, Expired, StdinPolicy, TimeLimit, batch_ssh_command};
use crate::hosts::{RemoteRepo, is_known_github, is_plain_host};
use crate::inbox::{
    DETAILS_BATCH, DETAILS_QUERY, Details, InboxStore, PER_SEARCH, QUERY, key_of, plain_repo,
    problem_of, problem_of_text,
};
use crate::notice::as_sentence;
use crate::ops::Step;
use crate::ops::pr::{Write, check_write, send};
use crate::repo::fetch::unix_ms;
use crate::repo::manager::{RepoManager, lock};
use crate::repo::pr_details::{DetailsLoad, cached, unloaded};

/// Least time between two loads of the page.
const LOAD_EVERY: Duration = Duration::from_secs(10);
/// A clone gives up after this long without output, or this long in all.
const CLONE_IDLE: Duration = Duration::from_secs(120);
const CLONE_TOTAL: Duration = Duration::from_secs(2 * 60 * 60);
/// Fewest milliseconds between two `clone-progress` events of one clone.
const PROGRESS_EVERY_MS: u128 = 100;

/// The page's state in the manager.
#[derive(Default)]
pub(crate) struct InboxState {
    /// What gg keeps of the page (`pr-inbox.json`); none until the app gives it.
    store: Mutex<Option<Arc<InboxStore>>>,
    /// The load running, or the last one: when it started, and whether it has ended.
    load: Mutex<Option<(Instant, tokio::sync::watch::Receiver<bool>)>>,
    /// Each pull request's last (or running) load of its details, by key.
    details: Mutex<HashMap<String, DetailsLoad>>,
    /// The clones running, by the UI's id, each with its Cancel.
    clones: Mutex<HashMap<String, Arc<Notify>>>,
    /// Stops asked for clones not running yet (the request may reach the core after its
    /// Stop): such a clone, when it comes, never starts git. The newest few only.
    stopped_early: Mutex<std::collections::VecDeque<String>>,
    /// Clones that ended, the newest few: a Stop for one of them is too late, and is not kept
    /// for a later clone of that id.
    ended: Mutex<std::collections::VecDeque<String>>,
    /// Tests and the devserver only: least time between two loads, instead of 10 s.
    #[cfg(any(test, feature = "testutil", feature = "tool-path"))]
    every: Mutex<Option<Duration>>,
    /// Tests and the devserver only: clones come from `<this>/<owner>/<name>.git`, a local
    /// folder, instead of the host.
    #[cfg(feature = "tool-path")]
    clone_source: Mutex<Option<PathBuf>>,
    /// Tests and the devserver only: the program the clone source answers through (it runs
    /// `git upload-pack "$@"` itself).
    #[cfg(feature = "tool-path")]
    clone_hook: Mutex<Option<PathBuf>>,
}

/// How a clone ended (`clone_repo`). Serialized camelCase.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cloned {
    /// `done`, `failed` or `stopped` (Cancel).
    pub status: &'static str,
    /// The repository's folder, when done.
    pub path: Option<String>,
    /// What happened, in one plain line.
    pub message: String,
    /// What happened, as a headline and the line under it, on failure.
    pub notice: Option<Notice>,
    /// git's own output, on failure.
    pub detail: Option<String>,
}

/// The program name of `gh` here.
pub(super) fn gh_name() -> &'static str {
    if cfg!(windows) { "gh.exe" } else { "gh" }
}

/// Where `gh` runs for the page: a folder of no repository.
pub(super) fn neutral_dir() -> PathBuf {
    std::env::temp_dir()
}

fn bad(message: impl Into<String>) -> CoreError {
    CoreError::new(ErrorCode::BadRequest, message)
}

/// Checks a pull request's repository as the UI names it, and its number.
fn check_pr(host: &str, owner: &str, name: &str, number: u32) -> Result<()> {
    if !plain_repo(host, owner, name) {
        return Err(bad("That is not a repository gg can name to gh."));
    }
    if number == 0 {
        return Err(bad("No pull request is number 0."));
    }
    Ok(())
}

impl RepoManager {
    /// Gives the page the store it keeps its data in (the app's `pr-inbox.json`).
    pub fn set_inbox_store(&self, store: Arc<InboxStore>) {
        *lock(&self.inbox.store) = Some(store);
    }

    fn inbox_store(&self) -> Result<Arc<InboxStore>> {
        lock(&self.inbox.store)
            .clone()
            .ok_or_else(|| CoreError::new(ErrorCode::Io, "The pull requests page is not set up."))
    }

    /// The page: as kept (`refresh` false: no `gh`), or loaded again first (see the module
    /// comment). `gh` failing is not an error: each host says why in the page.
    pub async fn pr_inbox(self: &Arc<Self>, refresh: bool) -> Result<PrInbox> {
        let store = self.inbox_store()?;
        if !refresh {
            return Ok(store.data().page(false));
        }
        let every = self.inbox_every();
        // The load running, or the last one when it ended less than the floor ago; else a new
        // one, on its own task: a caller that goes away (a closed HTTP request) neither stops
        // it halfway nor lets the next request start a second one beside it.
        let mut done = {
            let mut load = lock(&self.inbox.load);
            match &*load {
                Some((started, done)) if !*done.borrow() || started.elapsed() < every => {
                    done.clone()
                }
                _ => {
                    let (tx, rx) = tokio::sync::watch::channel(false);
                    *load = Some((Instant::now(), rx.clone()));
                    let (m, kept) = (self.clone(), store.clone());
                    tokio::spawn(async move {
                        m.load_inbox(kept).await;
                        let _ = tx.send(true);
                    });
                    rx
                }
            }
        };
        let _ = done.wait_for(|ended| *ended).await;
        Ok(store.data().page(true))
    }

    /// Records pull request `key` as seen now (its page was opened in gg). Unknown keys are
    /// ignored.
    pub async fn pr_inbox_seen(&self, key: &str) -> Result<()> {
        let store = self.inbox_store()?;
        let key = key.to_owned();
        let _ = tokio::task::spawn_blocking(move || store.change(|d| d.mark_seen(&key, unix_ms())))
            .await;
        Ok(())
    }

    /// Asks every GitHub host for its pull requests and keeps the answers.
    async fn load_inbox(self: &Arc<Self>, store: Arc<InboxStore>) {
        let (hosts, from_repos) = self.inbox_hosts(&store).await;
        let cwd = neutral_dir();
        let Some((program, tool_path)) = self.tool_program_at(None, gh_name()) else {
            let (problem, message) = problem_of(&GhFailure::NoGh, "github.com");
            let _ = tokio::task::spawn_blocking(move || {
                store.change(|d| {
                    // Every host shown says it, those of the last load too (without gh they
                    // cannot be told GitHub, but their list stays, as of then).
                    let kept: Vec<String> = d.hosts.keys().cloned().collect();
                    for host in hosts.iter().chain(kept.iter()) {
                        d.take_failure(host, problem, message.clone(), None);
                    }
                });
            })
            .await;
            return;
        };
        let mut asked = tokio::task::JoinSet::new();
        for host in hosts {
            let on = RemoteRepo {
                host: host.clone(),
                web: host.clone(),
                owner: String::new(),
                name: String::new(),
            };
            let Some(gh) = Gh::new(
                &self.runner,
                program.clone(),
                tool_path.clone(),
                &cwd,
                &on,
                self.gh_limit(),
            ) else {
                continue;
            };
            asked.spawn(async move {
                let answer = ask_inbox(&gh, &host).await;
                (host, answer)
            });
        }
        let mut answers = Vec::new();
        while let Some(done) = asked.join_next().await {
            if let Ok(done) = done {
                answers.push(done);
            }
        }
        let now = unix_ms();
        let _ = tokio::task::spawn_blocking(move || {
            store.change(|d| {
                // The page's first load is the starting point for every host's answer,
                // whichever answered first.
                let first = !d.started;
                for (host, answer) in answers {
                    match answer {
                        Ok(answer) => d.take_answer(&host, answer, now, first),
                        // A host only an open repository (or the last load) named, which gh
                        // is not signed in to now: no longer asked.
                        Err((InboxProblem::SignedOut, _, _))
                            if host != "github.com" && !from_repos.contains(&host) =>
                        {
                            d.forget_host(&host);
                        }
                        Err((problem, message, detail)) => {
                            d.take_failure(&host, problem, message, detail);
                        }
                    }
                }
            });
        })
        .await;
    }

    /// The hosts the page asks: github.com, the GitHub hosts of the open repositories'
    /// remotes, and those of the last load still GitHub; and which came from the open
    /// repositories.
    async fn inbox_hosts(&self, store: &InboxStore) -> (Vec<String>, HashSet<String>) {
        let mut hosts = vec!["github.com".to_owned()];
        let mut from_repos = HashSet::new();
        let repos: Vec<_> = lock(&self.repos).values().cloned().collect();
        for repo in repos {
            let Ok(facts) = self.remote_facts(&repo, &repo.runner, &repo.root).await else {
                continue;
            };
            let names: Vec<String> = facts.remote_names().map(str::to_owned).collect();
            for name in names {
                if let Some(on) = self.on_github(&repo, &repo.runner, &facts, &name).await {
                    from_repos.insert(on.host.clone());
                    if !hosts.contains(&on.host) {
                        hosts.push(on.host);
                    }
                }
            }
        }
        let kept: Vec<String> = store.data().hosts.keys().cloned().collect();
        for host in kept {
            if !hosts.contains(&host) && self.is_github_host(&host).await {
                hosts.push(host);
            }
        }
        (hosts, from_repos)
    }

    /// Whether `host` is GitHub: github.com and `*.ghe.com` by name, any other host only when
    /// `gh` has credentials stored for it (asked once per host per run of the app, as for a
    /// repository's remote).
    pub(crate) async fn is_github_host(&self, host: &str) -> bool {
        if !is_plain_host(host) {
            return false;
        }
        if is_known_github(host) {
            return true;
        }
        if let Some(known) = lock(&self.gh_hosts).get(host).copied() {
            return known;
        }
        let cwd = neutral_dir();
        let Some((program, tool_path)) = self.tool_program_at(None, gh_name()) else {
            return false;
        };
        let Some(stored) = Gh::has_stored_credentials(
            &self.runner,
            program,
            tool_path,
            &cwd,
            host,
            self.gh_limit(),
        )
        .await
        else {
            return false;
        };
        lock(&self.gh_hosts).insert(host.to_owned(), stored);
        stored
    }

    /// `gh` for repository `owner/name` on GitHub host `host`, run in a folder of no
    /// repository; or why there is none, as the details say it.
    async fn inbox_gh(&self, host: &str, owner: &str, name: &str) -> Result<Gh> {
        if !self.is_github_host(host).await {
            return Err(unloaded(PrUnavailable::NotGithub, None));
        }
        let cwd = neutral_dir();
        let Some((program, tool_path)) = self.tool_program_at(None, gh_name()) else {
            return Err(unloaded(PrUnavailable::NoGh, Some(&GhFailure::NoGh)));
        };
        let on = RemoteRepo {
            host: host.to_owned(),
            web: host.to_owned(),
            owner: owner.to_owned(),
            name: name.to_owned(),
        };
        Gh::new(&self.runner, program, tool_path, &cwd, &on, self.gh_limit())
            .ok_or_else(|| unloaded(PrUnavailable::NotGithub, None))
    }

    /// Pull request `number` of `owner/name` on `host`, as `pr_details` reads one (at most
    /// once every 10 s per pull request).
    pub async fn inbox_pr_details(
        &self,
        host: &str,
        owner: &str,
        name: &str,
        number: u32,
    ) -> Result<PrDetails> {
        check_pr(host, owner, name, number)?;
        let key = key_of(host, owner, name, number);
        cached(
            &self.inbox.details,
            key,
            self.pr_details_every(),
            || async {
                let gh = self.inbox_gh(host, owner, name).await?;
                self.details_from(&gh, number).await
            },
        )
        .await
    }

    /// Writes `request` (`pr_reply`, `pr_resolve` or `pr_comment`) to its pull request of
    /// `owner/name` on `host`, as the Pull request tab does; once done, its details are
    /// forgotten so the next load asks `gh`.
    pub async fn inbox_pr_write(
        &self,
        host: &str,
        owner: &str,
        name: &str,
        request: &OpRequest,
    ) -> Result<InboxWritten> {
        let (number, what) = match request {
            OpRequest::PrReply {
                number,
                thread_id,
                body,
            } => {
                check_write(*number, Some(thread_id), Some(body))?;
                (
                    *number,
                    Write::Reply {
                        thread: thread_id,
                        body,
                    },
                )
            }
            OpRequest::PrResolve {
                number,
                thread_id,
                resolved,
            } => {
                check_write(*number, Some(thread_id), None)?;
                (
                    *number,
                    Write::Resolve {
                        thread: thread_id,
                        resolved: *resolved,
                    },
                )
            }
            OpRequest::PrComment { number, body } => {
                check_write(*number, None, Some(body))?;
                (*number, Write::Comment { body })
            }
            _ => {
                return Err(bad(
                    "Only a reply, a comment or resolving is sent from here.",
                ));
            }
        };
        check_pr(host, owner, name, number)?;
        let gh = match self.inbox_gh(host, owner, name).await {
            Ok(gh) => gh,
            Err(e) => {
                return Ok(InboxWritten {
                    done: false,
                    message: format!("Nothing was sent. {}", e.message),
                    detail: e.detail,
                });
            }
        };
        let step = send(&gh, &|_| {}, number, what).await?;
        Ok(match step {
            Step::Done(message) => {
                lock(&self.inbox.details).remove(&key_of(host, owner, name, number));
                InboxWritten {
                    done: true,
                    message: as_sentence(&message),
                    detail: None,
                }
            }
            Step::Failed {
                message, detail, ..
            } => InboxWritten {
                done: false,
                message: as_sentence(&message),
                detail: Some(detail).filter(|d| !d.trim().is_empty()),
            },
            Step::DoneWith { message, .. } | Step::Stopped { message, .. } => InboxWritten {
                done: true,
                message: as_sentence(&message),
                detail: None,
            },
        })
    }

    /// Reads the details (state, checks, review decision, activity) of the page's pull
    /// requests `keys` (listed or pinned): one batched `gh api graphql` per host and
    /// [`DETAILS_BATCH`] of them, all at once; the page as it is after. A host that fails
    /// keeps what was read before (its rows say nothing new until the next).
    pub async fn pr_inbox_details(self: &Arc<Self>, keys: &[String]) -> Result<PrInbox> {
        let store = self.inbox_store()?;
        let asked_at = unix_ms();
        let data = store.data();
        let wanted = data.ids_of(keys);
        let cwd = neutral_dir();
        let Some((program, tool_path)) = self.tool_program_at(None, gh_name()) else {
            return Ok(data.page(false));
        };
        let mut asked = tokio::task::JoinSet::new();
        for (host, ids) in wanted {
            let on = RemoteRepo {
                host: host.clone(),
                web: host.clone(),
                owner: String::new(),
                name: String::new(),
            };
            let Some(gh) = Gh::new(
                &self.runner,
                program.clone(),
                tool_path.clone(),
                &cwd,
                &on,
                self.gh_limit(),
            ) else {
                continue;
            };
            let gh = Arc::new(gh);
            let viewer = data.login_of(&host);
            for batch in ids.chunks(DETAILS_BATCH) {
                let (gh, host, viewer, batch) =
                    (gh.clone(), host.clone(), viewer.clone(), batch.to_vec());
                asked.spawn(async move {
                    let found = ask_details(&gh, &host, &batch, viewer.as_deref()).await;
                    (host, found)
                });
            }
        }
        let mut found = Vec::new();
        while let Some(done) = asked.join_next().await {
            if let Ok((host, Some(details))) = done {
                found.push((host, details));
            }
        }
        // As of when it was asked: an answer to an older request is never taken over this one's.
        let now = asked_at;
        let page = tokio::task::spawn_blocking(move || {
            store.change(|d| {
                for (host, details) in found {
                    d.take_details(&host, details, now);
                }
                d.page(false)
            })
        })
        .await
        .map_err(|e| {
            CoreError::new(ErrorCode::Io, "Could not keep the details").with_detail(e.to_string())
        })?;
        Ok(page)
    }

    /// Pins pull request `key` to the top of the page (or, `pinned` false, unpins it); the
    /// page after. Kept in the page's file.
    pub async fn pr_inbox_pin(&self, key: &str, pinned: bool) -> Result<PrInbox> {
        let store = self.inbox_store()?;
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || {
            store.change(|d| {
                if d.set_pin(&key, pinned) {
                    Ok(d.page(false))
                } else {
                    Err(bad("That pull request is not on the page."))
                }
            })
        })
        .await
        .map_err(|e| {
            CoreError::new(ErrorCode::Io, "Could not keep the pin").with_detail(e.to_string())
        })?
    }

    /// How the page groups its list, from now on (kept in the page's file).
    pub async fn pr_inbox_group_by(&self, by: crate::dto::InboxGroupBy) -> Result<()> {
        let store = self.inbox_store()?;
        let _ = tokio::task::spawn_blocking(move || store.change(|d| d.group_by = by)).await;
        Ok(())
    }

    /// The files pull request `number` of `owner/name` on `host` changes: `gh api --paginate
    /// repos/<owner>/<name>/pulls/<number>/files?per_page=100` (GitHub lists at most 3000).
    pub async fn inbox_pr_files(
        &self,
        host: &str,
        owner: &str,
        name: &str,
        number: u32,
    ) -> Result<crate::dto::PrFiles> {
        check_pr(host, owner, name, number)?;
        let gh = self.inbox_gh(host, owner, name).await?;
        let path = format!("repos/{owner}/{name}/pulls/{number}/files?per_page=100");
        let args = ["api", "--hostname", gh.host(), "--paginate", &path];
        match gh.run(&args, CmdKind::Read, StdinPolicy::Null).await? {
            Ok(out) => crate::inbox::parse_files(&out.stdout).map_err(|why| {
                CoreError::new(
                    ErrorCode::Io,
                    "Could not read the files: gh's answer could not be read.",
                )
                .with_detail(why)
            }),
            Err(GhFailure::Failed { message, detail }) => Err(CoreError::new(
                ErrorCode::Io,
                format!("Could not read the files: {message}"),
            )
            .with_detail(detail)),
            Err(failure) => Err(unloaded(failure.reason(), Some(&failure))),
        }
    }

    /// The open repositories that have `owner/name` on `host` as a remote (by their remotes'
    /// addresses, ssh aliases resolved as for the Pull request tab), each with the remote that
    /// names `head_repo` (`owner/name` of the pull request's head), if one does. By id.
    pub async fn inbox_repo_match(
        &self,
        host: &str,
        owner: &str,
        name: &str,
        head_repo: Option<&str>,
    ) -> Result<Vec<InboxRepoMatch>> {
        if !plain_repo(host, owner, name) {
            return Err(bad("That is not a repository gg can name."));
        }
        let names = |r: &RemoteRepo, owner: &str, name: &str| {
            r.host.eq_ignore_ascii_case(host)
                && r.owner.eq_ignore_ascii_case(owner)
                && r.name.eq_ignore_ascii_case(name)
        };
        let head = head_repo.and_then(|h| h.split_once('/'));
        let mut repos: Vec<_> = lock(&self.repos).values().cloned().collect();
        repos.sort_by_key(|r| r.identity.repo_id);
        let mut found = Vec::new();
        for repo in repos {
            let Ok(facts) = self.remote_facts(&repo, &repo.runner, &repo.root).await else {
                continue;
            };
            let remotes: Vec<(String, Option<RemoteRepo>)> = facts
                .remote_names()
                .map(|r| (r.to_owned(), facts.repo_of(r)))
                .collect();
            if !remotes
                .iter()
                .any(|(_, on)| on.as_ref().is_some_and(|on| names(on, owner, name)))
            {
                continue;
            }
            let head_remote = head.and_then(|(o, n)| {
                // The base repository's own remote first, when the head is there too.
                remotes
                    .iter()
                    .filter(|(_, on)| on.as_ref().is_some_and(|on| names(on, o, n)))
                    .map(|(r, _)| r.clone())
                    .min_by_key(|r| (facts.base_remote() != Some(r.as_str()), r.clone()))
            });
            found.push(InboxRepoMatch {
                repo_id: repo.identity.repo_id,
                head_remote,
            });
        }
        Ok(found)
    }

    /// The hosts the page lists (each answered once at least, or was asked).
    pub fn inbox_host_names(&self) -> Vec<String> {
        self.inbox_store()
            .map(|s| s.data().hosts.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// The folder the last clone went into, if any.
    pub fn clone_parent(&self) -> Option<String> {
        self.inbox_store().ok()?.data().clone_parent
    }

    /// Clones `source` (see the module comment) into the folder `folder` of `parent`: a
    /// repository on a GitHub host by the protocol `gh` is set to use for git there (`gh config
    /// get git_protocol`: `ssh`, else `https`), or an address [`crate::clone::read_link`] kept.
    /// `clone_id` names it for [`Self::clone_cancel`] and its `clone-progress` events. Refused
    /// (`bad_request`) for a parent that is not a full path, a folder name that is not one plain
    /// name, or something not empty there already (see [`crate::clone::check_dest`]). The
    /// parent folders missing are made; a clone that does not finish (failed, stopped, timed
    /// out) takes away what it made and nothing else (see [`crate::clone::Made::undo`]).
    pub async fn clone_repo(
        &self,
        clone_id: &str,
        source: &CloneSource,
        parent: &Path,
        folder: &str,
    ) -> Result<Cloned> {
        let what = match source {
            CloneSource::Repo { host, owner, name } => {
                if !plain_repo(host, owner, name) {
                    return Err(bad("That is not a repository gg can name."));
                }
                format!("{owner}/{name}")
            }
            // Named by its owner and name when the link has them (a link is long to read).
            CloneSource::Url(url) => crate::hosts::parse_remote(url)
                .map_or_else(|| url.clone(), |r| format!("{}/{}", r.owner, r.name)),
        };
        if clone_id.is_empty() || clone_id.len() > 100 {
            return Err(bad("A clone needs a name to be cancelled by."));
        }
        if !parent.is_absolute() {
            return Err(CoreError::new(
                ErrorCode::PathMissing,
                format!("{} is not a full path.", parent.display()),
            ));
        }
        if let Some(problem) = check_dest(parent, folder).problem {
            return Err(bad(problem));
        }
        // Counted in at once (refused while gg restarts to update, and holding a restart off
        // until it ends), so a Stop before git starts is kept: git then never starts.
        let cancel = Arc::new(Notify::new());
        let mut taken = false;
        self.admit(|| {
            let mut clones = lock(&self.inbox.clones);
            taken = clones.contains_key(clone_id);
            if !taken {
                clones.insert(clone_id.to_owned(), cancel.clone());
            }
        })?;
        if taken {
            return Err(bad("A clone of that name runs already."));
        }
        let _registered = Registered {
            manager: self,
            id: clone_id,
        };
        let shown = crate::notice::name(&what);
        let stopped = || Cloned {
            status: "stopped",
            path: None,
            message: format!("The clone of {what} was stopped."),
            notice: None,
            detail: None,
        };
        // Stopped before it got here.
        {
            let mut early = lock(&self.inbox.stopped_early);
            if let Some(at) = early.iter().position(|id| id == clone_id) {
                early.remove(at);
                return Ok(stopped());
            }
        }
        let (url, local) = match source {
            CloneSource::Repo { host, owner, name } => {
                if !self.is_github_host(host).await {
                    return Err(bad(format!(
                        "{host} is not a GitHub host gh is signed in to."
                    )));
                }
                self.clone_url(host, owner, name).await
            }
            CloneSource::Url(url) => self.link_url(url),
        };
        // The folder (and the parents missing), made now: what a clone that does not finish
        // takes away.
        let made = {
            let (parent, folder) = (parent.to_owned(), folder.to_owned());
            tokio::task::spawn_blocking(move || make_dest(&parent, &folder))
                .await
                .map_err(|e| {
                    CoreError::new(ErrorCode::Io, "The clone failed").with_detail(e.to_string())
                })?
                .map_err(bad)?
        };
        let made = Arc::new(made);
        let undo = || {
            let made = made.clone();
            async move {
                let _ = tokio::task::spawn_blocking(move || made.undo()).await;
            }
        };
        let progress = CloneProgress::new(self, clone_id);
        let dest_text = made.dest().to_string_lossy().into_owned();
        let args = clone_args(&url, &dest_text, local, self.clone_hook_args());
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let has_env = |var: &str| std::env::var_os(var).is_some_and(|v| !v.is_empty());
        let runner =
            self.runner
                .clone()
                .with_ssh_command(batch_ssh_command(cfg!(windows), has_env, false));
        let ran = runner
            .run_with_progress_cancellable(
                parent,
                &args,
                CmdKind::Write,
                TimeLimit::idle(CLONE_IDLE, CLONE_TOTAL),
                &|chunk| progress.seen(chunk),
                &cancel,
            )
            .await;
        let failed = |said: String, detail: String| {
            let notice = crate::notice::notice(&said);
            Cloned {
                status: "failed",
                path: None,
                message: notice.plain(),
                notice: Some(notice),
                detail: Some(detail).filter(|d| !d.trim().is_empty()),
            }
        };
        match ran {
            Ok(Ok(out)) if out.code == 0 => {
                if let Ok(store) = self.inbox_store() {
                    let parent = parent.to_string_lossy().into_owned();
                    let _ = tokio::task::spawn_blocking(move || {
                        store.change(|d| d.clone_parent = Some(parent));
                    })
                    .await;
                }
                Ok(Cloned {
                    status: "done",
                    path: Some(dest_text),
                    message: format!("Cloned {what}."),
                    notice: None,
                    detail: None,
                })
            }
            Ok(Ok(out)) => {
                undo().await;
                Ok(failed(
                    clone_failure(&shown, &out.stderr, out.code),
                    out.stderr,
                ))
            }
            Ok(Err(gave_up)) if gave_up.limit == Expired::Cancelled => {
                undo().await;
                Ok(stopped())
            }
            Ok(Err(gave_up)) => {
                undo().await;
                let why = crate::ops::explain::timed_out(gave_up.limit);
                Ok(failed(
                    crate::notice::said(format!("Could not clone {shown}"), why.trim()),
                    gave_up.stderr,
                ))
            }
            Err(e) => {
                undo().await;
                Err(e)
            }
        }
    }

    /// Whether a clone runs (restarting gg to update waits until it ends).
    pub(crate) fn clone_running(&self) -> bool {
        !lock(&self.inbox.clones).is_empty()
    }

    /// Stops clone `clone_id` if it runs: git is ended with what it started (it removes the
    /// folder it made). Whether it was running; if it was not, a clone of that id that comes
    /// later is stopped before git starts. A clone git already finished stays done.
    pub fn clone_cancel(&self, clone_id: &str) -> bool {
        // Under the clones' lock: a clone registering now is either seen here or sees this.
        let clones = lock(&self.inbox.clones);
        if let Some(cancel) = clones.get(clone_id) {
            cancel.notify_one();
            return true;
        }
        // Ended already: nothing to stop, and the Stop must not touch a later clone.
        if lock(&self.inbox.ended).iter().any(|id| id == clone_id) {
            return false;
        }
        // Not running (yet): kept, so one that comes after its Stop never starts.
        let mut early = lock(&self.inbox.stopped_early);
        if !early.iter().any(|id| id == clone_id) {
            early.push_back(clone_id.to_owned());
            while early.len() > 16 {
                early.pop_front();
            }
        }
        false
    }

    /// The address `owner/name` on `host` is cloned from (see [`Self::clone_repo`]), and
    /// whether it is a folder of the clone source (tests and the devserver only).
    async fn clone_url(&self, host: &str, owner: &str, name: &str) -> (String, bool) {
        if let Some(local) = self.clone_source_url(owner, name) {
            return (local, true);
        }
        let ssh = match self.git_protocol(host).await {
            Some(p) => p == "ssh",
            None => false,
        };
        let url = if ssh {
            format!("git@{host}:{owner}/{name}.git")
        } else {
            format!("https://{host}/{owner}/{name}.git")
        };
        (url, false)
    }

    /// The address a link is cloned from: as [`crate::clone::read_link`] kept it, or (tests and
    /// the devserver only) the clone source's folder for its `owner/name`; and whether it is
    /// that folder.
    fn link_url(&self, url: &str) -> (String, bool) {
        if let Some(on) = crate::hosts::parse_remote(url)
            && let Some(local) = self.clone_source_url(&on.owner, &on.name)
        {
            return (local, true);
        }
        (url.to_owned(), false)
    }

    /// Tests and the devserver only: `owner/name`'s folder in the clone source, as a `file://`
    /// address (so git clones through its pack protocol, with progress, as from a host).
    #[allow(unused_variables)]
    fn clone_source_url(&self, owner: &str, name: &str) -> Option<String> {
        #[cfg(feature = "tool-path")]
        if let Some(dir) = lock(&self.inbox.clone_source).clone() {
            let path = dir.join(owner).join(format!("{name}.git"));
            return Some(
                url::Url::from_file_path(&path)
                    .map_or_else(|()| path.to_string_lossy().into_owned(), |u| u.to_string()),
            );
        }
        None
    }

    /// Tests and the devserver only: the program the clone source answers through, when one
    /// is set (git's `--upload-pack`).
    fn clone_hook_args(&self) -> Option<String> {
        #[cfg(feature = "tool-path")]
        if let Some(hook) = lock(&self.inbox.clone_hook).clone() {
            return Some(hook.to_string_lossy().into_owned());
        }
        None
    }

    /// What `gh config get git_protocol --host <host>` says, if it says.
    async fn git_protocol(&self, host: &str) -> Option<String> {
        let cwd = neutral_dir();
        let (program, tool_path) = self.tool_program_at(None, gh_name())?;
        let on = RemoteRepo {
            host: host.to_owned(),
            web: host.to_owned(),
            owner: String::new(),
            name: String::new(),
        };
        let gh = Gh::new(&self.runner, program, tool_path, &cwd, &on, self.gh_limit())?;
        let out = gh
            .run(
                &["config", "get", "git_protocol", "--host", host],
                CmdKind::Read,
                StdinPolicy::Null,
            )
            .await
            .ok()?
            .ok()?;
        Some(out.text().trim().to_ascii_lowercase())
    }

    fn inbox_every(&self) -> Duration {
        #[cfg(any(test, feature = "testutil", feature = "tool-path"))]
        if let Some(every) = *lock(&self.inbox.every) {
            return every;
        }
        LOAD_EVERY
    }

    /// Sets the least time between two loads of the page (tests and the devserver only;
    /// normally 10 s).
    #[cfg(any(test, feature = "testutil", feature = "tool-path"))]
    pub fn set_inbox_every(&self, every: Duration) {
        *lock(&self.inbox.every) = Some(every);
    }

    /// Makes clones come from `<dir>/<owner>/<name>.git` (tests and the devserver only: a
    /// local folder, never the network).
    #[cfg(feature = "tool-path")]
    pub fn set_clone_source(&self, dir: Option<PathBuf>) {
        *lock(&self.inbox.clone_source) = dir;
    }

    /// Makes the clone source answer through `hook` (git's `--upload-pack`: run with the
    /// source's path, it runs `git upload-pack "$@"` itself), to slow a clone down or hold it
    /// halfway (tests and the devserver only).
    #[cfg(feature = "tool-path")]
    pub fn set_clone_hook(&self, hook: Option<PathBuf>) {
        *lock(&self.inbox.clone_hook) = hook;
    }
}

/// Host `gh`'s answer to the page's query: its pull requests, or why not (the problem, a plain
/// line, and gh's words).
async fn ask_inbox(
    gh: &Gh,
    host: &str,
) -> std::result::Result<crate::inbox::Answer, (InboxProblem, String, Option<String>)> {
    let (query, first) = (format!("query={QUERY}"), format!("first={PER_SEARCH}"));
    // The host named: gh's own default may be another (one signed in to alone).
    let args = [
        "api",
        "graphql",
        "--hostname",
        host,
        "-f",
        &query,
        "-F",
        &first,
    ];
    match gh.run(&args, CmdKind::Read, StdinPolicy::Null).await {
        Ok(Ok(out)) => crate::inbox::parse_answer(&out.stdout, host).map_err(|why| {
            let problem = problem_of_text(&why).unwrap_or(InboxProblem::Failed);
            let message = match problem {
                InboxProblem::Failed => format!("Could not load them: {why}"),
                _ => {
                    problem_of(
                        &GhFailure::Failed {
                            message: why.clone(),
                            detail: String::new(),
                        },
                        host,
                    )
                    .1
                }
            };
            (problem, message, Some(why))
        }),
        Ok(Err(failure)) => {
            let (problem, message) = problem_of(&failure, host);
            Err((problem, message, Some(failure.detail())))
        }
        Err(e) => Err((
            InboxProblem::Failed,
            format!("Could not load them: {}", e.message),
            e.detail,
        )),
    }
}

/// The details of pull requests `ids` on `host`, asked as `viewer`; `None` when `gh` could not
/// give them (the page keeps what it had).
async fn ask_details(
    gh: &Gh,
    host: &str,
    ids: &[String],
    viewer: Option<&str>,
) -> Option<Vec<(String, Details)>> {
    let query = format!("query={DETAILS_QUERY}");
    let mut args: Vec<String> = vec![
        "api".into(),
        "graphql".into(),
        "--hostname".into(),
        host.into(),
        "-f".into(),
        query,
    ];
    for id in ids {
        // `-f`: a string as it is; `ids[]=`: one element of the list each.
        args.push("-f".into());
        args.push(format!("ids[]={id}"));
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match gh.run(&args, CmdKind::Read, StdinPolicy::Null).await {
        Ok(Ok(out)) => crate::inbox::parse_details(&out.stdout, viewer).ok(),
        _ => None,
    }
}

/// Sends the latest progress line of a clone as `clone-progress`, at most every
/// [`PROGRESS_EVERY_MS`]: a line that comes sooner is sent once that time is up, unless a later
/// one replaces it, so the last line before git goes quiet (a slow host) is always shown.
struct CloneProgress {
    sink: Arc<dyn crate::EventSink>,
    id: String,
    state: Arc<Mutex<ProgressState>>,
}

#[derive(Default)]
struct ProgressState {
    /// The unfinished line.
    partial: String,
    /// When the last event went.
    sent: Option<Instant>,
    /// A line waiting for its turn, and whether a send of it is due.
    waiting: Option<String>,
    due: bool,
}

impl CloneProgress {
    fn new(manager: &RepoManager, id: &str) -> Self {
        Self {
            sink: manager.sink.clone(),
            id: id.to_owned(),
            state: Arc::default(),
        }
    }

    fn seen(&self, chunk: &[u8]) {
        let mut state = lock(&self.state);
        let Some(line) = crate::ops::latest_line(&mut state.partial, chunk) else {
            return;
        };
        let every = Duration::from_millis(PROGRESS_EVERY_MS as u64);
        let wait = state
            .sent
            .map_or(Duration::ZERO, |at| every.saturating_sub(at.elapsed()));
        if wait.is_zero() {
            state.sent = Some(Instant::now());
            state.waiting = None;
            drop(state);
            emit_progress(&*self.sink, &self.id, line);
            return;
        }
        state.waiting = Some(line);
        if state.due {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        state.due = true;
        let (sink, id, shared) = (self.sink.clone(), self.id.clone(), self.state.clone());
        runtime.spawn(async move {
            tokio::time::sleep(wait).await;
            let line = {
                let mut state = lock(&shared);
                state.due = false;
                state.sent = Some(Instant::now());
                state.waiting.take()
            };
            if let Some(line) = line {
                emit_progress(&*sink, &id, line);
            }
        });
    }
}

fn emit_progress(sink: &dyn crate::EventSink, id: &str, line: String) {
    sink.emit(
        "clone-progress",
        serde_json::json!({ "cloneId": id, "line": line }),
    );
}

/// A running clone's entry in the manager, removed when the clone ends however it ends.
struct Registered<'a> {
    manager: &'a RepoManager,
    id: &'a str,
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        let mut clones = lock(&self.manager.inbox.clones);
        clones.remove(self.id);
        let mut ended = lock(&self.manager.inbox.ended);
        ended.push_back(self.id.to_owned());
        while ended.len() > 16 {
            ended.pop_front();
        }
    }
}
