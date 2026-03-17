/// Graphical file manager for MerlionOS.
/// Provides directory browsing, file operations, preview,
/// and integration with the VFS.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// View modes
// ---------------------------------------------------------------------------

/// View mode for the file manager.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewMode {
    List,
    Icon,
    Tree,
}

impl ViewMode {
    fn as_str(&self) -> &'static str {
        match self {
            ViewMode::List => "List",
            ViewMode::Icon => "Icon",
            ViewMode::Tree => "Tree",
        }
    }
}

// ---------------------------------------------------------------------------
// Sort settings
// ---------------------------------------------------------------------------

/// Sort field for file listings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortField {
    Name,
    Size,
    FileType,
    Date,
}

impl SortField {
    fn as_str(&self) -> &'static str {
        match self {
            SortField::Name => "Name",
            SortField::Size => "Size",
            SortField::FileType => "Type",
            SortField::Date => "Date",
        }
    }
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortDir {
    Ascending,
    Descending,
}

impl SortDir {
    fn as_str(&self) -> &'static str {
        match self {
            SortDir::Ascending => "Asc",
            SortDir::Descending => "Desc",
        }
    }
}

// ---------------------------------------------------------------------------
// File type detection
// ---------------------------------------------------------------------------

/// Detected file type category.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileCategory {
    Text,
    Source,
    Config,
    Log,
    Executable,
    DiskImage,
    Audio,
    Image,
    Directory,
    Unknown,
}

impl FileCategory {
    fn as_str(&self) -> &'static str {
        match self {
            FileCategory::Text => "Text",
            FileCategory::Source => "Source",
            FileCategory::Config => "Config",
            FileCategory::Log => "Log",
            FileCategory::Executable => "Executable",
            FileCategory::DiskImage => "Disk Image",
            FileCategory::Audio => "Audio",
            FileCategory::Image => "Image",
            FileCategory::Directory => "Directory",
            FileCategory::Unknown => "Unknown",
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            FileCategory::Text => "[TXT]",
            FileCategory::Source => "[SRC]",
            FileCategory::Config => "[CFG]",
            FileCategory::Log => "[LOG]",
            FileCategory::Executable => "[EXE]",
            FileCategory::DiskImage => "[IMG]",
            FileCategory::Audio => "[WAV]",
            FileCategory::Image => "[BMP]",
            FileCategory::Directory => "[DIR]",
            FileCategory::Unknown => "[   ]",
        }
    }
}

/// Detect file category from extension.
pub fn detect_category(name: &str) -> FileCategory {
    if let Some(dot) = name.rfind('.') {
        let ext = &name[dot + 1..];
        match ext {
            "txt" => FileCategory::Text,
            "rs" | "sh" => FileCategory::Source,
            "conf" | "toml" | "cfg" => FileCategory::Config,
            "log" => FileCategory::Log,
            "elf" | "bin" => FileCategory::Executable,
            "img" | "iso" => FileCategory::DiskImage,
            "wav" | "mp3" => FileCategory::Audio,
            "bmp" | "png" => FileCategory::Image,
            _ => FileCategory::Unknown,
        }
    } else {
        FileCategory::Unknown
    }
}

// ---------------------------------------------------------------------------
// File entry for display
// ---------------------------------------------------------------------------

/// Information about a single file entry.
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub category: FileCategory,
    pub permissions: u16,
    pub owner: String,
    pub modified_tick: u64,
}

/// Format a byte size as human-readable string (KB, MB, GB).
pub fn human_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        let gb = bytes / (1024 * 1024 * 1024);
        let frac = (bytes % (1024 * 1024 * 1024)) / (1024 * 1024 * 10);
        format!("{}.{} GB", gb, frac)
    } else if bytes >= 1024 * 1024 {
        let mb = bytes / (1024 * 1024);
        let frac = (bytes % (1024 * 1024)) / (1024 * 10);
        format!("{}.{} MB", mb, frac)
    } else if bytes >= 1024 {
        let kb = bytes / 1024;
        format!("{} KB", kb)
    } else {
        format!("{} B", bytes)
    }
}

