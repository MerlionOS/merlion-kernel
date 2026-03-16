/// Memory-mapped file I/O for MerlionOS.
/// Maps VFS file contents into virtual address space with lazy page fault
/// loading, protection flags, and writeback via msync.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// Allow reads from the mapped region.
pub const PROT_READ: u8 = 0b001;
/// Allow writes to the mapped region.
pub const PROT_WRITE: u8 = 0b010;
/// Allow execution from the mapped region.
pub const PROT_EXEC: u8 = 0b100;

const MMAP_REGION_BASE: u64 = 0x7777_0000_0000;
static MMAP_NEXT: AtomicU64 = AtomicU64::new(MMAP_REGION_BASE);
const PAGE_SIZE: u64 = 4096;

static PAGES_MAPPED: AtomicU64 = AtomicU64::new(0);
static PAGES_FAULTED: AtomicU64 = AtomicU64::new(0);
static SYNCS_PERFORMED: AtomicU64 = AtomicU64::new(0);

/// Describes a single memory-mapped region backed by a VFS file.
#[derive(Clone)]
pub struct MmapRegion {
    /// Virtual address where this region starts.
    pub virt_addr: u64,
    /// Length of the mapped region in bytes (page-aligned).
    pub length: u64,
    /// Path of the backing file in the VFS.
    pub file_path: String,
    /// Byte offset into the file where the mapping begins.
    pub offset: u64,
    /// Protection flags (PROT_READ / PROT_WRITE / PROT_EXEC).
    pub prot: u8,
    /// Process ID that owns this mapping.
    pub pid: usize,
    /// True once pages have been physically populated via fault.
    pub populated: bool,
}

/// Tracks all active memory-mapped regions across processes.
pub struct MmapTable {
    regions: Vec<MmapRegion>,
}

static MMAP_TABLE: Mutex<Option<MmapTable>> = Mutex::new(None);

impl MmapTable {
    fn new() -> Self { Self { regions: Vec::new() } }

    fn insert(&mut self, region: MmapRegion) { self.regions.push(region); }

    /// Remove all regions overlapping [addr, addr+length) for pid.
    fn remove(&mut self, pid: usize, addr: u64, length: u64) -> usize {
        let before = self.regions.len();
        self.regions.retain(|r| {
            !(r.pid == pid && r.virt_addr < addr + length && addr < r.virt_addr + r.length)
        });
        before - self.regions.len()
    }

    fn find(&self, pid: usize, addr: u64) -> Option<&MmapRegion> {
        self.regions.iter()
            .find(|r| r.pid == pid && addr >= r.virt_addr && addr < r.virt_addr + r.length)
    }

    fn find_mut(&mut self, pid: usize, addr: u64) -> Option<&mut MmapRegion> {
        self.regions.iter_mut()
            .find(|r| r.pid == pid && addr >= r.virt_addr && addr < r.virt_addr + r.length)
    }
}

/// Initialize the global mmap table. Call once during kernel boot.
pub fn init() {
    *MMAP_TABLE.lock() = Some(MmapTable::new());
    crate::serial_println!("[mmap] subsystem initialized");
    crate::klog_println!("[mmap] memory-mapped I/O ready");
}

/// Map a VFS file into virtual address space with lazy page loading.
///
/// Pages are not allocated until first access triggers a page fault
/// handled by [`handle_page_fault`]. Returns a pointer to the mapped
/// region, or null on error.
pub fn mmap(path: &str, offset: u64, length: u64, prot: u8) -> *mut u8 {
    if length == 0 {
        crate::serial_println!("[mmap] error: zero-length mapping");
        return core::ptr::null_mut();
    }
    if crate::vfs::cat(path).is_err() {
        crate::serial_println!("[mmap] error: file not found: {}", path);
        return core::ptr::null_mut();
    }

    let aligned_len = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let vaddr = MMAP_NEXT.fetch_add(aligned_len, Ordering::SeqCst);
    let pid = crate::task::current_pid();

    let region = MmapRegion {
        virt_addr: vaddr, length: aligned_len, file_path: String::from(path),
        offset, prot, pid, populated: false,
    };

    if let Some(ref mut tbl) = *MMAP_TABLE.lock() { tbl.insert(region); }
    PAGES_MAPPED.fetch_add(aligned_len / PAGE_SIZE, Ordering::SeqCst);

    crate::serial_println!("[mmap] mapped {} ({} bytes) at {:#x} prot={:#04b}", path, aligned_len, vaddr, prot);
    crate::klog_println!("[mmap] pid {} mapped {} at {:#x}", pid, path, vaddr);
    vaddr as *mut u8
}

/// Unmap a previously mapped region. Removes all overlapping regions
/// owned by the current process. Returns an error if nothing matched.
pub fn munmap(addr: u64, length: u64) -> Result<(), &'static str> {
    let pid = crate::task::current_pid();
    let mut table = MMAP_TABLE.lock();
    let tbl = table.as_mut().ok_or("mmap not initialized")?;
    let removed = tbl.remove(pid, addr, length);
    if removed == 0 { return Err("no mapping found at that address"); }
    crate::serial_println!("[mmap] munmap pid {} addr {:#x} — removed {}", pid, addr, removed);
    crate::klog_println!("[mmap] pid {} unmapped {:#x}", pid, addr);
    Ok(())
}

