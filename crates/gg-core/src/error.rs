//! The one error type every gg-core operation returns; it crosses to the UI as JSON.

use serde::Serialize;

use crate::dto::InputField;

/// Machine-readable error category. Serialized as a snake_case string (`"not_a_repo"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotARepo,
    BareRepo,
    PathMissing,
    GitNotFound,
    GitFailed,
    UnknownRepo,
    StaleGeneration,
    BadRequest,
    Io,
    /// A branch or tag name that git would not take, or that is taken.
    BadName,
    /// The repository is busy with something that must finish first.
    Busy,
    /// There was nothing to do (nothing to commit, already up to date).
    NothingToDo,
}

/// An error with a code for the UI to branch on, a one-line `message` for humans and an
/// optional `detail` (for example the full stderr of a failed git command).
#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct CoreError {
    pub code: ErrorCode,
    pub message: String,
    pub detail: Option<String>,
    /// The field of the form the request came from that the error is about, when the core
    /// knows (a create-worktree dialog's location or new branch name). Absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<InputField>,
}

impl CoreError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
            field: None,
        }
    }

    /// This error, about form field `field`.
    pub fn about(self, field: InputField) -> Self {
        Self {
            field: Some(field),
            ..self
        }
    }

    pub fn with_detail(self, d: impl Into<String>) -> Self {
        Self {
            detail: Some(d.into()),
            ..self
        }
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;
