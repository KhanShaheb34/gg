//! Which dangers a request carries, given the repository's state: the paths that cannot
//! run unconfirmed (force push, hard reset, deleting an unmerged branch, deleting a remote
//! branch, dropping a stash, aborting what git is stopped in). Checked by the core, in the
//! operation's turn, so the state looked at is the state the operation runs on. Each danger
//! carries a token of that state (R39): a confirmation counts only with the same token.

use crate::dto::{Danger, DangerKind, InProgressKind, OpRequest, ResetMode};
use crate::error::Result;
use crate::git::progress::state_files;
use crate::git::status::read_status;
use crate::ops::OpCtx;
use crate::ops::local::{current_branch, head_commit, kind_name};
use crate::ops::network::{push_target, ref_commit};
use crate::ops::progress::unmerged_paths;
use crate::ops::stash;

/// The dangers `request` would run into now.
pub(crate) async fn dangers(ctx: &OpCtx<'_>, request: &OpRequest) -> Result<Vec<Danger>> {
    Ok(match request {
        OpRequest::DeleteBranch { branch } => {
            unmerged_branch(ctx, branch).await?.into_iter().collect()
        }
        OpRequest::Reset {
            hash,
            mode: ResetMode::Hard,
            ..
        } => vec![hard_reset(ctx, hash).await?],
        OpRequest::Push {
            branch,
            remote,
            to,
            force: true,
            ..
        } => vec![force_push(ctx, branch, remote, to.as_deref()).await?],
        OpRequest::DeleteRemoteBranch { remote, branch } => {
            vec![remote_branch_deleted(ctx, remote, branch).await?]
        }
        OpRequest::StashDrop { stash } => stash::dropped(ctx, stash).await?,
        OpRequest::Abort { .. } => aborted(ctx).await?.into_iter().collect(),
        OpRequest::Skip { .. } => skip_discards(ctx).await?.into_iter().collect(),
        // The others carry none.
        _ => Vec::new(),
    })
}

/// `delete_unmerged_branch` exactly when `branch -d` would refuse: git compares the branch
/// with its upstream when it has one (and that exists), else with HEAD, and refuses unless
/// the branch is contained in it. So a branch merged into its upstream deletes without
/// confirmation even if HEAD lacks it, and one merged into HEAD but not into its upstream
/// needs confirmation (then `-D`). The message counts the commits only the branch has,
/// relative to that reference, and says when the current branch has them nevertheless.
async fn unmerged_branch(ctx: &OpCtx<'_>, branch: &str) -> Result<Option<Danger>> {
    let full = format!("refs/heads/{branch}");
    if !verifies(ctx, &full).await? {
        // git says it does not exist.
        return Ok(None);
    }
    let upstream = match upstream_of(ctx, &full).await? {
        Some(up) if verifies(ctx, &up).await? => Some(up),
        _ => None,
    };
    let head = verifies(ctx, "HEAD").await?;
    let reference = upstream.clone().or_else(|| head.then(|| "HEAD".to_owned()));
    if let Some(reference) = &reference {
        let merged = ctx
            .read(&["merge-base", "--is-ancestor", &full, reference])
            .await?;
        if merged.code == 0 {
            return Ok(None);
        }
    }
    let mut args = vec!["rev-list", "--count", full.as_str()];
    if let Some(reference) = &reference {
        args.extend(["--not", reference.as_str()]);
    }
    let count = ctx.read(&args).await?;
    let n: u64 = count.text().trim().parse().unwrap_or(0);
    let (commits, them) = if n == 1 {
        ("1 commit".to_owned(), "it")
    } else {
        (format!("{n} commits"), "them")
    };
    let current = current_branch(ctx).await?;
    let message = match (&upstream, &current) {
        (Some(up), current) => {
            let up = short_ref(up);
            let in_head = head
                && ctx
                    .read(&["merge-base", "--is-ancestor", &full, "HEAD"])
                    .await?
                    .code
                    == 0;
            match current {
                Some(current) if in_head && current != branch => format!(
                    "Branch \u{201c}{branch}\u{201d} has {commits} not in its upstream branch \
                     {up}: its commits are in {current}, but not yet in {up}."
                ),
                _ => format!(
                    "Branch \u{201c}{branch}\u{201d} has {commits} not in its upstream branch \
                     {up}: deleting it loses {them}."
                ),
            }
        }
        (None, _) if !head => format!(
            "Branch \u{201c}{branch}\u{201d} has {commits}, and no commit is checked out to \
             compare it with: deleting it loses {them}."
        ),
        (None, current) => {
            let into = current.as_deref().unwrap_or("HEAD");
            format!(
                "Branch \u{201c}{branch}\u{201d} has {commits} not merged into {into}: deleting \
                 it loses {them}."
            )
        }
    };
    Ok(Some(Danger {
        kind: DangerKind::DeleteUnmergedBranch,
        message,
        detail: None,
        // The tip the count was made from: a branch that grew since is asked about again.
        token: ref_commit(ctx, &full).await?,
    }))
}

