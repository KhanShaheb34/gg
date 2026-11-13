//! Local operations: checkout, branches, merge, rebase, cherry-pick, revert, reset and
//! tags. Each body runs git in the active worktree and turns how it ended into a [`Step`]:
//! done, stopped on conflicts, or failed (with an explanation where one is known).

use crate::dto::{
    Danger, DangerKind, InProgressKind, MergeTarget, MergeTargetKind, OpHint, ResetMode,
};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::progress::read_kind;
use crate::git::runner::GitOutput;
use crate::ops::explain::{NO_BRANCH, nothing_to_do};
use crate::ops::progress::unmerged_paths;
use crate::ops::{OpCtx, Step, explain, network};

/// The first 7 characters of `hash`, for messages.
pub(crate) fn short(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

/// Runs `args`; done with `done` if git exits 0, else as [`explain`] says.
async fn run(ctx: &OpCtx<'_>, args: &[&str], done: String) -> Result<Step> {
    let out = ctx.git(args).await?;
    Ok(if out.code == 0 {
        Step::Done(done)
    } else {
        explain(args, &out)
    })
}

/// `checkout <branch> --`; with `fast_forward`, then brought up to its upstream if that is
/// a fast-forward (see [`network::fast_forward_checked_out`]).
pub(crate) async fn checkout_branch(
    ctx: &OpCtx<'_>,
    branch: &str,
    fast_forward: bool,
) -> Result<Step> {
    let step = run(
        ctx,
        &["checkout", branch, "--"],
        format!("Checked out {branch}."),
    )
    .await?;
    match step {
        Step::Done(checked_out) if fast_forward => {
            network::fast_forward_checked_out(ctx, branch, checked_out).await
        }
        other => Ok(other),
    }
}

/// `checkout -b <local> --track refs/remotes/<remote_branch>`.
pub(crate) async fn checkout_remote(
    ctx: &OpCtx<'_>,
    remote_branch: &str,
    local: &str,
) -> Result<Step> {
    let start = format!("refs/remotes/{remote_branch}");
    run(
        ctx,
        &["checkout", "-b", local, "--track", &start],
        format!("Checked out {remote_branch} as {local}, which tracks it."),
    )
    .await
}

/// `checkout --detach <hash>`.
pub(crate) async fn checkout_commit(ctx: &OpCtx<'_>, hash: &str) -> Result<Step> {
    run(
        ctx,
        &["checkout", "--detach", hash],
        format!(
            "Checked out {}: HEAD is detached (no branch is checked out).",
            short(hash)
        ),
    )
    .await
}

/// A new branch at `at`: `branch -- <name> <at>`, or with `checkout`,
/// `checkout -b <name> <at>`. `bad_name` if a branch of that name exists (git is not asked
/// to make it).
pub(crate) async fn create_branch(
    ctx: &OpCtx<'_>,
    name: &str,
    at: &str,
    checkout: bool,
) -> Result<Step> {
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
    if checkout {
        run(
            ctx,
            &["checkout", "-b", name, at],
            format!("Created branch {name} and checked it out."),
        )
        .await
    } else {
        run(
            ctx,
            &["branch", "--", name, at],
            format!("Created branch {name}."),
        )
        .await
    }
}

/// `branch -m -- <old> <new>`.
pub(crate) async fn rename_branch(ctx: &OpCtx<'_>, old: &str, new: &str) -> Result<Step> {
    run(
        ctx,
        &["branch", "-m", "--", old, new],
        format!("Renamed branch {old} to {new}."),
    )
    .await
}

/// `branch -d -- <name>`, or `-D` once deleting it unmerged has been confirmed (it is
/// among `dangers`).
pub(crate) async fn delete_branch(ctx: &OpCtx<'_>, dangers: &[Danger], name: &str) -> Result<Step> {
    let unmerged = dangers
        .iter()
        .any(|d| d.kind == DangerKind::DeleteUnmergedBranch);
    let flag = if unmerged { "-D" } else { "-d" };
    run(
        ctx,
        &["branch", flag, "--", name],
        format!("Deleted branch {name}."),
    )
    .await
}

/// What git is stopped in, in the operation's worktree, if anything (see
/// [`read_kind`]); `None` too if that cannot be read.
pub(crate) async fn stopped_kind(ctx: &OpCtx<'_>) -> Option<InProgressKind> {
    read_kind(&ctx.runner, &ctx.cwd).await.ok().flatten()
}

/// Runs `args`, a command that can stop on conflicts (with `net`, one that talks to a
/// remote: see [`OpCtx::git_net`]). `Ok(out)` if git exits 0; else the step it ended with:
/// done if there was nothing to do, stopped if git is now in a state it was not in before,
/// with conflicts to resolve, else failed.
pub(crate) async fn git_may_stop(
    ctx: &OpCtx<'_>,
    args: &[&str],
    net: bool,
) -> Result<std::result::Result<GitOutput, Step>> {
    let before = stopped_kind(ctx).await;
    let out = if net {
        ctx.git_net(args).await?
    } else {
        ctx.git(args).await?
    };
    if out.code == 0 {
        return Ok(Ok(out));
    }
    let step = explain(args, &out);
    let after = if before.is_none() {
        stopped_kind(ctx).await
    } else {
        None
    };
    if let Step::Done(_) = step {
        // A cherry-pick that comes out empty stays in progress, waiting for a decision
        // about the empty commit; there is nothing to decide, so it is left.
        if after == Some(InProgressKind::CherryPick) {
            ctx.git(&["cherry-pick", "--quit"]).await?;
        }
        return Ok(Err(step));
    }
    let Some(kind) = after else {
        return Ok(Err(step));
    };
    let Step::Failed { detail, .. } = step else {
        return Ok(Err(step));
    };
    // In progress now, but only conflicts make it a stop: a hook that refused the commit
    // (pre-merge-commit, commit-msg) leaves it in progress too, with nothing to resolve.
    let unmerged = ctx.read(&["ls-files", "--unmerged", "-z"]).await?;
    Ok(Err(if unmerged.code == 0 && !unmerged.stdout.is_empty() {
        Step::Stopped {
            message: stopped_message(kind),
            detail,
        }
    } else {
        Step::Failed {
            message: stopped_without_conflicts(kind),
            detail,
            hints: Vec::new(),
        }
    }))
}

/// What `kind` is called in messages.
pub(crate) fn kind_name(kind: InProgressKind) -> &'static str {
    match kind {
        InProgressKind::Merge => "merge",
        InProgressKind::Rebase => "rebase",
        InProgressKind::CherryPick => "cherry-pick",
        InProgressKind::Revert => "revert",
    }
}

