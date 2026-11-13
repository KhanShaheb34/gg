//! Stash operations: save, apply, pop, drop, and a branch from a stash.
//!
//! A request names its stash by commit hash, which does not change as stashes come and go;
//! where it is in the list (`stash@{n}`, which git's commands take) is found in the
//! operation's turn. A stash that is no longer in the list fails plainly, before anything
//! runs (see [`gone`]).
//!
//! git has no in-progress state for a stash: one that applies with conflicts leaves them in
//! the working tree and keeps the stash, and the outcome says so (`failed`).

use crate::dto::{Danger, DangerKind};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::runner::{GitOutput, git_failed};
use crate::git::stash::{Stash, read_stashes};
use crate::ops::explain::{file_list, recognised};
use crate::ops::local::{current_branch, short};
use crate::ops::{OpCtx, Step};

/// Said when the stash a request names is no longer in the list.
const GONE: &str = "That stash no longer exists: it was dropped or popped since the list was \
                    shown. Nothing was changed.";

/// The stash whose commit is `hash`, as the list has it now (its `stash@{n}` included).
async fn find(ctx: &OpCtx<'_>, hash: &str) -> Result<Option<Stash>> {
    Ok(read_stashes(&ctx.runner, &ctx.cwd)
        .await?
        .into_iter()
        .find(|s| s.hash == hash))
}

/// Why a request for the stash `hash` cannot run, if it cannot: that stash is gone.
pub(crate) async fn gone(ctx: &OpCtx<'_>, hash: &str) -> Result<Option<String>> {
    Ok(find(ctx, hash).await?.is_none().then(|| GONE.to_owned()))
}

/// `drop_stash`, always: dropping a stash deletes the only place its changes may be. The
/// message names it and its subject; the token is its commit.
pub(crate) async fn dropped(ctx: &OpCtx<'_>, hash: &str) -> Result<Vec<Danger>> {
    let Some(stash) = find(ctx, hash).await? else {
        return Ok(Vec::new()); // the body fails plainly
    };
    Ok(vec![Danger {
        kind: DangerKind::DropStash,
        message: format!(
            "Dropping {} (\u{201c}{}\u{201d}) deletes it: its changes are lost unless they \
             are somewhere else too.",
            stash.selector, stash.subject
        ),
        detail: None,
        token: Some(stash.hash),
    }])
}

/// `stash push [--include-untracked] [--message=<message>] --`. A blank message is no
/// message (git then writes "WIP on <branch>: …"). Nothing to stash is `nothing_to_do`.
pub(crate) async fn save(
    ctx: &OpCtx<'_>,
    message: Option<&str>,
    include_untracked: bool,
) -> Result<Step> {
    let mut args = vec!["stash".to_owned(), "push".to_owned()];
    if include_untracked {
        args.push("--include-untracked".to_owned());
    }
    if let Some(message) = message.filter(|m| !m.trim().is_empty()) {
        // One argument: a message starting with `-` stays the message.
        args.push(format!("--message={message}"));
    }
    args.push("--".to_owned());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = ctx.git(&args).await?;
    let all = format!("{}\n{}", out.stderr, out.text());
    if all.lines().any(|l| l.trim() == "No local changes to save") {
        return Err(CoreError::new(
            ErrorCode::NothingToDo,
            if include_untracked {
                "Nothing to stash: there are no uncommitted changes."
            } else {
                "Nothing to stash: no tracked file has changed (untracked files are stashed \
                 only when you include them)."
            },
        ));
    }
    if out.code != 0 {
        if all.contains("You do not have the initial commit yet") {
            return Ok(Step::Failed {
                message: "There is nothing to stash on yet: this branch has no commits.".to_owned(),
                detail: detail(&out),
                hints: Vec::new(),
            });
        }
        return Ok(failed(&args, &out, None));
    }
    Ok(Step::Done(if include_untracked {
        "Stashed your changes and untracked files as stash@{0}.".to_owned()
    } else {
        "Stashed your changes as stash@{0}.".to_owned()
    }))
}

/// How a stash is put back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PutBack {
    /// `stash apply`: the stash stays.
    Apply,
    /// `stash pop`: the stash is dropped if it applied cleanly.
    Pop,
}

