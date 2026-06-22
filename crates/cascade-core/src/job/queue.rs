//! A small, UI-agnostic scheduler that limits how many jobs run at once.
//!
//! It holds opaque items (the GUI uses job ids of type `u64`) and only decides
//! *when* something may start, given a parallelism cap. Actually spawning the
//! processes and updating widgets is the caller's job.

use std::collections::VecDeque;

/// FIFO queue with a concurrency limit.
pub struct Queue<T> {
    max: usize,
    running: usize,
    pending: VecDeque<T>,
}

impl<T> Queue<T> {
    /// Create a queue allowing `max` concurrent jobs (clamped to at least 1).
    pub fn new(max: usize) -> Self {
        Self {
            max: max.max(1),
            running: 0,
            pending: VecDeque::new(),
        }
    }

    /// Update the concurrency cap (clamped to at least 1).
    pub fn set_max(&mut self, max: usize) {
        self.max = max.max(1);
    }

    /// Add an item to the back of the queue.
    pub fn enqueue(&mut self, item: T) {
        self.pending.push_back(item);
    }

    /// Move as many items as there are free slots from pending to running,
    /// returning them so the caller can start them.
    pub fn start_ready(&mut self) -> Vec<T> {
        let mut out = Vec::new();
        while self.running < self.max {
            match self.pending.pop_front() {
                Some(item) => {
                    self.running += 1;
                    out.push(item);
                }
                None => break,
            }
        }
        out
    }

    /// Mark one running job finished, freeing a slot.
    pub fn complete(&mut self) {
        self.running = self.running.saturating_sub(1);
    }

    pub fn running(&self) -> usize {
        self.running
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

impl<T: PartialEq> Queue<T> {
    /// Remove a still-pending item. Returns `true` if it was found.
    pub fn remove(&mut self, item: &T) -> bool {
        if let Some(pos) = self.pending.iter().position(|x| x == item) {
            self.pending.remove(pos);
            true
        } else {
            false
        }
    }

    /// Move a pending item one position earlier (starts sooner).
    pub fn move_up(&mut self, item: &T) -> bool {
        match self.pending.iter().position(|x| x == item) {
            Some(pos) if pos > 0 => {
                self.pending.swap(pos, pos - 1);
                true
            }
            _ => false,
        }
    }

    /// Move a pending item one position later (starts later).
    pub fn move_down(&mut self, item: &T) -> bool {
        match self.pending.iter().position(|x| x == item) {
            Some(pos) if pos + 1 < self.pending.len() => {
                self.pending.swap(pos, pos + 1);
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respects_parallelism_cap() {
        let mut q: Queue<i32> = Queue::new(2);
        for i in 0..5 {
            q.enqueue(i);
        }
        // Only 2 may start initially.
        let first = q.start_ready();
        assert_eq!(first, vec![0, 1]);
        assert_eq!(q.running(), 2);
        assert_eq!(q.pending(), 3);

        // No free slots → nothing new starts.
        assert!(q.start_ready().is_empty());

        // One finishes → exactly one more starts.
        q.complete();
        assert_eq!(q.start_ready(), vec![2]);
        assert_eq!(q.running(), 2);
    }

    #[test]
    fn drains_in_fifo_order() {
        let mut q: Queue<i32> = Queue::new(1);
        for i in 0..3 {
            q.enqueue(i);
        }
        let mut order = Vec::new();
        order.extend(q.start_ready());
        for _ in 0..3 {
            q.complete();
            order.extend(q.start_ready());
        }
        assert_eq!(order, vec![0, 1, 2]);
    }

    #[test]
    fn raising_max_starts_more() {
        let mut q: Queue<i32> = Queue::new(1);
        for i in 0..4 {
            q.enqueue(i);
        }
        assert_eq!(q.start_ready().len(), 1);
        q.set_max(3);
        assert_eq!(q.start_ready().len(), 2); // now 3 running total
        assert_eq!(q.running(), 3);
    }

    #[test]
    fn remove_and_reorder_pending() {
        let mut q: Queue<i32> = Queue::new(1);
        for i in [10, 20, 30] {
            q.enqueue(i);
        }
        // Reorder: move 30 up once → [10, 30, 20].
        assert!(q.move_up(&30));
        // Remove 10.
        assert!(q.remove(&10));
        assert!(!q.remove(&999));
        // Start order now reflects [30, 20].
        let mut order = Vec::new();
        order.extend(q.start_ready());
        q.complete();
        order.extend(q.start_ready());
        assert_eq!(order, vec![30, 20]);
    }

    #[test]
    fn move_bounds_are_safe() {
        let mut q: Queue<i32> = Queue::new(1);
        q.enqueue(1);
        q.enqueue(2);
        assert!(!q.move_up(&1)); // already first
        assert!(!q.move_down(&2)); // already last
    }

    #[test]
    fn max_is_clamped_to_one() {
        let mut q: Queue<i32> = Queue::new(0);
        q.enqueue(7);
        assert_eq!(q.start_ready(), vec![7]);
    }
}
