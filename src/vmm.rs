/// Virtual memory manager for MerlionOS.
/// Provides memory-mapped I/O, copy-on-write pages, page cache,
/// memory-mapped files, and address space management.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Page size (4 KiB).
const PAGE_SIZE: usize = 4096;

/// Huge page size (2 MiB).
const HUGE_PAGE_SIZE: usize = 2 * 1024 * 1024;

/// Maximum VMAs per address space.
const MAX_VMAS: usize = 256;

/// Maximum address spaces tracked.
const MAX_ADDRESS_SPACES: usize = 64;

/// Maximum page cache entries.
const MAX_PAGE_CACHE: usize = 512;

/// Maximum shared memory regions.
const MAX_SHARED_REGIONS: usize = 32;

/// Maximum huge page allocations tracked.
const MAX_HUGE_PAGES: usize = 64;

/// Maximum processes tracked by OOM killer.
const MAX_OOM_ENTRIES: usize = 64;

/// Maximum free fragments for compaction.
const MAX_FREE_FRAGMENTS: usize = 128;

// ---------------------------------------------------------------------------
// VMA types and permissions
// ---------------------------------------------------------------------------

/// Permission flags for a virtual memory area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmaPermissions {
    pub read: bool,
    pub write: bool,
    pub exec: bool,
}

impl VmaPermissions {
    pub const fn rwx() -> Self { Self { read: true, write: true, exec: true } }
    pub const fn rw() -> Self { Self { read: true, write: true, exec: false } }
    pub const fn ro() -> Self { Self { read: true, write: false, exec: false } }
    pub const fn rx() -> Self { Self { read: true, write: false, exec: true } }

    /// Format as "rwx" string.
    pub fn as_str(&self) -> &'static str {
        match (self.read, self.write, self.exec) {
            (true, true, true)   => "rwx",
            (true, true, false)  => "rw-",
            (true, false, true)  => "r-x",
            (true, false, false) => "r--",
            (false, true, false) => "-w-",
            (false, false, true) => "--x",
            (false, true, true)  => "-wx",
            (false, false, false)=> "---",
        }
    }
}

/// Type of a virtual memory area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmaType {
    /// Anonymous (zero-filled) mapping.
    Anonymous,
    /// File-backed mapping.
    File,
    /// Shared mapping visible to multiple processes.
    Shared,
    /// Stack region.
    Stack,
    /// Heap region (brk/sbrk).
    Heap,
}

// ---------------------------------------------------------------------------
// Virtual Memory Area
// ---------------------------------------------------------------------------

/// Describes a contiguous virtual address range in a process's address space.
#[derive(Debug, Clone)]
pub struct Vma {
    /// Start address (page-aligned).
    pub start: u64,
    /// End address (exclusive, page-aligned).
    pub end: u64,
    /// Access permissions.
    pub perm: VmaPermissions,
    /// Mapping type.
    pub vma_type: VmaType,
    /// Backing file path (empty for anonymous).
    pub backing_file: String,
    /// Offset into the backing file.
    pub file_offset: u64,
}

impl Vma {
    /// Size of this VMA in bytes.
    pub fn size(&self) -> u64 { self.end - self.start }

    /// Number of pages spanned by this VMA.
    pub fn page_count(&self) -> u64 { self.size() / PAGE_SIZE as u64 }

    /// Check whether an address falls within this VMA.
    pub fn contains(&self, addr: u64) -> bool { addr >= self.start && addr < self.end }
}

// ---------------------------------------------------------------------------
// Copy-on-Write page tracking
// ---------------------------------------------------------------------------

/// A single copy-on-write page entry.
#[derive(Debug, Clone)]
struct CowPage {
    /// Physical frame number backing this page.
    frame: u64,
    /// Number of address spaces sharing this frame.
    ref_count: usize,
    /// Virtual address in the original mapping.
    virt_addr: u64,
    /// Owning process id.
    owner_pid: usize,
}

/// Maximum number of CoW-tracked pages.
const MAX_COW_PAGES: usize = 256;

/// Global CoW page table.
static COW_TABLE: Mutex<CowTable> = Mutex::new(CowTable::new());

struct CowTable {
    pages: [Option<CowPage>; MAX_COW_PAGES],
    total_shared: u64,
    total_copied: u64,
}

