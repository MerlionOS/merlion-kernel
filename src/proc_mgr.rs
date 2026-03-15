/// Advanced process manager with Copy-on-Write fork for MerlionOS.
/// Provides full process lifecycle: fork with CoW page table cloning,
/// parent-child relationships, resource limits, wait, and cleanup.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

const MAX_PROCESSES: usize = 128;
const DEFAULT_MAX_MEMORY: usize = 64 * 1024 * 1024; // 64 MiB
const DEFAULT_MAX_FDS: usize = 256;
/// Bit 9 (OS-available) marks a Copy-on-Write page table entry.
const COW_FLAG: u64 = 1 << 9;

/// Monotonically increasing PID counter.  PID 0 is reserved for the kernel.
static NEXT_PID: AtomicU32 = AtomicU32::new(1);

/// Allocate a fresh, globally unique process ID.
fn alloc_pid() -> u32 {
    NEXT_PID.fetch_add(1, Ordering::Relaxed)
}

/// Lifecycle state of a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// Ready to be scheduled.
    Ready,
    /// Currently executing on a CPU.
    Running,
    /// Blocked inside `wait()` for a child to exit.
    Waiting,
    /// Terminated with an exit code; not yet reaped by parent.
    Zombie(i32),
    /// Fully reaped — slot can be recycled.
    Dead,
}

/// A contiguous virtual memory region owned by a process.
#[derive(Debug, Clone)]
pub struct MemoryRegion {
    /// Start virtual address (page-aligned).
    pub base: u64,
    /// Length in bytes (multiple of 4 KiB).
    pub len: usize,
    /// True when pages are mapped Copy-on-Write.
    pub cow: bool,
    /// Human-readable label (e.g. "stack", "heap", "code").
    pub label: String,
}

/// Per-process resource limits.
#[derive(Debug, Clone, Copy)]
pub struct ResourceLimits {
    /// Maximum resident memory in bytes.
    pub max_memory: usize,
    /// Maximum number of simultaneously open file descriptors.
    pub max_fds: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self { max_memory: DEFAULT_MAX_MEMORY, max_fds: DEFAULT_MAX_FDS }
    }
}

/// Core process descriptor.
#[derive(Clone)]
pub struct Process {
    /// Process identifier.
    pub pid: u32,
    /// Parent process identifier (0 for init / orphans).
    pub ppid: u32,
    /// Current lifecycle state.
    pub state: ProcessState,
    /// Physical address of the PML4 (level-4 page table root).
    pub page_table_root: u64,
    /// Open file descriptor numbers.
    pub open_fds: Vec<usize>,
    /// Virtual memory regions belonging to this process.
    pub memory_regions: Vec<MemoryRegion>,
    /// Enforced resource limits.
    pub limits: ResourceLimits,
    /// Human-readable name.
    pub name: String,
}

impl Process {
    /// Total mapped memory across all regions, in bytes.
    pub fn total_memory(&self) -> usize {
        self.memory_regions.iter().map(|r| r.len).sum()
    }
    /// Number of currently open file descriptors.
    pub fn fd_count(&self) -> usize {
        self.open_fds.len()
    }
}

/// Global process table: `BTreeMap` keyed by PID, protected by a spin-lock.
struct ProcessTable { map: BTreeMap<u32, Process> }

impl ProcessTable {
    const fn new() -> Self { Self { map: BTreeMap::new() } }
    fn insert(&mut self, proc: Process) -> Result<(), &'static str> {
        if self.map.len() >= MAX_PROCESSES { return Err("process table full"); }
        if self.map.contains_key(&proc.pid) { return Err("duplicate PID"); }
        self.map.insert(proc.pid, proc); Ok(())
    }
    fn get(&self, pid: u32) -> Option<&Process> { self.map.get(&pid) }
    fn get_mut(&mut self, pid: u32) -> Option<&mut Process> { self.map.get_mut(&pid) }
    fn remove(&mut self, pid: u32) -> Option<Process> { self.map.remove(&pid) }
    fn iter(&self) -> impl Iterator<Item = &Process> { self.map.values() }
}

static PROCESS_TABLE: Mutex<ProcessTable> = Mutex::new(ProcessTable::new());

/// Register the kernel idle process as PID 0.  Called once during boot.
pub fn init() {
    let kernel = Process {
        pid: 0, ppid: 0, state: ProcessState::Running,
        page_table_root: 0, open_fds: Vec::new(),
        memory_regions: Vec::new(), limits: ResourceLimits::default(),
        name: String::from("kernel"),
    };
    PROCESS_TABLE.lock().insert(kernel).expect("failed to register kernel process");
}

