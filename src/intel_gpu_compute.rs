/// Intel GPU Compute driver for MerlionOS.
/// Uses EU (Execution Unit) compute shaders for matrix operations.
/// Targets Gen9/Gen9.5 (Kaby Lake HD 630) for AI inference.
/// Uses batch buffers and GPGPU_WALKER for compute dispatch.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Constants — MMIO registers for compute
// ---------------------------------------------------------------------------

// Render ring MMIO offsets (relative to ring base)
const RING_HEAD: u32 = 0x00;
const RING_TAIL: u32 = 0x04;
const RING_START: u32 = 0x08;
const RING_CTL: u32 = 0x0C;

// Render ring base address in MMIO space
const RENDER_RING_BASE: u32 = 0x02000;

// GGTT (Global GTT) base offset in BAR0
const GGTT_BASE: u32 = 0x80_0000; // 8 MB offset into BAR0

// Batch buffer command opcodes (MI = Memory Interface)
const MI_NOOP: u32 = 0x00;
const MI_BATCH_BUFFER_START: u32 = 0x31;
const MI_BATCH_BUFFER_END: u32 = 0x0A;
const MI_STORE_DATA_IMM: u32 = 0x20;
const MI_FLUSH_DW: u32 = 0x26;

// 3D pipeline commands
const PIPE_CONTROL: u32 = 0x7A;
const GPGPU_WALKER: u32 = 0x05;
const MEDIA_VFE_STATE: u32 = 0x00;
const MEDIA_CURBE_LOAD: u32 = 0x01;
const MEDIA_INTERFACE_DESCRIPTOR_LOAD: u32 = 0x02;

// Simulation limits
const SIM_VRAM_SIZE: usize = 64 * 1024; // 64 KB simulated VRAM (save heap)
const RING_SIZE_DWORDS: usize = 8192;

// Page size for GGTT
const GGTT_PAGE_SIZE: u64 = 4096;

// Allocation ID counter
static NEXT_ALLOC_ID: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------
// 1. GPU Memory Manager
// ---------------------------------------------------------------------------

/// A single GPU memory allocation.
#[derive(Clone)]
pub struct GpuAlloc {
    pub id: u32,
    pub gpu_addr: u64,
    pub size: usize,
    pub name: String,
}

/// GPU memory allocator using a simple bump allocator over stolen memory.
pub struct GpuMemory {
    vram_base: u64,
    vram_size: u64,
    next_free: u64,
    allocations: Vec<GpuAlloc>,
    sim_buffer: Option<Vec<u8>>,
}

impl GpuMemory {
    /// Create a new GPU memory manager.
    /// If `simulated` is true, allocates a host buffer to back VRAM.
    fn new(vram_base: u64, vram_size: u64, simulated: bool) -> Self {
        let sim_buffer = if simulated {
            let mut buf = Vec::new();
            buf.resize(vram_size as usize, 0u8);
            Some(buf)
        } else {
            None
        };
        Self {
            vram_base,
            vram_size,
            next_free: 0,
            allocations: Vec::new(),
            sim_buffer,
        }
    }

