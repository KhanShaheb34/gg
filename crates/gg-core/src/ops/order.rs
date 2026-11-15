//! The order numbered requests join a repository's operation queue in (R28). The UI numbers
//! each `run_op` it sends to a repository (1, 2, 3…) under a client id of its own (one per
//! page load: a reload or another window starts again from 1 under a new id), because two
//! requests sent in the same instant can reach the core in either order. A request joins
//! the queue once every number before its own (from the same client) has joined it, or was
//! given up (refused before it could, or unreadable). One whose predecessor has not arrived
//! waits for it up to [`ORDER_WAIT`]; then the missing numbers are skipped, and not waited
//! for again, but those that are waiting still go first, in order. A repository opened
//! again starts from 1.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use crate::lock_ignoring_poison as lock;

/// Longest a numbered request waits for the ones numbered before it.
pub(crate) const ORDER_WAIT: Duration = Duration::from_millis(500);

/// Most clients whose numbers a repository keeps; the one heard from least recently goes.
const MAX_CLIENTS: usize = 64;

/// The numbering of each client of one repository.
#[derive(Default)]
pub(crate) struct ClientOrders {
    orders: Mutex<HashMap<String, (u64, Arc<OpOrder>)>>,
    used: std::sync::atomic::AtomicU64,
}

impl ClientOrders {
    /// The numbering of client `client`, fresh the first time.
    pub(crate) fn of(&self, client: &str) -> Arc<OpOrder> {
        let stamp = self.used.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut orders = lock(&self.orders);
        if !orders.contains_key(client) && orders.len() >= MAX_CLIENTS {
            let oldest = orders
                .iter()
                .min_by_key(|(_, (used, _))| *used)
                .map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                orders.remove(&oldest);
            }
        }
        let entry = orders
            .entry(client.to_owned())
            .or_insert_with(|| (stamp, Arc::default()));
        entry.0 = stamp;
        entry.1.clone()
    }
}

/// Which numbers of one client have joined a repository's queue.
#[derive(Default)]
pub(crate) struct OpOrder {
    state: Mutex<Numbers>,
    changed: Notify,
}

struct Numbers {
    /// The lowest number that has not joined yet: every one below it has (or was given up).
    next: u64,
    /// Numbers above `next` that have joined.
    joined: BTreeSet<u64>,
    /// Numbers waiting for their turn.
    waiting: BTreeSet<u64>,
}

impl Default for Numbers {
    fn default() -> Self {
        Self {
            next: 1,
            joined: BTreeSet::new(),
            waiting: BTreeSet::new(),
        }
    }
}

impl Numbers {
    /// Moves `next` past the numbers that have joined.
    fn absorb(&mut self) {
        while self.joined.remove(&self.next) {
            self.next += 1;
        }
        let next = self.next;
        self.joined.retain(|&j| j > next);
    }

    /// Gives up the missing numbers below `n`: those neither joined nor waiting. A waiting
    /// one keeps its place: `next` stops there.
    fn skip_missing_below(&mut self, n: u64) {
        while self.next < n {
            match self.waiting.range(self.next..n).next() {
                Some(&w) if w == self.next => return,
                Some(&w) => self.next = w,
                None => self.next = n,
            }
            self.absorb();
        }
    }
}

impl OpOrder {
    /// Waits until every number before `n` has joined, or was given up after waiting
    /// [`ORDER_WAIT`] for it. The returned turn lets the numbers after `n` in once it is
    /// dropped: drop it once the request has its place in the queue (or will never take one).
    pub(crate) async fn wait_for_turn(self: &Arc<Self>, n: u64) -> NumberedTurn {
        let deadline = tokio::time::Instant::now() + ORDER_WAIT;
        let turn = NumberedTurn {
            order: self.clone(),
            n,
        };
        lock(&self.state).waiting.insert(n);
        let mut gave_up = false;
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut numbers = lock(&self.state);
                let before = numbers.next;
                if gave_up {
                    numbers.skip_missing_below(n);
                }
                if numbers.next >= n {
                    numbers.waiting.remove(&n);
                    break;
                }
                if numbers.next != before {
                    // A number waiting before this one may go now.
                    self.changed.notify_waiters();
                }
            }
            if gave_up {
                changed.await;
            } else if tokio::time::timeout_at(deadline, changed).await.is_err() {
                gave_up = true;
            }
        }
        turn
    }

    /// Gives up number `n` at once: a request that will never join (its arguments could
    /// not be read). The numbers after it do not wait for it.
    pub(crate) fn release(&self, n: u64) {
        self.joined(n);
    }

    /// How many numbered requests are waiting for their turn (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub(crate) fn waiting(&self) -> usize {
        lock(&self.state).waiting.len()
    }

    fn joined(&self, n: u64) {
        {
            let mut numbers = lock(&self.state);
            numbers.waiting.remove(&n);
            if n >= numbers.next {
                numbers.joined.insert(n);
                numbers.absorb();
            }
        }
        self.changed.notify_waiters();
    }
}

/// A numbered request whose predecessors have joined the queue (or were given up on). When
/// dropped, its own number counts as joined; dropped while still waiting (its caller went
/// away), it counts as given up.
pub(crate) struct NumberedTurn {
    order: Arc<OpOrder>,
    n: u64,
}

impl Drop for NumberedTurn {
    fn drop(&mut self) {
        self.order.joined(self.n);
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

    /// Review item 3c: two requests that give up on the same missing number still join in
    /// their numbers' order, whichever arrived (and so gave up) first.
    #[tokio::test(start_paused = true)]
    async fn numbers_that_give_up_on_the_same_gap_join_in_order() {
        let order = Arc::new(OpOrder::default());
        let joined = Arc::new(Mutex::new(Vec::new()));
        let wait = |n: u64| {
            let (order, joined) = (order.clone(), joined.clone());
            tokio::spawn(async move {
                let turn = order.wait_for_turn(n).await;
                lock(&joined).push(n);
                drop(turn);
            })
        };
        // 3 arrives first, 2 a little later; 1 never comes.
        let third = wait(3);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let second = wait(2);
        third.await.unwrap();
        second.await.unwrap();
        assert_eq!(*lock(&joined), [2, 3]);
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
