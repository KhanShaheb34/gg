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

use tokio::sync::{Notify, OnceCell};
use tokio::time::Instant;

use crate::dto::{
    InboxProblem, InboxRepoMatch, InboxWritten, OpRequest, PrDetails, PrInbox, PrUnavailable,
};
use crate::error::{CoreError, ErrorCode, Result};
use crate::gh::{Gh, GhFailure};
use crate::git::runner::{CmdKind, Expired, StdinPolicy, TimeLimit};
use crate::hosts::{RemoteRepo, is_known_github, is_plain_host};
use crate::inbox::{
    InboxStore, PER_SEARCH, QUERY, key_of, plain_repo, problem_of, problem_of_text,
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
    /// The load running, or the last one, and when it started.
    load: Mutex<Option<(Instant, Arc<OnceCell<()>>)>>,
    /// Each pull request's last (or running) load of its details, by key.
    details: Mutex<HashMap<String, DetailsLoad>>,
    /// The clones running, by the UI's id, each with its Cancel.
    clones: Mutex<HashMap<String, Arc<Notify>>>,
    /// Tests and the devserver only: least time between two loads, instead of 10 s.
    #[cfg(any(test, feature = "testutil", feature = "tool-path"))]
    every: Mutex<Option<Duration>>,
    /// Tests and the devserver only: clones come from `<this>/<owner>/<name>.git`, a local
    /// folder, instead of the host.
    #[cfg(feature = "tool-path")]
    clone_source: Mutex<Option<PathBuf>>,
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
    /// git's own output, on failure.
    pub detail: Option<String>,
}

/// The program name of `gh` here.
fn gh_name() -> &'static str {
    if cfg!(windows) { "gh.exe" } else { "gh" }
}

