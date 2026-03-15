/// Minimal FAT16-like filesystem implementation.
/// Operates on a block device and provides file read/write operations.
/// Simplified: flat directory (no subdirectories), 512-byte clusters.
///
/// Layout on disk:
///   Block 0:     Boot sector / superblock (magic, file count)
///   Block 1-3:   FAT table (maps cluster -> next cluster)
///   Block 4-7:   Root directory (32-byte entries, max 64 files)
///   Block 8+:    Data clusters

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;

const MAGIC: [u8; 4] = *b"MF16";   // MerlionOS FAT16
const FAT_START: usize = 1;
const FAT_BLOCKS: usize = 3;
const DIR_START: usize = 4;
const DIR_BLOCKS: usize = 4;
const DATA_START: usize = 8;
const MAX_FILES: usize = 64;
const ENTRY_SIZE: usize = 32;
const CLUSTER_SIZE: usize = 512;
const FAT_FREE: u16 = 0x0000;
const FAT_EOF: u16 = 0xFFFF;

/// Directory entry (32 bytes).
#[derive(Clone)]
struct DirEntry {
    name: [u8; 16],
    name_len: u8,
    flags: u8,          // 0 = deleted, 1 = active
    start_cluster: u16,
    file_size: u32,
    _reserved: [u8; 10],
}

/// File info for display.
pub struct FileInfo {
    pub name: String,
    pub size: u32,
}

/// Format a RAM disk region as MF16 filesystem.
pub fn format(disk: &mut [u8]) -> Result<(), &'static str> {
    if disk.len() < DATA_START * CLUSTER_SIZE + CLUSTER_SIZE {
        return Err("disk too small");
    }

    // Clear everything
    for b in disk.iter_mut() { *b = 0; }

    // Write magic
    disk[0..4].copy_from_slice(&MAGIC);

    // Initialize FAT: all clusters free
    // FAT entries are 16-bit, starting at block 1
    // Already zero = FAT_FREE

    Ok(())
}

/// List files in the root directory.
pub fn list_files(disk: &[u8]) -> Vec<FileInfo> {
    if disk[0..4] != MAGIC { return Vec::new(); }

    let mut files = Vec::new();
    let dir_offset = DIR_START * CLUSTER_SIZE;

    for i in 0..MAX_FILES {
        let off = dir_offset + i * ENTRY_SIZE;
        if off + ENTRY_SIZE > disk.len() { break; }
        if disk[off + 17] == 1 { // flags: active
            let name_len = disk[off + 16] as usize;
            let name = core::str::from_utf8(&disk[off..off + name_len])
                .unwrap_or("?").to_owned();
            let size = u32::from_le_bytes([
                disk[off + 20], disk[off + 21], disk[off + 22], disk[off + 23],
            ]);
            files.push(FileInfo { name, size });
        }
    }
    files
}

/// Read a file's contents.
pub fn read_file(disk: &[u8], name: &str) -> Option<Vec<u8>> {
    if disk[0..4] != MAGIC { return None; }
    let dir_offset = DIR_START * CLUSTER_SIZE;

    for i in 0..MAX_FILES {
        let off = dir_offset + i * ENTRY_SIZE;
        if disk[off + 17] != 1 { continue; }
        let name_len = disk[off + 16] as usize;
        let entry_name = core::str::from_utf8(&disk[off..off + name_len]).unwrap_or("");
        if entry_name != name { continue; }

        let start_cluster = u16::from_le_bytes([disk[off + 18], disk[off + 19]]);
        let file_size = u32::from_le_bytes([
            disk[off + 20], disk[off + 21], disk[off + 22], disk[off + 23],
        ]);

        // Read cluster chain
        let mut data = Vec::new();
        let mut cluster = start_cluster;
        let mut remaining = file_size as usize;

        while cluster != FAT_EOF && remaining > 0 {
            let data_off = (DATA_START + cluster as usize) * CLUSTER_SIZE;
            let to_read = remaining.min(CLUSTER_SIZE);
            if data_off + to_read <= disk.len() {
                data.extend_from_slice(&disk[data_off..data_off + to_read]);
            }
            remaining -= to_read;
            cluster = read_fat(disk, cluster);
        }

        return Some(data);
    }
    None
}

