//! Operations: changes to a repository the user asked for (`run_op`).
//!
//! A request is checked first (names, what is implemented), without git. Then it waits for
//! its turn in the repository's queue: one operation at a time per repository, in the order
//! they were asked for. With its turn it is checked for dangers: if any is not confirmed,
//! nothing runs and nothing is published. Otherwise it is published as `snapshot.op`, run in
//! the active worktree, and ended with one refresh that also clears `snapshot.op`. Every git
//! command it runs is logged with the repository's and the operation's id.

mod commit;
mod danger;
mod explain;
mod local;
mod network;
pub(crate) mod order;
mod progress;
mod stash;
mod worktree;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Instant;

pub(crate) use danger::dangers;
pub use explain::explain;

use crate::EventSink;
use crate::dto::{
    Confirmed, Danger, DangerKind, HeadInfo, InProgressKind, MergeTargetKind, OpHint, OpOutcome,
    OpRequest, OpStatus, ResetMode, WorktreeBranch,
};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::names::{check_branch_name, check_tag_name};
use crate::git::progress::read_kind;
use crate::git::runner::{CmdKind, GitOutput, GitRunner, STOPPED, Stop, TimeLimit};
use crate::repo::{Repo, RepoManager, unknown_repo};
use order::NumberedTurn;

/// What an operation's body ended with; `run_op` turns it into the outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// It worked; a plain line saying what was done.
    Done(String),
    /// It worked, with git's output worth showing (a note about it) or follow-ups worth
    /// offering (push after a merge).
    DoneWith {
        message: String,
        detail: Option<String>,
        hints: Vec<OpHint>,
    },
    /// git stopped on conflicts (not a failure): a plain line saying so, and git's output.
    Stopped { message: String, detail: String },
    /// It failed: a plain line (an explanation where one is known), git's own output and
    /// follow-ups to offer.
    Failed {
        message: String,
        detail: String,
        hints: Vec<OpHint>,
    },
}

/// What an operation's body works with: the repository, a runner that logs as this
/// operation, the worktree to run in (the active one; the main one for the worktree
/// operations) and the operation's id.
pub(crate) struct OpCtx<'a> {
    pub repo: &'a Arc<Repo>,
    /// The manager, for what an operation does to the repository's state beside git
    /// (making the main worktree active before the active one is removed).
    pub manager: &'a Arc<RepoManager>,
    pub runner: GitRunner,
    pub cwd: PathBuf,
    pub op_id: u64,
    sink: &'a dyn EventSink,
    net_limit: TimeLimit,
    /// The outcome the body gives stands when the user stops it: it says what was done that
    /// cannot be taken back (a commit made before the Stop).
    kept: std::sync::atomic::AtomicBool,
    /// A read the operation's turn made that its later steps in the turn reuse (the worktree
    /// removal's status with ignored files: the folder read, and git's output).
    pub(crate) held_read: std::sync::Mutex<Option<(PathBuf, Vec<u8>)>>,
    /// The worktree the operation made (its canonical path, as the snapshot lists it).
    pub(crate) created: Arc<std::sync::Mutex<Option<String>>>,
    /// The commit the operation made (its full hash), once made.
    pub(crate) committed: Arc<std::sync::Mutex<Option<String>>>,
}

/// Fewest milliseconds between two `op-progress` events of one operation.
const PROGRESS_EVERY_MS: u128 = 100;

impl OpCtx<'_> {
    /// From now on, a Stop does not replace the body's outcome with "Stopped.": the body
    /// says itself what was done before it (see [`OpCtx::kept`]).
    pub fn keep_outcome_when_stopped(&self) {
        self.kept.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn kept(&self) -> bool {
        self.kept.load(std::sync::atomic::Ordering::SeqCst)
    }

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

/// What a title is made from, read in the operation's turn.
struct TurnFacts<'a> {
    /// The branch HEAD is on ("detached HEAD" without one).
    head: &'a str,
    /// What git is stopped in, if anything.
    in_progress: Option<InProgressKind>,
}

/// An operation's title, made in its turn: some titles name the branch HEAD is on then
/// ("Merge x into main") or what git is stopped in ("Continue the merge").
type Title = Box<dyn FnOnce(&TurnFacts) -> String + Send>;

/// A title that does not depend on HEAD.
fn fixed(title: String) -> Title {
    Box::new(move |_| title)
}

/// A title made from the branch HEAD is on.
fn on_head(f: impl FnOnce(&str) -> String + Send + 'static) -> Title {
    Box::new(move |facts| f(facts.head))
}

/// A title made from what git is stopped in.
fn on_progress(f: impl FnOnce(Option<InProgressKind>) -> String + Send + 'static) -> Title {
    Box::new(move |facts| f(facts.in_progress))
}

/// A request that can run: its title and its body.
struct Prepared {
    title: Title,
    body: Body,
}

