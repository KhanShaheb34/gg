//! Everything that runs the git CLI.

pub mod runner;

pub use runner::{CmdKind, CommandLog, CommandRecord, GitOutput, GitRunner};