/// The plain line for git having stopped a `kind` on conflicts.
pub(crate) fn stopped_message(kind: InProgressKind) -> String {
    let what = kind_name(kind);
    format!("The {what} stopped on conflicts. Resolve them, then continue the {what}, or abort it.")
}

/// The plain line for git having failed with a `kind` left in progress but no conflicts:
/// a hook refused the commit, or something else git says below went wrong.
pub(crate) fn stopped_without_conflicts(kind: InProgressKind) -> String {
    match kind {
        InProgressKind::Merge => {
            "A hook stopped the merge commit; the merge is staged and not committed.".to_owned()
        }
        _ => {
            let what = kind_name(kind);
            format!(
                "The {what} stopped without conflicts (git says why below). It is still in \
                 progress: continue it, or abort it."
            )
        }
    }
}

/// The branch HEAD is on, or `None` when detached (or unborn and unreadable).
pub(crate) async fn current_branch(ctx: &OpCtx<'_>) -> Result<Option<String>> {
    let out = ctx.read(&["symbolic-ref", "-q", "--short", "HEAD"]).await?;
    Ok((out.code == 0)
        .then(|| out.text().trim().to_owned())
        .filter(|b| !b.is_empty()))
}

/// The commit HEAD points at, if any.
pub(crate) async fn head_commit(ctx: &OpCtx<'_>) -> Result<Option<String>> {
    let out = ctx
        .read(&["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
        .await?;
    Ok((out.code == 0).then(|| out.text().trim().to_owned()))
}

/// Whether `rev` names a commit.
async fn is_commit(ctx: &OpCtx<'_>, rev: &str) -> Result<bool> {
    let out = ctx
        .read(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ])
        .await?;
    Ok(out.code == 0)
}

/// What to give git for the branch `name` whose full ref is `full`: the short name, which
/// reads well in a merge message, unless git would take it for something else (a tag of the
/// same name); then the full ref.
async fn unambiguous<'n>(ctx: &OpCtx<'_>, name: &'n str, full: &'n str) -> Result<&'n str> {
    let out = ctx
        .read(&[
            "rev-parse",
            "--symbolic-full-name",
            "--verify",
            "--quiet",
            name,
        ])
        .await?;
    Ok(if out.code == 0 && out.text().trim() == full {
        name
    } else {
        full
    })
}

