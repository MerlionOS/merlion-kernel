/// Syscall latency and frequency statistics for MerlionOS.
/// Tracks per-syscall call counts, total time, min/max/avg latency.
/// Integrates with the syscall dispatcher to measure real execution times.

use alloc::string::String;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, Ordering};

/// Maximum syscall number we track (0..MAX_SYSCALL).
const MAX_SYSCALL: usize = 16;

/// Whether syscall statistics collection is enabled.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Per-syscall statistics.
struct SyscallStat {
    /// Call count.
    count: u64,
    /// Total execution time in timer ticks.
    total_ticks: u64,
    /// Minimum latency observed (ticks).
    min_ticks: u64,
    /// Maximum latency observed (ticks).
    max_ticks: u64,
    /// Total execution time in TSC cycles (for sub-tick resolution).
    total_cycles: u64,
    /// Minimum TSC cycles.
    min_cycles: u64,
    /// Maximum TSC cycles.
    max_cycles: u64,
}

impl SyscallStat {
    const fn new() -> Self {
        Self {
            count: 0,
            total_ticks: 0,
            min_ticks: u64::MAX,
            max_ticks: 0,
            total_cycles: 0,
            min_cycles: u64::MAX,
            max_cycles: 0,
        }
    }

    fn record(&mut self, ticks: u64, cycles: u64) {
        self.count += 1;
        self.total_ticks += ticks;
        self.total_cycles += cycles;
        if ticks < self.min_ticks { self.min_ticks = ticks; }
        if ticks > self.max_ticks { self.max_ticks = ticks; }
        if cycles < self.min_cycles { self.min_cycles = cycles; }
        if cycles > self.max_cycles { self.max_cycles = cycles; }
    }

    fn avg_cycles(&self) -> u64 {
        if self.count > 0 { self.total_cycles / self.count } else { 0 }
    }
}

static STATS: Mutex<[SyscallStat; MAX_SYSCALL]> = Mutex::new([const { SyscallStat::new() }; MAX_SYSCALL]);

/// Syscall number to name mapping.
fn syscall_name(num: usize) -> &'static str {
    match num {
        0 => "write",
        1 => "exit",
        2 => "yield",
        3 => "getpid",
        4 => "sleep",
        5 => "send",
        6 => "recv",
        7 => "getuid",
        8 => "setuid",
        9 => "getgid",
        10 => "setgid",
        11 => "getgroups",
        12 => "chmod",
        13 => "chown",
        14 => "access",
        _ => "unknown",
    }
}

/// Read the TSC (Time Stamp Counter) for cycle-accurate timing.
#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Enable syscall statistics collection.
pub fn enable() {
    reset();
    ENABLED.store(true, Ordering::SeqCst);
    crate::serial_println!("[syscall_stats] enabled");
}

/// Disable syscall statistics collection.
pub fn disable() {
    ENABLED.store(false, Ordering::SeqCst);
    crate::serial_println!("[syscall_stats] disabled");
}

/// Check if enabled.
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Reset all statistics.
pub fn reset() {
    let mut stats = STATS.lock();
    for s in stats.iter_mut() {
        *s = SyscallStat::new();
    }
}

/// Record the start of a syscall. Returns (tick, tsc) for the caller to pass to `end()`.
pub fn begin() -> (u64, u64) {
    if !ENABLED.load(Ordering::Relaxed) { return (0, 0); }
    (crate::timer::ticks(), read_tsc())
}

/// Record the end of a syscall with its number and the start timestamps.
pub fn end(syscall_num: u64, start: (u64, u64)) {
    if !ENABLED.load(Ordering::Relaxed) { return; }
    let num = syscall_num as usize;
    if num >= MAX_SYSCALL { return; }

    let end_tick = crate::timer::ticks();
    let end_tsc = read_tsc();

    let elapsed_ticks = end_tick.saturating_sub(start.0);
    let elapsed_cycles = end_tsc.saturating_sub(start.1);

    if let Some(mut stats) = STATS.try_lock() {
        stats[num].record(elapsed_ticks, elapsed_cycles);
    }
}

/// Format a summary report of all syscall statistics.
pub fn report() -> String {
    let stats = STATS.lock();

    let mut out = String::from("=== Syscall Latency Statistics ===\n");
    out.push_str(&format!(
        "{:<12} {:>8} {:>12} {:>12} {:>12} {:>12}\n",
        "Syscall", "Calls", "Tot Cycles", "Avg Cycles", "Min Cycles", "Max Cycles"
    ));
    out.push_str(&format!("{}\n", "-".repeat(72)));

    let mut total_calls: u64 = 0;
    let mut total_cycles: u64 = 0;

    for i in 0..MAX_SYSCALL {
        let s = &stats[i];
        if s.count == 0 { continue; }

        total_calls += s.count;
        total_cycles += s.total_cycles;

        out.push_str(&format!(
            "{:<12} {:>8} {:>12} {:>12} {:>12} {:>12}\n",
            syscall_name(i),
            s.count,
            s.total_cycles,
            s.avg_cycles(),
            if s.min_cycles == u64::MAX { 0 } else { s.min_cycles },
            s.max_cycles,
        ));
    }

    out.push_str(&format!("{}\n", "-".repeat(72)));
    out.push_str(&format!("Total: {} calls, {} cycles\n", total_calls, total_cycles));

    // Top syscalls by frequency
    out.push_str("\nTop syscalls by frequency:\n");
    let mut sorted: alloc::vec::Vec<(usize, u64)> = (0..MAX_SYSCALL)
        .filter(|&i| stats[i].count > 0)
        .map(|i| (i, stats[i].count))
        .collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    for (i, (num, count)) in sorted.iter().enumerate() {
        if i >= 5 { break; }
        let pct = if total_calls > 0 { (*count * 100) / total_calls } else { 0 };
        out.push_str(&format!("  {}: {} ({}%)\n", syscall_name(*num), count, pct));
    }

    // Slowest syscalls by avg latency
    out.push_str("\nSlowest syscalls (avg cycles):\n");
    let mut by_latency: alloc::vec::Vec<(usize, u64)> = (0..MAX_SYSCALL)
        .filter(|&i| stats[i].count > 0)
        .map(|i| (i, stats[i].avg_cycles()))
        .collect();
    by_latency.sort_by(|a, b| b.1.cmp(&a.1));

    for (i, (num, avg)) in by_latency.iter().enumerate() {
        if i >= 5 { break; }
        out.push_str(&format!("  {}: {} avg cycles\n", syscall_name(*num), avg));
    }

    out
}

/// Get a one-line summary for a specific syscall.
pub fn syscall_info(num: usize) -> String {
    if num >= MAX_SYSCALL { return String::from("invalid syscall number"); }

    let stats = STATS.lock();
    let s = &stats[num];

    if s.count == 0 {
        return format!("{}: no calls recorded", syscall_name(num));
    }

    format!(
        "{}: {} calls, avg {} cycles, min {} max {}",
        syscall_name(num),
        s.count,
        s.avg_cycles(),
        if s.min_cycles == u64::MAX { 0 } else { s.min_cycles },
        s.max_cycles,
    )
}