    /// Bump-allocate `size` bytes. Returns the allocation descriptor.
    pub fn alloc(&mut self, size: usize, name: &str) -> Result<GpuAlloc, &'static str> {
        if size == 0 {
            return Err("allocation size must be non-zero");
        }
        // Align to 256 bytes (GPU cacheline alignment)
        let aligned_size = (size + 255) & !255;
        if self.next_free + aligned_size as u64 > self.vram_size {
            return Err("out of GPU memory");
        }
        let id = NEXT_ALLOC_ID.fetch_add(1, Ordering::Relaxed) as u32;
        let alloc = GpuAlloc {
            id,
            gpu_addr: self.next_free,
            size: aligned_size,
            name: String::from(name),
        };
        self.next_free += aligned_size as u64;
        self.allocations.push(alloc.clone());
        Ok(alloc)
    }

    /// Mark an allocation as freed. Does not reclaim space (bump allocator).
    pub fn free(&mut self, id: u32) {
        self.allocations.retain(|a| a.id != id);
    }

    /// Write data to GPU memory at `alloc.gpu_addr + offset`.
    pub fn write(&mut self, alloc: &GpuAlloc, offset: usize, data: &[u8]) {
        if offset + data.len() > alloc.size {
            return;
        }
        let addr = alloc.gpu_addr as usize + offset;
        if let Some(ref mut buf) = self.sim_buffer {
            if addr + data.len() <= buf.len() {
                buf[addr..addr + data.len()].copy_from_slice(data);
            }
        } else {
            let base = self.vram_base + addr as u64;
            for (i, &byte) in data.iter().enumerate() {
                unsafe {
                    let ptr = (base + i as u64) as *mut u8;
                    core::ptr::write_volatile(ptr, byte);
                }
            }
        }
    }

    /// Read data from GPU memory at `alloc.gpu_addr + offset`.
    pub fn read(&self, alloc: &GpuAlloc, offset: usize, buf: &mut [u8]) {
        if offset + buf.len() > alloc.size {
            return;
        }
        let addr = alloc.gpu_addr as usize + offset;
        if let Some(ref sim) = self.sim_buffer {
            if addr + buf.len() <= sim.len() {
                buf.copy_from_slice(&sim[addr..addr + buf.len()]);
            }
        } else {
            let base = self.vram_base + addr as u64;
            for (i, byte) in buf.iter_mut().enumerate() {
                unsafe {
                    let ptr = (base + i as u64) as *const u8;
                    *byte = core::ptr::read_volatile(ptr);
                }
            }
        }
    }

    /// List all current allocations.
    fn list_allocations(&self) -> String {
        if self.allocations.is_empty() {
            return String::from("  (no allocations)");
        }
        let mut s = String::new();
        for a in &self.allocations {
            s.push_str(&format!(
                "  [{}] '{}' @ 0x{:X}, {} bytes\n",
                a.id, a.name, a.gpu_addr, a.size,
            ));
        }
        s
    }

    /// Return memory usage summary.
    fn mem_info(&self) -> String {
        let used = self.next_free;
        let free = self.vram_size - used;
        let total_kb = self.vram_size / 1024;
        let used_kb = used / 1024;
        let free_kb = free / 1024;
        format!(
            "GPU Memory: {} KB total, {} KB used, {} KB free ({} allocations)",
            total_kb, used_kb, free_kb, self.allocations.len(),
        )
    }
}

// ---------------------------------------------------------------------------
// 2. GGTT (Global Graphics Translation Table)
// ---------------------------------------------------------------------------

/// Global GTT manager for mapping GPU virtual addresses to physical addresses.
struct GgttManager {
    bar0_virt: u64,
    num_entries: u32,
    simulated: bool,
    sim_entries: Vec<u64>,
}

impl GgttManager {
    /// Create a new GGTT manager.
    fn new(bar0_virt: u64, aperture_size: u64, simulated: bool) -> Self {
        let num_entries = (aperture_size / GGTT_PAGE_SIZE) as u32;
        let sim_entries = if simulated {
            let count = if num_entries > 0 { num_entries as usize } else { 4096 };
            let mut entries = Vec::new();
            entries.resize(count, 0u64);
            entries
        } else {
            Vec::new()
        };
        Self {
            bar0_virt,
            num_entries,
            simulated,
            sim_entries,
        }
    }

    /// Insert a GGTT page table entry mapping gpu_addr to phys_addr.
    fn ggtt_insert(&mut self, gpu_page: u32, phys_addr: u64) {
        if self.simulated {
            if (gpu_page as usize) < self.sim_entries.len() {
                // Entry format: physical address | valid bit
                self.sim_entries[gpu_page as usize] = phys_addr | 1;
            }
            return;
        }

        if self.bar0_virt == 0 || gpu_page >= self.num_entries {
            return;
        }

        // Each GGTT entry is 8 bytes (Gen8+)
        let entry_offset = GGTT_BASE as u64 + (gpu_page as u64) * 8;
        let pte = phys_addr | 1; // valid bit
        unsafe {
            let ptr = (self.bar0_virt + entry_offset) as *mut u64;
            core::ptr::write_volatile(ptr, pte);
        }
    }

