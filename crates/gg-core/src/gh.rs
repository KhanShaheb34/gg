//! Pull requests through the GitHub CLI (`gh`), which the user installed and signed in to:
//! the app never reads, stores or asks for a token. `gh` is found on the login `PATH` (never
//! through a relative entry, never inside the repository), run as an argument array through
//! the runner (no terminal, stdin null or the bytes given, 60 s, logged and masked) with
//! `GH_PROMPT_DISABLED=1`, `GH_NO_UPDATE_NOTIFIER=1`, `NO_COLOR=1`, and `GH_HOST` only for a
//! host other than github.com. It runs in the main worktree, and every call names the
//! repository the app chose with `--repo` (`GH_REPO` removed), so the app and `gh` never pick
//! different ones (P3-R26). `gh` runs `git` there itself: that git gets the
//! protection the app's own git children have, through the environment
//! ([`crate::git::runner::git_protection_env`]).
//!
//! Which hosts are GitHub: github.com and `*.ghe.com` by name; any other host only when `gh`
//! has credentials stored for exactly that host ([`Gh::has_stored_credentials`], asked with
//! the token variables removed and its output never read). A command for a host other than
//! github.com runs without `GH_ENTERPRISE_TOKEN` / `GITHUB_ENTERPRISE_TOKEN`, so a token in
//! the user's environment is never sent to a host because a repository's remote names it.
//!
//! Also here: what the repository's config says of its remotes (which one a branch is pushed
//! to, which one `gh` takes for the repository), and which branches the pull requests listed
//! belong to.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::dto::{PrUnavailable, PullRequest, PullRequestState, RefKind};
use crate::error::Result;
use crate::git::refs::RefInfo;
use crate::git::runner::{
    CmdKind, Expired, GitOutput, GitRunner, StdinPolicy, TimeLimit, ToolRunner,
};
use crate::hosts::{RemoteRepo, is_plain_host, parse_remote, ssh_host};

/// The variables `gh` takes a token from instead of its stored credentials.
const TOKEN_VARS: [&str; 4] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
];

/// How long a `gh` command may run.
pub(crate) const GH_TIMEOUT: Duration = Duration::from_secs(60);
/// Most open pull requests listed; as many listed means some may be missing (`partial`).
pub(crate) const LIST_LIMIT: usize = 1000;
/// The fields `gh pr list` is asked for.
pub(crate) const PR_FIELDS: &str =
    "number,title,url,isDraft,headRefName,baseRefName,headRepositoryOwner,isCrossRepository";

/// Program `name` (`gh`, `ssh`; `.exe` on Windows) for the repository at `root`: with a
/// tool `PATH` set (`tool_path`: tests and the devserver, for a fake `gh` and `ssh`), only
/// from its first folder, in every build, so a real program in a later folder never runs;
/// else on `login` (see [`find_tool`]).
pub(crate) fn tool_on(
    name: &str,
    tool_path: Option<&OsString>,
    login: Option<&OsString>,
    root: &Path,
) -> Option<PathBuf> {
    match tool_path {
        Some(tool_path) => {
            let first = std::env::split_paths(tool_path).next()?.into_os_string();
            find_tool(name, Some(&first), root)
        }
        None => find_tool(name, login, root),
    }
}

/// Program `name` (`gh`, `ssh`; `.exe` on Windows), found on `path` (the login shell's
/// `PATH`, else the app's): the first in an absolute entry, unless it lies inside the
/// repository at `root` (a program of the repository's never runs).
pub(crate) fn find_tool(name: &str, path: Option<&OsString>, root: &Path) -> Option<PathBuf> {
    let found = crate::launch::find_program(OsStr::new(name), path)?;
    let real = dunce::canonicalize(&found).unwrap_or_else(|_| found.clone());
    let root = dunce::canonicalize(root).unwrap_or_else(|_| root.to_owned());
    (!real.starts_with(&root) && !found.starts_with(&root)).then_some(found)
}

/// Why a `gh` command did not give an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GhFailure {
    /// `gh` is not installed.
    NoGh,
    /// `gh` is not signed in (its output, for the detail).
    SignedOut(String),
    /// Anything else: a plain line, and `gh`'s output.
    Failed { message: String, detail: String },
}

impl GhFailure {
    /// The reason as the snapshot gives it.
    pub(crate) fn reason(&self) -> PrUnavailable {
        match self {
            Self::NoGh => PrUnavailable::NoGh,
            Self::SignedOut(_) => PrUnavailable::SignedOut,
            Self::Failed { .. } => PrUnavailable::Failed,
        }
    }

    /// `gh`'s output, if it said anything.
    pub(crate) fn detail(&self) -> String {
        match self {
            Self::NoGh => String::new(),
            Self::SignedOut(detail) | Self::Failed { detail, .. } => detail.clone(),
        }
    }
}

