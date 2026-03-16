/// tmpfs — size-limited in-memory filesystem for MerlionOS.
/// Provides fast temporary storage with configurable size limits,
/// file permissions, and automatic cleanup on unmount.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Maximum number of mounts.
const MAX_MOUNTS: usize = 8;

/// Maximum files per tmpfs instance.
const MAX_FILES: usize = 128;

/// Maximum subdirectories per tmpfs instance.
const MAX_DIRS: usize = 32;

/// Default max size for /tmp (64 KiB).
const DEFAULT_MAX_SIZE: usize = 65536;

/// File permissions (rwx style).
#[derive(Debug, Clone, Copy)]
pub struct Permissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

impl Permissions {
    pub const fn default_file() -> Self {
        Self { read: true, write: true, execute: false }
    }

    pub const fn default_dir() -> Self {
        Self { read: true, write: true, execute: true }
    }

    pub fn mode_string(&self) -> String {
        format!("{}{}{}",
            if self.read { 'r' } else { '-' },
            if self.write { 'w' } else { '-' },
            if self.execute { 'x' } else { '-' },
        )
    }
}

/// An in-memory file stored in tmpfs.
#[derive(Clone)]
pub struct TmpfsFile {
    pub name: String,
    pub data: Vec<u8>,
    pub permissions: Permissions,
    pub owner_uid: u32,
    pub owner_gid: u32,
    pub created_tick: u64,
    pub modified_tick: u64,
    pub accessed_tick: u64,
}

impl TmpfsFile {
    fn size(&self) -> usize {
        self.data.len()
    }
}

/// A directory in tmpfs containing files and subdirectories.
#[derive(Clone)]
pub struct TmpfsDir {
    pub name: String,
    pub files: Vec<TmpfsFile>,
    pub subdirs: Vec<TmpfsDir>,
    pub permissions: Permissions,
}

impl TmpfsDir {
    fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            files: Vec::new(),
            subdirs: Vec::new(),
            permissions: Permissions::default_dir(),
        }
    }

    fn find_file(&self, name: &str) -> Option<usize> {
        self.files.iter().position(|f| f.name == name)
    }

    fn find_subdir(&self, name: &str) -> Option<usize> {
        self.subdirs.iter().position(|d| d.name == name)
    }

    fn total_size(&self) -> usize {
        let file_size: usize = self.files.iter().map(|f| f.size()).sum();
        let sub_size: usize = self.subdirs.iter().map(|d| d.total_size()).sum();
        file_size + sub_size
    }

    fn file_count(&self) -> usize {
        let own = self.files.len();
        let sub: usize = self.subdirs.iter().map(|d| d.file_count()).sum();
        own + sub
    }
}

/// A mounted tmpfs instance.
struct TmpfsMount {
    path: String,
    root: TmpfsDir,
    max_size: usize,
    inode_counter: u64,
}

impl TmpfsMount {
    fn new(path: &str, max_size: usize) -> Self {
        Self {
            path: String::from(path),
            root: TmpfsDir::new("/"),
            max_size,
            inode_counter: 1,
        }
    }

    fn current_size(&self) -> usize {
        self.root.total_size()
    }

    fn file_count(&self) -> usize {
        self.root.file_count()
    }

    fn next_inode(&mut self) -> u64 {
        let id = self.inode_counter;
        self.inode_counter += 1;
        id
    }

    /// Navigate to a subdirectory by path components, returning mutable ref.
    fn navigate_mut<'a>(&'a mut self, parts: &[&str]) -> Option<&'a mut TmpfsDir> {
        let mut dir = &mut self.root;
        for &part in parts {
            if part.is_empty() { continue; }
            let idx = dir.find_subdir(part)?;
            dir = &mut dir.subdirs[idx];
        }
        Some(dir)
    }

    /// Navigate to a subdirectory by path components, returning shared ref.
    fn navigate<'a>(&'a self, parts: &[&str]) -> Option<&'a TmpfsDir> {
        let mut dir = &self.root;
        for &part in parts {
            if part.is_empty() { continue; }
            let idx = dir.find_subdir(part)?;
            dir = &dir.subdirs[idx];
        }
        Some(dir)
    }
}

/// Global tmpfs state.
struct TmpfsState {
    mounts: Vec<TmpfsMount>,
}

