/// Virtual Filesystem (VFS).
/// In-memory filesystem with directories, regular files, and device/proc nodes.
/// Provides a Unix-like path-based interface for the kernel shell.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use spin::Mutex;

static VFS: Mutex<Option<Filesystem>> = Mutex::new(None);

const MAX_INODES: usize = 64;
const MAX_FILE_SIZE: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NodeType {
    Directory,
    RegularFile,
    DevNull,
    DevSerial,
    ProcUptime,
    ProcMeminfo,
    ProcTasks,
}

struct Inode {
    name: String,
    node_type: NodeType,
    parent: usize,     // inode index of parent (0 for root)
    data: Vec<u8>,     // file contents (regular files only)
}

struct Filesystem {
    inodes: Vec<Inode>,
}

impl Filesystem {
    fn new() -> Self {
        let mut fs = Self { inodes: Vec::new() };

        // 0: /
        fs.inodes.push(Inode {
            name: String::new(),
            node_type: NodeType::Directory,
            parent: 0,
            data: Vec::new(),
        });
        // 1: /dev
        fs.inodes.push(Inode {
            name: "dev".to_owned(),
            node_type: NodeType::Directory,
            parent: 0,
            data: Vec::new(),
        });
        // 2: /dev/null
        fs.inodes.push(Inode {
            name: "null".to_owned(),
            node_type: NodeType::DevNull,
            parent: 1,
            data: Vec::new(),
        });
        // 3: /dev/serial
        fs.inodes.push(Inode {
            name: "serial".to_owned(),
            node_type: NodeType::DevSerial,
            parent: 1,
            data: Vec::new(),
        });
        // 4: /proc
        fs.inodes.push(Inode {
            name: "proc".to_owned(),
            node_type: NodeType::Directory,
            parent: 0,
            data: Vec::new(),
        });
        // 5: /proc/uptime
        fs.inodes.push(Inode {
            name: "uptime".to_owned(),
            node_type: NodeType::ProcUptime,
            parent: 4,
            data: Vec::new(),
        });
        // 6: /proc/meminfo
        fs.inodes.push(Inode {
            name: "meminfo".to_owned(),
            node_type: NodeType::ProcMeminfo,
            parent: 4,
            data: Vec::new(),
        });
        // 7: /proc/tasks
        fs.inodes.push(Inode {
            name: "tasks".to_owned(),
            node_type: NodeType::ProcTasks,
            parent: 4,
            data: Vec::new(),
        });
        // 8: /tmp
        fs.inodes.push(Inode {
            name: "tmp".to_owned(),
            node_type: NodeType::Directory,
            parent: 0,
            data: Vec::new(),
        });

        fs
    }

    /// Resolve a path to an inode index.
    fn resolve(&self, path: &str) -> Option<usize> {
        if path == "/" {
            return Some(0);
        }

        let path = path.trim_start_matches('/');
        let mut current = 0usize; // start at root

        for component in path.split('/') {
            if component.is_empty() {
                continue;
            }
            // Find child of current with matching name
            let mut found = false;
            for (i, inode) in self.inodes.iter().enumerate() {
                if i != 0 && inode.parent == current && inode.name == component {
                    current = i;
                    found = true;
                    break;
                }
            }
            if !found {
                return None;
            }
        }

        Some(current)
    }

    /// List children of a directory.
    fn list_dir(&self, dir_idx: usize) -> Vec<&Inode> {
        self.inodes
            .iter()
            .enumerate()
            .filter(|&(i, node)| i != 0 && node.parent == dir_idx)
            .map(|(_, node)| node)
            .collect()
    }

    /// Read a file's contents. For proc/dev nodes, generates content dynamically.
    fn read_file(&self, idx: usize) -> String {
        let inode = &self.inodes[idx];
        match inode.node_type {
            NodeType::RegularFile => {
                String::from_utf8_lossy(&inode.data).into_owned()
            }
            NodeType::DevNull => String::new(),
            NodeType::DevSerial => "(serial device)\n".to_owned(),
            NodeType::ProcUptime => {
                let (h, m, s) = crate::timer::uptime_hms();
                let ticks = crate::timer::ticks();
                alloc::format!("{:02}:{:02}:{:02} ({} ticks)\n", h, m, s, ticks)
            }
            NodeType::ProcMeminfo => {
                let stats = crate::allocator::stats();
                alloc::format!(
                    "Heap total:  {} bytes\nHeap used:   {} bytes\nHeap free:   {} bytes\n",
                    stats.total, stats.used, stats.free
                )
            }
            NodeType::ProcTasks => {
                let mut out = String::from("PID  STATE     NAME\n");
                for t in crate::task::list() {
                    let st = match t.state {
                        crate::task::TaskState::Running  => "running ",
                        crate::task::TaskState::Ready    => "ready   ",
                        crate::task::TaskState::Finished => "finished",
                    };
                    out.push_str(&alloc::format!("{:3}  {}  {}\n", t.pid, st, t.name));
                }
                out
            }
            NodeType::Directory => "(directory)\n".to_owned(),
        }
    }

