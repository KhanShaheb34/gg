//! The background task that reads a repository's log into its state, batch by batch.

use std::sync::Arc;
use std::sync::atomic::Ordering;
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
    repo.loading.send_replace(Some(slot.load_id));
    let args = slot.pending.as_ref().unwrap_or(&slot.current).log_args();
    // Made here, outside the task, so it also runs if the task is dropped before it starts.
    let guard = LoadGuard {
        repo: repo.clone(),
        load_id: slot.load_id,
        finished: false,
    };
    let task = tokio::spawn(load(guard, sink.clone(), args));
    if let Some(old) = lock(&repo.load_task).replace(task) {
        old.abort();
    }
}

/// Clears `loading` when a load ends without finishing (a panic, an abort) while it is
/// still the newest load, and records it as abandoned so the next refresh reloads.
struct LoadGuard {
    repo: Arc<Repo>,
    load_id: u64,
    finished: bool,
}

impl Drop for LoadGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let (repo, id) = (&self.repo, self.load_id);
        repo.loading.send_if_modified(|loading| {
            if *loading != Some(id) {
                return false; // superseded or closed: someone else owns `loading`
            }
            repo.abandoned_load.store(id, Ordering::SeqCst);
            *loading = None;
            true
        });
    }
}

/// Reads the log and appends each batch while this load is the newest one, then emits a
/// final snapshot with `loading: false` (a failed load also emits `repo-error`).
///
/// The first load of a repository fills the served state directly, emitting progress after
/// the first 500 rows and then at most every 150 ms. A reload fills its pending state while
/// the old rows stay served, and swaps it in (emitting at once) when it has as many rows as
/// the old view or when it finishes, so a reload never shrinks the view mid-scroll.
async fn load(mut guard: LoadGuard, sink: Arc<dyn EventSink>, args: Vec<String>) {
    let repo = guard.repo.clone();
    let load_id = guard.load_id;
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<Commit>>();
    let read = read_log(&repo.runner, &repo.worktree, &args, move |batch| {
        // Fails only once the consumer below has stopped because this load was superseded.
        let _ = tx.send(batch);
    });
    let consume = async {
        let mut progress = Progress::new();
        while let Some(batch) = rx.recv().await {
            let to_emit = {
                let mut guard = repo.slot.write().await;
                let slot = &mut *guard;
                if slot.load_id != load_id {
                    return false;
                }
                let emit = match slot.pending.as_mut() {
                    Some(pending) => {
                        pending.append(batch);
                        let caught_up = pending.total_rows() >= slot.current.total_rows();
                        if caught_up {
                            slot.promote_pending();
                            progress.emitted_now();
                        }
                        caught_up
                    }
                    None => {
                        let shifted = slot.current.append(batch);
                        if shifted {
                            // The uncommitted row was inserted at index 0.
                            slot.current.generation += 1;
                            progress.emitted_now();
                        }
                        shifted || progress.due(slot.current.total_rows())
                    }
                };
                emit.then(|| slot.snapshot_to_emit(&repo.identity, true))
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
        // A reload that never caught up (a smaller view, or a failure) swaps in now.
        slot.promote_pending();
        slot.load_failed = result.is_err();
        repo.loading.send_replace(None);
        guard.finished = true;
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

/// Decides when the first load of a repository reports progress.
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
            self.emitted_now();
        }
        due
    }

    /// Records an event emitted for another reason, so progress waits 150 ms from it.
    fn emitted_now(&mut self) {
        self.last = Some(Instant::now());
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
        p.last = Instant::now().checked_sub(EVENT_INTERVAL);
        assert!(p.due(901));
        assert!(!p.due(902));
        p.emitted_now();
        assert!(!p.due(903));
    }
}
