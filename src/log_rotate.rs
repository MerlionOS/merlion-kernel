/// Log rotation for MerlionOS.
/// Manages rotation of log files when they exceed size limits.
/// Supports configurable rotation count and size thresholds.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Configuration for a rotated log file.
#[derive(Debug, Clone)]
pub struct RotateConfig {
    /// Base path for the log file (e.g., "/var/log/kernel.log").
    pub path: String,
    /// Maximum size in bytes before rotation triggers.
    pub max_size: usize,
    /// Number of rotated files to keep (e.g., 3 means .1, .2, .3).
    pub keep_count: usize,
    /// Whether to compress rotated files (simulated — just adds .lz77 suffix).
    pub compress: bool,
}

const MAX_CONFIGS: usize = 16;
const DEFAULT_MAX_SIZE: usize = 8192;  // 8KB per log file
const DEFAULT_KEEP_COUNT: usize = 4;   // keep 4 rotated files

/// Track current size of each log file.
struct LogTracker {
    configs: Vec<RotateConfig>,
    current_sizes: Vec<usize>,
    rotation_counts: Vec<usize>,
}

static TRACKER: Mutex<LogTracker> = Mutex::new(LogTracker {
    configs: Vec::new(),
    current_sizes: Vec::new(),
    rotation_counts: Vec::new(),
});

/// Total rotations performed.
static TOTAL_ROTATIONS: AtomicUsize = AtomicUsize::new(0);

/// Initialize the log rotation system with default log files.
pub fn init() {
    let mut tracker = TRACKER.lock();

    // Register default log files
    let defaults = [
        ("/var/log/kernel.log", DEFAULT_MAX_SIZE, DEFAULT_KEEP_COUNT),
        ("/var/log/auth.log", 4096, 4),
        ("/var/log/syslog", DEFAULT_MAX_SIZE, DEFAULT_KEEP_COUNT),
    ];

    for &(path, max_size, keep) in &defaults {
        tracker.configs.push(RotateConfig {
            path: path.to_owned(),
            max_size,
            keep_count: keep,
            compress: false,
        });
        tracker.current_sizes.push(0);
        tracker.rotation_counts.push(0);
    }

    // Create /var/log directory in VFS
    let _ = crate::vfs::mkdir("/var");
    let _ = crate::vfs::mkdir("/var/log");

    crate::serial_println!("[log_rotate] initialized with {} log files", defaults.len());
    crate::klog_println!("[log_rotate] initialized");
}

/// Register a new log file for rotation tracking.
pub fn register(path: &str, max_size: usize, keep_count: usize) -> Result<(), &'static str> {
    let mut tracker = TRACKER.lock();
    if tracker.configs.len() >= MAX_CONFIGS {
        return Err("log_rotate: max configs reached");
    }
    if tracker.configs.iter().any(|c| c.path == path) {
        return Err("log_rotate: already registered");
    }
    tracker.configs.push(RotateConfig {
        path: path.to_owned(),
        max_size,
        keep_count,
        compress: false,
    });
    tracker.current_sizes.push(0);
    tracker.rotation_counts.push(0);
    Ok(())
}

/// Append data to a managed log file, triggering rotation if needed.
/// Returns true if rotation occurred.
pub fn append(path: &str, data: &str) -> bool {
    let mut rotated = false;

    let needs_rotate = {
        let mut tracker = TRACKER.lock();
        if let Some(idx) = tracker.configs.iter().position(|c| c.path == path) {
            tracker.current_sizes[idx] += data.len();
            tracker.current_sizes[idx] > tracker.configs[idx].max_size
        } else {
            false
        }
    };

    if needs_rotate {
        rotate(path);
        rotated = true;
    }

    // Append to the VFS file
    let existing = crate::vfs::cat(path).unwrap_or_default();
    let mut new_content = existing;
    new_content.push_str(data);
    let _ = crate::vfs::write(path, &new_content);

    rotated
}

