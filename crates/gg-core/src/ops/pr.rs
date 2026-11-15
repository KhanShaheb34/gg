//! Opening a pull request (`pr_create`) through `gh`, as the owner's routine ends: push the
//! branch to its namesake on its remote (R38) when that is missing or behind, then `gh pr
//! create --head <branch> --base <base> --title <title> --body-file - [--draft]`, the
//! description written to `gh`'s stdin. Runs from the main worktree; needs no working tree.
//! When `gh` cannot make it (not installed, signed out, a failure), the outcome says so
//! plainly and offers the host's compare page, which opens one in the browser.

use crate::dto::{OpHint, PrUnavailable};
use crate::error::{CoreError, ErrorCode, Result};
use crate::gh::{
    GhFailure, RemoteFacts, already_exists, created_url, number_of, unavailable_message,
};
use crate::git::names::check_branch_name;
use crate::git::runner::{CmdKind, StdinPolicy};
use crate::hosts::{compare_url, is_github};
use crate::ops::network::{push, ref_commit};
use crate::ops::{OpCtx, Step};
use crate::repo::prs::PrTrigger;

/// Checks a `pr_create` request without git: the branch and the base can be branch names
/// (never option-like), and the title is one line with something on it.
pub(crate) fn check(branch: &str, base: &str, title: &str) -> Result<()> {
    check_branch_name(branch)?;
    check_branch_name(base).map_err(|e| CoreError::new(ErrorCode::BadRequest, e.message))?;
    if title.trim().is_empty() {
        return Err(CoreError::new(
            ErrorCode::BadRequest,
            "A pull request needs a title.",
        ));
    }
    if title.contains(['\n', '\r']) {
        return Err(CoreError::new(
            ErrorCode::BadRequest,
            "A pull request's title is one line: put the rest in the description.",
        ));
    }
    Ok(())
}

/// A failure that pushed nothing and made nothing.
fn refused(message: String) -> Step {
    Step::Failed {
        message,
        detail: String::new(),
        hints: Vec::new(),
    }
}

/// Opens a pull request of `branch` into `base` (see the module comment).
pub(crate) async fn create(
    ctx: &OpCtx<'_>,
    branch: &str,
    base: &str,
    title: &str,
    body: &str,
    draft: bool,
) -> Result<Step> {
    if ref_commit(ctx, &format!("refs/heads/{branch}"))
        .await?
        .is_none()
    {
        return Ok(refused(format!(
            "Nothing was opened: there is no local branch named \u{201c}{branch}\u{201d}."
        )));
    }
    let facts = RemoteFacts::read(&ctx.runner, &ctx.cwd).await?;
    let (Some(remote), Some(base_remote)) = (facts.push_remote(branch), facts.base_remote()) else {
        return Ok(refused(format!(
            "Nothing was opened: it is not clear which remote to push {branch} to."
        )));
    };
    let Some(on) = facts.repo_of(base_remote).filter(|r| is_github(&r.host)) else {
        return Ok(refused(format!(
            "Nothing was opened: {}",
            unavailable_message(PrUnavailable::NotGithub, None)
        )));
    };
    if ref_commit(ctx, &format!("refs/remotes/{base_remote}/{base}"))
        .await?
        .is_none()
    {
        return Ok(refused(format!(
            "Nothing was opened: {base} is not a branch on {base_remote}. Choose another base, \
             or fetch first."
        )));
    }
    // A branch pushed to a fork is offered from there (`<owner>:<branch>`).
    let fork = facts
        .repo_of(remote)
        .filter(|p| !p.owner.eq_ignore_ascii_case(&on.owner))
        .map(|p| p.owner);
    let head = match &fork {
        Some(owner) => format!("{owner}:{branch}"),
        None => branch.to_owned(),
    };
    let compare: Vec<OpHint> = compare_url(&on, base, branch, fork.as_deref())
        .map(|url| OpHint::OpenUrl {
            url,
            label: "Open the compare page".to_owned(),
        })
        .into_iter()
        .collect();

    // Pushed first (R38) when its namesake there is missing or behind.
    let tracking = format!("refs/remotes/{remote}/{branch}");
    let behind = match ref_commit(ctx, &tracking).await? {
        None => true,
        Some(_) => {
            let range = format!("{tracking}..refs/heads/{branch}");
            let out = ctx.read(&["rev-list", "--count", &range, "--"]).await?;
            out.code != 0 || out.text().trim() != "0"
        }
    };
    let mut pushed = String::new();
    if behind {
        match push(ctx, branch, remote, None, false, None, None).await? {
            Step::Done(message) => pushed = format!("{message} "),
            Step::DoneWith { message, .. } => pushed = format!("{message} "),
            failed => return Ok(failed),
        }
    }

    let Some(gh) = ctx.manager.gh(ctx.repo, &ctx.runner, &on) else {
        return Ok(Step::Failed {
            message: format!(
                "{pushed}{}",
                unavailable_message(PrUnavailable::NoGh, Some(&GhFailure::NoGh))
            ),
            detail: String::new(),
            hints: compare,
        });
    };
    let mut args = vec![
        "pr",
        "create",
        "--head",
        &head,
        "--base",
        base,
        "--title",
        title,
        "--body-file",
        "-",
    ];
    if draft {
        args.push("--draft");
    }
    let ran = gh
        .run(
            &args,
            CmdKind::Write,
            StdinPolicy::Bytes(body.as_bytes().to_vec()),
        )
        .await?;
    let opened = |url: String, message: String| {
        let hints = vec![OpHint::OpenUrl {
            url,
            label: "Open pull request".to_owned(),
        }];
        Step::DoneWith {
            message,
            detail: None,
            hints,
        }
    };
    match ran {
        Ok(out) => {
            let Some(url) = created_url(&out.text(), &on.web) else {
                return Ok(Step::Failed {
                    message: format!(
                        "{pushed}gh did not say where the pull request is: look for it on the \
                         repository's page."
                    ),
                    detail: format!("{}\n{}", out.text(), out.stderr).trim().to_owned(),
                    hints: compare,
                });
            };
            remember_base(ctx, base);
            ctx.manager.spawn_load_prs(ctx.repo, PrTrigger::Created);
            let number = number_of(&url).map_or_else(String::new, |n| format!(" #{n}"));
            let draft = if draft { "draft " } else { "" };
            Ok(opened(
                url.clone(),
                format!(
                    "{pushed}Opened {draft}pull request{number} of {branch} into {base}: {url}"
                ),
            ))
        }
        Err(failure) => {
            if let Some(url) = already_exists(&failure.detail(), &on.web) {
                ctx.manager.spawn_load_prs(ctx.repo, PrTrigger::Created);
                let number = number_of(&url).map_or_else(String::new, |n| format!(" #{n}"));
                return Ok(opened(
                    url,
                    format!("{pushed}{branch} has a pull request open already{number}."),
                ));
            }
            Ok(Step::Failed {
                message: format!(
                    "{pushed}{}",
                    unavailable_message(failure.reason(), Some(&failure))
                ),
                detail: failure.detail(),
                hints: compare,
            })
        }
    }
}

/// Remembers `base` as the next pull request's suggested base, in the background.
fn remember_base(ctx: &OpCtx<'_>, base: &str) {
    let Some(memory) = ctx.manager.memory() else {
        return;
    };
    let (root, base) = (ctx.repo.root.clone(), base.to_owned());
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn_blocking(move || memory.remember_pr_base(&root, &base));
    }
}
