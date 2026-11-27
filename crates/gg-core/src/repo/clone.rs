//! The Clone dialog's reads (the pure parts are [`crate::clone`]; the clone itself is
//! `RepoManager::clone_repo`):
//!
//! - `clone_repos`: the user's repositories on github.com, one `gh api graphql` in a folder of
//!   no repository; `gh` missing, signed out, rate limited or offline said as the Pull requests
//!   page says it.
//! - `clone_known`: the repositories gg already has, by the repositories their remotes name:
//!   the open ones (as their remotes were read) and the recent ones (each one's remote URLs
//!   read with `git config`), so a repository there is switched to or opened, not cloned again.

use std::path::PathBuf;

use serde::Serialize;

use super::inbox::{gh_name, neutral_dir};
use crate::clone::{REPOS_CAP, REPOS_QUERY, RepoList, parse_repos};
use crate::dto::InboxProblem;
use crate::gh::{Gh, GhFailure};
use crate::git::runner::{CmdKind, StdinPolicy};
use crate::hosts::{RemoteRepo, parse_remote};
use crate::inbox::problem_of;
use crate::repo::manager::{RepoManager, lock};

/// The host the user's repositories are listed from.
const LIST_HOST: &str = "github.com";

/// A repository gg already has, by one of its remotes. Serialized camelCase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownRepo {
    /// Its folder (the main worktree's).
    pub path: String,
    /// Its tab's repository, when it is open.
    pub repo_id: Option<u32>,
    pub host: String,
    pub owner: String,
    pub name: String,
}

impl RepoManager {
    /// The user's repositories on github.com, the most recently pushed to first, at most
    /// [`REPOS_CAP`]; or why not (see the module comment).
    pub async fn clone_repos(&self) -> RepoList {
        let host = LIST_HOST;
        let failed = |failure: &GhFailure| {
            let (problem, message) = problem_of(failure, host);
            let message = match problem {
                InboxProblem::Failed => format!("Could not list them: {}", failure_line(failure)),
                _ => message,
            };
            RepoList::failed(host, problem, message, Some(failure.detail()))
        };
        let Some((program, tool_path)) = self.tool_program_at(None, gh_name()) else {
            return failed(&GhFailure::NoGh);
        };
        let on = RemoteRepo {
            host: host.to_owned(),
            web: host.to_owned(),
            owner: String::new(),
            name: String::new(),
        };
        let cwd = neutral_dir();
        let Some(gh) = Gh::new(&self.runner, program, tool_path, &cwd, &on, self.gh_limit()) else {
            return failed(&GhFailure::NoGh);
        };
        let (query, first) = (format!("query={REPOS_QUERY}"), format!("first={REPOS_CAP}"));
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
            // GitHub's own error (a rate limit, say) is told apart as gh's words are.
            Ok(Ok(out)) => parse_repos(&out.stdout, host).unwrap_or_else(|why| {
                failed(&GhFailure::Failed {
                    message: why.clone(),
                    detail: why,
                })
            }),
            Ok(Err(failure)) => failed(&failure),
            Err(e) => RepoList::failed(
                host,
                InboxProblem::Failed,
                format!("Could not list them: {}", e.message),
                e.detail,
            ),
        }
    }

    /// The repositories gg already has (see the module comment): each open one's remotes, then
    /// each of `recents` not open (its folder still there), by the repository each remote
    /// names.
    pub async fn clone_known(&self, recents: Vec<String>) -> Vec<KnownRepo> {
        let mut known = Vec::new();
        let mut open = Vec::new();
        let mut repos: Vec<_> = lock(&self.repos).values().cloned().collect();
        repos.sort_by_key(|r| r.identity.repo_id);
        for repo in repos {
            let path = repo.root.to_string_lossy().into_owned();
            open.push(repo.root.clone());
            let Ok(facts) = self.remote_facts(&repo, &repo.runner, &repo.root).await else {
                continue;
            };
            let remotes: Vec<String> = facts.remote_names().map(str::to_owned).collect();
            for remote in remotes {
                if let Some(on) = facts.repo_of(&remote) {
                    known.push(KnownRepo {
                        path: path.clone(),
                        repo_id: Some(repo.identity.repo_id),
                        host: on.host,
                        owner: on.owner,
                        name: on.name,
                    });
                }
            }
        }
        let mut reads = tokio::task::JoinSet::new();
        for (rank, path) in recents.into_iter().enumerate() {
            let dir = PathBuf::from(&path);
            if open.contains(&dir) || !dir.is_dir() {
                continue;
            }
            let runner = self.runner.clone();
            reads.spawn(async move {
                let out = runner
                    .run_unchecked(
                        &dir,
                        &["config", "--get-regexp", r"^remote\..*\.url$"],
                        CmdKind::Read,
                    )
                    .await;
                let urls: Vec<String> = match out {
                    Ok(out) if out.code == 0 => out
                        .text()
                        .lines()
                        .filter_map(|l| l.split_once(' ').map(|(_, url)| url.trim().to_owned()))
                        .collect(),
                    _ => Vec::new(),
                };
                (rank, path, urls)
            });
        }
        let mut recent = Vec::new();
        while let Some(done) = reads.join_next().await {
            if let Ok(done) = done {
                recent.push(done);
            }
        }
        recent.sort_by_key(|(rank, _, _)| *rank);
        for (_, path, urls) in recent {
            for on in urls.iter().filter_map(|u| parse_remote(u)) {
                known.push(KnownRepo {
                    path: path.clone(),
                    repo_id: None,
                    host: on.host,
                    owner: on.owner,
                    name: on.name,
                });
            }
        }
        known
    }
}

/// The plain line of a failure of `gh`.
fn failure_line(failure: &GhFailure) -> String {
    match failure {
        GhFailure::Failed { message, .. } => message.clone(),
        _ => failure.detail(),
    }
}
