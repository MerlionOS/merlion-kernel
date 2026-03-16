/// GPU compute module for MerlionOS.
/// Provides a basic GPU driver (virtio-gpu compatible), GPU memory management,
/// compute shader dispatch, and a software fallback for compute operations.
/// All compute is performed in software (i32 math), simulating what a real
/// GPU would do. When a virtio-gpu device is detected on the PCI bus, the
/// driver will report its capabilities; otherwise we fall back to a pure
/// software compute backend.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// GPU backend enum
// ---------------------------------------------------------------------------

/// Which GPU backend is active.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GpuBackend {
    /// No GPU detected — compute calls will fail.
    None,
    /// A virtio-gpu PCI device was found and initialised.
    VirtioGpu,
    /// Pure software fallback — always available.
    Software,
}

// ---------------------------------------------------------------------------
// Compute operation types
// ---------------------------------------------------------------------------

/// Element-wise map functions applied by `ComputeOp::Map`.
#[derive(Debug, Clone, Copy)]
pub enum MapFn {
    /// Absolute value.
    Abs,
    /// Square each element.
    Square,
    /// Negate each element.
    Negate,
    /// Clamp each element to `[lo, hi]`.
    Clamp(i32, i32),
}

/// Compute operations that can be dispatched to the GPU (or software).
#[derive(Debug, Clone)]
pub enum ComputeOp {
    /// Element-wise C\[i\] = A\[i\] + B\[i\].
    VectorAdd,
    /// Element-wise C\[i\] = A\[i\] * B\[i\].
    VectorMul,
    /// Matrix multiply C = A x B (square matrices, row-major).
    MatMul,
    /// B\[i\] = A\[i\] * scalar.
    ScalarMul(i32),
    /// Reduce (sum) all elements of A into output\[0\].
    Reduce,
    /// Apply a `MapFn` to every element.
    Map(MapFn),
}

// ---------------------------------------------------------------------------
// Job status
// ---------------------------------------------------------------------------

/// Status of a queued compute job.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

// ---------------------------------------------------------------------------
// Internal types (behind the global mutex)
// ---------------------------------------------------------------------------

/// Per-buffer metadata.
struct GpuBuffer {
    id: u32,
    size: usize,
    data: Vec<i32>,
    gpu_addr: u64,
    mapped: bool,
}

/// A compute job in the queue.
struct ComputeJob {
    id: u32,
    op: ComputeOp,
    input_ids: Vec<u32>,
    output_id: u32,
    status: JobStatus,
    submit_tick: u64,
    complete_tick: u64,
    cycles: u64,
}

/// Mutable GPU driver state protected by a spinlock.
struct GpuState {
    backend: GpuBackend,
    vendor: String,
    device_name: String,
    memory_total: usize,
    memory_used: usize,
    compute_units: usize,
    max_workgroup_size: usize,
    buffers: Vec<GpuBuffer>,
    jobs: Vec<ComputeJob>,
}

// ---------------------------------------------------------------------------
// Globals
// ---------------------------------------------------------------------------

static GPU: Mutex<Option<GpuState>> = Mutex::new(None);
static NEXT_BUFFER_ID: AtomicU32 = AtomicU32::new(1);
static NEXT_JOB_ID: AtomicU32 = AtomicU32::new(1);
static NEXT_GPU_ADDR: AtomicU64 = AtomicU64::new(0x8000_0000);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static TOTAL_JOBS_COMPLETED: AtomicU64 = AtomicU64::new(0);
static TOTAL_CYCLES: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the GPU subsystem.
/// Attempts to detect a virtio-gpu device on the PCI bus; falls back to the
/// software compute backend when none is found.
pub fn init() {
    let backend = detect_backend();
    let (vendor, name, units) = match backend {
        GpuBackend::VirtioGpu => (
            "Red Hat / Virtio".to_owned(),
            "virtio-gpu".to_owned(),
            4,
        ),
        GpuBackend::Software => (
            "MerlionOS".to_owned(),
            "Software Compute".to_owned(),
            1,
        ),
        GpuBackend::None => (
            "none".to_owned(),
            "none".to_owned(),
            0,
        ),
    };

    let state = GpuState {
        backend,
        vendor,
        device_name: name,
        memory_total: 16 * 1024 * 1024, // 16 MiB simulated VRAM
        memory_used: 0,
        compute_units: units,
        max_workgroup_size: 256,
        buffers: Vec::new(),
        jobs: Vec::new(),
    };

    *GPU.lock() = Some(state);
    INITIALIZED.store(true, Ordering::SeqCst);

    crate::serial_println!("[gpu] initialised — backend: {:?}", backend);
}

