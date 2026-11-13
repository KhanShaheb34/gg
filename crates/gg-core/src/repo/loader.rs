//! The background task that reads a repository's log into its state, batch by batch.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::EventSink;
use crate::git::log::{Commit, read_log};
use crate::repo::manager::{Repo, Slot, lock};

/// Rows loaded before the first progress event.
const FIRST_EVENT_ROWS: u32 = 500;
/// Least time between progress events.
const EVENT_INTERVAL: Duration = Duration::from_millis(150);

/// Starts loading the log of `slot`'s pending state (or, with none, its current state),
/// superseding any load in progress. Call with the state lock held for writing.
pub(super) fn start_load(repo: &Arc<Repo>, slot: &mut Slot, sink: &Arc<dyn EventSink>) {
    if slot.closed {
        return;
    }
    slot.load_id += 1;
    slot.load_failed = false;
    repo.loading.send_replace(true);
    let args = slot.pending.as_ref().unwrap_or(&slot.current).log_args();
    let task = tokio::spawn(load(repo.clone(), sink.clone(), slot.load_id, args));
    if let Some(old) = lock(&repo.load_task).replace(task) {
        old.abort();
    }
}

/// Reads the log and appends each batch to the state while this load is the newest one.
/// The first batch of a reload swaps the pending state in, so the old rows are served until
/// then. Emits progress after the first 500 rows, then at most every 150 ms, and a final
/// snapshot with `loading: false`; a failed load also emits `repo-error`.
async fn load(repo: Arc<Repo>, sink: Arc<dyn EventSink>, load_id: u64, args: Vec<String>) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<Commit>>();
    let read = read_log(&repo.runner, &repo.worktree, &args, move |batch| {
        // Fails only once the consumer below has stopped because this load was superseded.
        let _ = tx.send(batch);
    });
    let consume = async {
        let mut progress = Progress::new();
        while let Some(batch) = rx.recv().await {
            let to_emit = {
                let mut slot = repo.slot.write().await;
                if slot.load_id != load_id {
                    return false;
                }
                slot.promote_pending();
                slot.current.append(batch);
                progress
                    .due(slot.current.total_rows())
                    .then(|| slot.snapshot_to_emit(&repo.identity, true))
            };
            if let Some(stamped) = to_emit {
                repo.publish(sink.as_ref(), stamped);
            }
        }
        true
    };
    let (result, still_newest) = tokio::join!(read, consume);
    if !still_newest {
        return;
    }
    let to_emit = {
        let mut slot = repo.slot.write().await;
        if slot.load_id != load_id {
            return;
        }
        // A reload whose log came back empty (or failed) still replaces the old state.
        slot.promote_pending();
        slot.load_failed = result.is_err();
        repo.loading.send_replace(false);
        slot.snapshot_to_emit(&repo.identity, false)
    };
    if let Err(error) = result {
        sink.emit(
            "repo-error",
            serde_json::json!({ "repoId": repo.identity.repo_id, "error": error }),
        );
    }
    repo.publish(sink.as_ref(), to_emit);
}

/// Decides when a load reports progress.
struct Progress {
    /// When the last progress event went out; `None` before the first.
    last: Option<Instant>,
}

impl Progress {
    fn new() -> Self {
        Self { last: None }
    }

    /// Whether to emit now, with `rows` loaded.
    fn due(&mut self, rows: u32) -> bool {
        let due = match self.last {
            None => rows >= FIRST_EVENT_ROWS,
            Some(at) => at.elapsed() >= EVENT_INTERVAL,
        };
        if due {
            self.last = Some(Instant::now());
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_waits_for_500_rows_then_throttles() {
        let mut p = Progress::new();
        assert!(!p.due(1));
        assert!(!p.due(499));
        assert!(p.due(500));
        assert!(!p.due(900), "less than 150 ms since the last event");
        p.last = Some(Instant::now() - EVENT_INTERVAL);
        assert!(p.due(901));
        assert!(!p.due(902));
    }
}
