/// Memory allocation tracker for MerlionOS.
/// Records allocation events (alloc/dealloc) with size, caller context,
/// and timestamps to help diagnose leaks and understand memory usage patterns.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Maximum tracked allocations (ring buffer).
const MAX_TRACKED: usize = 256;

/// Whether tracking is active.
static TRACKING_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Total allocations since tracking started.
static TOTAL_ALLOCS: AtomicU64 = AtomicU64::new(0);
/// Total deallocations since tracking started.
static TOTAL_DEALLOCS: AtomicU64 = AtomicU64::new(0);
/// Total bytes allocated since tracking started.
static TOTAL_BYTES_ALLOC: AtomicU64 = AtomicU64::new(0);
/// Total bytes freed since tracking started.
static TOTAL_BYTES_FREED: AtomicU64 = AtomicU64::new(0);
/// Peak concurrent allocated bytes.
static PEAK_BYTES: AtomicU64 = AtomicU64::new(0);
/// Current allocated bytes.
static CURRENT_BYTES: AtomicU64 = AtomicU64::new(0);

/// Size bucket counters for allocation histogram.
/// Buckets: [0-64], [65-256], [257-1024], [1025-4096], [4097+]
static SIZE_BUCKETS: [AtomicU64; 5] = [
    AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
    AtomicU64::new(0), AtomicU64::new(0),
];

/// An allocation event record.
#[derive(Debug, Clone)]
pub struct AllocEvent {
    /// Sequence number.
    pub seq: u64,
    /// Timer tick when event occurred.
    pub timestamp: u64,
    /// Whether this is an allocation (true) or deallocation (false).
    pub is_alloc: bool,
    /// Size in bytes.
    pub size: usize,
    /// Alignment requested.
    pub align: usize,
    /// Address of the allocation.
    pub addr: u64,
    /// PID of the task that triggered it.
    pub pid: usize,
}

static EVENT_SEQ: AtomicU64 = AtomicU64::new(0);
static EVENTS: Mutex<Vec<AllocEvent>> = Mutex::new(Vec::new());

/// Outstanding (not yet freed) allocations for leak detection.
const MAX_OUTSTANDING: usize = 512;

struct OutstandingAlloc {
    addr: u64,
    size: usize,
    timestamp: u64,
    pid: usize,
}

static OUTSTANDING: Mutex<Vec<OutstandingAlloc>> = Mutex::new(Vec::new());

/// Start allocation tracking.
pub fn start() {
    TOTAL_ALLOCS.store(0, Ordering::SeqCst);
    TOTAL_DEALLOCS.store(0, Ordering::SeqCst);
    TOTAL_BYTES_ALLOC.store(0, Ordering::SeqCst);
    TOTAL_BYTES_FREED.store(0, Ordering::SeqCst);
    PEAK_BYTES.store(0, Ordering::SeqCst);
    CURRENT_BYTES.store(0, Ordering::SeqCst);
    EVENT_SEQ.store(0, Ordering::SeqCst);
    for b in &SIZE_BUCKETS { b.store(0, Ordering::SeqCst); }
    if let Some(mut events) = EVENTS.try_lock() { events.clear(); }
    if let Some(mut out) = OUTSTANDING.try_lock() { out.clear(); }
    TRACKING_ACTIVE.store(true, Ordering::SeqCst);
    crate::serial_println!("[alloc_track] tracking started");
}

/// Stop allocation tracking.
pub fn stop() {
    TRACKING_ACTIVE.store(false, Ordering::SeqCst);
    crate::serial_println!("[alloc_track] tracking stopped");
}

/// Check if tracking is active.
pub fn is_active() -> bool {
    TRACKING_ACTIVE.load(Ordering::Relaxed)
}

/// Record an allocation event. Called from the allocator.
pub fn record_alloc(addr: u64, size: usize, align: usize) {
    if !TRACKING_ACTIVE.load(Ordering::Relaxed) { return; }

    TOTAL_ALLOCS.fetch_add(1, Ordering::Relaxed);
    TOTAL_BYTES_ALLOC.fetch_add(size as u64, Ordering::Relaxed);
    let current = CURRENT_BYTES.fetch_add(size as u64, Ordering::Relaxed) + size as u64;

    // Update peak
    let mut peak = PEAK_BYTES.load(Ordering::Relaxed);
    while current > peak {
        match PEAK_BYTES.compare_exchange_weak(peak, current, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(p) => peak = p,
        }
    }

    // Size bucket
    let bucket = match size {
        0..=64 => 0,
        65..=256 => 1,
        257..=1024 => 2,
        1025..=4096 => 3,
        _ => 4,
    };
    SIZE_BUCKETS[bucket].fetch_add(1, Ordering::Relaxed);

    let seq = EVENT_SEQ.fetch_add(1, Ordering::Relaxed);
    let timestamp = crate::timer::ticks();
    let pid = crate::task::current_pid();

    // Record event
    if let Some(mut events) = EVENTS.try_lock() {
        if events.len() >= MAX_TRACKED {
            events.remove(0);
        }
        events.push(AllocEvent {
            seq, timestamp, is_alloc: true, size, align, addr, pid,
        });
    }

    // Track outstanding
    if let Some(mut out) = OUTSTANDING.try_lock() {
        if out.len() < MAX_OUTSTANDING {
            out.push(OutstandingAlloc { addr, size, timestamp, pid });
        }
    }
}

