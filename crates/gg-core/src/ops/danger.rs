//! Which dangers a request carries, given the repository's state: the paths that cannot
//! run unconfirmed (force push, hard reset, deleting an unmerged branch, deleting a remote
//! branch, dropping a stash). Checked by the core, in the operation's turn, so the state
//! looked at is the state the operation runs on.

use crate::dto::{Danger, OpRequest};
use crate::error::Result;
use crate::ops::OpCtx;

/// The dangers `request` would run into now. The operations of this task carry none; the
/// operations that do read the repository here as they are built (Tasks 2, 3 and 5).
pub(crate) async fn dangers(ctx: &OpCtx<'_>, request: &OpRequest) -> Result<Vec<Danger>> {
    let _ = ctx;
    Ok(match request {
        OpRequest::CreateBranch { .. } => Vec::new(),
        _ => Vec::new(),
    })
}
