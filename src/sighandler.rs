/// Enhanced signal handling for MerlionOS.
///
/// Extends the basic signal framework with user-defined signal handlers,
/// per-process handler tables, a pending signal queue, and built-in
/// behaviour for uncatchable signals (SIGKILL, SIGSTOP, SIGCHLD).
///
/// Each process may register up to [`MAX_SIGNALS`] handlers via
/// [`register_handler`] or [`sigaction`].  The scheduler calls
/// [`check_pending_signals`] on every tick to drain each task's queue.

use alloc::vec::Vec;
use spin::Mutex;

/// Maximum number of distinct signal numbers supported (1..=MAX_SIGNALS).
const MAX_SIGNALS: usize = 16;

/// Maximum number of concurrent processes the handler table tracks.
const MAX_PROCESSES: usize = 8;

/// Capacity of the per-process pending signal queue.
const SIGNAL_QUEUE_CAP: usize = 32;

// ── well-known signal numbers ───────────────────────────────────────────

/// Immediate, uncatchable termination.
pub const SIGKILL: u8 = 9;
/// Suspend execution (uncatchable).
pub const SIGSTOP: u8 = 19;
/// Child process state change — notify parent.
pub const SIGCHLD: u8 = 17;
/// Graceful termination (catchable).
pub const SIGTERM: u8 = 15;
/// Interrupt from terminal (catchable).
pub const SIGINT: u8 = 2;
/// User-defined signal 1.
pub const SIGUSR1: u8 = 10;
/// User-defined signal 2.
pub const SIGUSR2: u8 = 12;
/// Continue a stopped process.
pub const SIGCONT: u8 = 18;

// ── handler types ───────────────────────────────────────────────────────

/// The action taken when a signal is delivered to a process.
#[derive(Clone, Copy)]
pub enum HandlerType {
    /// Use the kernel's built-in default behaviour for this signal.
    Default,
    /// Silently ignore the signal.
    Ignore,
    /// Invoke a user-supplied function, passing the signal number.
    Custom(fn(u8)),
}

/// A registered signal handler entry.
#[derive(Clone, Copy)]
pub struct SignalHandler {
    /// Signal number this handler is bound to (1..=MAX_SIGNALS).
    pub signal_num: u8,
    /// Action to take when the signal arrives.
    pub handler: HandlerType,
}

// ── per-process handler table ───────────────────────────────────────────

/// Per-process signal configuration: handler table + pending queue.
struct ProcessSignalState {
    /// PID this slot belongs to, or 0 if unused.
    pid: usize,
    /// One handler entry per signal number (index 0 → signal 1).
    handlers: [HandlerType; MAX_SIGNALS],
    /// FIFO queue of pending (not yet delivered) signal numbers.
    pending: Vec<u8>,
    /// True when the process is in a stopped state (SIGSTOP).
    stopped: bool,
}

/// Global table of per-process signal state, protected by a spinlock.
static STATE: Mutex<Vec<ProcessSignalState>> = Mutex::new(Vec::new());

/// Ensure the global table is initialised with `MAX_PROCESSES` slots.
fn ensure_init(table: &mut Vec<ProcessSignalState>) {
    if table.is_empty() {
        for _ in 0..MAX_PROCESSES {
            table.push(ProcessSignalState {
                pid: 0,
                handlers: [HandlerType::Default; MAX_SIGNALS],
                pending: Vec::new(),
                stopped: false,
            });
        }
    }
}

/// Find (or allocate) the slot index for `pid`.
fn slot_for(table: &mut Vec<ProcessSignalState>, pid: usize) -> Option<usize> {
    // Existing slot?
    for (i, s) in table.iter().enumerate() {
        if s.pid == pid {
            return Some(i);
        }
    }
    // Allocate a free slot.
    for (i, s) in table.iter_mut().enumerate() {
        if s.pid == 0 {
            s.pid = pid;
            return Some(i);
        }
    }
    None
}

// ── public API ──────────────────────────────────────────────────────────

/// Register a signal handler for `pid` and `signal`.
///
/// Returns `Err` if the signal number is out of range, if the process
/// table is full, or if the caller tries to override an uncatchable
/// signal (SIGKILL / SIGSTOP).
pub fn register_handler(
    pid: usize,
    signal: u8,
    handler: HandlerType,
) -> Result<(), &'static str> {
    if signal == 0 || signal as usize > MAX_SIGNALS {
        return Err("signal number out of range");
    }
    // SIGKILL and SIGSTOP cannot be caught or ignored.
    if matches!(signal, SIGKILL | SIGSTOP) && !matches!(handler, HandlerType::Default) {
        return Err("cannot override SIGKILL or SIGSTOP");
    }

    let mut table = STATE.lock();
    ensure_init(&mut table);
    let idx = slot_for(&mut table, pid).ok_or("process signal table full")?;
    table[idx].handlers[(signal - 1) as usize] = handler;
    Ok(())
}

