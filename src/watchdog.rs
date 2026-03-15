/// Kernel watchdog for MerlionOS stability.
///
/// Monitors system health by requiring periodic "pet" calls from healthy
/// subsystems. If no pet arrives within the configured timeout, the watchdog
/// escalates through warning, recovery (killing runaway tasks), and ultimately
/// a forced reboot. Pluggable health checks allow subsystems to register
/// liveness probes that the watchdog aggregates on demand.

#[allow(dead_code)]

use alloc::boxed::Box;
use alloc::string::String;
use alloc::format;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use spin::Mutex;
use crate::timer;
use crate::task;
use crate::power;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of registered health checks.
const MAX_HEALTH_CHECKS: usize = 16;

/// Default watchdog timeout in seconds when none is specified.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Number of consecutive missed deadlines before attempting recovery.
const WARNING_THRESHOLD: u64 = 1;

/// Number of consecutive missed deadlines before forcing a reboot.
const RECOVERY_THRESHOLD: u64 = 3;

// ---------------------------------------------------------------------------
// Watchdog core state
// ---------------------------------------------------------------------------

/// Watchdog timeout expressed in PIT ticks.
static TIMEOUT_TICKS: AtomicU64 = AtomicU64::new(0);

/// Tick value recorded at the most recent `pet()` call.
static LAST_PET_TICK: AtomicU64 = AtomicU64::new(0);

/// Whether the watchdog is armed and monitoring.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Consecutive check cycles where the watchdog was not pet in time.
static MISS_COUNT: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// Cumulative watchdog statistics since boot.
pub struct WatchdogStats {
    /// Total number of successful pet() calls.
    pub pet_count: u64,
    /// Total warnings emitted (timeout detected but not yet critical).
    pub warning_count: u64,
    /// Total recovery actions taken (runaway task kills).
    pub recovery_count: u64,
    /// Total forced reboots triggered (should be 0 or 1 in practice).
    pub reboot_count: u64,
}

static PET_COUNT: AtomicU64 = AtomicU64::new(0);
static WARNING_COUNT: AtomicU64 = AtomicU64::new(0);
static RECOVERY_COUNT: AtomicU64 = AtomicU64::new(0);
static REBOOT_COUNT: AtomicU64 = AtomicU64::new(0);

/// Return a snapshot of watchdog statistics.
pub fn stats() -> WatchdogStats {
    WatchdogStats {
        pet_count: PET_COUNT.load(Ordering::Relaxed),
        warning_count: WARNING_COUNT.load(Ordering::Relaxed),
        recovery_count: RECOVERY_COUNT.load(Ordering::Relaxed),
        reboot_count: REBOOT_COUNT.load(Ordering::Relaxed),
    }
}

// ---------------------------------------------------------------------------
// Health-check trait and registry
// ---------------------------------------------------------------------------

/// Trait for subsystem liveness probes.
///
/// Implementors return `true` when the subsystem is healthy and `false` when
/// something is wrong. The watchdog will aggregate all registered probes when
/// `run_health_checks()` is called.
pub trait HealthCheck: Send + Sync {
    /// Return `true` if the subsystem is healthy.
    fn check_health(&self) -> bool;
}

/// A named health-check entry.
struct HealthEntry {
    name: &'static str,
    checker: Box<dyn HealthCheck>,
}

/// Global registry of health checks, guarded by a spinlock.
static HEALTH_CHECKS: Mutex<Vec<HealthEntry>> = Mutex::new(Vec::new());

/// Register a new named health check.
///
/// The `checker` will be invoked each time `run_health_checks()` is called.
/// Registration silently fails if the registry is full (MAX_HEALTH_CHECKS).
pub fn register_health_check(name: &'static str, checker: Box<dyn HealthCheck>) {
    let mut checks = HEALTH_CHECKS.lock();
    if checks.len() >= MAX_HEALTH_CHECKS {
        crate::serial_println!("[watchdog] health-check registry full, ignoring '{}'", name);
        return;
    }
    crate::serial_println!("[watchdog] registered health check '{}'", name);
    crate::klog_println!("[watchdog] registered health check '{}'", name);
    checks.push(HealthEntry { name, checker });
}

/// Run every registered health check and return a list of (name, healthy)
/// results. Also logs any failures to the kernel log.
pub fn run_health_checks() -> Vec<(&'static str, bool)> {
    let checks = HEALTH_CHECKS.lock();
    let mut results = Vec::with_capacity(checks.len());
    for entry in checks.iter() {
        let ok = entry.checker.check_health();
        if !ok {
            crate::serial_println!("[watchdog] health check '{}' FAILED", entry.name);
            crate::klog_println!("[watchdog] health check '{}' FAILED", entry.name);
        }
        results.push((entry.name, ok));
    }
    results
}

// ---------------------------------------------------------------------------
// Watchdog API
// ---------------------------------------------------------------------------