/// `merge --no-edit [--no-ff | --ff] [--squash] [--no-commit] <target>`. `--squash` goes
/// with `--ff`: git refuses it with `--no-ff`, which `merge.ff` may set. `--no-commit` goes
/// with `--no-ff`, else a fast-forward would commit after all. After a merge that
/// committed on a branch, push is offered (see [`push_hint`]).
pub(crate) async fn merge(
    ctx: &OpCtx<'_>,
    target: &MergeTarget,
    no_ff: bool,
    squash: bool,
    no_commit: bool,
) -> Result<Step> {
    let full = match target.kind {
        MergeTargetKind::Branch => format!("refs/heads/{}", target.name),
        MergeTargetKind::Remote => format!("refs/remotes/{}", target.name),
        MergeTargetKind::Commit => target.name.clone(),
    };
    let name = match target.kind {
        MergeTargetKind::Commit => target.name.as_str(),
        _ => unambiguous(ctx, &target.name, &full).await?,
    };
    let shown = match target.kind {
        MergeTargetKind::Commit => short(&target.name),
        _ => target.name.as_str(),
    };
    let mut args = vec!["merge", "--no-edit"];
    // Without committing, a fast-forward would still move the branch: always a merge then.
    args.push(if (no_ff || no_commit) && !squash {
        "--no-ff"
    } else {
        "--ff"
    });
    if squash {
        args.push("--squash");
    }
    if no_commit {
        args.push("--no-commit");
    }
    args.push(name);

    let branch = current_branch(ctx).await?;
    let before = head_commit(ctx).await?;
    // Unmerged entries the squash did not make must never be undone with it (run_op refuses
    // a merge over them; this is the second line).
    let unmerged_before = squash && !unmerged_paths(ctx).await?.is_empty();
    let out = match git_may_stop(ctx, &args, false).await? {
        Ok(out) => out,
        Err(step) if squash && !unmerged_before => return squash_failed(ctx, step).await,
        Err(step) if squash => return Ok(step),
        Err(step) => return Ok(step),
    };
    if let Some(message) = nothing_to_do(&args, &out) {
        return Ok(Step::Done(message));
    }
    let into = branch.as_deref().unwrap_or("detached HEAD");
    if squash {
        return Ok(autostash_note(
            Step::Done(format!(
                "Squashed {shown} into staged changes on {into}: commit them to finish."
            )),
            &out,
        ));
    }
    if stopped_kind(ctx).await == Some(InProgressKind::Merge) {
        return Ok(autostash_note(
            Step::Done(format!(
                "Merged {shown} into {into} without committing: the result is staged; commit \
                 it to finish the merge."
            )),
            &out,
        ));
    }
    let hints = match &branch {
        Some(b) if head_commit(ctx).await? != before => {
            push_hint(ctx, b).await?.into_iter().collect()
        }
        _ => Vec::new(),
    };
    let message = format!("Merged {shown} into {into}.");
    Ok(autostash_note(
        if hints.is_empty() {
            Step::Done(message)
        } else {
            Step::DoneWith {
                message,
                detail: None,
                hints,
            }
        },
        &out,
    ))
}

