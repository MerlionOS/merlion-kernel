/// AMD GPU Compute driver for MerlionOS.
/// Provides GPU memory management, compute queue submission,
/// and matrix operation dispatch for AI inference workloads.
/// Targets GCN 4 (Polaris) architecture.
/// Does NOT handle display output — compute only.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Constants — MMIO registers for compute
// ---------------------------------------------------------------------------

// GFX engine status
const MM_GRBM_STATUS: u32 = 0xD010;
const MM_GRBM_STATUS2: u32 = 0xD014;
const MM_CP_STAT: u32 = 0xD048;

// Compute Queue (MEC — Micro Engine Compute)
const MM_CP_HQD_ACTIVE: u32 = 0xF43C;
const MM_CP_HQD_PQ_BASE: u32 = 0xF448;
const MM_CP_HQD_PQ_BASE_HI: u32 = 0xF44C;
const MM_CP_HQD_PQ_RPTR: u32 = 0xF454;
const MM_CP_HQD_PQ_WPTR: u32 = 0xF458;
const MM_CP_HQD_PQ_CONTROL: u32 = 0xF460;

// SDMA (System DMA)
const MM_SDMA0_GFX_RB_BASE: u32 = 0x3480;
const MM_SDMA0_GFX_RB_RPTR: u32 = 0x3484;
const MM_SDMA0_GFX_RB_WPTR: u32 = 0x3488;
const MM_SDMA0_GFX_DOORBELL: u32 = 0x34A0;

// Memory Controller
const MM_MC_VM_FB_LOCATION: u32 = 0x80C;
const MM_MC_VM_FB_OFFSET: u32 = 0x810;
const MM_CONFIG_MEMSIZE: u32 = 0x150A;

// MM_INDEX / MM_DATA for indexed register access
const MM_INDEX: u32 = 0x0000;
const MM_DATA: u32 = 0x0004;

// Indexed access threshold (256KB)
const DIRECT_MMIO_LIMIT: u32 = 0x4_0000;

// PM4 packet constants
const PM4_TYPE3: u32 = 3 << 30;
const PM4_OP_NOP: u32 = 0x10;
const PM4_OP_SET_SH_REG: u32 = 0x76;
const PM4_OP_DISPATCH_DIRECT: u32 = 0x15;
const PM4_OP_WRITE_DATA: u32 = 0x37;
const PM4_OP_WAIT_REG_MEM: u32 = 0x3C;
const PM4_OP_RELEASE_MEM: u32 = 0x49;
const PM4_OP_ACQUIRE_MEM: u32 = 0x58;
const PM4_OP_DMA_DATA: u32 = 0x50;

// Compute shader register base (SH register space)
const SH_REG_BASE: u32 = 0x2C00;
const COMPUTE_PGM_LO: u32 = 0x2E0C;
const COMPUTE_PGM_HI: u32 = 0x2E10;
const COMPUTE_PGM_RSRC1: u32 = 0x2E14;
const COMPUTE_PGM_RSRC2: u32 = 0x2E18;
const COMPUTE_NUM_THREAD_X: u32 = 0x2E20;
const COMPUTE_NUM_THREAD_Y: u32 = 0x2E24;
const COMPUTE_NUM_THREAD_Z: u32 = 0x2E28;

// Simulation limits
const SIM_VRAM_SIZE: usize = 64 * 1024; // 64 KB simulated VRAM (save heap)
const RING_SIZE_DWORDS: usize = 8192;

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

/// GPU memory allocator using a simple bump allocator over VRAM.
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

    /// Bump-allocate `size` bytes from VRAM. Returns the allocation descriptor.
    pub fn alloc(&mut self, size: usize, name: &str) -> Result<GpuAlloc, &'static str> {
        if size == 0 {
            return Err("allocation size must be non-zero");
        }
        // Align to 256 bytes (GPU cacheline alignment)
        let aligned_size = (size + 255) & !255;
        if self.next_free + aligned_size as u64 > self.vram_size {
            return Err("out of VRAM");
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

    /// Write data to VRAM at `alloc.gpu_addr + offset` via BAR2 aperture.
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
            // Real hardware: write through BAR2 aperture
            let base = self.vram_base + addr as u64;
            for (i, &byte) in data.iter().enumerate() {
                unsafe {
                    let ptr = (base + i as u64) as *mut u8;
                    core::ptr::write_volatile(ptr, byte);
                }
            }
        }
    }

    /// Read data from VRAM at `alloc.gpu_addr + offset` via BAR2 aperture.
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
    pub fn list_allocations(&self) -> String {
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

    /// Return VRAM usage summary.
    pub fn vram_info(&self) -> String {
        let used = self.next_free;
        let free = self.vram_size - used;
        let total_kb = self.vram_size / 1024;
        let used_kb = used / 1024;
        let free_kb = free / 1024;
        format!(
            "VRAM: {} KB total, {} KB used, {} KB free ({} allocations)",
            total_kb, used_kb, free_kb, self.allocations.len(),
        )
    }
}

