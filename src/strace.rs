/// System call tracer for MerlionOS (analogous to Linux `strace`).
///
/// Records every syscall dispatched by the kernel into a fixed-size ring
/// buffer, with per-PID filtering, human-readable dump output, and
/// aggregate statistics (call counts and average duration per syscall).
///
/// # Usage
///
/// ```ignore
/// strace::enable_tracing(pid);
/// // ... syscalls happen via dispatch ...
/// let output = strace::dump_traces(Some(pid));
/// serial_println!("{}", output);
/// serial_println!("{}", strace::trace_stats());
/// strace::disable_tracing(pid);
/// ```

use alloc::string::String;
use alloc::format;
use spin::Mutex;

const SYS_WRITE: u64 = 0;
const SYS_EXIT: u64 = 1;
const SYS_YIELD: u64 = 2;
const SYS_GETPID: u64 = 3;
const SYS_SLEEP: u64 = 4;
const SYS_SEND: u64 = 5;
const SYS_RECV: u64 = 6;
const NUM_SYSCALLS: usize = 7;

/// A single recorded syscall invocation.
#[derive(Debug, Clone, Copy)]
pub struct SyscallTrace {
    /// Timer tick at which the syscall was entered.
    pub timestamp: u64,
    /// PID of the calling process.
    pub pid: usize,
    /// Raw syscall number (0..6).
    pub syscall_num: u64,
    /// First three arguments passed via rdi, rsi, rdx.
    pub args: [u64; 3],
    /// Return value of the syscall (0 for void calls).
    pub result: i64,
    /// Wall-clock duration of the syscall in timer ticks.
    pub duration_ticks: u64,
}

impl SyscallTrace {
    const fn empty() -> Self {
        Self { timestamp: 0, pid: 0, syscall_num: 0, args: [0; 3], result: 0, duration_ticks: 0 }
    }
}

/// Maximum number of traces retained in the ring buffer.
const TRACE_BUF_SIZE: usize = 256;

/// Fixed-capacity ring buffer storing the most recent [`TRACE_BUF_SIZE`]
/// syscall traces. Older entries are silently overwritten.
pub struct TraceBuffer {
    entries: [SyscallTrace; TRACE_BUF_SIZE],
    write_pos: usize,
    total_recorded: usize,
}

impl TraceBuffer {
    const fn new() -> Self {
        Self {
            entries: [SyscallTrace::empty(); TRACE_BUF_SIZE],
            write_pos: 0,
            total_recorded: 0,
        }
    }

    /// Push a trace, overwriting the oldest entry when full.
    fn push(&mut self, trace: SyscallTrace) {
        self.entries[self.write_pos] = trace;
        self.write_pos = (self.write_pos + 1) % TRACE_BUF_SIZE;
        self.total_recorded += 1;
    }

    /// Iterate over stored traces in chronological order (oldest first).
    fn iter(&self) -> impl Iterator<Item = &SyscallTrace> {
        let count = core::cmp::min(self.total_recorded, TRACE_BUF_SIZE);
        let start = if self.total_recorded <= TRACE_BUF_SIZE { 0 } else { self.write_pos };
        (0..count).map(move |i| &self.entries[(start + i) % TRACE_BUF_SIZE])
    }
}

/// Global trace ring buffer, protected by a spin lock.
static TRACE_BUF: Mutex<TraceBuffer> = Mutex::new(TraceBuffer::new());

/// Per-PID tracing enable flags. Index = PID, value = enabled.
const MAX_PIDS: usize = 64;
static TRACING_ENABLED: Mutex<[bool; MAX_PIDS]> = Mutex::new([false; MAX_PIDS]);

/// Per-syscall aggregate statistics: (call_count, total_duration_ticks).
static STATS: Mutex<[(u64, u64); NUM_SYSCALLS]> = Mutex::new([(0, 0); NUM_SYSCALLS]);

/// Enable syscall tracing for the given PID.
///
/// All subsequent syscalls by this PID will be recorded until
/// [`disable_tracing`] is called.
pub fn enable_tracing(pid: usize) {
    if pid < MAX_PIDS {
        TRACING_ENABLED.lock()[pid] = true;
    }
}

