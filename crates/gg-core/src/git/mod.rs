//! Everything that runs the git CLI.

pub mod details;
pub mod log;
pub mod names;
pub mod progress;
pub mod refs;
pub mod runner;
pub mod stash;
pub mod status;
pub mod worktrees;

pub use runner::{CmdKind, CommandLog, CommandRecord, GitOutput, GitRunner};
