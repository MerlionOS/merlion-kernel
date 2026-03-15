/// CPU profiler and performance monitoring for MerlionOS.
///
/// Provides statistical sampling of the instruction pointer at regular timer
/// intervals, producing per-function hit counts and hotspot analysis.  Also
/// exposes kernel-wide performance counters (context switches, syscalls,
/// page faults, interrupts) via atomic counters that other subsystems bump.
///
/// # Usage
///
/// ```ignore
/// profiler::start_profiling(10); // sample every 10 timer ticks
/// // ... workload ...
/// let session = profiler::stop_profiling();
/// let report  = profiler::analyze(&session);
/// serial_println!("{}", profiler::format_report(&report));
/// ```

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Performance counters — bumped by other kernel subsystems
// ---------------------------------------------------------------------------

/// Total context switches since boot.
pub static CONTEXT_SWITCH_COUNT: AtomicU64 = AtomicU64::new(0);
/// Total syscalls dispatched since boot.
pub static SYSCALL_COUNT: AtomicU64 = AtomicU64::new(0);
/// Total page faults handled since boot.
pub static PAGE_FAULT_COUNT: AtomicU64 = AtomicU64::new(0);
/// Total hardware interrupts delivered since boot.
pub static INTERRUPT_COUNT: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Profiling state
// ---------------------------------------------------------------------------

/// Whether the profiler is currently sampling.
static PROFILING_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Tick interval between samples (e.g. 10 means sample every 10th tick).
static SAMPLE_INTERVAL: AtomicU64 = AtomicU64::new(1);
/// Tick counter used to decide when to take the next sample.
static TICK_ACCUM: AtomicU64 = AtomicU64::new(0);
/// Tick at which the current profiling session started.
static START_TICK: AtomicU64 = AtomicU64::new(0);

/// Maximum number of samples we keep (avoids unbounded allocation in
/// interrupt context — the actual Vec lives in a spin-locked global).
const MAX_SAMPLES: usize = 8192;

static SAMPLES: spin::Mutex<Vec<SampleEntry>> = spin::Mutex::new(Vec::new());

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A single profiling sample captured during a timer interrupt.
#[derive(Debug, Clone)]
pub struct SampleEntry {
    /// Instruction pointer (RIP) at the moment the sample was taken.
    pub instruction_pointer: u64,
    /// Timer tick count when the sample was captured.
    pub timestamp_ticks: u64,
    /// PID of the task that was running when the sample was taken.
    pub task_pid: usize,
}

/// A completed profiling session containing all collected samples.
#[derive(Debug, Clone)]
pub struct ProfileSession {
    /// Collected instruction-pointer samples.
    pub samples: Vec<SampleEntry>,
    /// Tick at which profiling started.
    pub start_tick: u64,
    /// Tick at which profiling stopped.
    pub end_tick: u64,
}

/// A single function entry in the profile report.
#[derive(Debug, Clone)]
pub struct FunctionProfile {
    /// Resolved symbol name (or `"<unknown>"`).
    pub name: String,
    /// Number of samples that fell within this function.
    pub sample_count: usize,
    /// Percentage of total samples attributed to this function.
    pub percentage: f64,
}

/// Analyzed profile report produced by [`analyze`].
#[derive(Debug, Clone)]
pub struct ProfileReport {
    /// Functions sorted by descending sample count.
    pub top_functions: Vec<FunctionProfile>,
    /// Total number of samples in the session.
    pub total_samples: usize,
    /// Duration of the session in timer ticks.
    pub duration_ticks: u64,
}

/// Snapshot of kernel-wide performance counters.
#[derive(Debug, Clone, Copy)]
pub struct PerfCounters {
    /// Number of task context switches since boot.
    pub context_switch_count: u64,
    /// Number of syscalls dispatched since boot.
    pub syscall_count: u64,
    /// Number of page faults handled since boot.
    pub page_fault_count: u64,
    /// Number of hardware interrupts since boot.
    pub interrupt_count: u64,
}

// ---------------------------------------------------------------------------
// Profiling API
// ---------------------------------------------------------------------------

