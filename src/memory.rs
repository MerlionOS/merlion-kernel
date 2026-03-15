/// Physical and virtual memory management.
/// Provides access to the active page table and a frame allocator
/// backed by the bootloader's memory map.

use bootloader::bootinfo::{MemoryMap, MemoryRegionType};
use x86_64::structures::paging::{OffsetPageTable, PageTable, PhysFrame, Size4KiB};
use x86_64::structures::paging::FrameAllocator;
use x86_64::{PhysAddr, VirtAddr};

/// Initialize an OffsetPageTable using the bootloader's physical memory mapping.
///
/// # Safety
/// The caller must ensure `physical_memory_offset` is the correct offset
/// that the bootloader used to map all physical memory.
pub unsafe fn init(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    let level_4_table = active_level_4_table(physical_memory_offset);
    unsafe { OffsetPageTable::new(level_4_table, physical_memory_offset) }
}

/// Returns a mutable reference to the active level 4 page table.
unsafe fn active_level_4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (level_4_frame, _) = Cr3::read();
    let phys = level_4_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();
    unsafe { &mut *page_table_ptr }
}

/// A frame allocator that walks through usable memory regions linearly.
/// Each allocate_frame() call is O(1).
pub struct BootInfoFrameAllocator {
    memory_map: &'static MemoryMap,
    region_index: usize,
    offset_in_region: u64,
}

impl BootInfoFrameAllocator {
    /// Create a new allocator from the bootloader-provided memory map.
    ///
    /// # Safety
    /// The caller must guarantee that the memory map is valid and that
    /// all `Usable` regions are actually unused.
    pub unsafe fn init(memory_map: &'static MemoryMap) -> Self {
        let mut alloc = Self {
            memory_map,
            region_index: 0,
            offset_in_region: 0,
        };
        // Advance to the first usable region
        alloc.skip_to_usable();
        alloc
    }

    /// Advance region_index to the next Usable region.
    fn skip_to_usable(&mut self) {
        while self.region_index < self.memory_map.len() {
            if self.memory_map[self.region_index].region_type == MemoryRegionType::Usable {
                return;
            }
            self.region_index += 1;
        }
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        loop {
            if self.region_index >= self.memory_map.len() {
                return None;
            }

            let region = &self.memory_map[self.region_index];
            let region_start = region.range.start_addr();
            let region_size = region.range.end_addr() - region_start;
            let addr = region_start + self.offset_in_region;

            if self.offset_in_region < region_size {
                self.offset_in_region += 4096;
                return Some(PhysFrame::containing_address(PhysAddr::new(addr)));
            }

            // Current region exhausted, move to next usable region
            self.region_index += 1;
            self.offset_in_region = 0;
            self.skip_to_usable();
        }
    }
}