    /// Clear a GGTT page table entry.
    fn ggtt_clear(&mut self, gpu_page: u32) {
        if self.simulated {
            if (gpu_page as usize) < self.sim_entries.len() {
                self.sim_entries[gpu_page as usize] = 0;
            }
            return;
        }

        if self.bar0_virt == 0 || gpu_page >= self.num_entries {
            return;
        }

        let entry_offset = GGTT_BASE as u64 + (gpu_page as u64) * 8;
        unsafe {
            let ptr = (self.bar0_virt + entry_offset) as *mut u64;
            core::ptr::write_volatile(ptr, 0);
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Batch Buffer
// ---------------------------------------------------------------------------

/// Intel GPU batch buffer — a sequence of GPU commands.
/// Intel GPUs use batch buffers instead of AMD's PM4 packets.
pub struct BatchBuffer {
    commands: Vec<u32>,
}

impl BatchBuffer {
    /// Create a new empty batch buffer.
    pub fn new() -> Self {
        Self { commands: Vec::new() }
    }

    /// Emit MI_NOOP padding.
    pub fn mi_noop(&mut self) {
        // MI_NOOP: opcode in bits [28:23] = 0x00
        self.commands.push(0x0000_0000);
    }

    /// Emit MI_BATCH_BUFFER_END to terminate the batch.
    pub fn mi_batch_buffer_end(&mut self) {
        self.commands.push((MI_BATCH_BUFFER_END as u32) << 23);
    }

    /// Emit MI_STORE_DATA_IMM — write an immediate value to a GPU address.
    pub fn mi_store_data_imm(&mut self, addr: u64, value: u32) {
        // MI_STORE_DATA_IMM: length = 4 DWORDs total (header + addr_lo + addr_hi + data)
        let header = ((MI_STORE_DATA_IMM as u32) << 23) | (1 << 22) | 2;
        self.commands.push(header);
        self.commands.push(addr as u32);
        self.commands.push((addr >> 32) as u32);
        self.commands.push(value);
    }

    /// Emit MI_FLUSH_DW — flush write caches.
    pub fn mi_flush_dw(&mut self) {
        let header = ((MI_FLUSH_DW as u32) << 23) | 1;
        self.commands.push(header);
        self.commands.push(0); // addr_lo (unused when not writing)
        self.commands.push(0); // addr_hi
    }

    /// Emit MI_BATCH_BUFFER_START — chain to another batch buffer.
    pub fn mi_batch_buffer_start(&mut self, addr: u64) {
        let header = ((MI_BATCH_BUFFER_START as u32) << 23) | (1 << 8) | 1;
        self.commands.push(header);
        self.commands.push(addr as u32);
        self.commands.push((addr >> 32) as u32);
    }

    /// Emit PIPE_CONTROL — pipeline synchronization and cache flush.
    pub fn pipe_control(&mut self, flags: u32) {
        // PIPE_CONTROL is a 3D command: type=3, subtype=3, opcode=0x7A
        // Header: [31:29]=3 (type3), [28:27]=3 (3D), [26:24]=0, [23:16]=opcode, [7:0]=length-2
        let header = (3u32 << 29) | (3u32 << 27) | ((PIPE_CONTROL as u32) << 16) | 3;
        self.commands.push(header);
        self.commands.push(flags);
        self.commands.push(0); // addr_lo
        self.commands.push(0); // addr_hi
        self.commands.push(0); // imm_data
    }

    /// Emit MEDIA_VFE_STATE — configure Variable Function Execution engine.
    pub fn media_vfe_state(&mut self, max_threads: u32, curbe_size: u32) {
        // MEDIA_VFE_STATE: type=3, subtype=2 (media), opcode=0x00
        let header = (3u32 << 29) | (2u32 << 27) | ((MEDIA_VFE_STATE as u32) << 16) | 6;
        self.commands.push(header);
        self.commands.push(0); // scratch space base
        self.commands.push(max_threads & 0xFFFF); // max number of threads
        self.commands.push(0); // scoreboard control
        self.commands.push(curbe_size & 0xFFFF); // CURBE allocation size (in 256-bit units)
        self.commands.push(0); // reserved
        self.commands.push(0); // reserved
        self.commands.push(0); // reserved
    }

    /// Emit MEDIA_CURBE_LOAD — load constant URB entry data.
    pub fn media_curbe_load(&mut self, data_addr: u64, size: u32) {
        // MEDIA_CURBE_LOAD: type=3, subtype=2 (media), opcode=0x01
        let header = (3u32 << 29) | (2u32 << 27) | ((MEDIA_CURBE_LOAD as u32) << 16) | 2;
        self.commands.push(header);
        self.commands.push(size); // CURBE total data length
        self.commands.push(data_addr as u32); // CURBE data start address
        self.commands.push((data_addr >> 32) as u32);
    }

    /// Emit MEDIA_INTERFACE_DESCRIPTOR_LOAD — load shader interface descriptor.
    pub fn media_interface_descriptor_load(&mut self, desc_addr: u64, size: u32) {
        let header = (3u32 << 29) | (2u32 << 27)
            | ((MEDIA_INTERFACE_DESCRIPTOR_LOAD as u32) << 16) | 2;
        self.commands.push(header);
        self.commands.push(size); // interface descriptor data length
        self.commands.push(desc_addr as u32);
        self.commands.push((desc_addr >> 32) as u32);
    }

    /// Emit GPGPU_WALKER — dispatch compute workgroups.
    pub fn gpgpu_walker(
        &mut self,
        interface_desc_offset: u32,
        thread_width: u32,
        thread_height: u32,
        thread_depth: u32,
        group_x: u32,
        group_y: u32,
        group_z: u32,
    ) {
        // GPGPU_WALKER: type=3, subtype=2 (media), opcode=0x05
        // Length = 15 - 2 = 13
        let header = (3u32 << 29) | (2u32 << 27)
            | ((GPGPU_WALKER as u32) << 16) | 13;
        self.commands.push(header);
        self.commands.push(interface_desc_offset); // interface descriptor offset
        self.commands.push(0); // indirect data length (0 = use inline)
        self.commands.push(0); // indirect data start address
        // Thread dimensions (SIMD size encoding)
        let simd_size = if thread_width >= 32 { 2u32 } else if thread_width >= 16 { 1u32 } else { 0u32 };
        self.commands.push(simd_size); // SIMD size: 0=SIMD8, 1=SIMD16, 2=SIMD32
        self.commands.push(thread_width); // thread width count in execution mask
        self.commands.push(thread_height); // thread height
        self.commands.push(thread_depth); // thread depth
        // Group start / end
        self.commands.push(0); // group ID start X
        self.commands.push(0); // reserved
        self.commands.push(group_x); // group ID end X (exclusive)
        self.commands.push(group_y); // group ID end Y
        self.commands.push(group_z); // group ID end Z
        // Right / bottom execution masks
        self.commands.push(0xFFFF_FFFF); // right execution mask
        self.commands.push(0xFFFF_FFFF); // bottom execution mask
    }

    /// Return current size in dwords.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Finalize and return the command stream.
    pub fn build(self) -> Vec<u32> {
        self.commands
    }
}

// ---------------------------------------------------------------------------
// 4. Render Ring
// ---------------------------------------------------------------------------

/// Render ring buffer for submitting batch buffers to the GPU.
pub struct RenderRing {
    ring_buffer: GpuAlloc,
    ring_size: usize,
    head: u32,
    tail: u32,
    active: bool,
}

impl RenderRing {
    /// Initialize the render ring by allocating a ring buffer.
    fn init_ring(mem: &mut GpuMemory) -> Result<Self, &'static str> {
        let ring_bytes = RING_SIZE_DWORDS * 4;
        let ring_buffer = mem.alloc(ring_bytes, "render_ring")?;

        Ok(Self {
            ring_buffer,
            ring_size: RING_SIZE_DWORDS,
            head: 0,
            tail: 0,
            active: true,
        })
    }

    /// Submit a batch buffer's commands to the ring.
    fn submit(
        &mut self,
        mem: &mut GpuMemory,
        commands: &[u32],
        bar0_virt: u64,
        simulated: bool,
    ) -> u32 {
        if !self.active || commands.is_empty() {
            return self.tail;
        }

        let ring_mask = (self.ring_size as u32) - 1;

        // Write commands into ring buffer
        for (i, &cmd) in commands.iter().enumerate() {
            let ring_offset = ((self.tail + i as u32) & ring_mask) as usize * 4;
            let bytes = cmd.to_le_bytes();
            mem.write(&self.ring_buffer, ring_offset, &bytes);
        }

        // Advance tail pointer
        self.tail = (self.tail + commands.len() as u32) & ring_mask;

        if !simulated && bar0_virt != 0 {
            // Notify GPU: write tail to ring tail register
            unsafe {
                let ptr = (bar0_virt + (RENDER_RING_BASE + RING_TAIL) as u64) as *mut u32;
                core::ptr::write_volatile(ptr, self.tail * 4); // byte offset
            }
        }

        self.tail
    }

    /// Wait for the ring to become idle (head catches up to tail).
    fn wait_idle(&self, bar0_virt: u64, simulated: bool) -> bool {
        if simulated {
            return true;
        }
        if bar0_virt == 0 {
            return true;
        }

        for _ in 0..1_000_000u32 {
            let head = unsafe {
                let ptr = (bar0_virt + (RENDER_RING_BASE + RING_HEAD) as u64) as *const u32;
                core::ptr::read_volatile(ptr)
            };
            // Head is in bytes, tail stored as dwords
            if head / 4 == self.tail {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }
}

// ---------------------------------------------------------------------------
// 5. Compute Dispatch
// ---------------------------------------------------------------------------

/// Create a test/NOP compute shader.
/// In a real system, this would be pre-compiled Gen9 EU ISA.
/// For now, generates a minimal valid shader (NOP + END).
fn create_compute_shader() -> Vec<u32> {
    // Gen9 EU ISA: a minimal shader that just returns.
    // Real compute shaders would load data, do math, store results.
    alloc::vec![
        0x00000000, // NOP
        0x00800031, // send (end of thread)
        0x00000000, // padding
        0x00000000, // padding
    ]
}

/// Dispatch compute shader on Intel GPU.
/// Uses GPGPU_WALKER command in batch buffer.
pub fn dispatch_compute(
    state: &mut ComputeState,
    shader_addr: u64,
    thread_group_x: u32,
    thread_group_y: u32,
    thread_group_z: u32,
    _args: &[u32],
) -> Result<u32, &'static str> {
    let ring = state.ring.as_mut().ok_or("render ring not initialized")?;

    let mut batch = BatchBuffer::new();

    // Set up VFE state (max threads = EU count * 7 threads/EU for Gen9)
    let max_threads = state.eu_count * 7;
    batch.media_vfe_state(max_threads, 1);

    // Dispatch compute workgroups
    batch.gpgpu_walker(
        0, // interface descriptor offset
        8, // thread width (SIMD8)
        1, // thread height
        1, // thread depth
        thread_group_x,
        thread_group_y,
        thread_group_z,
    );

    // Flush and end
    batch.pipe_control(0x0010_0000); // CS stall
    batch.mi_batch_buffer_end();

    // Pad to even number of dwords (HW requirement)
    if batch.len() & 1 != 0 {
        batch.mi_noop();
    }

    let cmds = batch.build();
    let tail = ring.submit(&mut state.gpu_memory, &cmds, state.bar0_virt, state.simulated);

    state.dispatch_count += 1;

    // In simulated mode, fence always marks batch as "addr" for tracking
    let _ = shader_addr;

    Ok(tail)
}

/// High-level matrix multiply dispatch: C = A * B.
/// A is M x K, B is K x N, C is M x N. All buffers in GPU memory.
pub fn gpu_matmul(
    state: &mut ComputeState,
    _a: &GpuAlloc,
    _b: &GpuAlloc,
    _c: &GpuAlloc,
    m: u32,
    n: u32,
    k: u32,
) -> Result<u32, &'static str> {
    if state.shader_code.is_empty() {
        return Err("no compute shader loaded");
    }

    // Calculate workgroup grid: one group per output tile
    let wg_size = 8u32; // SIMD8
    let groups_x = (n + wg_size - 1) / wg_size;
    let groups_y = m;

    let shader_alloc = state.gpu_memory.alloc(
        state.shader_code.len() * 4,
        "matmul_shader",
    )?;

    // Upload shader code
    let mut bytes = Vec::with_capacity(state.shader_code.len() * 4);
    for &dword in &state.shader_code {
        bytes.extend_from_slice(&dword.to_le_bytes());
    }
    state.gpu_memory.write(&shader_alloc, 0, &bytes);

    let result = dispatch_compute(
        state,
        shader_alloc.gpu_addr,
        groups_x, groups_y, 1,
        &[],
    );

    // Track compute stats
    let flops = 2 * (m as u64) * (n as u64) * (k as u64);
    state.total_flops += flops;

    state.gpu_memory.free(shader_alloc.id);
    result
}

/// INT8 quantized matrix multiply: C = A(int8) * B(int8).
pub fn gpu_matmul_int8(
    state: &mut ComputeState,
    _a: &GpuAlloc,
    _b: &GpuAlloc,
    _c: &GpuAlloc,
    m: u32,
    n: u32,
    k: u32,
) -> Result<u32, &'static str> {
    if state.shader_code.is_empty() {
        return Err("no compute shader loaded");
    }

    let wg_size = 8u32;
    let groups_x = (n + wg_size - 1) / wg_size;
    let groups_y = m;

    let shader_alloc = state.gpu_memory.alloc(
        state.shader_code.len() * 4,
        "matmul_int8_shader",
    )?;

    let mut bytes = Vec::with_capacity(state.shader_code.len() * 4);
    for &dword in &state.shader_code {
        bytes.extend_from_slice(&dword.to_le_bytes());
    }
    state.gpu_memory.write(&shader_alloc, 0, &bytes);

    let result = dispatch_compute(
        state,
        shader_alloc.gpu_addr,
        groups_x, groups_y, 1,
        &[],
    );

    // INT8: 2 ops per multiply-accumulate
    let ops = 2 * (m as u64) * (n as u64) * (k as u64);
    state.total_flops += ops;

    state.gpu_memory.free(shader_alloc.id);
    result
}

// ---------------------------------------------------------------------------
// 6. Inference API
// ---------------------------------------------------------------------------

/// Upload model weights to GPU memory.
pub fn upload_weights_inner(
    state: &mut ComputeState,
    name: &str,
    data: &[u8],
) -> Result<GpuAlloc, &'static str> {
    let alloc = state.gpu_memory.alloc(data.len(), name)?;
    state.gpu_memory.write(&alloc, 0, data);
    Ok(alloc)
}

