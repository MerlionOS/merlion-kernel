/// System logger for MerlionOS kernel.
///
/// Captures all kernel messages with timestamps in a structured ring buffer.
/// Each entry records the timer tick, log level, originating module, and message.
/// The log can be queried, filtered by severity, formatted for display, and
/// persisted to the virtio disk via diskfs.
///
/// # Shell commands (handled in shell.rs dispatch):
///   - `syslog`    — show last 20 entries
///   - `syslog -e` — show only error-level entries
///   - `syslog -s` — save log to disk as "syslog.txt"

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use spin::Mutex;

use crate::timer;

/// Maximum number of entries the ring buffer holds before wrapping.
const SYSLOG_CAPACITY: usize = 256;

/// Global syslog ring buffer, protected by a spin mutex.
pub static SYSLOG: Mutex<SyslogBuffer> = Mutex::new(SyslogBuffer::new());

// ---------------------------------------------------------------------------
// LogLevel
// ---------------------------------------------------------------------------

/// Severity level for a syslog entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    /// Verbose debugging information.
    Debug,
    /// Routine operational messages.
    Info,
    /// Non-fatal anomalies that deserve attention.
    Warn,
    /// Serious failures that impair functionality.
    Error,
    /// Unrecoverable conditions — the kernel cannot continue.
    Panic,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogLevel::Debug => write!(f, "DEBUG"),
            LogLevel::Info  => write!(f, "INFO "),
            LogLevel::Warn  => write!(f, "WARN "),
            LogLevel::Error => write!(f, "ERROR"),
            LogLevel::Panic => write!(f, "PANIC"),
        }
    }
}

// ---------------------------------------------------------------------------
// LogEntry
// ---------------------------------------------------------------------------

/// A single syslog record.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Timer tick at the moment the entry was recorded.
    pub timestamp_ticks: u64,
    /// Severity level.
    pub level: LogLevel,
    /// Kernel module that produced the message (e.g. "timer", "diskfs").
    pub module: &'static str,
    /// Human-readable log message.
    pub message: String,
}

// ---------------------------------------------------------------------------
// SyslogBuffer — fixed-capacity ring buffer of LogEntry
// ---------------------------------------------------------------------------

/// Ring buffer that stores the most recent [`SYSLOG_CAPACITY`] log entries.
///
/// When the buffer is full, the oldest entry is silently overwritten.
pub struct SyslogBuffer {
    entries: Vec<LogEntry>,
    /// Next write position (wraps at SYSLOG_CAPACITY).
    write_pos: usize,
    /// Total entries ever written (used to detect wrap-around).
    total_written: usize,
}

impl SyslogBuffer {
    /// Create an empty syslog buffer.
    ///
    /// The internal `Vec` is initially empty and grows on first use (after the
    /// heap is available).
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            write_pos: 0,
            total_written: 0,
        }
    }

    /// Append one entry to the ring buffer.
    fn push(&mut self, entry: LogEntry) {
        if self.entries.len() < SYSLOG_CAPACITY {
            // Buffer still growing — just push.
            self.entries.push(entry);
        } else {
            // Buffer full — overwrite oldest.
            self.entries[self.write_pos] = entry;
        }
        self.write_pos = (self.write_pos + 1) % SYSLOG_CAPACITY;
        self.total_written += 1;
    }

    /// Return the last `count` entries in chronological order.
    fn last_n(&self, count: usize) -> Vec<LogEntry> {
        let len = self.entries.len();
        if len == 0 {
            return Vec::new();
        }
        let count = count.min(len);

        let mut result = Vec::with_capacity(count);
        // The most recent entry is at write_pos - 1 (mod len).
        // We want the last `count` entries in oldest-first order.
        let start = if len < SYSLOG_CAPACITY {
            // Haven't wrapped yet — entries are in order 0..len.
            len - count
        } else {
            // Wrapped — oldest is at write_pos, newest at write_pos - 1.
            (self.write_pos + len - count) % len
        };

        for i in 0..count {
            let idx = (start + i) % len;
            result.push(self.entries[idx].clone());
        }
        result
    }

    /// Return all entries that match the given level, in chronological order.
    fn filter_by_level(&self, level: LogLevel) -> Vec<LogEntry> {
        let all = self.last_n(self.entries.len());
        all.into_iter().filter(|e| e.level == level).collect()
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Record a log entry with the current timer tick.
pub fn log(level: LogLevel, module: &'static str, message: String) {
    let entry = LogEntry {
        timestamp_ticks: timer::ticks(),
        level,
        module,
        message,
    };

    // Also echo to the kernel ring buffer for serial visibility.
    crate::klog_println!("[{}] {}: {}", entry.level, entry.module, entry.message);

    x86_64::instructions::interrupts::without_interrupts(|| {
        SYSLOG.lock().push(entry);
    });
}

/// Convenience: log an informational message.
pub fn log_info(module: &'static str, msg: String) {
    log(LogLevel::Info, module, msg);
}

/// Convenience: log a warning message.
pub fn log_warn(module: &'static str, msg: String) {
    log(LogLevel::Warn, module, msg);
}

/// Convenience: log an error message.
pub fn log_error(module: &'static str, msg: String) {
    log(LogLevel::Error, module, msg);
}

/// Retrieve the last `count` log entries in chronological order.
pub fn get_entries(count: usize) -> Vec<LogEntry> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        SYSLOG.lock().last_n(count)
    })
}

/// Retrieve all entries matching `level`, in chronological order.
pub fn get_entries_by_level(level: LogLevel) -> Vec<LogEntry> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        SYSLOG.lock().filter_by_level(level)
    })
}

/// Format a slice of log entries into a human-readable string.
///
/// Each line has the form:
/// ```text
/// [    1234] INFO  timer: PIT initialized at 100 Hz
/// ```
pub fn format_entries(entries: &[LogEntry]) -> String {
    use core::fmt::Write;

    let mut out = String::new();
    for entry in entries {
        let _ = writeln!(
            out,
            "[{:>8}] {} {}: {}",
            entry.timestamp_ticks, entry.level, entry.module, entry.message,
        );
    }
    out
}

/// Persist the entire syslog to the virtio disk as `syslog.txt`.
///
/// Returns `Ok(())` on success, or an error string if the disk is unavailable
/// or the write fails.
pub fn save_to_disk() -> Result<(), &'static str> {
    let entries = get_entries(SYSLOG_CAPACITY);
    if entries.is_empty() {
        return Err("syslog is empty");
    }
    let text = format_entries(&entries);
    crate::diskfs::write_file("syslog.txt", text.as_bytes())?;
    log_info("syslog", String::from("log saved to disk as syslog.txt"));
    Ok(())
}

/// Initialize the syslog subsystem.
///
/// Records the first entry so operators know when the log started.
pub fn init() {
    log(
        LogLevel::Info,
        "syslog",
        String::from("MerlionOS syslog initialized"),
    );
}