/// Said when a squash merge conflicts (R20).
const SQUASH_CONFLICTS: &str = "This squash merge has conflicts. A squash can't be continued \
                                here, so nothing was changed. Merge normally, or squash in a \
                                terminal.";

/// `step`, how a squash merge failed, when no file was unmerged before it started. One that
/// left conflicts (git leaves no MERGE_HEAD for a squash, so it could be neither continued
/// nor aborted) is undone with `reset --merge`,
/// which puts the index and the files the merge touched back and keeps the uncommitted
/// changes it did not touch; the step then says so (R20), with git's output.
async fn squash_failed(ctx: &OpCtx<'_>, step: Step) -> Result<Step> {
    let Step::Failed { detail, hints, .. } = &step else {
        return Ok(step);
    };
    // Unreadable: nothing is undone.
    if unmerged_paths(ctx)
        .await
        .map_or(true, |paths| paths.is_empty())
    {
        return Ok(step);
    }
    let args = ["reset", "--merge"];
    let out = ctx.git(&args).await?;
    if out.code != 0 {
        return Ok(explain(&args, &out));
    }
    Ok(Step::Failed {
        message: SQUASH_CONFLICTS.to_owned(),
        detail: detail.clone(),
        hints: hints.clone(),
    })
}

/// `step` (done), saying so if git's output `out` shows that the changes it auto-stashed
/// (`rebase.autoStash`, `merge.autoStash`) conflicted as they were put back: git exits 0,
/// keeps them in the stash and leaves its attempt in the working tree. git's text becomes
/// the detail.
pub(crate) fn autostash_note(step: Step, out: &GitOutput) -> Step {
    let all = format!("{}\n{}", out.stderr, out.text());
    if !all.contains("Applying autostash resulted in conflicts") {
        return step;
    }
    let (message, hints) = match step {
        Step::Done(message) => (message, Vec::new()),
        Step::DoneWith { message, hints, .. } => (message, hints),
        other => return other,
    };
    Step::DoneWith {
        message: format!(
            "{message} Your uncommitted changes could not be put back cleanly: they are kept \
             in the stash, and git left its attempt, with conflicts, in the working tree."
        ),
        detail: Some(all.trim().to_owned()),
        hints,
    }
}

/// Push for `branch` after it got new commits: to its upstream's remote, or, without an
/// upstream, to the repository's only remote (setting it as upstream). `None` otherwise.
async fn push_hint(ctx: &OpCtx<'_>, branch: &str) -> Result<Option<OpHint>> {
    let full = format!("refs/heads/{branch}");
    let out = ctx
        .read(&[
            "for-each-ref",
            "--format=%(refname)%00%(upstream:remotename)",
            &full,
        ])
        .await?;
    let upstream_remote = out.text().lines().find_map(|line| {
        let (name, remote) = line.split_once('\0')?;
        (name == full && !remote.is_empty()).then(|| remote.to_owned())
    });
    if let Some(remote) = upstream_remote {
        // "." is the repository itself: an upstream that is a local branch.
        return Ok((remote != ".").then(|| OpHint::Push {
            branch: branch.to_owned(),
            remote,
            set_upstream: false,
        }));
    }
    let remotes = ctx.read(&["remote"]).await?;
    let remotes: Vec<String> = remotes
        .text()
        .lines()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_owned)
        .collect();
    Ok(match <[String; 1]>::try_from(remotes) {
        Ok([remote]) => Some(OpHint::Push {
            branch: branch.to_owned(),
            remote,
            set_upstream: true,
        }),
        Err(_) => None,
    })
}