/// Checks `request` (names, arguments) without git, and returns its title and body.
/// `bad_name` for a bad name, `bad_request` for a bad argument. Every operation is matched
/// here, once.
fn prepare(request: &OpRequest) -> Result<Prepared> {
    let (title, body) = match request.clone() {
        OpRequest::CheckoutBranch {
            branch,
            fast_forward,
            ..
        } => {
            check_arg(&branch, "branch")?;
            (
                fixed(format!("Check out {branch}")),
                body(move |ctx, _| {
                    Box::pin(
                        async move { local::checkout_branch(ctx, &branch, fast_forward).await },
                    )
                }),
            )
        }
        OpRequest::CheckoutRemote {
            remote_branch,
            local_name,
            ..
        } => {
            check_arg(&remote_branch, "remote branch")?;
            check_branch_name(&local_name)?;
            (
                fixed(format!("Check out {remote_branch} as {local_name}")),
                body(move |ctx, _| {
                    Box::pin(async move {
                        local::checkout_remote(ctx, &remote_branch, &local_name).await
                    })
                }),
            )
        }
        OpRequest::CheckoutCommit { hash, .. } => {
            check_full_hash(&hash)?;
            (
                fixed(format!("Check out {}", local::short(&hash))),
                body(move |ctx, _| {
                    Box::pin(async move { local::checkout_commit(ctx, &hash).await })
                }),
            )
        }
        OpRequest::CreateBranch {
            name, at, checkout, ..
        } => {
            check_branch_name(&name)?;
            check_full_hash(&at)?;
            let title = if checkout {
                format!("Create and check out branch {name}")
            } else {
                format!("Create branch {name}")
            };
            (
                fixed(title),
                body(move |ctx, _| {
                    Box::pin(async move { local::create_branch(ctx, &name, &at, checkout).await })
                }),
            )
        }
        OpRequest::RenameBranch {
            branch, new_name, ..
        } => {
            check_arg(&branch, "branch")?;
            check_branch_name(&new_name)?;
            (
                fixed(format!("Rename branch {branch} to {new_name}")),
                body(move |ctx, _| {
                    Box::pin(async move { local::rename_branch(ctx, &branch, &new_name).await })
                }),
            )
        }
        OpRequest::DeleteBranch { branch, .. } => {
            check_arg(&branch, "branch")?;
            (
                fixed(format!("Delete branch {branch}")),
                body(move |ctx, dangers| {
                    Box::pin(async move { local::delete_branch(ctx, dangers, &branch).await })
                }),
            )
        }
        OpRequest::Merge {
            target,
            no_ff,
            squash,
            no_commit,
            ..
        } => {
            match target.kind {
                MergeTargetKind::Commit => check_full_hash(&target.name)?,
                _ => check_arg(&target.name, "branch")?,
            }
            let shown = match target.kind {
                MergeTargetKind::Commit => local::short(&target.name).to_owned(),
                _ => target.name.clone(),
            };
            (
                on_head(move |head| format!("Merge {shown} into {head}")),
                body(move |ctx, _| {
                    Box::pin(
                        async move { local::merge(ctx, &target, no_ff, squash, no_commit).await },
                    )
                }),
            )
        }
        OpRequest::Rebase { onto, .. } => {
            check_arg(&onto, "branch")?;
            let shown = onto.clone();
            (
                on_head(move |head| format!("Rebase {head} onto {shown}")),
                body(move |ctx, _| Box::pin(async move { local::rebase(ctx, &onto).await })),
            )
        }
        OpRequest::CherryPick {
            hash,
            mainline,
            no_commit,
            ..
        } => {
            check_full_hash(&hash)?;
            check_mainline(mainline)?;
            (
                fixed(format!("Cherry-pick {}", local::short(&hash))),
                body(move |ctx, _| {
                    Box::pin(
                        async move { local::cherry_pick(ctx, &hash, mainline, no_commit).await },
                    )
                }),
            )
        }
        OpRequest::Revert { hash, mainline, .. } => {
            check_full_hash(&hash)?;
            check_mainline(mainline)?;
            (
                fixed(format!("Revert {}", local::short(&hash))),
                body(move |ctx, _| {
                    Box::pin(async move { local::revert(ctx, &hash, mainline).await })
                }),
            )
        }
        OpRequest::Reset { hash, mode, .. } => {
            check_full_hash(&hash)?;
            let how = match mode {
                ResetMode::Soft => "soft",
                ResetMode::Mixed => "mixed",
                ResetMode::Hard => "hard",
            };
            let to = local::short(&hash).to_owned();
            (
                on_head(move |head| format!("Reset {head} to {to} ({how})")),
                body(move |ctx, _| Box::pin(async move { local::reset(ctx, &hash, mode).await })),
            )
        }
        OpRequest::CreateTag {
            name,
            hash,
            message,
            ..
        } => {
            check_tag_name(&name)?;
            check_full_hash(&hash)?;
            if message.as_ref().is_some_and(|m| m.trim().is_empty()) {
                return Err(CoreError::new(
                    ErrorCode::BadRequest,
                    "An annotated tag needs a message.",
                ));
            }
            (
                fixed(format!("Create tag {name}")),
                body(move |ctx, _| {
                    Box::pin(async move {
                        local::create_tag(ctx, &name, &hash, message.as_deref()).await
                    })
                }),
            )
        }
        OpRequest::DeleteTag { name, remote: None } => {
            check_arg(&name, "tag")?;
            (
                fixed(format!("Delete tag {name}")),
                body(move |ctx, _| Box::pin(async move { local::delete_tag(ctx, &name).await })),
            )
        }
        OpRequest::DeleteTag {
            name,
            remote: Some(remote),
        } => {
            check_tag_name(&name)?;
            check_arg(&remote, "remote")?;
            (
                fixed(format!("Delete tag {name} here and on {remote}")),
                body(move |ctx, _| {
                    Box::pin(
                        async move { network::delete_tag_everywhere(ctx, &name, &remote).await },
                    )
                }),
            )
        }
        OpRequest::PushTag { name, remote } => {
            check_tag_name(&name)?;
            check_arg(&remote, "remote")?;
            (
                fixed(format!("Push tag {name} to {remote}")),
                body(move |ctx, _| {
                    Box::pin(async move { network::push_tag(ctx, &name, &remote).await })
                }),
            )
        }
        OpRequest::Push {
            branch,
            remote,
            to,
            force,
        } => {
            check_branch_name(&branch)?;
            check_arg(&remote, "remote")?;
            if let Some(to) = &to {
                check_branch_name(to)?;
            }
            let there = format!("{remote}/{}", to.as_deref().unwrap_or(&branch));
            let title = if force {
                format!("Force push {branch} to {there}")
            } else {
                format!("Push {branch} to {there}")
            };
            (
                fixed(title),
                body(move |ctx, dangers| {
                    Box::pin(async move {
                        // Confirmed with the token the user saw: the remote commit it leases on
                        // (R11) and the local tip it pushes (M1).
                        let token = token_of(dangers, DangerKind::ForcePush);
                        let (lease, tip) = token
                            .and_then(|t| t.split_once('\n'))
                            .map_or((None, None), |(l, t)| {
                                (Some(l), Some(t).filter(|t| !t.is_empty()))
                            });
                        network::push(ctx, &branch, &remote, to.as_deref(), force, lease, tip).await
                    })
                }),
            )
        }
        OpRequest::Pull { branch, mode, .. } => {
            check_branch_name(&branch)?;
            (
                fixed(format!("Pull {branch}")),
                body(move |ctx, _| {
                    Box::pin(async move { network::pull(ctx, &branch, mode).await })
                }),
            )
        }
        OpRequest::FetchIntoLocal {
            remote_branch,
            local_name,
        } => {
            check_arg(&remote_branch, "remote branch")?;
            check_branch_name(&local_name)?;
            (
                fixed(format!("Fetch {remote_branch} into {local_name}")),
                body(move |ctx, _| {
                    Box::pin(async move {
                        network::fetch_into_local(ctx, &remote_branch, &local_name).await
                    })
                }),
            )
        }
        OpRequest::DeleteRemoteBranch { remote, branch } => {
            check_arg(&remote, "remote")?;
            check_branch_name(&branch)?;
            (
                fixed(format!("Delete branch {branch} on {remote}")),
                body(move |ctx, dangers| {
                    Box::pin(async move {
                        let lease = token_of(dangers, DangerKind::DeleteRemoteBranch);
                        network::delete_remote_branch(ctx, &remote, &branch, lease).await
                    })
                }),
            )
        }
        OpRequest::StashSave {
            message,
            include_untracked,
            ..
        } => (
            fixed(if include_untracked {
                "Stash changes and untracked files".to_owned()
            } else {
                "Stash changes".to_owned()
            }),
            body(move |ctx, _| {
                Box::pin(
                    async move { stash::save(ctx, message.as_deref(), include_untracked).await },
                )
            }),
        ),
        OpRequest::StashApply { stash: hash, .. } => {
            check_full_hash(&hash)?;
            (
                fixed(format!("Apply stash {}", local::short(&hash))),
                body(move |ctx, _| {
                    Box::pin(
                        async move { stash::put_back(ctx, &hash, stash::PutBack::Apply).await },
                    )
                }),
            )
        }
        OpRequest::StashPop { stash: hash, .. } => {
            check_full_hash(&hash)?;
            (
                fixed(format!("Pop stash {}", local::short(&hash))),
                body(move |ctx, _| {
                    Box::pin(async move { stash::put_back(ctx, &hash, stash::PutBack::Pop).await })
                }),
            )
        }
        OpRequest::StashDrop { stash: hash, .. } => {
            check_full_hash(&hash)?;
            (
                fixed(format!("Drop stash {}", local::short(&hash))),
                body(move |ctx, _| Box::pin(async move { stash::drop(ctx, &hash).await })),
            )
        }
        OpRequest::StashBranch {
            stash: hash, name, ..
        } => {
            check_full_hash(&hash)?;
            check_branch_name(&name)?;
            (
                fixed(format!(
                    "Create branch {name} from stash {}",
                    local::short(&hash)
                )),
                body(move |ctx, _| Box::pin(async move { stash::branch(ctx, &hash, &name).await })),
            )
        }
        OpRequest::Continue { .. } => (
            on_progress(|kind| progress::title("Continue", kind)),
            body(move |ctx, _| Box::pin(async move { progress::continue_op(ctx).await })),
        ),
        OpRequest::Abort { .. } => (
            on_progress(|kind| progress::title("Abort", kind)),
            body(move |ctx, _| Box::pin(async move { progress::abort(ctx).await })),
        ),
        OpRequest::Skip { commit, .. } => {
            check_full_hash(&commit)?;
            (
                on_progress(progress::skip_title),
                body(move |ctx, _| Box::pin(async move { progress::skip(ctx).await })),
            )
        }
        OpRequest::WorktreeAdd { path, branch } => {
            check_path(&path)?;
            match &branch {
                WorktreeBranch::Existing { existing } => check_arg(existing, "branch")?,
                WorktreeBranch::Create { create, at } => {
                    check_branch_name(create)?;
                    check_full_hash(at)?;
                }
                WorktreeBranch::Detach { detach } => check_full_hash(detach)?,
            }
            (
                fixed(format!("Create worktree {path}")),
                body(move |ctx, _| {
                    Box::pin(async move { worktree::add(ctx, &path, &branch).await })
                }),
            )
        }
        OpRequest::WorktreeRemove {
            path,
            delete_branch,
        } => {
            check_path(&path)?;
            let title = if delete_branch {
                format!("Delete worktree {path} and its branch")
            } else {
                format!("Delete worktree {path}")
            };
            (
                fixed(title),
                body(move |ctx, dangers| {
                    Box::pin(
                        async move { worktree::remove(ctx, dangers, &path, delete_branch).await },
                    )
                }),
            )
        }
        OpRequest::WorktreePrune {} => (
            fixed("Prune stale worktree entries".to_owned()),
            body(move |ctx, _| Box::pin(async move { worktree::prune(ctx).await })),
        ),
        OpRequest::Commit {
            paths,
            message,
            push,
            ..
        } => {
            commit::check(&paths, &message)?;
            let files = commit::files(paths.len());
            (
                on_head(move |head| {
                    if push {
                        format!("Commit {files} on {head} and push")
                    } else {
                        format!("Commit {files} on {head}")
                    }
                }),
                body(move |ctx, _| {
                    Box::pin(async move { commit::commit(ctx, &paths, &message, push).await })
                }),
            )
        }
    };
    Ok(Prepared { title, body })
}

