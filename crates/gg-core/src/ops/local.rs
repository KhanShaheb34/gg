//! Local operations: checkout, branches, merge, rebase, cherry-pick, revert, reset and
//! tags. Each body runs git in the active worktree and turns how it ended into a [`Step`]:
//! done, stopped on conflicts, or failed (with an explanation where one is known).

use crate::dto::{
    Danger, DangerKind, InProgressKind, MergeTarget, MergeTargetKind, OpHint, ResetMode,
};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::progress::{incoming_commit, read_kind};
use crate::git::runner::GitOutput;
use crate::notice::{self, add_note, name, said, said_const};
use crate::ops::explain::{NO_BRANCH, nothing_to_do};
use crate::ops::progress::unmerged_paths;
use crate::ops::{OpCtx, Step, explain, network};

/// The first 8 characters of `hash`, for messages (as the UI shows hashes).
pub(crate) fn short(hash: &str) -> &str {
    hash.get(..8).unwrap_or(hash)
}

/// Remembers `name` as how the user named the commit coming in to the merge or rebase that
/// just ran, if git stopped in it: the banner says it so.
async fn remember_incoming(ctx: &OpCtx<'_>, name: &str) -> Result<()> {
    if let Some(hash) = incoming_commit(&ctx.runner, &ctx.cwd).await? {
        *ctx.repo
            .incoming_name
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some((hash, name.to_owned()));
    }
    Ok(())
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
        &["checkout", "--no-guess", branch, "--"],
        format!("Checked out {}", name(branch)),
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
        said(
            format!("Checked out {}", name(local)),
            format!("A new branch tracking {}.", name(remote_branch)),
        ),
    )
    .await
}

/// `checkout --detach --end-of-options <hash> --`: the hash after `--end-of-options`, and
/// `--` after it, so that a file named like it cannot make it ambiguous.
pub(crate) async fn checkout_commit(ctx: &OpCtx<'_>, hash: &str) -> Result<Step> {
    run(
        ctx,
        &["checkout", "--detach", "--end-of-options", hash, "--"],
        said(
            format!("Checked out {}", name(short(hash))),
            "HEAD is detached: no branch is checked out.",
        ),
    )
    .await
}

/// A new branch at `at`: `branch -- <name> <at>`, or with `checkout`,
/// `checkout -b <name> <at>`: one git command either way. `bad_name` if a branch of that name
/// exists: refused without git when the branches as last read have it, else as git refuses it
/// (git never moves an existing branch for these, and writes nothing when it refuses).
pub(crate) async fn create_branch(
    ctx: &OpCtx<'_>,
    name: &str,
    at: &str,
    checkout: bool,
) -> Result<Step> {
    let taken = || {
        CoreError::new(
            ErrorCode::BadName,
            format!("A branch named \u{201c}{name}\u{201d} already exists."),
        )
    };
    if ctx.repo.knows_branch(name).await {
        return Err(taken());
    }
    let (args, done): (&[&str], _) = if checkout {
        (
            &["checkout", "-b", name, at],
            format!("Created and checked out {}", notice::name(name)),
        )
    } else {
        (
            &["branch", "--", name, at],
            format!("Created branch {}", notice::name(name)),
        )
    };
    let out = ctx.git(args).await?;
    if out.code == 0 {
        return Ok(Step::Done(done));
    }
    // Made since the branches were last read (in a terminal, say).
    if refused_as_taken(&out.stderr, name) {
        return Err(taken());
    }
    Ok(explain(args, &out))
}

/// Whether git's `stderr` refuses to make branch `name` because it exists
/// ("fatal: a branch named 'x' already exists"; git runs with `LC_ALL=C`).
fn refused_as_taken(stderr: &str, name: &str) -> bool {
    let said = format!("fatal: a branch named '{name}' already exists");
    stderr.lines().any(|l| l.trim() == said)
}