static STATE: Mutex<TmpfsState> = Mutex::new(TmpfsState { mounts: Vec::new() });
static TOTAL_WRITES: AtomicU64 = AtomicU64::new(0);
static TOTAL_READS: AtomicU64 = AtomicU64::new(0);

fn current_tick() -> u64 {
    crate::timer::ticks()
}

/// Split a path into (mount_path, relative components).
fn split_path(path: &str) -> (&str, Vec<&str>) {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if parts.is_empty() {
        ("", Vec::new())
    } else {
        let mount = parts[0];
        let rest = parts[1..].to_vec();
        (mount, rest)
    }
}

/// Find mount index for a given path.
fn find_mount(state: &TmpfsState, path: &str) -> Option<usize> {
    let mount_name = path.trim_start_matches('/').split('/').next().unwrap_or("");
    state.mounts.iter().position(|m| {
        let mp = m.path.trim_start_matches('/');
        mp == mount_name
    })
}

// ── Public API ──

/// Mount a new tmpfs at `path` with the given max size in bytes.
pub fn mount(path: &str, max_size: usize) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    if state.mounts.len() >= MAX_MOUNTS {
        return Err("max tmpfs mounts reached");
    }
    if state.mounts.iter().any(|m| m.path == path) {
        return Err("already mounted");
    }
    state.mounts.push(TmpfsMount::new(path, max_size));
    Ok(())
}

/// Unmount a tmpfs, discarding all data.
pub fn unmount(path: &str) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    let idx = state.mounts.iter().position(|m| m.path == path)
        .ok_or("not mounted")?;
    state.mounts.remove(idx);
    Ok(())
}

/// List all tmpfs mount points.
pub fn list_mounts() -> Vec<String> {
    let state = STATE.lock();
    state.mounts.iter().map(|m| m.path.clone()).collect()
}

/// Create a file. `path` is e.g. "/tmp/foo.txt".
pub fn create(path: &str, data: &[u8]) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    let idx = find_mount(&state, path).ok_or("no tmpfs mount for path")?;
    let mount = &mut state.mounts[idx];

    if mount.current_size() + data.len() > mount.max_size {
        return Err("tmpfs size limit exceeded");
    }
    if mount.file_count() >= MAX_FILES {
        return Err("max file count reached");
    }

    let (_mp, parts) = split_path(path);
    if parts.is_empty() {
        return Err("invalid file path");
    }
    let (dir_parts, file_name) = parts.split_at(parts.len() - 1);
    let file_name = file_name[0];

    let inode = mount.next_inode();
    let dir = mount.navigate_mut(dir_parts).ok_or("directory not found")?;
    if dir.find_file(file_name).is_some() {
        return Err("file already exists");
    }

    let tick = current_tick();
    let _inode = inode;
    dir.files.push(TmpfsFile {
        name: String::from(file_name),
        data: data.to_vec(),
        permissions: Permissions::default_file(),
        owner_uid: 0,
        owner_gid: 0,
        created_tick: tick,
        modified_tick: tick,
        accessed_tick: tick,
    });
    TOTAL_WRITES.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Read a file, returning its data.
pub fn read(path: &str) -> Result<Vec<u8>, &'static str> {
    let mut state = STATE.lock();
    let idx = find_mount(&state, path).ok_or("no tmpfs mount for path")?;
    let mount = &mut state.mounts[idx];

    let (_mp, parts) = split_path(path);
    if parts.is_empty() {
        return Err("invalid file path");
    }
    let (dir_parts, file_name) = parts.split_at(parts.len() - 1);
    let file_name = file_name[0];

    let dir = mount.navigate_mut(dir_parts).ok_or("directory not found")?;
    let fi = dir.find_file(file_name).ok_or("file not found")?;
    dir.files[fi].accessed_tick = current_tick();
    TOTAL_READS.fetch_add(1, Ordering::Relaxed);
    Ok(dir.files[fi].data.clone())
}

