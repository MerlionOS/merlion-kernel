/// Tar archive reader/writer for MerlionOS.
///
/// Implements the POSIX UStar format with 512-byte headers.
/// Provides parsing, creation, listing, and single-file extraction.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Size of a single tar block (header or data padding unit).
const BLOCK_SIZE: usize = 512;

/// Tar header as stored on disk (POSIX UStar layout, exactly 512 bytes).
#[repr(C)]
#[derive(Clone)]
pub struct TarHeader {
    /// File name (null-terminated, up to 100 bytes).
    pub name: [u8; 100],
    /// File mode in octal ASCII.
    pub mode: [u8; 8],
    /// Owner user ID in octal ASCII.
    pub uid: [u8; 8],
    /// Owner group ID in octal ASCII.
    pub gid: [u8; 8],
    /// File size in octal ASCII.
    pub size: [u8; 12],
    /// Last modification time in octal ASCII (Unix epoch).
    pub mtime: [u8; 12],
    /// Header checksum in octal ASCII.
    pub checksum: [u8; 8],
    /// Entry type: b'0' regular file, b'5' directory, etc.
    pub typeflag: u8,
    /// Name of the linked file (for hard/symlinks).
    pub linkname: [u8; 100],
    /// UStar magic ("ustar\0").
    pub magic: [u8; 6],
    /// UStar version ("00").
    pub version: [u8; 2],
    /// Owner user name.
    pub uname: [u8; 32],
    /// Owner group name.
    pub gname: [u8; 32],
    /// Device major number.
    pub devmajor: [u8; 8],
    /// Device minor number.
    pub devminor: [u8; 8],
    /// Filename prefix for paths longer than 100 bytes.
    pub prefix: [u8; 155],
    /// Padding to reach 512 bytes.
    pub _pad: [u8; 12],
}

const _ASSERT_SIZE: () = assert!(core::mem::size_of::<TarHeader>() == BLOCK_SIZE);

/// A single entry inside a tar archive (header + file data).
#[derive(Clone)]
pub struct TarEntry {
    /// The raw 512-byte header.
    pub header: TarHeader,
    /// File contents (empty for directories).
    pub data: Vec<u8>,
}

/// File type tag returned by [`list_tar`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TarEntryType {
    /// Regular file (typeflag '0' or '\0').
    File,
    /// Directory (typeflag '5').
    Directory,
    /// Symlink (typeflag '2').
    Symlink,
    /// Any other type.
    Other(u8),
}

/// Parse an octal ASCII field into a `usize`.
///
/// Parsing stops at the first null or space terminator, which is
/// standard for tar header fields.
pub fn parse_octal(field: &[u8]) -> usize {
    let mut value: usize = 0;
    for &b in field {
        if b == 0 || b == b' ' {
            break;
        }
        if b >= b'0' && b <= b'7' {
            value = value * 8 + (b - b'0') as usize;
        }
    }
    value
}

/// Extract the null-terminated name string from a header field.
fn name_from_field(field: &[u8]) -> String {
    let len = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..len]).into_owned()
}

/// Classify a typeflag byte into a [`TarEntryType`].
fn classify(typeflag: u8) -> TarEntryType {
    match typeflag {
        0 | b'0' => TarEntryType::File,
        b'5' => TarEntryType::Directory,
        b'2' => TarEntryType::Symlink,
        other => TarEntryType::Other(other),
    }
}

