//! Operations: changes to a repository the user asked for (`run_op`).
//!
//! A request is checked first (names, what is implemented), without git. Then it waits for
//! its turn in the repository's queue: one operation at a time per repository, in the order
//! they were asked for. With its turn it is published as `snapshot.op`, checked for dangers
//! (anything not confirmed means nothing runs), run in the active worktree, and ended with
//! one refresh that also clears `snapshot.op`. Every git command it runs is logged with the
//! repository's and the operation's id.

mod danger;
mod explain;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub(crate) use danger::dangers;
pub use explain::explain;

use crate::EventSink;
use crate::dto::{Danger, DangerKind, HeadInfo, OpHint, OpOutcome, OpRequest, OpStatus};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::names::check_branch_name;
use crate::git::runner::{CmdKind, GitOutput, GitRunner, TimeLimit};
use crate::repo::{Repo, RepoManager};

/// What an operation's body ended with; `run_op` turns it into the outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// It worked; a plain line saying what was done.
    Done(String),
    /// git stopped on conflicts (not a failure); a plain line saying so.
    Stopped(String),
    /// It failed: a plain line (an explanation where one is known), git's own output and
    /// follow-ups to offer.
    Failed {
        message: String,
        detail: String,
        hints: Vec<OpHint>,
    },
}

/// What an operation's body works with: the repository, a runner that logs as this
/// operation, the worktree to run in and the operation's id.
pub(crate) struct OpCtx<'a> {
    pub repo: &'a Arc<Repo>,
    pub runner: GitRunner,
    pub cwd: PathBuf,
    pub op_id: u64,
    sink: &'a dyn EventSink,
    net_limit: TimeLimit,
}

/// Fewest milliseconds between two `op-progress` events of one operation.
const PROGRESS_EVERY_MS: u128 = 100;

impl OpCtx<'_> {
    /// Runs a git command that changes the repository; its exit code is the caller's to
    /// check (see [`explain`]).
    pub async fn git(&self, args: &[&str]) -> Result<GitOutput> {
        self.runner
            .run_unchecked(&self.cwd, args, CmdKind::Write)
            .await
    }

    /// Runs a git command that only reads, as part of the operation.
    pub async fn read(&self, args: &[&str]) -> Result<GitOutput> {
        self.runner
            .run_unchecked(&self.cwd, args, CmdKind::Read)
            .await
    }

    /// Like [`OpCtx::git`], for a command that talks to a remote: it gives up after 120 s
    /// without output or 2 h in all (git and its helpers are killed; the output then has
    /// exit code -1 and ends with a line saying so), and the latest line of its progress is
    /// sent as `op-progress` events, at most 10 a second. Pass `--progress`.
    // Network operations (push, pull) use it from Task 3 on.
    #[allow(dead_code)]
    pub async fn git_net(&self, args: &[&str]) -> Result<GitOutput> {
        let progress = Progress::new(self.sink, self.repo.identity.repo_id, self.op_id);
        let run = self
            .runner
            .run_with_progress(&self.cwd, args, CmdKind::Write, self.net_limit, &|chunk| {
                progress.seen(chunk);
            })
            .await?;
        Ok(match run {
            Ok(out) => out,
            Err(gave_up) => {
                let mut stderr = gave_up.stderr;
                if !stderr.is_empty() && !stderr.ends_with('\n') {
                    stderr.push('\n');
                }
                stderr.push_str(&explain::timed_out(gave_up.limit));
                GitOutput {
                    stdout: gave_up.stdout,
                    stderr,
                    code: -1,
                }
            }
        })
    }
}

/// Sends the latest progress line of a network command as `op-progress`, at most every
/// [`PROGRESS_EVERY_MS`].
struct Progress<'a> {
    sink: &'a dyn EventSink,
    repo_id: u32,
    op_id: u64,
    state: Mutex<ProgressState>,
}

#[derive(Default)]
struct ProgressState {
    /// Text after the last line end seen.
    partial: String,
    last_sent: Option<Instant>,
}

/// Longest unfinished line kept; git's progress lines are far shorter.
const MAX_PARTIAL: usize = 4096;