/// Deliver a signal to `pid`.
///
/// For uncatchable signals the kernel acts immediately.  For everything
/// else the signal is pushed onto the process's pending queue so
/// [`check_pending_signals`] can dispatch it later.
pub fn deliver_signal(pid: usize, signal: u8) -> Result<(), &'static str> {
    if signal == 0 || signal as usize > MAX_SIGNALS {
        return Err("signal number out of range");
    }

    let mut table = STATE.lock();
    ensure_init(&mut table);

    // SIGKILL: release lock before calling task::kill (avoids deadlock).
    if signal == SIGKILL {
        drop(table);
        let _ = crate::task::kill(pid);
        crate::serial_println!("[sighandler] SIGKILL → pid {}", pid);
        return Ok(());
    }

    let idx = slot_for(&mut table, pid).ok_or("process signal table full")?;

    if signal == SIGSTOP {
        table[idx].stopped = true;
        crate::serial_println!("[sighandler] SIGSTOP → pid {}", pid);
    } else if signal == SIGCONT {
        table[idx].stopped = false;
        crate::serial_println!("[sighandler] SIGCONT → pid {}", pid);
    } else if table[idx].pending.len() < SIGNAL_QUEUE_CAP {
        table[idx].pending.push(signal);
    }
    Ok(())
}

/// Set the handler for `signal` in the *current* process (convenience
/// wrapper around [`register_handler`] using `task::current_pid()`).
pub fn sigaction(signal: u8, handler: HandlerType) -> Result<(), &'static str> {
    let pid = crate::task::current_pid();
    register_handler(pid, signal, handler)
}

/// Drain and dispatch all pending signals for `pid`.
///
/// The scheduler should call this once per tick (or on context-switch
/// entry) so that queued signals are delivered promptly.
pub fn check_pending_signals(pid: usize) {
    let mut table = STATE.lock();
    ensure_init(&mut table);

    let idx = match table.iter().position(|s| s.pid == pid) {
        Some(i) => i,
        None => return, // no state for this pid — nothing to do
    };

    // If the process is stopped, do not deliver any signals.
    if table[idx].stopped {
        return;
    }

    // Drain the queue into a local vec so we can release the lock
    // before invoking handlers (which may themselves send signals).
    let pending: Vec<u8> = table[idx].pending.drain(..).collect();
    let handlers: [HandlerType; MAX_SIGNALS] = table[idx].handlers;
    drop(table);

    for sig in pending {
        if sig == 0 || sig as usize > MAX_SIGNALS {
            continue;
        }
        match handlers[(sig - 1) as usize] {
            HandlerType::Custom(f) => f(sig),
            HandlerType::Ignore => { /* silently discard */ }
            HandlerType::Default => dispatch_default(pid, sig),
        }
    }
}

/// Release the signal-handler slot for `pid` (call on process exit).
pub fn cleanup(pid: usize) {
    let mut table = STATE.lock();
    ensure_init(&mut table);
    for slot in table.iter_mut() {
        if slot.pid == pid {
            slot.pid = 0;
            slot.handlers = [HandlerType::Default; MAX_SIGNALS];
            slot.pending.clear();
            slot.stopped = false;
            break;
        }
    }
}

// ── built-in default handlers ───────────────────────────────────────────

/// Execute the kernel's default behaviour for `signal` on `pid`.
fn dispatch_default(pid: usize, signal: u8) {
    match signal {
        SIGTERM | SIGINT | SIGUSR1 | SIGUSR2 => {
            // Default: terminate the process.
            let _ = crate::task::kill(pid);
            crate::serial_println!(
                "[sighandler] default terminate pid {} on signal {}",
                pid,
                signal
            );
        }
        SIGCHLD => {
            // Notify the parent — for now, log only.
            crate::serial_println!(
                "[sighandler] SIGCHLD delivered to pid {} (default: ignore)",
                pid
            );
        }
        _ => {
            // Unknown or unhandled signal — terminate.
            let _ = crate::task::kill(pid);
            crate::serial_println!(
                "[sighandler] unhandled signal {} → terminate pid {}",
                signal,
                pid
            );
        }
    }
}