/// The token of the danger of `kind` among `dangers` (confirmed with it), if any.
fn token_of(dangers: &[Danger], kind: DangerKind) -> Option<&str> {
    dangers
        .iter()
        .find(|d| d.kind == kind)
        .and_then(|d| d.token.as_deref())
}

/// `Ok` unless `mainline` is 0: git numbers a merge's parents from 1.
fn check_mainline(mainline: Option<u32>) -> Result<()> {
    if mainline == Some(0) {
        return Err(CoreError::new(
            ErrorCode::BadRequest,
            "A merge's parents are numbered from 1.",
        ));
    }
    Ok(())
}

/// `Ok` if `hash` is a full commit hash: 40 (SHA-1) or 64 (SHA-256) lowercase hex digits.
/// Anything else (a range, a ref name, a short hash) is refused, so it never reaches git.
fn check_full_hash(hash: &str) -> Result<()> {
    let hex = hash
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if hex && (hash.len() == 40 || hash.len() == 64) {
        return Ok(());
    }
    Err(CoreError::new(
        ErrorCode::BadRequest,
        format!("\u{201c}{hash}\u{201d} is not a full commit hash."),
    ))
}

/// `Ok` if `path` can name a folder: not empty, no NUL.
fn check_path(path: &str) -> Result<()> {
    if path.trim().is_empty() || path.contains('\0') {
        return Err(CoreError::new(
            ErrorCode::BadRequest,
            format!("\u{201c}{path}\u{201d} is not a folder path."),
        ));
    }
    Ok(())
}

