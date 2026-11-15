//! What git stopped in (a merge, rebase, cherry-pick or revert, started here or outside the
//! app): continuing it, aborting it, and refusing what would change HEAD or the working
//! tree meanwhile. Opening files to resolve conflicts is in [`crate::editor`].
//!
//! The app never edits conflicted files: continuing stages the paths that are unmerged at
//! that moment (the user resolved them; the UI asked them to confirm first) and nothing
//! else.

use crate::dto::ConflictState;
use crate::dto::{InProgressKind, OpHint, OpRequest};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::progress::{judge, read_kind, state_files};
use crate::git::status::{paths, read_conflicted};
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
    // A commit concludes a merge (see `commit`); a rebase, cherry-pick or revert makes its
    // commit by continuing.
    if matches!(request, OpRequest::Commit { .. }) {
        return (kind != InProgressKind::Merge).then(|| {
            format!(
                "A {} is in progress: Continue commits its changes and goes on (or abort it). \
                 A commit of its own would end up in the middle of it.",
                kind_name(kind)
            )
        });
    }
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
    Ok(Some(format!(
        "Some files still have conflicts from an earlier step ({}). Resolve them and stage \
         them, or discard them, first.",
        named(&paths)
    )))
}

/// Why continue cannot run without the user's go-ahead: files git stopped on still have
/// conflict markers, or git kept one side of them and only the user can say which they want
/// (see [`judge`]); continuing would stage them as they are. None when every file is
/// resolved.
pub(crate) async fn unresolved_conflicts(ctx: &OpCtx<'_>) -> Result<Option<String>> {
    let conflicted = read_conflicted(&ctx.runner, &ctx.cwd).await?;
    let files = judge(&ctx.runner, &ctx.cwd, &conflicted).await?;
    let with = |state| {
        files
            .iter()
            .filter(|f| f.state == state)
            .map(|f| f.path.clone())
            .collect::<Vec<_>>()
    };
    let (marked, undecided) = (with(ConflictState::Markers), with(ConflictState::Undecided));
    let mut why = Vec::new();
    if !marked.is_empty() {
        why.push(format!(
            "Some files still have conflict markers ({}).",
            named(&marked)
        ));
    }
    if !undecided.is_empty() {
        why.push(format!(
            "Some files have no conflict markers to go by: binary, deleted on one side, or too \
             large to read here ({}).",
            named(&undecided)
        ));
    }
    if why.is_empty() {
        return Ok(None);
    }
    why.push("Resolve them, or continue anyway.".to_owned());
    Ok(Some(why.join(" ")))
}

/// The first [`NAMED_CONFLICTS`] of `paths`, joined, with an ellipsis if there are more.
fn named(paths: &[String]) -> String {
    let mut named = paths
        .iter()
        .take(NAMED_CONFLICTS)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > NAMED_CONFLICTS {
        named.push('\u{2026}');
    }
    named
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
    let unmerged = paths(&read_conflicted(&ctx.runner, &ctx.cwd).await?);
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
    let stopped_on = match state_files(&ctx.runner, &ctx.cwd).await? {
        Some(files) => files.stopped_commit(still),
        None => None,
    };
    if comes_out_empty(&all)
        && kind != InProgressKind::Merge
        && let Some(commit) = stopped_on
    {
        return Ok(Step::Failed {
            message: format!(
                "With the conflicts resolved this way the commit being applied has no \
                 changes left (they are already here), and git will not continue with an \
                 empty commit: skip this commit, or abort the {what}."
            ),
            detail,
            hints: vec![OpHint::Skip { commit }],
        });
    }
    let message = recognised(args, &all).unwrap_or_else(|| stopped_without_conflicts(still));
    Ok(Step::Failed {
        message,
        detail,
        hints: Vec::new(),
    })
}

