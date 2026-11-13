//! What git stopped in (a merge, rebase, cherry-pick or revert, started here or outside the
//! app): continuing it, aborting it, and refusing what would change HEAD or the working
//! tree meanwhile. Opening files to resolve conflicts is in [`crate::editor`].
//!
//! The app never edits conflicted files: continuing stages the paths that are unmerged at
//! that moment (the user resolved them; the UI asked them to confirm first) and nothing
//! else.

use crate::dto::{InProgressKind, OpRequest};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::progress::read_kind;
use crate::git::status::read_conflicted;
use crate::ops::explain::{explain, recognised};
use crate::ops::local::{current_branch, kind_name, stopped_message, stopped_without_conflicts};
use crate::ops::{OpCtx, Step};

/// Most paths given to one `git add`, so the command line stays short on any platform.
const ADD_CHUNK: usize = 100;

/// The error for continue or abort with nothing in progress.
pub(crate) fn nothing_in_progress() -> CoreError {
    CoreError::new(
        ErrorCode::NothingToDo,
        "Nothing is in progress: there is no merge, rebase, cherry-pick or revert to continue \
         or abort.",
    )
}

/// Whether `request` would check out something else, or merge something into the working
/// tree (a merge, rebase, cherry-pick, revert, pull of the branch checked out here, or a
/// stash apply, pop or branch). `head` is the branch HEAD is on.
pub(crate) fn changes_head_or_tree(request: &OpRequest, head: Option<&str>) -> bool {
    match request {
        OpRequest::CheckoutBranch { .. }
        | OpRequest::CheckoutRemote { .. }
        | OpRequest::CheckoutCommit { .. }
        | OpRequest::Merge { .. }
        | OpRequest::Rebase { .. }
        | OpRequest::CherryPick { .. }
        | OpRequest::Revert { .. }
        | OpRequest::StashApply { .. }
        | OpRequest::StashPop { .. }
        | OpRequest::StashBranch { .. } => true,
        OpRequest::CreateBranch { checkout, .. } => *checkout,
        // Pulling a branch that is not checked out only fetches into it.
        OpRequest::Pull { branch, .. } => head == Some(branch.as_str()),
        _ => false,
    }
}

/// Why `request` cannot run while git is stopped in `kind`, if it cannot: it would check
/// out something else or start another merge, rebase, cherry-pick or revert under the
/// stopped one (a stash pop or apply merges too; see [`changes_head_or_tree`]). Saving a
/// stash is refused too: git would take the stopped operation's state with it (a merge
/// whose conflicts are resolved silently ends; R17 revised). `head` is the branch HEAD is
/// on. A reset runs (it is a way out; a hard one still needs its confirmation), and so does
/// dropping a stash. Continue and abort are not refused here.
pub(crate) fn refused_while(
    request: &OpRequest,
    kind: InProgressKind,
    head: Option<&str>,
) -> Option<String> {
    (changes_head_or_tree(request, head) || matches!(request, OpRequest::StashSave { .. })).then(
        || {
            format!(
                "A {} is in progress: continue it or abort it first. Until then this would \
                 change what it is working on.",
                kind_name(kind)
            )
        },
    )
}

/// Most conflicted paths named in [`unexplained_conflicts`]'s message.
const NAMED_CONFLICTS: usize = 3;

/// Why nothing that changes HEAD or the working tree may run, when nothing is in progress:
/// files still have conflicts from an earlier step (a stash pop or apply that conflicted,
/// say). git would refuse most such operations, but some (a squash merge) would fail in a
/// way that cannot tell those conflicts from their own, and undoing them would throw away
/// the user's edits of those files. None when no file is unmerged.
pub(crate) async fn unexplained_conflicts(ctx: &OpCtx<'_>) -> Result<Option<String>> {
    let paths = unmerged_paths(ctx).await?;
    if paths.is_empty() {
        return Ok(None);
    }
    let mut named = paths
        .iter()
        .take(NAMED_CONFLICTS)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > NAMED_CONFLICTS {
        named.push('\u{2026}');
    }
    Ok(Some(format!(
        "Some files still have conflicts from an earlier step ({named}). Resolve them and \
         stage them, or discard them, first."
    )))
}