/// `branch -m -- <old> <new>`.
pub(crate) async fn rename_branch(ctx: &OpCtx<'_>, old: &str, new: &str) -> Result<Step> {
    run(
        ctx,
        &["branch", "-m", "--", old, new],
        format!("Renamed {} to {}", name(old), name(new)),
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
        format!("Deleted branch {}", notice::name(name)),
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
    said(
        format!("The {what} stopped on conflicts"),
        format!("Resolve them, then continue the {what}, or abort it."),
    )
}

/// The plain line for git having failed with a `kind` left in progress but no conflicts:
/// a hook refused the commit, or something else git says below went wrong.
pub(crate) fn stopped_without_conflicts(kind: InProgressKind) -> String {
    match kind {
        InProgressKind::Merge => said(
            "A hook stopped the merge commit",
            "The merge is staged, not committed. What the hook said is in the log.",
        ),
        _ => {
            let what = kind_name(kind);
            said(
                format!("The {what} stopped without conflicts"),
                format!(
                    "It is still in progress: continue it, or abort it. Why git stopped is in \
                     the log."
                ),
            )
        }
    }
}

/// The branch HEAD is on, or `None` when detached (or unborn and unreadable): the full ref
/// with `refs/heads/` taken off, as the snapshot names it (`read_head`). Not `--short`, which
/// gives `heads/main` when a tag is also named `main`.
pub(crate) async fn current_branch(ctx: &OpCtx<'_>) -> Result<Option<String>> {
    let out = ctx.read(&["symbolic-ref", "-q", "HEAD"]).await?;
    let text = out.text();
    Ok((out.code == 0)
        .then(|| text.trim().strip_prefix("refs/heads/").map(str::to_owned))
        .flatten()
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

/// `merge --no-edit [--no-ff | --ff] [--squash] [--no-commit] --end-of-options <target>`. `--squash` goes
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
    let merged = notice::name(shown);
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
    args.extend(["--end-of-options", name]);

    let branch = current_branch(ctx).await?;
    let before = head_commit(ctx).await?;
    // Unmerged entries the squash did not make must never be undone with it (run_op refuses
    // a merge over them; this is the second line).
    let unmerged_before = squash && !unmerged_paths(ctx).await?.is_empty();
    let stash_before = stash_top(ctx).await?;
    let ran = git_may_stop(ctx, &args, false).await?;
    remember_incoming(ctx, shown).await?;
    let out = match ran {
        Ok(out) => out,
        Err(step) if squash && !unmerged_before => return squash_failed(ctx, step).await,
        Err(step) if squash => return Ok(step),
        Err(step) => return Ok(step),
    };
    if let Some(message) = nothing_to_do(&args, &out) {
        return Ok(Step::Done(message));
    }
    let into = branch
        .as_deref()
        .map_or_else(|| "the detached HEAD".to_owned(), notice::name);
    let stash_before = stash_before.as_deref();
    if squash {
        let done = said(
            format!("Squashed {merged} into {into}"),
            "The changes are staged: commit them to finish.",
        );
        return autostash_note(ctx, Step::Done(done), &out, stash_before).await;
    }
    if stopped_kind(ctx).await == Some(InProgressKind::Merge) {
        let done = said(
            format!("Merged {merged} into {into}, not committed"),
            "The result is staged: commit it to finish the merge.",
        );
        return autostash_note(ctx, Step::Done(done), &out, stash_before).await;
    }
    let hints = match &branch {
        Some(b) if head_commit(ctx).await? != before => {
            push_hint(ctx, b).await?.into_iter().collect()
        }
        _ => Vec::new(),
    };
    let message = format!("Merged {merged} into {into}");
    let step = if hints.is_empty() {
        Step::Done(message)
    } else {
        Step::DoneWith {
            message,
            detail: None,
            hints,
        }
    };
    autostash_note(ctx, step, &out, stash_before).await
}

/// Said when a squash merge conflicts.
const SQUASH_CONFLICTS: &str = said_const!(
    "The squash merge has conflicts: nothing was changed",
    "A squash can't be continued here. Merge normally, or squash in a terminal."
);

/// `step`, how a squash merge failed, when no file was unmerged before it started. One that
/// left conflicts (git leaves no MERGE_HEAD for a squash, so it could be neither continued
/// nor aborted) is undone with `reset --merge`,
/// which puts the index and the files the merge touched back and keeps the uncommitted
/// changes it did not touch; the step then says so, with git's output.
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

/// Whether git's output says that the changes it auto-stashed conflicted as they were put
/// back. git 2.54 says "Applying autostash resulted in conflicts."; newer ones (2.56 at least)
/// "Your local changes are stashed, however applying them resulted in conflicts.", broken
/// over lines (the runner runs git with `LC_ALL=C`, so the text is English).
fn autostash_conflicted(output: &str) -> bool {
    let words = output.split_whitespace().collect::<Vec<_>>().join(" ");
    words.contains("Applying autostash resulted in conflicts")
        || words
            .contains("Your local changes are stashed, however applying them resulted in conflicts")
}

/// The stash's top entry (`refs/stash`), read before an op that may auto-stash: see
/// [`autostash_note`].
pub(crate) async fn stash_top(ctx: &OpCtx<'_>) -> Result<Option<String>> {
    let out = ctx
        .read(&["rev-parse", "--verify", "--quiet", "refs/stash"])
        .await?;
    Ok((out.code == 0).then(|| out.text().trim().to_owned()))
}

/// Whether git kept auto-stashed changes in the stash during an op: the stash's top entry
/// is another than `before` (read before the op), and git's reflog calls it "autostash"
/// (git stores the auto-stash only when putting it back failed; every git version says so).
async fn autostash_kept(ctx: &OpCtx<'_>, before: Option<&str>) -> Result<bool> {
    let after = stash_top(ctx).await?;
    if after.is_none() || after.as_deref() == before {
        return Ok(false);
    }
    let out = ctx
        .read(&["log", "--walk-reflogs", "-1", "--format=%gs", "refs/stash"])
        .await?;
    Ok(out.code == 0 && out.text().trim() == "autostash")
}

/// `step` (done), saying so if the changes git auto-stashed (`rebase.autoStash`,
/// `merge.autoStash`) conflicted as they were put back: git exits 0, keeps them in the
/// stash and leaves its attempt in the working tree. Known by git's output `out`, or else
/// by a new "autostash" entry in the stash since `stash_before` (the top entry before the
/// op, [`stash_top`]), so a git that words it differently is still understood. git's text
/// becomes the detail.
pub(crate) async fn autostash_note(
    ctx: &OpCtx<'_>,
    step: Step,
    out: &GitOutput,
    stash_before: Option<&str>,
) -> Result<Step> {
    if !matches!(step, Step::Done(_) | Step::DoneWith { .. }) {
        return Ok(step);
    }
    let all = format!("{}\n{}", out.stderr, out.text());
    // git has run by now: a stash that cannot be read must not turn what it did into an
    // error, so it counts as nothing kept (the text has been looked at). `refs/stash` is shared
    // by every worktree: an auto-stash a terminal's rebase kept in another worktree during
    // this op would be taken for this op's (rare: both would have to end at once).
    #[cfg(any(test, feature = "testutil"))]
    let said = autostash_conflicted(&all)
        && !crate::testutil::AUTOSTASH_BY_STATE_ONLY.load(std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(any(test, feature = "testutil")))]
    let said = autostash_conflicted(&all);
    if !said && !autostash_kept(ctx, stash_before).await.unwrap_or(false) {
        return Ok(step);
    }
    let (message, hints) = match step {
        Step::Done(message) => (message, Vec::new()),
        Step::DoneWith { message, hints, .. } => (message, hints),
        other => return Ok(other),
    };
    Ok(Step::DoneWith {
        message: add_note(
            &message,
            "Your uncommitted changes conflicted as they were put back: they are kept in the \
             stash, and the attempt, with conflicts, is in the working tree.",
        ),
        detail: Some(all.trim().to_owned()),
        hints,
    })
}

/// Push for `branch` after it got new commits, to the branch of the same name: on
/// its upstream's remote when the upstream has that name, or, without an upstream, on the
/// repository's only remote (where the push then sets it). `None` otherwise: a branch whose
/// upstream has another name (or is a local branch) is pushed from the Push dialog, which
/// names where it goes.
async fn push_hint(ctx: &OpCtx<'_>, branch: &str) -> Result<Option<OpHint>> {
    if let Some(up) = network::upstream(ctx, branch).await? {
        // "." is the repository itself: an upstream that is a local branch.
        let same_name = up.remote_ref == format!("refs/heads/{branch}");
        return Ok((up.remote != "." && same_name).then(|| OpHint::Push {
            branch: branch.to_owned(),
            remote: up.remote,
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
    let stash_before = stash_top(ctx).await?;
    let ran = git_may_stop(ctx, &args, false).await?;
    remember_incoming(ctx, onto).await?;
    let out = match ran {
        Ok(out) => out,
        Err(step) => return Ok(step),
    };
    let done = nothing_to_do(&args, &out)
        .unwrap_or_else(|| format!("Rebased {} onto {}", name(&branch), name(onto)));
    autostash_note(ctx, Step::Done(done), &out, stash_before.as_deref()).await
}

/// `cherry-pick [-m N] [--no-commit] --end-of-options <hash>`.
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
    args.extend(["--end-of-options", hash]);
    let into = current_branch(ctx).await?;
    let into = into
        .as_deref()
        .map_or_else(|| "the detached HEAD".to_owned(), name);
    if let Err(step) = git_may_stop(ctx, &args, false).await? {
        return Ok(step);
    }
    let picked = name(short(hash));
    Ok(Step::Done(if no_commit {
        said(
            format!("Applied {picked} to {into}, not committed"),
            "Its changes are staged.",
        )
    } else {
        format!("Cherry-picked {picked} onto {into}")
    }))
}

/// `revert --no-edit [-m N] --end-of-options <hash>`.
pub(crate) async fn revert(ctx: &OpCtx<'_>, hash: &str, mainline: Option<u32>) -> Result<Step> {
    let parent = mainline.map(|n| n.to_string());
    let mut args = vec!["revert", "--no-edit"];
    if let Some(n) = &parent {
        args.extend(["-m", n]);
    }
    args.extend(["--end-of-options", hash]);
    if let Err(step) = git_may_stop(ctx, &args, false).await? {
        return Ok(step);
    }
    Ok(Step::Done(said(
        format!("Reverted {}", name(short(hash))),
        "A new commit undoes its changes.",
    )))
}

/// `reset --soft|--mixed|--hard --end-of-options <hash> --` (a hard one only once confirmed;
/// see the `hard_reset` danger): `--` after the hash, so that a file named like it does not
/// make git refuse it as ambiguous.
pub(crate) async fn reset(ctx: &OpCtx<'_>, hash: &str, mode: ResetMode) -> Result<Step> {
    let (flag, kept) = match mode {
        ResetMode::Soft => ("--soft", "The changes since are staged."),
        ResetMode::Mixed => (
            "--mixed",
            "The changes since are in the working tree, unstaged.",
        ),
        ResetMode::Hard => ("--hard", "Uncommitted changes were discarded."),
    };
    let branch = current_branch(ctx).await?;
    let what = branch.as_deref().map_or_else(|| "HEAD".to_owned(), name);
    run(
        ctx,
        &["reset", flag, "--end-of-options", hash, "--"],
        said(format!("Reset {what} to {}", name(short(hash))), kept),
    )
    .await
}

/// `tag -- <name> <hash>`, or with a message, an annotated tag:
/// `tag -a -m <message> -- <name> <hash>`. `bad_name` if a tag of that name exists (git is
/// not asked to make it), as for a branch.
pub(crate) async fn create_tag(
    ctx: &OpCtx<'_>,
    name: &str,
    hash: &str,
    message: Option<&str>,
) -> Result<Step> {
    let full = format!("refs/tags/{name}");
    let exists = ctx
        .read(&["rev-parse", "--verify", "--quiet", &full])
        .await?;
    if exists.code == 0 {
        return Err(CoreError::new(
            ErrorCode::BadName,
            format!("A tag named \u{201c}{name}\u{201d} already exists."),
        ));
    }
    let mut args = vec!["tag"];
    if let Some(message) = message {
        args.extend(["-a", "-m", message]);
    }
    args.extend(["--", name, hash]);
    run(
        ctx,
        &args,
        format!(
            "Created tag {} at {}",
            notice::name(name),
            notice::name(short(hash))
        ),
    )
    .await
}

/// `tag -d -- <name>`.
pub(crate) async fn delete_tag(ctx: &OpCtx<'_>, name: &str) -> Result<Step> {
    run(
        ctx,
        &["tag", "-d", "--", name],
        format!("Deleted tag {}", notice::name(name)),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{autostash_conflicted, refused_as_taken};

    #[test]
    fn a_taken_branch_name_is_recognised_in_gits_words() {
        let err = "fatal: a branch named 'feature/x' already exists\n";
        assert!(refused_as_taken(err, "feature/x"));
        assert!(!refused_as_taken(err, "feature"));
        assert!(!refused_as_taken("fatal: invalid reference: x\n", "x"));
    }

    #[test]
    fn an_autostash_that_conflicted_is_recognised_in_old_and_new_words() {
        // git 2.54 and earlier.
        assert!(autostash_conflicted(
            "Successfully rebased and updated refs/heads/main.\nApplying autostash resulted in \
             conflicts.\nYour changes are safe in the stash.\n"
        ));
        // git 2.56, as it prints it.
        assert!(autostash_conflicted(
            "Your local changes are stashed, however applying them\nresulted in conflicts.  You \
             can either resolve the conflicts\nand then discard the stash with \"git stash drop\"\n"
        ));
        assert!(!autostash_conflicted(
            "Applied autostash.\nSuccessfully rebased."
        ));
    }
}