// ---------------------------------------------------------------------------
// 2. MMIO Register Access
// ---------------------------------------------------------------------------

/// Read a GPU MMIO register via BAR0.
/// For offsets < 256KB, uses direct access.
/// For larger offsets, uses MM_INDEX/MM_DATA indexed access.
fn mmio_read(bar0_virt: u64, offset: u32) -> u32 {
    if bar0_virt == 0 {
        return 0;
    }
    if offset < DIRECT_MMIO_LIMIT {
        mmio_read_direct(bar0_virt, offset)
    } else {
        // Indexed access: write offset to MM_INDEX, read from MM_DATA
        mmio_write_direct(bar0_virt, MM_INDEX, offset);
        mmio_read_direct(bar0_virt, MM_DATA)
    }
}

/// Write a GPU MMIO register via BAR0.
fn mmio_write(bar0_virt: u64, offset: u32, val: u32) {
    if bar0_virt == 0 {
        return;
    }
    if offset < DIRECT_MMIO_LIMIT {
        mmio_write_direct(bar0_virt, offset, val);
    } else {
        mmio_write_direct(bar0_virt, MM_INDEX, offset);
        mmio_write_direct(bar0_virt, MM_DATA, val);
    }
}

/// Direct MMIO read within the first 256KB of BAR0.
fn mmio_read_direct(bar0_virt: u64, offset: u32) -> u32 {
    unsafe {
        let ptr = (bar0_virt + offset as u64) as *const u32;
        core::ptr::read_volatile(ptr)
    }
}

/// Direct MMIO write within the first 256KB of BAR0.
fn mmio_write_direct(bar0_virt: u64, offset: u32, val: u32) {
    unsafe {
        let ptr = (bar0_virt + offset as u64) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }
}

/// Read a compute-related register, returning a human-readable dump line.
fn read_compute_reg(bar0_virt: u64, name: &str, offset: u32) -> String {
    let val = mmio_read(bar0_virt, offset);
    format!("  0x{:04X} {:<24} = 0x{:08X}", offset, name, val)
}

// ---------------------------------------------------------------------------
// 3. PM4 Command Packets
// ---------------------------------------------------------------------------

/// Builder for PM4 command packet streams.
pub struct Pm4Builder {
    commands: Vec<u32>,
}

impl Pm4Builder {
    /// Create a new empty PM4 command builder.
    pub fn new() -> Self {
        Self { commands: Vec::new() }
    }

    /// Build a PM4 type-3 header.
    fn type3_header(opcode: u32, count: u32) -> u32 {
        // count is number of DWORDs following the header minus 1
        PM4_TYPE3 | ((opcode & 0xFF) << 8) | ((count - 1) & 0x3FFF)
    }

    /// Insert NOP padding packets.
    pub fn nop(&mut self, count: u32) {
        if count == 0 {
            return;
        }
        self.commands.push(Self::type3_header(PM4_OP_NOP, count));
        for _ in 0..count {
            self.commands.push(0);
        }
    }

    /// Set a shader hardware register (SH register space).
    pub fn set_sh_reg(&mut self, offset: u32, value: u32) {
        // SET_SH_REG: header + reg_offset + value
        self.commands.push(Self::type3_header(PM4_OP_SET_SH_REG, 2));
        self.commands.push((offset - SH_REG_BASE) >> 2);
        self.commands.push(value);
    }

    /// Dispatch compute workgroups.
    pub fn dispatch(&mut self, group_x: u32, group_y: u32, group_z: u32) {
        // DISPATCH_DIRECT: header + dim_x + dim_y + dim_z + dispatch_initiator
        self.commands.push(Self::type3_header(PM4_OP_DISPATCH_DIRECT, 4));
        self.commands.push(group_x);
        self.commands.push(group_y);
        self.commands.push(group_z);
        self.commands.push(1); // dispatch initiator (compute)
    }

    /// Write a 32-bit value to a GPU memory address.
    pub fn write_data(&mut self, addr: u64, value: u32) {
        // WRITE_DATA: header + control + addr_lo + addr_hi + data
        self.commands.push(Self::type3_header(PM4_OP_WRITE_DATA, 4));
        // control: dst_sel=5 (memory), wr_confirm=1
        self.commands.push((5 << 8) | (1 << 20));
        self.commands.push(addr as u32);
        self.commands.push((addr >> 32) as u32);
        self.commands.push(value);
    }