/// `Ok` if `value`, naming an existing `what` (a branch, a commit), can be given to git:
/// not empty and not option-like.
fn check_arg(value: &str, what: &str) -> Result<()> {
    if value.is_empty() || value.starts_with('-') {
        return Err(CoreError::new(
            ErrorCode::BadRequest,
            format!("\u{201c}{value}\u{201d} is not a {what}."),
        ));
    }
    Ok(())
}

/// An operation asked for: it waits for its turn in the repository's queue, then runs.
struct Turn {
    repo: Arc<Repo>,
    request: OpRequest,
    confirmed: Vec<Confirmed>,
    title: Title,
    body: Body,
    /// A numbered request's turn to join the queue (R28): released once it has joined.
    numbered: Option<NumberedTurn>,
}

/// What a turn in the queue ended with.
enum Ran {
    Step(Step),
    /// Nothing ran: these dangers were not all confirmed.
    Unconfirmed(Vec<Danger>),
    /// Nothing ran: it cannot while git is stopped in something (the plain line why).
    Refused(String),
}

/// Said when an operation's body panicked.
const STOPPED_UNEXPECTEDLY: &str = "The operation stopped unexpectedly.";

impl RepoManager {
    /// Runs `request` on repository `repo_id` once the operations asked for before it have
    /// ended, and returns how it went, with the repository as it is afterwards.
    /// `confirmed` lists the dangers the user has agreed to, each with the token it came
    /// with; if the request carries any other, or one whose state has changed since (another
    /// token), nothing runs and the outcome is `needs_confirmation` with every danger it
    /// carries, as they are now. A bad name is a `bad_name` error, before anything is queued or run.
    ///
    /// The operation runs on its own task: a caller that goes away does not stop it
    /// halfway, nor leave the repository marked busy.
    pub async fn run_op(
        self: &Arc<Self>,
        repo_id: u32,
        request: OpRequest,
        confirmed: &[Confirmed],
    ) -> Result<OpOutcome> {
        self.run_numbered(repo_id, None, request, confirmed).await
    }

    /// Stops operation `op_id` of repository `repo_id` if it is running (R41): the git
    /// command it runs, with everything that command started, is ended, nothing further
    /// runs, and the operation ends as failed ("Stopped."), with the usual refresh. Whether
    /// it was running.
    pub fn stop_op(&self, repo_id: u32, op_id: u64) -> Result<bool> {
        let repo = self.get(repo_id)?;
        let running = repo
            .running_stop
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        Ok(match running {
            Some((id, stop)) if id == op_id => {
                stop.stop();
                true
            }
            _ => false,
        })
    }

    /// Gives up number `n` of client `client` on repository `repo_id`: a request that will
    /// never run (its arguments could not be read). Nothing for an unknown repository.
    pub fn release_number(&self, repo_id: u32, client: &str, n: u64) {
        if let Ok(repo) = self.get(repo_id) {
            repo.op_order.of(client).release(n);
        }
    }

