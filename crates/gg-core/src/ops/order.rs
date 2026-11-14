//! The order numbered requests join a repository's operation queue in (R28). The UI numbers
//! each `run_op` it sends to a repository (1, 2, 3…), because two requests sent in the same
//! instant can reach the core in either order. A request joins the queue once every number
//! before its own has joined it (or was refused before it could); one whose predecessor has
//! not arrived waits for it up to [`ORDER_WAIT`], then joins anyway, and the numbers it
//! skipped are not waited for again. A repository opened again starts from 1.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use crate::lock_ignoring_poison as lock;

/// Longest a numbered request waits for the ones numbered before it.
pub(crate) const ORDER_WAIT: Duration = Duration::from_millis(500);

/// Which numbers have joined a repository's queue.
#[derive(Default)]
pub(crate) struct OpOrder {
    state: Mutex<Numbers>,
    changed: Notify,
}

struct Numbers {
    /// The lowest number that has not joined yet: every one below it has.
    next: u64,
    /// Numbers above `next` that have joined.
    joined: BTreeSet<u64>,
}

impl Default for Numbers {
    fn default() -> Self {
        Self {
            next: 1,
            joined: BTreeSet::new(),
        }
    }
}

impl OpOrder {
    /// Waits until every number before `n` has joined, or [`ORDER_WAIT`] has passed. The
    /// returned turn lets the numbers after `n` in once it is dropped: drop it once the
    /// request has its place in the queue (or will never take one).
    pub(crate) async fn wait_for_turn(self: &Arc<Self>, n: u64) -> NumberedTurn {
        let deadline = tokio::time::Instant::now() + ORDER_WAIT;
        let mut skipped = false;
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if lock(&self.state).next >= n {
                break;
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                skipped = true;
                break;
            }
        }
        NumberedTurn {
            order: self.clone(),
            n,
            skipped,
        }
    }

    fn joined(&self, n: u64, skipped: bool) {
        {
            let mut guard = lock(&self.state);
            let numbers = &mut *guard;
            if skipped && n >= numbers.next {
                // The missing ones are not waited for again.
                numbers.next = n;
            }
            if n >= numbers.next {
                numbers.joined.insert(n);
                while numbers.joined.remove(&numbers.next) {
                    numbers.next += 1;
                }
                let next = numbers.next;
                numbers.joined.retain(|&j| j > next);
            }
        }
        self.changed.notify_waiters();
    }
}

/// A numbered request whose predecessors have joined the queue (or were given up on). When
/// dropped, its own number counts as joined.
pub(crate) struct NumberedTurn {
    order: Arc<OpOrder>,
    n: u64,
    skipped: bool,
}

impl Drop for NumberedTurn {
    fn drop(&mut self) {
        self.order.joined(self.n, self.skipped);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn numbers_join_in_order_and_gaps_are_skipped_once() {
        let order = Arc::new(OpOrder::default());
        // 1 goes at once.
        drop(order.wait_for_turn(1).await);
        // 3 waits for 2, then gives up on it after ORDER_WAIT.
        let t = tokio::time::Instant::now();
        drop(order.wait_for_turn(3).await);
        assert_eq!(t.elapsed(), ORDER_WAIT);
        // 4 goes at once; so does the late 2.
        let t = tokio::time::Instant::now();
        drop(order.wait_for_turn(4).await);
        drop(order.wait_for_turn(2).await);
        assert_eq!(t.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn a_waiting_number_goes_once_its_predecessor_joins() {
        let order = Arc::new(OpOrder::default());
        let second = tokio::spawn({
            let order = order.clone();
            async move {
                let t = tokio::time::Instant::now();
                drop(order.wait_for_turn(2).await);
                t.elapsed()
            }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let first = order.wait_for_turn(1).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!second.is_finished(), "2 waits until 1 has joined");
        drop(first);
        assert_eq!(second.await.unwrap(), Duration::from_millis(150));
    }
}