/// Write a file (create or overwrite).
pub fn write_file(disk: &mut [u8], name: &str, contents: &[u8]) -> Result<(), &'static str> {
    if disk[0..4] != MAGIC { return Err("not formatted as MF16"); }
    if name.len() > 16 { return Err("filename too long"); }

    // Delete existing file with same name
    delete_file(disk, name).ok();

    // Allocate clusters
    let clusters_needed = (contents.len() + CLUSTER_SIZE - 1) / CLUSTER_SIZE;
    if clusters_needed == 0 { return Err("empty file"); }

    let mut allocated = Vec::new();
    let max_clusters = (FAT_BLOCKS * CLUSTER_SIZE) / 2; // FAT entries are 16-bit

    for c in 0..max_clusters as u16 {
        if read_fat(disk, c) == FAT_FREE {
            allocated.push(c);
            if allocated.len() == clusters_needed { break; }
        }
    }

    if allocated.len() < clusters_needed {
        return Err("disk full");
    }

    // Write FAT chain
    for i in 0..allocated.len() {
        let next = if i + 1 < allocated.len() { allocated[i + 1] } else { FAT_EOF };
        write_fat(disk, allocated[i], next);
    }

    // Write data
    let mut written = 0;
    for &cluster in &allocated {
        let data_off = (DATA_START + cluster as usize) * CLUSTER_SIZE;
        let to_write = (contents.len() - written).min(CLUSTER_SIZE);
        disk[data_off..data_off + to_write].copy_from_slice(&contents[written..written + to_write]);
        written += to_write;
    }

    // Write directory entry
    let dir_offset = DIR_START * CLUSTER_SIZE;
    for i in 0..MAX_FILES {
        let off = dir_offset + i * ENTRY_SIZE;
        if disk[off + 17] == 0 { // free slot
            disk[off..off + name.len()].copy_from_slice(name.as_bytes());
            disk[off + 16] = name.len() as u8;
            disk[off + 17] = 1; // active
            disk[off + 18..off + 20].copy_from_slice(&allocated[0].to_le_bytes());
            disk[off + 20..off + 24].copy_from_slice(&(contents.len() as u32).to_le_bytes());
            return Ok(());
        }
    }
    Err("directory full")
}

/// Delete a file.
pub fn delete_file(disk: &mut [u8], name: &str) -> Result<(), &'static str> {
    if disk[0..4] != MAGIC { return Err("not formatted"); }
    let dir_offset = DIR_START * CLUSTER_SIZE;

    for i in 0..MAX_FILES {
        let off = dir_offset + i * ENTRY_SIZE;
        if disk[off + 17] != 1 { continue; }
        let name_len = disk[off + 16] as usize;
        let entry_name = core::str::from_utf8(&disk[off..off + name_len]).unwrap_or("");
        if entry_name != name { continue; }

        // Free cluster chain
        let start = u16::from_le_bytes([disk[off + 18], disk[off + 19]]);
        let mut cluster = start;
        while cluster != FAT_EOF && cluster != FAT_FREE {
            let next = read_fat(disk, cluster);
            write_fat(disk, cluster, FAT_FREE);
            cluster = next;
        }

        // Clear directory entry
        disk[off + 17] = 0;
        return Ok(());
    }
    Err("file not found")
}

fn read_fat(disk: &[u8], cluster: u16) -> u16 {
    let fat_off = FAT_START * CLUSTER_SIZE + (cluster as usize) * 2;
    if fat_off + 2 > disk.len() { return FAT_EOF; }
    u16::from_le_bytes([disk[fat_off], disk[fat_off + 1]])
}

fn write_fat(disk: &mut [u8], cluster: u16, value: u16) {
    let fat_off = FAT_START * CLUSTER_SIZE + (cluster as usize) * 2;
    if fat_off + 2 <= disk.len() {
        disk[fat_off..fat_off + 2].copy_from_slice(&value.to_le_bytes());
    }
}
