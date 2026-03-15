/// Slab allocator for fixed-size kernel objects.
/// Provides fast allocation/deallocation for objects of known sizes
/// (e.g., task structs, IPC messages, file descriptors).
///
/// Each slab manages objects of one size. A cache holds multiple slabs.

use alloc::vec::Vec;
use spin::Mutex;

const SLAB_SIZE: usize = 4096; // one page per slab
const MAX_CACHES: usize = 8;

static CACHES: Mutex<Vec<SlabCache>> = Mutex::new(Vec::new());

struct SlabCache {
    name: &'static str,
    obj_size: usize,
    allocated: usize,
    freed: usize,
    slab_data: Vec<u8>,
    free_list: Vec<usize>, // offsets of free slots
}

impl SlabCache {
    fn new(name: &'static str, obj_size: usize) -> Self {
        let capacity = SLAB_SIZE / obj_size;
        let mut free_list = Vec::with_capacity(capacity);
        // Initialize free list with all slot offsets
        for i in (0..capacity).rev() {
            free_list.push(i * obj_size);
        }

        Self {
            name,
            obj_size,
            allocated: 0,
            freed: 0,
            slab_data: alloc::vec![0u8; SLAB_SIZE],
            free_list,
        }
    }

    fn alloc(&mut self) -> Option<*mut u8> {
        let offset = self.free_list.pop()?;
        self.allocated += 1;
        Some(unsafe { self.slab_data.as_mut_ptr().add(offset) })
    }

    fn free(&mut self, ptr: *mut u8) -> bool {
        let base = self.slab_data.as_ptr() as usize;
        let addr = ptr as usize;
        if addr < base || addr >= base + SLAB_SIZE {
            return false;
        }
        let offset = addr - base;
        if offset % self.obj_size != 0 {
            return false;
        }
        self.free_list.push(offset);
        self.freed += 1;
        true
    }

    fn in_use(&self) -> usize {
        self.allocated - self.freed
    }

    fn capacity(&self) -> usize {
        SLAB_SIZE / self.obj_size
    }
}

/// Create a named slab cache for objects of `obj_size` bytes.
pub fn create_cache(name: &'static str, obj_size: usize) -> Result<(), &'static str> {
    let mut caches = CACHES.lock();
    if caches.len() >= MAX_CACHES {
        return Err("max slab caches reached");
    }
    if caches.iter().any(|c| c.name == name) {
        return Err("cache already exists");
    }
    // Minimum object size of 8 bytes for alignment
    let size = if obj_size < 8 { 8 } else { obj_size };
    caches.push(SlabCache::new(name, size));
    crate::klog_println!("[slab] created cache '{}' (obj_size={})", name, size);
    Ok(())
}

/// Allocate an object from a named cache.
pub fn alloc(cache_name: &'static str) -> Option<*mut u8> {
    let mut caches = CACHES.lock();
    for cache in caches.iter_mut() {
        if cache.name == cache_name {
            return cache.alloc();
        }
    }
    None
}

/// Free an object back to a named cache.
pub fn free(cache_name: &'static str, ptr: *mut u8) -> bool {
    let mut caches = CACHES.lock();
    for cache in caches.iter_mut() {
        if cache.name == cache_name {
            return cache.free(ptr);
        }
    }
    false
}

/// Slab cache statistics.
pub struct SlabStats {
    pub name: &'static str,
    pub obj_size: usize,
    pub capacity: usize,
    pub in_use: usize,
    pub allocated: usize,
    pub freed: usize,
}

/// Get statistics for all slab caches.
pub fn stats() -> Vec<SlabStats> {
    let caches = CACHES.lock();
    caches.iter().map(|c| SlabStats {
        name: c.name,
        obj_size: c.obj_size,
        capacity: c.capacity(),
        in_use: c.in_use(),
        allocated: c.allocated,
        freed: c.freed,
    }).collect()
}

/// Initialize default slab caches for common kernel objects.
pub fn init() {
    let _ = create_cache("task", 256);      // task control blocks
    let _ = create_cache("ipc_msg", 64);    // IPC messages
    let _ = create_cache("fd", 32);         // file descriptors
    let _ = create_cache("page_info", 16);  // page metadata
}