/// Parse a tar archive from raw bytes into a list of entries.
///
/// Stops at the first all-zero block (end-of-archive marker) or when
/// the remaining data is too short for another header.
pub fn parse_tar(data: &[u8]) -> Vec<TarEntry> {
    let mut entries = Vec::new();
    let mut offset = 0;

    while offset + BLOCK_SIZE <= data.len() {
        let hdr = &data[offset..offset + BLOCK_SIZE];
        if hdr.iter().all(|&b| b == 0) {
            break;
        }

        // Safety: TarHeader is repr(C) with only byte-array fields.
        let header: TarHeader = unsafe {
            core::ptr::read_unaligned(hdr.as_ptr() as *const TarHeader)
        };

        let file_size = parse_octal(&header.size);
        let data_start = offset + BLOCK_SIZE;
        let data_end = data_start + file_size;

        let file_data = if data_end <= data.len() {
            data[data_start..data_end].to_vec()
        } else {
            data[data_start..data.len().min(data_end)].to_vec()
        };

        entries.push(TarEntry { header, data: file_data });

        // Advance past header + data rounded up to the next 512-byte block.
        let blocks = (file_size + BLOCK_SIZE - 1) / BLOCK_SIZE;
        offset = data_start + blocks * BLOCK_SIZE;
    }

    entries
}

/// Write an octal value into a fixed-width field with null terminator.
fn write_octal(buf: &mut [u8], value: usize) {
    let len = buf.len();
    buf[len - 1] = 0;
    let mut v = value;
    for i in (0..len - 1).rev() {
        buf[i] = b'0' + (v & 7) as u8;
        v >>= 3;
    }
}

/// Compute the unsigned checksum of a 512-byte tar header.
///
/// Per the POSIX spec the checksum field (bytes 148..156) is treated
/// as eight ASCII space characters during the computation.
fn compute_checksum(header: &[u8; BLOCK_SIZE]) -> usize {
    let mut sum: usize = 0;
    for (i, &b) in header.iter().enumerate() {
        sum += if (148..156).contains(&i) { b' ' as usize } else { b as usize };
    }
    sum
}

/// Create a tar archive from a list of (filename, contents) pairs.
///
/// Produces a valid UStar archive terminated by two zero blocks.
pub fn create_tar(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = Vec::new();

    for &(name, contents) in entries {
        let mut hdr = [0u8; BLOCK_SIZE];

        // Name (max 100 bytes).
        let nb = name.as_bytes();
        hdr[..nb.len().min(100)].copy_from_slice(&nb[..nb.len().min(100)]);

        write_octal(&mut hdr[100..108], 0o644);   // mode
        write_octal(&mut hdr[108..116], 0);        // uid
        write_octal(&mut hdr[116..124], 0);        // gid
        write_octal(&mut hdr[124..136], contents.len()); // size
        write_octal(&mut hdr[136..148], 0);        // mtime
        hdr[156] = b'0';                           // typeflag: regular file
        hdr[257..263].copy_from_slice(b"ustar\0"); // magic
        hdr[263..265].copy_from_slice(b"00");       // version

        // Checksum — fill field with spaces first, then write result.
        hdr[148..156].copy_from_slice(b"        ");
        let cksum = compute_checksum(<&[u8; BLOCK_SIZE]>::try_from(&hdr[..]).unwrap());
        write_octal(&mut hdr[148..155], cksum);
        hdr[155] = b' ';

        archive.extend_from_slice(&hdr);
        archive.extend_from_slice(contents);

        // Pad data to a full 512-byte block.
        let rem = contents.len() % BLOCK_SIZE;
        if rem != 0 {
            archive.extend_from_slice(&vec![0u8; BLOCK_SIZE - rem]);
        }
    }

    // End-of-archive marker: two zero blocks.
    archive.extend_from_slice(&[0u8; BLOCK_SIZE * 2]);
    archive
}

/// List every entry in a tar archive.
///
/// Returns a vector of (name, size, entry type) tuples.
pub fn list_tar(data: &[u8]) -> Vec<(String, usize, TarEntryType)> {
    parse_tar(data)
        .iter()
        .map(|e| {
            (name_from_field(&e.header.name),
             parse_octal(&e.header.size),
             classify(e.header.typeflag))
        })
        .collect()
}

/// Extract a single file from a tar archive by name.
///
/// Returns `None` if the file is not found.
pub fn extract_file(archive: &[u8], name: &str) -> Option<Vec<u8>> {
    parse_tar(archive)
        .into_iter()
        .find(|e| name_from_field(&e.header.name) == name)
        .map(|e| e.data)
}