/// Why a skip for `commit` cannot run, if it cannot: git is not stopped on that commit (it
/// has moved on since the skip was offered, in a terminal say), so skipping would leave out
/// another (R40).
pub(crate) async fn skip_refused(ctx: &OpCtx<'_>, commit: &str) -> Result<Option<String>> {
    let Some(files) = state_files(&ctx.runner, &ctx.cwd).await? else {
        return Ok(None);
    };
    let Some(kind) = files.kind() else {
        return Ok(None);
    };
    if kind == InProgressKind::Merge {
        // Said by the skip itself: a merge has no commit to skip.
        return Ok(None);
    }
    let short = |h: &str| h.get(..8).unwrap_or(h).to_owned();
    Ok(match files.stopped_commit(kind) {
        Some(now) if now == commit => None,
        Some(now) => Some(format!(
            "Nothing was skipped: git has moved on, and the {} is now stopped on {}, not on \
             {}. Look at it again, and skip again if it still applies.",
            kind_name(kind),
            short(&now),
            short(commit)
        )),
        None => Some(format!(
            "Nothing was skipped: the {} is no longer stopped on {}, the commit the skip was \
             for.",
            kind_name(kind),
            short(commit)
        )),
    })
}

/// The title of skip for what is in progress: "Skip a commit of the cherry-pick".
pub(crate) fn skip_title(kind: Option<InProgressKind>) -> String {
    match kind {
        Some(kind) => format!("Skip a commit of the {}", kind_name(kind)),
        None => "Skip".to_owned(),
    }
}

/// Leaves out the commit a rebase, cherry-pick or revert stopped on (`<kind> --skip`, which
/// puts the files back as they were before it) and goes on with the next: stopped if git
/// stops on conflicts again, done when nothing is left in progress.
pub(crate) async fn skip(ctx: &OpCtx<'_>) -> Result<Step> {
    let Some(kind) = read_kind(&ctx.runner, &ctx.cwd).await? else {
        return Err(nothing_in_progress());
    };
    let what = kind_name(kind);
    if kind == InProgressKind::Merge {
        return Ok(Step::Failed {
            message: "A merge has no commit to skip: continue it, or abort it.".to_owned(),
            detail: String::new(),
            hints: Vec::new(),
        });
    }
    let args = [what, "--skip"];
    let out = ctx.git(&args).await?;
    let after = read_kind(&ctx.runner, &ctx.cwd).await?;
    let detail = format!("{}\n{}", out.stderr.trim_end(), out.text())
        .trim()
        .to_owned();
    let Some(still) = after else {
        if out.code != 0 {
            return Ok(explain(&args, &out));
        }
        return Ok(Step::Done(format!(
            "Skipped the commit; the {what} is finished."
        )));
    };
    if !read_conflicted(&ctx.runner, &ctx.cwd).await?.is_empty() {
        return Ok(Step::Stopped {
            message: stopped_message(still),
            detail,
        });
    }
    if out.code == 0 {
        return Ok(Step::DoneWith {
            message: format!(
                "Skipped the commit; git stopped again without conflicts. The {what} is still \
                 in progress: continue it, or abort it."
            ),
            detail: Some(detail),
            hints: Vec::new(),
        });
    }
    Ok(explain(&args, &out))
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
        assert!(
            refused(OpRequest::StashApply {
                stash: stash(),
                expect_worktree: String::new()
            })
            .is_some()
        );
        assert!(
            refused(OpRequest::StashPop {
                stash: stash(),
                expect_worktree: String::new()
            })
            .is_some()
        );
        assert!(
            refused(OpRequest::StashBranch {
                stash: stash(),
                name: "x".into(),
                expect_worktree: String::new()
            })
            .is_some()
        );
        let why = refused(OpRequest::StashPop {
            stash: stash(),
            expect_worktree: String::new(),
        })
        .unwrap();
        assert!(why.starts_with("A rebase is in progress"), "{why}");
        // Saving would take the stopped operation's state with it (R17, revised).
        assert!(
            refused(OpRequest::StashSave {
                message: None,
                include_untracked: false,
                expect_worktree: String::new()
            })
            .is_some()
        );
        assert!(
            refused(OpRequest::StashDrop {
                stash: stash(),
                expect_worktree: String::new()
            })
            .is_none()
        );
        let pull = |branch: &str| OpRequest::Pull {
            branch: branch.into(),
            mode: crate::dto::PullMode::Default,
            expect_head: crate::dto::HeadInfo::default(),
            expect_worktree: String::new(),
        };
        assert!(refused(pull("main")).is_some());
        assert!(refused(pull("other")).is_none(), "only fetches into it");
    }
}
