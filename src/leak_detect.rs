/// Memory leak detection module for MerlionOS.
///
/// Tracks heap allocations and deallocations at runtime, allowing the kernel
/// to identify suspected memory leaks — allocations that have not been freed
/// within a configurable tick threshold.  Each allocation is tagged with the
/// originating module name so that per-module statistics can be produced.
///
/// The tracker is guarded by a [`spin::Mutex`] and is safe to call from
/// interrupt-disabled kernel contexts.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::timer;

/// A single tracked heap allocation.
#[derive(Debug, Clone)]
pub struct AllocationRecord {
    /// Virtual address returned by the allocator.
    pub address: u64,
    /// Size of the allocation in bytes.
    pub size: usize,
    /// Kernel module that requested the allocation.
    pub module: &'static str,
    /// PIT tick at which the allocation was recorded.
    pub tick: u64,
}

/// Global allocation tracker protected by a spin-lock.
static TRACKER: Mutex<Vec<AllocationRecord>> = Mutex::new(Vec::new());

/// Cumulative number of allocations that have been tracked since boot.
static TOTAL_TRACKED: AtomicU64 = AtomicU64::new(0);

/// Cumulative number of allocations that have been freed since boot.
static TOTAL_FREED: AtomicU64 = AtomicU64::new(0);

/// Record a new heap allocation.
///
/// Call this immediately after a successful allocation.  `addr` is the
/// pointer value, `size` is the byte count, and `module` identifies the
/// requesting kernel subsystem (e.g. `"vfs"`, `"net"`, `"task"`).
pub fn track_alloc(addr: u64, size: usize, module: &'static str) {
    let record = AllocationRecord {
        address: addr,
        size,
        module,
        tick: timer::ticks(),
    };
    TRACKER.lock().push(record);
    TOTAL_TRACKED.fetch_add(1, Ordering::Relaxed);
}

/// Remove the record for the allocation at `addr`.
///
/// Returns `true` if the address was found and removed, `false` otherwise
/// (which may indicate a double-free or an untracked allocation).
pub fn track_free(addr: u64) -> bool {
    let mut tracker = TRACKER.lock();
    if let Some(pos) = tracker.iter().position(|r| r.address == addr) {
        tracker.swap_remove(pos);
        TOTAL_FREED.fetch_add(1, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// Return the total number of allocations that have been tracked since boot.
pub fn total_tracked() -> u64 {
    TOTAL_TRACKED.load(Ordering::Relaxed)
}

/// Return the total number of allocations that have been freed since boot.
pub fn total_freed() -> u64 {
    TOTAL_FREED.load(Ordering::Relaxed)
}

/// Return the number of allocations currently outstanding (not yet freed).
pub fn outstanding() -> usize {
    TRACKER.lock().len()
}

/// Find allocations older than `age_threshold_ticks` PIT ticks.
///
/// An allocation whose age (`current_tick - record.tick`) exceeds the
/// threshold is considered a suspected leak.  The returned vector is a
/// clone of the matching records so that the lock is not held by the
/// caller.
pub fn find_leaks(age_threshold_ticks: u64) -> Vec<AllocationRecord> {
    let now = timer::ticks();
    let tracker = TRACKER.lock();
    tracker
        .iter()
        .filter(|r| now.saturating_sub(r.tick) > age_threshold_ticks)
        .cloned()
        .collect()
}

/// Produce a human-readable report of suspected memory leaks.
///
/// Allocations older than 500 ticks (~5 s at 100 Hz PIT) are flagged.
/// The report includes per-record details and aggregate counters.
pub fn leak_report() -> String {
    const LEAK_THRESHOLD: u64 = 500;

    let leaks = find_leaks(LEAK_THRESHOLD);
    let tracked = total_tracked();
    let freed = total_freed();
    let live = outstanding();

    let mut buf = String::with_capacity(512);

    let _ = writeln!(buf, "=== MerlionOS Leak Report ===");
    let _ = writeln!(buf, "Total tracked : {}", tracked);
    let _ = writeln!(buf, "Total freed   : {}", freed);
    let _ = writeln!(buf, "Outstanding   : {}", live);
    let _ = writeln!(
        buf,
        "Leak threshold: {} ticks ({} s)",
        LEAK_THRESHOLD,
        LEAK_THRESHOLD / timer::PIT_FREQUENCY_HZ
    );
    let _ = writeln!(buf, "Suspected leaks: {}", leaks.len());
    let _ = writeln!(buf, "-----------------------------");

    if leaks.is_empty() {
        let _ = writeln!(buf, "(none)");
    } else {
        for (i, rec) in leaks.iter().enumerate() {
            let age = timer::ticks().saturating_sub(rec.tick);
            let _ = writeln!(
                buf,
                " {}: addr=0x{:016X}  size={:<8} module={:<12} age={} ticks",
                i + 1,
                rec.address,
                rec.size,
                rec.module,
                age,
            );
        }
    }

    let _ = writeln!(buf, "=============================");
    buf
}

/// Allocation statistics for a single kernel module.
#[derive(Debug, Clone)]
pub struct ModuleStats {
    /// Module name.
    pub module: &'static str,
    /// Number of live (outstanding) allocations.
    pub count: usize,
    /// Total bytes across live allocations.
    pub total_bytes: usize,
}

/// Return per-module allocation statistics for all currently outstanding
/// allocations.
///
/// The returned vector is sorted by total bytes descending so the biggest
/// consumers appear first.
pub fn module_summary() -> Vec<ModuleStats> {
    let tracker = TRACKER.lock();

    // Collect into a small vec of (module, count, bytes).
    let mut stats: Vec<ModuleStats> = Vec::new();
    for rec in tracker.iter() {
        if let Some(entry) = stats.iter_mut().find(|s| s.module == rec.module) {
            entry.count += 1;
            entry.total_bytes += rec.size;
        } else {
            stats.push(ModuleStats {
                module: rec.module,
                count: 1,
                total_bytes: rec.size,
            });
        }
    }

    // Sort biggest consumers first.
    stats.sort_by(|a, b| b.total_bytes.cmp(&a.total_bytes));
    stats
}

/// Format the module summary as a printable table.
pub fn module_summary_report() -> String {
    let stats = module_summary();
    let mut buf = String::with_capacity(256);

    let _ = writeln!(buf, "=== Per-Module Allocation Summary ===");
    let _ = writeln!(buf, " {:<14} {:>6} {:>10}", "MODULE", "COUNT", "BYTES");
    let _ = writeln!(buf, " {:-<14} {:-^6} {:-^10}", "", "", "");

    for s in &stats {
        let _ = writeln!(buf, " {:<14} {:>6} {:>10}", s.module, s.count, s.total_bytes);
    }

    if stats.is_empty() {
        let _ = writeln!(buf, " (no outstanding allocations)");
    }

    let _ = writeln!(buf, "=====================================");
    buf
}
