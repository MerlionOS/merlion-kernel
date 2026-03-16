/// Shared memory IPC for MerlionOS.
///
/// Allows processes to create named shared memory regions backed by physical
/// frames.  Other processes can attach (map) and detach (unmap) regions to
/// exchange data at memory speed.  A region is automatically destroyed when
/// its reference count drops to zero.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::memory;
use x86_64::structures::paging::{PhysFrame, Size4KiB};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Maximum number of concurrent shared memory regions.
const MAX_REGIONS: usize = 16;

/// Base virtual address where shared memory is mapped for attaching processes.
/// Each region occupies `size` bytes starting at SHMEM_VIRT_BASE + id * MAX_REGION_SIZE.
const SHMEM_VIRT_BASE: u64 = 0x6000_0000_0000;

/// Maximum size of a single shared memory region (64 KiB).
const MAX_REGION_SIZE: usize = 64 * 1024;

/// Page size constant (4 KiB).
const PAGE_SIZE: usize = 4096;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Unique identifier for a shared memory region.
pub type ShmemId = usize;

/// Unique identifier for a process.
pub type Pid = usize;

/// Describes a shared memory region.
struct SharedMemRegion {
    /// Unique region identifier (index into the table).
    id: ShmemId,
    /// Human-readable name for the region.
    name: String,
    /// Size in bytes (rounded up to page boundary at creation time).
    size: usize,
    /// First physical frame backing this region (contiguous allocation).
    phys_frame: PhysFrame<Size4KiB>,
    /// Number of processes currently attached.
    ref_count: usize,
    /// PID of the process that created this region.
    owner_pid: Pid,
    /// PIDs of all currently-attached processes.
    attached: Vec<Pid>,
}

/// Public snapshot of a region's metadata, safe to return to callers.
pub struct ShmemInfo {
    pub id: ShmemId,
    pub name: String,
    pub size: usize,
    pub ref_count: usize,
    pub owner_pid: Pid,
}

/// Slot in the global shared-memory table.
enum Slot {
    Free,
    Active(SharedMemRegion),
}

/// Global table holding all shared memory regions.
struct ShmemTable {
    slots: [Slot; MAX_REGIONS],
    next_id: usize,
}

impl ShmemTable {
    const fn new() -> Self {
        Self {
            slots: [const { Slot::Free }; MAX_REGIONS],
            next_id: 0,
        }
    }
}

