/// Copy-on-Write page tracking for fork.
///
/// Tracks which physical frames are shared between processes.
/// When a process forks, shared pages are marked read-only and
/// tracked here. A write fault on a CoW page triggers a copy:
/// allocate new frame, copy data, remap as writable, decrement refcount.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::serial_println;

// ═══════════════════════════════════════════════════════════════════
//  COW TRACKER
// ═══════════════════════════════════════════════════════════════════

struct CowTracker {
    /// Map from physical frame address (page-aligned) to reference count.
    ref_counts: BTreeMap<u64, u32>,
}

impl CowTracker {
    const fn new() -> Self {
        Self { ref_counts: BTreeMap::new() }
    }

    /// Mark a frame as shared (increment refcount).
    fn share_frame(&mut self, frame_addr: u64) {
        let aligned = frame_addr & !0xFFF;
        let count = self.ref_counts.entry(aligned).or_insert(1);
        *count += 1;
    }

    /// Check if a frame is shared (refcount > 1).
    fn is_shared(&self, frame_addr: u64) -> bool {
        let aligned = frame_addr & !0xFFF;
        self.ref_counts.get(&aligned).map_or(false, |&c| c > 1)
    }

    /// Decrement refcount. Returns true if frame is no longer shared.
    fn unshare_frame(&mut self, frame_addr: u64) -> bool {
        let aligned = frame_addr & !0xFFF;
        if let Some(count) = self.ref_counts.get_mut(&aligned) {
            *count = count.saturating_sub(1);
            if *count <= 1 {
                self.ref_counts.remove(&aligned);
                return true;
            }
        }
        false
    }

    fn shared_count(&self) -> usize {
        self.ref_counts.len()
    }

    fn total_refs(&self) -> u64 {
        self.ref_counts.values().map(|&v| v as u64).sum()
    }
}

static COW_TRACKER: Mutex<CowTracker> = Mutex::new(CowTracker::new());
static COW_FAULTS: AtomicU64 = AtomicU64::new(0);
static COW_COPIES: AtomicU64 = AtomicU64::new(0);

// ═══════════════════════════════════════════════════════════════════
//  PUBLIC API
// ═══════════════════════════════════════════════════════════════════

/// Mark a physical frame as shared between two processes.
pub fn share_frame(frame_addr: u64) {
    COW_TRACKER.lock().share_frame(frame_addr);
}

/// Check if a frame is shared (refcount > 1).
pub fn is_shared(frame_addr: u64) -> bool {
    COW_TRACKER.lock().is_shared(frame_addr)
}

/// Decrement the refcount on a frame. Returns true if no longer shared.
pub fn unshare_frame(frame_addr: u64) -> bool {
    COW_TRACKER.lock().unshare_frame(frame_addr)
}

/// Handle a Copy-on-Write page fault at `fault_addr`.
/// If the faulting page maps to a shared frame, copies it and returns
/// the new frame's physical address. Returns None if not a CoW fault.
#[cfg(target_arch = "x86_64")]
pub fn handle_cow_fault(fault_addr: u64) -> Option<u64> {
    use x86_64::structures::paging::{Page, PageTableFlags};
    use x86_64::VirtAddr;

    COW_FAULTS.fetch_add(1, Ordering::Relaxed);

    let page_addr = fault_addr & !0xFFF;

    // Check if the page maps to a shared frame
    // We need to read the current PTE to get the physical frame
    let offset = crate::memory::phys_mem_offset();
    let level_4 = unsafe { crate::memory::active_level_4_table(offset) };
    let mapper = unsafe { x86_64::structures::paging::OffsetPageTable::new(level_4, offset) };

    use x86_64::structures::paging::Mapper;
    let page = Page::<x86_64::structures::paging::Size4KiB>::containing_address(
        VirtAddr::new(page_addr),
    );

    let translate = mapper.translate_page(page);
    let old_frame = match translate {
        Ok(frame) => frame,
        Err(_) => return None,
    };

    let old_phys = old_frame.start_address().as_u64();

    if !is_shared(old_phys) {
        return None; // Not a CoW page
    }

    // Allocate new frame
    let new_frame = crate::memory::alloc_frame()?;
    let new_phys = new_frame.start_address().as_u64();

    // Copy old frame contents to new frame
    let old_virt = crate::memory::phys_to_virt(old_frame.start_address());
    let new_virt = crate::memory::phys_to_virt(new_frame.start_address());
    unsafe {
        core::ptr::copy_nonoverlapping(
            old_virt.as_ptr::<u8>(),
            new_virt.as_mut_ptr::<u8>(),
            4096,
        );
    }

    // Unmap old, remap with new frame + writable
    let user_rw = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE;

    // We can't easily remap in the current architecture without the full
    // page table management infrastructure, so just unmap + map.
    // In practice, this would use the per-process page table.
    let offset2 = crate::memory::phys_mem_offset();
    let level_4_2 = unsafe { crate::memory::active_level_4_table(offset2) };
    let mut mapper2 = unsafe { x86_64::structures::paging::OffsetPageTable::new(level_4_2, offset2) };

    // Unmap old mapping
    if let Ok((_frame, flusher)) = mapper2.unmap(page) {
        flusher.flush();
    }

    // Map new frame
    let mut pre = crate::memory::PreAllocatedFrames {
        frames: [crate::memory::alloc_frame(), crate::memory::alloc_frame(), crate::memory::alloc_frame()],
        next: 0,
    };
    unsafe {
        if let Ok(flusher) = mapper2.map_to(page, new_frame, user_rw, &mut pre) {
            flusher.flush();
        }
    }

    // Decrement refcount on old frame
    unshare_frame(old_phys);

    COW_COPIES.fetch_add(1, Ordering::Relaxed);
    serial_println!("[cow] page fault at {:#x}: copied frame {:#x} -> {:#x}", fault_addr, old_phys, new_phys);

    Some(new_phys)
}

#[cfg(not(target_arch = "x86_64"))]
pub fn handle_cow_fault(_fault_addr: u64) -> Option<u64> { None }

// ═══════════════════════════════════════════════════════════════════
//  INFO
// ═══════════════════════════════════════════════════════════════════

pub fn info() -> String {
    let tracker = COW_TRACKER.lock();
    format!(
        "Copy-on-Write Page Tracker\n\
         Shared frames:  {}\n\
         Total refs:     {}\n\
         CoW faults:     {}\n\
         Pages copied:   {}\n",
        tracker.shared_count(),
        tracker.total_refs(),
        COW_FAULTS.load(Ordering::Relaxed),
        COW_COPIES.load(Ordering::Relaxed),
    )
}

pub fn init() {
    serial_println!("[cow] Copy-on-Write page tracker initialized");
}