impl<'a> Progress<'a> {
    fn new(sink: &'a dyn EventSink, repo_id: u32, op_id: u64) -> Self {
        Self {
            sink,
            repo_id,
            op_id,
            state: Mutex::new(ProgressState::default()),
        }
    }

    /// Takes the next chunk of stderr.
    fn seen(&self, chunk: &[u8]) {
        let line = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(line) = latest_line(&mut state.partial, chunk) else {
                return;
            };
            let due = state
                .last_sent
                .is_none_or(|at| at.elapsed().as_millis() >= PROGRESS_EVERY_MS);
            if !due {
                return;
            }
            state.last_sent = Some(Instant::now());
            line
        };
        self.sink.emit(
            "op-progress",
            serde_json::json!({ "repoId": self.repo_id, "opId": self.op_id, "line": line }),
        );
    }
}

/// Adds `chunk` to the unfinished line `partial`; returns the last non-empty line the chunk
/// finished (git redraws progress lines ending them with `\r`), if any.
fn latest_line(partial: &mut String, chunk: &[u8]) -> Option<String> {
    partial.push_str(&String::from_utf8_lossy(chunk));
    let end = partial.rfind(['\r', '\n'])?;
    let latest = partial[..end]
        .split(['\r', '\n'])
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(str::to_owned);
    partial.drain(..=end);
    if partial.len() > MAX_PARTIAL {
        partial.clear();
    }
    latest
}

/// The future an operation's body returns.
pub(crate) type BodyFuture<'c> = Pin<Box<dyn Future<Output = Result<Step>> + Send + 'c>>;

/// An operation's body: what it runs in its turn, once its dangers (given) are confirmed.
pub(crate) type Body =
    Box<dyn for<'c> FnOnce(&'c OpCtx<'c>, &'c [Danger]) -> BodyFuture<'c> + Send>;

/// Boxes `f` as a [`Body`] (giving the closure its signature).
fn body<F>(f: F) -> Body
where
    F: for<'c> FnOnce(&'c OpCtx<'c>, &'c [Danger]) -> BodyFuture<'c> + Send + 'static,
{
    Box::new(f)
}

/// A request that can run: its title and its body.
struct Prepared {
    title: String,
    body: Body,
}

/// Checks `request` (names, arguments, whether it is built yet) without git, and returns
/// its title and body; `head` is the repository's HEAD as last read, for the title.
/// `bad_name` for a bad name, `bad_request` for a bad argument or an operation not built
/// yet. Every operation is matched here, once.
fn prepare(request: &OpRequest, head: &HeadInfo) -> Result<Prepared> {
    let _ = head;
    match request {
        OpRequest::CreateBranch {
            name,
            at,
            checkout: false,
        } => {
            check_branch_name(name)?;
            check_commit(at)?;
            let (name, at) = (name.clone(), at.clone());
            Ok(Prepared {
                title: format!("Create branch {name}"),
                body: body(move |ctx, _| {
                    Box::pin(async move { create_branch(ctx, &name, &at).await })
                }),
            })
        }
        _ => Err(CoreError::new(
            ErrorCode::BadRequest,
            "This operation is not implemented yet.",
        )),
    }
}

/// `Ok` if `at` can be given to git as a commit: not empty and not option-like.
fn check_commit(at: &str) -> Result<()> {
    if at.is_empty() || at.starts_with('-') {
        return Err(CoreError::new(
            ErrorCode::BadRequest,
            format!("\u{201c}{at}\u{201d} is not a commit."),
        ));
    }
    Ok(())
}

/// What a turn in the queue ended with.
enum Ran {
    Step(Step),
    /// Nothing ran: these dangers were not all confirmed.
    Unconfirmed(Vec<Danger>),
}

/// Said when an operation's body panicked.
const STOPPED_UNEXPECTEDLY: &str = "The operation stopped unexpectedly.";