/// `stash apply|pop <stash@{n}>` for the stash whose commit is `hash`.
pub(crate) async fn put_back(ctx: &OpCtx<'_>, hash: &str, how: PutBack) -> Result<Step> {
    let Some(stash) = find(ctx, hash).await? else {
        return Ok(gone_step());
    };
    let verb = match how {
        PutBack::Apply => "apply",
        PutBack::Pop => "pop",
    };
    let args = ["stash", verb, stash.selector.as_str()];
    let out = ctx.git(&args).await?;
    if out.code != 0 {
        return Ok(failed(&args, &out, Some(how)));
    }
    let named = format!("{} (\u{201c}{}\u{201d})", stash.selector, stash.subject);
    Ok(Step::Done(match how {
        PutBack::Apply => {
            format!("Applied {named}: its changes are in the working tree, and the stash is kept.")
        }
        PutBack::Pop => format!(
            "Popped {named}: its changes are in the working tree, and the stash is dropped."
        ),
    }))
}

/// `stash drop <stash@{n}>` (only once confirmed; see [`dropped`]).
pub(crate) async fn drop(ctx: &OpCtx<'_>, hash: &str) -> Result<Step> {
    let Some(stash) = find(ctx, hash).await? else {
        return Ok(gone_step());
    };
    let args = ["stash", "drop", stash.selector.as_str()];
    let out = ctx.git(&args).await?;
    if out.code != 0 {
        return Ok(failed(&args, &out, None));
    }
    Ok(Step::Done(format!(
        "Dropped {} (\u{201c}{}\u{201d}).",
        stash.selector, stash.subject
    )))
}

/// `stash branch <name> <stash@{n}>`: a new branch at the stash's base, checked out, with
/// the stash applied (and dropped if it applied cleanly). `bad_name` if a branch of that
/// name exists (git is not asked).
pub(crate) async fn branch(ctx: &OpCtx<'_>, hash: &str, name: &str) -> Result<Step> {
    let exists = ctx
        .read(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ])
        .await?;
    if exists.code == 0 {
        return Err(CoreError::new(
            ErrorCode::BadName,
            format!("A branch named \u{201c}{name}\u{201d} already exists."),
        ));
    }
    let Some(stash) = find(ctx, hash).await? else {
        return Ok(gone_step());
    };
    let args = ["stash", "branch", name, stash.selector.as_str()];
    let out = ctx.git(&args).await?;
    if out.code != 0 {
        let made = current_branch(ctx).await?.as_deref() == Some(name);
        return Ok(branch_failed(&args, &out, name, made));
    }
    Ok(Step::Done(format!(
        "Created branch {name} at {} from {} and checked it out: the stash's changes are in \
         the working tree, and the stash is dropped.",
        short(&stash.base),
        stash.selector
    )))
}

/// The failed step of `stash branch <name>` (`args`, `out`). git first checks out a new
/// branch at the stash's base, then applies the stash. `made`: the checkout happened (the
/// branch exists and is checked out) and applying failed; the stash is kept. Otherwise the
/// checkout was blocked and nothing was made: told as a checkout's failure.
fn branch_failed(args: &[&str], out: &GitOutput, name: &str, made: bool) -> Step {
    let detail = detail(out);
    let message = if made {
        match put_back_failure(&detail, PutBack::Apply) {
            Some(why) => format!(
                "Created branch {name} and checked it out, but the stash could not be applied: \
                 {}",
                lowercase_first(&why)
            ),
            None => format!(
                "Created branch {name} and checked it out, but the stash could not be applied. \
                 The stash is kept."
            ),
        }
    } else {
        recognised(&["checkout"], &detail)
            .unwrap_or_else(|| git_failed(args, out.code, &detail).message)
    };
    Step::Failed {
        message,
        detail,
        hints: Vec::new(),
    }
}

/// `text` with its first letter in lower case, to follow a colon.
fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_lowercase().chain(chars).collect())
}

/// The step for a stash that went between the turn's check and the command.
fn gone_step() -> Step {
    Step::Failed {
        message: GONE.to_owned(),
        detail: String::new(),
        hints: Vec::new(),
    }
}

/// git's output, stderr then stdout: a stash that fails to apply says why on stderr and
/// shows the working tree's status on stdout.
fn detail(out: &GitOutput) -> String {
    format!("{}\n{}", out.stderr.trim_end(), out.text().trim_end())
        .trim()
        .to_owned()
}