    /// Write data to a file. Creates regular files in writable directories.
    fn write_file(&mut self, idx: usize, data: &[u8]) -> Result<(), &'static str> {
        let inode = &mut self.inodes[idx];
        match inode.node_type {
            NodeType::RegularFile => {
                if data.len() > MAX_FILE_SIZE {
                    return Err("file too large");
                }
                inode.data = data.to_vec();
                Ok(())
            }
            NodeType::DevNull => Ok(()), // discard
            NodeType::DevSerial => {
                if let Ok(s) = core::str::from_utf8(data) {
                    crate::serial_println!("{}", s);
                }
                Ok(())
            }
            _ => Err("cannot write to this file"),
        }
    }

    /// Create a new regular file in a directory.
    fn create_file(&mut self, parent: usize, name: &str) -> Result<usize, &'static str> {
        if self.inodes[parent].node_type != NodeType::Directory {
            return Err("parent is not a directory");
        }
        if self.inodes.len() >= MAX_INODES {
            return Err("filesystem full");
        }
        // Check if name already exists
        for (i, inode) in self.inodes.iter().enumerate() {
            if i != 0 && inode.parent == parent && inode.name == name {
                return Ok(i); // already exists, return it
            }
        }
        let idx = self.inodes.len();
        self.inodes.push(Inode {
            name: name.to_owned(),
            node_type: NodeType::RegularFile,
            parent,
            data: Vec::new(),
        });
        Ok(idx)
    }

    /// Delete a regular file by index.
    fn delete_file(&mut self, idx: usize) -> Result<(), &'static str> {
        if idx == 0 || idx >= self.inodes.len() {
            return Err("invalid inode");
        }
        if self.inodes[idx].node_type != NodeType::RegularFile {
            return Err("can only delete regular files");
        }
        // Check no children reference this (shouldn't for regular files)
        self.inodes[idx].data.clear();
        self.inodes[idx].name = String::from("(deleted)");
        self.inodes[idx].node_type = NodeType::DevNull; // mark as dead
        Ok(())
    }
}

// --- Public API ---

/// Initialize the VFS with default structure.
pub fn init() {
    *VFS.lock() = Some(Filesystem::new());
}

/// List directory contents. Returns (name, type_char) pairs.
pub fn ls(path: &str) -> Result<Vec<(String, char)>, &'static str> {
    let vfs = VFS.lock();
    let fs = vfs.as_ref().ok_or("VFS not initialized")?;
    let idx = fs.resolve(path).ok_or("path not found")?;

    if fs.inodes[idx].node_type != NodeType::Directory {
        return Err("not a directory");
    }

    let entries: Vec<(String, char)> = fs
        .list_dir(idx)
        .iter()
        .map(|node| {
            let type_char = match node.node_type {
                NodeType::Directory => 'd',
                NodeType::RegularFile => '-',
                _ => 'c', // device/proc
            };
            (node.name.clone(), type_char)
        })
        .collect();

    Ok(entries)
}

/// Read file contents.
pub fn cat(path: &str) -> Result<String, &'static str> {
    let vfs = VFS.lock();
    let fs = vfs.as_ref().ok_or("VFS not initialized")?;
    let idx = fs.resolve(path).ok_or("file not found")?;

    if fs.inodes[idx].node_type == NodeType::Directory {
        return Err("is a directory");
    }

    Ok(fs.read_file(idx))
}

/// Write string data to a file. Creates the file if it doesn't exist in /tmp.
pub fn write(path: &str, data: &str) -> Result<(), &'static str> {
    let mut vfs = VFS.lock();
    let fs = vfs.as_mut().ok_or("VFS not initialized")?;

    let idx = if let Some(idx) = fs.resolve(path) {
        idx
    } else {
        // Try to create the file — extract parent dir and filename
        let (parent_path, filename) = path.rsplit_once('/').ok_or("invalid path")?;
        let parent_path = if parent_path.is_empty() { "/" } else { parent_path };
        let parent_idx = fs.resolve(parent_path).ok_or("parent directory not found")?;
        fs.create_file(parent_idx, filename)?
    };

    fs.write_file(idx, data.as_bytes())
}

/// Remove a file.
pub fn rm(path: &str) -> Result<(), &'static str> {
    let mut vfs = VFS.lock();
    let fs = vfs.as_mut().ok_or("VFS not initialized")?;
    let idx = fs.resolve(path).ok_or("file not found")?;
    fs.delete_file(idx)
}

/// Check if a path exists.
#[allow(dead_code)]
pub fn exists(path: &str) -> bool {
    let vfs = VFS.lock();
    vfs.as_ref()
        .and_then(|fs| fs.resolve(path))
        .is_some()
}