/// Said for each reason pull requests are not shown.
pub(crate) fn unavailable_message(reason: PrUnavailable, failure: Option<&GhFailure>) -> String {
    match reason {
        PrUnavailable::NoGh => {
            let mut message = "Pull requests need the GitHub CLI (gh), which was not found. \
                               Install it from cli.github.com and sign in with gh auth login."
                .to_owned();
            if cfg!(windows) {
                // A gh.cmd or gh.bat shim is never run (it would go through cmd.exe).
                message.push_str(" gg runs gh.exe itself only, never a .cmd or .bat file.");
            }
            message
        }
        PrUnavailable::SignedOut => "The GitHub CLI (gh) is not signed in. Run gh auth login in \
                                     a terminal, then refresh."
            .to_owned(),
        PrUnavailable::NotGithub => "Pull requests are shown for GitHub repositories only; this \
                                     repository's remote is elsewhere."
            .to_owned(),
        PrUnavailable::Failed => match failure {
            Some(GhFailure::Failed { message, .. }) => {
                format!("Could not load pull requests: {message}")
            }
            _ => "Could not load pull requests.".to_owned(),
        },
    }
}

/// Runs `gh` for one repository, the one the app chose: every command names it (`--repo`,
/// or `gh repo view`'s argument), so `gh` never picks one of its own from the remotes, nor
/// from an inherited `GH_REPO` or `GH_HOST`.
pub(crate) struct Gh {
    runner: ToolRunner,
    cwd: PathBuf,
    limit: TimeLimit,
    /// The repository as `gh` takes it: `owner/name` on github.com, else `host/owner/name`.
    repo: String,
}

impl Gh {
    /// `gh` at `program`, logged as `git`'s commands are (its repository, its operation and
    /// Stop), run in `cwd` with `path` as its `PATH` (`None`: git's), for repository `on`
    /// (on a host known to be GitHub), given up on after `limit`. `GH_HOST` is set only for
    /// a host other than github.com (and removed for github.com); `GH_REPO` is never passed
    /// on. `None` for a host that is not a plain name ([`is_plain_host`]): it is never
    /// named to `gh`.
    pub(crate) fn new(
        git: &GitRunner,
        program: PathBuf,
        path: Option<OsString>,
        cwd: &Path,
        on: &RemoteRepo,
        limit: Duration,
    ) -> Option<Self> {
        if !is_plain_host(&on.host) {
            return None;
        }
        let mut runner = git.tool(program, "gh");
        if path.is_some() {
            runner = runner.with_path(path);
        }
        runner = Self::base(runner);
        let host = on.host.as_str();
        let repo = if host == "github.com" {
            runner = runner.without_env("GH_HOST");
            format!("{}/{}", on.owner, on.name)
        } else {
            runner = runner
                .with_env("GH_HOST", host)
                .without_env("GH_ENTERPRISE_TOKEN")
                .without_env("GITHUB_ENTERPRISE_TOKEN");
            format!("{host}/{}/{}", on.owner, on.name)
        };
        Some(Self {
            runner,
            cwd: cwd.to_owned(),
            limit: TimeLimit::total(limit),
            repo,
        })
    }

    /// The repository every command names (`--repo <this>`).
    pub(crate) fn repo(&self) -> &str {
        &self.repo
    }

    /// What every `gh` command gets: no prompt, no update notice, no colour, no repository
    /// but the one named, and the protection of the git it runs.
    fn base(runner: ToolRunner) -> ToolRunner {
        runner
            .with_env("GH_PROMPT_DISABLED", "1")
            .with_env("GH_NO_UPDATE_NOTIFIER", "1")
            .with_env("NO_COLOR", "1")
            .without_env("GH_REPO")
            .with_git_protection()
    }

    /// Whether `gh` (at `program`, with `path` as for [`Gh::new`]) has credentials stored
    /// for exactly `host`: `gh auth token --hostname <host>` exits 0. Its output (the
    /// token) is discarded, never read, and the command log records the command alone; the
    /// token variables of the environment are removed, so only credentials stored for that
    /// host count. `None` when it could not tell (it did not start or answer in time).
    pub(crate) async fn has_stored_credentials(
        git: &GitRunner,
        program: PathBuf,
        path: Option<OsString>,
        cwd: &Path,
        host: &str,
        limit: Duration,
    ) -> Option<bool> {
        if !is_plain_host(host) {
            return Some(false);
        }
        let mut runner = git.tool(program, "gh");
        if path.is_some() {
            runner = runner.with_path(path);
        }
        runner = Self::base(runner).without_env("GH_HOST");
        for var in TOKEN_VARS {
            runner = runner.without_env(var);
        }
        let ran = runner
            .run_status_only(
                cwd,
                &["auth", "token", "--hostname", host],
                CmdKind::Read,
                TimeLimit::total(limit),
            )
            .await;
        match ran {
            Ok(Ok(out)) => Some(out.code == 0),
            _ => None,
        }
    }