/// Probe for a virtio-gpu PCI device.  Returns `Software` when nothing is
/// found so the system always has a working compute path.
fn detect_backend() -> GpuBackend {
    // virtio-gpu has PCI device ID 0x1050 (modern) or 0x1040+16=0x1050
    #[cfg(not(test))]
    {
        // Walk PCI devices if our PCI module exposes them.
        // For now we always use software — real probing would go here.
    }
    GpuBackend::Software
}

// ---------------------------------------------------------------------------
// Buffer management
// ---------------------------------------------------------------------------

/// Allocate a GPU buffer of `size` elements (i32).  Returns a buffer ID.
pub fn alloc_buffer(size: usize) -> u32 {
    let id = NEXT_BUFFER_ID.fetch_add(1, Ordering::SeqCst);
    let gpu_addr = NEXT_GPU_ADDR.fetch_add(size as u64 * 4, Ordering::SeqCst);
    let byte_size = size * 4;

    let buf = GpuBuffer {
        id,
        size,
        data: alloc::vec![0i32; size],
        gpu_addr,
        mapped: false,
    };

    let mut guard = GPU.lock();
    if let Some(ref mut s) = *guard {
        s.memory_used += byte_size;
        s.buffers.push(buf);
    }
    id
}

/// Free a previously allocated GPU buffer.
pub fn free_buffer(id: u32) {
    let mut guard = GPU.lock();
    if let Some(ref mut s) = *guard {
        if let Some(pos) = s.buffers.iter().position(|b| b.id == id) {
            let byte_size = s.buffers[pos].size * 4;
            s.buffers.remove(pos);
            s.memory_used = s.memory_used.saturating_sub(byte_size);
        }
    }
}

/// Write data into a GPU buffer (CPU -> GPU transfer).
pub fn write_buffer(id: u32, data: &[i32]) {
    let mut guard = GPU.lock();
    if let Some(ref mut s) = *guard {
        if let Some(buf) = s.buffers.iter_mut().find(|b| b.id == id) {
            let len = data.len().min(buf.size);
            buf.data[..len].copy_from_slice(&data[..len]);
        }
    }
}

/// Read data from a GPU buffer (GPU -> CPU transfer).
pub fn read_buffer(id: u32) -> Vec<i32> {
    let guard = GPU.lock();
    if let Some(ref s) = *guard {
        if let Some(buf) = s.buffers.iter().find(|b| b.id == id) {
            return buf.data.clone();
        }
    }
    Vec::new()
}

/// List all allocated GPU buffers.
pub fn list_buffers() -> String {
    let guard = GPU.lock();
    if let Some(ref s) = *guard {
        if s.buffers.is_empty() {
            return "No GPU buffers allocated.\n".to_owned();
        }
        let mut out = format!("{:<6} {:>10} {:>16} {}\n", "ID", "Size", "GPU Addr", "Mapped");
        for b in &s.buffers {
            out += &format!(
                "{:<6} {:>10} 0x{:014x} {}\n",
                b.id, b.size, b.gpu_addr, b.mapped
            );
        }
        out
    } else {
        "GPU not initialised.\n".to_owned()
    }
}

// ---------------------------------------------------------------------------
// Compute dispatch — synchronous
// ---------------------------------------------------------------------------

