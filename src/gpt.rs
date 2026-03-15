/// GPT (GUID Partition Table) parser for MerlionOS.
/// Parses GPT headers and partition entries from raw disk sectors,
/// supporting well-known partition type identification and formatted
/// table output for the shell.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Expected GPT header signature: "EFI PART"
const GPT_SIGNATURE: u64 = 0x5452_4150_2049_4645; // little-endian "EFI PART"

/// Standard sector size in bytes.
const SECTOR_SIZE: usize = 512;

// ---------------------------------------------------------------------------
// Well-known partition type GUIDs (mixed-endian as stored on disk)
// ---------------------------------------------------------------------------

/// EFI System Partition: C12A7328-F81F-11D2-BA4B-00A0C93EC93B
const GUID_EFI_SYSTEM: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11,
    0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

/// Linux filesystem: 0FC63DAF-8483-4772-8E79-3D69D8477DE4
const GUID_LINUX_FS: [u8; 16] = [
    0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47,
    0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D, 0xE4,
];

/// Microsoft Basic Data: EBD0A0A2-B9E5-4433-87C0-68B6B72699C7
const GUID_MS_BASIC_DATA: [u8; 16] = [
    0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44,
    0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7,
];

/// Linux Swap: 0657FD6D-A4AB-43C4-84E5-0933C84B4F4F
const GUID_LINUX_SWAP: [u8; 16] = [
    0x6D, 0xFD, 0x57, 0x06, 0xAB, 0xA4, 0xC4, 0x43,
    0x84, 0xE5, 0x09, 0x33, 0xC8, 0x4B, 0x4F, 0x4F,
];

/// Unused / empty entry (all zeros).
const GUID_UNUSED: [u8; 16] = [0u8; 16];

// ---------------------------------------------------------------------------
// On-disk structures
// ---------------------------------------------------------------------------

/// Raw GPT header as stored at LBA 1 (bytes 0..92 of the sector).
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct GptHeader {
    /// Must be `GPT_SIGNATURE` ("EFI PART").
    pub signature: u64,
    /// GPT revision (typically 0x0001_0000 for 1.0).
    pub revision: u32,
    /// Header size in bytes (usually 92).
    pub header_size: u32,
    /// CRC32 of the header (with this field zeroed during computation).
    pub header_crc32: u32,
    /// Reserved; must be zero.
    pub reserved: u32,
    /// LBA of this header (usually 1).
    pub current_lba: u64,
    /// LBA of the backup header (usually last sector on disk).
    pub backup_lba: u64,
    /// First usable LBA for partitions.
    pub first_usable_lba: u64,
    /// Last usable LBA for partitions.
    pub last_usable_lba: u64,
    /// Disk GUID (mixed-endian).
    pub disk_guid: [u8; 16],
    /// Starting LBA of the partition entry array.
    pub partition_entry_lba: u64,
    /// Number of partition entries in the array.
    pub num_partition_entries: u32,
    /// Size of each partition entry in bytes (usually 128).
    pub partition_entry_size: u32,
    /// CRC32 of the partition entry array.
    pub partition_entries_crc32: u32,
}

/// Raw GPT partition entry (128 bytes minimum).
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct GptEntryRaw {
    /// Partition type GUID (mixed-endian).
    pub type_guid: [u8; 16],
    /// Unique partition GUID (mixed-endian).
    pub unique_guid: [u8; 16],
    /// First LBA of the partition.
    pub starting_lba: u64,
    /// Last LBA of the partition (inclusive).
    pub ending_lba: u64,
    /// Attribute flags.
    pub attributes: u64,
    /// Partition name in UTF-16LE (up to 36 code units, 72 bytes).
    pub name: [u8; 72],
}

// ---------------------------------------------------------------------------
// Parsed partition info
// ---------------------------------------------------------------------------

/// A parsed, human-friendly partition description.
#[derive(Debug, Clone)]
pub struct GptPartition {
    /// Zero-based index in the partition table.
    pub index: u32,
    /// Human-readable partition type name.
    pub type_name: String,
    /// First LBA.
    pub start_lba: u64,
    /// Last LBA (inclusive).
    pub end_lba: u64,
    /// Approximate size in MiB.
    pub size_mb: u64,
    /// Partition name decoded from UTF-16LE.
    pub name: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read a little-endian `u32` from a byte slice at the given offset.
#[inline]
fn read_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

/// Read a little-endian `u64` from a byte slice at the given offset.
#[inline]
fn read_u64(buf: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
        buf[off + 4],
        buf[off + 5],
        buf[off + 6],
        buf[off + 7],
    ])
}