    /// Wait until a memory/register value matches.
    pub fn wait_reg_mem(&mut self, addr: u64, value: u32, mask: u32) {
        // WAIT_REG_MEM: header + control + addr_lo + addr_hi + ref + mask + poll_interval
        self.commands.push(Self::type3_header(PM4_OP_WAIT_REG_MEM, 6));
        // function=3 (equal), mem_space=1 (memory)
        self.commands.push((3 << 0) | (1 << 4));
        self.commands.push(addr as u32);
        self.commands.push((addr >> 32) as u32);
        self.commands.push(value);
        self.commands.push(mask);
        self.commands.push(10); // poll interval
    }

    /// Release memory event — GPU writes a fence value on completion.
    pub fn release_mem(&mut self, addr: u64, value: u32) {
        // RELEASE_MEM: header + event_type + addr_lo + addr_hi + data_lo + data_hi
        self.commands.push(Self::type3_header(PM4_OP_RELEASE_MEM, 5));
        // event_type: cache flush + EOP + write data
        self.commands.push((0x28 << 0) | (1 << 12) | (1 << 25));
        self.commands.push(addr as u32);
        self.commands.push((addr >> 32) as u32);
        self.commands.push(value);
        self.commands.push(0);
    }

    /// DMA copy between two GPU addresses.
    pub fn dma_copy(&mut self, src: u64, dst: u64, size: u32) {
        // DMA_DATA: header + control + src_lo + src_hi + dst_lo + dst_hi + size
        self.commands.push(Self::type3_header(PM4_OP_DMA_DATA, 6));
        // control: src_sel=0 (addr), dst_sel=0 (addr)
        self.commands.push(0);
        self.commands.push(src as u32);
        self.commands.push((src >> 32) as u32);
        self.commands.push(dst as u32);
        self.commands.push((dst >> 32) as u32);
        self.commands.push(size);
    }

    /// Finalize and return the PM4 command stream.
    pub fn build(self) -> Vec<u32> {
        self.commands
    }

    /// Return current size in dwords.
    pub fn len(&self) -> usize {
        self.commands.len()
    }
}

// ---------------------------------------------------------------------------
// 4. Compute Queue
// ---------------------------------------------------------------------------

/// Ring buffer for submitting PM4 commands to the GPU's compute engine.
pub struct ComputeQueue {
    ring_alloc: GpuAlloc,
    ring_size: usize,
    wptr: u32,
    rptr: u32,
    fence_alloc: GpuAlloc,
    fence_value: u32,
    active: bool,
}

impl ComputeQueue {
    /// Initialize a compute queue by allocating a ring buffer in VRAM.
    fn init(mem: &mut GpuMemory) -> Result<Self, &'static str> {
        let ring_bytes = RING_SIZE_DWORDS * 4;
        let ring_alloc = mem.alloc(ring_bytes, "compute_ring")?;
        let fence_alloc = mem.alloc(256, "compute_fence")?;

        // Zero the fence memory
        let zeros = [0u8; 4];
        mem.write(&fence_alloc, 0, &zeros);

        Ok(Self {
            ring_alloc,
            ring_size: RING_SIZE_DWORDS,
            wptr: 0,
            rptr: 0,
            fence_alloc,
            fence_value: 0,
            active: true,
        })
    }

    /// Submit PM4 commands to the ring buffer. Returns a fence value.
    fn submit(&mut self, mem: &mut GpuMemory, commands: &[u32], bar0_virt: u64, simulated: bool) -> u32 {
        if !self.active || commands.is_empty() {
            return self.fence_value;
        }

        // Check space in ring
        let needed = commands.len() as u32;
        let ring_mask = (self.ring_size as u32) - 1;

        // Write commands into ring buffer
        for (i, &cmd) in commands.iter().enumerate() {
            let ring_offset = ((self.wptr + i as u32) & ring_mask) as usize * 4;
            let bytes = cmd.to_le_bytes();
            mem.write(&self.ring_alloc, ring_offset, &bytes);
        }

        // Advance write pointer
        self.wptr = (self.wptr + needed) & ring_mask;

        // Increment fence
        self.fence_value += 1;
        let fence = self.fence_value;

        // Write fence value to fence memory
        let fence_bytes = fence.to_le_bytes();
        mem.write(&self.fence_alloc, 0, &fence_bytes);

        if !simulated && bar0_virt != 0 {
            // Notify GPU: write wptr to HQD register
            mmio_write(bar0_virt, MM_CP_HQD_PQ_WPTR, self.wptr);
        }

        // In simulated mode, the fence is immediately "completed"
        fence
    }

    /// Busy-wait until the GPU writes the expected fence value.
    fn wait_fence(&self, mem: &GpuMemory, fence: u32, simulated: bool) -> bool {
        if simulated {
            // Simulation: fence is always ready immediately
            return true;
        }

        // Read fence value from VRAM, poll until it matches
        let mut buf = [0u8; 4];
        for _ in 0..1_000_000u32 {
            mem.read(&self.fence_alloc, 0, &mut buf);
            let current = u32::from_le_bytes(buf);
            if current >= fence {
                return true;
            }
            // Spin
            core::hint::spin_loop();
        }
        false
    }

    /// Check if the GPU has consumed all submitted commands.
    fn is_idle(&self, bar0_virt: u64, simulated: bool) -> bool {
        if simulated {
            return true;
        }
        if bar0_virt == 0 {
            return true;
        }
        let rptr = mmio_read(bar0_virt, MM_CP_HQD_PQ_RPTR);
        rptr == self.wptr
    }
}