    /// How many numbered requests of client `client` wait for their turn on repository
    /// `repo_id` (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn numbered_waiting(&self, repo_id: u32, client: &str) -> usize {
        self.get(repo_id)
            .map_or(0, |repo| repo.op_order.of(client).waiting())
    }

    /// Like [`RepoManager::run_op`], for request number `n` of client `client`'s requests to
    /// the repository (R28): it joins the queue only after the ones numbered before it,
    /// waiting up to [`order::ORDER_WAIT`] for one that has not arrived (see [`order`]).
    pub async fn run_op_in_order(
        self: &Arc<Self>,
        repo_id: u32,
        client: &str,
        n: u64,
        request: OpRequest,
        confirmed: &[Confirmed],
    ) -> Result<OpOutcome> {
        self.run_numbered(repo_id, Some((client, n)), request, confirmed)
            .await
    }

    async fn run_numbered(
        self: &Arc<Self>,
        repo_id: u32,
        n: Option<(&str, u64)>,
        request: OpRequest,
        confirmed: &[Confirmed],
    ) -> Result<OpOutcome> {
        let prepared = prepare(&request);
        let repo = self.get(repo_id)?;
        let numbered = match n {
            Some((client, n)) => Some(repo.op_order.of(client).wait_for_turn(n).await),
            None => None,
        };
        // Refused here, it never joins the queue: dropping its turn lets the next one in.
        let Prepared { title, body } = prepared?;
        let turn = Turn {
            repo,
            request,
            confirmed: confirmed.to_vec(),
            title,
            body,
            numbered,
        };
        let m = self.clone();
        let task = tokio::spawn(async move { m.run_in_turn(turn).await });
        // Only the body can panic, and that is handled within the turn.
        task.await.unwrap_or_else(|e| {
            Err(CoreError::new(ErrorCode::Io, STOPPED_UNEXPECTEDLY).with_detail(e.to_string()))
        })
    }

    /// Waits for the turn, then runs the operation and ends it (see [`RepoManager::run_op`]).
    /// The body runs as a task of its own, so that if it panics the operation still ends
    /// here, within the turn: the repository is refreshed and no longer marked busy before
    /// the next operation starts.
    async fn run_in_turn(self: &Arc<Self>, mut turn: Turn) -> Result<OpOutcome> {
        let repo = turn.repo.clone();
        repo.op_waiting.fetch_add(1, Ordering::SeqCst);
        self.op_queue_changed(&repo).await;
        // The mutex is fair: polled once, the request has its place in the queue, and the
        // next numbered one may join behind it.
        let mut queue = std::pin::pin!(repo.op_queue.lock());
        let first = std::future::poll_fn(|cx| Poll::Ready(queue.as_mut().poll(cx))).await;
        drop(turn.numbered.take());
        let _turn = match first {
            Poll::Ready(guard) => guard,
            Poll::Pending => queue.await,
        };
        repo.op_waiting.fetch_sub(1, Ordering::SeqCst);
        // Numbered when its turn comes, so ids follow the order operations run in.
        let op_id = self.next_op_id();
        // Nothing of ours runs beside an operation (R13).
        self.cancel_scheduled_fetch(&repo).await;
        if repo.is_closed().await {
            return Err(unknown_repo(repo.identity.repo_id));
        }

        // Set once the operation is published as running; only then is it ended with a
        // refresh. A request that needs confirmation shows nothing.
        let started = Arc::new(AtomicBool::new(false));
        let continuing = matches!(turn.request, OpRequest::Continue { .. });
        // Where the body says which worktree it made (worktree_add), or which commit (commit).
        let created = Arc::new(std::sync::Mutex::new(None));
        let committed = Arc::new(std::sync::Mutex::new(None));
        let inner = tokio::spawn({
            let made = (created.clone(), committed.clone());
            let (m, started) = (self.clone(), started.clone());
            async move { m.run_body(turn, op_id, &started, &made).await }
        });
        let ran = inner.await.unwrap_or_else(|e| {
            Err(CoreError::new(ErrorCode::Io, STOPPED_UNEXPECTEDLY).with_detail(e.to_string()))
        });
        // A refused continue is read again too (R33): the files it names were edited outside
        // the app, where nothing git watches changes, and the banner must show them as they are.
        let refused_continue = continuing && matches!(ran, Ok((_, Ran::Refused(_))));
        let snapshot = if started.load(Ordering::SeqCst) || refused_continue {
            self.finish_op(&repo, op_id).await
        } else {
            Ok(self.outcome_snapshot(&repo).await)
        };
        let (title, ran) = ran?;
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
            created: created.lock().unwrap_or_else(|e| e.into_inner()).take(),
            committed: committed.lock().unwrap_or_else(|e| e.into_inner()).take(),
            snapshot,
        };
        match ran {
            Ran::Step(Step::Done(message)) => outcome.message = message,
            Ran::Step(Step::DoneWith {
                message,
                detail,
                hints,
            }) => {
                outcome.message = message;
                outcome.detail = detail;
                outcome.hints = hints;
            }
            Ran::Step(Step::Stopped { message, detail }) => {
                outcome.status = OpStatus::Stopped;
                outcome.message = message;
                outcome.detail = Some(detail);
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
            Ran::Refused(message) => {
                outcome.status = OpStatus::Failed;
                outcome.message = message;
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
        // Making the active worktree active is no follow-up.
        let active = std::path::Path::new(&outcome.snapshot.active_worktree).to_path_buf();
        outcome.hints.retain(|hint| {
            !matches!(hint, OpHint::MakeActive { worktree }
                if crate::repo::canonical(std::path::Path::new(worktree)) == active)
        });
        Ok(outcome)
    }

    /// Titles the operation after the branch HEAD is on now and what git is stopped in,
    /// refuses it if it cannot run while git is stopped (continue or abort with nothing
    /// stopped is `nothing_to_do`), checks its request for dangers and, if every one is
    /// confirmed, publishes it as running (setting `started`) and runs its body in the
    /// active worktree. Returns the title and how it ended.
    async fn run_body(
        self: &Arc<Self>,
        turn: Turn,
        op_id: u64,
        started: &AtomicBool,
        made: &(
            Arc<std::sync::Mutex<Option<String>>>,
            Arc<std::sync::Mutex<Option<String>>>,
        ),
    ) -> Result<(String, Ran)> {
        let Turn {
            repo,
            request,
            confirmed,
            title,
            body,
            numbered: _,
        } = turn;
        let stop = Stop::new();
        let ctx = OpCtx {
            repo: &repo,
            manager: self,
            runner: repo.runner.for_op(op_id, stop.clone()),
            // The worktree operations name the worktree they are about, and run from the
            // main one whichever is active.
            cwd: if request.runs_in_main() {
                repo.root.clone()
            } else {
                repo.worktree()
            },
            op_id,
            sink: self.sink.as_ref(),
            net_limit: self.net_limit(),
            kept: std::sync::atomic::AtomicBool::new(false),
            held_read: std::sync::Mutex::new(None),
            created: made.0.clone(),
            committed: made.1.clone(),
        };
        // A request runs only in the worktree it was asked for: switching worktrees never
        // redirects one already asked for. Checked before anything is read there.
        if let Some(expected) = request.expect_worktree() {
            let why = match worktree_moved(expected, &ctx.cwd) {
                Some(why) => Some(why),
                None => no_longer_a_worktree(&ctx, expected).await,
            };
            if let Some(why) = why {
                let title = title(&TurnFacts {
                    head: "HEAD",
                    in_progress: None,
                });
                repo.runner.log().name_op(op_id, &title);
                return Ok((title, Ran::Refused(why)));
            }
        }
        let head = local::current_branch(&ctx).await?;
        let in_progress = read_kind(&ctx.runner, &ctx.cwd).await?;
        let title = title(&TurnFacts {
            head: head.as_deref().unwrap_or("detached HEAD"),
            in_progress,
        });
        repo.runner.log().name_op(op_id, &title);
        if matches!(
            (&request, in_progress),
            (
                OpRequest::Continue { .. } | OpRequest::Abort { .. } | OpRequest::Skip { .. },
                None
            )
        ) {
            return Err(progress::nothing_in_progress());
        }
        // A request on the branch checked out acts on the one the user saw (R39).
        if let Some((expected, strict)) = expected_head(&request, head.as_deref()) {
            let now = HeadInfo {
                branch: head.clone(),
                hash: local::head_commit(&ctx).await?,
            };
            if let Some(why) = head_moved(expected, &now, strict) {
                return Ok((title, Ran::Refused(why)));
            }
        }
        match (&request, in_progress) {
            (_, Some(kind)) => {
                if let Some(why) = progress::refused_while(&request, kind, head.as_deref()) {
                    return Ok((title, Ran::Refused(why)));
                }
                // Continuing stages the conflicted files: never one the user has not resolved,
                // unless they saw it listed and went ahead (R31).
                if matches!(
                    request,
                    OpRequest::Continue {
                        even_with_markers: false,
                        ..
                    }
                ) && let Some(why) = progress::unresolved_conflicts(&ctx).await?
                {
                    return Ok((title, Ran::Refused(why)));
                }
            }
            // Conflicts nothing in progress explains (a stash pop's) stop what would touch
            // the conflicted files, before git can.
            (_, None) if progress::changes_head_or_tree(&request, head.as_deref()) => {
                if let Some(why) = progress::unexplained_conflicts(&ctx).await? {
                    return Ok((title, Ran::Refused(why)));
                }
            }
            _ => {}
        }
        if let Some(remote) = remote_of(&request)
            && let Some(why) = network::unknown_remote(&ctx, remote).await?
        {
            return Ok((title, Ran::Refused(why)));
        }
        if let OpRequest::Push {
            branch,
            remote,
            to: Some(to),
            ..
        } = &request
            && to != branch
            && let Some(why) = network::not_its_upstream(&ctx, branch, remote, to).await?
        {
            return Ok((title, Ran::Refused(why)));
        }
        if let OpRequest::Skip { commit, .. } = &request
            && let Some(why) = progress::skip_refused(&ctx, commit).await?
        {
            return Ok((title, Ran::Refused(why)));
        }
        if let Some(hash) = stash_of(&request)
            && let Some(why) = stash::gone(&ctx, hash).await?
        {
            return Ok((title, Ran::Refused(why)));
        }
        if let Some(why) = worktree::refused(&ctx, &request).await? {
            return Ok((title, Ran::Refused(why)));
        }
        if let OpRequest::Commit { paths, push, .. } = &request
            && let Some(why) =
                commit::refused(&ctx, paths, *push, in_progress, head.as_deref()).await?
        {
            return Ok((title, Ran::Refused(why)));
        }
        let dangers = dangers(&ctx, &request).await?;
        if !all_confirmed(&dangers, &confirmed) {
            return Ok((title, Ran::Unconfirmed(dangers)));
        }
        self.start_op(&repo, op_id, &title).await?;
        started.store(true, Ordering::SeqCst);
        *repo.running_stop.lock().unwrap_or_else(|e| e.into_inner()) = Some((op_id, stop.clone()));
        let step = body(&ctx, &dangers).await;
        *repo.running_stop.lock().unwrap_or_else(|e| e.into_inner()) = None;
        // Stopped by the user (R41): whatever the body made of it, the operation failed, with
        // what git said before it was ended; unless the body says what was done before the
        // Stop that stands (a commit made).
        let step = if stop.is_stopped() && !ctx.kept() {
            let detail = match step {
                Ok(Step::Failed { detail, .. } | Step::Stopped { detail, .. }) => detail,
                Ok(Step::DoneWith { detail, .. }) => detail.unwrap_or_default(),
                Ok(Step::Done(_)) => String::new(),
                Err(e) => e.detail.unwrap_or_default(),
            };
            Ok(Step::Failed {
                message: STOPPED.to_owned(),
                detail,
                hints: Vec::new(),
            })
        } else {
            step
        };
        let delay = self.op_delay();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        #[cfg(any(test, feature = "testutil"))]
        if self.take_op_panic() {
            panic!("test: the operation's body panicked");
        }
        step.map(|step| (title, Ran::Step(step)))
    }
}

/// Whether every danger in `dangers` is confirmed: `confirmed` has its kind with the same
/// token, so the user saw the state it is about as it is now (R11, R39).
fn all_confirmed(dangers: &[Danger], confirmed: &[Confirmed]) -> bool {
    dangers.iter().all(|d| {
        confirmed
            .iter()
            .any(|c| c.kind == d.kind && c.token == d.token)
    })
}

/// HEAD as the UI showed it for `request`, if the request acts on the branch checked out,
/// and whether its commit must be the same too (a reset, a commit, or a continue, abort or
/// skip: what they do depends on where HEAD is; a merge, rebase, cherry-pick or revert only on which
/// branch, so that several asked for one after another still run). `head` is the branch
/// HEAD is on now. A pull counts when the branch it pulls is checked out, then or now.
fn expected_head<'r>(request: &'r OpRequest, head: Option<&str>) -> Option<(&'r HeadInfo, bool)> {
    match request {
        OpRequest::Reset { expect_head, .. }
        | OpRequest::Commit { expect_head, .. }
        | OpRequest::Continue { expect_head, .. }
        | OpRequest::Abort { expect_head, .. }
        | OpRequest::Skip { expect_head, .. } => Some((expect_head, true)),
        OpRequest::Merge { expect_head, .. }
        | OpRequest::Rebase { expect_head, .. }
        | OpRequest::CherryPick { expect_head, .. }
        | OpRequest::Revert { expect_head, .. } => Some((expect_head, false)),
        OpRequest::Pull {
            branch,
            expect_head,
            ..
        } => (expect_head.branch.as_deref() == Some(branch) || head == Some(branch))
            .then_some((expect_head, false)),
        _ => None,
    }
}

