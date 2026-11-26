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
use crate::notice::{name, said};
use crate::ops::explain::{Rejected, needs_pull_mode, nothing_to_do, push_rejected};
use crate::ops::local::{
    autostash_note, current_branch, git_may_stop, head_commit, kind_name, short, stash_top,
    stopped_kind,
};
use crate::ops::{OpCtx, Step, explain};

/// Where `branch` is on `remote` for a notice, `short` its remote-tracking name there
/// (`origin/x`): the remote's name alone when it is the branch of the same name, else that
/// branch, set off.
pub(crate) fn where_on(branch: &str, remote: &str, short: &str) -> String {
    if short == format!("{remote}/{branch}") {
        remote.to_owned()
    } else {
        name(short)
    }
}

/// `remote` of an upstream's short name `short` (`origin/x`), when the upstream is known;
/// for a notice (see [`where_on`]).
fn upstream_where(branch: &str, up: &Upstream) -> String {
    if up.remote == "." {
        name(&up.short)
    } else {
        where_on(branch, &up.remote, &up.short)
    }
}

/// A local branch's upstream, as configured.
pub(crate) struct Upstream {
    /// The remote's name ("." for the repository itself).
    pub remote: String,
    /// The branch on the remote, as a full ref (`refs/heads/x`).
    pub remote_ref: String,
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

/// Where pushing `branch` to `remote` goes: the branch of the same name there, or
/// `to`, a branch of another name there (only the branch's upstream: see
/// [`not_its_upstream`]). Never to an upstream the user did not name: git's own default
/// (`push.default=simple`) refuses that too.
pub(crate) fn push_target(branch: &str, remote: &str, to: Option<&str>) -> PushTarget {
    let there = to.unwrap_or(branch);
    PushTarget {
        remote_ref: format!("refs/heads/{there}"),
        tracking: format!("refs/remotes/{remote}/{there}"),
        short: format!("{remote}/{there}"),
    }
}

/// Why `branch` cannot be pushed to branch `to` on `remote`, if it cannot: a branch of
/// another name is a push's target only when it is the branch's upstream there (the Push
/// dialog offers nothing else).
pub(crate) async fn not_its_upstream(
    ctx: &OpCtx<'_>,
    branch: &str,
    remote: &str,
    to: &str,
) -> Result<Option<String>> {
    let wanted = format!("refs/heads/{to}");
    let ok = upstream(ctx, branch)
        .await?
        .is_some_and(|up| up.remote == remote && up.remote_ref == wanted);
    Ok((!ok).then(|| {
        said(
            format!(
                "{} is not the upstream of {}",
                name(&format!("{remote}/{to}")),
                name(branch)
            ),
            format!(
                "A push goes to {remote}/{branch}, or to the branch's upstream when you choose \
                 it."
            ),
        )
    }))
}

/// The upstream of local branch `branch`, if one is configured.
pub(crate) async fn upstream(ctx: &OpCtx<'_>, branch: &str) -> Result<Option<Upstream>> {
    let full = format!("refs/heads/{branch}");
    let out = ctx
        .read(&[
            "for-each-ref",
            "--format=%(refname)%00%(upstream:remotename)%00%(upstream:remoteref)%00%(upstream)",
            &full,
        ])
        .await?;
    Ok(out.text().lines().find_map(|line| {
        let mut parts = line.split('\0');
        let (name, remote, remote_ref, short) =
            (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        // Its full name shortened by hand: `:short` gives `remotes/origin/x` when a branch
        // or tag is also named `origin/x`.
        let short = short
            .strip_prefix("refs/remotes/")
            .or_else(|| short.strip_prefix("refs/heads/"))
            .unwrap_or(short);
        (name == full && !remote.is_empty() && !remote_ref.is_empty()).then(|| Upstream {
            remote: remote.to_owned(),
            remote_ref: remote_ref.to_owned(),
            short: short.to_owned(),
        })
    }))
}

/// The repository's remotes, by name.
pub(crate) async fn remotes(ctx: &OpCtx<'_>) -> Result<Vec<String>> {
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
        "It has none.".to_owned()
    } else {
        format!("Its remotes: {}.", known.join(", "))
    };
    Ok(Some(said(
        format!("{} is not a remote of this repository", name(remote)),
        there,
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
/// refs/heads/<branch>:<remote ref>`, to the branch of the same name on `remote`, or to `to`
/// (its upstream there; see [`push_target`]). A branch without an upstream gets one (the
/// branch pushed to): a pushed branch should know where it was pushed. An existing upstream
/// is never changed. `force` only once confirmed with a `lease` that is still the
/// remote-tracking commit and a `tip` that is still the local branch's (see the `force_push`
/// danger); git then refuses unless the remote branch is exactly there (an empty lease:
/// unless it does not exist), and `tip` is what is pushed.
pub(crate) async fn push(
    ctx: &OpCtx<'_>,
    branch: &str,
    remote: &str,
    to: Option<&str>,
    force: bool,
    lease: Option<&str>,
    tip: Option<&str>,
) -> Result<Step> {
    let set_upstream = upstream(ctx, branch).await?.is_none();
    let target = push_target(branch, remote, to);
    // A force push sends the tip that was confirmed, unless it sets the upstream, which
    // takes the branch's name (the tip was checked in this turn all the same).
    let source = match tip {
        Some(tip) if force && !set_upstream => tip.to_owned(),
        _ => format!("refs/heads/{branch}"),
    };
    let refspec = format!("{source}:{}", target.remote_ref);
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
    let there = where_on(branch, remote, &target.short);
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
                    to: to.map(str::to_owned),
                },
            ],
            Some(Rejected::StaleInfo) => vec![OpHint::Fetch],
            _ => Vec::new(),
        };
        return Ok(with_hints(explain(&args, &out), hints));
    }
    if pushed_nothing(&out) {
        return Ok(Step::Done(said(
            "Nothing to push",
            format!("{} is up to date on {there}.", name(branch)),
        )));
    }
    let what = if force { "Force-pushed" } else { "Pushed" };
    let pushed = format!("{what} {} to {there}", name(branch));
    Ok(Step::Done(if set_upstream {
        said(pushed, "Upstream set.")
    } else {
        pushed
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
        PullMode::Merge => {
            args.push("--no-rebase");
            // Merging was chosen over the fast-forward-only rule that stopped the pull: for
            // this pull only (older git does not let --no-rebase lift it).
            if fast_forward_only_configured(ctx).await? {
                args.push("--ff");
            }
        }
        PullMode::Rebase => args.push("--rebase"),
    }
    let from = upstream(ctx, branch)
        .await?
        .map(|up| upstream_where(branch, &up));
    let stash_before = stash_top(ctx).await?;
    let stash_before = stash_before.as_deref();
    let head_before = head_commit(ctx).await?;
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
        let done = said(
            format!("{} is up to date", name(branch)),
            format!(
                "Nothing new on {}.",
                from.as_deref().unwrap_or("its upstream")
            ),
        );
        return autostash_note(ctx, Step::Done(done), &out, stash_before).await;
    }
    // How it came in: a merge commit made, a rebase asked for, else (a fast-forward, or a
    // rebase the config chose) just where from.
    let merged = head_commit(ctx).await? != head_before
        && ctx
            .read(&["rev-parse", "--verify", "--quiet", "HEAD^2"])
            .await?
            .code
            == 0;
    let from = from.unwrap_or_else(|| "its upstream".to_owned());
    let how = if merged {
        format!("Merged from {from}.")
    } else if mode == PullMode::Rebase {
        format!("Rebased onto {from}.")
    } else {
        format!("From {from}.")
    };
    let done = said(format!("Pulled {}", name(branch)), how);
    autostash_note(ctx, Step::Done(done), &out, stash_before).await
}

/// Whether a pull here may only fast-forward: `pull.ff=only`, or with no `pull.ff`,
/// `merge.ff=only`.
async fn fast_forward_only_configured(ctx: &OpCtx<'_>) -> Result<bool> {
    for key in ["pull.ff", "merge.ff"] {
        let out = ctx.read(&["config", "--get", key]).await?;
        if out.code == 0 {
            return Ok(out.text().trim() == "only");
        }
    }
    Ok(false)
}

async fn pull_other(ctx: &OpCtx<'_>, branch: &str) -> Result<Step> {
    let Some(up) = upstream(ctx, branch).await? else {
        return Ok(no_upstream(branch));
    };
    // From the repository's config, so checked like every other argument: never an option.
    if up.remote.starts_with('-') || up.remote_ref.starts_with('-') {
        return Ok(Step::Failed {
            message: said(
                format!("The upstream of {} cannot be used", name(branch)),
                format!(
                    "branch.{branch}.remote and .merge do not name a remote and a branch git \
                     can be given."
                ),
            ),
            detail: format!(
                "remote \u{201c}{}\u{201d}, branch \u{201c}{}\u{201d}\n",
                up.remote, up.remote_ref
            ),
            hints: Vec::new(),
        });
    }
    let into = format!("refs/heads/{branch}");
    let refspec = format!("{}:{into}", up.remote_ref);
    let args = ["fetch", "--progress", up.remote.as_str(), refspec.as_str()];
    let before = ref_commit(ctx, &into).await?;
    let out = ctx.git_net(&args).await?;
    if out.code != 0 {
        return fetch_into_failed(ctx, &args, &out, "pull").await;
    }
    let after = ref_commit(ctx, &into).await?;
    let from = upstream_where(branch, &up);
    Ok(Step::Done(if before == after {
        said(
            format!("{} is up to date", name(branch)),
            format!("Nothing new on {from}."),
        )
    } else {
        said(
            format!("Fast-forwarded {}", name(branch)),
            format!(
                "To {} from {from}, without checking it out.",
                name(after.as_deref().map_or("", short))
            ),
        )
    }))
}

/// The failure of pulling `branch`, which has no upstream.
fn no_upstream(branch: &str) -> Step {
    Step::Failed {
        message: said(
            format!("{} has no upstream", name(branch)),
            "There is nothing to pull from: push it with an upstream first, or set one.",
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

/// The step for a fetch into a local branch (`args`) that failed with `out`, as [`explain`]
/// says, except when git refused because the branch is checked out in this very worktree:
/// then the message does not say to switch to another worktree. With git stopped here (a
/// rebase working on that branch) it says what is in progress, and to `then` ("pull",
/// "fetch") once it has ended; with nothing stopped, to pull the branch instead.
async fn fetch_into_failed(
    ctx: &OpCtx<'_>,
    args: &[&str],
    out: &GitOutput,
    then: &str,
) -> Result<Step> {
    let step = explain(args, out);
    let refused = out.stderr.lines().find_map(|l| {
        let rest = l
            .trim()
            .strip_prefix("fatal: refusing to fetch into branch 'refs/heads/")?;
        let (branch, path) = rest.split_once("' checked out at '")?;
        Some((
            branch.to_owned(),
            path.strip_suffix('\'').unwrap_or(path).to_owned(),
        ))
    });
    let Some((branch, path)) = refused else {
        return Ok(step);
    };
    if !crate::platform::same_path(std::path::Path::new(&path), &ctx.cwd) {
        return Ok(step);
    }
    let stopped = stopped_kind(ctx).await;
    let Step::Failed { detail, hints, .. } = step else {
        return Ok(step);
    };
    // Nothing stopped: the branch is simply checked out here.
    let Some(kind) = stopped else {
        let instead = if then == "pull" {
            "pull it as the current branch instead"
        } else {
            "use Pull to update it"
        };
        return Ok(Step::Failed {
            message: said(
                format!("{} is checked out here", name(&branch)),
                format!("git does not move a checked-out branch by fetching into it: {instead}."),
            ),
            detail,
            hints,
        });
    };
    let kind = kind_name(kind);
    Ok(Step::Failed {
        message: said(
            format!("A {kind} is in progress on {}", name(&branch)),
            format!(
                "Until it ends, git counts the branch as checked out here and does not move it \
                 by fetching. Continue or abort the {kind} first, then {then}."
            ),
        ),
        detail,
        hints,
    })
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
                "{} is not a branch of any remote of this repository",
                name(remote_branch)
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
        return fetch_into_failed(ctx, &args, &out, "fetch").await;
    }
    let after = ref_commit(ctx, &into).await?;
    let at = name(after.as_deref().map_or("", short));
    let from = where_on(local, &remote, remote_branch);
    Ok(Step::Done(match before {
        None => said(
            format!("Created branch {}", name(local)),
            format!("At {at} from {from}."),
        ),
        Some(b) if Some(&b) == after.as_ref() => said(
            format!("{} is up to date", name(local)),
            format!("Nothing new on {from}."),
        ),
        Some(_) => said(
            format!("Fast-forwarded {}", name(local)),
            format!("To {at} from {from}."),
        ),
    }))
}

/// `push --progress --force-with-lease=refs/heads/<branch>:<lease> <remote> --delete
/// refs/heads/<branch>` (only once confirmed; see the `delete_remote_branch` danger, whose
/// token is `lease`): git deletes it only if it is still where it was last fetched (with no
/// lease: only if it is not there at all), so commits pushed since are never deleted
/// unseen. The local branch of that name is left alone.
pub(crate) async fn delete_remote_branch(
    ctx: &OpCtx<'_>,
    remote: &str,
    branch: &str,
    lease: Option<&str>,
) -> Result<Step> {
    let full = format!("refs/heads/{branch}");
    let lease_arg = format!("--force-with-lease={full}:{}", lease.unwrap_or(""));
    let args = [
        "push",
        "--progress",
        lease_arg.as_str(),
        remote,
        "--delete",
        full.as_str(),
    ];
    let out = ctx.git_net(&args).await?;
    if out.code == 0 {
        return Ok(Step::Done(format!("Deleted {} on {remote}", name(branch))));
    }
    let all = format!("{}\n{}", out.stderr, out.text());
    let step = explain(&args, &out);
    if push_rejected(&all) != Some(Rejected::StaleInfo) {
        return Ok(step);
    }
    let Step::Failed { detail, .. } = step else {
        return Ok(step);
    };
    // Without a lease commit it was never fetched here: nothing of it has been seen.
    let title = format!("{} on {remote} was not deleted", name(branch));
    let message = if lease.is_none_or(str::is_empty) {
        said(
            title,
            "It has not been fetched here, and deleting it would delete commits you have not \
             seen. Fetch, look at what is on it, then decide again.",
        )
    } else {
        said(
            title,
            "It changed since your last fetch, and deleting it would delete commits you have \
             not seen. Fetch, look at what is new, then decide again.",
        )
    };
    Ok(Step::Failed {
        message,
        detail,
        hints: vec![OpHint::Fetch],
    })
}

/// `push --progress <remote> refs/tags/<name>`.
pub(crate) async fn push_tag(ctx: &OpCtx<'_>, tag: &str, remote: &str) -> Result<Step> {
    let full = format!("refs/tags/{tag}");
    let args = ["push", "--progress", remote, full.as_str()];
    let out = ctx.git_net(&args).await?;
    Ok(if out.code != 0 {
        explain(&args, &out)
    } else if pushed_nothing(&out) {
        Step::Done(said(
            "Nothing to push",
            format!("{remote} has tag {} already.", name(tag)),
        ))
    } else {
        Step::Done(format!("Pushed tag {} to {remote}", name(tag)))
    })
}

/// Deletes tag `name` on `remote` (`push --progress <remote> --delete refs/tags/<name>`),
/// then here (`tag -d -- <name>`). The remote goes first, so that if it cannot be reached
/// nothing has changed; a remote that does not have the tag is fine.
pub(crate) async fn delete_tag_everywhere(
    ctx: &OpCtx<'_>,
    tag: &str,
    remote: &str,
) -> Result<Step> {
    let full = format!("refs/tags/{tag}");
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
    let local = ["tag", "-d", "--", tag];
    let out = ctx.git(&local).await?;
    if out.code != 0 {
        return Ok(explain(&local, &out));
    }
    let deleted = format!("Deleted tag {}", name(tag));
    Ok(Step::Done(if absent {
        said(deleted, format!("{remote} did not have it."))
    } else {
        said(deleted, format!("Here and on {remote}."))
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
        return Ok(Step::Done(said(
            &checked_out,
            "It has no upstream to fast-forward to.",
        )));
    };
    let from = upstream_where(branch, &up);
    let args = ["merge", "--ff-only", "@{upstream}"];
    let before = head_commit(ctx).await?;
    let out = ctx.git(&args).await?;
    if out.code != 0 {
        let all = format!("{}\n{}", out.stderr, out.text());
        let why = if all.contains("Not possible to fast-forward") {
            format!("it has commits {from} does not have.")
        } else {
            match explain(&args, &out) {
                Step::Failed { message, .. } => crate::notice::as_sentence(&message),
                _ => "git refused.".to_owned(),
            }
        };
        return Ok(Step::DoneWith {
            message: said(&checked_out, format!("Not fast-forwarded to {from}: {why}")),
            detail: Some(all.trim().to_owned()),
            hints: Vec::new(),
        });
    }
    Ok(Step::Done(if head_commit(ctx).await? == before {
        said(&checked_out, format!("Up to date with {from}."))
    } else {
        said(&checked_out, format!("Fast-forwarded to {from}."))
    }))
}