/// Flush changes in a mapped region back to the underlying VFS file.
///
/// Reads virtual memory contents and writes them to the backing file.
/// Only works for regions with PROT_WRITE that have been faulted in.
pub fn msync(addr: u64, length: u64) -> Result<(), &'static str> {
    let pid = crate::task::current_pid();
    let table = MMAP_TABLE.lock();
    let tbl = table.as_ref().ok_or("mmap not initialized")?;
    let region = tbl.find(pid, addr).ok_or("no mapping at that address")?;

    if region.prot & PROT_WRITE == 0 { return Err("region is not writable"); }
    if !region.populated { return Err("region has not been faulted in yet"); }

    let sync_len = core::cmp::min(length, region.length) as usize;
    let mut buf = Vec::with_capacity(sync_len);
    for i in 0..sync_len {
        let byte = unsafe { core::ptr::read_volatile((region.virt_addr + i as u64) as *const u8) };
        buf.push(byte);
    }
    crate::vfs::write(&region.file_path, &buf);

    SYNCS_PERFORMED.fetch_add(1, Ordering::SeqCst);
    crate::serial_println!("[mmap] msync {} bytes {:#x} → {}", sync_len, addr, region.file_path);
    Ok(())
}

/// Handle a page fault at `fault_addr` by checking mmap regions.
///
/// If the address falls inside an mmap region, allocates a physical page,
/// copies the corresponding file data into it, and maps it. Returns `true`
/// if the fault was handled.
pub fn handle_page_fault(fault_addr: u64) -> bool {
    let pid = crate::task::current_pid();
    let mut table = MMAP_TABLE.lock();
    let tbl = match table.as_mut() { Some(t) => t, None => return false };
    let region = match tbl.find_mut(pid, fault_addr) { Some(r) => r, None => return false };

    let page_addr = fault_addr & !(PAGE_SIZE - 1);
    let page_offset = page_addr - region.virt_addr;

    use x86_64::structures::paging::{Page, PageTableFlags, Size4KiB};
    use x86_64::VirtAddr;

    let mut flags = PageTableFlags::PRESENT;
    if region.prot & PROT_WRITE != 0 { flags |= PageTableFlags::WRITABLE; }

    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(page_addr));
    if crate::memory::map_page_global(page, flags).is_err() {
        crate::serial_println!("[mmap] FAILED to map page at {:#x}", page_addr);
        return false;
    }

    // Load file content into the freshly mapped page.
    if let Some(data) = crate::vfs::cat(&region.file_path) {
        let file_off = (region.offset + page_offset) as usize;
        let page_buf = unsafe {
            core::slice::from_raw_parts_mut(page_addr as *mut u8, PAGE_SIZE as usize)
        };
        for b in page_buf.iter_mut() { *b = 0; }
        if file_off < data.len() {
            let n = core::cmp::min(data.len() - file_off, PAGE_SIZE as usize);
            page_buf[..n].copy_from_slice(&data[file_off..file_off + n]);
        }
    }

    region.populated = true;
    PAGES_FAULTED.fetch_add(1, Ordering::SeqCst);
    crate::serial_println!("[mmap] fault-in {:#x} for {} (pid {})", page_addr, region.file_path, pid);
    true
}

/// Return a snapshot of all active mappings.
pub fn list_mappings() -> Vec<MmapRegion> {
    let table = MMAP_TABLE.lock();
    match table.as_ref() {
        Some(tbl) => tbl.regions.clone(),
        None => Vec::new(),
    }
}

/// Return mappings belonging to the specified process.
pub fn list_mappings_for(pid: usize) -> Vec<MmapRegion> {
    let table = MMAP_TABLE.lock();
    match table.as_ref() {
        Some(tbl) => tbl.regions.iter().filter(|r| r.pid == pid).cloned().collect(),
        None => Vec::new(),
    }
}

/// Format protection flags as a human-readable "rwx" string.
pub fn prot_string(prot: u8) -> [u8; 3] {
    [
        if prot & PROT_READ != 0 { b'r' } else { b'-' },
        if prot & PROT_WRITE != 0 { b'w' } else { b'-' },
        if prot & PROT_EXEC != 0 { b'x' } else { b'-' },
    ]
}

/// Print a formatted table of all active mappings to serial.
pub fn print_mappings() {
    let mappings = list_mappings();
    if mappings.is_empty() {
        crate::serial_println!("[mmap] no active mappings");
        return;
    }
    crate::serial_println!("[mmap] {} active mapping(s):", mappings.len());
    crate::serial_println!("  {:>4}  {:>18}  {:>8}  {:>4}  {}", "PID", "VADDR", "LEN", "PROT", "FILE");
    for m in &mappings {
        let p = prot_string(m.prot);
        let ps = core::str::from_utf8(&p).unwrap_or("---");
        crate::serial_println!("  {:>4}  {:#018x}  {:>8}  {:>4}  {} +{:#x}",
            m.pid, m.virt_addr, m.length, ps, m.file_path, m.offset);
    }
}

/// Return mmap subsystem statistics: (pages_mapped, pages_faulted, syncs).
pub fn stats() -> (u64, u64, u64) {
    (
        PAGES_MAPPED.load(Ordering::Relaxed),
        PAGES_FAULTED.load(Ordering::Relaxed),
        SYNCS_PERFORMED.load(Ordering::Relaxed),
    )
}
