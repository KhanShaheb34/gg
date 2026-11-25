//! `pr_details`: one pull request's merge status, checks and comments, read through `gh`
//! (see [`crate::pr_details`]) when the side panel's Pull request tab shows it, never in the
//! background. Two `gh` commands, at once, under the `gh` time limit: `gh pr view <number>
//! --repo <repo> --json …` and `gh api graphql` with the review threads' query (owner, name
//! and number passed as its variables; the host as every `gh` command gets it).
//!
//! At most one load per repository and pull request every 10 s: asked again sooner, the
//! answer of the last load is given again (its `loadedAtMs` says when it was read), or, when
//! that load failed, a new one starts once the 10 s are up. Asked while a load runs, the
//! request waits for that load's answer rather than starting another.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::OnceCell;
use tokio::time::Instant;

use crate::dto::{PrDetails, PrUnavailable};
use crate::error::{CoreError, ErrorCode, Result};
use crate::gh::{GhFailure, unavailable_message};
use crate::git::runner::{CmdKind, StdinPolicy};
use crate::pr_details::{THREADS_QUERY, VIEW_FIELDS, assemble, parse_threads, parse_view};
use crate::repo::fetch::unix_ms;
use crate::repo::manager::{Repo, RepoManager, lock};

/// Least time between two loads of one pull request's details.
const LOAD_EVERY: Duration = Duration::from_secs(10);

/// One pull request's last (or running) load: when it started, and its answer once given.
#[derive(Debug, Clone)]
pub(crate) struct DetailsLoad {
    started: Instant,
    answer: Arc<OnceCell<Result<PrDetails>>>,
}

/// What a request does with the load it finds.
enum Plan {
    /// Wait for this load's answer (it runs, or it is recent), or run it (it is new).
    Take(Arc<OnceCell<Result<PrDetails>>>),
    /// The last load failed less than 10 s ago: wait this long, then look again.
    Wait(Duration),
}

/// Said when the details cannot be asked for: why, as for the list, or gh's own reason.
fn unloaded(reason: PrUnavailable, failure: Option<&GhFailure>) -> CoreError {
    let message = match (reason, failure) {
        (PrUnavailable::Failed, Some(GhFailure::Failed { message, .. })) => {
            format!("Could not load the checks and comments: {message}")
        }
        (PrUnavailable::Failed, _) => "Could not load the checks and comments.".to_owned(),
        _ => unavailable_message(reason, failure),
    };
    let mut error = CoreError::new(ErrorCode::Io, message);
    error.detail = failure.map(GhFailure::detail).filter(|d| !d.is_empty());
    error
}

impl RepoManager {
    /// Pull request `number` of repository `repo_id`: its state, merge status, review
    /// decision, checks and comments (see the module comment for how often `gh` is asked).
    /// `bad_request` for number 0; `io` when `gh` cannot be asked or its answer read (the
    /// message says why). The review threads failing alone is not an error: the rest comes
    /// with `threadsError` saying so.
    pub async fn pr_details(&self, repo_id: u32, number: u32) -> Result<PrDetails> {
        if number == 0 {
            return Err(CoreError::new(
                ErrorCode::BadRequest,
                "No pull request is number 0.",
            ));
        }
        let repo = self.get(repo_id)?;
        let every = self.pr_details_every();
        loop {
            let plan = {
                let mut loads = lock(&repo.pr_details);
                let last = loads.get(&number);
                match last.map(|l| (l, l.answer.get())) {
                    // Running (or never finished): its answer is this one's too.
                    Some((l, None)) => Plan::Take(l.answer.clone()),
                    Some((l, Some(Ok(_)))) if l.started.elapsed() < every => {
                        Plan::Take(l.answer.clone())
                    }
                    Some((l, Some(Err(_)))) if l.started.elapsed() < every => {
                        Plan::Wait(every.saturating_sub(l.started.elapsed()))
                    }
                    _ => {
                        let answer = Arc::new(OnceCell::new());
                        loads.insert(
                            number,
                            DetailsLoad {
                                started: Instant::now(),
                                answer: answer.clone(),
                            },
                        );
                        Plan::Take(answer)
                    }
                }
            };
            match plan {
                Plan::Take(answer) => {
                    let loaded = answer
                        .get_or_init(|| Box::pin(self.load_details(&repo, number)))
                        .await;
                    return loaded.clone();
                }
                Plan::Wait(wait) => tokio::time::sleep(wait).await,
            }
        }
    }

    /// Reads pull request `number`'s details from `gh` (see the module comment).
    async fn load_details(&self, repo: &Arc<Repo>, number: u32) -> Result<PrDetails> {
        #[cfg(any(test, feature = "testutil"))]
        self.details_loads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (gh, _facts) = self
            .gh_for_list(repo)
            .await
            .map_err(|(reason, failure, _)| unloaded(reason, failure.as_ref()))?;
        let n = number.to_string();
        let view_args = ["pr", "view", &n, "--repo", gh.repo(), "--json", VIEW_FIELDS];
        let (owner, name) = gh.owner_and_name();
        let (query, owner, name, typed_number) = (
            format!("query={THREADS_QUERY}"),
            format!("o={owner}"),
            format!("r={name}"),
            format!("n={number}"),
        );
        // `-f`: a string taken as it is (never a file's contents); `-F` only for the number.
        let graphql_args = [
            "api",
            "graphql",
            "-f",
            &query,
            "-f",
            &owner,
            "-f",
            &name,
            "-F",
            &typed_number,
        ];
        let (view, threads) = tokio::join!(
            gh.run(&view_args, CmdKind::Read, StdinPolicy::Null),
            gh.run(&graphql_args, CmdKind::Read, StdinPolicy::Null),
        );
        let view = match view? {
            Ok(out) => parse_view(&out.stdout).map_err(|why| {
                CoreError::new(
                    ErrorCode::Io,
                    "Could not load the checks and comments: gh's answer could not be read.",
                )
                .with_detail(why)
            })?,
            Err(failure) => return Err(unloaded(failure.reason(), Some(&failure))),
        };
        let threads = match threads {
            Ok(Ok(out)) => parse_threads(&out.stdout),
            Ok(Err(GhFailure::Failed { message, .. })) => Err(message),
            Ok(Err(failure)) => Err(unavailable_message(failure.reason(), Some(&failure))),
            Err(e) => Err(e.message),
        };
        Ok(assemble(view, threads, unix_ms()))
    }

    /// Least time between two loads of a pull request's details (see
    /// [`RepoManager::set_pr_details_every`]).
    fn pr_details_every(&self) -> Duration {
        #[cfg(any(test, feature = "testutil", feature = "tool-path"))]
        if let Some(every) = *lock(&self.pr_details_every) {
            return every;
        }
        LOAD_EVERY
    }

    /// Sets the least time between two loads of a pull request's details (tests and the
    /// devserver only; normally 10 s).
    #[cfg(any(test, feature = "testutil", feature = "tool-path"))]
    pub fn set_pr_details_every(&self, every: Duration) {
        *lock(&self.pr_details_every) = Some(every);
    }

    /// How many loads of pull request details have started (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn pr_details_loads(&self) -> u64 {
        self.details_loads
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}
