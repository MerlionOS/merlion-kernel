/// Lock implementations: spinlock, ticket lock, and lock statistics.
/// Demonstrates lock progression from basic spinlocks to fair ticket locks.
/// Used for educational comparison — the kernel uses spin::Mutex in practice.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use alloc::vec::Vec;

// --- Basic Spinlock ---

/// Simple test-and-set spinlock. Not fair (possible starvation).
pub struct Spinlock {
    locked: AtomicBool,
    name: &'static str,
    pub acquires: AtomicU64,
    pub spins: AtomicU64,
}

impl Spinlock {
    pub const fn new(name: &'static str) -> Self {
        Self {
            locked: AtomicBool::new(false),
            name,
            acquires: AtomicU64::new(0),
            spins: AtomicU64::new(0),
        }
    }

    pub fn lock(&self) {
        let mut spin_count = 0u64;
        while self.locked.compare_exchange_weak(
            false, true, Ordering::Acquire, Ordering::Relaxed
        ).is_err() {
            spin_count += 1;
            core::hint::spin_loop();
        }
        self.acquires.fetch_add(1, Ordering::Relaxed);
        self.spins.fetch_add(spin_count, Ordering::Relaxed);
    }

    pub fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }

    pub fn name(&self) -> &str { self.name }
}

// --- Ticket Lock (fair) ---

/// Ticket lock: FIFO ordering, prevents starvation.
pub struct TicketLock {
    next_ticket: AtomicU64,
    now_serving: AtomicU64,
    name: &'static str,
    pub acquires: AtomicU64,
    pub spins: AtomicU64,
}

impl TicketLock {
    pub const fn new(name: &'static str) -> Self {
        Self {
            next_ticket: AtomicU64::new(0),
            now_serving: AtomicU64::new(0),
            name,
            acquires: AtomicU64::new(0),
            spins: AtomicU64::new(0),
        }
    }

    pub fn lock(&self) -> u64 {
        let ticket = self.next_ticket.fetch_add(1, Ordering::Relaxed);
        let mut spin_count = 0u64;
        while self.now_serving.load(Ordering::Acquire) != ticket {
            spin_count += 1;
            core::hint::spin_loop();
        }
        self.acquires.fetch_add(1, Ordering::Relaxed);
        self.spins.fetch_add(spin_count, Ordering::Relaxed);
        ticket
    }

    pub fn unlock(&self) {
        self.now_serving.fetch_add(1, Ordering::Release);
    }

    pub fn name(&self) -> &str { self.name }
}

// --- Lock statistics ---

pub struct LockStats {
    pub name: &'static str,
    pub kind: &'static str,
    pub acquires: u64,
    pub total_spins: u64,
    pub avg_spins: u64,
}

/// Demo: create and exercise both lock types, return stats.
pub fn demo() -> Vec<LockStats> {
    let spin = Spinlock::new("demo-spin");
    let ticket = TicketLock::new("demo-ticket");

    // Exercise each lock 100 times
    for _ in 0..100 {
        spin.lock();
        spin.unlock();

        let _ = ticket.lock();
        ticket.unlock();
    }

    let spin_acq = spin.acquires.load(Ordering::Relaxed);
    let spin_spins = spin.spins.load(Ordering::Relaxed);
    let ticket_acq = ticket.acquires.load(Ordering::Relaxed);
    let ticket_spins = ticket.spins.load(Ordering::Relaxed);

    alloc::vec![
        LockStats {
            name: "demo-spin",
            kind: "spinlock",
            acquires: spin_acq,
            total_spins: spin_spins,
            avg_spins: if spin_acq > 0 { spin_spins / spin_acq } else { 0 },
        },
        LockStats {
            name: "demo-ticket",
            kind: "ticket",
            acquires: ticket_acq,
            total_spins: ticket_spins,
            avg_spins: if ticket_acq > 0 { ticket_spins / ticket_acq } else { 0 },
        },
    ]
}