// ---------------------------------------------------------------------------
// 5. Compute Shader Dispatch
// ---------------------------------------------------------------------------

/// A pre-compiled compute shader for GPU dispatch.
pub struct ComputeShader {
    pub name: String,
    pub code: Vec<u32>,
    pub num_sgprs: u32,
    pub num_vgprs: u32,
    pub lds_size: u32,
    pub workgroup_size: [u32; 3],
}

/// Shader register state for programming the compute pipeline.
struct ShaderState {
    pgm_lo: u32,
    pgm_hi: u32,
    pgm_rsrc1: u32,
    pgm_rsrc2: u32,
    num_thread_x: u32,
    num_thread_y: u32,
    num_thread_z: u32,
}

/// Arguments for a compute shader dispatch.
pub struct ShaderArgs {
    pub buffer_addrs: [u64; 4],
    pub params: [u32; 8],
}

/// Create a test/NOP matrix multiply shader.
/// In a real system, this would be pre-compiled GCN ISA.
/// For now, generates a minimal valid GCN shader (s_endpgm).
fn create_matmul_shader() -> ComputeShader {
    // GCN ISA: s_endpgm = 0xBF810000
    // A real matmul shader would:
    //   1. Load matrix elements from VRAM via buffer descriptors
    //   2. Multiply using v_mul_i32 / v_mad_i32
    //   3. Store results back
    // For testing, we use a minimal shader that just exits.
    let code = alloc::vec![
        0xBF810000, // s_endpgm
        0x00000000, // padding (shader must be 256-byte aligned)
        0x00000000,
        0x00000000,
    ];

    ComputeShader {
        name: String::from("matmul_test"),
        code,
        num_sgprs: 8,
        num_vgprs: 4,
        lds_size: 0,
        workgroup_size: [64, 1, 1],
    }
}

/// Create an INT8 quantized matrix multiply shader stub.
fn create_matmul_int8_shader() -> ComputeShader {
    let code = alloc::vec![
        0xBF810000, // s_endpgm
        0x00000000,
        0x00000000,
        0x00000000,
    ];

    ComputeShader {
        name: String::from("matmul_int8"),
        code,
        num_sgprs: 8,
        num_vgprs: 8,
        lds_size: 4096,
        workgroup_size: [64, 1, 1],
    }
}

/// Upload a shader's code to VRAM and return the allocation.
fn upload_shader(mem: &mut GpuMemory, shader: &ComputeShader) -> Result<GpuAlloc, &'static str> {
    let byte_size = shader.code.len() * 4;
    let alloc = mem.alloc(byte_size, &shader.name)?;

    // Convert code to bytes and write to VRAM
    let mut bytes = Vec::with_capacity(byte_size);
    for &dword in &shader.code {
        bytes.extend_from_slice(&dword.to_le_bytes());
    }
    mem.write(&alloc, 0, &bytes);

    Ok(alloc)
}

/// Build PM4 commands to set shader registers for dispatch.
fn build_shader_state(shader: &ComputeShader, code_addr: u64) -> ShaderState {
    let addr_lo = (code_addr >> 8) as u32;
    let addr_hi = (code_addr >> 40) as u32;

    // PGM_RSRC1: encode VGPR and SGPR allocation
    // vgprs: (num/4)-1, sgprs: (num/8)-1
    let vgpr_blocks = if shader.num_vgprs > 0 {
        (shader.num_vgprs / 4).saturating_sub(1)
    } else {
        0
    };
    let sgpr_blocks = if shader.num_sgprs > 0 {
        (shader.num_sgprs / 8).saturating_sub(1)
    } else {
        0
    };
    let pgm_rsrc1 = (vgpr_blocks & 0x3F) | ((sgpr_blocks & 0xF) << 6);

    // PGM_RSRC2: LDS size in 256-byte blocks
    let lds_blocks = shader.lds_size / 256;
    let pgm_rsrc2 = lds_blocks & 0x1FF;

    ShaderState {
        pgm_lo: addr_lo,
        pgm_hi: addr_hi,
        pgm_rsrc1,
        pgm_rsrc2,
        num_thread_x: shader.workgroup_size[0],
        num_thread_y: shader.workgroup_size[1],
        num_thread_z: shader.workgroup_size[2],
    }
}