/// Where `gh` runs for the page: a folder of no repository.
fn neutral_dir() -> PathBuf {
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
        let cell = {
            let mut load = lock(&self.inbox.load);
            match &*load {
                Some((started, cell)) if cell.get().is_none() || started.elapsed() < every => {
                    cell.clone()
                }
                _ => {
                    let cell = Arc::new(OnceCell::new());
                    *load = Some((Instant::now(), cell.clone()));
                    cell
                }
            }
        };
        let (m, kept) = (self.clone(), store.clone());
        cell.get_or_init(|| async move {
            // On its own task: a caller that goes away (a closed HTTP request) does not
            // stop it halfway.
            let _ = tokio::spawn(async move { m.load_inbox(kept).await }).await;
        })
        .await;
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
                    for host in &hosts {
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
                for (host, answer) in answers {
                    match answer {
                        Ok(answer) => d.take_answer(&host, answer, now),
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

    /// Clones `owner/name` of GitHub host `host` into a new folder `<parent>/<name>` (see the
    /// module comment), by the protocol `gh` is set to use for git there (`gh config get
    /// git_protocol`: `ssh`, else `https`). `clone_id` names it for [`Self::clone_cancel`]
    /// and its `clone-progress` events. Refused (`bad_request`) for a parent that is not an
    /// existing absolute folder, or a folder of that name already there.
    pub async fn clone_repo(
        &self,
        clone_id: &str,
        host: &str,
        owner: &str,
        name: &str,
        parent: &Path,
    ) -> Result<Cloned> {
        if !plain_repo(host, owner, name) {
            return Err(bad("That is not a repository gg can name."));
        }
        if clone_id.is_empty() || clone_id.len() > 100 {
            return Err(bad("A clone needs a name to be cancelled by."));
        }
        if !parent.is_absolute() || !parent.is_dir() {
            return Err(CoreError::new(
                ErrorCode::PathMissing,
                format!("{} is not a folder.", parent.display()),
            ));
        }
        let dest = parent.join(name);
        if std::fs::symlink_metadata(&dest).is_ok() {
            return Err(bad(format!(
                "There is a {name} in {} already. Choose another folder.",
                parent.display()
            )));
        }
        if !self.is_github_host(host).await {
            return Err(bad(format!(
                "{host} is not a GitHub host gh is signed in to."
            )));
        }
        let url = self.clone_url(host, owner, name).await;
        let cancel = Arc::new(Notify::new());
        lock(&self.inbox.clones).insert(clone_id.to_owned(), cancel.clone());
        let progress = CloneProgress {
            manager: self,
            id: clone_id,
            state: Mutex::new((String::new(), None)),
        };
        let dest_text = dest.to_string_lossy().into_owned();
        let ran = self
            .runner
            .run_with_progress_cancellable(
                parent,
                &["clone", "--progress", "--", &url, &dest_text],
                CmdKind::Write,
                TimeLimit::idle(CLONE_IDLE, CLONE_TOTAL),
                &|chunk| progress.seen(chunk),
                &cancel,
            )
            .await;
        lock(&self.inbox.clones).remove(clone_id);
        let repo = format!("{owner}/{name}");
        match ran? {
            Ok(out) if out.code == 0 => {
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
                    message: format!("Cloned {repo}."),
                    detail: None,
                })
            }
            Ok(out) => {
                let why = crate::notice::telling_line(&out.stderr)
                    .unwrap_or_else(|| format!("git exited with code {}.", out.code));
                Ok(Cloned {
                    status: "failed",
                    path: None,
                    message: format!("Could not clone {repo}: {why}"),
                    detail: Some(out.stderr).filter(|d| !d.trim().is_empty()),
                })
            }
            Err(gave_up) if gave_up.limit == Expired::Cancelled => Ok(Cloned {
                status: "stopped",
                path: None,
                message: format!("The clone of {repo} was stopped."),
                detail: None,
            }),
            Err(gave_up) => Ok(Cloned {
                status: "failed",
                path: None,
                message: format!(
                    "Could not clone {repo}: {}",
                    crate::ops::explain::timed_out(gave_up.limit).trim()
                ),
                detail: Some(gave_up.stderr).filter(|d| !d.trim().is_empty()),
            }),
        }
    }

    /// Stops clone `clone_id` if it runs: git is ended with what it started (it removes the
    /// folder it made). Whether it was running.
    pub fn clone_cancel(&self, clone_id: &str) -> bool {
        match lock(&self.inbox.clones).get(clone_id) {
            Some(cancel) => {
                cancel.notify_one();
                true
            }
            None => false,
        }
    }

    /// The address `owner/name` on `host` is cloned from (see [`Self::clone_repo`]).
    async fn clone_url(&self, host: &str, owner: &str, name: &str) -> String {
        #[cfg(feature = "tool-path")]
        if let Some(dir) = lock(&self.inbox.clone_source).clone() {
            return dir
                .join(owner)
                .join(format!("{name}.git"))
                .to_string_lossy()
                .into_owned();
        }
        let ssh = match self.git_protocol(host).await {
            Some(p) => p == "ssh",
            None => false,
        };
        if ssh {
            format!("git@{host}:{owner}/{name}.git")
        } else {
            format!("https://{host}/{owner}/{name}.git")
        }
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
}

/// Host `gh`'s answer to the page's query: its pull requests, or why not (the problem, a plain
/// line, and gh's words).
async fn ask_inbox(
    gh: &Gh,
    host: &str,
) -> std::result::Result<crate::inbox::Answer, (InboxProblem, String, Option<String>)> {
    let (query, first) = (format!("query={QUERY}"), format!("first={PER_SEARCH}"));
    let args = ["api", "graphql", "-f", &query, "-F", &first];
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

/// Sends the latest progress line of a clone as `clone-progress`, at most every
/// [`PROGRESS_EVERY_MS`].
struct CloneProgress<'a> {
    manager: &'a RepoManager,
    id: &'a str,
    /// The unfinished line, and when the last event went.
    state: Mutex<(String, Option<Instant>)>,
}

impl CloneProgress<'_> {
    fn seen(&self, chunk: &[u8]) {
        let line = {
            let mut state = lock(&self.state);
            let Some(line) = crate::ops::latest_line(&mut state.0, chunk) else {
                return;
            };
            if state
                .1
                .is_some_and(|at| at.elapsed().as_millis() < PROGRESS_EVERY_MS)
            {
                return;
            }
            state.1 = Some(Instant::now());
            line
        };
        self.manager.sink.emit(
            "clone-progress",
            serde_json::json!({ "cloneId": self.id, "line": line }),
        );
    }
}