/// Record a deallocation event.
pub fn record_dealloc(addr: u64, size: usize, align: usize) {
    if !TRACKING_ACTIVE.load(Ordering::Relaxed) { return; }

    TOTAL_DEALLOCS.fetch_add(1, Ordering::Relaxed);
    TOTAL_BYTES_FREED.fetch_add(size as u64, Ordering::Relaxed);
    CURRENT_BYTES.fetch_sub(size.min(CURRENT_BYTES.load(Ordering::Relaxed) as usize) as u64, Ordering::Relaxed);

    let seq = EVENT_SEQ.fetch_add(1, Ordering::Relaxed);
    let timestamp = crate::timer::ticks();
    let pid = crate::task::current_pid();

    if let Some(mut events) = EVENTS.try_lock() {
        if events.len() >= MAX_TRACKED {
            events.remove(0);
        }
        events.push(AllocEvent {
            seq, timestamp, is_alloc: false, size, align, addr, pid,
        });
    }

    // Remove from outstanding
    if let Some(mut out) = OUTSTANDING.try_lock() {
        if let Some(pos) = out.iter().position(|o| o.addr == addr) {
            out.remove(pos);
        }
    }
}

/// Get allocation statistics summary.
pub fn stats() -> String {
    let allocs = TOTAL_ALLOCS.load(Ordering::Relaxed);
    let deallocs = TOTAL_DEALLOCS.load(Ordering::Relaxed);
    let bytes_alloc = TOTAL_BYTES_ALLOC.load(Ordering::Relaxed);
    let bytes_freed = TOTAL_BYTES_FREED.load(Ordering::Relaxed);
    let peak = PEAK_BYTES.load(Ordering::Relaxed);
    let current = CURRENT_BYTES.load(Ordering::Relaxed);
    let outstanding = OUTSTANDING.try_lock().map(|o| o.len()).unwrap_or(0);

    let mut out = String::from("=== Allocation Tracker ===\n");
    out.push_str(&format!("Status      : {}\n", if is_active() { "active" } else { "stopped" }));
    out.push_str(&format!("Allocations : {}\n", allocs));
    out.push_str(&format!("Deallocations: {}\n", deallocs));
    out.push_str(&format!("Bytes alloc : {}\n", bytes_alloc));
    out.push_str(&format!("Bytes freed : {}\n", bytes_freed));
    out.push_str(&format!("Current     : {} bytes\n", current));
    out.push_str(&format!("Peak        : {} bytes\n", peak));
    out.push_str(&format!("Outstanding : {} allocations\n", outstanding));
    out.push_str(&format!("Leak suspect: {}\n", if allocs > deallocs { allocs - deallocs } else { 0 }));

    // Histogram
    out.push_str("\nSize distribution:\n");
    let labels = ["0-64B", "65-256B", "257-1KB", "1-4KB", "4KB+"];
    for (i, label) in labels.iter().enumerate() {
        let count = SIZE_BUCKETS[i].load(Ordering::Relaxed);
        out.push_str(&format!("  {:<10}: {}\n", label, count));
    }

    out
}

/// List outstanding (potentially leaked) allocations.
pub fn leaks() -> String {
    let out_lock = match OUTSTANDING.try_lock() {
        Some(o) => o,
        None => return String::from("(lock contention)"),
    };

    if out_lock.is_empty() {
        return String::from("No outstanding allocations.\n");
    }

    let mut result = format!("Outstanding allocations ({}):\n", out_lock.len());
    result.push_str(&format!("{:<18} {:>8} {:>10} {:>5}\n", "Address", "Size", "Tick", "PID"));

    for (i, alloc) in out_lock.iter().enumerate() {
        if i >= 50 {
            result.push_str(&format!("  ... and {} more\n", out_lock.len() - 50));
            break;
        }
        result.push_str(&format!(
            "  0x{:016x} {:>8} {:>10} {:>5}\n",
            alloc.addr, alloc.size, alloc.timestamp, alloc.pid
        ));
    }

    result
}

/// Get recent allocation events.
pub fn recent_events(count: usize) -> String {
    let events = match EVENTS.try_lock() {
        Some(e) => e,
        None => return String::from("(lock contention)"),
    };

    let start = if events.len() > count { events.len() - count } else { 0 };
    let mut out = String::from("Recent allocation events:\n");
    out.push_str(&format!("{:<6} {:>5} {:>10} {:>8} {:>18} {:>5}\n",
        "Seq", "Type", "Tick", "Size", "Address", "PID"));

    for event in events[start..].iter() {
        let typ = if event.is_alloc { "alloc" } else { "free" };
        out.push_str(&format!(
            "{:<6} {:>5} {:>10} {:>8} 0x{:016x} {:>5}\n",
            event.seq, typ, event.timestamp, event.size, event.addr, event.pid
        ));
    }

    out
}

/// Per-PID allocation summary.
pub fn per_pid_stats() -> String {
    let events = match EVENTS.try_lock() {
        Some(e) => e,
        None => return String::from("(lock contention)"),
    };

    // Collect per-pid stats
    let mut pid_stats: Vec<(usize, u64, u64)> = Vec::new(); // (pid, alloc_bytes, free_bytes)

    for event in events.iter() {
        let entry = pid_stats.iter_mut().find(|e| e.0 == event.pid);
        if let Some(entry) = entry {
            if event.is_alloc { entry.1 += event.size as u64; }
            else { entry.2 += event.size as u64; }
        } else {
            if event.is_alloc {
                pid_stats.push((event.pid, event.size as u64, 0));
            } else {
                pid_stats.push((event.pid, 0, event.size as u64));
            }
        }
    }

    let mut out = String::from("Per-PID allocation stats:\n");
    out.push_str(&format!("{:>5} {:>12} {:>12} {:>12}\n", "PID", "Allocated", "Freed", "Net"));
    for (pid, alloc, freed) in &pid_stats {
        let net = *alloc as i64 - *freed as i64;
        out.push_str(&format!("{:>5} {:>12} {:>12} {:>12}\n", pid, alloc, freed, net));
    }

    out
}