    /// Runs `gh args`, reading `stdin`; its output when it exits 0.
    pub(crate) async fn run(
        &self,
        args: &[&str],
        kind: CmdKind,
        stdin: StdinPolicy,
    ) -> Result<std::result::Result<GitOutput, GhFailure>> {
        let ran = self
            .runner
            .run(&self.cwd, args, kind, stdin, self.limit)
            .await;
        let out = match ran {
            Ok(Ok(out)) => out,
            Ok(Err(gave_up)) => {
                let message = match gave_up.limit {
                    Expired::Cancelled => crate::git::runner::STOPPED.to_owned(),
                    _ => format!("gh did not answer within {} s.", self.limit.total.as_secs()),
                };
                return Ok(Err(GhFailure::Failed {
                    message,
                    detail: gave_up.stderr,
                }));
            }
            Err(e) if e.message == crate::git::runner::STOPPED => return Err(e),
            // Found a moment ago, gone now (uninstalled meanwhile).
            Err(e) if e.message == "gh was not found on PATH" => return Ok(Err(GhFailure::NoGh)),
            Err(e) => {
                return Ok(Err(GhFailure::Failed {
                    message: e.message,
                    detail: e.detail.unwrap_or_default(),
                }));
            }
        };
        if out.code == 0 {
            return Ok(Ok(out));
        }
        Ok(Err(classify(&out)))
    }
}

/// What a `gh` that exited non-zero said: not signed in (its words "not logged in", or its
/// advice to run `gh auth login`), else a failure with its first line.
pub(crate) fn classify(out: &GitOutput) -> GhFailure {
    let all = format!("{}\n{}", out.stderr, out.text());
    let lower = all.to_lowercase();
    if lower.contains("not logged in") || lower.contains("gh auth login") {
        return GhFailure::SignedOut(all.trim().to_owned());
    }
    let message = all
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map_or_else(
            || format!("gh exited with code {}.", out.code),
            str::to_owned,
        );
    GhFailure::Failed {
        message,
        detail: all.trim().to_owned(),
    }
}

/// One open pull request as `gh pr list --json` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPr {
    number: u32,
    title: String,
    url: String,
    #[serde(default)]
    is_draft: bool,
    head_ref_name: String,
    base_ref_name: String,
    #[serde(default)]
    head_repository_owner: Option<GhOwner>,
    #[serde(default)]
    is_cross_repository: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct GhOwner {
    login: String,
}

/// An open pull request listed, with what it takes to match it to a branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListedPr {
    pub pr: PullRequest,
    /// The owner of the repository its head branch is in.
    pub head_owner: Option<String>,
    /// Its head branch is in another repository (a fork) than its base.
    pub cross: bool,
}

/// The pull requests in `gh pr list --json` output `json`.
pub(crate) fn parse_list(json: &[u8]) -> std::result::Result<Vec<ListedPr>, String> {
    let listed: Vec<GhPr> = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    Ok(listed
        .into_iter()
        .map(|p| ListedPr {
            pr: PullRequest {
                number: p.number,
                title: p.title,
                url: p.url,
                state: if p.is_draft {
                    PullRequestState::Draft
                } else {
                    PullRequestState::Open
                },
                head: p.head_ref_name,
                base: p.base_ref_name,
            },
            head_owner: p.head_repository_owner.map(|o| o.login),
            cross: p.is_cross_repository,
        })
        .collect())
}

/// The default branch in `gh repo view --json defaultBranchRef` output.
pub(crate) fn parse_default_branch(json: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct View {
        default_branch_ref: Option<Name>,
    }
    #[derive(Deserialize)]
    struct Name {
        name: String,
    }
    let view: View = serde_json::from_slice(json).ok()?;
    view.default_branch_ref
        .map(|r| r.name)
        .filter(|n| !n.is_empty() && !n.starts_with('-'))
}

/// The address of the pull request `gh pr create` printed: its last line that is an
/// `https://` URL on `host`, with any port or none.
pub(crate) fn created_url(stdout: &str, host: &str) -> Option<String> {
    let on_host = |line: &str| {
        let Some(rest) = line.strip_prefix("https://") else {
            return false;
        };
        let Some(rest) = rest
            .get(..host.len())
            .filter(|h| h.eq_ignore_ascii_case(host))
            .map(|_| &rest[host.len()..])
        else {
            return false;
        };
        let rest = match rest.strip_prefix(':') {
            Some(port) => port.trim_start_matches(|c: char| c.is_ascii_digit()),
            None => rest,
        };
        rest.starts_with('/')
    };
    stdout
        .lines()
        .map(str::trim)
        .rfind(|l| on_host(l) && !l.contains(char::is_whitespace))
        .map(str::to_owned)
}