/// Why a request asked for with HEAD at `expected` must not run with HEAD at `now`, if it
/// must not: another branch is checked out (or HEAD is detached, or a branch is checked out
/// where it was detached), or, while detached or with `strict`, HEAD is at another commit.
fn head_moved(expected: &HeadInfo, now: &HeadInfo, strict: bool) -> Option<String> {
    let at = |h: &HeadInfo| {
        h.hash
            .as_deref()
            .map_or("no commit", local::short)
            .to_owned()
    };
    let why = match (expected.branch.as_deref(), now.branch.as_deref()) {
        (Some(was), Some(is)) if was != is => {
            format!("it was asked for on {was}, but {is} is checked out now")
        }
        (Some(was), None) => format!(
            "it was asked for on {was}, but HEAD is detached now (at {})",
            at(now)
        ),
        (None, Some(is)) => format!(
            "it was asked for on the detached HEAD at {}, but {is} is checked out now",
            at(expected)
        ),
        (None, None) if expected.hash != now.hash => format!(
            "HEAD has moved since it was asked for (from {} to {})",
            at(expected),
            at(now)
        ),
        (Some(was), Some(_)) if strict && expected.hash != now.hash => format!(
            "{was} has moved since it was asked for (from {} to {})",
            at(expected),
            at(now)
        ),
        _ => return None,
    };
    Some(format!(
        "Nothing was done: {why}. Look at it again, and ask again if it still applies."
    ))
}