/// Dispatch a compute operation synchronously.
/// `input_bufs` are the IDs of the input buffers, `output_buf` is the ID of
/// the output buffer.  Returns the number of simulated compute cycles on
/// success.
pub fn dispatch(op: &ComputeOp, input_bufs: &[u32], output_buf: u32) -> Result<u64, &'static str> {
    let mut guard = GPU.lock();
    let s = guard.as_mut().ok_or("GPU not initialised")?;
    if s.backend == GpuBackend::None {
        return Err("no GPU backend available");
    }

    // Helper: find buffer data by id (immutable snapshot).
    let find = |id: u32| -> Option<Vec<i32>> {
        s.buffers.iter().find(|b| b.id == id).map(|b| b.data.clone())
    };

    let cycles: u64;
    let result: Vec<i32>;

    match op {
        ComputeOp::VectorAdd => {
            if input_bufs.len() < 2 {
                return Err("VectorAdd requires 2 input buffers");
            }
            let a = find(input_bufs[0]).ok_or("input buffer A not found")?;
            let b_data = find(input_bufs[1]).ok_or("input buffer B not found")?;
            let len = a.len().min(b_data.len());
            let mut out = alloc::vec![0i32; len];
            for i in 0..len {
                out[i] = a[i].wrapping_add(b_data[i]);
            }
            cycles = len as u64;
            result = out;
        }
        ComputeOp::VectorMul => {
            if input_bufs.len() < 2 {
                return Err("VectorMul requires 2 input buffers");
            }
            let a = find(input_bufs[0]).ok_or("input buffer A not found")?;
            let b_data = find(input_bufs[1]).ok_or("input buffer B not found")?;
            let len = a.len().min(b_data.len());
            let mut out = alloc::vec![0i32; len];
            for i in 0..len {
                out[i] = a[i].wrapping_mul(b_data[i]);
            }
            cycles = len as u64;
            result = out;
        }
        ComputeOp::MatMul => {
            if input_bufs.len() < 2 {
                return Err("MatMul requires 2 input buffers");
            }
            let a = find(input_bufs[0]).ok_or("input buffer A not found")?;
            let b_data = find(input_bufs[1]).ok_or("input buffer B not found")?;
            // Assume square matrices, row-major.
            let n = isqrt(a.len());
            if n * n != a.len() || n * n != b_data.len() {
                return Err("MatMul buffers must be square (n*n elements)");
            }
            let mut out = alloc::vec![0i32; n * n];
            for i in 0..n {
                for j in 0..n {
                    let mut sum: i32 = 0;
                    for k in 0..n {
                        sum = sum.wrapping_add(a[i * n + k].wrapping_mul(b_data[k * n + j]));
                    }
                    out[i * n + j] = sum;
                }
            }
            cycles = (n * n * n) as u64;
            result = out;
        }
        ComputeOp::ScalarMul(scalar) => {
            if input_bufs.is_empty() {
                return Err("ScalarMul requires 1 input buffer");
            }
            let a = find(input_bufs[0]).ok_or("input buffer A not found")?;
            let mut out = alloc::vec![0i32; a.len()];
            for i in 0..a.len() {
                out[i] = a[i].wrapping_mul(*scalar);
            }
            cycles = a.len() as u64;
            result = out;
        }
        ComputeOp::Reduce => {
            if input_bufs.is_empty() {
                return Err("Reduce requires 1 input buffer");
            }
            let a = find(input_bufs[0]).ok_or("input buffer A not found")?;
            let sum: i32 = a.iter().fold(0i32, |acc, &x| acc.wrapping_add(x));
            cycles = a.len() as u64;
            result = alloc::vec![sum];
        }
        ComputeOp::Map(f) => {
            if input_bufs.is_empty() {
                return Err("Map requires 1 input buffer");
            }
            let a = find(input_bufs[0]).ok_or("input buffer A not found")?;
            let mut out = alloc::vec![0i32; a.len()];
            for i in 0..a.len() {
                out[i] = match f {
                    MapFn::Abs => a[i].wrapping_abs(),
                    MapFn::Square => a[i].wrapping_mul(a[i]),
                    MapFn::Negate => a[i].wrapping_neg(),
                    MapFn::Clamp(lo, hi) => {
                        if a[i] < *lo { *lo } else if a[i] > *hi { *hi } else { a[i] }
                    }
                };
            }
            cycles = a.len() as u64;
            result = out;
        }
    }

    // Write result into the output buffer.
    if let Some(buf) = s.buffers.iter_mut().find(|b| b.id == output_buf) {
        let len = result.len().min(buf.size);
        buf.data[..len].copy_from_slice(&result[..len]);
    } else {
        return Err("output buffer not found");
    }

    TOTAL_CYCLES.fetch_add(cycles, Ordering::Relaxed);
    TOTAL_JOBS_COMPLETED.fetch_add(1, Ordering::Relaxed);

    Ok(cycles)
}

// ---------------------------------------------------------------------------
// Compute queue — asynchronous jobs
// ---------------------------------------------------------------------------