impl CowTable {
    const fn new() -> Self {
        Self {
            pages: [const { None }; MAX_COW_PAGES],
            total_shared: 0,
            total_copied: 0,
        }
    }

    fn add(&mut self, frame: u64, virt_addr: u64, owner_pid: usize) -> bool {
        for slot in self.pages.iter_mut() {
            if slot.is_none() {
                *slot = Some(CowPage { frame, ref_count: 1, virt_addr, owner_pid });
                self.total_shared += 1;
                return true;
            }
        }
        false
    }

    fn share(&mut self, frame: u64) -> bool {
        for slot in self.pages.iter_mut() {
            if let Some(ref mut page) = slot {
                if page.frame == frame {
                    page.ref_count += 1;
                    self.total_shared += 1;
                    return true;
                }
            }
        }
        false
    }

    fn fault(&mut self, frame: u64) -> Option<u64> {
        for slot in self.pages.iter_mut() {
            if let Some(ref mut page) = slot {
                if page.frame == frame {
                    if page.ref_count <= 1 {
                        // Only one reference, just make it writable.
                        *slot = None;
                        return Some(frame);
                    }
                    page.ref_count -= 1;
                    self.total_copied += 1;
                    // Caller should allocate a new frame and copy.
                    return Some(0); // 0 signals "allocate new frame"
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Page cache
// ---------------------------------------------------------------------------

/// A cached page from a file.
#[derive(Debug, Clone)]
struct CachedPage {
    /// File path this page belongs to.
    file_path: String,
    /// Page-aligned offset into the file.
    offset: u64,
    /// Cached data (up to PAGE_SIZE bytes).
    data: Vec<u8>,
    /// Whether this page has been modified.
    dirty: bool,
    /// Access counter for LRU eviction.
    access_count: u64,
    /// Last access tick (monotonic).
    last_access: u64,
}

/// Global page cache.
static PAGE_CACHE: Mutex<PageCache> = Mutex::new(PageCache::new());

struct PageCache {
    entries: Vec<CachedPage>,
    hits: u64,
    misses: u64,
    evictions: u64,
    writebacks: u64,
}

impl PageCache {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            hits: 0,
            misses: 0,
            evictions: 0,
            writebacks: 0,
        }
    }

    fn lookup(&mut self, path: &str, offset: u64) -> Option<&[u8]> {
        let tick = TICK_COUNTER.load(Ordering::Relaxed);
        for entry in self.entries.iter_mut() {
            if entry.file_path == path && entry.offset == offset {
                entry.access_count += 1;
                entry.last_access = tick;
                self.hits += 1;
                return Some(&entry.data);
            }
        }
        self.misses += 1;
        None
    }

    fn insert(&mut self, path: &str, offset: u64, data: Vec<u8>) {
        // Evict LRU entry if at capacity.
        if self.entries.len() >= MAX_PAGE_CACHE {
            self.evict_lru();
        }
        let tick = TICK_COUNTER.load(Ordering::Relaxed);
        self.entries.push(CachedPage {
            file_path: String::from(path),
            offset,
            data,
            dirty: false,
            access_count: 1,
            last_access: tick,
        });
    }

    fn mark_dirty(&mut self, path: &str, offset: u64) -> bool {
        for entry in self.entries.iter_mut() {
            if entry.file_path == path && entry.offset == offset {
                entry.dirty = true;
                return true;
            }
        }
        false
    }

    fn evict_lru(&mut self) {
        if self.entries.is_empty() { return; }
        let mut min_access = u64::MAX;
        let mut min_idx = 0;
        for (i, entry) in self.entries.iter().enumerate() {
            if entry.last_access < min_access {
                min_access = entry.last_access;
                min_idx = i;
            }
        }
        let evicted = self.entries.remove(min_idx);
        if evicted.dirty {
            self.writebacks += 1;
        }
        self.evictions += 1;
    }

    fn writeback_all(&mut self) -> usize {
        let mut count = 0;
        for entry in self.entries.iter_mut() {
            if entry.dirty {
                entry.dirty = false;
                self.writebacks += 1;
                count += 1;
            }
        }
        count
    }

    fn invalidate(&mut self, path: &str) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| e.file_path != path);
        before - self.entries.len()
    }
}

// ---------------------------------------------------------------------------
// Address space
// ---------------------------------------------------------------------------

/// Per-process address space containing a list of VMAs.
#[derive(Clone)]
pub struct AddressSpace {
    /// Process ID owning this address space.
    pub pid: usize,
    /// Virtual memory areas.
    pub vmas: Vec<Vma>,
    /// Total mapped bytes.
    pub mapped_bytes: u64,
    /// Current program break (heap end).
    pub brk: u64,
}

impl AddressSpace {
    /// Create a new empty address space for a process.
    pub fn new(pid: usize) -> Self {
        Self { pid, vmas: Vec::new(), mapped_bytes: 0, brk: 0x0040_0000 }
    }

