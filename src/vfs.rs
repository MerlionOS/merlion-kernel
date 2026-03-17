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
    ProcVersion,
    ProcCpuinfo,
    ProcModules,
    ProcSelf,
}

struct Inode {
    name: String,
    node_type: NodeType,
    parent: usize,     // inode index of parent (0 for root)
    data: Vec<u8>,     // file contents (regular files only)
    owner_uid: u32,
    owner_gid: u32,
}

const MAX_PATH_CACHE: usize = 256;

struct Filesystem {
    inodes: Vec<Inode>,
    path_cache: Vec<(u64, usize)>, // (fnv_hash, inode_index) for fast lookup
}

/// FNV-1a hash for fast path lookups.
fn fnv_hash(path: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in path.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x00000100000001B3);
    }
    hash
}

impl Filesystem {
    fn new() -> Self {
        let mut fs = Self { inodes: Vec::new(), path_cache: Vec::new() };

        // 0: /
        fs.inodes.push(Inode {
            name: String::new(),
            node_type: NodeType::Directory,
            parent: 0,
            data: Vec::new(),
            owner_uid: 0,
            owner_gid: 0,
        });
        // 1: /dev
        fs.inodes.push(Inode {
            name: "dev".to_owned(),
            node_type: NodeType::Directory,
            parent: 0,
            data: Vec::new(),
            owner_uid: 0,
            owner_gid: 0,
        });
        // 2: /dev/null
        fs.inodes.push(Inode {
            name: "null".to_owned(),
            node_type: NodeType::DevNull,
            parent: 1,
            data: Vec::new(),
            owner_uid: 0,
            owner_gid: 0,
        });
        // 3: /dev/serial
        fs.inodes.push(Inode {
            name: "serial".to_owned(),
            node_type: NodeType::DevSerial,
            parent: 1,
            data: Vec::new(),
            owner_uid: 0,
            owner_gid: 0,
        });
        // 4: /proc
        fs.inodes.push(Inode {
            name: "proc".to_owned(),
            node_type: NodeType::Directory,
            parent: 0,
            data: Vec::new(),
            owner_uid: 0,
            owner_gid: 0,
        });
        // 5: /proc/uptime
        fs.inodes.push(Inode {
            name: "uptime".to_owned(),
            node_type: NodeType::ProcUptime,
            parent: 4,
            data: Vec::new(),
            owner_uid: 0,
            owner_gid: 0,
        });
        // 6: /proc/meminfo
        fs.inodes.push(Inode {
            name: "meminfo".to_owned(),
            node_type: NodeType::ProcMeminfo,
            parent: 4,
            data: Vec::new(),
            owner_uid: 0,
            owner_gid: 0,
        });
        // 7: /proc/tasks
        fs.inodes.push(Inode {
            name: "tasks".to_owned(),
            node_type: NodeType::ProcTasks,
            parent: 4,
            data: Vec::new(),
            owner_uid: 0,
            owner_gid: 0,
        });
        // 8: /proc/version
        fs.inodes.push(Inode {
            name: "version".to_owned(),
            node_type: NodeType::ProcVersion,
            parent: 4,
            data: Vec::new(),
            owner_uid: 0,
            owner_gid: 0,
        });
        // 9: /proc/cpuinfo
        fs.inodes.push(Inode {
            name: "cpuinfo".to_owned(),
            node_type: NodeType::ProcCpuinfo,
            parent: 4,
            data: Vec::new(),
            owner_uid: 0,
            owner_gid: 0,
        });
        // 10: /proc/modules
        fs.inodes.push(Inode {
            name: "modules".to_owned(),
            node_type: NodeType::ProcModules,
            parent: 4,
            data: Vec::new(),
            owner_uid: 0,
            owner_gid: 0,
        });
        // 11: /proc/self
        fs.inodes.push(Inode {
            name: "self".to_owned(),
            node_type: NodeType::ProcSelf,
            parent: 4,
            data: Vec::new(),
            owner_uid: 0,
            owner_gid: 0,
        });
        // 12: /etc
        fs.inodes.push(Inode {
            name: "etc".to_owned(),
            node_type: NodeType::Directory,
            parent: 0,
            data: Vec::new(),
            owner_uid: 0,
            owner_gid: 0,
        });
        // 13: /tmp
        fs.inodes.push(Inode {
            name: "tmp".to_owned(),
            node_type: NodeType::Directory,
            parent: 0,
            data: Vec::new(),
            owner_uid: 0,
            owner_gid: 0,
        });

        // Populate path cache for initial inodes
        fs.populate_initial_cache();

        fs
    }

    /// Build the full path string for an inode by walking parents.
    fn build_path(&self, idx: usize) -> String {
        if idx == 0 {
            return "/".to_owned();
        }
        let mut parts: Vec<&str> = Vec::new();
        let mut cur = idx;
        while cur != 0 {
            parts.push(&self.inodes[cur].name);
            cur = self.inodes[cur].parent;
        }
        parts.reverse();
        let mut path = String::new();
        for p in &parts {
            path.push('/');
            path.push_str(p);
        }
        path
    }

    /// Populate path cache for all existing inodes.
    fn populate_initial_cache(&mut self) {
        for idx in 0..self.inodes.len() {
            let path = self.build_path(idx);
            let hash = fnv_hash(&path);
            if self.path_cache.len() < MAX_PATH_CACHE {
                self.path_cache.push((hash, idx));
            }
        }
    }

