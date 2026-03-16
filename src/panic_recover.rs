/// Panic recovery and resilience for MerlionOS.
/// Provides automatic recovery from non-fatal panics by killing the offending
/// task and logging the crash. Also includes memory guard pages (red zones)
/// and stack overflow protection.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

/// Whether panic recovery mode is enabled.
static RECOVERY_ENABLED: AtomicBool = AtomicBool::new(true);
/// Number of panics recovered from.
static PANIC_COUNT: AtomicU32 = AtomicU32::new(0);
/// Number of unrecoverable panics (kernel task, double panic).
static FATAL_COUNT: AtomicU32 = AtomicU32::new(0);
/// Whether we're currently in a panic handler (to detect double panics).
static IN_PANIC: AtomicBool = AtomicBool::new(false);

const MAX_CRASH_LOG: usize = 32;

/// A crash record.
#[derive(Debug, Clone)]
pub struct CrashRecord {
    /// Timer tick when the crash occurred.
    pub timestamp: u64,
    /// PID of the crashed task.
    pub pid: usize,
    /// Task name.
    pub task_name: String,
    /// Panic message (truncated).
    pub message: String,
    /// Whether recovery was successful.
    pub recovered: bool,
}

static CRASH_LOG: Mutex<Vec<CrashRecord>> = Mutex::new(Vec::new());

/// Enable or disable panic recovery.
pub fn set_recovery(enabled: bool) {
    RECOVERY_ENABLED.store(enabled, Ordering::SeqCst);
    crate::serial_println!("[panic_recover] recovery {}", if enabled { "enabled" } else { "disabled" });
}

/// Check if recovery is enabled.
pub fn is_recovery_enabled() -> bool {
    RECOVERY_ENABLED.load(Ordering::SeqCst)
}

/// Attempt to recover from a panic.
/// Returns true if recovery was successful (task killed, kernel continues).
/// Returns false if this is a fatal panic (kernel task, double panic, etc.).
pub fn try_recover(info: &core::panic::PanicInfo) -> bool {
    // Detect double panic
    if IN_PANIC.swap(true, Ordering::SeqCst) {
        FATAL_COUNT.fetch_add(1, Ordering::Relaxed);
        return false;
    }

    if !RECOVERY_ENABLED.load(Ordering::SeqCst) {
        IN_PANIC.store(false, Ordering::SeqCst);
        return false;
    }

    let pid = crate::task::current_pid();
    let msg = format!("{}", info);
    let truncated_msg = if msg.len() > 128 { format!("{}...", &msg[..125]) } else { msg };

    // Cannot recover from kernel task panics
    if pid == 0 {
        crate::serial_println!("[panic_recover] FATAL: kernel task panic, cannot recover");
        record_crash(pid, "kernel", &truncated_msg, false);
        FATAL_COUNT.fetch_add(1, Ordering::Relaxed);
        IN_PANIC.store(false, Ordering::SeqCst);
        return false;
    }

    crate::serial_println!("[panic_recover] recovering from panic in pid {}", pid);
    crate::serial_println!("[panic_recover] cause: {}", truncated_msg);

    // Record the crash
    record_crash(pid, "task", &truncated_msg, true);
    PANIC_COUNT.fetch_add(1, Ordering::Relaxed);

    // Kill the offending task
    let _ = crate::task::kill(pid);
    crate::serial_println!("[panic_recover] killed pid {}, system continues", pid);

    IN_PANIC.store(false, Ordering::SeqCst);
    true
}

fn record_crash(pid: usize, task_name: &str, message: &str, recovered: bool) {
    if let Some(mut log) = CRASH_LOG.try_lock() {
        if log.len() >= MAX_CRASH_LOG {
            log.remove(0);
        }
        log.push(CrashRecord {
            timestamp: crate::timer::ticks(),
            pid,
            task_name: task_name.into(),
            message: message.into(),
            recovered,
        });
    }
}

/// Get crash statistics.
pub fn stats() -> String {
    let recovered = PANIC_COUNT.load(Ordering::Relaxed);
    let fatal = FATAL_COUNT.load(Ordering::Relaxed);
    format!(
        "Panic recovery: {}\nRecovered panics: {}\nFatal panics: {}\nTotal crashes: {}",
        if is_recovery_enabled() { "enabled" } else { "disabled" },
        recovered, fatal, recovered + fatal,
    )
}

/// Get crash log.
pub fn crash_log() -> String {
    let log = match CRASH_LOG.try_lock() {
        Some(l) => l,
        None => return String::from("(lock contention)"),
    };

    if log.is_empty() {
        return String::from("No crashes recorded.\n");
    }

    let mut out = format!("Crash log ({} entries):\n", log.len());
    out.push_str(&format!("{:<10} {:>5} {:<10} {:<8} {}\n", "Tick", "PID", "Task", "Status", "Message"));

    for entry in log.iter() {
        let status = if entry.recovered { "OK" } else { "FATAL" };
        let msg = if entry.message.len() > 50 {
            format!("{}...", &entry.message[..47])
        } else {
            entry.message.clone()
        };
        out.push_str(&format!(
            "{:<10} {:>5} {:<10} {:<8} {}\n",
            entry.timestamp, entry.pid, entry.task_name, status, msg
        ));
    }

    out
}