/// The pull request `gh pr create` said already exists for the branch: the URL in its
/// message ("a pull request for branch "x" into branch "main" already exists:\n<url>").
pub(crate) fn already_exists(output: &str, web: &str) -> Option<String> {
    output
        .contains("already exists")
        .then(|| created_url(output, web))
        .flatten()
}

/// The number at the end of a pull request's URL (`…/pull/12`).
pub(crate) fn number_of(url: &str) -> Option<u32> {
    url.trim_end_matches('/').rsplit('/').next()?.parse().ok()
}

/// What the repository's config says of its remotes: their addresses, where each branch is
/// pushed, and which remote `gh` was told is the repository (`gh repo set-default`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RemoteFacts {
    /// Remote name → its addresses, best first: as configured (`url`, then `pushurl`), then as
    /// `git remote -v` gives them with `insteadOf` rewrites applied (a shorthand address).
    urls: BTreeMap<String, Vec<String>>,
    /// `branch.<b>.pushRemote`.
    branch_push_remote: HashMap<String, String>,
    /// `branch.<b>.remote` (its upstream's remote).
    branch_remote: HashMap<String, String>,
    /// `remote.pushDefault`.
    push_default: Option<String>,
    /// The remote marked `gh-resolved` (by `gh repo set-default`) and the mark's value
    /// (`base`, or the `OWNER/REPO` chosen), if any.
    resolved: Option<(String, String)>,
    /// ssh address host (maybe an alias) → the real host name it stands for.
    real_hosts: HashMap<String, String>,
}

impl RemoteFacts {
    /// Reads them in `cwd`: one `git config --get-regexp`, and `git remote -v`.
    pub(crate) async fn read(runner: &GitRunner, cwd: &Path) -> Result<Self> {
        let mut facts = Self::default();
        let mut push_urls: Vec<(String, String)> = Vec::new();
        let config = runner
            .run_unchecked(
                cwd,
                &[
                    "config",
                    "-z",
                    "--get-regexp",
                    r"^(branch\..*\.(pushremote|remote)|remote\.pushdefault|remote\..*\.(gh-resolved|url|pushurl))$",
                ],
                CmdKind::Read,
            )
            .await?;
        for item in config.stdout.split(|&b| b == 0) {
            let item = String::from_utf8_lossy(item);
            let Some((key, value)) = item.split_once('\n') else {
                continue;
            };
            if key == "remote.pushdefault" {
                facts.push_default = Some(value.to_owned());
            } else if let Some(rest) = key.strip_prefix("branch.") {
                if let Some(branch) = rest.strip_suffix(".pushremote") {
                    facts
                        .branch_push_remote
                        .insert(branch.to_owned(), value.to_owned());
                } else if let Some(branch) = rest.strip_suffix(".remote") {
                    facts
                        .branch_remote
                        .insert(branch.to_owned(), value.to_owned());
                }
            } else if let Some(rest) = key.strip_prefix("remote.") {
                if let Some(remote) = rest.strip_suffix(".gh-resolved") {
                    if !value.is_empty() {
                        facts.resolved = Some((remote.to_owned(), value.to_owned()));
                    }
                } else if let Some(remote) = rest.strip_suffix(".pushurl") {
                    push_urls.push((remote.to_owned(), value.to_owned()));
                } else if let Some(remote) = rest.strip_suffix(".url") {
                    facts
                        .urls
                        .entry(remote.to_owned())
                        .or_default()
                        .push(value.to_owned());
                }
            }
        }
        for (remote, url) in push_urls {
            facts.urls.entry(remote).or_default().push(url);
        }
        let remotes = runner.run(cwd, &["remote", "-v"], CmdKind::Read).await?;
        for line in remotes.text().lines() {
            let Some((name, rest)) = line.split_once('\t') else {
                continue;
            };
            let url = rest
                .strip_suffix(" (fetch)")
                .or_else(|| rest.strip_suffix(" (push)"))
                .unwrap_or(rest);
            let urls = facts.urls.entry(name.to_owned()).or_default();
            if !urls.iter().any(|u| u == url) {
                urls.push(url.to_owned());
            }
        }
        Ok(facts)
    }

    fn known(&self, remote: &str) -> bool {
        self.urls.contains_key(remote)
    }

    /// The hosts the remotes' ssh addresses are written with (maybe aliases of the user's
    /// ssh config), each once.
    pub(crate) fn ssh_hosts(&self) -> std::collections::BTreeSet<String> {
        self.urls
            .values()
            .flatten()
            .filter_map(|u| ssh_host(u))
            .collect()
    }

    /// Takes `real` as the host ssh address host `alias` stands for (`ssh -G`'s hostname).
    pub(crate) fn set_real_host(&mut self, alias: String, real: String) {
        self.real_hosts.insert(alias, real);
    }

