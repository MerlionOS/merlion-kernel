/// AI System Monitor — watches system metrics and reports anomalies.
/// Collects CPU, memory, and task data; flags unusual patterns.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use core::sync::atomic::{AtomicU64, Ordering};

/// System snapshot for analysis.
struct Snapshot {
    ticks: u64,
    task_count: usize,
    heap_used: usize,
    heap_total: usize,
    frames_allocated: u64,
}

/// Alert severity.
#[derive(Debug, Clone, Copy)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

/// System alert.
pub struct Alert {
    pub severity: Severity,
    pub message: String,
}

static LAST_TASK_COUNT: AtomicU64 = AtomicU64::new(0);

/// Run a system health check and return any alerts.
pub fn check() -> Vec<Alert> {
    let mut alerts = Vec::new();

    // Memory check
    let heap = crate::allocator::stats();
    let heap_pct = if heap.total > 0 { (heap.used * 100) / heap.total } else { 0 };

    if heap_pct > 90 {
        alerts.push(Alert {
            severity: Severity::Critical,
            message: format!("Heap usage critical: {}% ({}/{})",
                heap_pct, heap.used, heap.total),
        });
    } else if heap_pct > 70 {
        alerts.push(Alert {
            severity: Severity::Warning,
            message: format!("Heap usage high: {}%", heap_pct),
        });
    }

    // Task check
    let tasks = crate::task::list();
    let task_count = tasks.len();
    let prev_count = LAST_TASK_COUNT.swap(task_count as u64, Ordering::SeqCst);

    if task_count >= 7 {
        alerts.push(Alert {
            severity: Severity::Warning,
            message: format!("Task table nearly full: {}/8 slots", task_count),
        });
    }

    if prev_count > 0 && task_count as u64 > prev_count + 3 {
        alerts.push(Alert {
            severity: Severity::Warning,
            message: format!("Rapid task creation: {} → {} tasks", prev_count, task_count),
        });
    }

    // Uptime info
    let (h, m, s) = crate::timer::uptime_hms();

    // Physical memory
    let mem = crate::memory::stats();
    let frame_pct = if mem.total_usable_bytes > 0 {
        (mem.allocated_frames * 4096 * 100) / mem.total_usable_bytes
    } else { 0 };

    if frame_pct > 80 {
        alerts.push(Alert {
            severity: Severity::Warning,
            message: format!("Physical memory usage high: {}%", frame_pct),
        });
    }

    // All OK message if no issues
    if alerts.is_empty() {
        alerts.push(Alert {
            severity: Severity::Info,
            message: format!(
                "System healthy. Uptime {:02}:{:02}:{:02}, {} tasks, heap {}%, phys {}%",
                h, m, s, task_count, heap_pct, frame_pct
            ),
        });
    }

    alerts
}

/// Format alerts for display.
pub fn format_alerts(alerts: &[Alert]) -> String {
    let mut out = String::new();
    for alert in alerts {
        let (prefix, color) = match alert.severity {
            Severity::Info => ("INFO", "\x1b[32m"),
            Severity::Warning => ("WARN", "\x1b[33m"),
            Severity::Critical => ("CRIT", "\x1b[31m"),
        };
        out.push_str(&format!("  {}[{}]\x1b[0m {}\n", color, prefix, alert.message));
    }
    out
}
