/// System snapshot and restore module.
///
/// Captures a point-in-time snapshot of the kernel environment: environment
/// variables, VFS files under `/tmp`, and the kernel configuration. Snapshots
/// can later be restored to roll the system state back.
///
/// Storage is bounded to [`MAX_SNAPSHOTS`] entries (oldest are evicted when
/// the limit is reached). An automatic snapshot helper is provided for
/// periodic invocation from the timer subsystem.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;
use spin::Mutex;

/// Maximum number of snapshots retained in storage.
const MAX_SNAPSHOTS: usize = 8;

/// Interval in seconds between automatic snapshots.
const AUTO_SNAPSHOT_INTERVAL_SECS: u64 = 300;

/// Monotonically increasing snapshot identifier.
static NEXT_ID: Mutex<u64> = Mutex::new(1);

/// Timestamp (in PIT ticks) of the last automatic snapshot.
static LAST_AUTO_TICK: Mutex<u64> = Mutex::new(0);

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Full snapshot of captured system state.
#[derive(Clone)]
struct Snapshot {
    /// Unique identifier for this snapshot.
    id: u64,
    /// PIT tick count when the snapshot was taken.
    timestamp: u64,
    /// Human-readable description supplied by the caller.
    description: String,
    /// Copy of all environment variables at capture time.
    env_vars: Vec<(String, String)>,
    /// Contents of regular files under `/tmp` (path, data).
    vfs_files: Vec<(String, String)>,
    /// Serialised kernel configuration (`key=value` lines).
    config: String,
}

/// Lightweight summary returned by [`list_snapshots`].
pub struct SnapshotInfo {
    /// Snapshot identifier.
    pub id: u64,
    /// Uptime in seconds when the snapshot was created.
    pub uptime_secs: u64,
    /// Description provided at creation time.
    pub description: String,
}

/// Global snapshot storage protected by a spin-lock.
struct SnapshotStorage {
    snapshots: Vec<Snapshot>,
}

impl SnapshotStorage {
    /// Create an empty storage instance.
    const fn new() -> Self {
        Self {
            snapshots: Vec::new(),
        }
    }

    /// Evict the oldest snapshot if the capacity limit is reached.
    fn enforce_limit(&mut self) {
        while self.snapshots.len() >= MAX_SNAPSHOTS {
            self.snapshots.remove(0);
        }
    }

    /// Find a snapshot by its identifier.
    fn find(&self, id: u64) -> Option<&Snapshot> {
        self.snapshots.iter().find(|s| s.id == id)
    }

    /// Remove a snapshot by its identifier. Returns `true` if found.
    fn remove(&mut self, id: u64) -> bool {
        if let Some(pos) = self.snapshots.iter().position(|s| s.id == id) {
            self.snapshots.remove(pos);
            true
        } else {
            false
        }
    }
}

static STORAGE: Mutex<SnapshotStorage> = Mutex::new(SnapshotStorage::new());

// ---------------------------------------------------------------------------
// Capture helpers
// ---------------------------------------------------------------------------

/// Capture current environment variables as a list of `(key, value)` pairs.
fn capture_env_vars() -> Vec<(String, String)> {
    crate::env::list()
}

/// Capture regular files under `/tmp` in the VFS.
///
/// Each entry is `(path, contents)`. Only files whose content can be read
/// as UTF-8 strings are included (binary blobs are skipped).
fn capture_tmp_files() -> Vec<(String, String)> {
    let mut files = Vec::new();
    if let Ok(entries) = crate::vfs::ls("/tmp") {
        for (name, kind) in entries {
            if kind == '-' {
                let path = format!("/tmp/{}", name);
                if let Ok(data) = crate::vfs::cat(&path) {
                    files.push((path, data));
                }
            }
        }
    }
    files
}