    /// The remote `branch` is pushed to: its `pushRemote`, else `remote.pushDefault`, else its
    /// upstream's remote, else `origin`, else the only remote.
    pub(crate) fn push_remote(&self, branch: &str) -> Option<&str> {
        [
            self.branch_push_remote.get(branch),
            self.push_default.as_ref(),
            self.branch_remote.get(branch).filter(|r| *r != "."),
        ]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .find(|r| self.known(r))
        .or_else(|| self.known("origin").then_some("origin"))
        .or_else(|| match self.urls.keys().collect::<Vec<_>>().as_slice() {
            [only] => Some(only.as_str()),
            _ => None,
        })
    }

    /// The remote of the repository pull requests go to (the base repository) unless the
    /// user names another, the same for the list and for a new pull request: the one `gh
    /// repo set-default` chose (`remote.<r>.gh-resolved`: `base` marks that remote; an
    /// `OWNER/REPO` value names the repository, so the remote whose address names it, else
    /// the marked one), else `upstream`, `github`, `origin`, else the first. Every `gh`
    /// command is then told this repository (`--repo`).
    pub(crate) fn base_remote(&self) -> Option<&str> {
        if let Some((marked, value)) = &self.resolved
            && self.known(marked)
        {
            let named = value.split_once('/').and_then(|(owner, name)| {
                self.urls.keys().map(String::as_str).find(|r| {
                    self.repo_of(r).is_some_and(|repo| {
                        repo.owner.eq_ignore_ascii_case(owner)
                            && repo.name.eq_ignore_ascii_case(name)
                    })
                })
            });
            return Some(named.unwrap_or(marked));
        }
        ["upstream", "github", "origin"]
            .into_iter()
            .find(|r| self.known(r))
            .or_else(|| self.urls.keys().next().map(String::as_str))
    }

    /// Whether `remote` is one of the repository's remotes.
    pub(crate) fn is_remote(&self, remote: &str) -> bool {
        self.known(remote)
    }

    /// Every remote's name.
    pub(crate) fn remote_names(&self) -> impl Iterator<Item = &str> {
        self.urls.keys().map(String::as_str)
    }

    /// The repository `remote` names: the first of its addresses that names one, on the
    /// real host when the address's host is an ssh alias resolved ([`Self::set_real_host`]).
    pub(crate) fn repo_of(&self, remote: &str) -> Option<RemoteRepo> {
        self.urls.get(remote)?.iter().find_map(|u| {
            let repo = parse_remote(u)?;
            match ssh_host(u).and_then(|alias| self.real_hosts.get(&alias)) {
                Some(real) => Some(repo.on_host(real)),
                None => Some(repo),
            }
        })
    }

    /// The remote names, longest first (for splitting `origin/x`).
    fn names_longest_first(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.urls.keys().map(String::as_str).collect();
        names.sort_by_key(|n| std::cmp::Reverse(n.len()));
        names
    }
}

/// The pull requests listed, with the remote facts they were matched with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PrData {
    pub listed: Vec<ListedPr>,
    pub facts: RemoteFacts,
}

impl PrData {
    /// The open pull request whose head is branch `name` on `remote`: a pull request's head
    /// is matched only by the same name AND the same owner as the remote's (a fork's branch
    /// of the same name is another branch); with the remote's owner not known, none. The
    /// newest when there are several.
    fn on(&self, remote: Option<&str>, name: &str) -> Option<&PullRequest> {
        let owner = remote.and_then(|r| self.facts.repo_of(r)).map(|r| r.owner);
        pick(
            self.listed.iter().filter(|l| l.pr.head == name),
            owner.as_deref(),
        )
    }

    /// The open pull request of local branch `branch` (pushed to its namesake, R38).
    pub(crate) fn of_branch(&self, branch: &str) -> Option<&PullRequest> {
        self.on(self.facts.push_remote(branch), branch)
    }

    /// The markers of the branch labels `refs` that have an open pull request: local
    /// branches by where they are pushed, remote branches by their remote. The list is
    /// indexed by head name and each remote's owner read once, so this stays cheap with
    /// thousands of refs and a thousand pull requests.
    pub(crate) fn marks(&self, refs: &[RefInfo]) -> BTreeMap<(RefKind, String), PullRequest> {
        let mut by_head: HashMap<&str, Vec<&ListedPr>> = HashMap::new();
        for l in &self.listed {
            by_head.entry(l.pr.head.as_str()).or_default().push(l);
        }
        let mut owners: HashMap<&str, Option<String>> = HashMap::new();
        let remotes = self.facts.names_longest_first();
        let mut marks = BTreeMap::new();
        for r in refs {
            let (remote, name) = match r.kind {
                RefKind::Branch => (self.facts.push_remote(&r.name), r.name.as_str()),
                RefKind::Remote => {
                    let Some(split) = remotes.iter().find_map(|remote| {
                        let branch = r.name.strip_prefix(remote)?.strip_prefix('/')?;
                        Some((*remote, branch))
                    }) else {
                        continue;
                    };
                    (Some(split.0), split.1)
                }
                RefKind::Tag | RefKind::Stash => continue,
            };
            let Some(candidates) = by_head.get(name) else {
                continue;
            };
            let owner = remote.and_then(|remote| {
                owners
                    .entry(remote)
                    .or_insert_with(|| self.facts.repo_of(remote).map(|r| r.owner))
                    .clone()
            });
            if let Some(pr) = pick(candidates.iter().copied(), owner.as_deref()) {
                marks.insert((r.kind, r.name.clone()), pr.clone());
            }
        }
        marks
    }
}

