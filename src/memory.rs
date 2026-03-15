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

/// A frame allocator that returns usable frames from the bootloader's memory map.
pub struct BootInfoFrameAllocator {
    memory_map: &'static MemoryMap,
    next: usize,
}

impl BootInfoFrameAllocator {
    /// Create a new allocator from the bootloader-provided memory map.
    ///
    /// # Safety
    /// The caller must guarantee that the memory map is valid and that
    /// all `Usable` regions are actually unused.
    pub unsafe fn init(memory_map: &'static MemoryMap) -> Self {
        Self { memory_map, next: 0 }
    }

    /// Iterator over all usable physical frames.
    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> + '_ {
        self.memory_map
            .iter()
            .filter(|r| r.region_type == MemoryRegionType::Usable)
            .map(|r| r.range.start_addr()..r.range.end_addr())
            .flat_map(|r| r.step_by(4096))
            .map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let frame = self.usable_frames().nth(self.next);
        self.next += 1;
        frame
    }
}
