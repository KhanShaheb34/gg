//! Network operations: push, pull, fetch into a local branch, deleting a branch or a tag on
//! a remote, and pushing a tag. Each runs git with `--progress` through
//! [`OpCtx::git_net`] (time limits, `op-progress` events) in the active worktree, and turns
//! how it ended into a [`Step`], with an explanation and follow-ups where the failure is
//! one the core recognises.
//!
//! Branches are given to git as full refs (`refs/heads/x:refs/heads/x`), so a name can never
//! be taken for an option or for something else. A force push is always
//! `--force-with-lease`: it is refused if the remote branch has moved since the last fetch.

use crate::dto::{OpHint, PullMode};
use crate::error::Result;
use crate::git::runner::GitOutput;
use crate::ops::explain::{Rejected, needs_pull_mode, nothing_to_do, push_rejected};
use crate::ops::local::{autostash_note, current_branch, git_may_stop, head_commit, short};
use crate::ops::{OpCtx, Step, explain};

/// A local branch's upstream, as configured.
pub(crate) struct Upstream {
    /// The remote's name ("." for the repository itself).
    pub remote: String,
    /// The branch on the remote, as a full ref (`refs/heads/x`).
    pub remote_ref: String,
    /// Its remote-tracking branch here, as a full ref (`refs/remotes/origin/x`).
    pub tracking: String,
    /// Its short name for messages (`origin/x`).
    pub short: String,
}

/// Where a push of a local branch to a remote goes.
pub(crate) struct PushTarget {
    /// The branch on the remote, as a full ref (`refs/heads/x`).
    pub remote_ref: String,
    /// Its remote-tracking branch here (`refs/remotes/origin/x`).
    pub tracking: String,
    /// Its short name for messages (`origin/x`).
    pub short: String,
}

/// Where pushing `branch` to `remote` goes (R12): to the branch's upstream when that is on
/// `remote` (whatever its name there), else to the branch of the same name.
pub(crate) async fn push_target(ctx: &OpCtx<'_>, branch: &str, remote: &str) -> Result<PushTarget> {
    if let Some(up) = upstream(ctx, branch).await?
        && up.remote == remote
        && up.remote_ref.starts_with("refs/heads/")
        && up.tracking.starts_with("refs/remotes/")
    {
        return Ok(PushTarget {
            remote_ref: up.remote_ref,
            tracking: up.tracking,
            short: up.short,
        });
    }
    Ok(PushTarget {
        remote_ref: format!("refs/heads/{branch}"),
        tracking: format!("refs/remotes/{remote}/{branch}"),
        short: format!("{remote}/{branch}"),
    })
}

/// The upstream of local branch `branch`, if one is configured.
pub(crate) async fn upstream(ctx: &OpCtx<'_>, branch: &str) -> Result<Option<Upstream>> {
    let full = format!("refs/heads/{branch}");
    let out = ctx
        .read(&[
            "for-each-ref",
            "--format=%(refname)%00%(upstream:remotename)%00%(upstream:remoteref)%00%(upstream)%00%(upstream:short)",
            &full,
        ])
        .await?;
    Ok(out.text().lines().find_map(|line| {
        let mut parts = line.split('\0');
        let (name, remote, remote_ref, tracking, short) = (
            parts.next()?,
            parts.next()?,
            parts.next()?,
            parts.next()?,
            parts.next()?,
        );
        (name == full && !remote.is_empty() && !remote_ref.is_empty()).then(|| Upstream {
            remote: remote.to_owned(),
            remote_ref: remote_ref.to_owned(),
            tracking: tracking.to_owned(),
            short: short.to_owned(),
        })
    }))
}