    /// Map a region into this address space.
    pub fn mmap(&mut self, start: u64, length: u64, perm: VmaPermissions,
                vma_type: VmaType, file: &str, file_offset: u64) -> Option<u64> {
        if self.vmas.len() >= MAX_VMAS { return None; }
        let aligned_start = start & !(PAGE_SIZE as u64 - 1);
        let aligned_len = (length + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
        let end = aligned_start + aligned_len;

        // Check for overlap with existing VMAs.
        for vma in &self.vmas {
            if aligned_start < vma.end && end > vma.start {
                return None; // overlapping
            }
        }

        self.vmas.push(Vma {
            start: aligned_start,
            end,
            perm,
            vma_type,
            backing_file: String::from(file),
            file_offset,
        });
        self.mapped_bytes += aligned_len;
        Some(aligned_start)
    }

    /// Unmap a region from this address space.
    pub fn munmap(&mut self, start: u64, length: u64) -> usize {
        let end = start + length;
        let before = self.vmas.len();
        self.vmas.retain(|vma| {
            !(vma.start >= start && vma.end <= end)
        });
        let removed = before - self.vmas.len();
        self.mapped_bytes = self.vmas.iter().map(|v| v.size()).sum();
        removed
    }

    /// Change permissions on a region.
    pub fn mprotect(&mut self, start: u64, length: u64, new_perm: VmaPermissions) -> bool {
        let end = start + length;
        let mut changed = false;
        for vma in self.vmas.iter_mut() {
            if vma.start >= start && vma.end <= end {
                vma.perm = new_perm;
                changed = true;
            }
        }
        changed
    }

    /// Find the VMA containing the given address.
    pub fn find_vma(&self, addr: u64) -> Option<&Vma> {
        self.vmas.iter().find(|v| v.contains(addr))
    }
}

/// Global address space table.
static ADDRESS_SPACES: Mutex<Vec<AddressSpace>> = Mutex::new(Vec::new());

// ---------------------------------------------------------------------------
// Shared memory regions
// ---------------------------------------------------------------------------

/// A named shared memory region accessible by multiple processes.
#[derive(Clone)]
struct SharedRegion {
    /// Unique identifier.
    id: usize,
    /// Human-readable name.
    name: String,
    /// Size in bytes (page-aligned).
    size: usize,
    /// Simulated backing buffer.
    data: Vec<u8>,
    /// PIDs that have attached this region.
    attached_pids: Vec<usize>,
}

/// Global shared memory table.
static SHARED_REGIONS: Mutex<SharedMemTable> = Mutex::new(SharedMemTable::new());

struct SharedMemTable {
    regions: Vec<SharedRegion>,
    next_id: usize,
}

impl SharedMemTable {
    const fn new() -> Self {
        Self { regions: Vec::new(), next_id: 1 }
    }

    fn create(&mut self, name: &str, size: usize) -> Option<usize> {
        if self.regions.len() >= MAX_SHARED_REGIONS { return None; }
        if size == 0 { return None; }
        let aligned = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let id = self.next_id;
        self.next_id += 1;
        self.regions.push(SharedRegion {
            id,
            name: String::from(name),
            size: aligned,
            data: vec![0u8; aligned],
            attached_pids: Vec::new(),
        });
        Some(id)
    }

    fn attach(&mut self, id: usize, pid: usize) -> bool {
        for region in self.regions.iter_mut() {
            if region.id == id {
                if !region.attached_pids.contains(&pid) {
                    region.attached_pids.push(pid);
                }
                return true;
            }
        }
        false
    }

    fn detach(&mut self, id: usize, pid: usize) -> bool {
        for region in self.regions.iter_mut() {
            if region.id == id {
                if let Some(pos) = region.attached_pids.iter().position(|&p| p == pid) {
                    region.attached_pids.remove(pos);
                    return true;
                }
                return false;
            }
        }
        false
    }