/// Benchmark: run N matmul dispatches of given size and report throughput.
fn gpu_benchmark_inner(state: &mut ComputeState, size: u32, iterations: u32) -> String {
    let elem_bytes = (size * size) as usize;

    let a = match state.gpu_memory.alloc(elem_bytes, "bench_A") {
        Ok(a) => a,
        Err(e) => return format!("Benchmark failed: {}", e),
    };
    let b = match state.gpu_memory.alloc(elem_bytes, "bench_B") {
        Ok(b) => b,
        Err(e) => {
            state.gpu_memory.free(a.id);
            return format!("Benchmark failed: {}", e);
        }
    };
    let c = match state.gpu_memory.alloc(elem_bytes, "bench_C") {
        Ok(c) => c,
        Err(e) => {
            state.gpu_memory.free(a.id);
            state.gpu_memory.free(b.id);
            return format!("Benchmark failed: {}", e);
        }
    };

    let start_dispatches = state.dispatch_count;

    for _ in 0..iterations {
        let _ = gpu_matmul(state, &a, &b, &c, size, size, size);
    }

    let dispatches = state.dispatch_count - start_dispatches;
    let total_ops = 2 * (size as u64) * (size as u64) * (size as u64) * (iterations as u64);

    state.gpu_memory.free(a.id);
    state.gpu_memory.free(b.id);
    state.gpu_memory.free(c.id);

    let mode_str = if state.simulated { "SIMULATED" } else { "HARDWARE" };

    format!(
        "Intel GPU Compute Benchmark ({})\n\
         Matrix size: {}x{}\n\
         Iterations: {}\n\
         Dispatches: {}\n\
         Total ops: {}\n\
         EU count: {}\n\
         Mode: {}",
        mode_str, size, size, iterations, dispatches, total_ops,
        state.eu_count, mode_str,
    )
}