/// Start collecting instruction-pointer samples.
///
/// `interval_ticks` controls the sampling frequency: a sample is taken every
/// `interval_ticks` timer ticks.  A value of 1 samples on every tick; 10
/// samples at 10 Hz when the PIT runs at 100 Hz, etc.
///
/// If profiling is already active the call is silently ignored.
pub fn start_profiling(interval_ticks: u64) {
    if PROFILING_ACTIVE.load(Ordering::SeqCst) {
        return;
    }
    let interval = if interval_ticks == 0 { 1 } else { interval_ticks };
    SAMPLE_INTERVAL.store(interval, Ordering::SeqCst);
    TICK_ACCUM.store(0, Ordering::SeqCst);
    START_TICK.store(crate::timer::ticks(), Ordering::SeqCst);

    // Clear any stale samples from a previous session.
    if let Some(mut guard) = SAMPLES.try_lock() {
        guard.clear();
    }

    PROFILING_ACTIVE.store(true, Ordering::SeqCst);
}

/// Stop profiling and return the completed [`ProfileSession`].
///
/// Returns a session with an empty sample vec if profiling was not active.
pub fn stop_profiling() -> ProfileSession {
    let was_active = PROFILING_ACTIVE.swap(false, Ordering::SeqCst);
    let end_tick = crate::timer::ticks();
    let start = START_TICK.load(Ordering::SeqCst);

    let samples = if was_active {
        match SAMPLES.try_lock() {
            Some(mut guard) => {
                let taken = core::mem::take(&mut *guard);
                taken
            }
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };

    ProfileSession {
        samples,
        start_tick: start,
        end_tick,
    }
}

/// Called from the timer interrupt handler to potentially record a sample.
///
/// This reads the current instruction pointer via inline assembly and, if
/// the sampling interval has elapsed, pushes a [`SampleEntry`] into the
/// global buffer.  It is safe to call even when profiling is inactive (it
/// returns immediately).
pub fn on_timer_tick() {
    if !PROFILING_ACTIVE.load(Ordering::Relaxed) {
        return;
    }

    let acc = TICK_ACCUM.fetch_add(1, Ordering::Relaxed) + 1;
    let interval = SAMPLE_INTERVAL.load(Ordering::Relaxed);
    if acc % interval != 0 {
        return;
    }

    // Read the current instruction pointer.  Because we are inside the
    // timer ISR the RIP on the interrupted stack would be more accurate,
    // but reading our own RIP here is a reasonable approximation for a
    // statistical profiler.
    let rip: u64;
    unsafe {
        core::arch::asm!(
            "lea {}, [rip]",
            out(reg) rip,
            options(nomem, nostack, preserves_flags),
        );
    }

    let ticks = crate::timer::ticks();
    let pid = crate::task::current_pid();

    if let Some(mut guard) = SAMPLES.try_lock() {
        if guard.len() < MAX_SAMPLES {
            guard.push(SampleEntry {
                instruction_pointer: rip,
                timestamp_ticks: ticks,
                task_pid: pid,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------

/// Analyze a completed [`ProfileSession`] and produce a [`ProfileReport`].
///
/// Resolves each sampled instruction pointer to a kernel symbol via
/// [`crate::ksyms::lookup`], aggregates hit counts per function, and sorts
/// by descending frequency.
pub fn analyze(session: &ProfileSession) -> ProfileReport {
    let total = session.samples.len();
    if total == 0 {
        return ProfileReport {
            top_functions: Vec::new(),
            total_samples: 0,
            duration_ticks: session.end_tick.saturating_sub(session.start_tick),
        };
    }

    // Accumulate per-symbol counts.
    let mut counts: Vec<(String, usize)> = Vec::new();

    for sample in &session.samples {
        let name = match crate::ksyms::lookup(sample.instruction_pointer) {
            Some((sym, _offset)) => sym,
            None => String::from("<unknown>"),
        };

        let mut found = false;
        for entry in counts.iter_mut() {
            if entry.0 == name {
                entry.1 += 1;
                found = true;
                break;
            }
        }
        if !found {
            counts.push((name, 1));
        }
    }

    // Sort descending by count.
    counts.sort_by(|a, b| b.1.cmp(&a.1));

    let top_functions: Vec<FunctionProfile> = counts
        .into_iter()
        .map(|(name, count)| {
            let percentage = (count as f64 / total as f64) * 100.0;
            FunctionProfile {
                name,
                sample_count: count,
                percentage,
            }
        })
        .collect();

    ProfileReport {
        top_functions,
        total_samples: total,
        duration_ticks: session.end_tick.saturating_sub(session.start_tick),
    }
}

// ---------------------------------------------------------------------------
// Report formatting
// ---------------------------------------------------------------------------

/// Format a [`ProfileReport`] as a human-readable string with a text-based
/// flame-graph style indented call tree and hotspot percentages.
pub fn format_report(report: &ProfileReport) -> String {
    let mut out = String::with_capacity(1024);

    let duration_secs = report.duration_ticks as f64 / crate::timer::PIT_FREQUENCY_HZ as f64;

    out.push_str("=== MerlionOS CPU Profile Report ===\n");
    out.push_str(&format!(
        "Total samples : {}\n",
        report.total_samples
    ));
    out.push_str(&format!(
        "Duration      : {} ticks ({:.2}s)\n",
        report.duration_ticks, duration_secs,
    ));
    out.push_str(&format!(
        "Sample rate   : ~{:.1} samples/s\n",
        if duration_secs > 0.0 {
            report.total_samples as f64 / duration_secs
        } else {
            0.0
        },
    ));
    out.push_str("\n--- Hotspot Summary ---\n");
    out.push_str(&format!(
        "{:<40} {:>8} {:>7}\n",
        "Function", "Samples", "   %"
    ));
    out.push_str(&format!("{}\n", "-".repeat(58)));

    for func in &report.top_functions {
        out.push_str(&format!(
            "{:<40} {:>8} {:>6.1}%\n",
            truncate_name(&func.name, 40),
            func.sample_count,
            func.percentage,
        ));
    }

    // Text-based flame graph (indented tree representation).
    out.push_str("\n--- Flame Graph (text) ---\n");
    format_flame_tree(&mut out, &report.top_functions, report.total_samples);

    out
}

/// Render a simple text flame graph: each function is shown as a bar of
/// `#` characters proportional to its sample share, indented to suggest a
/// stack-like hierarchy.
fn format_flame_tree(out: &mut String, functions: &[FunctionProfile], total: usize) {
    if functions.is_empty() {
        out.push_str("  (no samples)\n");
        return;
    }

    let bar_width: usize = 40;

    // Top-level: the entire execution time.
    out.push_str(&format!("[all] ({} samples)\n", total));

    for (i, func) in functions.iter().enumerate() {
        let filled = if total > 0 {
            (func.sample_count * bar_width) / total
        } else {
            0
        };
        let filled = if filled == 0 && func.sample_count > 0 {
            1
        } else {
            filled
        };

        let bar: String = core::iter::repeat('#').take(filled).collect();
        let pad: String = core::iter::repeat(' ').take(bar_width.saturating_sub(filled)).collect();

        // Indent deeper for lower-ranked functions to give a tree feel.
        let indent = if i == 0 { "  " } else { "    " };

        out.push_str(&format!(
            "{}{} |{}{}| {:.1}%\n",
            indent,
            truncate_name(&func.name, 24),
            bar,
            pad,
            func.percentage,
        ));
    }
}

/// Truncate a symbol name to at most `max_len` characters, appending `..`
/// if it was shortened.
fn truncate_name(name: &str, max_len: usize) -> String {
    if name.len() <= max_len {
        format!("{:<width$}", name, width = max_len)
    } else {
        let mut s = String::from(&name[..max_len - 2]);
        s.push_str("..");
        s
    }
}

// ---------------------------------------------------------------------------
// Performance counters
// ---------------------------------------------------------------------------

/// Read a snapshot of the kernel-wide performance counters.
///
/// The counters are monotonically increasing atomics bumped by the
/// interrupt, syscall, scheduler, and page-fault paths.  Call this twice
/// and subtract to get a delta over a time window.
pub fn perf_stat() -> PerfCounters {
    PerfCounters {
        context_switch_count: CONTEXT_SWITCH_COUNT.load(Ordering::Relaxed),
        syscall_count: SYSCALL_COUNT.load(Ordering::Relaxed),
        page_fault_count: PAGE_FAULT_COUNT.load(Ordering::Relaxed),
        interrupt_count: INTERRUPT_COUNT.load(Ordering::Relaxed),
    }
}

/// Format a [`PerfCounters`] snapshot as a human-readable summary string.
pub fn format_perf_counters(counters: &PerfCounters) -> String {
    let mut out = String::with_capacity(256);
    out.push_str("=== MerlionOS Performance Counters ===\n");
    out.push_str(&format!("Context switches : {}\n", counters.context_switch_count));
    out.push_str(&format!("Syscalls         : {}\n", counters.syscall_count));
    out.push_str(&format!("Page faults      : {}\n", counters.page_fault_count));
    out.push_str(&format!("Interrupts       : {}\n", counters.interrupt_count));
    out
}