/// `rebase <onto>`, with `onto` given as a full ref: the local branch of that name, else
/// the remote-tracking one. Refused (nothing runs) on a detached HEAD: there is no branch
/// to rebase.
pub(crate) async fn rebase(ctx: &OpCtx<'_>, onto: &str) -> Result<Step> {
    let Some(branch) = current_branch(ctx).await? else {
        let at = head_commit(ctx).await?;
        return Ok(Step::Failed {
            message: NO_BRANCH.to_owned(),
            detail: format!(
                "HEAD is detached at {}.\n",
                at.as_deref().map_or("no commit", short)
            ),
            hints: Vec::new(),
        });
    };
    let local = format!("refs/heads/{onto}");
    let remote = format!("refs/remotes/{onto}");
    let full = if is_commit(ctx, &local).await? {
        local
    } else if is_commit(ctx, &remote).await? {
        remote
    } else {
        // git explains it does not exist.
        onto.to_owned()
    };
    let args = ["rebase", full.as_str()];
    let out = match git_may_stop(ctx, &args, false).await? {
        Ok(out) => out,
        Err(step) => return Ok(step),
    };
    let done =
        nothing_to_do(&args, &out).unwrap_or_else(|| format!("Rebased {branch} onto {onto}."));
    Ok(autostash_note(Step::Done(done), &out))
}

/// `cherry-pick [-m N] [--no-commit] <hash>`.
pub(crate) async fn cherry_pick(
    ctx: &OpCtx<'_>,
    hash: &str,
    mainline: Option<u32>,
    no_commit: bool,
) -> Result<Step> {
    let parent = mainline.map(|n| n.to_string());
    let mut args = vec!["cherry-pick"];
    if let Some(n) = &parent {
        args.extend(["-m", n]);
    }
    if no_commit {
        args.push("--no-commit");
    }
    args.push(hash);
    let into = current_branch(ctx).await?;
    let into = into.as_deref().unwrap_or("detached HEAD");
    if let Err(step) = git_may_stop(ctx, &args, false).await? {
        return Ok(step);
    }
    Ok(Step::Done(if no_commit {
        format!(
            "Applied the changes of {} to {into} without committing: they are staged.",
            short(hash)
        )
    } else {
        format!("Cherry-picked {} onto {into}.", short(hash))
    }))
}

/// `revert --no-edit [-m N] <hash>`.
pub(crate) async fn revert(ctx: &OpCtx<'_>, hash: &str, mainline: Option<u32>) -> Result<Step> {
    let parent = mainline.map(|n| n.to_string());
    let mut args = vec!["revert", "--no-edit"];
    if let Some(n) = &parent {
        args.extend(["-m", n]);
    }
    args.push(hash);
    if let Err(step) = git_may_stop(ctx, &args, false).await? {
        return Ok(step);
    }
    Ok(Step::Done(format!(
        "Reverted {}: a new commit undoes its changes.",
        short(hash)
    )))
}

/// `reset --soft|--mixed|--hard <hash>` (a hard one only once confirmed; see the
/// `hard_reset` danger).
pub(crate) async fn reset(ctx: &OpCtx<'_>, hash: &str, mode: ResetMode) -> Result<Step> {
    let (flag, kept) = match mode {
        ResetMode::Soft => ("--soft", "the changes since are staged"),
        ResetMode::Mixed => (
            "--mixed",
            "the changes since are in the working tree, unstaged",
        ),
        ResetMode::Hard => ("--hard", "uncommitted changes were discarded"),
    };
    let branch = current_branch(ctx).await?;
    let what = branch.as_deref().unwrap_or("HEAD");
    run(
        ctx,
        &["reset", flag, hash],
        format!("Reset {what} to {}: {kept}.", short(hash)),
    )
    .await
}

/// `tag -- <name> <hash>`, or with a message, an annotated tag:
/// `tag -a -m <message> -- <name> <hash>`.
pub(crate) async fn create_tag(
    ctx: &OpCtx<'_>,
    name: &str,
    hash: &str,
    message: Option<&str>,
) -> Result<Step> {
    let mut args = vec!["tag"];
    if let Some(message) = message {
        args.extend(["-a", "-m", message]);
    }
    args.extend(["--", name, hash]);
    run(
        ctx,
        &args,
        format!("Created tag {name} at {}.", short(hash)),
    )
    .await
}

/// `tag -d -- <name>`.
pub(crate) async fn delete_tag(ctx: &OpCtx<'_>, name: &str) -> Result<Step> {
    run(
        ctx,
        &["tag", "-d", "--", name],
        format!("Deleted tag {name}."),
    )
    .await
}