// ---------------------------------------------------------------------------
// 7. Global State & API
// ---------------------------------------------------------------------------

/// Global Intel GPU compute driver state.
pub struct ComputeState {
    pub gpu_memory: GpuMemory,
    pub ring: Option<RenderRing>,
    ggtt: Option<GgttManager>,
    pub shader_code: Vec<u32>,
    pub initialized: bool,
    pub simulated: bool,
    pub dispatch_count: u64,
    pub total_flops: u64,
    pub eu_count: u32,
    bar0_virt: u64,
}

static COMPUTE: Mutex<Option<ComputeState>> = Mutex::new(None);

/// Initialize the Intel GPU compute engine.
/// If no real Intel GPU is detected, runs in simulation mode.
pub fn init() {
    let detected = crate::intel_gpu::is_detected();

    let (vram_base, vram_size, bar0_virt, eu_count, simulated) = if detected {
        // Intel iGPUs use stolen memory, not dedicated VRAM.
        // Full HW init requires GuC firmware loading, so start in simulation.
        crate::serial_println!(
            "[intel_gpu_compute] Intel GPU detected but using simulation mode (no firmware)"
        );
        (0u64, SIM_VRAM_SIZE as u64, 0u64, 24u32, true)
    } else {
        crate::serial_println!(
            "[intel_gpu_compute] no Intel GPU detected, using simulation mode"
        );
        (0u64, SIM_VRAM_SIZE as u64, 0u64, 24u32, true)
    };

    let mut gpu_memory = GpuMemory::new(vram_base, vram_size, simulated);

    // Initialize render ring
    let ring = match RenderRing::init_ring(&mut gpu_memory) {
        Ok(r) => {
            crate::serial_println!(
                "[intel_gpu_compute] render ring initialized (ring: {} dwords)",
                RING_SIZE_DWORDS,
            );
            Some(r)
        }
        Err(e) => {
            crate::serial_println!("[intel_gpu_compute] failed to init ring: {}", e);
            None
        }
    };

    // Initialize GGTT
    let ggtt = Some(GgttManager::new(bar0_virt, vram_size, simulated));

    // Load default compute shader
    let shader_code = create_compute_shader();
    crate::serial_println!(
        "[intel_gpu_compute] loaded compute shader ({} dwords)",
        shader_code.len(),
    );

    let state = ComputeState {
        gpu_memory,
        ring,
        ggtt,
        shader_code,
        initialized: true,
        simulated,
        dispatch_count: 0,
        total_flops: 0,
        eu_count,
        bar0_virt,
    };

    *COMPUTE.lock() = Some(state);
    crate::serial_println!(
        "[intel_gpu_compute] compute engine ready (simulated={}, eu_count={})",
        simulated, eu_count,
    );
}