/// Why a request asked for in the worktree `expected` must not run with `active` (a path with
/// symlinks resolved) the active worktree, if it must not.
fn worktree_moved(expected: &str, active: &std::path::Path) -> Option<String> {
    let same = dunce::canonicalize(expected).map_or_else(
        |_| std::path::Path::new(expected) == active,
        |p| p == active,
    );
    (!same).then(|| {
        format!(
            "That was asked for in {expected}, but {} is the active worktree now.",
            active.display()
        )
    })
}

/// Why a request asked for in the worktree `expected` must not run in `ctx.cwd`, the active
/// worktree, if its folder is no longer the top of a worktree (removed, or replaced by a
/// plain folder, outside the app).
async fn no_longer_a_worktree(ctx: &OpCtx<'_>, expected: &str) -> Option<String> {
    let top = ctx
        .read(&["rev-parse", "--show-toplevel"])
        .await
        .ok()
        .filter(|out| out.code == 0)
        .map(|out| out.text().trim().to_owned());
    let same = top.is_some_and(|top| dunce::canonicalize(&top).is_ok_and(|top| top == ctx.cwd));
    (!same).then(|| {
        format!(
            "That was asked for in {expected}, but {} is no longer a worktree.",
            ctx.cwd.display()
        )
    })
}

/// The remote a request names, if any: it must be one of the repository's remotes.
fn remote_of(request: &OpRequest) -> Option<&str> {
    match request {
        OpRequest::Push { remote, .. }
        | OpRequest::PushTag { remote, .. }
        | OpRequest::DeleteRemoteBranch { remote, .. }
        | OpRequest::DeleteTag {
            remote: Some(remote),
            ..
        } => Some(remote),
        _ => None,
    }
}