/// Write (overwrite) data to an existing file.
pub fn write(path: &str, data: &[u8]) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    let idx = find_mount(&state, path).ok_or("no tmpfs mount for path")?;
    let mount = &mut state.mounts[idx];

    let (_mp, parts) = split_path(path);
    if parts.is_empty() {
        return Err("invalid file path");
    }
    let (dir_parts, file_name) = parts.split_at(parts.len() - 1);
    let file_name = file_name[0];

    let max_sz = mount.max_size;
    let cur_sz = mount.current_size();
    let dir = mount.navigate_mut(dir_parts).ok_or("directory not found")?;
    let fi = dir.find_file(file_name).ok_or("file not found")?;

    let old_size = dir.files[fi].data.len();
    let size_delta = data.len() as isize - old_size as isize;
    let total = cur_sz as isize + size_delta;
    if total > max_sz as isize {
        return Err("tmpfs size limit exceeded");
    }

    dir.files[fi].data = data.to_vec();
    dir.files[fi].modified_tick = current_tick();
    TOTAL_WRITES.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Delete a file.
pub fn delete(path: &str) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    let idx = find_mount(&state, path).ok_or("no tmpfs mount for path")?;
    let mount = &mut state.mounts[idx];

    let (_mp, parts) = split_path(path);
    if parts.is_empty() {
        return Err("invalid file path");
    }
    let (dir_parts, file_name) = parts.split_at(parts.len() - 1);
    let file_name = file_name[0];

    let dir = mount.navigate_mut(dir_parts).ok_or("directory not found")?;
    let fi = dir.find_file(file_name).ok_or("file not found")?;
    dir.files.remove(fi);
    Ok(())
}

/// Get file metadata as a formatted string.
pub fn stat(path: &str) -> Result<String, &'static str> {
    let state = STATE.lock();
    let idx = find_mount(&state, path).ok_or("no tmpfs mount for path")?;
    let mount = &state.mounts[idx];

    let (_mp, parts) = split_path(path);
    if parts.is_empty() {
        return Err("invalid file path");
    }
    let (dir_parts, file_name) = parts.split_at(parts.len() - 1);
    let file_name = file_name[0];

    let dir = mount.navigate(dir_parts).ok_or("directory not found")?;
    let fi = dir.find_file(file_name).ok_or("file not found")?;
    let f = &dir.files[fi];
    Ok(format!("{}: {} bytes, perms={}, uid={}, gid={}, created={}, modified={}",
        f.name, f.size(), f.permissions.mode_string(),
        f.owner_uid, f.owner_gid, f.created_tick, f.modified_tick))
}

/// Truncate a file to the given length.
pub fn truncate(path: &str, len: usize) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    let idx = find_mount(&state, path).ok_or("no tmpfs mount for path")?;
    let mount = &mut state.mounts[idx];

    let (_mp, parts) = split_path(path);
    if parts.is_empty() {
        return Err("invalid file path");
    }
    let (dir_parts, file_name) = parts.split_at(parts.len() - 1);
    let file_name = file_name[0];

    let dir = mount.navigate_mut(dir_parts).ok_or("directory not found")?;
    let fi = dir.find_file(file_name).ok_or("file not found")?;
    dir.files[fi].data.truncate(len);
    dir.files[fi].modified_tick = current_tick();
    Ok(())
}

/// Create a subdirectory.
pub fn mkdir(path: &str) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    let idx = find_mount(&state, path).ok_or("no tmpfs mount for path")?;
    let mount = &mut state.mounts[idx];

    let (_mp, parts) = split_path(path);
    if parts.is_empty() {
        return Err("invalid directory path");
    }
    let (parent_parts, dir_name) = parts.split_at(parts.len() - 1);
    let dir_name = dir_name[0];

    let parent = mount.navigate_mut(parent_parts).ok_or("parent directory not found")?;
    if parent.find_subdir(dir_name).is_some() {
        return Err("directory already exists");
    }
    if parent.subdirs.len() >= MAX_DIRS {
        return Err("max directory count reached");
    }
    parent.subdirs.push(TmpfsDir::new(dir_name));
    Ok(())
}

/// Remove an empty subdirectory.
pub fn rmdir(path: &str) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    let idx = find_mount(&state, path).ok_or("no tmpfs mount for path")?;
    let mount = &mut state.mounts[idx];

    let (_mp, parts) = split_path(path);
    if parts.is_empty() {
        return Err("invalid directory path");
    }
    let (parent_parts, dir_name) = parts.split_at(parts.len() - 1);
    let dir_name = dir_name[0];

    let parent = mount.navigate_mut(parent_parts).ok_or("parent not found")?;
    let di = parent.find_subdir(dir_name).ok_or("directory not found")?;
    if !parent.subdirs[di].files.is_empty() || !parent.subdirs[di].subdirs.is_empty() {
        return Err("directory not empty");
    }
    parent.subdirs.remove(di);
    Ok(())
}

