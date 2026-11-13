//! Open repositories: their in-memory state, the background log loader and the manager
//! that ties them together.

mod fetch;
mod loader;
mod manager;
pub mod state;

pub(crate) use manager::Repo;
pub use manager::RepoManager;