/// Create a new process (not via fork).  Returns the allocated PID.
pub fn create_process(
    ppid: u32, page_table_root: u64, regions: Vec<MemoryRegion>, name: &str,
) -> Result<u32, &'static str> {
    let pid = alloc_pid();
    let proc = Process {
        pid, ppid, state: ProcessState::Ready, page_table_root,
        open_fds: Vec::new(), memory_regions: regions,
        limits: ResourceLimits::default(), name: String::from(name),
    };
    PROCESS_TABLE.lock().insert(proc)?;
    Ok(pid)
}

/// Fork `parent_pid` with Copy-on-Write semantics.  Page table entries are
/// marked CoW in both parent and child; a write fault triggers a private copy.
/// Open file descriptors are duplicated.  Returns `Ok(child_pid)`.
pub fn fork(parent_pid: u32) -> Result<u32, &'static str> {
    let mut table = PROCESS_TABLE.lock();
    let parent = table.get(&parent_pid).ok_or("parent not found")?;

    let child_pid = alloc_pid();
    let child_root = clone_page_table_cow(parent.page_table_root)?;

    let child_regions: Vec<MemoryRegion> = parent.memory_regions.iter().map(|r| {
        MemoryRegion { base: r.base, len: r.len, cow: true, label: r.label.clone() }
    }).collect();
    let child_fds = parent.open_fds.clone();
    let child_limits = parent.limits;
    let child_name = { let mut n = parent.name.clone(); n.push_str(".child"); n };

    // Mark parent's regions as CoW too (both sides must fault on write).
    let parent_mut = table.get_mut(&parent_pid).ok_or("parent vanished")?;
    for region in parent_mut.memory_regions.iter_mut() { region.cow = true; }
    mark_page_table_cow(parent_mut.page_table_root);

    let child = Process {
        pid: child_pid, ppid: parent_pid, state: ProcessState::Ready,
        page_table_root: child_root, open_fds: child_fds,
        memory_regions: child_regions, limits: child_limits, name: child_name,
    };
    table.insert(child)?;

    crate::serial_println!("[proc_mgr] fork: parent {} -> child {} (CoW)", parent_pid, child_pid);
    Ok(child_pid)
}

/// Clone a PML4, setting user-half writable entries to read-only + CoW flag.
fn clone_page_table_cow(src_root: u64) -> Result<u64, &'static str> {
    let frame = crate::memory::alloc_frame().ok_or("out of frames for child PML4")?;
    let child_root = frame.start_address().as_u64();
    let offset = crate::memory::phys_mem_offset().as_u64();

    unsafe {
        let src_ptr = (src_root + offset) as *const u64;
        let dst_ptr = (child_root + offset) as *mut u64;
        // Copy all 512 PML4 entries.
        core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, 512);
        // Lower 256 entries (user half): clear WRITABLE, set COW_FLAG.
        for i in 0..256 {
            let entry = dst_ptr.add(i).read();
            if entry & 1 != 0 {
                dst_ptr.add(i).write((entry & !(1 << 1)) | COW_FLAG);
            }
        }
    }
    Ok(child_root)
}

/// Re-mark an existing page table's user-half entries as CoW.
fn mark_page_table_cow(root: u64) {
    if root == 0 { return; }
    let offset = crate::memory::phys_mem_offset().as_u64();
    unsafe {
        let ptr = (root + offset) as *mut u64;
        for i in 0..256 {
            let entry = ptr.add(i).read();
            if entry & 1 != 0 {
                ptr.add(i).write((entry & !(1 << 1)) | COW_FLAG);
            }
        }
    }
}

/// Block until any direct child of `parent_pid` exits.
/// Returns `(child_pid, exit_code)` of the reaped child.
pub fn wait(parent_pid: u32) -> Result<(u32, i32), &'static str> {
    loop {
        {
            let mut table = PROCESS_TABLE.lock();
            // Look for a zombie child.
            let zombie: Option<(u32, i32)> = table.iter()
                .find(|p| p.ppid == parent_pid && matches!(p.state, ProcessState::Zombie(_)))
                .map(|p| match p.state { ProcessState::Zombie(c) => (p.pid, c), _ => unreachable!() });

            if let Some((child_pid, code)) = zombie {
                table.remove(child_pid);
                crate::serial_println!("[proc_mgr] wait: reaped child {} (exit {})", child_pid, code);
                return Ok((child_pid, code));
            }
            if !table.iter().any(|p| p.ppid == parent_pid) {
                return Err("no children");
            }
            if let Some(p) = table.get_mut(&parent_pid) {
                p.state = ProcessState::Waiting;
            }
        }
        crate::task::yield_now();
    }
}

/// Return the PID of the process (validates it exists).
pub fn getpid(pid: u32) -> Result<u32, &'static str> {
    PROCESS_TABLE.lock().get(&pid).map(|p| p.pid).ok_or("process not found")
}