/// List files and subdirectories in a directory.
pub fn ls(path: &str) -> Result<Vec<String>, &'static str> {
    let state = STATE.lock();
    let idx = find_mount(&state, path).ok_or("no tmpfs mount for path")?;
    let mount = &state.mounts[idx];

    let (_mp, parts) = split_path(path);
    let dir = mount.navigate(&parts).ok_or("directory not found")?;

    let mut entries = Vec::new();
    for d in &dir.subdirs {
        entries.push(format!("{}/", d.name));
    }
    for f in &dir.files {
        entries.push(format!("{} ({} bytes)", f.name, f.size()));
    }
    Ok(entries)
}

/// Find files matching a name pattern (simple substring match).
pub fn find(mount_path: &str, pattern: &str) -> Vec<String> {
    let state = STATE.lock();
    let idx = match find_mount(&state, mount_path) {
        Some(i) => i,
        None => return Vec::new(),
    };
    let mount = &state.mounts[idx];
    let mut results = Vec::new();
    find_recursive(&mount.root, "", pattern, &mut results);
    results
}

fn find_recursive(dir: &TmpfsDir, prefix: &str, pattern: &str, results: &mut Vec<String>) {
    for f in &dir.files {
        if f.name.contains(pattern) {
            results.push(format!("{}/{}", prefix, f.name));
        }
    }
    for d in &dir.subdirs {
        let subpath = format!("{}/{}", prefix, d.name);
        find_recursive(d, &subpath, pattern, results);
    }
}

/// Remove files older than `max_age_ticks`.
pub fn cleanup_old(mount_path: &str, max_age_ticks: u64) -> usize {
    let mut state = STATE.lock();
    let idx = match find_mount(&state, mount_path) {
        Some(i) => i,
        None => return 0,
    };
    let now = current_tick();
    let mount = &mut state.mounts[idx];
    cleanup_dir(&mut mount.root, now, max_age_ticks)
}

fn cleanup_dir(dir: &mut TmpfsDir, now: u64, max_age: u64) -> usize {
    let mut removed = 0;
    dir.files.retain(|f| {
        if now.saturating_sub(f.modified_tick) > max_age {
            removed += 1;
            false
        } else {
            true
        }
    });
    for sub in &mut dir.subdirs {
        removed += cleanup_dir(sub, now, max_age);
    }
    removed
}

/// Summary information about all tmpfs mounts.
pub fn tmpfs_info() -> String {
    let state = STATE.lock();
    if state.mounts.is_empty() {
        return String::from("No tmpfs mounts");
    }
    let mut out = String::from("tmpfs mounts:\n");
    for m in &state.mounts {
        out.push_str(&format!("  {} — {}/{} bytes used, {} files\n",
            m.path, m.current_size(), m.max_size, m.file_count()));
    }
    out
}

/// Detailed statistics.
pub fn tmpfs_stats() -> String {
    let state = STATE.lock();
    let total_mounts = state.mounts.len();
    let total_files: usize = state.mounts.iter().map(|m| m.file_count()).sum();
    let total_bytes: usize = state.mounts.iter().map(|m| m.current_size()).sum();
    let total_capacity: usize = state.mounts.iter().map(|m| m.max_size).sum();
    let reads = TOTAL_READS.load(Ordering::Relaxed);
    let writes = TOTAL_WRITES.load(Ordering::Relaxed);
    format!(
        "tmpfs statistics:\n  mounts: {}\n  files: {}\n  used: {} bytes\n  capacity: {} bytes\n  reads: {}\n  writes: {}",
        total_mounts, total_files, total_bytes, total_capacity, reads, writes
    )
}

/// Initialize tmpfs — mount default /tmp with 64 KiB limit.
pub fn init() {
    let _ = mount("/tmp", DEFAULT_MAX_SIZE);
    crate::serial_println!("[tmpfs] mounted /tmp ({}K limit)", DEFAULT_MAX_SIZE / 1024);
}