/// Disable syscall tracing for the given PID.
pub fn disable_tracing(pid: usize) {
    if pid < MAX_PIDS {
        TRACING_ENABLED.lock()[pid] = false;
    }
}

/// Check whether tracing is currently enabled for `pid`.
fn is_tracing(pid: usize) -> bool {
    if pid >= MAX_PIDS { return false; }
    TRACING_ENABLED.lock()[pid]
}

/// Record a completed syscall invocation.
///
/// Intended to be called from the syscall dispatch path after the handler
/// returns. No-op if tracing is not enabled for the current PID.
///
/// * `num`      - Syscall number (0..6).
/// * `args`     - The three register arguments [rdi, rsi, rdx].
/// * `result`   - Return value (use 0 for void calls).
/// * `duration` - Duration of the syscall in timer ticks.
pub fn record_syscall(num: u64, args: [u64; 3], result: i64, duration: u64) {
    let pid = crate::task::current_pid();
    if !is_tracing(pid) {
        return;
    }

    let trace = SyscallTrace {
        timestamp: crate::timer::ticks(),
        pid,
        syscall_num: num,
        args,
        result,
        duration_ticks: duration,
    };

    TRACE_BUF.lock().push(trace);

    let idx = num as usize;
    if idx < NUM_SYSCALLS {
        let mut stats = STATS.lock();
        stats[idx].0 += 1;
        stats[idx].1 += duration;
    }
}

/// Return a human-readable name for the given syscall number.
pub fn syscall_name(num: u64) -> &'static str {
    match num {
        SYS_WRITE  => "write",
        SYS_EXIT   => "exit",
        SYS_YIELD  => "yield",
        SYS_GETPID => "getpid",
        SYS_SLEEP  => "sleep",
        SYS_SEND   => "send",
        SYS_RECV   => "recv",
        _          => "unknown",
    }
}

/// Produce a formatted dump of recorded traces.
///
/// If `pid` is `Some(p)`, only traces belonging to that PID are shown.
/// If `pid` is `None`, all traces are included.
///
/// Each output line has the form:
/// ```text
/// [tick] pid=N  write(0xBUF, 42, 0) = 0  (3 ticks)
/// ```
pub fn dump_traces(pid: Option<usize>) -> String {
    let buf = TRACE_BUF.lock();
    let mut out = String::with_capacity(4096);

    out.push_str("=== MerlionOS Syscall Trace ===\n");
    out.push_str(&format!(
        "{:<10} {:<6} {:<8} {:<28} {:<8} {}\n",
        "Tick", "PID", "Syscall", "Args", "Result", "Duration"
    ));
    out.push_str(&format!("{}\n", "-".repeat(72)));

    let mut count = 0usize;
    for trace in buf.iter() {
        if let Some(fp) = pid {
            if trace.pid != fp { continue; }
        }
        out.push_str(&format!(
            "{:<10} {:<6} {:<8} ({:#x}, {:#x}, {:#x}) = {:<6} ({} ticks)\n",
            trace.timestamp,
            trace.pid,
            syscall_name(trace.syscall_num),
            trace.args[0], trace.args[1], trace.args[2],
            trace.result,
            trace.duration_ticks,
        ));
        count += 1;
    }

    if count == 0 {
        out.push_str("  (no traces recorded)\n");
    } else {
        out.push_str(&format!("\nTotal: {} trace(s)\n", count));
    }
    out
}

/// Return a formatted summary of per-syscall call counts and average
/// duration (in timer ticks).
pub fn trace_stats() -> String {
    let stats = STATS.lock();
    let mut out = String::with_capacity(512);

    out.push_str("=== MerlionOS Syscall Statistics ===\n");
    out.push_str(&format!("{:<10} {:>10} {:>12}\n", "Syscall", "Calls", "Avg ticks"));
    out.push_str(&format!("{}\n", "-".repeat(34)));

    for i in 0..NUM_SYSCALLS {
        let (calls, total_dur) = stats[i];
        let avg = if calls > 0 { total_dur / calls } else { 0 };
        out.push_str(&format!("{:<10} {:>10} {:>12}\n", syscall_name(i as u64), calls, avg));
    }
    out
}
