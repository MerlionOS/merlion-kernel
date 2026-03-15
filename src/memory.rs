/// Physical and virtual memory management.
/// Provides page table access, frame allocation, and per-process
/// address space creation.

use bootloader::bootinfo::{MemoryMap, MemoryRegionType};
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};
use spin::Mutex;

/// Global frame allocator, initialized during boot.
static FRAME_ALLOCATOR: Mutex<Option<BootInfoFrameAllocator>> = Mutex::new(None);

/// Saved reference to the memory map for display.
static MEMORY_MAP: Mutex<Option<&'static MemoryMap>> = Mutex::new(None);

/// Physical memory offset used by the bootloader's identity mapping.
static mut PHYS_MEM_OFFSET: u64 = 0;

/// Initialize the memory system: page tables, frame allocator, global state.
///
/// # Safety
/// Must be called exactly once with the correct boot info values.
pub unsafe fn init(
    physical_memory_offset: VirtAddr,
    memory_map: &'static MemoryMap,
) -> OffsetPageTable<'static> {
    unsafe { PHYS_MEM_OFFSET = physical_memory_offset.as_u64() };
    *MEMORY_MAP.lock() = Some(memory_map);

    let frame_alloc = unsafe { BootInfoFrameAllocator::init(memory_map) };
    *FRAME_ALLOCATOR.lock() = Some(frame_alloc);

    let level_4_table = unsafe { active_level_4_table(physical_memory_offset) };
    unsafe { OffsetPageTable::new(level_4_table, physical_memory_offset) }
}

pub fn phys_mem_offset() -> VirtAddr {
    VirtAddr::new(unsafe { PHYS_MEM_OFFSET })
}

/// Allocate a physical frame from the global allocator.
pub fn alloc_frame() -> Option<PhysFrame<Size4KiB>> {
    FRAME_ALLOCATOR.lock().as_mut()?.allocate_frame()
}

/// Provide mutable access to the global frame allocator (for heap init).
pub fn with_frame_allocator<R>(f: impl FnOnce(&mut BootInfoFrameAllocator) -> R) -> Option<R> {
    let mut lock = FRAME_ALLOCATOR.lock();
    lock.as_mut().map(f)
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

/// Convert a physical address to a virtual address using the bootloader's mapping.
pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    phys_mem_offset() + phys.as_u64()
}

/// Map a page in the current (kernel) page table using the global frame allocator.
/// Used by the demand paging fault handler.
pub fn map_page_global(
    page: Page<Size4KiB>,
    flags: PageTableFlags,
) -> Option<PhysFrame> {
    let frame = alloc_frame()?;
    let offset = phys_mem_offset();
    let level_4_table = unsafe { active_level_4_table(offset) };
    let mut mapper = unsafe { OffsetPageTable::new(level_4_table, offset) };

    unsafe {
        mapper
            .map_to(page, frame, flags, &mut GlobalFrameAllocWrapper)
            .ok()?
            .flush();
    }
    Some(frame)
}

/// Create a new page table for a user process by cloning kernel mappings.
/// Returns the PML4 physical frame and an OffsetPageTable for the new address space.
pub fn create_user_page_table() -> Option<(PhysFrame, OffsetPageTable<'static>)> {
    let pml4_frame = alloc_frame()?;
    let offset = phys_mem_offset();

    // Get a mutable reference to the new PML4
    let new_pml4: &mut PageTable = unsafe {
        &mut *(phys_to_virt(pml4_frame.start_address()).as_mut_ptr())
    };

    // Zero the entire table
    new_pml4.zero();

    // Copy kernel entries (upper half: indices 256..512)
    let kernel_pml4 = unsafe { active_level_4_table(offset) };
    for i in 256..512 {
        new_pml4[i] = kernel_pml4[i].clone();
    }

    let mapper = unsafe { OffsetPageTable::new(new_pml4, offset) };
    Some((pml4_frame, mapper))
}