/// Convert a UTF-16LE encoded byte slice to an ASCII-safe `String`.
/// Non-ASCII code points are replaced with '?'. Trailing NUL code
/// units are stripped.
fn utf16le_to_string(raw: &[u8]) -> String {
    let mut s = String::new();
    let pairs = raw.len() / 2;
    for i in 0..pairs {
        let lo = raw[i * 2] as u16;
        let hi = raw[i * 2 + 1] as u16;
        let cp = lo | (hi << 8);
        if cp == 0 {
            break;
        }
        if cp >= 0x20 && cp <= 0x7E {
            s.push(cp as u8 as char);
        } else {
            s.push('?');
        }
    }
    s
}

/// Look up a human-readable name for a partition type GUID.
fn type_guid_name(guid: &[u8; 16]) -> &'static str {
    if *guid == GUID_EFI_SYSTEM {
        "EFI System"
    } else if *guid == GUID_LINUX_FS {
        "Linux filesystem"
    } else if *guid == GUID_MS_BASIC_DATA {
        "Microsoft Basic Data"
    } else if *guid == GUID_LINUX_SWAP {
        "Linux Swap"
    } else if *guid == GUID_UNUSED {
        "Unused"
    } else {
        "Unknown"
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a GPT header from the raw bytes of LBA 1 (a 512-byte sector).
///
/// Returns `None` if the signature does not match `"EFI PART"`.
pub fn parse_header(sector1: &[u8; 512]) -> Option<GptHeader> {
    let sig = read_u64(sector1, 0);
    if sig != GPT_SIGNATURE {
        return None;
    }

    let mut guid = [0u8; 16];
    guid.copy_from_slice(&sector1[56..72]);

    Some(GptHeader {
        signature: sig,
        revision: read_u32(sector1, 8),
        header_size: read_u32(sector1, 12),
        header_crc32: read_u32(sector1, 16),
        reserved: read_u32(sector1, 20),
        current_lba: read_u64(sector1, 24),
        backup_lba: read_u64(sector1, 32),
        first_usable_lba: read_u64(sector1, 40),
        last_usable_lba: read_u64(sector1, 48),
        disk_guid: guid,
        partition_entry_lba: read_u64(sector1, 72),
        num_partition_entries: read_u32(sector1, 80),
        partition_entry_size: read_u32(sector1, 84),
        partition_entries_crc32: read_u32(sector1, 88),
    })
}

/// Parse partition entries from a contiguous byte buffer.
///
/// `data` should contain at least `entry_count * entry_size` bytes
/// starting from the partition entry array on disk. Entries whose
/// type GUID is all-zeros (unused) are skipped.
pub fn parse_entries(data: &[u8], entry_count: u32, entry_size: u32) -> Vec<GptPartition> {
    let mut partitions = Vec::new();
    let esz = entry_size as usize;

    for i in 0..entry_count {
        let base = i as usize * esz;
        if base + 128 > data.len() {
            break;
        }
        let entry_bytes = &data[base..base + esz.min(data.len() - base)];

        // Read type GUID
        let mut type_guid = [0u8; 16];
        type_guid.copy_from_slice(&entry_bytes[0..16]);

        if type_guid == GUID_UNUSED {
            continue;
        }

        let starting_lba = read_u64(entry_bytes, 32);
        let ending_lba = read_u64(entry_bytes, 40);

        // Size in MiB: ((ending - starting + 1) * 512) / 1048576
        let sectors = ending_lba.saturating_sub(starting_lba).saturating_add(1);
        let size_mb = sectors * SECTOR_SIZE as u64 / (1024 * 1024);

        let name_bytes = &entry_bytes[56..128.min(entry_bytes.len())];
        let name = utf16le_to_string(name_bytes);

        partitions.push(GptPartition {
            index: i,
            type_name: String::from(type_guid_name(&type_guid)),
            start_lba: starting_lba,
            end_lba: ending_lba,
            size_mb,
            name,
        });
    }

    partitions
}

/// Format a slice of parsed partitions into a human-readable table string.
///
/// Example output:
/// ```text
/// #   Start LBA    End LBA      Size(MiB) Type                 Name
/// 0   2048         1048575      512       EFI System           boot
/// 2   1048576      62914559     30208     Linux filesystem     rootfs
/// ```
pub fn format_table(partitions: &[GptPartition]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<4}{:<13}{:<13}{:<10}{:<21}{}\n",
        "#", "Start LBA", "End LBA", "Size(MiB)", "Type", "Name"
    ));

    for p in partitions {
        out.push_str(&format!(
            "{:<4}{:<13}{:<13}{:<10}{:<21}{}\n",
            p.index, p.start_lba, p.end_lba, p.size_mb, p.type_name, p.name,
        ));
    }

    out
}