/// Dispatch a compute shader on the GPU. Returns a fence value to wait on.
fn dispatch_shader(
    queue: &mut ComputeQueue,
    mem: &mut GpuMemory,
    shader: &ComputeShader,
    code_alloc: &GpuAlloc,
    groups: [u32; 3],
    bar0_virt: u64,
    simulated: bool,
) -> u32 {
    let state = build_shader_state(shader, code_alloc.gpu_addr);

    let mut pm4 = Pm4Builder::new();

    // Set shader program address
    pm4.set_sh_reg(COMPUTE_PGM_LO, state.pgm_lo);
    pm4.set_sh_reg(COMPUTE_PGM_HI, state.pgm_hi);

    // Set resource descriptors
    pm4.set_sh_reg(COMPUTE_PGM_RSRC1, state.pgm_rsrc1);
    pm4.set_sh_reg(COMPUTE_PGM_RSRC2, state.pgm_rsrc2);

    // Set workgroup dimensions
    pm4.set_sh_reg(COMPUTE_NUM_THREAD_X, state.num_thread_x);
    pm4.set_sh_reg(COMPUTE_NUM_THREAD_Y, state.num_thread_y);
    pm4.set_sh_reg(COMPUTE_NUM_THREAD_Z, state.num_thread_z);

    // Dispatch
    pm4.dispatch(groups[0], groups[1], groups[2]);

    // Fence — release_mem to write fence value on completion
    let fence_addr = queue.fence_alloc.gpu_addr;
    let fence_val = queue.fence_value + 1;
    pm4.release_mem(fence_addr, fence_val);

    let cmds = pm4.build();
    queue.submit(mem, &cmds, bar0_virt, simulated)
}

/// High-level: dispatch a matrix multiply (C = A * B).
/// A is M x K, B is K x N, C is M x N.
fn dispatch_matmul(
    queue: &mut ComputeQueue,
    mem: &mut GpuMemory,
    shader: &ComputeShader,
    code_alloc: &GpuAlloc,
    _a_addr: u64,
    _b_addr: u64,
    _c_addr: u64,
    m: u32,
    n: u32,
    _k: u32,
    bar0_virt: u64,
    simulated: bool,
) -> u32 {
    // Calculate workgroup grid dimensions
    let wg_x = shader.workgroup_size[0];
    let groups_x = (n + wg_x - 1) / wg_x;
    let groups_y = m;
    let groups_z = 1;

    dispatch_shader(queue, mem, shader, code_alloc, [groups_x, groups_y, groups_z], bar0_virt, simulated)
}

// ---------------------------------------------------------------------------
// 6. DMA Engine (SDMA)
// ---------------------------------------------------------------------------

/// Copy data from CPU memory to a GPU allocation using DMA.
/// In simulation mode, this is a direct memcpy.
fn dma_cpu_to_gpu(
    mem: &mut GpuMemory,
    cpu_data: &[u8],
    gpu_alloc: &GpuAlloc,
    offset: usize,
) -> Result<(), &'static str> {
    if offset + cpu_data.len() > gpu_alloc.size {
        return Err("DMA transfer exceeds allocation size");
    }
    // For both simulated and real hardware, we use BAR2 CPU writes
    // (real SDMA would use ring buffer submission, but BAR2 works for small transfers)
    mem.write(gpu_alloc, offset, cpu_data);
    Ok(())
}

/// Copy data from a GPU allocation to CPU memory.
fn dma_gpu_to_cpu(
    mem: &GpuMemory,
    gpu_alloc: &GpuAlloc,
    offset: usize,
    cpu_buf: &mut [u8],
) -> Result<(), &'static str> {
    if offset + cpu_buf.len() > gpu_alloc.size {
        return Err("DMA transfer exceeds allocation size");
    }
    mem.read(gpu_alloc, offset, cpu_buf);
    Ok(())
}