/// Return compute engine status information.
pub fn compute_info() -> String {
    let lock = COMPUTE.lock();
    let state = match lock.as_ref() {
        Some(s) => s,
        None => return String::from("Intel GPU Compute: not initialized"),
    };

    let mode = if state.simulated { "Simulation" } else { "Hardware" };
    let ring_status = match &state.ring {
        Some(r) => {
            if r.active {
                let idle = r.wait_idle(state.bar0_virt, state.simulated);
                if idle { "active (idle)" } else { "active (busy)" }
            } else {
                "inactive"
            }
        }
        None => "not initialized",
    };

    let mut s = format!(
        "Intel GPU Compute Engine\n\
         Mode: {}\n\
         EU count: {}\n\
         Render ring: {}\n\
         Shader loaded: {} dwords\n\
         Dispatches: {}\n\
         Total ops: {}\n",
        mode, state.eu_count, ring_status, state.shader_code.len(),
        state.dispatch_count, state.total_flops,
    );

    s.push_str(&state.gpu_memory.mem_info());
    s.push('\n');

    s
}

/// Return compute statistics.
pub fn compute_stats() -> String {
    let lock = COMPUTE.lock();
    let state = match lock.as_ref() {
        Some(s) => s,
        None => return String::from("Intel GPU Compute: not initialized"),
    };

    let mode = if state.simulated { "SIM" } else { "HW" };
    format!(
        "Intel GPU Compute Stats [{}]\n\
         EU count: {}\n\
         Dispatches: {}\n\
         Total ops: {}\n\
         {}",
        mode, state.eu_count, state.dispatch_count, state.total_flops,
        state.gpu_memory.mem_info(),
    )
}