/// Most commits a danger's detail lists.
const MAX_COMMITS_LISTED: &str = "10";

/// `force_push`, always: it replaces the push's target on the remote (see
/// [`push_target`]) with the local branch. The message counts the commits there that the
/// local branch lacks, as of the last fetch, which a force push discards (the detail lists
/// them). Its token is that remote-tracking commit: the push runs only if it is confirmed
/// with that token, and git refuses unless the remote branch is still there.
async fn force_push(
    ctx: &OpCtx<'_>,
    branch: &str,
    remote: &str,
    to: Option<&str>,
) -> Result<Danger> {
    let target = push_target(branch, remote, to);
    let tracking = target.tracking;
    let local = format!("refs/heads/{branch}");
    let shown = target.short;
    let Some(token) = ref_commit(ctx, &tracking).await? else {
        let there = target
            .remote_ref
            .strip_prefix("refs/heads/")
            .unwrap_or(&target.remote_ref);
        return Ok(Danger {
            kind: DangerKind::ForcePush,
            message: format!(
                "A force push replaces {there} on {remote} with {branch}. It has not been \
                 fetched, so what it would discard is not known here; if it exists there, the \
                 push is refused."
            ),
            detail: None,
            token: None,
        });
    };
    // From the token's commit, not the ref: a fetch could move it between these reads.
    let mut only_there = vec![token.as_str()];
    if verifies(ctx, &local).await? {
        only_there.extend(["--not", local.as_str()]);
    }
    let mut count = vec!["rev-list", "--count"];
    count.extend(&only_there);
    let n: u64 = ctx.read(&count).await?.text().trim().parse().unwrap_or(0);
    let message = match n {
        0 => format!(
            "A force push replaces {shown} with {branch}. As of the last fetch it has no \
             commits that {branch} lacks, so nothing is discarded; if it has changed since, \
             the push is refused."
        ),
        1 => format!(
            "A force push replaces {shown} with {branch}, discarding 1 commit there that \
             {branch} does not have (as of the last fetch)."
        ),
        n => format!(
            "A force push replaces {shown} with {branch}, discarding {n} commits there that \
             {branch} does not have (as of the last fetch)."
        ),
    };
    let detail = if n == 0 {
        None
    } else {
        let mut log = vec![
            "log",
            "--abbrev=8",
            "--format=%h %s",
            "-n",
            MAX_COMMITS_LISTED,
        ];
        log.extend(&only_there);
        let listed = ctx.read(&log).await?.text();
        Some(listed).filter(|l| !l.trim().is_empty())
    };
    Ok(Danger {
        kind: DangerKind::ForcePush,
        message,
        detail,
        token: Some(token),
    })
}