/// Format permissions as rwx string.
fn perm_str(perm: u16) -> String {
    let mut s = String::with_capacity(9);
    let bits = [(0o400, 'r'), (0o200, 'w'), (0o100, 'x'),
                (0o040, 'r'), (0o020, 'w'), (0o010, 'x'),
                (0o004, 'r'), (0o002, 'w'), (0o001, 'x')];
    for (mask, ch) in bits {
        if perm & mask != 0 { s.push(ch); } else { s.push('-'); }
    }
    s
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

/// Clipboard operation kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClipOp {
    Copy,
    Cut,
}

/// Clipboard entry — a file reference for paste.
pub struct ClipEntry {
    pub path: String,
    pub op: ClipOp,
}

// ---------------------------------------------------------------------------
// Bookmarks
// ---------------------------------------------------------------------------

/// A bookmark entry for the sidebar.
pub struct Bookmark {
    pub label: String,
    pub path: String,
}

fn default_bookmarks() -> Vec<Bookmark> {
    let mut v = Vec::new();
    let entries: [(&str, &str); 5] = [
        ("Home", "/"),
        ("Root", "/"),
        ("Tmp", "/tmp"),
        ("Proc", "/proc"),
        ("Dev", "/dev"),
    ];
    for (label, path) in entries {
        v.push(Bookmark {
            label: String::from(label),
            path: String::from(path),
        });
    }
    v
}

// ---------------------------------------------------------------------------
// Drag & drop tracking
// ---------------------------------------------------------------------------

/// Tracks an in-progress drag operation.
pub struct DragState {
    pub source: String,
    pub dest: String,
    pub active: bool,
}

// ---------------------------------------------------------------------------
// Navigation history
// ---------------------------------------------------------------------------

struct NavHistory {
    back: Vec<String>,
    forward: Vec<String>,
    current: String,
}

impl NavHistory {
    const fn new() -> Self {
        Self {
            back: Vec::new(),
            forward: Vec::new(),
            current: String::new(),
        }
    }

    fn go_to(&mut self, path: String) {
        if !self.current.is_empty() {
            self.back.push(self.current.clone());
        }
        self.forward.clear();
        self.current = path;
    }

    fn go_back(&mut self) -> bool {
        if let Some(prev) = self.back.pop() {
            self.forward.push(self.current.clone());
            self.current = prev;
            true
        } else {
            false
        }
    }

    fn go_forward(&mut self) -> bool {
        if let Some(next) = self.forward.pop() {
            self.back.push(self.current.clone());
            self.current = next;
            true
        } else {
            false
        }
    }

