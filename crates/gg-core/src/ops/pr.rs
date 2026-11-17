//! Opening a pull request (`pr_create`) through `gh`, at the end of the everyday routine: push the
//! branch to its namesake on its remote when that is missing or behind, then `gh pr
//! create --head <branch> --base <base> --title <title> --body-file - [--draft]`, the
//! description written to `gh`'s stdin. Runs from the main worktree; needs no working tree.
//! When `gh` cannot make it (not installed, signed out), the branch is still pushed and the
//! outcome is done with the host's compare page to finish on; any other failure says so
//! plainly and offers the compare page too.

use crate::dto::{OpHint, PrUnavailable};
use crate::error::{CoreError, ErrorCode, Result};
use crate::gh::{GhFailure, already_exists, created_url, number_of, unavailable_message};
use crate::git::names::check_branch_name;
use crate::git::runner::{CmdKind, StdinPolicy};
use crate::hosts::compare_url;
use crate::ops::network::{push, ref_commit};
use crate::ops::{OpCtx, Step};
use crate::repo::prs::{PrTrigger, push_plan};

/// The step said (`op-progress`) while `gh pr create` runs.
pub const CREATING: &str = "Creating the pull request…";

/// A `pr_create` request (see [`crate::dto::OpRequest::PrCreate`]).
pub(crate) struct Request<'a> {
    pub branch: &'a str,
    pub base: &'a str,
    pub base_remote: Option<&'a str>,
    pub title: &'a str,
    pub body: &'a str,
    pub draft: bool,
    /// The dialog promised GitHub's compare page: gh is never run.
    pub compare_page: bool,
}