/// Check if the compute engine is available.
pub fn is_available() -> bool {
    let lock = COMPUTE.lock();
    match lock.as_ref() {
        Some(s) => s.initialized,
        None => false,
    }
}

/// Upload weights (public API).
pub fn upload_weights(name: &str, data: &[u8]) -> String {
    let mut lock = COMPUTE.lock();
    let state = match lock.as_mut() {
        Some(s) => s,
        None => return String::from("Intel GPU Compute not initialized"),
    };

    match upload_weights_inner(state, name, data) {
        Ok(alloc) => format!(
            "Uploaded '{}' ({} bytes) to GPU memory @ 0x{:X}",
            alloc.name, alloc.size, alloc.gpu_addr,
        ),
        Err(e) => format!("Upload failed: {}", e),
    }
}

/// Run GPU compute benchmark (public API).
pub fn benchmark(size_str: &str) -> String {
    let (size, iterations) = match size_str.trim().split_once(' ') {
        Some((s, i)) => {
            let sz: u32 = match s.parse() {
                Ok(n) if n > 0 && n <= 4096 => n,
                _ => return String::from("Usage: intel-gpu-bench <size> [iterations]"),
            };
            let it: u32 = i.parse().unwrap_or(10);
            (sz, it)
        }
        None => {
            let sz: u32 = match size_str.trim().parse() {
                Ok(n) if n > 0 && n <= 4096 => n,
                _ => 64,
            };
            (sz, 10)
        }
    };

    let mut lock = COMPUTE.lock();
    let state = match lock.as_mut() {
        Some(s) => s,
        None => return String::from("Intel GPU Compute not initialized"),
    };

    gpu_benchmark_inner(state, size, iterations)
}