/// `delete_remote_branch`, always: the branch goes for everyone who uses the remote. The
/// message says how many of its commits (as last fetched) are on no local branch here, and
/// whether it is the remote's default branch. Its token is the remote-tracking commit the
/// count was made from (none if it was never fetched): the delete is a lease on it, so git
/// refuses it if someone pushed to the branch since the last fetch (I1).
async fn remote_branch_deleted(ctx: &OpCtx<'_>, remote: &str, branch: &str) -> Result<Danger> {
    let tracking = format!("refs/remotes/{remote}/{branch}");
    let gone = format!(
        "Deleting branch \u{201c}{branch}\u{201d} on {remote} removes it there, for everyone \
         who uses {remote}."
    );
    let token = ref_commit(ctx, &tracking).await?;
    let message = if let Some(seen) = &token {
        let out = ctx
            .read(&["rev-list", "--count", seen, "--not", "--branches"])
            .await?;
        match out.text().trim().parse::<u64>().unwrap_or(0) {
            0 => format!("{gone} Its commits are all on local branches here."),
            1 => {
                format!("{gone} 1 of its commits is on no local branch here, and is lost here too.")
            }
            n => format!(
                "{gone} {n} of its commits are on no local branch here, and are lost here too."
            ),
        }
    } else {
        format!(
            "{gone} It has not been fetched, so its commits are not known here; if it is \
             there, git refuses to delete it until it is fetched."
        )
    };
    // `<remote>/HEAD`, as last learnt (at clone, or `remote set-head`), names the remote's
    // default branch.
    let head = ctx
        .read(&["symbolic-ref", "-q", &format!("refs/remotes/{remote}/HEAD")])
        .await?;
    let message = if head.code == 0 && head.text().trim() == tracking {
        format!(
            "{message} It is {remote}'s default branch: clones, and the remote's own pages, \
             expect it to be there."
        )
    } else {
        message
    };
    Ok(Danger {
        kind: DangerKind::DeleteRemoteBranch,
        message,
        detail: None,
        token,
    })
}

/// `refs/remotes/origin/x` as `origin/x`, `refs/heads/x` as `x`.
fn short_ref(full: &str) -> &str {
    full.strip_prefix("refs/remotes/")
        .or_else(|| full.strip_prefix("refs/heads/"))
        .unwrap_or(full)
}

/// Most files a hard reset's confirmation lists; the rest are counted.
const MAX_LOST_LISTED: usize = 20;

/// `hard_reset`, always: it discards the uncommitted changes, which the message counts and
/// the detail lists (MAX_LOST_LISTED files, then how many more); untracked files are kept,
/// which it says too. Its token is HEAD (branch and commit) and the files that differ from
/// it: confirmed for another branch, another commit or other changes, it is asked again.
async fn hard_reset(ctx: &OpCtx<'_>, hash: &str) -> Result<Danger> {
    let status = read_status(&ctx.runner, &ctx.cwd).await?;
    let to = crate::ops::local::short(hash);
    let lost = match status.changed {
        0 => format!(
            "A hard reset to {to} moves HEAD there; there are no uncommitted changes to lose."
        ),
        1 => format!("A hard reset to {to} discards the uncommitted changes to 1 file."),
        n => format!("A hard reset to {to} discards the uncommitted changes to {n} files."),
    };
    let kept = match status.untracked {
        0 => "Untracked files are kept: git leaves them alone.".to_owned(),
        1 => "1 untracked file is kept: git leaves untracked files alone.".to_owned(),
        n => format!("{n} untracked files are kept: git leaves untracked files alone."),
    };
    // The tracked files that differ from HEAD, staged or not: what the reset puts back.
    let out = ctx
        .read(&["diff", "--name-only", "-z", "HEAD", "--"])
        .await?;
    let text = out.text();
    let files: Vec<&str> = text.split('\0').filter(|f| !f.is_empty()).collect();
    let detail = (out.code == 0 && !files.is_empty()).then(|| {
        let mut lines: Vec<String> = files
            .iter()
            .take(MAX_LOST_LISTED)
            .map(|f| (*f).to_owned())
            .collect();
        if files.len() > MAX_LOST_LISTED {
            lines.push(format!("and {} more", files.len() - MAX_LOST_LISTED));
        }
        lines.join("\n")
    });
    let token = format!("{}\n{:016x}", head_token(ctx).await?, fnv1a(&out.stdout));
    Ok(Danger {
        kind: DangerKind::HardReset,
        message: format!("{lost} {kept}"),
        detail,
        token: Some(token),
    })
}