/// Submit a compute job to the queue (deferred execution).
/// Returns the job ID.
pub fn submit_job(op: ComputeOp, input_ids: &[u32], output_id: u32) -> u32 {
    let id = NEXT_JOB_ID.fetch_add(1, Ordering::SeqCst);
    let tick = crate::timer::ticks();

    let job = ComputeJob {
        id,
        op,
        input_ids: input_ids.to_vec(),
        output_id,
        status: JobStatus::Queued,
        submit_tick: tick,
        complete_tick: 0,
        cycles: 0,
    };

    let mut guard = GPU.lock();
    if let Some(ref mut s) = *guard {
        s.jobs.push(job);
    }
    id
}

/// Poll the job queue and execute any queued jobs.
/// Returns the number of jobs completed during this call.
pub fn poll_jobs() -> usize {
    // Collect queued job indices first, then execute outside the lock-free
    // section.  Because `dispatch` also takes the GPU lock we need to pull
    // job data out, drop the lock, execute, then re-acquire.
    let mut pending: Vec<(usize, ComputeOp, Vec<u32>, u32)> = Vec::new();

    {
        let mut guard = GPU.lock();
        if let Some(ref mut s) = *guard {
            for (idx, job) in s.jobs.iter_mut().enumerate() {
                if job.status == JobStatus::Queued {
                    job.status = JobStatus::Running;
                    pending.push((idx, job.op.clone(), job.input_ids.clone(), job.output_id));
                }
            }
        }
    }

    let mut completed = 0usize;
    for (idx, op, inputs, output) in &pending {
        let result = dispatch(op, inputs, *output);
        let mut guard = GPU.lock();
        if let Some(ref mut s) = *guard {
            if let Some(job) = s.jobs.get_mut(*idx) {
                match result {
                    Ok(cyc) => {
                        job.status = JobStatus::Completed;
                        job.cycles = cyc;
                        job.complete_tick = crate::timer::ticks();
                        completed += 1;
                    }
                    Err(_) => {
                        job.status = JobStatus::Failed;
                    }
                }
            }
        }
    }
    completed
}

/// Return the status of a specific job.
pub fn job_status(id: u32) -> Option<JobStatus> {
    let guard = GPU.lock();
    if let Some(ref s) = *guard {
        s.jobs.iter().find(|j| j.id == id).map(|j| j.status)
    } else {
        None
    }
}

/// List all jobs and their status.
pub fn list_jobs() -> String {
    let guard = GPU.lock();
    if let Some(ref s) = *guard {
        if s.jobs.is_empty() {
            return "No compute jobs.\n".to_owned();
        }
        let mut out = format!("{:<6} {:<14} {:<12} {:>8}\n", "ID", "Op", "Status", "Cycles");
        for j in &s.jobs {
            let op_name = match &j.op {
                ComputeOp::VectorAdd => "VectorAdd".to_owned(),
                ComputeOp::VectorMul => "VectorMul".to_owned(),
                ComputeOp::MatMul => "MatMul".to_owned(),
                ComputeOp::ScalarMul(v) => format!("ScalarMul({})", v),
                ComputeOp::Reduce => "Reduce".to_owned(),
                ComputeOp::Map(_) => "Map".to_owned(),
            };
            let status = match j.status {
                JobStatus::Queued => "Queued",
                JobStatus::Running => "Running",
                JobStatus::Completed => "Completed",
                JobStatus::Failed => "Failed",
            };
            out += &format!("{:<6} {:<14} {:<12} {:>8}\n", j.id, op_name, status, j.cycles);
        }
        out
    } else {
        "GPU not initialised.\n".to_owned()
    }
}

// ---------------------------------------------------------------------------
// Info and statistics
// ---------------------------------------------------------------------------

/// Return a human-readable summary of the GPU device and its capabilities.
pub fn gpu_info() -> String {
    let guard = GPU.lock();
    if let Some(ref s) = *guard {
        format!(
            "GPU Device Info\n\
             ---------------\n\
             Backend:          {:?}\n\
             Vendor:           {}\n\
             Device:           {}\n\
             VRAM Total:       {} KiB\n\
             VRAM Used:        {} KiB\n\
             Compute Units:    {}\n\
             Max Workgroup:    {}\n\
             Buffers:          {}\n",
            s.backend,
            s.vendor,
            s.device_name,
            s.memory_total / 1024,
            s.memory_used / 1024,
            s.compute_units,
            s.max_workgroup_size,
            s.buffers.len(),
        )
    } else {
        "GPU not initialised.\n".to_owned()
    }
}