/// Global shared memory table protected by a spin-lock.
static TABLE: Mutex<ShmemTable> = Mutex::new(ShmemTable::new());

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Round `size` up to the nearest multiple of `PAGE_SIZE`.
fn round_up(size: usize) -> usize {
    (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

/// Compute the virtual address for a region given its table index.
fn virt_addr_for(index: usize) -> u64 {
    SHMEM_VIRT_BASE + (index as u64) * (MAX_REGION_SIZE as u64)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new shared memory region.
///
/// Allocates enough physical frames to cover `size` bytes (page-aligned)
/// and registers the region under `name`.  Returns the new region's
/// [`ShmemId`] or `None` if the table is full or allocation fails.
pub fn create_shmem(name: &str, size: usize, owner_pid: Pid) -> Option<ShmemId> {
    if size == 0 || size > MAX_REGION_SIZE {
        return None;
    }
    let aligned = round_up(size);
    let pages_needed = aligned / PAGE_SIZE;

    // Allocate the first physical frame (we allocate one frame at a time).
    let first_frame = memory::alloc_frame()?;

    // Allocate remaining frames (best-effort contiguous from the global
    // allocator).  In a production kernel we would use a buddy allocator;
    // here we simply ensure each page has a backing frame.
    let mut _extra_frames: Vec<PhysFrame<Size4KiB>> = Vec::new();
    for _ in 1..pages_needed {
        _extra_frames.push(memory::alloc_frame()?);
    }

    let mut table = TABLE.lock();
    for (i, slot) in table.slots.iter_mut().enumerate() {
        if matches!(slot, Slot::Free) {
            let id = table.next_id;
            table.next_id += 1;
            *slot = Slot::Active(SharedMemRegion {
                id,
                name: String::from(name),
                size: aligned,
                phys_frame: first_frame,
                ref_count: 0,
                owner_pid,
                attached: Vec::new(),
            });
            return Some(id);
        }
    }
    None // table full
}

/// Attach a process to an existing shared memory region.
///
/// Conceptually maps the region into the process's address space and bumps
/// the reference count.  Returns the virtual address where the region is
/// accessible, or `None` if the region does not exist or the process is
/// already attached.
pub fn attach_shmem(id: ShmemId, pid: Pid) -> Option<u64> {
    let mut table = TABLE.lock();
    for (i, slot) in table.slots.iter_mut().enumerate() {
        if let Slot::Active(ref mut region) = slot {
            if region.id == id {
                if region.attached.contains(&pid) {
                    // Already attached -- return the existing address.
                    return Some(virt_addr_for(i));
                }
                region.attached.push(pid);
                region.ref_count += 1;
                return Some(virt_addr_for(i));
            }
        }
    }
    None
}

/// Detach a process from a shared memory region.
///
/// Decrements the reference count and removes `pid` from the attached list.
/// If the reference count reaches zero the region is destroyed via
/// [`destroy_shmem`].  Returns `true` on success.
pub fn detach_shmem(id: ShmemId, pid: Pid) -> bool {
    let mut table = TABLE.lock();
    for slot in table.slots.iter_mut() {
        if let Slot::Active(ref mut region) = slot {
            if region.id == id {
                if let Some(pos) = region.attached.iter().position(|&p| p == pid) {
                    region.attached.remove(pos);
                    region.ref_count = region.ref_count.saturating_sub(1);
                    if region.ref_count == 0 {
                        *slot = Slot::Free;
                    }
                    return true;
                }
                return false; // pid was not attached
            }
        }
    }
    false
}

/// Explicitly destroy a shared memory region regardless of reference count.
///
/// This is typically called by the owning process.  All attached processes
/// lose access immediately.
pub fn destroy_shmem(id: ShmemId) -> bool {
    let mut table = TABLE.lock();
    for slot in table.slots.iter_mut() {
        if let Slot::Active(ref region) = slot {
            if region.id == id {
                *slot = Slot::Free;
                return true;
            }
        }
    }
    false
}

/// List all active shared memory regions.
pub fn list_shmem() -> Vec<ShmemInfo> {
    let table = TABLE.lock();
    let mut out = Vec::new();
    for slot in table.slots.iter() {
        if let Slot::Active(ref region) = slot {
            out.push(ShmemInfo {
                id: region.id,
                name: region.name.clone(),
                size: region.size,
                ref_count: region.ref_count,
                owner_pid: region.owner_pid,
            });
        }
    }
    out
}

/// Convenience helper: write `data` into shared memory region `id` at
/// byte offset `offset`.
///
/// Because we cannot safely dereference arbitrary physical addresses from
/// every context, this helper works through the physical-memory identity
/// mapping provided by the bootloader.  Returns the number of bytes
/// actually written (capped to the region size).
pub fn shmem_write(id: ShmemId, offset: usize, data: &[u8]) -> usize {
    let table = TABLE.lock();
    for slot in table.slots.iter() {
        if let Slot::Active(ref region) = slot {
            if region.id == id {
                if offset >= region.size {
                    return 0;
                }
                let max_len = region.size - offset;
                let len = data.len().min(max_len);
                let phys_base = region.phys_frame.start_address().as_u64();
                let virt = memory::phys_mem_offset() + phys_base + offset as u64;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        data.as_ptr(),
                        virt.as_mut_ptr::<u8>(),
                        len,
                    );
                }
                return len;
            }
        }
    }
    0
}

/// Convenience helper: read up to `len` bytes from shared memory region
/// `id` starting at byte offset `offset`.
pub fn shmem_read(id: ShmemId, offset: usize, len: usize) -> Vec<u8> {
    let table = TABLE.lock();
    for slot in table.slots.iter() {
        if let Slot::Active(ref region) = slot {
            if region.id == id {
                if offset >= region.size {
                    return Vec::new();
                }
                let max_len = region.size - offset;
                let actual = len.min(max_len);
                let phys_base = region.phys_frame.start_address().as_u64();
                let virt = memory::phys_mem_offset() + phys_base + offset as u64;
                let mut buf = alloc::vec![0u8; actual];
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        virt.as_ptr::<u8>(),
                        buf.as_mut_ptr(),
                        actual,
                    );
                }
                return buf;
            }
        }
    }
    Vec::new()
}