/// Perform rotation on a specific log file.
/// kernel.log -> kernel.log.1 -> kernel.log.2 -> ... -> kernel.log.N (deleted)
pub fn rotate(path: &str) {
    let config = {
        let tracker = TRACKER.lock();
        tracker.configs.iter().find(|c| c.path == path).cloned()
    };

    let config = match config {
        Some(c) => c,
        None => return,
    };

    // Rotate existing files: .N -> delete, .N-1 -> .N, ..., .1 -> .2, current -> .1
    for i in (1..config.keep_count).rev() {
        let from = format!("{}.{}", path, i);
        let to = format!("{}.{}", path, i + 1);
        // Read old, write to new name, delete old
        if let Ok(content) = crate::vfs::cat(&from) {
            let _ = crate::vfs::write(&to, &content);
            let _ = crate::vfs::rm(&from);
        }
    }

    // Current -> .1
    if let Ok(content) = crate::vfs::cat(path) {
        let rotated_path = format!("{}.1", path);
        if config.compress {
            // Simulated compression — just note it
            let _ = crate::vfs::write(&rotated_path, &content);
        } else {
            let _ = crate::vfs::write(&rotated_path, &content);
        }
    }

    // Clear current log
    let _ = crate::vfs::write(path, "");

    // Update tracker
    {
        let mut tracker = TRACKER.lock();
        if let Some(idx) = tracker.configs.iter().position(|c| c.path == path) {
            tracker.current_sizes[idx] = 0;
            tracker.rotation_counts[idx] += 1;
        }
    }

    TOTAL_ROTATIONS.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[log_rotate] rotated {}", path);
}

/// Force rotation of all managed log files.
pub fn rotate_all() {
    let paths: Vec<String> = {
        let tracker = TRACKER.lock();
        tracker.configs.iter().map(|c| c.path.clone()).collect()
    };

    for path in &paths {
        rotate(path);
    }
}

/// Get status of all managed log files.
pub fn status() -> String {
    let tracker = TRACKER.lock();
    let mut out = String::from("Log rotation status:\n");

    for (i, config) in tracker.configs.iter().enumerate() {
        let size = tracker.current_sizes.get(i).copied().unwrap_or(0);
        let rotations = tracker.rotation_counts.get(i).copied().unwrap_or(0);
        let pct = if config.max_size > 0 { (size * 100) / config.max_size } else { 0 };
        out.push_str(&format!(
            "  {} — {}/{} bytes ({}%) — {} rotations — keep {}\n",
            config.path, size, config.max_size, pct, rotations, config.keep_count
        ));
    }

    out.push_str(&format!(
        "Total rotations: {}\n",
        TOTAL_ROTATIONS.load(Ordering::Relaxed)
    ));

    out
}

/// List all managed log files.
pub fn list() -> Vec<String> {
    let tracker = TRACKER.lock();
    tracker.configs.iter().map(|c| c.path.clone()).collect()
}

/// Get the rotation count for a specific log file.
pub fn rotation_count(path: &str) -> usize {
    let tracker = TRACKER.lock();
    tracker.configs.iter()
        .position(|c| c.path == path)
        .and_then(|idx| tracker.rotation_counts.get(idx).copied())
        .unwrap_or(0)
}

/// Set max size for a log file.
pub fn set_max_size(path: &str, max_size: usize) -> Result<(), &'static str> {
    let mut tracker = TRACKER.lock();
    let config = tracker.configs.iter_mut()
        .find(|c| c.path == path)
        .ok_or("log_rotate: not found")?;
    config.max_size = max_size;
    Ok(())
}

/// Set keep count for a log file.
pub fn set_keep_count(path: &str, keep_count: usize) -> Result<(), &'static str> {
    let mut tracker = TRACKER.lock();
    let config = tracker.configs.iter_mut()
        .find(|c| c.path == path)
        .ok_or("log_rotate: not found")?;
    config.keep_count = keep_count;
    Ok(())
}