/// Serialise the current kernel configuration to a single string.
fn capture_config() -> String {
    let pairs = crate::kconfig::list();
    let mut buf = String::new();
    for (key, value) in pairs {
        buf.push_str(&key);
        buf.push('=');
        buf.push_str(&value);
        buf.push('\n');
    }
    buf
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new snapshot capturing the current system state.
///
/// Returns the unique snapshot identifier on success.
///
/// # Arguments
/// * `description` – free-form text describing why the snapshot was taken.
pub fn create_snapshot(description: &str) -> u64 {
    let id = {
        let mut next = NEXT_ID.lock();
        let id = *next;
        *next += 1;
        id
    };

    let snapshot = Snapshot {
        id,
        timestamp: crate::timer::ticks(),
        description: description.to_owned(),
        env_vars: capture_env_vars(),
        vfs_files: capture_tmp_files(),
        config: capture_config(),
    };

    let mut storage = STORAGE.lock();
    storage.enforce_limit();
    storage.snapshots.push(snapshot);

    crate::klog_println!("[snapshot] created #{} – {}", id, description);
    id
}

/// Restore the system state from a previously captured snapshot.
///
/// Environment variables are replaced wholesale, `/tmp` files are
/// re-written, and the kernel configuration is reloaded from the
/// saved data.
///
/// Returns `Err` if the snapshot identifier is not found.
pub fn restore_snapshot(id: u64) -> Result<(), &'static str> {
    let storage = STORAGE.lock();
    let snap = storage.find(id).ok_or("snapshot not found")?;

    // --- Restore environment variables ---
    let current_env = crate::env::list();
    for (key, _) in &current_env {
        crate::env::unset(key);
    }
    for (key, value) in &snap.env_vars {
        crate::env::set(key, value);
    }

    // --- Restore /tmp files ---
    // Remove existing /tmp entries first.
    if let Ok(entries) = crate::vfs::ls("/tmp") {
        for (name, kind) in entries {
            if kind == '-' {
                let path = format!("/tmp/{}", name);
                let _ = crate::vfs::rm(&path);
            }
        }
    }
    // Write back saved files.
    for (path, data) in &snap.vfs_files {
        let _ = crate::vfs::write(path, data);
    }

    // --- Restore kernel configuration ---
    let config_str = snap.config.clone();
    drop(storage);

    // Parse saved config and push into kconfig.
    for line in config_str.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            crate::kconfig::set(key.trim(), value.trim());
        }
    }
    let _ = crate::kconfig::save();

    crate::klog_println!("[snapshot] restored #{}", id);
    Ok(())
}

/// List all stored snapshots in chronological order.
///
/// Returns lightweight [`SnapshotInfo`] summaries (the full data is not
/// copied).
pub fn list_snapshots() -> Vec<SnapshotInfo> {
    let storage = STORAGE.lock();
    storage
        .snapshots
        .iter()
        .map(|s| SnapshotInfo {
            id: s.id,
            uptime_secs: s.timestamp / crate::timer::PIT_FREQUENCY_HZ,
            description: s.description.clone(),
        })
        .collect()
}

/// Delete a snapshot by identifier.
///
/// Returns `Err` if the identifier does not match any stored snapshot.
pub fn delete_snapshot(id: u64) -> Result<(), &'static str> {
    let mut storage = STORAGE.lock();
    if storage.remove(id) {
        crate::klog_println!("[snapshot] deleted #{}", id);
        Ok(())
    } else {
        Err("snapshot not found")
    }
}

/// Periodic automatic snapshot hook.
///
/// Intended to be called from the timer tick path or a housekeeping loop.
/// Creates a snapshot at most once every [`AUTO_SNAPSHOT_INTERVAL_SECS`]
/// seconds to avoid flooding storage.
pub fn auto_snapshot() {
    let now = crate::timer::ticks();
    let interval_ticks = AUTO_SNAPSHOT_INTERVAL_SECS * crate::timer::PIT_FREQUENCY_HZ;

    let mut last = LAST_AUTO_TICK.lock();
    if now.saturating_sub(*last) < interval_ticks {
        return;
    }
    *last = now;
    drop(last);

    create_snapshot("auto");
}