/// Return the parent PID of `pid`.
pub fn getppid(pid: u32) -> Result<u32, &'static str> {
    PROCESS_TABLE.lock().get(&pid).map(|p| p.ppid).ok_or("process not found")
}

/// Update resource limits for a process.
pub fn set_limits(pid: u32, limits: ResourceLimits) -> Result<(), &'static str> {
    let mut table = PROCESS_TABLE.lock();
    table.get_mut(&pid).ok_or("process not found")?.limits = limits;
    crate::serial_println!("[proc_mgr] set_limits pid={}: mem={}, fds={}", pid, limits.max_memory, limits.max_fds);
    Ok(())
}

/// Check whether `pid` can map `extra_bytes` without exceeding its memory limit.
pub fn check_memory_limit(pid: u32, extra_bytes: usize) -> Result<(), &'static str> {
    let table = PROCESS_TABLE.lock();
    let proc = table.get(&pid).ok_or("process not found")?;
    if proc.total_memory() + extra_bytes > proc.limits.max_memory {
        return Err("memory limit exceeded");
    }
    Ok(())
}

/// Check whether `pid` can open another file descriptor.
pub fn check_fd_limit(pid: u32) -> Result<(), &'static str> {
    let table = PROCESS_TABLE.lock();
    let proc = table.get(&pid).ok_or("process not found")?;
    if proc.fd_count() >= proc.limits.max_fds { return Err("fd limit exceeded"); }
    Ok(())
}

/// Terminate a process: close FDs, free memory, zombie state, re-parent
/// orphans to PID 0, wake parent if blocked in wait().
pub fn kill_process(pid: u32, exit_code: i32) -> Result<(), &'static str> {
    if pid == 0 { return Err("cannot kill kernel process"); }

    let mut table = PROCESS_TABLE.lock();
    let proc = table.get_mut(&pid).ok_or("process not found")?;

    // Close file descriptors.
    let fds: Vec<usize> = proc.open_fds.drain(..).collect();
    for fd in &fds { let _ = crate::fd::close(*fd); }

    // Release memory regions.
    let freed_bytes: usize = proc.memory_regions.iter().map(|r| r.len).sum();
    proc.memory_regions.clear();

    let ppid = proc.ppid;
    proc.state = ProcessState::Zombie(exit_code);

    crate::serial_println!("[proc_mgr] kill pid={} exit={}: {} fds, {} bytes freed", pid, exit_code, fds.len(), freed_bytes);
    crate::klog_println!("[proc_mgr] pid {} exited (code {})", pid, exit_code);

    // Re-parent orphaned children to kernel (PID 0).
    let orphans: Vec<u32> = table.iter()
        .filter(|p| p.ppid == pid && p.pid != pid).map(|p| p.pid).collect();
    for oid in &orphans {
        if let Some(child) = table.get_mut(*oid) { child.ppid = 0; }
    }

    // Wake parent if it is blocked in wait().
    if let Some(parent) = table.get_mut(&ppid) {
        if parent.state == ProcessState::Waiting { parent.state = ProcessState::Ready; }
    }
    Ok(())
}

/// Snapshot of a single process for display purposes.
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub state: ProcessState,
    pub name: String,
    pub memory_bytes: usize,
    pub fd_count: usize,
    pub region_count: usize,
    pub max_memory: usize,
    pub max_fds: usize,
}

/// Return a point-in-time snapshot of every process in the table.
pub fn list_processes() -> Vec<ProcessInfo> {
    let table = PROCESS_TABLE.lock();
    table.iter().map(|p| ProcessInfo {
        pid: p.pid, ppid: p.ppid, state: p.state, name: p.name.clone(),
        memory_bytes: p.total_memory(), fd_count: p.fd_count(),
        region_count: p.memory_regions.len(),
        max_memory: p.limits.max_memory, max_fds: p.limits.max_fds,
    }).collect()
}

/// Format a human-readable process listing similar to `ps aux`.
pub fn format_process_list() -> String {
    let procs = list_processes();
    let mut out = String::from("  PID  PPID  STATE       MEM(KiB)  FDs  REGIONS  NAME\n");
    for p in &procs {
        let st = match p.state {
            ProcessState::Ready => "ready  ", ProcessState::Running => "running",
            ProcessState::Waiting => "waiting", ProcessState::Zombie(_) => "zombie ",
            ProcessState::Dead => "dead   ",
        };
        use core::fmt::Write;
        let _ = write!(out, "{:>5}  {:>4}  {}  {:>8}  {:>3}  {:>7}  {}\n",
            p.pid, p.ppid, st, p.memory_bytes / 1024, p.fd_count, p.region_count, p.name);
    }
    out
}
