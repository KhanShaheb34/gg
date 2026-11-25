//! Open repositories: their in-memory state, the background log loader and the manager
//! that ties them together.

mod fetch;
mod loader;
mod manager;
mod pr_details;
pub(crate) mod prs;
pub mod state;

pub(crate) use manager::{Repo, canonical, canonical_paths, unknown_repo};
pub use manager::{RepoManager, WorktreeMemory};
