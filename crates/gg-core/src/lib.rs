//! Git graph core: everything that talks to git and shapes data for the UI.

pub mod dto;
pub mod error;
pub mod git;
pub mod graph;
pub mod repo;
#[cfg(any(test, feature = "testutil"))]
pub mod testutil;

pub use error::{CoreError, ErrorCode, Result};

/// Receives events for the UI (`repo-changed`, `repo-error`). Called from background tasks
/// and request handlers, never while repository state is locked. Must return quickly and
/// must not block on gg-core work: a repository's `repo-changed` events are emitted one at a
/// time, in order.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, name: &str, payload: serde_json::Value);
}

/// The gg-core crate version (`MAJOR.MINOR.PATCH`).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_the_crate_semver() {
        let v = version();
        assert_eq!(v, env!("CARGO_PKG_VERSION"));
        let parts: Vec<&str> = v.split('.').collect();
        assert_eq!(parts.len(), 3, "expected MAJOR.MINOR.PATCH, got {v:?}");
        assert!(
            parts.iter().all(|p| p.parse::<u32>().is_ok()),
            "non-numeric part in {v:?}"
        );
    }
}