/// The paths with unmerged entries in the index (`ls-files --unmerged`), sorted, each once.
pub(crate) async fn unmerged_paths(ctx: &OpCtx<'_>) -> Result<Vec<String>> {
    let out = ctx.read(&["ls-files", "--unmerged", "-z"]).await?;
    if out.code != 0 {
        return Err(
            CoreError::new(ErrorCode::GitFailed, "Could not read the index")
                .with_detail(out.stderr.clone()),
        );
    }
    let mut paths: Vec<String> = out
        .stdout
        .split(|&b| b == 0)
        .filter_map(|entry| {
            // "<mode> <object> <stage>\t<path>"
            let tab = entry.iter().position(|&b| b == b'\t')?;
            Some(String::from_utf8_lossy(&entry[tab + 1..]).into_owned())
        })
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// The title of continue (`verb` "Continue") or abort ("Abort") for what is in progress.
pub(crate) fn title(verb: &str, kind: Option<InProgressKind>) -> String {
    match kind {
        Some(kind) => format!("{verb} the {}", kind_name(kind)),
        None => verb.to_owned(),
    }
}

/// Continues what git stopped in: stages the paths unmerged now (`add -- <paths>`; a path
/// resolved by deleting it stages the deletion), then `<kind> --continue` with editors
/// suppressed. Stopped if git stops on conflicts again (a rebase's next step), failed with
/// a plain line if it stops otherwise.
pub(crate) async fn continue_op(ctx: &OpCtx<'_>) -> Result<Step> {
    let Some(kind) = read_kind(&ctx.runner, &ctx.cwd).await? else {
        return Err(nothing_in_progress());
    };
    let unmerged = read_conflicted(&ctx.runner, &ctx.cwd).await?;
    for chunk in unmerged.chunks(ADD_CHUNK) {
        // Literal: a path like `[ab].txt` must not match other files as a glob.
        let mut args = vec!["--literal-pathspecs", "add", "--"];
        args.extend(chunk.iter().map(String::as_str));
        let out = ctx.git(&args).await?;
        if out.code != 0 {
            return Ok(explain(&args, &out));
        }
    }
    let args: &[&str] = match kind {
        // `merge --continue` takes no options; it commits with the prepared message.
        InProgressKind::Merge => &["merge", "--continue"],
        InProgressKind::Rebase => &["rebase", "--continue"],
        InProgressKind::CherryPick => &["cherry-pick", "--continue", "--no-edit"],
        InProgressKind::Revert => &["revert", "--continue", "--no-edit"],
    };
    let out = ctx.git(args).await?;
    let after = read_kind(&ctx.runner, &ctx.cwd).await?;
    let what = kind_name(kind);
    let detail = if out.stderr.trim().is_empty() {
        out.text()
    } else {
        format!("{}\n{}", out.stderr.trim_end(), out.text())
            .trim()
            .to_owned()
    };
    let Some(still) = after else {
        if out.code != 0 {
            return Ok(explain(args, &out));
        }
        let branch = current_branch(ctx).await?;
        return Ok(Step::Done(match (kind, branch) {
            (InProgressKind::Rebase, Some(branch)) => format!("Finished rebasing {branch}."),
            _ => format!("Finished the {what}: it is committed."),
        }));
    };
    let conflicted = read_conflicted(&ctx.runner, &ctx.cwd).await?;
    if !conflicted.is_empty() {
        return Ok(Step::Stopped {
            message: stopped_message(still),
            detail,
        });
    }
    if out.code == 0 {
        // An interactive rebase stopping where it was told to (`edit`, `break`).
        return Ok(Step::DoneWith {
            message: format!(
                "Continued the {what}; git stopped again without conflicts. It is still in \
                 progress: continue it, or abort it."
            ),
            detail: Some(detail),
            hints: Vec::new(),
        });
    }
    let all = format!("{}\n{}", out.stderr, out.text());
    let message = if comes_out_empty(&all) {
        format!(
            "With the conflicts resolved this way the commit being applied has no changes \
             left (they are already here), and git will not continue with an empty commit: \
             abort the {what}, or skip this commit in a terminal (git says how below)."
        )
    } else {
        recognised(args, &all).unwrap_or_else(|| stopped_without_conflicts(still))
    };
    Ok(Step::Failed {
        message,
        detail,
        hints: Vec::new(),
    })
}

/// Whether git's output `all` says the commit being continued came out empty.
fn comes_out_empty(all: &str) -> bool {
    all.contains("is now empty")
        || all.contains("No changes - did you forget to use 'git add'?")
        || all.contains("nothing to commit")
}

/// Aborts what git stopped in: `<kind> --abort`, which puts HEAD, the index and the working
/// tree back as they were before it started.
pub(crate) async fn abort(ctx: &OpCtx<'_>) -> Result<Step> {
    let Some(kind) = read_kind(&ctx.runner, &ctx.cwd).await? else {
        return Err(nothing_in_progress());
    };
    let what = kind_name(kind);
    let args = [what, "--abort"];
    let out = ctx.git(&args).await?;
    if out.code != 0 {
        return Ok(explain(&args, &out));
    }
    let all = format!("{}\n{}", out.stderr, out.text());
    if all.contains("You seem to have moved HEAD") {
        // A sequence whose commit was made by hand: git ends it without rewinding.
        return Ok(Step::DoneWith {
            message: format!(
                "Stopped the {what}, without going back: commits were made since it started \
                 (HEAD moved), so git leaves them as they are."
            ),
            detail: Some(all.trim().to_owned()),
            hints: Vec::new(),
        });
    }
    let branch = current_branch(ctx).await?;
    let back = branch.as_deref().unwrap_or("HEAD");
    Ok(Step::Done(format!(
        "Aborted the {what}: {back} is back as it was before it."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stash_operations_that_merge_are_refused_while_stopped() {
        let stash = || "0123456789abcdef0123456789abcdef01234567".to_owned();
        let refused = |r: OpRequest| refused_while(&r, InProgressKind::Rebase, Some("main"));
        assert!(refused(OpRequest::StashApply { stash: stash() }).is_some());
        assert!(refused(OpRequest::StashPop { stash: stash() }).is_some());
        assert!(
            refused(OpRequest::StashBranch {
                stash: stash(),
                name: "x".into()
            })
            .is_some()
        );
        let why = refused(OpRequest::StashPop { stash: stash() }).unwrap();
        assert!(why.starts_with("A rebase is in progress"), "{why}");
        // Saving would take the stopped operation's state with it (R17, revised).
        assert!(
            refused(OpRequest::StashSave {
                message: None,
                include_untracked: false
            })
            .is_some()
        );
        assert!(refused(OpRequest::StashDrop { stash: stash() }).is_none());
        let pull = |branch: &str| OpRequest::Pull {
            branch: branch.into(),
            mode: crate::dto::PullMode::Default,
        };
        assert!(refused(pull("main")).is_some());
        assert!(refused(pull("other")).is_none(), "only fetches into it");
    }
}