/// Of `candidates` (pull requests whose head has the branch's name), the newest whose head
/// is in `owner`'s repository (one that does not say its head's owner: only from the
/// repository itself); with `owner` not known, none: the head may be anyone's branch of
/// that name.
fn pick<'a>(
    candidates: impl Iterator<Item = &'a ListedPr>,
    owner: Option<&str>,
) -> Option<&'a PullRequest> {
    let owner = owner?;
    candidates
        .filter(|l| match &l.head_owner {
            Some(head) => owner.eq_ignore_ascii_case(head),
            None => !l.cross,
        })
        .max_by_key(|l| l.pr.number)
        .map(|l| &l.pr)
}

/// The host name in `ssh -G <alias>` output `text` (its `hostname` line), if it is a plain
/// host name ([`is_plain_host`]).
pub(crate) fn ssh_hostname(text: &str) -> Option<String> {
    let host = text
        .lines()
        .find_map(|l| l.strip_prefix("hostname "))?
        .trim()
        .to_ascii_lowercase();
    is_plain_host(&host).then_some(host)
}

/// `branch` as a title: its last part, words split at `-` and `_`, the first capitalised
/// (`feature/fix-pr_loading` → `Fix pr loading`).
pub(crate) fn humanise(branch: &str) -> String {
    let last = branch.rsplit('/').next().unwrap_or(branch);
    let words = last.replace(['-', '_'], " ");
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => branch.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Task 9 review I2: with a tool `PATH` set (tests, the devserver), `gh` and `ssh` come
    /// from its first folder only, in every build: a real `gh` in a later folder (`/usr/bin`,
    /// say) never runs. Without one, the login `PATH` is searched as usual.
    #[cfg(unix)]
    #[test]
    fn a_tool_path_gives_programs_from_its_first_folder_only() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let (empty, real) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let gh = real.path().join("gh");
        std::fs::write(&gh, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let joined = |dirs: &[&Path]| std::env::join_paths(dirs).unwrap();
        let later = joined(&[empty.path(), real.path()]);
        assert_eq!(tool_on("gh", Some(&later), None, root.path()), None);
        let first = joined(&[real.path(), empty.path()]);
        assert_eq!(
            tool_on("gh", Some(&first), None, root.path()),
            Some(gh.clone())
        );
        // The login PATH is not looked at once a tool PATH is set.
        assert_eq!(tool_on("gh", Some(&later), Some(&first), root.path()), None);
        assert_eq!(tool_on("gh", None, Some(&later), root.path()), Some(gh));
    }

    fn out(code: i32, stdout: &str, stderr: &str) -> GitOutput {
        GitOutput {
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.to_owned(),
            code,
        }
    }

    #[test]
    fn signed_out_is_told_from_other_failures() {
        for said in [
            "You are not logged into any GitHub hosts. To log in, run: gh auth login\n",
            "To get started with GitHub CLI, please run:  gh auth login\n",
            "error: not logged in to github.example.com\n",
        ] {
            assert!(
                matches!(classify(&out(4, "", said)), GhFailure::SignedOut(_)),
                "{said}"
            );
        }
        let failed = classify(&out(
            1,
            "",
            "\nGraphQL: API rate limit exceeded for user ID 1.\n",
        ));
        assert_eq!(
            failed,
            GhFailure::Failed {
                message: "GraphQL: API rate limit exceeded for user ID 1.".into(),
                detail: "GraphQL: API rate limit exceeded for user ID 1.".into()
            }
        );
        assert_eq!(failed.reason(), PrUnavailable::Failed);
        assert!(
            unavailable_message(PrUnavailable::Failed, Some(&failed))
                .ends_with("rate limit exceeded for user ID 1.")
        );
    }

    #[test]
    fn listed_pull_requests_are_read() {
        let json = br#"[
          {"number": 12, "title": "Add login", "url": "https://github.com/o/r/pull/12",
           "isDraft": false, "headRefName": "feature/login", "baseRefName": "main",
           "headRepositoryOwner": {"id": "X", "login": "o"}, "isCrossRepository": false},
          {"number": 13, "title": "Draft", "url": "https://github.com/o/r/pull/13",
           "isDraft": true, "headRefName": "wip", "baseRefName": "main",
           "headRepositoryOwner": {"login": "ada", "name": "Ada"}, "isCrossRepository": true}
        ]"#;
        let listed = parse_list(json).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].pr.state, PullRequestState::Open);
        assert_eq!(listed[1].pr.state, PullRequestState::Draft);
        assert_eq!(listed[1].head_owner.as_deref(), Some("ada"));
        assert!(listed[1].cross);
        assert!(parse_list(b"not json").is_err());
        assert_eq!(
            parse_default_branch(br#"{"defaultBranchRef":{"name":"trunk"}}"#).as_deref(),
            Some("trunk")
        );
        assert_eq!(parse_default_branch(br#"{"defaultBranchRef":null}"#), None);
    }

    #[test]
    fn the_created_url_is_the_last_url_on_the_host() {
        let printed = "Warning: 1 uncommitted change\nhttps://github.com/o/r/pull/7\n";
        assert_eq!(
            created_url(printed, "github.com").as_deref(),
            Some("https://github.com/o/r/pull/7")
        );
        assert_eq!(created_url("https://evil.example/x\n", "github.com"), None);
        assert_eq!(
            created_url("https://github.com.evil.example/x\n", "github.com"),
            None
        );
        assert_eq!(
            created_url("https://ghe.example:8443/t/a/pull/2\n", "ghe.example").as_deref(),
            Some("https://ghe.example:8443/t/a/pull/2")
        );
        assert_eq!(
            created_url("https://ghe.example:x/t\n", "ghe.example"),
            None
        );
        assert_eq!(number_of("https://github.com/o/r/pull/7"), Some(7));
        let exists = "a pull request for branch \"x\" into branch \"main\" already exists:\nhttps://github.com/o/r/pull/3\n";
        assert_eq!(
            already_exists(exists, "github.com").as_deref(),
            Some("https://github.com/o/r/pull/3")
        );
        assert_eq!(
            already_exists("https://github.com/o/r/pull/3", "github.com"),
            None
        );
    }

    #[test]
    fn ssh_tells_the_real_host_name() {
        let out = "user git\nhostname github.com\nport 22\nidentityfile ~/.ssh/work\n";
        assert_eq!(ssh_hostname(out).as_deref(), Some("github.com"));
        assert_eq!(
            ssh_hostname("hostname GH.Example.COM\n").as_deref(),
            Some("gh.example.com")
        );
        assert_eq!(ssh_hostname("port 22\n"), None);
        assert_eq!(ssh_hostname("hostname -oProxyCommand=x\n"), None);
        assert_eq!(ssh_hostname("hostname a b\n"), None);
        assert_eq!(ssh_hostname("hostname github.com;id\n"), None);
        assert_eq!(ssh_hostname("hostname $(id)\n"), None);
    }

    #[test]
    fn branch_names_read_as_titles() {
        assert_eq!(humanise("feature/fix-pr_loading"), "Fix pr loading");
        assert_eq!(humanise("readme"), "Readme");
        assert_eq!(humanise("ümlaut-fix"), "Ümlaut fix");
    }

    fn facts(remotes: &[(&str, &str)], config: &[(&str, &str)]) -> RemoteFacts {
        let mut f = RemoteFacts::default();
        for (name, url) in remotes {
            f.urls.insert((*name).into(), vec![(*url).into()]);
        }
        for (key, value) in config {
            match *key {
                "remote.pushdefault" => f.push_default = Some((*value).into()),
                k if k.ends_with(".pushremote") => {
                    let b = k
                        .strip_prefix("branch.")
                        .unwrap()
                        .strip_suffix(".pushremote");
                    f.branch_push_remote
                        .insert(b.unwrap().into(), (*value).into());
                }
                k if k.ends_with(".remote") => {
                    let b = k.strip_prefix("branch.").unwrap().strip_suffix(".remote");
                    f.branch_remote.insert(b.unwrap().into(), (*value).into());
                }
                k => f.resolved = Some((k.into(), (*value).into())),
            }
        }
        f
    }

    /// A branch is pushed to its pushRemote, else remote.pushDefault, else its upstream's
    /// remote, else origin, else the only remote.
    #[test]
    fn the_push_remote_follows_gits_order() {
        let two = [
            ("origin", "git@github.com:o/r"),
            ("fork", "git@github.com:me/r"),
        ];
        assert_eq!(facts(&two, &[]).push_remote("x"), Some("origin"));
        let f = facts(&two, &[("branch.x.remote", "fork")]);
        assert_eq!(f.push_remote("x"), Some("fork"));
        let f = facts(
            &two,
            &[
                ("branch.x.remote", "origin"),
                ("remote.pushdefault", "fork"),
            ],
        );
        assert_eq!(f.push_remote("x"), Some("fork"));
        let f = facts(
            &two,
            &[
                ("remote.pushdefault", "fork"),
                ("branch.x.pushremote", "origin"),
            ],
        );
        assert_eq!(f.push_remote("x"), Some("origin"));
        assert_eq!(
            facts(&[("mine", "git@github.com:me/r")], &[]).push_remote("x"),
            Some("mine")
        );
        let none = facts(&[("a", "u"), ("b", "u")], &[("branch.x.remote", ".")]);
        assert_eq!(none.push_remote("x"), None);
        // gh's repository: set-default's, else upstream, github, origin.
        let three = [
            ("origin", "git@github.com:me/r"),
            ("upstream", "git@github.com:o/r"),
            ("zz", "git@github.com:z/r"),
        ];
        assert_eq!(facts(&three, &[]).base_remote(), Some("upstream"));
        assert_eq!(facts(&three, &[("zz", "base")]).base_remote(), Some("zz"));
        // Task 9 review minor 3: an OWNER/REPO value names the repository, whichever remote
        // carries the mark; one no remote names leaves the marked remote.
        assert_eq!(
            facts(&three, &[("zz", "O/R")]).base_remote(),
            Some("upstream")
        );
        assert_eq!(
            facts(&three, &[("upstream", "me/r")]).base_remote(),
            Some("origin")
        );
        assert_eq!(
            facts(&three, &[("zz", "nobody/r")]).base_remote(),
            Some("zz")
        );
        // None of those names: the first, whatever branch it is for (the list and a new
        // pull request agree).
        let forked = [
            ("fork", "git@github.com:ada/r"),
            ("main-repo", "git@github.com:o/r"),
            ("zzz", "git@github.com:z/r"),
        ];
        let f = facts(&forked, &[("branch.x.remote", "main-repo")]);
        assert_eq!(f.base_remote(), Some("fork"));
    }

    fn listed(number: u32, head: &str, owner: &str, cross: bool) -> ListedPr {
        ListedPr {
            pr: PullRequest {
                number,
                title: format!("PR {number}"),
                url: format!("https://github.com/o/r/pull/{number}"),
                state: PullRequestState::Open,
                head: head.into(),
                base: "main".into(),
            },
            head_owner: Some(owner.into()),
            cross,
        }
    }

    fn reference(kind: RefKind, name: &str) -> RefInfo {
        RefInfo {
            kind,
            name: name.into(),
            hash: "a".repeat(40),
            upstream: None,
            ahead: 0,
            behind: 0,
        }
    }

    /// Forks: a pull request's head is matched to a branch only by the same name and the
    /// owner of the remote the branch is pushed to; a fork's same-named branch is not ours.
    #[test]
    fn pull_requests_match_branches_by_name_and_owner() {
        let data = PrData {
            listed: vec![
                listed(1, "feature", "o", false),
                listed(2, "topic", "ada", true),
                listed(3, "shared", "stranger", true),
            ],
            facts: facts(
                &[
                    ("origin", "https://github.com/o/r.git"),
                    ("ada", "git@github.com:ada/r.git"),
                ],
                &[("branch.topic.pushremote", "ada")],
            ),
        };
        assert_eq!(data.of_branch("feature").map(|p| p.number), Some(1));
        assert_eq!(data.of_branch("topic").map(|p| p.number), Some(2));
        assert_eq!(data.of_branch("shared"), None, "another owner's branch");
        assert_eq!(data.of_branch("nope"), None);
        let marks = data.marks(&[
            reference(RefKind::Branch, "feature"),
            reference(RefKind::Branch, "shared"),
            reference(RefKind::Remote, "origin/feature"),
            reference(RefKind::Remote, "ada/topic"),
            reference(RefKind::Remote, "origin/topic"),
            reference(RefKind::Tag, "feature"),
        ]);
        let keys: Vec<(RefKind, &str)> = marks.keys().map(|(k, n)| (*k, n.as_str())).collect();
        assert_eq!(
            keys,
            [
                (RefKind::Branch, "feature"),
                (RefKind::Remote, "ada/topic"),
                (RefKind::Remote, "origin/feature"),
            ]
        );
        // A remote whose owner is unknown (not a hosting URL): no pull request is marked,
        // not even one from the repository itself (its head may be anyone's same-named
        // branch).
        let local = PrData {
            listed: vec![listed(4, "x", "o", false), listed(5, "y", "ada", true)],
            facts: facts(&[("origin", "/srv/git/r.git")], &[]),
        };
        assert_eq!(local.of_branch("x"), None);
        assert_eq!(local.of_branch("y"), None);
        assert!(
            local
                .marks(&[
                    reference(RefKind::Branch, "x"),
                    reference(RefKind::Remote, "origin/x")
                ])
                .is_empty()
        );
    }
}
