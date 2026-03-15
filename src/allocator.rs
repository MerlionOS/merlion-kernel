/// Kernel heap allocator.
/// Maps a fixed virtual address range for the heap and uses a
/// linked-list allocator for dynamic memory allocation.

use x86_64::structures::paging::{
    FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB,
};
use x86_64::VirtAddr;
use linked_list_allocator::LockedHeap;

/// Heap starts at a fixed virtual address above the kernel.
pub const HEAP_START: usize = 0x4444_4444_0000;
/// 64 KiB heap — plenty for Phase 3.
pub const HEAP_SIZE: usize = 64 * 1024;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// Initialize the kernel heap: map pages and set up the allocator.
pub fn init(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), &'static str> {
    let heap_start = VirtAddr::new(HEAP_START as u64);
    let heap_end = heap_start + HEAP_SIZE as u64 - 1u64;
    let page_range = {
        let start_page = Page::containing_address(heap_start);
        let end_page = Page::containing_address(heap_end);
        Page::range_inclusive(start_page, end_page)
    };

    // Map each heap page to a physical frame
    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or("out of physical memory for heap")?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        unsafe {
            mapper
                .map_to(page, frame, flags, frame_allocator)
                .map_err(|_| "failed to map heap page")?
                .flush();
        }
    }

    // Initialize the allocator with the mapped region
    unsafe {
        ALLOCATOR.lock().init(HEAP_START as *mut u8, HEAP_SIZE);
    }

    Ok(())
}

pub struct HeapStats {
    pub used: usize,
    pub free: usize,
    pub total: usize,
}

pub fn stats() -> HeapStats {
    let used = ALLOCATOR.lock().used();
    let free = ALLOCATOR.lock().free();
    HeapStats { used, free, total: HEAP_SIZE }
}