/// Copy data between two GPU allocations.
/// In real mode, uses PM4 DMA_DATA packet. In simulation, memcpy via temp buffer.
fn dma_gpu_to_gpu(
    mem: &mut GpuMemory,
    queue: &mut ComputeQueue,
    src: &GpuAlloc,
    dst: &GpuAlloc,
    size: usize,
    bar0_virt: u64,
    simulated: bool,
) -> Result<u32, &'static str> {
    if size > src.size || size > dst.size {
        return Err("DMA size exceeds allocation");
    }

    if simulated {
        // Simulate by reading src into temp buffer, then writing to dst
        let mut tmp = Vec::new();
        tmp.resize(size, 0u8);
        mem.read(src, 0, &mut tmp);
        mem.write(dst, 0, &tmp);
        Ok(queue.fence_value)
    } else {
        // Build PM4 DMA_DATA command
        let mut pm4 = Pm4Builder::new();
        pm4.dma_copy(src.gpu_addr, dst.gpu_addr, size as u32);
        let fence_addr = queue.fence_alloc.gpu_addr;
        let fence_val = queue.fence_value + 1;
        pm4.release_mem(fence_addr, fence_val);
        let cmds = pm4.build();
        Ok(queue.submit(mem, &cmds, bar0_virt, simulated))
    }
}

// ---------------------------------------------------------------------------
// 7. Inference Integration
// ---------------------------------------------------------------------------

/// Upload model weights to VRAM.
fn upload_weights(state: &mut ComputeState, name: &str, data: &[u8]) -> Result<GpuAlloc, &'static str> {
    let alloc = state.gpu_memory.alloc(data.len(), name)?;
    dma_cpu_to_gpu(&mut state.gpu_memory, data, &alloc, 0)?;
    Ok(alloc)
}

/// Run matrix multiply on GPU: C = A * B.
/// A(m,k), B(k,n), C(m,n). All matrices in VRAM.
fn gpu_matmul(state: &mut ComputeState, a: &GpuAlloc, b: &GpuAlloc, c: &GpuAlloc, m: u32, n: u32, k: u32) -> Result<u32, &'static str> {
    let queue = state.queue.as_mut().ok_or("compute queue not initialized")?;
    if state.shaders.is_empty() {
        return Err("no shaders loaded");
    }

    // Use first shader (matmul)
    let shader_alloc = state.gpu_memory.alloc(state.shaders[0].code.len() * 4, "shader_code")?;
    let mut bytes = Vec::new();
    for &dword in &state.shaders[0].code {
        bytes.extend_from_slice(&dword.to_le_bytes());
    }
    state.gpu_memory.write(&shader_alloc, 0, &bytes);

    let fence = dispatch_matmul(
        queue, &mut state.gpu_memory, &state.shaders[0], &shader_alloc,
        a.gpu_addr, b.gpu_addr, c.gpu_addr, m, n, k,
        state.bar0_virt, state.simulated,
    );

    // Track stats
    let flops = 2 * (m as u64) * (n as u64) * (k as u64);
    state.dispatch_count += 1;
    state.total_flops += flops;

    state.gpu_memory.free(shader_alloc.id);
    Ok(fence)
}

/// Run quantized INT8 matrix multiply: C = A(int8) * B(int8).
fn gpu_matmul_int8(state: &mut ComputeState, a: &GpuAlloc, b: &GpuAlloc, c: &GpuAlloc, m: u32, n: u32, k: u32) -> Result<u32, &'static str> {
    let queue = state.queue.as_mut().ok_or("compute queue not initialized")?;
    if state.shaders.len() < 2 {
        return Err("INT8 shader not loaded");
    }

    let shader_alloc = state.gpu_memory.alloc(state.shaders[1].code.len() * 4, "int8_shader")?;
    let mut bytes = Vec::new();
    for &dword in &state.shaders[1].code {
        bytes.extend_from_slice(&dword.to_le_bytes());
    }
    state.gpu_memory.write(&shader_alloc, 0, &bytes);

    let fence = dispatch_matmul(
        queue, &mut state.gpu_memory, &state.shaders[1], &shader_alloc,
        a.gpu_addr, b.gpu_addr, c.gpu_addr, m, n, k,
        state.bar0_virt, state.simulated,
    );

    let flops = 2 * (m as u64) * (n as u64) * (k as u64);
    state.dispatch_count += 1;
    state.total_flops += flops;

    state.gpu_memory.free(shader_alloc.id);
    Ok(fence)
}

/// Benchmark: run N matmul dispatches of given size and report throughput.
fn gpu_benchmark_inner(state: &mut ComputeState, size: u32, iterations: u32) -> String {
    let elem_bytes = (size * size) as usize;

    // Allocate A, B, C matrices
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

    // Clean up
    state.gpu_memory.free(a.id);
    state.gpu_memory.free(b.id);
    state.gpu_memory.free(c.id);

    let mode_str = if state.simulated { "SIMULATED" } else { "HARDWARE" };

    format!(
        "GPU Compute Benchmark ({})\n\
         Matrix size: {}x{}\n\
         Iterations: {}\n\
         Dispatches: {}\n\
         Total ops: {}\n\
         Mode: {}",
        mode_str, size, size, iterations, dispatches, total_ops, mode_str,
    )
}

// ---------------------------------------------------------------------------
// 8. Global State & API
// ---------------------------------------------------------------------------