/// Checks a `pr_create` request without git: the branch and the base can be branch names
/// (never option-like), the base remote a remote's name, and the title is one line with
/// something on it.
pub(crate) fn check(
    branch: &str,
    base: &str,
    base_remote: Option<&str>,
    title: &str,
) -> Result<()> {
    check_branch_name(branch)?;
    check_branch_name(base).map_err(|e| CoreError::new(ErrorCode::BadRequest, e.message))?;
    if let Some(remote) = base_remote
        && (remote.is_empty()
            || remote.starts_with('-')
            || remote.contains(|c: char| c.is_whitespace() || c.is_control()))
    {
        return Err(CoreError::new(
            ErrorCode::BadRequest,
            format!("\u{201c}{remote}\u{201d} is not a remote's name."),
        ));
    }
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

/// Opens the pull request `request` asks for (see the module comment).
pub(crate) async fn create(ctx: &OpCtx<'_>, request: Request<'_>) -> Result<Step> {
    let Request {
        branch,
        base,
        base_remote,
        title,
        body,
        draft,
        compare_page,
    } = request;
    if ref_commit(ctx, &format!("refs/heads/{branch}"))
        .await?
        .is_none()
    {
        return Ok(refused(format!(
            "Nothing was opened: there is no local branch named \u{201c}{branch}\u{201d}."
        )));
    }
    let facts = ctx
        .manager
        .remote_facts(ctx.repo, &ctx.runner, &ctx.cwd)
        .await?;
    if let Some(r) = base_remote
        && !facts.is_remote(r)
    {
        return Ok(refused(format!(
            "Nothing was opened: there is no remote named {r}."
        )));
    }
    let base_remote = base_remote.or_else(|| facts.base_remote());
    let plan = push_plan(&ctx.runner, &ctx.cwd, &facts, branch).await?;
    let (Some((remote, behind)), Some(base_remote)) = (plan, base_remote) else {
        return Ok(refused(format!(
            "Nothing was opened: it is not clear which remote to push {branch} to."
        )));
    };
    let Some(on) = ctx
        .manager
        .on_github(ctx.repo, &ctx.runner, &facts, base_remote)
        .await
    else {
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
    let remote = remote.as_str();
    let fork = facts
        .repo_of(remote)
        .filter(|p| !p.owner.eq_ignore_ascii_case(&on.owner))
        .map(|p| p.owner);
    let head = match &fork {
        Some(owner) => format!("{owner}:{branch}"),
        None => branch.to_owned(),
    };
    // The compare page, with what the form had in it: nothing
    // typed is lost when it ends there after all. Not when the dialog showed no form.
    let page = compare_url(&on, base, branch, fork.as_deref());
    let page = if compare_page {
        page
    } else {
        with_text(&page, title, body)
    };
    let compare = vec![OpHint::OpenUrl {
        url: page,
        label: "Open the compare page".to_owned(),
    }];

    // gh looked for before anything is pushed: without it, or signed out (as the list last
    // said), the branch is still pushed and the pull request is finished on GitHub's compare
    // page. Never when the dialog promised that page: gh found
    // since would make a pull request the user never saw the form of.
    let signed_out = ctx.repo.prs_unavailable().await == Some(PrUnavailable::SignedOut);
    let gh = if signed_out || compare_page {
        None
    } else {
        ctx.manager.gh(ctx.repo, &ctx.runner, &on)
    };

    // Pushed first when its namesake there is missing or behind (`push_plan`, as the
    // suggestion said).
    let mut pushed = String::new();
    if behind {
        ctx.step(&format!("Pushing {branch} to {remote}/{branch}…"));
        match push(ctx, branch, remote, None, false, None, None).await? {
            Step::Done(message) => pushed = format!("{message} "),
            Step::DoneWith { message, .. } => pushed = format!("{message} "),
            failed => return Ok(failed),
        }
    }

    let on_github_instead = |compare: Vec<OpHint>| {
        let message = if behind {
            format!("Pushed {branch} to {remote}/{branch}. Finish on GitHub:")
        } else {
            format!("{branch} is on {remote}/{branch} already. Finish on GitHub:")
        };
        Step::DoneWith {
            message,
            detail: None,
            hints: compare,
        }
    };
    let Some(gh) = gh else {
        return Ok(on_github_instead(compare));
    };
    let mut args = vec![
        "pr",
        "create",
        "--repo",
        gh.repo(),
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
    ctx.step(CREATING);
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
            remember_base(ctx, base);
            ctx.manager.spawn_load_prs(ctx.repo, PrTrigger::Created);
            let Some(url) = created_url(&out.text(), &on.host) else {
                return Ok(Step::Done(format!(
                    "{pushed}The pull request was created, but gh did not print its address."
                )));
            };
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
            if let Some(url) = already_exists(&failure.detail(), &on.host) {
                ctx.manager.spawn_load_prs(ctx.repo, PrTrigger::Created);
                let number = number_of(&url).map_or_else(String::new, |n| format!(" #{n}"));
                return Ok(opened(
                    url,
                    format!("{pushed}{branch} has a pull request open already{number}."),
                ));
            }
            // Not signed in: as without gh, finished on the compare page.
            if matches!(failure, GhFailure::SignedOut(_)) {
                return Ok(on_github_instead(compare));
            }
            // Not installed after all: said as such; anything else is gh's own reason.
            let why = match &failure {
                GhFailure::Failed { message, .. } => {
                    format!("gh could not open the pull request: {message}")
                }
                _ => unavailable_message(failure.reason(), Some(&failure)),
            };
            Ok(Step::Failed {
                message: format!("{pushed}{why}"),
                detail: failure.detail(),
                hints: compare,
            })
        }
    }
}

/// Most bytes of a description put into the compare page's address.
const MAX_BODY_IN_URL: usize = 4000;

/// The compare page `page` with `title` and `body` (its first [`MAX_BODY_IN_URL`] bytes, cut
/// at a character) as its `title` and `body` parameters, which GitHub fills the form with.
fn with_text(page: &str, title: &str, body: &str) -> String {
    let mut end = body.len().min(MAX_BODY_IN_URL);
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    let mut url = format!("{page}&title={}", query_encode(title));
    if end > 0 {
        url.push_str(&format!("&body={}", query_encode(&body[..end])));
    }
    url
}

/// `text` percent-encoded for a query value: every byte but letters, digits and `-._~`.
fn query_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
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
