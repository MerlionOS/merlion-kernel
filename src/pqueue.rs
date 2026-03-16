/// Generic priority queue (binary min-heap) for MerlionOS kernel use.
///
/// Provides `PriorityQueue<T>` with stable ordering (FIFO among equal
/// priorities) and `TimerQueue` for scheduling delayed callback events.
/// All operations are O(log n) unless noted otherwise.

use alloc::vec::Vec;

/// A single entry stored inside the priority queue.
///
/// Lower `priority` values are dequeued first.  When two entries share the
/// same priority, the one inserted earlier (lower `insertion_order`) wins,
/// giving FIFO / stable behaviour.
#[derive(Debug, Clone)]
pub struct HeapEntry<T> {
    /// The payload.
    pub item: T,
    /// Numeric priority — lower means higher urgency.
    pub priority: i64,
    /// Monotonic counter set at insertion time for tie-breaking.
    pub insertion_order: u64,
}

impl<T> HeapEntry<T> {
    /// Returns `true` when `self` should be dequeued before `other`.
    fn is_higher_priority(&self, other: &Self) -> bool {
        if self.priority != other.priority {
            self.priority < other.priority
        } else {
            self.insertion_order < other.insertion_order
        }
    }
}

/// Binary min-heap priority queue.
///
/// `push` and `pop` run in O(log n).  `peek`, `len`, and `is_empty` are O(1).
/// `update_priority` and `remove` operate by heap index and are O(log n).
pub struct PriorityQueue<T> {
    pub(crate) heap: Vec<HeapEntry<T>>,
    next_order: u64,
}

impl<T> PriorityQueue<T> {
    /// Create an empty priority queue.
    pub const fn new() -> Self {
        Self {
            heap: Vec::new(),
            next_order: 0,
        }
    }

    /// Number of entries currently in the queue.
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    /// Returns `true` when the queue contains no entries.
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// Insert `item` with the given `priority` (lower = higher urgency).
    pub fn push(&mut self, item: T, priority: i64) {
        let order = self.next_order;
        self.next_order += 1;
        self.heap.push(HeapEntry {
            item,
            priority,
            insertion_order: order,
        });
        self.sift_up(self.heap.len() - 1);
    }

    /// Remove and return the highest-priority (lowest numeric value) item,
    /// or `None` if the queue is empty.
    pub fn pop(&mut self) -> Option<T> {
        if self.heap.is_empty() {
            return None;
        }
        let last = self.heap.len() - 1;
        self.heap.swap(0, last);
        let entry = self.heap.pop().unwrap();
        if !self.heap.is_empty() {
            self.sift_down(0);
        }
        Some(entry.item)
    }

    /// Peek at the highest-priority item without removing it.
    pub fn peek(&self) -> Option<&T> {
        self.heap.first().map(|e| &e.item)
    }

    /// Change the priority of the entry at `index` and restore heap order.
    ///
    /// Returns `false` if `index` is out of bounds.
    pub fn update_priority(&mut self, index: usize, new_priority: i64) -> bool {
        if index >= self.heap.len() {
            return false;
        }
        let old = self.heap[index].priority;
        self.heap[index].priority = new_priority;
        if new_priority < old {
            self.sift_up(index);
        } else if new_priority > old {
            self.sift_down(index);
        }
        true
    }

    /// Remove the entry at `index` and return it, or `None` if out of bounds.
    ///
    /// Runs in O(log n) by swapping with the last element and re-heapifying.
    pub fn remove(&mut self, index: usize) -> Option<T> {
        if index >= self.heap.len() {
            return None;
        }
        let last = self.heap.len() - 1;
        if index == last {
            return self.heap.pop().map(|e| e.item);
        }
        self.heap.swap(index, last);
        let entry = self.heap.pop().unwrap();
        // The swapped element may need to move up or down.
        self.sift_up(index);
        self.sift_down(index);
        Some(entry.item)
    }

    /// Drain all entries in priority order and return them as a `Vec<T>`.
    pub fn drain(&mut self) -> Vec<T> {
        let mut out = Vec::with_capacity(self.heap.len());
        while let Some(item) = self.pop() {
            out.push(item);
        }
        out
    }

    /// Bubble the element at `index` upward until the heap property holds.
    fn sift_up(&mut self, mut index: usize) {
        while index > 0 {
            let parent = (index - 1) / 2;
            if self.heap[index].is_higher_priority(&self.heap[parent]) {
                self.heap.swap(index, parent);
                index = parent;
            } else {
                break;
            }
        }
    }

    /// Push the element at `index` downward until the heap property holds.
    fn sift_down(&mut self, mut index: usize) {
        let len = self.heap.len();
        loop {
            let left = 2 * index + 1;
            let right = 2 * index + 2;
            let mut smallest = index;

            if left < len && self.heap[left].is_higher_priority(&self.heap[smallest]) {
                smallest = left;
            }
            if right < len && self.heap[right].is_higher_priority(&self.heap[smallest]) {
                smallest = right;
            }
            if smallest == index {
                break;
            }
            self.heap.swap(index, smallest);
            index = smallest;
        }
    }
}

impl<T> Default for PriorityQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// A specialised priority queue for scheduling delayed events.
///
/// `schedule()` inserts a callback that expires after a given number of ticks.
/// `poll_expired()` returns all callback IDs whose deadline has passed.
pub struct TimerQueue {
    /// Min-heap ordered by deadline.
    entries: PriorityQueue<u64>,
    /// Current tick maintained by the caller via `advance()` or passed to `poll_expired`.
    current_tick: u64,
}

impl TimerQueue {
    /// Create an empty timer queue starting at tick 0.
    pub const fn new() -> Self {
        Self {
            entries: PriorityQueue::new(),
            current_tick: 0,
        }
    }

    /// Set the current tick value (typically called from the PIT/HPET handler).
    pub fn set_tick(&mut self, tick: u64) {
        self.current_tick = tick;
    }

    /// Return the current tick.
    pub fn current_tick(&self) -> u64 {
        self.current_tick
    }

    /// Schedule a callback to fire `ticks_from_now` ticks in the future.
    ///
    /// `callback_id` is an opaque identifier the caller uses to dispatch the
    /// event once it expires.
    pub fn schedule(&mut self, ticks_from_now: u64, callback_id: u64) {
        let deadline = self.current_tick.saturating_add(ticks_from_now);
        // Use deadline as the priority so earliest deadlines are dequeued first.
        self.entries.push(callback_id, deadline as i64);
    }

    /// Collect and return all callback IDs whose deadline is <= `now`.
    ///
    /// Updates the internal tick to `now` before polling.
    pub fn poll_expired(&mut self, now: u64) -> Vec<u64> {
        self.current_tick = now;
        let mut expired = Vec::new();
        loop {
            match self.entries.heap.first() {
                Some(entry) if entry.priority <= now as i64 => {}
                _ => break,
            }
            if let Some(id) = self.entries.pop() {
                expired.push(id);
            }
        }
        expired
    }

    /// Number of pending (not yet expired) timers.
    pub fn pending(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` when no timers are pending.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Cancel all pending timers, returning their callback IDs in deadline order.
    pub fn drain(&mut self) -> Vec<u64> {
        self.entries.drain()
    }
}

impl Default for TimerQueue {
    fn default() -> Self {
        Self::new()
    }
}