/// Map a page in the given page table with the specified flags.
pub fn map_page(
    mapper: &mut impl Mapper<Size4KiB>,
    page: Page<Size4KiB>,
    flags: PageTableFlags,
) -> Option<PhysFrame> {
    let frame = alloc_frame()?;

    // The frame allocator itself is behind a Mutex; we need a temporary
    // allocator wrapper for the map_to call's page-table frame allocation.
    unsafe {
        mapper
            .map_to(page, frame, flags, &mut GlobalFrameAllocWrapper)
            .ok()?
            .flush();
    }
    Some(frame)
}

/// Wrapper that lets map_to allocate page table frames from the global allocator.
struct GlobalFrameAllocWrapper;

unsafe impl FrameAllocator<Size4KiB> for GlobalFrameAllocWrapper {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        alloc_frame()
    }
}

// --- Frame allocator ---

/// A frame allocator that walks through usable memory regions linearly.
pub struct BootInfoFrameAllocator {
    memory_map: &'static MemoryMap,
    region_index: usize,
    offset_in_region: u64,
}

impl BootInfoFrameAllocator {
    /// # Safety
    /// The caller must guarantee that the memory map is valid.
    pub unsafe fn init(memory_map: &'static MemoryMap) -> Self {
        let mut alloc = Self {
            memory_map,
            region_index: 0,
            offset_in_region: 0,
        };
        alloc.skip_to_usable();
        alloc
    }

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

            self.region_index += 1;
            self.offset_in_region = 0;
            self.skip_to_usable();
        }
    }
}

// --- Memory statistics ---

pub struct MemoryStats {
    pub total_usable_bytes: u64,
    pub allocated_frames: u64,
    pub total_regions: usize,
}

/// Get physical memory statistics.
pub fn stats() -> MemoryStats {
    let map_lock = MEMORY_MAP.lock();
    let fa_lock = FRAME_ALLOCATOR.lock();

    let (total_usable, total_regions) = if let Some(map) = *map_lock {
        let usable: u64 = map
            .iter()
            .filter(|r| r.region_type == MemoryRegionType::Usable)
            .map(|r| r.range.end_addr() - r.range.start_addr())
            .sum();
        (usable, map.len())
    } else {
        (0, 0)
    };

    // Count allocated frames based on allocator position
    let allocated = if let Some(fa) = fa_lock.as_ref() {
        // Sum all frames from regions before current, plus offset in current
        let map = fa.memory_map;
        let mut count: u64 = 0;
        for (i, region) in map.iter().enumerate() {
            if region.region_type != MemoryRegionType::Usable {
                continue;
            }
            let region_size = region.range.end_addr() - region.range.start_addr();
            if i < fa.region_index {
                count += region_size / 4096;
            } else if i == fa.region_index {
                count += fa.offset_in_region / 4096;
                break;
            }
        }
        count
    } else {
        0
    };

    MemoryStats {
        total_usable_bytes: total_usable,
        allocated_frames: allocated,
        total_regions,
    }
}

/// Memory region info for display.
pub struct RegionInfo {
    pub start: u64,
    pub end: u64,
    pub kind: &'static str,
    pub size_kb: u64,
}

/// Get the bootloader memory map for display.
pub fn memory_map() -> alloc::vec::Vec<RegionInfo> {
    let map_lock = MEMORY_MAP.lock();
    let mut result = alloc::vec::Vec::new();

    if let Some(map) = *map_lock {
        for region in map.iter() {
            let kind = match region.region_type {
                MemoryRegionType::Usable => "usable",
                MemoryRegionType::InUse => "in use",
                MemoryRegionType::Reserved => "reserved",
                MemoryRegionType::AcpiReclaimable => "ACPI",
                MemoryRegionType::AcpiNvs => "ACPI NVS",
                MemoryRegionType::BadMemory => "bad",
                MemoryRegionType::Kernel => "kernel",
                MemoryRegionType::KernelStack => "kstack",
                MemoryRegionType::PageTable => "pagetbl",
                MemoryRegionType::Bootloader => "bootldr",
                MemoryRegionType::FrameZero => "frame0",
                MemoryRegionType::Empty => continue,
                _ => "other",
            };
            let size = region.range.end_addr() - region.range.start_addr();
            result.push(RegionInfo {
                start: region.range.start_addr(),
                end: region.range.end_addr(),
                kind,
                size_kb: size / 1024,
            });
        }
    }

    result
}
