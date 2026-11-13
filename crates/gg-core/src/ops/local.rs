//! Local operations: checkout, branches, merge, rebase, cherry-pick, revert, reset and
//! tags. Each body runs git in the active worktree and turns how it ended into a [`Step`]:
//! done, stopped on conflicts, or failed (with an explanation where one is known).

use crate::dto::{Danger, DangerKind};
use crate::error::{CoreError, ErrorCode, Result};
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