// ---------------------------------------------------------------------------
// Memory Red Zones
// ---------------------------------------------------------------------------

/// Red zone marker bytes.
const RED_ZONE_BYTE: u8 = 0xFE;
const RED_ZONE_SIZE: usize = 16;
const MAX_RED_ZONES: usize = 128;

/// A tracked red zone allocation.
struct RedZoneEntry {
    /// Start address of the red zone (before the user data).
    addr: u64,
    /// Size of the user allocation.
    user_size: usize,
    /// Total allocation (red_zone + user + red_zone).
    total_size: usize,
}

static RED_ZONES: Mutex<Vec<RedZoneEntry>> = Mutex::new(Vec::new());
static RED_ZONE_VIOLATIONS: AtomicUsize = AtomicUsize::new(0);

/// Check all tracked red zones for corruption.
/// Returns a list of (address, violation_type) for corrupted zones.
pub fn check_red_zones() -> Vec<(u64, &'static str)> {
    let zones = match RED_ZONES.try_lock() {
        Some(z) => z,
        None => return Vec::new(),
    };

    let mut violations = Vec::new();

    for zone in zones.iter() {
        let base = zone.addr as *const u8;

        // Check leading red zone
        let mut lead_ok = true;
        for i in 0..RED_ZONE_SIZE {
            let byte = unsafe { *base.add(i) };
            if byte != RED_ZONE_BYTE {
                lead_ok = false;
                break;
            }
        }
        if !lead_ok {
            violations.push((zone.addr, "underflow"));
            RED_ZONE_VIOLATIONS.fetch_add(1, Ordering::Relaxed);
        }

        // Check trailing red zone
        let trail_start = RED_ZONE_SIZE + zone.user_size;
        let mut trail_ok = true;
        for i in 0..RED_ZONE_SIZE {
            let byte = unsafe { *base.add(trail_start + i) };
            if byte != RED_ZONE_BYTE {
                trail_ok = false;
                break;
            }
        }
        if !trail_ok {
            violations.push((zone.addr + trail_start as u64, "overflow"));
            RED_ZONE_VIOLATIONS.fetch_add(1, Ordering::Relaxed);
        }
    }

    violations
}

/// Get red zone check summary.
pub fn red_zone_status() -> String {
    let count = RED_ZONES.try_lock().map(|z| z.len()).unwrap_or(0);
    let violations = RED_ZONE_VIOLATIONS.load(Ordering::Relaxed);
    format!(
        "Red zones tracked: {}\nViolations detected: {}",
        count, violations
    )
}

// ---------------------------------------------------------------------------
// Stack Overflow Protection
// ---------------------------------------------------------------------------

/// Stack guard page size.
const STACK_GUARD_SIZE: usize = 128;
/// Stack guard magic pattern.
const STACK_GUARD_MAGIC: u64 = 0xDEAD_BEEF_CAFE_BABE;

/// Check if any task has corrupted its stack guard.
pub fn check_all_stacks() -> String {
    let corrupted = crate::task::check_stack_guards();

    if corrupted.is_empty() {
        return String::from("All task stacks are healthy.\n");
    }

    let mut out = format!("WARNING: {} task(s) with corrupted stacks!\n", corrupted.len());
    for (pid, name) in &corrupted {
        out.push_str(&format!("  PID {} ({}): stack guard corrupted!\n", pid, name));
        crate::serial_println!("[stack_guard] CORRUPTED: pid {} ({})", pid, name);
    }

    out
}

/// Run all integrity checks (red zones + stack guards).
pub fn integrity_check() -> String {
    let mut out = String::from("=== System Integrity Check ===\n\n");

    // Stack guards
    out.push_str("Stack guards:\n");
    out.push_str(&check_all_stacks());
    out.push_str("\n");

    // Red zones
    out.push_str("Memory red zones:\n");
    let violations = check_red_zones();
    if violations.is_empty() {
        out.push_str("  All red zones intact.\n");
    } else {
        for (addr, kind) in &violations {
            out.push_str(&format!("  VIOLATION at 0x{:x}: {}\n", addr, kind));
        }
    }
    out.push_str("\n");

    // Heap integrity
    let heap_stats = crate::allocator::stats();
    out.push_str(&format!("Heap: {} used / {} total ({} free)\n",
        heap_stats.used, heap_stats.total, heap_stats.free));

    // Panic recovery stats
    out.push_str(&format!("\n{}\n", stats()));

    out
}

/// Initialize panic recovery.
pub fn init() {
    set_recovery(true);
    crate::serial_println!("[panic_recover] initialized");
    crate::klog_println!("[panic_recover] initialized");
}
