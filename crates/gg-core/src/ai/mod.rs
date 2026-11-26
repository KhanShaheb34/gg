//! "Generate with AI": a commit message, or a pull request's title and description, written
//! by a command-line AI tool the user installed and signed in to (Claude Code, Codex), run the
//! way `gh` is: found on the login `PATH`, run as an argument array (no shell, no terminal),
//! logged, given up on after a time limit, ended with everything it started when cancelled.
//! gg never talks to an AI service itself, and never reads, stores or passes on a token: the
//! tool uses its own sign-in.
//!
//! Only on the user's click. What is sent: the prompt (gg's, or the user's template from the
//! settings) with the branch's name, the list of files, and the patch (cut to a budget: see
//! [`prompt`]), on the tool's standard input. The tool runs in a temporary folder of gg's,
//! never in the repository, with its tools off or read-only (see [`tools`]), so it cannot
//! change the repository; gg reads what it answers and fills the field, and nothing is
//! committed or created by it.
//!
//! - [`tools`]: one adapter per tool.
//! - [`prompt`]: the prompts (Orca's) and reading the answers.
//! - [`context`]: what is read from the repository for a prompt.
//! - [`run`]: finding the tools, running one, telling its failures apart, cancelling.

pub(crate) mod context;
pub mod prompt;
pub(crate) mod run;
pub mod tools;

pub(crate) use run::AiState;

use crate::error::{CoreError, ErrorCode, Result};

/// Longest prompt template kept, in characters.
pub const MAX_TEMPLATE: usize = 20_000;

/// Checks a model setting for tool `tool` (an id): a tool gg knows, and a model name that can
/// go on a command line (see [`tools::check_model`]); `None` (back to gg's default) always.
pub fn check_model_setting(tool: &str, model: Option<&str>) -> Result<()> {
    if !tools::ADAPTERS.iter().any(|a| a.tool().id() == tool) {
        return Err(CoreError::new(
            ErrorCode::BadRequest,
            format!("There is no AI tool \u{201c}{tool}\u{201d}."),
        ));
    }
    if let Some(model) = model {
        tools::check_model(model.trim())
            .map_err(|m| CoreError::new(ErrorCode::BadRequest, m))?;
    }
    Ok(())
}

/// Checks a prompt template: at most [`MAX_TEMPLATE`] characters.
pub fn check_prompt(template: &str) -> Result<()> {
    if template.chars().count() > MAX_TEMPLATE {
        return Err(CoreError::new(
            ErrorCode::BadRequest,
            format!("A prompt template is at most {MAX_TEMPLATE} characters."),
        ));
    }
    Ok(())
}