/// The failed step for `out`, from git run with `args`, which put a stash back (`put_back`)
/// or did something else (`None`). The failures of putting a stash back are told in its
/// terms, saying that the stash is kept; others as [`recognised`], else git's first line.
/// git's "Already up to date" and "nothing added to commit" are not taken as nothing to do
/// here: a stash that failed to apply prints them too.
fn failed(args: &[&str], out: &GitOutput, put_back: Option<PutBack>) -> Step {
    let detail = detail(out);
    let message = put_back
        .and_then(|how| put_back_failure(&detail, how))
        .or_else(|| recognised(args, &detail))
        .unwrap_or_else(|| git_failed(args, out.code, &detail).message);
    Step::Failed {
        message,
        detail,
        hints: Vec::new(),
    }
}

/// The explanation of a stash that could not be put back (`how`), from git's output `all`.
fn put_back_failure(all: &str, how: PutBack) -> Option<String> {
    let kept = match how {
        PutBack::Apply => "The stash is kept.",
        PutBack::Pop => "The stash is kept, not dropped: drop it once you no longer need it.",
    };
    let doing = match how {
        PutBack::Apply => "Applying the stash",
        PutBack::Pop => "Popping the stash",
    };
    let conflicts: Vec<&str> = all
        .lines()
        .filter(|l| l.starts_with("CONFLICT ("))
        .map(|l| {
            l.split_once("Merge conflict in ")
                .map_or("", |(_, f)| f.trim())
        })
        .collect();
    if !conflicts.is_empty() {
        let named: Vec<&str> = conflicts
            .iter()
            .copied()
            .filter(|f| !f.is_empty())
            .collect();
        let files = if named.len() == conflicts.len() {
            file_list(&named)
        } else {
            format!("{} files", conflicts.len())
        };
        return Some(format!(
            "{doing} ran into conflicts in {files}, left in the working tree for you to \
             resolve. {kept}"
        ));
    }
    let mut lines = all.lines();
    if lines.any(|l| l.contains("Your local changes to the following files would be overwritten")) {
        let files: Vec<&str> = lines
            .take_while(|l| l.starts_with('\t'))
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .collect();
        return Some(format!(
            "Your uncommitted changes to {} would be overwritten by the stash, so nothing was \
             applied. Commit them, or stash them too, first. {kept}",
            file_list(&files)
        ));
    }
    let untracked: Vec<&str> = all
        .lines()
        .filter_map(|l| l.trim().strip_suffix(" already exists, no checkout"))
        .collect();
    if !untracked.is_empty() {
        return Some(format!(
            "Untracked files would be overwritten by the stash's untracked files: {}. Move \
             or delete them first. {kept}",
            file_list(&untracked)
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(stderr: &str, stdout: &str) -> GitOutput {
        GitOutput {
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.to_owned(),
            code: 1,
        }
    }

    fn message(step: Step) -> String {
        match step {
            Step::Failed { message, .. } => message,
            other => panic!("not failed: {other:?}"),
        }
    }

    /// git's output when a stash's untracked files are in the way says "Already up to date"
    /// and "nothing added to commit": still a failure, told as such.
    #[test]
    fn untracked_files_in_the_way_are_no_success() {
        let o = out(
            "u2 already exists, no checkout\nerror: could not restore untracked files from \
             stash\n",
            "Already up to date.\nOn branch main\nnothing added to commit but untracked files \
             present (use \"git add\" to track)\n",
        );
        let m = message(failed(
            &["stash", "pop", "stash@{0}"],
            &o,
            Some(PutBack::Pop),
        ));
        assert!(m.starts_with("Untracked files would be overwritten"), "{m}");
        assert!(m.contains("u2") && m.contains("not dropped"), "{m}");
    }

    #[test]
    fn conflicts_say_the_stash_is_kept() {
        let o = out(
            "The stash entry is kept in case you need it again.\n",
            "Auto-merging a\nCONFLICT (content): Merge conflict in a\n",
        );
        let m = message(failed(
            &["stash", "apply", "stash@{0}"],
            &o,
            Some(PutBack::Apply),
        ));
        assert_eq!(
            m,
            "Applying the stash ran into conflicts in a, left in the working tree for you to \
             resolve. The stash is kept."
        );
    }

    #[test]
    fn other_failures_lead_with_gits_line() {
        let o = out("fatal: log for 'stash' only has 3 entries\n", "");
        let m = message(failed(&["stash", "drop", "stash@{9}"], &o, None));
        assert_eq!(m, "fatal: log for 'stash' only has 3 entries");
    }
}