    /// Add an entry to the path cache, evicting oldest if full.
    fn cache_insert(&mut self, path: &str, idx: usize) {
        let hash = fnv_hash(path);
        // Don't add duplicates
        for &(h, i) in &self.path_cache {
            if h == hash && i == idx {
                return;
            }
        }
        if self.path_cache.len() >= MAX_PATH_CACHE {
            self.path_cache.remove(0); // evict oldest
        }
        self.path_cache.push((hash, idx));
    }

    /// Remove an inode from the path cache.
    fn cache_remove(&mut self, idx: usize) {
        self.path_cache.retain(|&(_, i)| i != idx);
    }

    /// Resolve a path to an inode index (cache-accelerated).
    fn resolve(&self, path: &str) -> Option<usize> {
        // Check cache first
        let hash = fnv_hash(path);
        for &(h, idx) in &self.path_cache {
            if h == hash && idx < self.inodes.len() {
                // Verify by re-resolving to confirm (hash collision guard)
                if let Some(verified) = self.resolve_uncached(path) {
                    if verified == idx {
                        return Some(idx);
                    }
                }
            }
        }

        // Fall through to linear search
        self.resolve_uncached(path)
    }

    /// Resolve a path to an inode index (uncached linear search).
    fn resolve_uncached(&self, path: &str) -> Option<usize> {
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
            NodeType::ProcVersion => {
                alloc::format!("MerlionOS v2.0.0 (x86_64)\nBorn for AI. Built by AI.\n")
            }
            NodeType::ProcCpuinfo => {
                let features = crate::smp::detect_features();
                alloc::format!(
                    "CPU:    {}\nCores:  {}\nAPIC:   {}\nSSE:    {}\n",
                    features.brand, features.logical_cores,
                    if features.has_apic { "yes" } else { "no" },
                    if features.has_sse { "yes" } else { "no" },
                )
            }
            NodeType::ProcModules => {
                let mut out = String::new();
                for m in crate::module::list() {
                    let state = match m.state {
                        crate::module::ModuleState::Loaded => "loaded",
                        crate::module::ModuleState::Unloaded => "unloaded",
                    };
                    out.push_str(&alloc::format!("{} {} {}\n", m.name, m.version, state));
                }
                out
            }
            NodeType::ProcSelf => {
                let pid = crate::task::current_pid();
                alloc::format!("pid: {}\n", pid)
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
            owner_uid: crate::security::current_uid(),
            owner_gid: crate::security::current_gid(),
        });
        // Add to path cache
        let path = self.build_path(idx);
        self.cache_insert(&path, idx);
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
        // Remove from path cache before mutating
        self.cache_remove(idx);
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
    if !crate::security::can_read(path) {
        crate::serial_println!("[audit] read denied: uid={} path={}", crate::security::current_uid(), path);
        return Err("permission denied");
    }
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
    // Check write permission (for existing files)
    if !crate::security::can_write(path) {
        crate::serial_println!("[audit] write denied: uid={} path={}", crate::security::current_uid(), path);
        return Err("permission denied");
    }
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
    if !crate::security::can_write(path) {
        crate::serial_println!("[audit] delete denied: uid={} path={}", crate::security::current_uid(), path);
        return Err("permission denied");
    }
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

/// List directory with permissions (ls -l style).
pub fn ls_long(path: &str) -> Result<Vec<String>, &'static str> {
    let vfs = VFS.lock();
    let fs = vfs.as_ref().ok_or("VFS not initialized")?;
    let idx = fs.resolve(path).ok_or("path not found")?;

    if fs.inodes[idx].node_type != NodeType::Directory {
        return Err("not a directory");
    }

    let entries: Vec<String> = fs
        .list_dir(idx)
        .iter()
        .map(|node| {
            let is_dir = node.node_type == NodeType::Directory;
            let full_path = if path == "/" {
                alloc::format!("/{}", node.name)
            } else {
                alloc::format!("{}/{}", path, node.name)
            };
            let perm_str = crate::security::format_perm_ls(&full_path, is_dir);
            alloc::format!("{} {}", perm_str, node.name)
        })
        .collect();

    Ok(entries)
}

/// Create a directory.
pub fn mkdir(path: &str) -> Result<(), &'static str> {
    let (parent_path, dirname) = path.rsplit_once('/').ok_or("invalid path")?;
    let parent_path = if parent_path.is_empty() { "/" } else { parent_path };

    // Check write permission on parent
    if !crate::security::can_write(parent_path) {
        return Err("permission denied");
    }

    let mut vfs = VFS.lock();
    let fs = vfs.as_mut().ok_or("VFS not initialized")?;
    let parent_idx = fs.resolve(parent_path).ok_or("parent not found")?;

    if fs.inodes[parent_idx].node_type != NodeType::Directory {
        return Err("parent is not a directory");
    }
    if fs.inodes.len() >= MAX_INODES {
        return Err("filesystem full");
    }
    // Check if already exists
    for (i, inode) in fs.inodes.iter().enumerate() {
        if i != 0 && inode.parent == parent_idx && inode.name == dirname {
            return Err("directory already exists");
        }
    }

    fs.inodes.push(Inode {
        name: dirname.to_owned(),
        node_type: NodeType::Directory,
        parent: parent_idx,
        data: Vec::new(),
        owner_uid: crate::security::current_uid(),
        owner_gid: crate::security::current_gid(),
    });
    Ok(())
}