/// Initialise the watchdog with the given timeout in seconds.
///
/// Converts the timeout to PIT ticks, records the current tick as the
/// initial pet time, and enables the watchdog. Calling `init` again
/// reconfigures the timeout and resets the timer.
pub fn init(timeout_secs: u64) {
    let secs = if timeout_secs == 0 { DEFAULT_TIMEOUT_SECS } else { timeout_secs };
    let ticks = secs * timer::PIT_FREQUENCY_HZ;

    TIMEOUT_TICKS.store(ticks, Ordering::SeqCst);
    LAST_PET_TICK.store(timer::ticks(), Ordering::SeqCst);
    MISS_COUNT.store(0, Ordering::SeqCst);
    ENABLED.store(true, Ordering::SeqCst);

    crate::serial_println!(
        "[watchdog] armed: timeout = {} s ({} ticks)",
        secs, ticks
    );
    crate::klog_println!("[watchdog] armed: timeout = {} s", secs);
}

/// Pet (reset) the watchdog timer.
///
/// Healthy subsystems should call this periodically to signal that the
/// system is making progress. Resets the consecutive-miss counter.
pub fn pet() {
    if !ENABLED.load(Ordering::SeqCst) {
        return;
    }
    LAST_PET_TICK.store(timer::ticks(), Ordering::SeqCst);
    MISS_COUNT.store(0, Ordering::Relaxed);
    PET_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Disable the watchdog entirely.
pub fn disable() {
    ENABLED.store(false, Ordering::SeqCst);
    crate::serial_println!("[watchdog] disabled");
    crate::klog_println!("[watchdog] disabled");
}

/// Check the watchdog, intended to be called from the timer-tick handler.
///
/// Escalation policy:
///   1. First missed deadline  -> log a warning.
///   2. Continued misses       -> attempt recovery by killing non-kernel tasks.
///   3. Still stuck            -> force a reboot.
pub fn check() {
    if !ENABLED.load(Ordering::SeqCst) {
        return;
    }

    let now = timer::ticks();
    let last = LAST_PET_TICK.load(Ordering::SeqCst);
    let timeout = TIMEOUT_TICKS.load(Ordering::SeqCst);

    if now.wrapping_sub(last) < timeout {
        // Still within the deadline — nothing to do.
        return;
    }

    // Deadline missed — bump the consecutive miss counter.
    let misses = MISS_COUNT.fetch_add(1, Ordering::SeqCst) + 1;

    if misses <= WARNING_THRESHOLD {
        // --- Stage 1: Warning ---
        WARNING_COUNT.fetch_add(1, Ordering::Relaxed);
        crate::serial_println!(
            "[watchdog] WARNING: not pet for {} ticks (timeout {})",
            now.wrapping_sub(last), timeout
        );
        crate::klog_println!("[watchdog] WARNING: missed deadline ({} consecutive)", misses);
    } else if misses <= RECOVERY_THRESHOLD {
        // --- Stage 2: Recovery — kill runaway tasks ---
        RECOVERY_COUNT.fetch_add(1, Ordering::Relaxed);
        crate::serial_println!("[watchdog] RECOVERY: attempting to kill runaway tasks");
        crate::klog_println!("[watchdog] RECOVERY: killing non-kernel tasks");
        attempt_recovery();

        // Give the system a fresh window after recovery.
        LAST_PET_TICK.store(timer::ticks(), Ordering::SeqCst);
    } else {
        // --- Stage 3: Forced reboot ---
        REBOOT_COUNT.fetch_add(1, Ordering::Relaxed);
        crate::serial_println!("[watchdog] FATAL: system unresponsive, forcing reboot");
        crate::klog_println!("[watchdog] FATAL: forced reboot");
        power::reboot(); // diverges — never returns
    }
}

// ---------------------------------------------------------------------------
// Recovery helpers
// ---------------------------------------------------------------------------

/// Attempt recovery by killing all non-kernel tasks that appear to be stuck.
fn attempt_recovery() {
    let tasks = task::list();
    for t in tasks.iter() {
        // Never kill the kernel (pid 0) or the idle task.
        if t.pid == 0 {
            continue;
        }
        crate::serial_println!("[watchdog] killing task '{}' (pid {})", t.name, t.pid);
        crate::klog_println!("[watchdog] killing task '{}' (pid {})", t.name, t.pid);
        let _ = task::kill(t.pid);
    }
}

// ---------------------------------------------------------------------------
// Status display
// ---------------------------------------------------------------------------

/// Format a human-readable status string for display (e.g. in a shell).
pub fn format_status() -> String {
    let enabled = ENABLED.load(Ordering::SeqCst);
    let s = stats();

    if !enabled {
        return format!(
            "[watchdog] disabled | pets: {} | warnings: {} | recoveries: {}",
            s.pet_count, s.warning_count, s.recovery_count,
        );
    }

    let now = timer::ticks();
    let last = LAST_PET_TICK.load(Ordering::SeqCst);
    let timeout = TIMEOUT_TICKS.load(Ordering::SeqCst);
    let elapsed = now.wrapping_sub(last);
    let remaining = if timeout > elapsed { timeout - elapsed } else { 0 };
    let timeout_secs = timeout / timer::PIT_FREQUENCY_HZ;

    format!(
        "[watchdog] ARMED | timeout: {} s | remaining: {} ticks | \
         pets: {} | warnings: {} | recoveries: {}",
        timeout_secs,
        remaining,
        s.pet_count,
        s.warning_count,
        s.recovery_count,
    )
}
