/// Persistent storage layer for MerlionOS.
///
/// Syncs VFS (in-memory) ↔ diskfs (virtio-blk) so files survive reboot.
///
/// - On boot: load files from disk into VFS (/disk/ mount point)
/// - On write: flush to disk immediately (write-through)
/// - Shell commands: sync, mount-disk, umount-disk
///
/// Files under /disk/ are persistent. Other paths remain in-memory.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::serial_println;

const DISK_MOUNT: &str = "/disk";

static MOUNTED: AtomicBool = AtomicBool::new(false);
static WRITES: AtomicU64 = AtomicU64::new(0);
static READS: AtomicU64 = AtomicU64::new(0);
static BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);

/// Mount the persistent disk at /disk/.
/// Loads all files from virtio-blk MF16 filesystem into VFS.
pub fn mount() -> Result<(), &'static str> {
    if !crate::virtio_blk::is_detected() {
        return Err("no virtio disk detected");
    }

    // Format if not already formatted
    if !crate::diskfs::is_formatted() {
        serial_println!("[persist] disk not formatted — formatting as MF16...");
        crate::diskfs::format()?;
    }

    // Create mount point
    let _ = crate::vfs::mkdir(DISK_MOUNT);

    // Load all files from disk into VFS
    match crate::diskfs::list_files() {
        Ok(files) => {
            for file in &files {
                let vfs_path = format!("{}/{}", DISK_MOUNT, file.name.trim());
                match crate::diskfs::read_file(file.name.trim()) {
                    Ok(data) => {
                        if let Ok(s) = core::str::from_utf8(&data) {
                            let _ = crate::vfs::write(&vfs_path, s);
                            READS.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(_) => {}
                }
            }
            serial_println!("[persist] mounted {} at {} ({} files loaded)",
                "MF16", DISK_MOUNT, files.len());
        }
        Err(e) => {
            serial_println!("[persist] mount: {}", e);
        }
    }

    MOUNTED.store(true, Ordering::SeqCst);
    Ok(())
}

/// Write a file to persistent storage.
/// Writes to both VFS (immediate access) and disk (survives reboot).
pub fn write(path: &str, data: &str) -> Result<(), &'static str> {
    // Always write to VFS
    crate::vfs::write(path, data)?;

    // If path is under /disk/, also persist to diskfs
    if path.starts_with(DISK_MOUNT) && MOUNTED.load(Ordering::SeqCst) {
        let disk_name = &path[DISK_MOUNT.len()..].trim_start_matches('/');
        if !disk_name.is_empty() {
            crate::diskfs::write_file(disk_name, data.as_bytes())?;
            WRITES.fetch_add(1, Ordering::Relaxed);
            BYTES_WRITTEN.fetch_add(data.len() as u64, Ordering::Relaxed);
        }
    }

    Ok(())
}

/// Read a file — tries VFS first, falls back to disk.
pub fn read(path: &str) -> Result<String, &'static str> {
    // Try VFS first (fast)
    if let Ok(data) = crate::vfs::cat(path) {
        return Ok(data);
    }

    // If under /disk/, try loading from diskfs
    if path.starts_with(DISK_MOUNT) && MOUNTED.load(Ordering::SeqCst) {
        let disk_name = &path[DISK_MOUNT.len()..].trim_start_matches('/');
        if let Ok(data) = crate::diskfs::read_file(disk_name) {
            READS.fetch_add(1, Ordering::Relaxed);
            if let Ok(s) = core::str::from_utf8(&data) {
                // Cache in VFS
                let _ = crate::vfs::write(path, s);
                return Ok(String::from(s));
            }
        }
    }

    Err("file not found")
}

/// Delete a file from both VFS and disk.
pub fn delete(path: &str) -> Result<(), &'static str> {
    let _ = crate::vfs::rm(path);

    if path.starts_with(DISK_MOUNT) && MOUNTED.load(Ordering::SeqCst) {
        let disk_name = &path[DISK_MOUNT.len()..].trim_start_matches('/');
        if !disk_name.is_empty() {
            crate::diskfs::delete_file(disk_name)?;
        }
    }

    Ok(())
}

/// Sync all /disk/ files from VFS to disk.
pub fn sync() -> Result<usize, &'static str> {
    if !MOUNTED.load(Ordering::SeqCst) {
        return Err("disk not mounted");
    }

    // List VFS files under /disk/
    let entries = crate::vfs::ls(DISK_MOUNT)?;
    let mut synced = 0;

    for (name, kind) in &entries {
        if *kind == 'f' {
            let vfs_path = format!("{}/{}", DISK_MOUNT, name);
            if let Ok(data) = crate::vfs::cat(&vfs_path) {
                let _ = crate::diskfs::write_file(name, data.as_bytes());
                synced += 1;
            }
        }
    }

    serial_println!("[persist] synced {} files to disk", synced);
    Ok(synced)
}

/// Check if persistent storage is mounted.
pub fn is_mounted() -> bool {
    MOUNTED.load(Ordering::SeqCst)
}

/// Info string.
pub fn info() -> String {
    if !MOUNTED.load(Ordering::SeqCst) {
        return String::from("Persistent storage: not mounted\nUse: mount-disk\n");
    }

    let disk_info = crate::diskfs::info();
    format!(
        "Persistent Storage:\n\
         Mount:         {}\n\
         Status:        mounted\n\
         Writes:        {}\n\
         Reads:         {}\n\
         Bytes written: {}\n\
         Disk:          {}\n",
        DISK_MOUNT,
        WRITES.load(Ordering::Relaxed),
        READS.load(Ordering::Relaxed),
        BYTES_WRITTEN.load(Ordering::Relaxed),
        disk_info,
    )
}

pub fn init() {
    if crate::virtio_blk::is_detected() {
        serial_println!("[persist] virtio disk detected — auto-mounting...");
        let _ = mount();
    } else {
        serial_println!("[persist] no disk — VFS is in-memory only");
    }
}