impl RepoManager {
    /// Runs `request` on repository `repo_id` once the operations asked for before it have
    /// ended, and returns how it went, with the repository as it is afterwards.
    /// `confirmed` lists the dangers the user has agreed to; if the request carries any
    /// other, nothing runs and the outcome is `needs_confirmation` with every danger it
    /// carries. A bad name is a `bad_name` error, before anything is queued or run.
    ///
    /// The operation runs on its own task: a caller that goes away does not stop it
    /// halfway, nor leave the repository marked busy.
    pub async fn run_op(
        self: &Arc<Self>,
        repo_id: u32,
        request: OpRequest,
        confirmed: &[DangerKind],
    ) -> Result<OpOutcome> {
        let head = self.snapshot(repo_id).await?.head;
        let Prepared { title, body } = prepare(&request, &head)?;
        let repo = self.get(repo_id)?;
        let op_id = self.next_op_id();
        let (m, confirmed) = (self.clone(), confirmed.to_vec());
        let task = tokio::spawn(async move {
            m.run_in_turn(repo, op_id, title, request, confirmed, body)
                .await
        });
        // Only the body can panic, and that is handled within the turn.
        task.await.unwrap_or_else(|e| {
            Err(CoreError::new(ErrorCode::Io, STOPPED_UNEXPECTEDLY).with_detail(e.to_string()))
        })
    }