/// Global compute driver state.
pub struct ComputeState {
    pub gpu_memory: GpuMemory,
    pub queue: Option<ComputeQueue>,
    pub shaders: Vec<ComputeShader>,
    pub initialized: bool,
    pub simulated: bool,
    pub dispatch_count: u64,
    pub total_flops: u64,
    bar0_virt: u64,
}

static COMPUTE: Mutex<Option<ComputeState>> = Mutex::new(None);

/// Initialize the AMD GPU compute engine.
/// If no real AMD GPU is detected, runs in simulation mode.
pub fn init() {
    let detected = crate::amdgpu::is_detected();

    let (vram_base, vram_size, bar0_virt, simulated) = if detected {
        // Get GPU info from the amdgpu detection module
        let info_str = crate::amdgpu::amdgpu_info();
        let _ = info_str; // we use atomics from amdgpu module

        // Read BAR addresses from the amdgpu module's stored state
        // BAR2 is the VRAM aperture; BAR0 is MMIO registers
        // In a real system, we would use the detected BAR2 address and size.
        // For safety, we start in simulated mode even with hardware present,
        // since full HW init requires firmware loading.
        let sim_size = SIM_VRAM_SIZE as u64;
        crate::serial_println!("[amdgpu_compute] GPU detected but using simulation mode (no firmware)");
        (0u64, sim_size, 0u64, true)
    } else {
        crate::serial_println!("[amdgpu_compute] no GPU detected, using simulation mode");
        (0u64, SIM_VRAM_SIZE as u64, 0u64, true)
    };

    let mut gpu_memory = GpuMemory::new(vram_base, vram_size, simulated);

    // Initialize compute queue
    let queue = match ComputeQueue::init(&mut gpu_memory) {
        Ok(q) => {
            crate::serial_println!("[amdgpu_compute] compute queue initialized (ring: {} dwords)", RING_SIZE_DWORDS);
            Some(q)
        }
        Err(e) => {
            crate::serial_println!("[amdgpu_compute] failed to init queue: {}", e);
            None
        }
    };

    // Load default shaders
    let mut shaders = Vec::new();
    shaders.push(create_matmul_shader());
    shaders.push(create_matmul_int8_shader());
    crate::serial_println!("[amdgpu_compute] loaded {} compute shaders", shaders.len());

    let state = ComputeState {
        gpu_memory,
        queue,
        shaders,
        initialized: true,
        simulated,
        dispatch_count: 0,
        total_flops: 0,
        bar0_virt,
    };

    *COMPUTE.lock() = Some(state);
    crate::serial_println!("[amdgpu_compute] compute engine ready (simulated={})", simulated);
}

/// Return compute engine status information.
pub fn compute_info() -> String {
    let lock = COMPUTE.lock();
    let state = match lock.as_ref() {
        Some(s) => s,
        None => return String::from("AMD GPU Compute: not initialized"),
    };

    let mode = if state.simulated { "Simulation" } else { "Hardware" };
    let queue_status = match &state.queue {
        Some(q) => {
            let idle = q.is_idle(state.bar0_virt, state.simulated);
            if q.active {
                if idle { "active (idle)" } else { "active (busy)" }
            } else {
                "inactive"
            }
        }
        None => "not initialized",
    };

    let mut s = format!(
        "AMD GPU Compute Engine\n\
         Mode: {}\n\
         Queue: {}\n\
         Shaders loaded: {}\n\
         Dispatches: {}\n\
         Total FLOPs: {}\n",
        mode, queue_status, state.shaders.len(),
        state.dispatch_count, state.total_flops,
    );

    s.push_str(&state.gpu_memory.vram_info());
    s.push('\n');

    // List loaded shaders
    if !state.shaders.is_empty() {
        s.push_str("Shaders:\n");
        for shader in &state.shaders {
            s.push_str(&format!(
                "  {} (sgpr={}, vgpr={}, lds={}, wg=[{},{},{}])\n",
                shader.name, shader.num_sgprs, shader.num_vgprs,
                shader.lds_size,
                shader.workgroup_size[0], shader.workgroup_size[1], shader.workgroup_size[2],
            ));
        }
    }

    s
}