/// The repository's remotes, by name.
async fn remotes(ctx: &OpCtx<'_>) -> Result<Vec<String>> {
    let out = ctx.read(&["remote"]).await?;
    Ok(out
        .text()
        .lines()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Why `remote` cannot be used, if it is not one of the repository's remotes (a URL, a
/// path or `x:y` in its place): a plain line naming the remotes there are.
pub(crate) async fn unknown_remote(ctx: &OpCtx<'_>, remote: &str) -> Result<Option<String>> {
    let known = remotes(ctx).await?;
    if known.iter().any(|r| r == remote) {
        return Ok(None);
    }
    let there = if known.is_empty() {
        "it has none".to_owned()
    } else {
        format!("its remotes: {}", known.join(", "))
    };
    Ok(Some(format!(
        "\u{201c}{remote}\u{201d} is not a remote of this repository ({there})."
    )))
}

/// The commit `full` (a full ref) points at, if it exists.
pub(crate) async fn ref_commit(ctx: &OpCtx<'_>, full: &str) -> Result<Option<String>> {
    let out = ctx
        .read(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{full}^{{commit}}"),
        ])
        .await?;
    Ok((out.code == 0).then(|| out.text().trim().to_owned()))
}

/// Adds `hints` to a failed `step`.
fn with_hints(step: Step, hints: Vec<OpHint>) -> Step {
    match step {
        Step::Failed {
            message, detail, ..
        } => Step::Failed {
            message,
            detail,
            hints,
        },
        other => other,
    }
}

/// Whether git's push output says there was nothing to send.
fn pushed_nothing(out: &GitOutput) -> bool {
    out.stderr.contains("Everything up-to-date") || out.text().contains("Everything up-to-date")
}

/// `push --progress [--set-upstream] [--force-with-lease=<remote ref>:<lease>] <remote>
/// refs/heads/<branch>:<remote ref>`, to the branch's upstream on `remote` or the branch of
/// the same name (see [`push_target`]). A branch without an upstream gets one (the branch
/// on `remote`), whether or not `set_upstream` asks for it: a pushed branch should know
/// where it was pushed. `force` only once confirmed with a `lease` that is still the
/// remote-tracking commit (see the `force_push` danger); git then refuses unless the
/// remote branch is exactly there (no lease: unless it does not exist).
pub(crate) async fn push(
    ctx: &OpCtx<'_>,
    branch: &str,
    remote: &str,
    set_upstream: bool,
    force: bool,
    lease: Option<&str>,
) -> Result<Step> {
    let set_upstream = set_upstream || upstream(ctx, branch).await?.is_none();
    let target = push_target(ctx, branch, remote).await?;
    let refspec = format!("refs/heads/{branch}:{}", target.remote_ref);
    let lease_arg = format!(
        "--force-with-lease={}:{}",
        target.remote_ref,
        lease.unwrap_or("")
    );
    let mut args = vec!["push", "--progress"];
    if set_upstream {
        args.push("--set-upstream");
    }
    if force {
        args.push(&lease_arg);
    }
    args.extend([remote, refspec.as_str()]);
    let to = if target.remote_ref == format!("refs/heads/{branch}") {
        remote.to_owned()
    } else {
        target.short.clone()
    };
    let out = ctx.git_net(&args).await?;
    if out.code != 0 {
        let all = format!("{}\n{}", out.stderr, out.text());
        let hints = match push_rejected(&all) {
            Some(Rejected::RemoteAhead) => vec![
                OpHint::Fetch,
                OpHint::Pull {
                    branch: branch.to_owned(),
                },
                OpHint::ForcePush {
                    branch: branch.to_owned(),
                    remote: remote.to_owned(),
                },
            ],
            Some(Rejected::StaleInfo) => vec![OpHint::Fetch],
            _ => Vec::new(),
        };
        return Ok(with_hints(explain(&args, &out), hints));
    }
    if pushed_nothing(&out) {
        return Ok(Step::Done(format!(
            "Already up to date: {to} has every commit of {branch}."
        )));
    }
    let what = if force { "Force-pushed" } else { "Pushed" };
    Ok(Step::Done(if set_upstream {
        format!(
            "{what} {branch} to {to}, and set {} as its upstream.",
            target.short
        )
    } else {
        format!("{what} {branch} to {to}.")
    }))
}

/// Pull `branch`. Checked out here: `pull --progress --no-edit`, with `--no-rebase` to
/// merge, `--rebase` to rebase, or neither to do what the user's config says; it can stop
/// on conflicts. Not checked out: `fetch --progress <remote> <remote ref>:refs/heads/<branch>`
/// from its upstream, which only fast-forwards it.
pub(crate) async fn pull(ctx: &OpCtx<'_>, branch: &str, mode: PullMode) -> Result<Step> {
    if current_branch(ctx).await?.as_deref() == Some(branch) {
        pull_current(ctx, branch, mode).await
    } else {
        pull_other(ctx, branch).await
    }
}

async fn pull_current(ctx: &OpCtx<'_>, branch: &str, mode: PullMode) -> Result<Step> {
    let mut args = vec!["pull", "--progress", "--no-edit"];
    match mode {
        PullMode::Default => {}
        PullMode::Merge => args.push("--no-rebase"),
        PullMode::Rebase => args.push("--rebase"),
    }
    let from = upstream(ctx, branch).await?.map(|u| u.short);
    let out = match git_may_stop(ctx, &args, true).await? {
        Ok(out) => out,
        Err(step) => {
            let all = match &step {
                Step::Failed {
                    message, detail, ..
                } => format!("{message}\n{detail}"),
                _ => String::new(),
            };
            let hints = if needs_pull_mode(&all) {
                vec![OpHint::ChoosePullMode {
                    branch: branch.to_owned(),
                }]
            } else {
                Vec::new()
            };
            return Ok(with_hints(step, hints));
        }
    };
    if nothing_to_do(&args, &out).is_some() {
        return Ok(autostash_note(
            Step::Done(format!(
                "Already up to date: {branch} has every commit of {}.",
                from.as_deref().unwrap_or("its upstream")
            )),
            &out,
        ));
    }
    Ok(autostash_note(
        Step::Done(match from {
            Some(from) => format!("Pulled {from} into {branch}."),
            None => format!("Pulled into {branch}."),
        }),
        &out,
    ))
}

async fn pull_other(ctx: &OpCtx<'_>, branch: &str) -> Result<Step> {
    let Some(up) = upstream(ctx, branch).await? else {
        return Ok(no_upstream(branch));
    };
    let into = format!("refs/heads/{branch}");
    let refspec = format!("{}:{into}", up.remote_ref);
    let args = ["fetch", "--progress", up.remote.as_str(), refspec.as_str()];
    let before = ref_commit(ctx, &into).await?;
    let out = ctx.git_net(&args).await?;
    if out.code != 0 {
        return Ok(explain(&args, &out));
    }
    let after = ref_commit(ctx, &into).await?;
    Ok(Step::Done(if before == after {
        format!(
            "Already up to date: {branch} has every commit of {}.",
            up.short
        )
    } else {
        format!(
            "Fast-forwarded {branch} to {} ({}), without checking it out.",
            up.short,
            after.as_deref().map_or("", short)
        )
    }))
}

/// The failure of pulling `branch`, which has no upstream.
fn no_upstream(branch: &str) -> Step {
    Step::Failed {
        message: format!(
            "Branch \u{201c}{branch}\u{201d} has no upstream branch, so there is nothing to \
             pull from: push it with an upstream first, or set one."
        ),
        detail: format!("Branch \u{201c}{branch}\u{201d} has no upstream configured.\n"),
        hints: Vec::new(),
    }
}

/// The remote of the remote-tracking branch `remote_branch` (`origin/x`) and the branch's
/// name there (`x`): the longest remote name that is a prefix. `None` if no remote is.
async fn split_remote_branch(
    ctx: &OpCtx<'_>,
    remote_branch: &str,
) -> Result<Option<(String, String)>> {
    Ok(remotes(ctx)
        .await?
        .iter()
        .filter_map(|remote| {
            let rest = remote_branch.strip_prefix(remote)?.strip_prefix('/')?;
            (!remote.is_empty() && !rest.is_empty()).then(|| (remote.to_owned(), rest.to_owned()))
        })
        .max_by_key(|(remote, _)| remote.len()))
}

/// `fetch --progress <remote> refs/heads/<branch>:refs/heads/<local>` for `remote_branch`
/// (`<remote>/<branch>`): creates `local` there, or fast-forwards it; never moves it
/// otherwise, and git refuses for a checked-out branch.
pub(crate) async fn fetch_into_local(
    ctx: &OpCtx<'_>,
    remote_branch: &str,
    local: &str,
) -> Result<Step> {
    let Some((remote, branch)) = split_remote_branch(ctx, remote_branch).await? else {
        return Ok(Step::Failed {
            message: format!(
                "\u{201c}{remote_branch}\u{201d} does not name a branch of any remote of this \
                 repository."
            ),
            detail: format!("No remote's name is a prefix of {remote_branch}.\n"),
            hints: Vec::new(),
        });
    };
    crate::git::names::check_branch_name(&branch)?;
    let into = format!("refs/heads/{local}");
    let refspec = format!("refs/heads/{branch}:{into}");
    let args = ["fetch", "--progress", remote.as_str(), refspec.as_str()];
    let before = ref_commit(ctx, &into).await?;
    let out = ctx.git_net(&args).await?;
    if out.code != 0 {
        return Ok(explain(&args, &out));
    }
    let after = ref_commit(ctx, &into).await?;
    let at = after.as_deref().map_or("", short);
    Ok(Step::Done(match before {
        None => format!("Created branch {local} at {remote_branch} ({at})."),
        Some(b) if Some(&b) == after.as_ref() => {
            format!("Already up to date: {local} has every commit of {remote_branch}.")
        }
        Some(_) => format!("Fast-forwarded {local} to {remote_branch} ({at})."),
    }))
}

/// `push --progress <remote> --delete refs/heads/<branch>` (only once confirmed; see the
/// `delete_remote_branch` danger). The local branch of that name is left alone.
pub(crate) async fn delete_remote_branch(
    ctx: &OpCtx<'_>,
    remote: &str,
    branch: &str,
) -> Result<Step> {
    let full = format!("refs/heads/{branch}");
    let args = ["push", "--progress", remote, "--delete", full.as_str()];
    let out = ctx.git_net(&args).await?;
    Ok(if out.code == 0 {
        Step::Done(format!("Deleted branch {branch} on {remote}."))
    } else {
        explain(&args, &out)
    })
}

/// `push --progress <remote> refs/tags/<name>`.
pub(crate) async fn push_tag(ctx: &OpCtx<'_>, name: &str, remote: &str) -> Result<Step> {
    let full = format!("refs/tags/{name}");
    let args = ["push", "--progress", remote, full.as_str()];
    let out = ctx.git_net(&args).await?;
    Ok(if out.code != 0 {
        explain(&args, &out)
    } else if pushed_nothing(&out) {
        Step::Done(format!("Already up to date: {remote} has tag {name}."))
    } else {
        Step::Done(format!("Pushed tag {name} to {remote}."))
    })
}

/// Deletes tag `name` on `remote` (`push --progress <remote> --delete refs/tags/<name>`),
/// then here (`tag -d -- <name>`). The remote goes first, so that if it cannot be reached
/// nothing has changed; a remote that does not have the tag is fine.
pub(crate) async fn delete_tag_everywhere(
    ctx: &OpCtx<'_>,
    name: &str,
    remote: &str,
) -> Result<Step> {
    let full = format!("refs/tags/{name}");
    let args = ["push", "--progress", remote, "--delete", full.as_str()];
    let out = ctx.git_net(&args).await?;
    let all = format!("{}\n{}", out.stderr, out.text());
    // "remote ref does not exist" (a failure), or a warning from a remote that deletes
    // nothing and reports success (git's own receive-pack).
    let absent =
        all.contains("remote ref does not exist") || all.contains("deleting a non-existent ref");
    if out.code != 0 && !absent {
        return Ok(explain(&args, &out));
    }
    let local = ["tag", "-d", "--", name];
    let out = ctx.git(&local).await?;
    if out.code != 0 {
        return Ok(explain(&local, &out));
    }
    Ok(Step::Done(if absent {
        format!("Deleted tag {name} ({remote} did not have it).")
    } else {
        format!("Deleted tag {name}, here and on {remote}.")
    }))
}

/// After checking out `branch`: `merge --ff-only @{upstream}`, to bring it up to its
/// upstream as last fetched. Always done (the checkout happened); the message says whether
/// it moved, was up to date, could not move or has no upstream. `checked_out` is the
/// checkout's own message.
pub(crate) async fn fast_forward_checked_out(
    ctx: &OpCtx<'_>,
    branch: &str,
    checked_out: String,
) -> Result<Step> {
    let Some(up) = upstream(ctx, branch).await? else {
        return Ok(Step::Done(format!(
            "{checked_out} It has no upstream branch, so there was nothing to fast-forward to."
        )));
    };
    let args = ["merge", "--ff-only", "@{upstream}"];
    let before = head_commit(ctx).await?;
    let out = ctx.git(&args).await?;
    if out.code != 0 {
        let all = format!("{}\n{}", out.stderr, out.text());
        let why = if all.contains("Not possible to fast-forward") {
            format!(
                "{branch} has commits of its own that {} does not have",
                up.short
            )
        } else {
            match explain(&args, &out) {
                Step::Failed { message, .. } => message.trim_end_matches('.').to_owned(),
                _ => "git refused".to_owned(),
            }
        };
        return Ok(Step::DoneWith {
            message: format!(
                "{checked_out} It could not be fast-forwarded to {}: {why}.",
                up.short
            ),
            detail: Some(all.trim().to_owned()),
            hints: Vec::new(),
        });
    }
    Ok(Step::Done(if head_commit(ctx).await? == before {
        format!("{checked_out} It is up to date with {}.", up.short)
    } else {
        format!("{checked_out} It was fast-forwarded to {}.", up.short)
    }))
}
