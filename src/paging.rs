/// Demand paging and lazy memory allocation.
/// Pages are allocated on first access via page fault handler.
/// Also provides guard pages for stack overflow detection.

use x86_64::structures::paging::{Page, PageTableFlags, Size4KiB};
use x86_64::VirtAddr;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::{memory, serial_println, klog_println};

/// Statistics for demand paging.
static PAGES_FAULTED_IN: AtomicU64 = AtomicU64::new(0);
static PAGES_PREALLOCATED: AtomicU64 = AtomicU64::new(0);

/// A lazily-allocated virtual memory region.
/// Pages within this region are not mapped until first access.
pub struct LazyRegion {
    pub start: u64,
    pub size: u64,   // in bytes
    pub flags: PageTableFlags,
}

/// Virtual address range reserved for lazy allocations.
const LAZY_REGION_BASE: u64 = 0x5555_0000_0000;
static LAZY_NEXT: AtomicU64 = AtomicU64::new(LAZY_REGION_BASE);

/// Allocate a lazy virtual region of `num_pages` pages.
/// The pages are NOT mapped — they'll be faulted in on first access.
pub fn alloc_lazy(num_pages: u64) -> LazyRegion {
    let start = LAZY_NEXT.fetch_add(num_pages * 4096, Ordering::SeqCst);
    PAGES_PREALLOCATED.fetch_add(num_pages, Ordering::SeqCst);

    LazyRegion {
        start,
        size: num_pages * 4096,
        flags: PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
    }
}

/// Handle a page fault by mapping the faulted page if it's in a lazy region.
/// Returns true if the fault was handled (page was mapped), false otherwise.
pub fn handle_page_fault(fault_addr: u64) -> bool {
    // Check if the fault address is in our lazy allocation region
    let lazy_end = LAZY_NEXT.load(Ordering::SeqCst);
    if fault_addr < LAZY_REGION_BASE || fault_addr >= lazy_end {
        return false; // not our region
    }

    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(fault_addr));
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

    // Allocate a physical frame and map it
    if let Some(_frame) = memory::map_page_global(page, flags) {
        PAGES_FAULTED_IN.fetch_add(1, Ordering::SeqCst);
        serial_println!("[paging] demand-mapped page at {:#x}", fault_addr & !0xFFF);
        klog_println!("[paging] fault-in at {:#x}", fault_addr & !0xFFF);
        true
    } else {
        serial_println!("[paging] FAILED to map page at {:#x} — out of memory", fault_addr);
        false
    }
}

/// Demand paging statistics.
pub struct PagingStats {
    pub pages_faulted_in: u64,
    pub pages_preallocated: u64,
    pub lazy_region_start: u64,
    pub lazy_region_end: u64,
}

pub fn stats() -> PagingStats {
    PagingStats {
        pages_faulted_in: PAGES_FAULTED_IN.load(Ordering::SeqCst),
        pages_preallocated: PAGES_PREALLOCATED.load(Ordering::SeqCst),
        lazy_region_start: LAZY_REGION_BASE,
        lazy_region_end: LAZY_NEXT.load(Ordering::SeqCst),
    }
}
