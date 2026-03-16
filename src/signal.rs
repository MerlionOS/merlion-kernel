/// Signal framework for inter-task signaling.
/// Simplified POSIX-like signals: SIGKILL, SIGTERM, SIGSTOP, SIGCONT.

use spin::Mutex;

const MAX_TASKS: usize = 8;

/// Signal numbers.
pub const SIGKILL: u8 = 9;
pub const SIGTERM: u8 = 15;
pub const SIGSTOP: u8 = 19;
pub const SIGCONT: u8 = 18;

/// Pending signals per task slot (bitmask).
static PENDING: Mutex<[u32; MAX_TASKS]> = Mutex::new([0; MAX_TASKS]);

/// Send a signal to a task by PID.
pub fn send_signal(pid: usize, signal: u8) -> Result<(), &'static str> {
    if pid == 0 {
        return Err("cannot signal kernel task");
    }

    // Find the task slot for this PID
    let tasks = crate::task::list();
    let task = tasks.iter().find(|t| t.pid == pid);
    if task.is_none() {
        return Err("task not found");
    }

    match signal {
        SIGKILL => {
            // Immediate termination
            crate::task::kill(pid)?;
            crate::serial_println!("[signal] SIGKILL → pid {}", pid);
            crate::klog_println!("[signal] SIGKILL sent to pid {}", pid);
        }
        SIGTERM => {
            // Graceful termination (same as kill for now)
            crate::task::kill(pid)?;
            crate::serial_println!("[signal] SIGTERM → pid {}", pid);
        }
        SIGSTOP | SIGCONT => {
            // Set pending flag (task checks on next schedule)
            let mut pending = PENDING.lock();
            // Find slot by scanning
            for (i, t) in tasks.iter().enumerate() {
                if t.pid == pid && i < MAX_TASKS {
                    pending[i] |= 1 << signal;
                    break;
                }
            }
            crate::serial_println!("[signal] SIG{} → pid {}",
                if signal == SIGSTOP { "STOP" } else { "CONT" }, pid);
        }
        _ => {
            return Err("unknown signal");
        }
    }

    Ok(())
}

/// Signal name from number.
pub fn name(sig: u8) -> &'static str {
    match sig {
        SIGKILL => "SIGKILL",
        SIGTERM => "SIGTERM",
        SIGSTOP => "SIGSTOP",
        SIGCONT => "SIGCONT",
        _ => "UNKNOWN",
    }
}

/// Parse signal name or number.
pub fn parse(s: &str) -> Option<u8> {
    match s.to_uppercase().as_str() {
        "SIGKILL" | "KILL" | "9" => Some(SIGKILL),
        "SIGTERM" | "TERM" | "15" => Some(SIGTERM),
        "SIGSTOP" | "STOP" | "19" => Some(SIGSTOP),
        "SIGCONT" | "CONT" | "18" => Some(SIGCONT),
        _ => s.parse::<u8>().ok(),
    }
}