/// Return memory allocation listing.
pub fn vram_info() -> String {
    let lock = COMPUTE.lock();
    let state = match lock.as_ref() {
        Some(s) => s,
        None => return String::from("Intel GPU Compute not initialized"),
    };

    let mut s = state.gpu_memory.mem_info();
    s.push('\n');
    s.push_str(&state.gpu_memory.list_allocations());
    s
}

/// Dispatch a test matmul of given size.
pub fn dispatch_test(size_str: &str) -> String {
    let size: u32 = match size_str.trim().parse() {
        Ok(n) if n > 0 && n <= 4096 => n,
        _ => return String::from("Usage: intel-gpu-dispatch <size> (1-4096)"),
    };

    let mut lock = COMPUTE.lock();
    let state = match lock.as_mut() {
        Some(s) => s,
        None => return String::from("Intel GPU Compute not initialized"),
    };

    let elem_bytes = (size * size) as usize;
    let a = match state.gpu_memory.alloc(elem_bytes, "test_A") {
        Ok(a) => a,
        Err(e) => return format!("Alloc failed: {}", e),
    };
    let b = match state.gpu_memory.alloc(elem_bytes, "test_B") {
        Ok(b) => b,
        Err(e) => {
            state.gpu_memory.free(a.id);
            return format!("Alloc failed: {}", e);
        }
    };
    let c = match state.gpu_memory.alloc(elem_bytes, "test_C") {
        Ok(c) => c,
        Err(e) => {
            state.gpu_memory.free(a.id);
            state.gpu_memory.free(b.id);
            return format!("Alloc failed: {}", e);
        }
    };

    let result = match gpu_matmul(state, &a, &b, &c, size, size, size) {
        Ok(tail) => {
            let ops = 2 * (size as u64) * (size as u64) * (size as u64);
            let mode = if state.simulated { "SIM" } else { "HW" };
            format!(
                "Dispatched {}x{} matmul on Intel GPU [{}]\n\
                 Ring tail: {}\n\
                 Operations: {}\n\
                 EU count: {}",
                size, size, mode, tail, ops, state.eu_count,
            )
        }
        Err(e) => format!("Dispatch failed: {}", e),
    };

    state.gpu_memory.free(a.id);
    state.gpu_memory.free(b.id);
    state.gpu_memory.free(c.id);

    result
}
