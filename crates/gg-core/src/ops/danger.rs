//! Which dangers a request carries, given the repository's state: the paths that cannot
//! run unconfirmed (force push, hard reset, deleting an unmerged branch, deleting a remote
//! branch, dropping a stash). Checked by the core, in the operation's turn, so the state
//! looked at is the state the operation runs on.

use crate::dto::{Danger, DangerKind, OpRequest};
use crate::error::Result;
use crate::ops::OpCtx;

/// The dangers `request` would run into now.
pub(crate) async fn dangers(ctx: &OpCtx<'_>, request: &OpRequest) -> Result<Vec<Danger>> {
    Ok(match request {
        OpRequest::DeleteBranch { branch } => {
            unmerged_branch(ctx, branch).await?.into_iter().collect()
        }
        // The others carry none (the network and stash ones are added with them).
        _ => Vec::new(),
    })
}

/// `delete_unmerged_branch` if `branch` exists and is an ancestor neither of HEAD nor of
/// its upstream: deleting it would lose the commits only it has, which the message counts.
async fn unmerged_branch(ctx: &OpCtx<'_>, branch: &str) -> Result<Option<Danger>> {
    let full = format!("refs/heads/{branch}");
    if !verifies(ctx, &full).await? {
        // git says it does not exist.
        return Ok(None);
    }
    let mut tips: Vec<String> = Vec::new();
    if verifies(ctx, "HEAD").await? {
        tips.push("HEAD".to_owned());
    }
    let upstream = upstream_of(ctx, &full).await?;
    if let Some(up) = &upstream
        && verifies(ctx, up).await?
    {
        tips.push(up.clone());
    }
    for tip in &tips {
        let merged = ctx
            .read(&["merge-base", "--is-ancestor", &full, tip])
            .await?;
        if merged.code == 0 {
            return Ok(None);
        }
    }
    let mut args = vec!["rev-list", "--count", full.as_str()];
    if !tips.is_empty() {
        args.push("--not");
        args.extend(tips.iter().map(String::as_str));
    }
    let count = ctx.read(&args).await?;
    let n: u64 = count.text().trim().parse().unwrap_or(0);
    let into = if upstream.is_some() {
        "HEAD or its upstream branch"
    } else {
        "HEAD"
    };
    let (commits, them) = if n == 1 {
        ("1 commit".to_owned(), "it")
    } else {
        (format!("{n} commits"), "them")
    };
    Ok(Some(Danger {
        kind: DangerKind::DeleteUnmergedBranch,
        message: format!(
            "Branch \u{201c}{branch}\u{201d} has {commits} not merged into {into}: deleting \
             it loses {them}."
        ),
        detail: None,
    }))
}

/// Whether `rev` names a commit.
async fn verifies(ctx: &OpCtx<'_>, rev: &str) -> Result<bool> {
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

/// The full name of the upstream of local branch `full` (`refs/heads/x`), if it has one.
async fn upstream_of(ctx: &OpCtx<'_>, full: &str) -> Result<Option<String>> {
    let out = ctx
        .read(&["for-each-ref", "--format=%(refname)%00%(upstream)", full])
        .await?;
    Ok(out.text().lines().find_map(|line| {
        let (name, upstream) = line.split_once('\0')?;
        (name == full && !upstream.is_empty()).then(|| upstream.to_owned())
    }))
}