/// The stash a request names (by commit), if any: it must still be in the list.
fn stash_of(request: &OpRequest) -> Option<&str> {
    match request {
        OpRequest::StashApply { stash, .. }
        | OpRequest::StashPop { stash, .. }
        | OpRequest::StashDrop { stash, .. }
        | OpRequest::StashBranch { stash, .. } => Some(stash),
        _ => None,
    }
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
    fn confirmation_covers_every_danger_with_its_token() {
        let danger = |kind, token: Option<&str>| Danger {
            kind,
            message: String::new(),
            detail: None,
            token: token.map(str::to_owned),
        };
        let yes = |kind, token: Option<&str>| Confirmed {
            kind,
            token: token.map(str::to_owned),
        };
        let both = [
            danger(DangerKind::HardReset, Some("main\nabc\n0")),
            danger(DangerKind::DropStash, Some("def")),
        ];
        assert!(all_confirmed(&[], &[]));
        assert!(!all_confirmed(
            &both,
            &[yes(DangerKind::HardReset, Some("main\nabc\n0"))]
        ));
        assert!(all_confirmed(
            &both,
            &[
                yes(DangerKind::DropStash, Some("def")),
                yes(DangerKind::HardReset, Some("main\nabc\n0")),
                yes(DangerKind::ForcePush, None),
            ]
        ));
        // Every danger is confirmed only with the token the user saw (R11, R39).
        let reset = [danger(DangerKind::HardReset, Some("main\nabc\n0"))];
        assert!(!all_confirmed(
            &reset,
            &[yes(DangerKind::HardReset, Some("feature\nabc\n0"))]
        ));
        assert!(!all_confirmed(&reset, &[yes(DangerKind::HardReset, None)]));
        let force = [danger(DangerKind::ForcePush, Some("abc"))];
        assert!(all_confirmed(
            &force,
            &[yes(DangerKind::ForcePush, Some("abc"))]
        ));
        assert!(!all_confirmed(
            &force,
            &[yes(DangerKind::ForcePush, Some("old"))]
        ));
        assert!(!all_confirmed(&force, &[yes(DangerKind::ForcePush, None)]));
        assert!(!all_confirmed(
            &force,
            &[yes(DangerKind::HardReset, Some("abc"))]
        ));
        let untracked = [danger(DangerKind::ForcePush, None)];
        assert!(all_confirmed(
            &untracked,
            &[yes(DangerKind::ForcePush, None)]
        ));
        assert!(!all_confirmed(
            &untracked,
            &[yes(DangerKind::ForcePush, Some("abc"))]
        ));
    }

    #[test]
    fn a_request_on_the_current_branch_runs_only_on_the_head_it_was_asked_for() {
        let head = |branch: Option<&str>, hash: &str| HeadInfo {
            branch: branch.map(str::to_owned),
            hash: Some(hash.repeat(40)),
        };
        let main_a = head(Some("main"), "a");
        // Same branch, same commit: runs.
        assert_eq!(head_moved(&main_a, &main_a, true), None);
        // Another branch: never.
        let why = head_moved(&main_a, &head(Some("feature"), "a"), false).unwrap();
        assert_eq!(
            why,
            "Nothing was done: it was asked for on main, but feature is checked out now. Look \
             at it again, and ask again if it still applies."
        );
        assert!(head_moved(&main_a, &head(None, "a"), false).is_some());
        assert!(head_moved(&head(None, "a"), &main_a, false).is_some());
        // The same branch moved on: only where the commit matters (strict).
        assert_eq!(head_moved(&main_a, &head(Some("main"), "b"), false), None);
        let why = head_moved(&main_a, &head(Some("main"), "b"), true).unwrap();
        assert!(why.contains("main has moved since"), "{why}");
        // Detached: the commit is what it is about.
        assert!(head_moved(&head(None, "a"), &head(None, "b"), false).is_some());
        assert_eq!(head_moved(&head(None, "a"), &head(None, "a"), false), None);

        // Which requests are bound, and how.
        let reset = OpRequest::Reset {
            hash: "c".repeat(40),
            mode: ResetMode::Soft,
            expect_head: main_a.clone(),
            expect_worktree: String::new(),
        };
        assert_eq!(expected_head(&reset, Some("main")), Some((&main_a, true)));
        let pull = |branch: &str| OpRequest::Pull {
            branch: branch.into(),
            mode: crate::dto::PullMode::Default,
            expect_head: main_a.clone(),
            expect_worktree: String::new(),
        };
        assert!(expected_head(&pull("main"), Some("main")).is_some());
        assert!(
            expected_head(&pull("other"), Some("other")).is_some(),
            "now current"
        );
        assert!(
            expected_head(&pull("other"), Some("main")).is_none(),
            "only fetches"
        );
        let create = OpRequest::CreateBranch {
            name: "x".into(),
            at: "c".repeat(40),
            checkout: false,
            expect_worktree: String::new(),
        };
        assert!(expected_head(&create, Some("main")).is_none());
    }

    #[test]
    fn bad_input_is_refused_without_git() {
        let refused = |r: &OpRequest| prepare(r).err().expect("refused").code;
        assert_eq!(
            refused(&OpRequest::StashDrop {
                stash: "stash@{0}".into(),
                expect_worktree: String::new()
            }),
            ErrorCode::BadRequest
        );
        assert_eq!(
            refused(&OpRequest::StashBranch {
                stash: "0123456789abcdef0123456789abcdef01234567".into(),
                name: "-x".into(),
                expect_worktree: String::new(),
            }),
            ErrorCode::BadName
        );
        let option_like = OpRequest::CreateBranch {
            name: "x".into(),
            at: "--all".into(),
            checkout: false,
            expect_worktree: String::new(),
        };
        assert_eq!(refused(&option_like), ErrorCode::BadRequest);
        let ok = OpRequest::CreateBranch {
            name: "x".into(),
            at: "0123456789abcdef0123456789abcdef01234567".into(),
            checkout: false,
            expect_worktree: String::new(),
        };
        let facts = |head| TurnFacts {
            head,
            in_progress: None,
        };
        assert_eq!(
            prepare(&ok).map(|p| (p.title)(&facts("main"))).expect("ok"),
            "Create branch x"
        );
        let merge = OpRequest::Merge {
            target: crate::dto::MergeTarget {
                kind: MergeTargetKind::Branch,
                name: "x".into(),
            },
            no_ff: true,
            squash: false,
            no_commit: false,
            expect_head: HeadInfo::default(),
            expect_worktree: String::new(),
        };
        let title = prepare(&merge).map(|p| p.title).expect("ok");
        assert_eq!(title(&facts("trunk")), "Merge x into trunk");
        let abort = OpRequest::Abort {
            expect_head: HeadInfo::default(),
            expect_worktree: String::new(),
        };
        let title = prepare(&abort).map(|p| p.title).expect("ok");
        assert_eq!(
            title(&TurnFacts {
                head: "main",
                in_progress: Some(InProgressKind::CherryPick)
            }),
            "Abort the cherry-pick"
        );
    }

    /// The second line behind the refusal of unexplained conflicts (Task 5 review, item 1b):
    /// a squash merge reached directly, over unmerged entries it did not make, fails as git
    /// says and is not undone with `reset --merge`, which would take the user's edit of the
    /// conflicted file with it.
    #[tokio::test]
    async fn a_squash_over_conflicts_it_did_not_make_is_not_undone() {
        use crate::dto::{MergeTarget, MergeTargetKind};
        use crate::git::runner::{CommandLog, GitRunner};
        use crate::testutil::TestRepo;

        let repo = TestRepo::new();
        repo.write("notes", "one\n");
        repo.commit_all("notes");
        repo.branch("side");
        repo.checkout("side");
        repo.write("side.txt", "side\n");
        repo.commit_all("on side");
        repo.checkout("main");
        repo.write("notes", "stashed\n");
        repo.git(&["stash", "push", "-q"]);
        repo.write("notes", "two\n");
        repo.commit_all("notes two");
        assert!(
            !repo.git_succeeds(&["stash", "pop", "-q"]),
            "the pop conflicts"
        );
        repo.write("notes", "the user's own edit\n");

        let log = CommandLog::new();
        let runner = GitRunner::detect(log.clone())
            .await
            .expect("git is installed");
        let m = RepoManager::new(runner, Arc::new(RecordingSink::default()));
        let id = m.open(repo.path()).await.expect("opens").repo_id;
        let r = m.get(id).expect("open");
        let ctx = OpCtx {
            repo: &r,
            manager: &m,
            runner: r.runner.for_op(1, Stop::new()),
            cwd: r.worktree(),
            op_id: 1,
            sink: m.sink.as_ref(),
            net_limit: m.net_limit(),
            kept: std::sync::atomic::AtomicBool::new(false),
            held_read: std::sync::Mutex::new(None),
            created: Arc::default(),
            committed: Arc::default(),
        };
        let mark = log.since(0).last().map_or(0, |c| c.id);

        let target = MergeTarget {
            kind: MergeTargetKind::Branch,
            name: "side".into(),
        };
        let step = local::merge(&ctx, &target, false, true, false)
            .await
            .unwrap();
        assert!(matches!(step, Step::Failed { .. }), "{step:?}");
        let resets: Vec<_> = log
            .since(mark)
            .into_iter()
            .filter(|c| c.args.first().map(String::as_str) == Some("reset"))
            .collect();
        assert!(resets.is_empty(), "nothing undone: {resets:?}");
        assert_eq!(
            std::fs::read(repo.path().join("notes")).unwrap(),
            b"the user's own edit\n"
        );
    }
}
