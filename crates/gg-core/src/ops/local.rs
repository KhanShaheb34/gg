//! Local operations: checkout, branches, merge, rebase, cherry-pick, revert, reset and
//! tags. Each body runs git in the active worktree and turns how it ended into a [`Step`]:
//! done, stopped on conflicts, or failed (with an explanation where one is known).

use std::path::PathBuf;

use crate::dto::{
    Danger, DangerKind, InProgressKind, MergeTarget, MergeTargetKind, OpHint, ResetMode,
};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::runner::GitOutput;
use crate::ops::explain::{NO_BRANCH, nothing_to_do};
use crate::ops::{OpCtx, Step, explain};

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

/// `checkout <branch> --`.
pub(crate) async fn checkout_branch(ctx: &OpCtx<'_>, branch: &str) -> Result<Step> {
    run(
        ctx,
        &["checkout", branch, "--"],
        format!("Checked out {branch}."),
    )
    .await
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

/// What git is stopped in, in the operation's worktree, if anything: a rebase
/// (`rebase-merge`, `rebase-apply`; it wins when several are there), a merge (`MERGE_HEAD`),
/// a cherry-pick (`CHERRY_PICK_HEAD`) or a revert (`REVERT_HEAD`). `None` too if the git dir
/// cannot be found.
pub(crate) async fn stopped_kind(ctx: &OpCtx<'_>) -> Option<InProgressKind> {
    let out = ctx.read(&["rev-parse", "--absolute-git-dir"]).await.ok()?;
    if out.code != 0 {
        return None;
    }
    let dir = PathBuf::from(out.text().trim_end_matches(['\n', '\r']));
    let has = |name: &str| dir.join(name).exists();
    if has("rebase-merge") || has("rebase-apply") {
        Some(InProgressKind::Rebase)
    } else if has("MERGE_HEAD") {
        Some(InProgressKind::Merge)
    } else if has("CHERRY_PICK_HEAD") {
        Some(InProgressKind::CherryPick)
    } else if has("REVERT_HEAD") {
        Some(InProgressKind::Revert)
    } else {
        None
    }
}

/// Runs `args`, a command that can stop on conflicts. `Ok(out)` if git exits 0; else the
/// step it ended with: done if there was nothing to do, stopped if git is now in a state it
/// was not in before (conflicts to resolve), else failed.
async fn git_may_stop(
    ctx: &OpCtx<'_>,
    args: &[&str],
) -> Result<std::result::Result<GitOutput, Step>> {
    let before = stopped_kind(ctx).await;
    let out = ctx.git(args).await?;
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
    Ok(Err(match after {
        Some(kind) => Step::Stopped(stopped_message(kind)),
        None => step,
    }))
}

/// The plain line for git having stopped a `kind` on conflicts.
fn stopped_message(kind: InProgressKind) -> String {
    let what = match kind {
        InProgressKind::Merge => "merge",
        InProgressKind::Rebase => "rebase",
        InProgressKind::CherryPick => "cherry-pick",
        InProgressKind::Revert => "revert",
    };
    format!("The {what} stopped on conflicts. Resolve them, then continue the {what}, or abort it.")
}

/// The branch HEAD is on, or `None` when detached (or unborn and unreadable).
async fn current_branch(ctx: &OpCtx<'_>) -> Result<Option<String>> {
    let out = ctx.read(&["symbolic-ref", "-q", "--short", "HEAD"]).await?;
    Ok((out.code == 0)
        .then(|| out.text().trim().to_owned())
        .filter(|b| !b.is_empty()))
}

/// The commit HEAD points at, if any.
async fn head_commit(ctx: &OpCtx<'_>) -> Result<Option<String>> {
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
/// with `--ff`: git refuses it with `--no-ff`, which `merge.ff` may set. After a merge that
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
    args.push(if no_ff && !squash { "--no-ff" } else { "--ff" });
    if squash {
        args.push("--squash");
    }
    if no_commit {
        args.push("--no-commit");
    }
    args.push(name);

    let branch = current_branch(ctx).await?;
    let before = head_commit(ctx).await?;
    let out = match git_may_stop(ctx, &args).await? {
        Ok(out) => out,
        Err(step) => return Ok(step),
    };
    if let Some(message) = nothing_to_do(&args, &out) {
        return Ok(Step::Done(message));
    }
    let into = branch.as_deref().unwrap_or("detached HEAD");
    if squash {
        return Ok(Step::Done(format!(
            "Squashed {shown} into staged changes on {into}: commit them to finish."
        )));
    }
    if stopped_kind(ctx).await == Some(InProgressKind::Merge) {
        return Ok(Step::Done(format!(
            "Merged {shown} into {into} without committing: the result is staged; commit it \
             to finish the merge."
        )));
    }
    let hints = match &branch {
        Some(b) if head_commit(ctx).await? != before => {
            push_hint(ctx, b).await?.into_iter().collect()
        }
        _ => Vec::new(),
    };
    let message = format!("Merged {shown} into {into}.");
    Ok(if hints.is_empty() {
        Step::Done(message)
    } else {
        Step::DoneWithHints { message, hints }
    })
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
    let out = match git_may_stop(ctx, &args).await? {
        Ok(out) => out,
        Err(step) => return Ok(step),
    };
    Ok(Step::Done(nothing_to_do(&args, &out).unwrap_or_else(
        || format!("Rebased {branch} onto {onto}."),
    )))
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
    if let Err(step) = git_may_stop(ctx, &args).await? {
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
    if let Err(step) = git_may_stop(ctx, &args).await? {
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
