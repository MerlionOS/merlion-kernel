/// File descriptor table.
/// Provides POSIX-like open/read/write/close operations.
/// Each task has its own file descriptor table (simplified: global for now).

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use spin::Mutex;

const MAX_FDS: usize = 32;

static FD_TABLE: Mutex<[FdEntry; MAX_FDS]> = Mutex::new([const { FdEntry::Free }; MAX_FDS]);

#[derive(Clone)]
enum FdEntry {
    Free,
    Open {
        path: String,
        kind: FdKind,
        offset: usize,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum FdKind {
    VfsFile,
    Serial,
    Null,
}

/// Open a file, returns a file descriptor number.
pub fn open(path: &str) -> Result<usize, &'static str> {
    let kind = match path {
        "/dev/null" => FdKind::Null,
        "/dev/serial" => FdKind::Serial,
        _ => FdKind::VfsFile,
    };

    let mut table = FD_TABLE.lock();
    // fd 0, 1, 2 are stdin/stdout/stderr — start from 3
    for i in 3..MAX_FDS {
        if matches!(table[i], FdEntry::Free) {
            table[i] = FdEntry::Open {
                path: path.to_owned(),
                kind,
                offset: 0,
            };
            return Ok(i);
        }
    }
    Err("too many open files")
}

/// Close a file descriptor.
pub fn close(fd: usize) -> Result<(), &'static str> {
    if fd >= MAX_FDS { return Err("invalid fd"); }
    let mut table = FD_TABLE.lock();
    if matches!(table[fd], FdEntry::Free) {
        return Err("fd not open");
    }
    table[fd] = FdEntry::Free;
    Ok(())
}

/// Read from a file descriptor. Returns bytes read.
pub fn read(fd: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
    if fd >= MAX_FDS { return Err("invalid fd"); }
    let table = FD_TABLE.lock();
    match &table[fd] {
        FdEntry::Free => Err("fd not open"),
        FdEntry::Open { path, kind, .. } => {
            match kind {
                FdKind::Null => Ok(0), // EOF
                FdKind::Serial => Ok(0), // no input yet
                FdKind::VfsFile => {
                    match crate::vfs::cat(path) {
                        Ok(content) => {
                            let bytes = content.as_bytes();
                            let to_copy = buf.len().min(bytes.len());
                            buf[..to_copy].copy_from_slice(&bytes[..to_copy]);
                            Ok(to_copy)
                        }
                        Err(e) => Err(e),
                    }
                }
            }
        }
    }
}

/// Write to a file descriptor. Returns bytes written.
pub fn write(fd: usize, data: &[u8]) -> Result<usize, &'static str> {
    if fd >= MAX_FDS { return Err("invalid fd"); }
    let table = FD_TABLE.lock();
    match &table[fd] {
        FdEntry::Free => Err("fd not open"),
        FdEntry::Open { path, kind, .. } => {
            match kind {
                FdKind::Null => Ok(data.len()), // discard
                FdKind::Serial => {
                    if let Ok(s) = core::str::from_utf8(data) {
                        crate::serial_println!("{}", s);
                    }
                    Ok(data.len())
                }
                FdKind::VfsFile => {
                    if let Ok(s) = core::str::from_utf8(data) {
                        crate::vfs::write(path, s)?;
                    }
                    Ok(data.len())
                }
            }
        }
    }
}

/// List open file descriptors.
pub fn list_open() -> Vec<(usize, String, &'static str)> {
    let table = FD_TABLE.lock();
    let mut result = Vec::new();
    for (i, entry) in table.iter().enumerate() {
        if let FdEntry::Open { path, kind, .. } = entry {
            let kind_str = match kind {
                FdKind::VfsFile => "file",
                FdKind::Serial => "serial",
                FdKind::Null => "null",
            };
            result.push((i, path.clone(), kind_str));
        }
    }
    result
}

/// Initialize: open stdin(0), stdout(1), stderr(2).
pub fn init() {
    let mut table = FD_TABLE.lock();
    table[0] = FdEntry::Open { path: "/dev/serial".to_owned(), kind: FdKind::Serial, offset: 0 };
    table[1] = FdEntry::Open { path: "/dev/serial".to_owned(), kind: FdKind::Serial, offset: 0 };
    table[2] = FdEntry::Open { path: "/dev/serial".to_owned(), kind: FdKind::Serial, offset: 0 };
}