    fn destroy(&mut self, id: usize) -> bool {
        if let Some(pos) = self.regions.iter().position(|r| r.id == id) {
            self.regions.remove(pos);
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Huge pages
// ---------------------------------------------------------------------------

/// Tracks a single huge page allocation.
#[derive(Debug, Clone)]
struct HugePageAlloc {
    /// Virtual address of the huge page.
    virt_addr: u64,
    /// Owning process ID.
    pid: usize,
    /// Whether the page is actively in use.
    in_use: bool,
}

static HUGE_PAGES: Mutex<HugePageTable> = Mutex::new(HugePageTable::new());

struct HugePageTable {
    allocs: Vec<HugePageAlloc>,
    next_addr: u64,
}

impl HugePageTable {
    const fn new() -> Self {
        Self { allocs: Vec::new(), next_addr: 0x8000_0000_0000 }
    }

    fn allocate(&mut self, pid: usize) -> Option<u64> {
        if self.allocs.len() >= MAX_HUGE_PAGES { return None; }
        let addr = self.next_addr;
        self.next_addr += HUGE_PAGE_SIZE as u64;
        self.allocs.push(HugePageAlloc { virt_addr: addr, pid, in_use: true });
        Some(addr)
    }

    fn free(&mut self, addr: u64) -> bool {
        for alloc in self.allocs.iter_mut() {
            if alloc.virt_addr == addr && alloc.in_use {
                alloc.in_use = false;
                return true;
            }
        }
        false
    }

    fn count_active(&self) -> usize {
        self.allocs.iter().filter(|a| a.in_use).count()
    }
}

// ---------------------------------------------------------------------------
// OOM killer
// ---------------------------------------------------------------------------

/// Per-process memory usage for OOM scoring.
#[derive(Debug, Clone)]
struct OomEntry {
    pid: usize,
    /// RSS in pages.
    rss_pages: u64,
    /// OOM adjustment score (-1000 to 1000, higher = more likely killed).
    oom_adj: i32,
    /// Process priority (lower = more important = less likely killed).
    priority: u8,
}

static OOM_TABLE: Mutex<OomTable> = Mutex::new(OomTable::new());

struct OomTable {
    entries: Vec<OomEntry>,
    kills: u64,
}

impl OomTable {
    const fn new() -> Self {
        Self { entries: Vec::new(), kills: 0 }
    }

    fn update(&mut self, pid: usize, rss_pages: u64, oom_adj: i32, priority: u8) {
        for entry in self.entries.iter_mut() {
            if entry.pid == pid {
                entry.rss_pages = rss_pages;
                entry.oom_adj = oom_adj;
                entry.priority = priority;
                return;
            }
        }
        if self.entries.len() < MAX_OOM_ENTRIES {
            self.entries.push(OomEntry { pid, rss_pages, oom_adj, priority });
        }
    }

    fn remove(&mut self, pid: usize) {
        self.entries.retain(|e| e.pid != pid);
    }

    /// Score all processes and return the PID with the highest score (best
    /// candidate for killing).  Score = rss_pages + oom_adj * 10 + priority * 5.
    /// PID 0 is never selected.
    fn select_victim(&self) -> Option<usize> {
        let mut best_pid = None;
        let mut best_score: i64 = i64::MIN;
        for entry in &self.entries {
            if entry.pid == 0 { continue; }
            let score = entry.rss_pages as i64
                + entry.oom_adj as i64 * 10
                + entry.priority as i64 * 5;
            if score > best_score {
                best_score = score;
                best_pid = Some(entry.pid);
            }
        }
        best_pid
    }

    fn trigger_kill(&mut self) -> Option<usize> {
        if let Some(pid) = self.select_victim() {
            self.entries.retain(|e| e.pid != pid);
            self.kills += 1;
            Some(pid)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Memory compaction
// ---------------------------------------------------------------------------

/// Describes a free memory fragment.
#[derive(Debug, Clone, Copy)]
struct FreeFragment {
    start: u64,
    size: u64,
}

static COMPACTION: Mutex<CompactionState> = Mutex::new(CompactionState::new());

struct CompactionState {
    fragments: Vec<FreeFragment>,
    compactions_run: u64,
    pages_moved: u64,
}

impl CompactionState {
    const fn new() -> Self {
        Self {
            fragments: Vec::new(),
            compactions_run: 0,
            pages_moved: 0,
        }
    }

    fn add_fragment(&mut self, start: u64, size: u64) {
        if self.fragments.len() >= MAX_FREE_FRAGMENTS { return; }
        self.fragments.push(FreeFragment { start, size });
    }

    /// Merge adjacent fragments to reduce fragmentation.
    fn compact(&mut self) -> u64 {
        if self.fragments.len() < 2 { return 0; }
        // Sort by start address.
        self.fragments.sort_unstable_by_key(|f| f.start);
        let mut merged: Vec<FreeFragment> = Vec::new();
        let mut pages_moved: u64 = 0;

        for frag in &self.fragments {
            if let Some(last) = merged.last_mut() {
                if last.start + last.size == frag.start {
                    last.size += frag.size;
                    pages_moved += frag.size / PAGE_SIZE as u64;
                    continue;
                }
            }
            merged.push(*frag);
        }

        let old_count = self.fragments.len();
        self.fragments = merged;
        self.compactions_run += 1;
        self.pages_moved += pages_moved;
        (old_count - self.fragments.len()) as u64
    }

    fn largest_contiguous(&self) -> u64 {
        self.fragments.iter().map(|f| f.size).max().unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// NUMA awareness (simulated single-node)
// ---------------------------------------------------------------------------

/// Per-NUMA-node memory statistics.
#[derive(Debug, Clone)]
pub struct NumaNodeStats {
    pub node_id: usize,
    pub total_pages: u64,
    pub free_pages: u64,
    pub used_pages: u64,
}

static NUMA_STATS: Mutex<NumaNodeStats> = Mutex::new(NumaNodeStats {
    node_id: 0,
    total_pages: 262144, // 1 GiB worth of 4K pages
    free_pages: 262144,
    used_pages: 0,
});

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

static TICK_COUNTER: AtomicU64 = AtomicU64::new(0);
static TOTAL_MMAPS: AtomicU64 = AtomicU64::new(0);
static TOTAL_MUNMAPS: AtomicU64 = AtomicU64::new(0);
static TOTAL_FAULTS: AtomicU64 = AtomicU64::new(0);
static INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the virtual memory manager subsystem.
pub fn init() {
    // Ensure single initialisation.
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::serial_println!("[vmm] virtual memory manager initialized");
    crate::klog_println!("[vmm] subsystem ready");
}

/// Advance the internal tick counter (called from timer interrupt).
pub fn tick() {
    TICK_COUNTER.fetch_add(1, Ordering::Relaxed);
}

/// Create an address space for a new process.
pub fn create_address_space(pid: usize) -> bool {
    let mut spaces = ADDRESS_SPACES.lock();
    if spaces.len() >= MAX_ADDRESS_SPACES { return false; }
    if spaces.iter().any(|s| s.pid == pid) { return false; }
    spaces.push(AddressSpace::new(pid));
    crate::serial_println!("[vmm] address space created for pid {}", pid);
    true
}

/// Destroy the address space for a process.
pub fn destroy_address_space(pid: usize) -> bool {
    let mut spaces = ADDRESS_SPACES.lock();
    if let Some(pos) = spaces.iter().position(|s| s.pid == pid) {
        spaces.remove(pos);
        // Clean up OOM entries.
        OOM_TABLE.lock().remove(pid);
        crate::serial_println!("[vmm] address space destroyed for pid {}", pid);
        true
    } else {
        false
    }
}

/// Map a region into a process's address space.
pub fn mmap(pid: usize, start: u64, length: u64, perm: VmaPermissions,
            vma_type: VmaType, file: &str, file_offset: u64) -> Option<u64> {
    let mut spaces = ADDRESS_SPACES.lock();
    for space in spaces.iter_mut() {
        if space.pid == pid {
            let result = space.mmap(start, length, perm, vma_type, file, file_offset);
            if result.is_some() {
                TOTAL_MMAPS.fetch_add(1, Ordering::Relaxed);
                crate::serial_println!("[vmm] mmap pid={} start={:#x} len={}", pid, start, length);
            }
            return result;
        }
    }
    None
}

/// Unmap a region from a process's address space.
pub fn munmap(pid: usize, start: u64, length: u64) -> usize {
    let mut spaces = ADDRESS_SPACES.lock();
    for space in spaces.iter_mut() {
        if space.pid == pid {
            let removed = space.munmap(start, length);
            if removed > 0 {
                TOTAL_MUNMAPS.fetch_add(removed as u64, Ordering::Relaxed);
                crate::serial_println!("[vmm] munmap pid={} start={:#x} removed={}", pid, start, removed);
            }
            return removed;
        }
    }
    0
}

/// Change permissions on a memory region.
pub fn mprotect(pid: usize, start: u64, length: u64, perm: VmaPermissions) -> bool {
    let mut spaces = ADDRESS_SPACES.lock();
    for space in spaces.iter_mut() {
        if space.pid == pid {
            return space.mprotect(start, length, perm);
        }
    }
    false
}

/// Register a CoW page for the given frame.
pub fn cow_share(frame: u64, virt_addr: u64, owner_pid: usize) -> bool {
    COW_TABLE.lock().add(frame, virt_addr, owner_pid)
}

/// Add a shared reference to an existing CoW frame.
pub fn cow_add_ref(frame: u64) -> bool {
    COW_TABLE.lock().share(frame)
}

/// Handle a CoW fault.  Returns `Some(original_frame)` if the page was the
/// last reference, or `Some(0)` if the caller must allocate a new frame
/// and copy the data.  Returns `None` if the frame is not CoW-tracked.
pub fn cow_fault(frame: u64) -> Option<u64> {
    let result = COW_TABLE.lock().fault(frame);
    if result.is_some() {
        TOTAL_FAULTS.fetch_add(1, Ordering::Relaxed);
        crate::serial_println!("[vmm] CoW fault on frame {:#x}", frame);
    }
    result
}

/// Look up a page in the page cache.
pub fn page_cache_lookup(path: &str, offset: u64) -> Option<Vec<u8>> {
    let mut cache = PAGE_CACHE.lock();
    cache.lookup(path, offset).map(|d| d.to_vec())
}

/// Insert a page into the page cache.
pub fn page_cache_insert(path: &str, offset: u64, data: Vec<u8>) {
    let mut cache = PAGE_CACHE.lock();
    cache.insert(path, offset, data);
}

/// Mark a cached page as dirty.
pub fn page_cache_mark_dirty(path: &str, offset: u64) -> bool {
    PAGE_CACHE.lock().mark_dirty(path, offset)
}

/// Write back all dirty pages in the cache.
pub fn page_cache_writeback() -> usize {
    PAGE_CACHE.lock().writeback_all()
}

/// Invalidate all cached pages for a file.
pub fn page_cache_invalidate(path: &str) -> usize {
    PAGE_CACHE.lock().invalidate(path)
}

/// Return page cache statistics: (hits, misses, evictions, writebacks).
pub fn page_cache_stats() -> (u64, u64, u64, u64) {
    let cache = PAGE_CACHE.lock();
    (cache.hits, cache.misses, cache.evictions, cache.writebacks)
}

/// Create a named shared memory region.
pub fn shm_create(name: &str, size: usize) -> Option<usize> {
    let result = SHARED_REGIONS.lock().create(name, size);
    if let Some(id) = result {
        crate::serial_println!("[vmm] shared region '{}' created (id={}, size={})", name, id, size);
    }
    result
}

/// Attach a process to a shared memory region.
pub fn shm_attach(id: usize, pid: usize) -> bool {
    SHARED_REGIONS.lock().attach(id, pid)
}

/// Detach a process from a shared memory region.
pub fn shm_detach(id: usize, pid: usize) -> bool {
    SHARED_REGIONS.lock().detach(id, pid)
}

/// Destroy a shared memory region.
pub fn shm_destroy(id: usize) -> bool {
    SHARED_REGIONS.lock().destroy(id)
}

/// Allocate a huge page (2 MiB) for a process.
pub fn huge_page_alloc(pid: usize) -> Option<u64> {
    let result = HUGE_PAGES.lock().allocate(pid);
    if let Some(addr) = result {
        crate::serial_println!("[vmm] huge page allocated at {:#x} for pid {}", addr, pid);
    }
    result
}

/// Free a huge page.
pub fn huge_page_free(addr: u64) -> bool {
    HUGE_PAGES.lock().free(addr)
}

/// Number of active huge page allocations.
pub fn huge_page_count() -> usize {
    HUGE_PAGES.lock().count_active()
}

/// Update OOM killer information for a process.
pub fn oom_update(pid: usize, rss_pages: u64, oom_adj: i32, priority: u8) {
    OOM_TABLE.lock().update(pid, rss_pages, oom_adj, priority);
}

/// Trigger the OOM killer.  Returns the PID of the killed process, if any.
pub fn oom_kill() -> Option<usize> {
    let result = OOM_TABLE.lock().trigger_kill();
    if let Some(pid) = result {
        crate::serial_println!("[vmm] OOM killer selected pid {} for termination", pid);
        crate::klog_println!("[vmm] OOM killed pid {}", pid);
    }
    result
}

/// Return OOM killer information as a formatted string.
pub fn oom_info() -> String {
    let table = OOM_TABLE.lock();
    let mut out = format!("OOM killer: {} kills total\n", table.kills);
    out.push_str("PID   RSS(pages) OOM_ADJ  PRI\n");
    for entry in &table.entries {
        out.push_str(&format!("{:<5} {:<10} {:<8} {}\n",
            entry.pid, entry.rss_pages, entry.oom_adj, entry.priority));
    }
    out
}

/// Add a free memory fragment for compaction tracking.
pub fn add_free_fragment(start: u64, size: u64) {
    COMPACTION.lock().add_fragment(start, size);
}

/// Run memory compaction.  Returns the number of fragments merged.
pub fn compact_memory() -> u64 {
    let merged = COMPACTION.lock().compact();
    if merged > 0 {
        crate::serial_println!("[vmm] compaction merged {} fragments", merged);
    }
    merged
}

/// Return the largest contiguous free region after compaction.
pub fn largest_contiguous_free() -> u64 {
    COMPACTION.lock().largest_contiguous()
}

/// Return NUMA node statistics.
pub fn numa_stats() -> NumaNodeStats {
    NUMA_STATS.lock().clone()
}

/// Update NUMA node page counts (simulated).
pub fn numa_update(free_pages: u64, used_pages: u64) {
    let mut stats = NUMA_STATS.lock();
    stats.free_pages = free_pages;
    stats.used_pages = used_pages;
}

/// Return a comprehensive VMM status string.
pub fn vmm_info() -> String {
    let spaces = ADDRESS_SPACES.lock();
    let cow = COW_TABLE.lock();
    let cache = PAGE_CACHE.lock();
    let shared = SHARED_REGIONS.lock();
    let huge = HUGE_PAGES.lock();
    let oom = OOM_TABLE.lock();
    let compact = COMPACTION.lock();
    let numa = NUMA_STATS.lock();

    let mut out = String::from("=== VMM Status ===\n");
    out.push_str(&format!("Address spaces: {}\n", spaces.len()));
    out.push_str(&format!("Total mmap calls: {}\n", TOTAL_MMAPS.load(Ordering::Relaxed)));
    out.push_str(&format!("Total munmap calls: {}\n", TOTAL_MUNMAPS.load(Ordering::Relaxed)));
    out.push_str(&format!("Total CoW faults: {}\n", TOTAL_FAULTS.load(Ordering::Relaxed)));
    out.push_str(&format!("CoW shared: {}, copied: {}\n", cow.total_shared, cow.total_copied));
    out.push_str(&format!("Page cache: {} entries, {} hits, {} misses\n",
        cache.entries.len(), cache.hits, cache.misses));
    out.push_str(&format!("Page cache evictions: {}, writebacks: {}\n",
        cache.evictions, cache.writebacks));
    out.push_str(&format!("Shared regions: {}\n", shared.regions.len()));
    out.push_str(&format!("Huge pages active: {}\n", huge.count_active()));
    out.push_str(&format!("OOM kills: {}, tracked processes: {}\n",
        oom.kills, oom.entries.len()));
    out.push_str(&format!("Compaction runs: {}, pages moved: {}\n",
        compact.compactions_run, compact.pages_moved));
    out.push_str(&format!("Free fragments: {}, largest: {} bytes\n",
        compact.fragments.len(), compact.largest_contiguous()));
    out.push_str(&format!("NUMA node {}: total={}, free={}, used={}\n",
        numa.node_id, numa.total_pages, numa.free_pages, numa.used_pages));
    out
}