/// Return compute statistics.
pub fn compute_stats() -> String {
    let lock = COMPUTE.lock();
    let state = match lock.as_ref() {
        Some(s) => s,
        None => return String::from("AMD GPU Compute: not initialized"),
    };

    let mode = if state.simulated { "SIM" } else { "HW" };
    format!(
        "GPU Compute Stats [{}]\n\
         Dispatches: {}\n\
         Total ops: {}\n\
         {}",
        mode, state.dispatch_count, state.total_flops,
        state.gpu_memory.vram_info(),
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

// ---------------------------------------------------------------------------
// 9. Shell command handlers
// ---------------------------------------------------------------------------

/// VRAM allocation listing for shell.
pub fn vram_info() -> String {
    let lock = COMPUTE.lock();
    let state = match lock.as_ref() {
        Some(s) => s,
        None => return String::from("GPU Compute not initialized"),
    };

    let mut s = state.gpu_memory.vram_info();
    s.push('\n');
    s.push_str(&state.gpu_memory.list_allocations());
    s
}

/// Dispatch a test matmul of given size.
pub fn dispatch_test(size_str: &str) -> String {
    let size: u32 = match size_str.trim().parse() {
        Ok(n) if n > 0 && n <= 4096 => n,
        _ => return String::from("Usage: gpu-dispatch <size> (1-4096)"),
    };

    let mut lock = COMPUTE.lock();
    let state = match lock.as_mut() {
        Some(s) => s,
        None => return String::from("GPU Compute not initialized"),
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
        Ok(fence) => {
            let ops = 2 * (size as u64) * (size as u64) * (size as u64);
            let mode = if state.simulated { "SIM" } else { "HW" };
            format!(
                "Dispatched {}x{} matmul [{}]\n\
                 Fence: {}\n\
                 Operations: {}",
                size, size, mode, fence, ops,
            )
        }
        Err(e) => format!("Dispatch failed: {}", e),
    };

    state.gpu_memory.free(a.id);
    state.gpu_memory.free(b.id);
    state.gpu_memory.free(c.id);

    result
}

/// Run GPU compute benchmark.
pub fn benchmark() -> String {
    let mut lock = COMPUTE.lock();
    let state = match lock.as_mut() {
        Some(s) => s,
        None => return String::from("GPU Compute not initialized"),
    };

    gpu_benchmark_inner(state, 64, 10)
}

/// Test DMA copy operations.
pub fn dma_test() -> String {
    let mut lock = COMPUTE.lock();
    let state = match lock.as_mut() {
        Some(s) => s,
        None => return String::from("GPU Compute not initialized"),
    };

    // Test CPU -> GPU -> CPU roundtrip
    let test_data: &[u8] = b"MerlionOS GPU DMA test pattern 1234567890";
    let alloc = match state.gpu_memory.alloc(256, "dma_test") {
        Ok(a) => a,
        Err(e) => return format!("DMA test alloc failed: {}", e),
    };

    // CPU -> GPU
    if let Err(e) = dma_cpu_to_gpu(&mut state.gpu_memory, test_data, &alloc, 0) {
        state.gpu_memory.free(alloc.id);
        return format!("DMA CPU->GPU failed: {}", e);
    }

    // GPU -> CPU
    let mut readback = alloc::vec![0u8; test_data.len()];
    if let Err(e) = dma_gpu_to_cpu(&state.gpu_memory, &alloc, 0, &mut readback) {
        state.gpu_memory.free(alloc.id);
        return format!("DMA GPU->CPU failed: {}", e);
    }

    let match_ok = readback == test_data;

    // Test GPU -> GPU copy
    let alloc2 = match state.gpu_memory.alloc(256, "dma_test2") {
        Ok(a) => a,
        Err(e) => {
            state.gpu_memory.free(alloc.id);
            return format!("DMA test alloc2 failed: {}", e);
        }
    };

    let queue_present = state.queue.is_some();
    let g2g_result = if queue_present {
        // Split borrow: take queue out temporarily
        let mut queue = state.queue.take().unwrap();
        let r = dma_gpu_to_gpu(
            &mut state.gpu_memory, &mut queue,
            &alloc, &alloc2, test_data.len(),
            state.bar0_virt, state.simulated,
        );
        state.queue = Some(queue);
        r
    } else {
        Err("no queue")
    };

    let g2g_ok = g2g_result.is_ok();

    // Verify GPU->GPU copy
    let mut readback2 = alloc::vec![0u8; test_data.len()];
    let _ = dma_gpu_to_cpu(&state.gpu_memory, &alloc2, 0, &mut readback2);
    let g2g_match = readback2 == test_data;

    state.gpu_memory.free(alloc.id);
    state.gpu_memory.free(alloc2.id);

    let mode = if state.simulated { "SIM" } else { "HW" };
    format!(
        "DMA Test Results [{}]\n\
         CPU->GPU->CPU roundtrip: {}\n\
         Data integrity: {}\n\
         GPU->GPU copy: {}\n\
         GPU->GPU verify: {}",
        mode,
        if match_ok { "PASS" } else { "FAIL" },
        if match_ok { "OK" } else { "MISMATCH" },
        if g2g_ok { "PASS" } else { "FAIL" },
        if g2g_match { "OK" } else { "MISMATCH" },
    )
}