/// `abort`, when git is stopped in something: aborting puts back what it changed, and the
/// changes made to resolve its conflicts with it. Its token is what is stopped and HEAD:
/// confirmed for one stop, it does not abort another.
async fn aborted(ctx: &OpCtx<'_>) -> Result<Option<Danger>> {
    let Some(files) = state_files(&ctx.runner, &ctx.cwd).await? else {
        return Ok(None);
    };
    let Some(kind) = files.kind() else {
        return Ok(None);
    };
    let what = kind_name(kind);
    let branch = current_branch(ctx).await?;
    let message = match kind {
        InProgressKind::Rebase => {
            let who = files.rebase_head_name().map_or_else(
                || "the branch".to_owned(),
                |b| format!("\u{201c}{b}\u{201d}"),
            );
            format!(
                "Aborting the rebase puts {who} back as it was before it: the commits it has \
                 made so far, and the changes made to resolve its conflicts, are lost."
            )
        }
        _ => {
            let who = branch
                .as_deref()
                .map_or_else(|| "HEAD".to_owned(), |b| format!("\u{201c}{b}\u{201d}"));
            format!(
                "Aborting the {what} puts {who} back as it was before it: the changes made to \
                 resolve its conflicts are lost."
            )
        }
    };
    Ok(Some(Danger {
        kind: DangerKind::Abort,
        message,
        detail: None,
        token: Some(format!("{what}\n{}", head_token(ctx).await?)),
    }))
}

/// `skip_discards`, for a rebase only: `rebase --skip` puts every tracked file back as HEAD
/// has it (a cherry-pick's or revert's skip keeps the changes it does not touch). So changes
/// beyond the stopped step's conflicted files (an edit made after continuing came out empty,
/// say) go with it: the message counts them, the detail lists them, and they are confirmed
/// first (R40). Its token is HEAD and those files.
async fn skip_discards(ctx: &OpCtx<'_>) -> Result<Option<Danger>> {
    let kind = state_files(&ctx.runner, &ctx.cwd)
        .await?
        .and_then(|files| files.kind());
    if kind != Some(InProgressKind::Rebase) {
        return Ok(None);
    }
    let out = ctx
        .read(&["diff", "--name-only", "-z", "HEAD", "--"])
        .await?;
    if out.code != 0 {
        return Ok(None);
    }
    let unmerged = unmerged_paths(ctx).await?;
    let text = out.text();
    let files: Vec<&str> = text
        .split('\0')
        .filter(|f| !f.is_empty() && !unmerged.iter().any(|u| u == f))
        .collect();
    let message = match files.len() {
        0 => return Ok(None),
        1 => "Skipping this commit puts every tracked file back as HEAD has it (git does that \
              for a rebase): the uncommitted changes to 1 file are discarded too."
            .to_owned(),
        n => format!(
            "Skipping this commit puts every tracked file back as HEAD has it (git does that \
             for a rebase): the uncommitted changes to {n} files are discarded too."
        ),
    };
    let mut lines: Vec<String> = files
        .iter()
        .take(MAX_LOST_LISTED)
        .map(|f| (*f).to_owned())
        .collect();
    if files.len() > MAX_LOST_LISTED {
        lines.push(format!("and {} more", files.len() - MAX_LOST_LISTED));
    }
    Ok(Some(Danger {
        kind: DangerKind::SkipDiscards,
        message,
        detail: Some(lines.join("\n")),
        token: Some(format!(
            "{}\n{:016x}",
            head_token(ctx).await?,
            fnv1a(files.join("\0").as_bytes())
        )),
    }))
}

/// HEAD as a token: the branch it is on (empty when detached) and its commit (empty before
/// the first).
async fn head_token(ctx: &OpCtx<'_>) -> Result<String> {
    Ok(format!(
        "{}\n{}",
        current_branch(ctx).await?.unwrap_or_default(),
        head_commit(ctx).await?.unwrap_or_default()
    ))
}

/// The 64-bit FNV-1a hash of `bytes`: a short, stable digest for a token.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
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