    /// Waits for the turn, then runs the operation and ends it (see [`RepoManager::run_op`]).
    /// The body runs as a task of its own, so that if it panics the operation still ends
    /// here, within the turn: the repository is refreshed and no longer marked busy before
    /// the next operation starts.
    async fn run_in_turn(
        self: &Arc<Self>,
        repo: Arc<Repo>,
        op_id: u64,
        title: String,
        request: OpRequest,
        confirmed: Vec<DangerKind>,
        body: Body,
    ) -> Result<OpOutcome> {
        repo.op_waiting
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.op_queue_changed(&repo).await;
        let _turn = repo.op_queue.lock().await;
        repo.op_waiting
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        self.start_op(&repo, op_id, &title).await?;

        let inner = tokio::spawn({
            let (m, repo) = (self.clone(), repo.clone());
            async move { m.run_body(&repo, op_id, &request, &confirmed, body).await }
        });
        let ran = inner.await.unwrap_or_else(|e| {
            Err(CoreError::new(ErrorCode::Io, STOPPED_UNEXPECTEDLY).with_detail(e.to_string()))
        });
        let snapshot = self.finish_op(&repo, op_id).await;
        let ran = ran?;
        let snapshot = snapshot?;

        let (log_from, log_to) = repo.runner.log().op_writes(op_id).unwrap_or((0, 0));
        let mut outcome = OpOutcome {
            op_id,
            status: OpStatus::Done,
            title,
            message: String::new(),
            detail: None,
            log_from,
            log_to,
            dangers: Vec::new(),
            hints: Vec::new(),
            snapshot,
        };
        match ran {
            Ran::Step(Step::Done(message)) => outcome.message = message,
            Ran::Step(Step::Stopped(message)) => {
                outcome.status = OpStatus::Stopped;
                outcome.message = message;
            }
            Ran::Step(Step::Failed {
                message,
                detail,
                hints,
            }) => {
                outcome.status = OpStatus::Failed;
                outcome.message = message;
                outcome.detail = Some(detail);
                outcome.hints = hints;
            }
            Ran::Unconfirmed(dangers) => {
                outcome.status = OpStatus::NeedsConfirmation;
                outcome.message = dangers
                    .iter()
                    .map(|d| d.message.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                outcome.dangers = dangers;
            }
        }
        Ok(outcome)
    }

    /// Checks `request` for dangers and, if every one is confirmed, runs its `body` in the
    /// active worktree.
    async fn run_body(
        &self,
        repo: &Arc<Repo>,
        op_id: u64,
        request: &OpRequest,
        confirmed: &[DangerKind],
        body: Body,
    ) -> Result<Ran> {
        let ctx = OpCtx {
            repo,
            runner: repo.runner.for_op(op_id),
            cwd: repo.worktree.clone(),
            op_id,
            sink: self.sink.as_ref(),
            net_limit: self.net_limit(),
        };
        let dangers = dangers(&ctx, request).await?;
        if !all_confirmed(&dangers, confirmed) {
            return Ok(Ran::Unconfirmed(dangers));
        }
        let step = body(&ctx, &dangers).await;
        let delay = self.op_delay();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        #[cfg(any(test, feature = "testutil"))]
        if self.take_op_panic() {
            panic!("test: the operation's body panicked");
        }
        step.map(Ran::Step)
    }
}

/// Whether every kind in `dangers` is in `confirmed`.
fn all_confirmed(dangers: &[Danger], confirmed: &[DangerKind]) -> bool {
    dangers.iter().all(|d| confirmed.contains(&d.kind))
}

/// `branch -- <name> <at>`: a new branch at `at`, not checked out. `bad_name` if a branch
/// of that name exists (git is not asked to make it).
async fn create_branch(ctx: &OpCtx<'_>, name: &str, at: &str) -> Result<Step> {
    let full = format!("refs/heads/{name}");
    let exists = ctx
        .read(&["rev-parse", "--verify", "--quiet", &full])
        .await?;
    if exists.code == 0 {
        return Err(CoreError::new(
            ErrorCode::BadName,
            format!("A branch named \u{201c}{name}\u{201d} already exists."),
        ));
    }
    let args = ["branch", "--", name, at];
    let out = ctx.git(&args).await?;
    if out.code != 0 {
        return Ok(explain(&args, &out));
    }
    Ok(Step::Done(format!("Created branch {name}.")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::RecordingSink;

    #[test]
    fn latest_line_follows_redrawn_progress() {
        let mut partial = String::new();
        assert_eq!(latest_line(&mut partial, b"Counting: 1"), None);
        assert_eq!(
            latest_line(&mut partial, b"0%\rCounting: 50%\rCount"),
            Some("Counting: 50%".to_owned())
        );
        assert_eq!(partial, "Count");
        assert_eq!(
            latest_line(&mut partial, b"ing: 100%, done.\n\n  \r"),
            Some("Counting: 100%, done.".to_owned())
        );
        assert_eq!(partial, "");
        assert_eq!(latest_line(&mut partial, b"\r\n"), None);
    }

    #[test]
    fn progress_is_sent_at_most_ten_times_a_second() {
        let sink = RecordingSink::default();
        let progress = Progress::new(&sink, 4, 9);
        for i in 0..50 {
            progress.seen(format!("Writing objects: {i}%\r").as_bytes());
        }
        let events = sink.events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].0, "op-progress");
        assert_eq!(
            events[0].1,
            serde_json::json!({"repoId": 4, "opId": 9, "line": "Writing objects: 0%"})
        );
    }

    #[test]
    fn confirmation_covers_every_danger() {
        let danger = |kind| Danger {
            kind,
            message: String::new(),
            detail: None,
        };
        let both = [danger(DangerKind::HardReset), danger(DangerKind::DropStash)];
        assert!(all_confirmed(&[], &[]));
        assert!(!all_confirmed(&both, &[DangerKind::HardReset]));
        assert!(all_confirmed(
            &both,
            &[
                DangerKind::DropStash,
                DangerKind::HardReset,
                DangerKind::ForcePush
            ]
        ));
    }

    #[test]
    fn unbuilt_operations_and_bad_input_are_refused_without_git() {
        let head = HeadInfo {
            branch: Some("main".into()),
            hash: None,
        };
        let refused = |r: &OpRequest| prepare(r, &head).err().expect("refused").code;
        assert_eq!(refused(&OpRequest::Continue), ErrorCode::BadRequest);
        let option_like = OpRequest::CreateBranch {
            name: "x".into(),
            at: "--all".into(),
            checkout: false,
        };
        assert_eq!(refused(&option_like), ErrorCode::BadRequest);
        let ok = OpRequest::CreateBranch {
            name: "x".into(),
            at: "abc".into(),
            checkout: false,
        };
        assert_eq!(
            prepare(&ok, &head).map(|p| p.title).expect("ok"),
            "Create branch x"
        );
    }
}