    fn go_up(&mut self) -> bool {
        if self.current.len() <= 1 {
            return false;
        }
        let trimmed = if self.current.ends_with('/') && self.current.len() > 1 {
            &self.current[..self.current.len() - 1]
        } else {
            &self.current
        };
        if let Some(slash) = trimmed.rfind('/') {
            let parent = if slash == 0 {
                String::from("/")
            } else {
                String::from(&trimmed[..slash])
            };
            self.go_to(parent);
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static DIR_OPENS: AtomicU32 = AtomicU32::new(0);
static FILE_OPS: AtomicU32 = AtomicU32::new(0);
static PREVIEWS: AtomicU32 = AtomicU32::new(0);
static SEARCHES: AtomicU32 = AtomicU32::new(0);

struct FileManagerState {
    view: ViewMode,
    sort_field: SortField,
    sort_dir: SortDir,
    filter: String,
    nav: NavHistory,
    bookmarks: Vec<Bookmark>,
    clipboard: Vec<ClipEntry>,
    drag: DragState,
    entries: Vec<FileEntry>,
}

impl FileManagerState {
    const fn new() -> Self {
        Self {
            view: ViewMode::List,
            sort_field: SortField::Name,
            sort_dir: SortDir::Ascending,
            filter: String::new(),
            nav: NavHistory::new(),
            bookmarks: Vec::new(),
            clipboard: Vec::new(),
            drag: DragState { source: String::new(), dest: String::new(), active: false },
            entries: Vec::new(),
        }
    }
}

static STATE: Mutex<FileManagerState> = Mutex::new(FileManagerState::new());

// ---------------------------------------------------------------------------
// VFS integration — read directory entries
// ---------------------------------------------------------------------------

fn load_directory(path: &str) -> Vec<FileEntry> {
    let mut entries = Vec::new();
    if let Ok(children) = crate::vfs::ls(path) {
        for (name, kind) in children {
            let is_dir = kind == 'd';
            let full_path = if path == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", path, name)
            };
            let size = if is_dir {
                0u64
            } else {
                crate::vfs::cat(&full_path)
                    .map(|c| c.len() as u64)
                    .unwrap_or(0)
            };
            let category = if is_dir { FileCategory::Directory } else { detect_category(&name) };
            entries.push(FileEntry {
                name,
                size,
                is_dir,
                category,
                permissions: 0o755,
                owner: String::from("root"),
                modified_tick: 0,
            });
        }
    }
    entries
}

/// Sort entries according to current settings.
fn sort_entries(entries: &mut Vec<FileEntry>, field: SortField, dir: SortDir) {
    entries.sort_by(|a, b| {
        // Directories always first
        if a.is_dir && !b.is_dir { return core::cmp::Ordering::Less; }
        if !a.is_dir && b.is_dir { return core::cmp::Ordering::Greater; }

        let cmp = match field {
            SortField::Name => a.name.cmp(&b.name),
            SortField::Size => a.size.cmp(&b.size),
            SortField::FileType => {
                let ca = a.category.as_str();
                let cb = b.category.as_str();
                ca.cmp(cb)
            }
            SortField::Date => a.modified_tick.cmp(&b.modified_tick),
        };
        match dir {
            SortDir::Ascending => cmp,
            SortDir::Descending => cmp.reverse(),
        }
    });
}

/// Filter entries by name pattern (simple substring match).
fn filter_entries(entries: &[FileEntry], pattern: &str) -> Vec<usize> {
    let mut indices = Vec::new();
    if pattern.is_empty() {
        for i in 0..entries.len() {
            indices.push(i);
        }
    } else {
        let pat_lower = pattern.to_ascii_lowercase();
        for (i, e) in entries.iter().enumerate() {
            let name_lower = e.name.to_ascii_lowercase();
            if name_lower.contains(pat_lower.as_str()) {
                indices.push(i);
            }
        }
    }
    indices
}

// ---------------------------------------------------------------------------
// File preview
// ---------------------------------------------------------------------------

/// Generate a text preview (first 20 lines) for a text file.
fn text_preview(path: &str) -> String {
    let content = match crate::vfs::cat(path) {
        Ok(c) => c,
        Err(_) => return String::from("(cannot read file)\n"),
    };
    let mut out = String::new();
    let mut line_count = 0u32;
    for line in content.lines() {
        if line_count >= 20 { break; }
        out.push_str(line);
        out.push('\n');
        line_count += 1;
    }
    if line_count == 0 {
        out.push_str("(empty file)\n");
    }
    PREVIEWS.fetch_add(1, Ordering::Relaxed);
    out
}

/// Generate a hex dump preview for a binary file.
fn hex_preview(path: &str) -> String {
    let content = match crate::vfs::cat(path) {
        Ok(c) => c,
        Err(_) => return String::from("(cannot read file)\n"),
    };
    let bytes = content.as_bytes();
    let mut out = String::new();
    let limit = if bytes.len() > 256 { 256 } else { bytes.len() };
    let mut offset = 0usize;
    while offset < limit {
        out.push_str(&format!("{:08x}  ", offset));
        let row_end = if offset + 16 > limit { limit } else { offset + 16 };
        for i in offset..offset + 16 {
            if i < row_end {
                out.push_str(&format!("{:02x} ", bytes[i]));
            } else {
                out.push_str("   ");
            }
        }
        out.push_str(" |");
        for i in offset..row_end {
            let ch = bytes[i];
            if ch >= 0x20 && ch < 0x7f {
                out.push(ch as char);
            } else {
                out.push('.');
            }
        }
        out.push_str("|\n");
        offset += 16;
    }
    PREVIEWS.fetch_add(1, Ordering::Relaxed);
    out
}

/// Preview a file (text or hex depending on category).
pub fn preview_file(path: &str) -> String {
    let cat = detect_category(path);
    match cat {
        FileCategory::Text | FileCategory::Source | FileCategory::Config | FileCategory::Log => {
            text_preview(path)
        }
        _ => hex_preview(path),
    }
}

// ---------------------------------------------------------------------------
// File operations
// ---------------------------------------------------------------------------

/// Copy a file within VFS.
pub fn copy_file(src: &str, dest: &str) -> bool {
    let content = match crate::vfs::cat(src) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let _ = crate::vfs::write(dest, &content);
    FILE_OPS.fetch_add(1, Ordering::Relaxed);
    true
}

/// Move a file (copy + delete).
pub fn move_file(src: &str, dest: &str) -> bool {
    if copy_file(src, dest) {
        let _ = crate::vfs::rm(src);
        FILE_OPS.fetch_add(1, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// Rename a file within VFS.
pub fn rename_file(path: &str, new_name: &str) -> bool {
    // Compute the parent directory
    let parent = if let Some(slash) = path.rfind('/') {
        if slash == 0 { "/" } else { &path[..slash] }
    } else {
        "/"
    };
    let new_path = if parent == "/" {
        format!("/{}", new_name)
    } else {
        format!("{}/{}", parent, new_name)
    };
    if move_file(path, &new_path) {
        FILE_OPS.fetch_add(1, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// Delete a file with confirmation message.
pub fn delete_file(path: &str, confirmed: bool) -> bool {
    if !confirmed {
        return false;
    }
    let _ = crate::vfs::rm(path);
    FILE_OPS.fetch_add(1, Ordering::Relaxed);
    true
}

/// Create a new empty file.
pub fn create_file(path: &str) -> bool {
    let _ = crate::vfs::write(path, "");
    FILE_OPS.fetch_add(1, Ordering::Relaxed);
    true
}

/// Create a new directory.
pub fn create_folder(path: &str) -> bool {
    let _ = crate::vfs::mkdir(path);
    FILE_OPS.fetch_add(1, Ordering::Relaxed);
    true
}

// ---------------------------------------------------------------------------
// Clipboard operations
// ---------------------------------------------------------------------------

/// Copy file reference to clipboard.
pub fn clip_copy(path: &str) {
    let mut st = STATE.lock();
    st.clipboard.push(ClipEntry { path: String::from(path), op: ClipOp::Copy });
}

/// Cut file reference to clipboard.
pub fn clip_cut(path: &str) {
    let mut st = STATE.lock();
    st.clipboard.push(ClipEntry { path: String::from(path), op: ClipOp::Cut });
}

/// Paste clipboard entries into a destination directory.
pub fn clip_paste(dest_dir: &str) -> u32 {
    let mut st = STATE.lock();
    let mut count = 0u32;
    let entries: Vec<ClipEntry> = st.clipboard.drain(..).collect();
    for entry in &entries {
        let name = if let Some(slash) = entry.path.rfind('/') {
            &entry.path[slash + 1..]
        } else {
            &entry.path
        };
        let dest = if dest_dir == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", dest_dir, name)
        };
        match entry.op {
            ClipOp::Copy => { copy_file(&entry.path, &dest); }
            ClipOp::Cut => { move_file(&entry.path, &dest); }
        }
        count += 1;
    }
    count
}

// ---------------------------------------------------------------------------
// Drag & drop
// ---------------------------------------------------------------------------

/// Begin a drag operation from source path.
pub fn drag_begin(source: &str) {
    let mut st = STATE.lock();
    st.drag.source = String::from(source);
    st.drag.dest.clear();
    st.drag.active = true;
}

/// Complete a drag operation to dest directory.
pub fn drag_drop(dest: &str) -> bool {
    let mut st = STATE.lock();
    if !st.drag.active {
        return false;
    }
    st.drag.dest = String::from(dest);
    let src = st.drag.source.clone();
    st.drag.active = false;
    drop(st);
    move_file(&src, dest)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the file manager.
pub fn init() {
    let mut st = STATE.lock();
    st.bookmarks = default_bookmarks();
    st.nav.go_to(String::from("/"));
    st.entries = load_directory("/");
}

/// Open and display a directory.
pub fn open_directory(path: &str) {
    DIR_OPENS.fetch_add(1, Ordering::Relaxed);
    let mut st = STATE.lock();
    st.nav.go_to(String::from(path));
    st.entries = load_directory(path);
    let sf = st.sort_field;
    let sd = st.sort_dir;
    sort_entries(&mut st.entries, sf, sd);

    let visible = filter_entries(&st.entries, &st.filter);
    let current = st.nav.current.clone();
    let view = st.view;
    let sort_f = st.sort_field;
    let sort_d = st.sort_dir;
    let bookmark_count = st.bookmarks.len();

    crate::println!("File Manager - {}", current);
    crate::println!("View: {}  Sort: {} {}  Bookmarks: {}",
        view.as_str(), sort_f.as_str(), sort_d.as_str(), bookmark_count);
    crate::println!("───────────────────────────────────────────────────────");

    match view {
        ViewMode::List => {
            crate::println!("{:<24} {:>10} {:<10} {:<9} {:<8}",
                "Name", "Size", "Type", "Perms", "Owner");
            crate::println!("────────────────────── ────────── ────────── ───────── ────────");
            for &idx in &visible {
                let e = &st.entries[idx];
                let size_s = if e.is_dir {
                    String::from("-")
                } else {
                    human_size(e.size)
                };
                crate::println!("{:<24} {:>10} {:<10} {} {:<8}",
                    e.name, size_s, e.category.as_str(),
                    perm_str(e.permissions), e.owner);
            }
        }
        ViewMode::Icon => {
            let mut col = 0u32;
            for &idx in &visible {
                let e = &st.entries[idx];
                crate::print!("{} {:<14} ", e.category.icon(), e.name);
                col += 1;
                if col >= 4 {
                    crate::println!();
                    col = 0;
                }
            }
            if col > 0 { crate::println!(); }
        }
        ViewMode::Tree => {
            for &idx in &visible {
                let e = &st.entries[idx];
                let prefix = if e.is_dir { "+-" } else { "|-" };
                crate::println!("  {} {} ({})", prefix, e.name, e.category.as_str());
            }
        }
    }

    crate::println!("\n{} item(s)", visible.len());
}

/// Return file manager summary string.
pub fn file_manager_info() -> String {
    let st = STATE.lock();
    format!(
        "File Manager: dir={} view={} sort={}/{} entries={} bookmarks={} clipboard={}",
        st.nav.current,
        st.view.as_str(),
        st.sort_field.as_str(),
        st.sort_dir.as_str(),
        st.entries.len(),
        st.bookmarks.len(),
        st.clipboard.len(),
    )
}

/// Return file manager statistics string.
pub fn file_manager_stats() -> String {
    format!(
        "File Manager Stats: dirs_opened={} file_ops={} previews={} searches={}",
        DIR_OPENS.load(Ordering::Relaxed),
        FILE_OPS.load(Ordering::Relaxed),
        PREVIEWS.load(Ordering::Relaxed),
        SEARCHES.load(Ordering::Relaxed),
    )
}

/// Set the view mode.
pub fn set_view(mode: ViewMode) {
    STATE.lock().view = mode;
}

/// Set sort field and direction.
pub fn set_sort(field: SortField, dir: SortDir) {
    let mut st = STATE.lock();
    st.sort_field = field;
    st.sort_dir = dir;
}

/// Set the filename filter.
pub fn set_filter(pattern: &str) {
    SEARCHES.fetch_add(1, Ordering::Relaxed);
    STATE.lock().filter = String::from(pattern);
}

/// Navigate back in history.
pub fn go_back() {
    let went = STATE.lock().nav.go_back();
    if went {
        let path = STATE.lock().nav.current.clone();
        open_directory(&path);
    }
}

/// Navigate forward in history.
pub fn go_forward() {
    let went = STATE.lock().nav.go_forward();
    if went {
        let path = STATE.lock().nav.current.clone();
        open_directory(&path);
    }
}

/// Navigate up to parent directory.
pub fn go_up() {
    let went = STATE.lock().nav.go_up();
    if went {
        let path = STATE.lock().nav.current.clone();
        open_directory(&path);
    }
}
