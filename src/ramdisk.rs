/// RAM disk block device with a simple flat filesystem.
/// Provides a 128K in-memory disk for persistent file storage.
///
/// Filesystem layout:
///   Block 0: Superblock (magic, file count)
///   Blocks 1-15: File headers (name + offset + size, 16 files max)
///   Blocks 16+: File data

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use spin::Mutex;

const DISK_SIZE: usize = 128 * 1024; // 128 KiB
const BLOCK_SIZE: usize = 512;
const MAX_FILES: usize = 16;
const DATA_START: usize = 16 * BLOCK_SIZE; // blocks 0-15 reserved
const MAGIC: u32 = 0x4D524C4E; // "MRLN"
const MAX_NAME: usize = 32;
const MAX_FILE_DATA: usize = DISK_SIZE - DATA_START;

pub static RAMDISK: Mutex<RamDisk> = Mutex::new(RamDisk::new());

pub struct RamDisk {
    pub data: [u8; DISK_SIZE],
    pub formatted: bool,
}

/// File header stored in blocks 1-15.
#[derive(Clone)]
struct FileEntry {
    name: [u8; MAX_NAME],
    name_len: u8,
    data_offset: u32, // offset from DATA_START
    data_size: u32,
    active: bool,
}

impl RamDisk {
    const fn new() -> Self {
        Self {
            data: [0; DISK_SIZE],
            formatted: false,
        }
    }

    /// Format the RAM disk with an empty filesystem.
    pub fn format(&mut self) {
        self.data.fill(0);
        // Write magic to superblock
        let magic = MAGIC.to_le_bytes();
        self.data[0..4].copy_from_slice(&magic);
        self.formatted = true;
    }

    pub fn is_formatted(&self) -> bool {
        self.formatted || u32::from_le_bytes([
            self.data[0], self.data[1], self.data[2], self.data[3]
        ]) == MAGIC
    }

    /// List all files.
    pub fn list_files(&self) -> Vec<(String, usize)> {
        if !self.is_formatted() { return Vec::new(); }
        let mut result = Vec::new();
        for i in 0..MAX_FILES {
            if let Some(entry) = self.read_entry(i) {
                if entry.active {
                    let name = core::str::from_utf8(&entry.name[..entry.name_len as usize])
                        .unwrap_or("?").to_owned();
                    result.push((name, entry.data_size as usize));
                }
            }
        }
        result
    }

    /// Read a file's contents.
    pub fn read_file(&self, name: &str) -> Option<Vec<u8>> {
        if !self.is_formatted() { return None; }
        for i in 0..MAX_FILES {
            if let Some(entry) = self.read_entry(i) {
                if entry.active {
                    let entry_name = core::str::from_utf8(&entry.name[..entry.name_len as usize])
                        .unwrap_or("");
                    if entry_name == name {
                        let start = DATA_START + entry.data_offset as usize;
                        let end = start + entry.data_size as usize;
                        return Some(self.data[start..end].to_vec());
                    }
                }
            }
        }
        None
    }

    /// Write a file. Overwrites if it exists, creates if it doesn't.
    pub fn write_file(&mut self, name: &str, contents: &[u8]) -> Result<(), &'static str> {
        if !self.is_formatted() { return Err("disk not formatted"); }
        if name.len() > MAX_NAME { return Err("filename too long"); }
        if contents.len() > MAX_FILE_DATA { return Err("file too large"); }

        // Delete existing file with same name
        self.delete_file(name).ok();

        // Find free entry slot
        let slot = (0..MAX_FILES)
            .find(|&i| {
                self.read_entry(i)
                    .map(|e| !e.active)
                    .unwrap_or(true)
            })
            .ok_or("no free file slots")?;

        // Find free space: pack after the last file
        let data_offset = self.next_free_offset();
        if data_offset + contents.len() > MAX_FILE_DATA {
            return Err("disk full");
        }

        // Write data
        let abs_offset = DATA_START + data_offset;
        self.data[abs_offset..abs_offset + contents.len()].copy_from_slice(contents);

        // Write entry
        let mut entry_name = [0u8; MAX_NAME];
        entry_name[..name.len()].copy_from_slice(name.as_bytes());

        self.write_entry(slot, &FileEntry {
            name: entry_name,
            name_len: name.len() as u8,
            data_offset: data_offset as u32,
            data_size: contents.len() as u32,
            active: true,
        });

        Ok(())
    }

    /// Delete a file by name.
    pub fn delete_file(&mut self, name: &str) -> Result<(), &'static str> {
        if !self.is_formatted() { return Err("disk not formatted"); }
        for i in 0..MAX_FILES {
            if let Some(entry) = self.read_entry(i) {
                if entry.active {
                    let entry_name = core::str::from_utf8(&entry.name[..entry.name_len as usize])
                        .unwrap_or("");
                    if entry_name == name {
                        let mut e = entry;
                        e.active = false;
                        self.write_entry(i, &e);
                        return Ok(());
                    }
                }
            }
        }
        Err("file not found")
    }

    /// Total used data bytes.
    pub fn used_bytes(&self) -> usize {
        if !self.is_formatted() { return 0; }
        self.list_files().iter().map(|(_, s)| s).sum()
    }

    fn next_free_offset(&self) -> usize {
        let mut max_end = 0usize;
        for i in 0..MAX_FILES {
            if let Some(entry) = self.read_entry(i) {
                if entry.active {
                    let end = entry.data_offset as usize + entry.data_size as usize;
                    if end > max_end { max_end = end; }
                }
            }
        }
        max_end
    }

    fn read_entry(&self, index: usize) -> Option<FileEntry> {
        if index >= MAX_FILES { return None; }
        let base = BLOCK_SIZE + index * 48; // 48 bytes per entry
        if base + 48 > DATA_START { return None; }

        let mut name = [0u8; MAX_NAME];
        name.copy_from_slice(&self.data[base..base + MAX_NAME]);
        let name_len = self.data[base + 32];
        let active = self.data[base + 33] != 0;
        let data_offset = u32::from_le_bytes([
            self.data[base + 34], self.data[base + 35],
            self.data[base + 36], self.data[base + 37],
        ]);
        let data_size = u32::from_le_bytes([
            self.data[base + 38], self.data[base + 39],
            self.data[base + 40], self.data[base + 41],
        ]);

        Some(FileEntry { name, name_len, data_offset, data_size, active })
    }

    fn write_entry(&mut self, index: usize, entry: &FileEntry) {
        let base = BLOCK_SIZE + index * 48;
        self.data[base..base + MAX_NAME].copy_from_slice(&entry.name);
        self.data[base + 32] = entry.name_len;
        self.data[base + 33] = if entry.active { 1 } else { 0 };
        self.data[base + 34..base + 38].copy_from_slice(&entry.data_offset.to_le_bytes());
        self.data[base + 38..base + 42].copy_from_slice(&entry.data_size.to_le_bytes());
    }
}