/// Return runtime statistics: completed jobs, total cycles, memory.
pub fn gpu_stats() -> String {
    let jobs_done = TOTAL_JOBS_COMPLETED.load(Ordering::Relaxed);
    let cycles = TOTAL_CYCLES.load(Ordering::Relaxed);

    let guard = GPU.lock();
    let (mem_used, mem_total, pending) = if let Some(ref s) = *guard {
        let pending = s.jobs.iter().filter(|j| j.status == JobStatus::Queued || j.status == JobStatus::Running).count();
        (s.memory_used, s.memory_total, pending)
    } else {
        (0, 0, 0)
    };

    format!(
        "GPU Statistics\n\
         ---------------\n\
         Jobs completed:   {}\n\
         Jobs pending:     {}\n\
         Total cycles:     {}\n\
         VRAM used:        {} / {} KiB\n",
        jobs_done,
        pending,
        cycles,
        mem_used / 1024,
        mem_total / 1024,
    )
}

// ---------------------------------------------------------------------------
// Benchmark
// ---------------------------------------------------------------------------

/// Run a small set of compute benchmarks and return a formatted report.
pub fn benchmark() -> String {
    const N: usize = 1024;

    let a_id = alloc_buffer(N);
    let b_id = alloc_buffer(N);
    let c_id = alloc_buffer(N);

    // Fill A and B with simple data.
    let a_data: Vec<i32> = (0..N as i32).collect();
    let b_data: Vec<i32> = (0..N as i32).map(|x| x * 2).collect();
    write_buffer(a_id, &a_data);
    write_buffer(b_id, &b_data);

    let mut report = String::from("GPU Compute Benchmark (1024 elements)\n");
    report += "--------------------------------------\n";

    // Vector Add
    let t0 = crate::timer::ticks();
    let cyc_add = dispatch(&ComputeOp::VectorAdd, &[a_id, b_id], c_id).unwrap_or(0);
    let t1 = crate::timer::ticks();
    report += &format!("VectorAdd:  {:>6} cycles, {} ticks\n", cyc_add, t1 - t0);

    // Vector Mul
    let t0 = crate::timer::ticks();
    let cyc_mul = dispatch(&ComputeOp::VectorMul, &[a_id, b_id], c_id).unwrap_or(0);
    let t1 = crate::timer::ticks();
    report += &format!("VectorMul:  {:>6} cycles, {} ticks\n", cyc_mul, t1 - t0);

    // Scalar Mul
    let t0 = crate::timer::ticks();
    let cyc_smul = dispatch(&ComputeOp::ScalarMul(7), &[a_id], c_id).unwrap_or(0);
    let t1 = crate::timer::ticks();
    report += &format!("ScalarMul:  {:>6} cycles, {} ticks\n", cyc_smul, t1 - t0);

    // Reduce
    let t0 = crate::timer::ticks();
    let cyc_red = dispatch(&ComputeOp::Reduce, &[a_id], c_id).unwrap_or(0);
    let t1 = crate::timer::ticks();
    let reduced = read_buffer(c_id);
    let sum_val = if reduced.is_empty() { 0 } else { reduced[0] };
    report += &format!("Reduce:     {:>6} cycles, {} ticks  (sum = {})\n", cyc_red, t1 - t0, sum_val);

    // MatMul — use a smaller 32x32 matrix to keep it quick.
    free_buffer(a_id);
    free_buffer(b_id);
    free_buffer(c_id);

    const M: usize = 32;
    let ma = alloc_buffer(M * M);
    let mb = alloc_buffer(M * M);
    let mc = alloc_buffer(M * M);
    let ma_data: Vec<i32> = (0..(M * M) as i32).collect();
    let mb_data: Vec<i32> = (0..(M * M) as i32).map(|x| x % 5).collect();
    write_buffer(ma, &ma_data);
    write_buffer(mb, &mb_data);

    let t0 = crate::timer::ticks();
    let cyc_mm = dispatch(&ComputeOp::MatMul, &[ma, mb], mc).unwrap_or(0);
    let t1 = crate::timer::ticks();
    report += &format!("MatMul 32x32: {:>6} cycles, {} ticks\n", cyc_mm, t1 - t0);

    free_buffer(ma);
    free_buffer(mb);
    free_buffer(mc);

    report
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Integer square root (floor).
fn isqrt(n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}
