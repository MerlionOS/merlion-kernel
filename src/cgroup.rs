/// Control Groups (cgroups) for MerlionOS.
/// Provides resource limiting, accounting, and isolation for process groups.
/// Implements cgroup v2 style hierarchy with controllers for CPU, memory, I/O, and PIDs.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of cgroups in the system.
const MAX_CGROUPS: usize = 128;
/// Maximum number of member PIDs per cgroup.
const MAX_MEMBERS: usize = 64;
/// Root cgroup path.
const ROOT_PATH: &str = "/sys/fs/cgroup/";
/// Default CPU period in microseconds (100ms).
const DEFAULT_CPU_PERIOD: u64 = 100_000;
/// Default CPU weight.
const DEFAULT_CPU_WEIGHT: u16 = 100;

// ---------------------------------------------------------------------------
// Controller settings
// ---------------------------------------------------------------------------

/// CPU controller: quota/period throttling and weight-based sharing.
#[derive(Debug, Clone, Copy)]
pub struct CpuController {
    /// Maximum CPU time in microseconds per period. 0 = unlimited.
    pub quota: u64,
    /// Period length in microseconds.
    pub period: u64,
    /// Proportional weight for fair sharing (1..10000).
    pub weight: u16,
}

impl CpuController {
    const fn default() -> Self {
        Self {
            quota: 0,
            period: DEFAULT_CPU_PERIOD,
            weight: DEFAULT_CPU_WEIGHT,
        }
    }
}

/// Memory controller: hard limits and swap control.
#[derive(Debug, Clone, Copy)]
pub struct MemoryController {
    /// Maximum memory in bytes. 0 = unlimited.
    pub max: u64,
    /// Current memory usage in bytes.
    pub current: u64,
    /// Maximum swap in bytes. 0 = unlimited.
    pub swap_max: u64,
    /// Current swap usage in bytes.
    pub swap_current: u64,
}

impl MemoryController {
    const fn default() -> Self {
        Self { max: 0, current: 0, swap_max: 0, swap_current: 0 }
    }
}

/// I/O controller: bandwidth and IOPS limits.
#[derive(Debug, Clone, Copy)]
pub struct IoController {
    /// Maximum read bytes per second. 0 = unlimited.
    pub rbps: u64,
    /// Maximum write bytes per second. 0 = unlimited.
    pub wbps: u64,
    /// Maximum read I/O operations per second. 0 = unlimited.
    pub riops: u64,
    /// Maximum write I/O operations per second. 0 = unlimited.
    pub wiops: u64,
}

impl IoController {
    const fn default() -> Self {
        Self { rbps: 0, wbps: 0, riops: 0, wiops: 0 }
    }
}

/// PIDs controller: limit number of processes.
#[derive(Debug, Clone, Copy)]
pub struct PidsController {
    /// Maximum number of PIDs. 0 = unlimited.
    pub max: u32,
    /// Current number of PIDs in this cgroup.
    pub current: u32,
}

impl PidsController {
    const fn default() -> Self {
        Self { max: 0, current: 0 }
    }
}

// ---------------------------------------------------------------------------
// Accounting and events
// ---------------------------------------------------------------------------

/// Per-cgroup resource usage statistics.
#[derive(Debug, Clone, Copy)]
pub struct CgroupStats {
    /// Total CPU time consumed in microseconds.
    pub cpu_usage_us: u64,
    /// Total memory high-watermark in bytes.
    pub memory_peak: u64,
    /// Total I/O bytes read.
    pub io_read_bytes: u64,
    /// Total I/O bytes written.
    pub io_write_bytes: u64,
    /// Total I/O read operations.
    pub io_read_ops: u64,
    /// Total I/O write operations.
    pub io_write_ops: u64,
}

impl CgroupStats {
    const fn zero() -> Self {
        Self {
            cpu_usage_us: 0,
            memory_peak: 0,
            io_read_bytes: 0,
            io_write_bytes: 0,
            io_read_ops: 0,
            io_write_ops: 0,
        }
    }
}

/// Memory-related event counters (mirrors cgroup v2 memory.events).
#[derive(Debug, Clone, Copy)]
pub struct MemoryEvents {
    /// Number of times memory usage hit the max limit.
    pub max: u64,
    /// Number of OOM situations.
    pub oom: u64,
    /// Number of processes killed by OOM.
    pub oom_kill: u64,
}

impl MemoryEvents {
    const fn zero() -> Self {
        Self { max: 0, oom: 0, oom_kill: 0 }
    }
}

/// Freezer state for a cgroup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezerState {
    /// Normal operation.
    Thawed,
    /// All processes in the cgroup are frozen (suspended).
    Frozen,
}

// ---------------------------------------------------------------------------
// Cgroup node
// ---------------------------------------------------------------------------

/// A single cgroup in the hierarchy.
#[derive(Clone)]
pub struct Cgroup {
    /// Unique cgroup identifier.
    pub id: usize,
    /// Short name of this cgroup (leaf component of path).
    pub name: String,
    /// Full path in the cgroup hierarchy (e.g. `/sys/fs/cgroup/app/web`).
    pub path: String,
    /// Parent cgroup id (`None` for root).
    pub parent: Option<usize>,
    /// Child cgroup ids.
    pub children: Vec<usize>,
    /// PIDs that are members of this cgroup.
    pub members: Vec<usize>,
    /// CPU controller settings.
    pub cpu: CpuController,
    /// Memory controller settings.
    pub memory: MemoryController,
    /// I/O controller settings.
    pub io: IoController,
    /// PIDs controller settings.
    pub pids: PidsController,
    /// Accounting statistics.
    pub stats: CgroupStats,
    /// Memory event counters.
    pub mem_events: MemoryEvents,
    /// Freezer state.
    pub freezer: FreezerState,
    /// Whether this cgroup is active (not deleted).
    pub active: bool,
}

impl Cgroup {
    fn new(id: usize, name: String, path: String, parent: Option<usize>) -> Self {
        Self {
            id,
            name,
            path,
            parent,
            children: Vec::new(),
            members: Vec::new(),
            cpu: CpuController::default(),
            memory: MemoryController::default(),
            io: IoController::default(),
            pids: PidsController::default(),
            stats: CgroupStats::zero(),
            mem_events: MemoryEvents::zero(),
            freezer: FreezerState::Thawed,
            active: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Cgroup manager
// ---------------------------------------------------------------------------

/// Manages all cgroups in the system.
struct CgroupManager {
    cgroups: Vec<Cgroup>,
    next_id: usize,
}

impl CgroupManager {
    const fn new() -> Self {
        Self {
            cgroups: Vec::new(),
            next_id: 0,
        }
    }

    fn find_by_path(&self, path: &str) -> Option<usize> {
        self.cgroups.iter().position(|c| c.active && c.path == path)
    }

    fn find_by_id(&self, id: usize) -> Option<usize> {
        self.cgroups.iter().position(|c| c.active && c.id == id)
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static MANAGER: Mutex<CgroupManager> = Mutex::new(CgroupManager::new());

/// Global counters (lock-free).
static TOTAL_CGROUPS: AtomicU64 = AtomicU64::new(0);
static TOTAL_CREATES: AtomicU64 = AtomicU64::new(0);
static TOTAL_REMOVES: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the cgroup subsystem: create root cgroup and register controllers.
pub fn init() {
    let mut mgr = MANAGER.lock();
    let root = Cgroup::new(0, "cgroup".to_owned(), ROOT_PATH.to_owned(), None);
    mgr.cgroups.push(root);
    mgr.next_id = 1;
    TOTAL_CGROUPS.store(1, Ordering::Relaxed);
    TOTAL_CREATES.store(1, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Cgroup lifecycle
// ---------------------------------------------------------------------------

/// Create a new cgroup at the given path.
/// Path must start with the root prefix and parent must exist.
/// Returns the cgroup id on success, or an error string.
pub fn create_cgroup(path: &str) -> Result<usize, &'static str> {
    if !path.starts_with(ROOT_PATH) {
        return Err("path must start with /sys/fs/cgroup/");
    }
    let mut mgr = MANAGER.lock();
    if mgr.cgroups.len() >= MAX_CGROUPS {
        return Err("maximum number of cgroups reached");
    }
    if mgr.find_by_path(path).is_some() {
        return Err("cgroup already exists");
    }

    // Derive parent path
    let trimmed = path.trim_end_matches('/');
    let parent_path = match trimmed.rfind('/') {
        Some(pos) => {
            let p = &trimmed[..pos];
            if p.is_empty() { "/" } else if p.ends_with('/') { p } else {
                // Add trailing slash if it matches root
                p
            }
        }
        None => return Err("invalid path"),
    };

    // Find parent — try with and without trailing slash
    let parent_idx = mgr.find_by_path(parent_path)
        .or_else(|| {
            let with_slash = format!("{}/", parent_path);
            mgr.find_by_path(&with_slash)
        })
        .ok_or("parent cgroup not found")?;
    let parent_id = mgr.cgroups[parent_idx].id;

    // Extract leaf name
    let name = trimmed.rsplit('/').next().unwrap_or("unnamed").to_owned();

    let id = mgr.next_id;
    mgr.next_id += 1;

    let cg = Cgroup::new(id, name, path.to_owned(), Some(parent_id));
    mgr.cgroups.push(cg);
    mgr.cgroups[parent_idx].children.push(id);

    TOTAL_CGROUPS.fetch_add(1, Ordering::Relaxed);
    TOTAL_CREATES.fetch_add(1, Ordering::Relaxed);

    Ok(id)
}

/// Remove a cgroup. It must have no children and no member processes.
pub fn remove_cgroup(path: &str) -> Result<(), &'static str> {
    let mut mgr = MANAGER.lock();
    let idx = mgr.find_by_path(path).ok_or("cgroup not found")?;

    if !mgr.cgroups[idx].children.is_empty() {
        return Err("cgroup has children — remove them first");
    }
    if !mgr.cgroups[idx].members.is_empty() {
        return Err("cgroup has member processes — remove them first");
    }
    if mgr.cgroups[idx].parent.is_none() {
        return Err("cannot remove root cgroup");
    }

    let id = mgr.cgroups[idx].id;
    let parent_id = mgr.cgroups[idx].parent.unwrap();
    mgr.cgroups[idx].active = false;

    // Remove from parent's children list
    if let Some(pidx) = mgr.find_by_id(parent_id) {
        mgr.cgroups[pidx].children.retain(|&c| c != id);
    }

    TOTAL_CGROUPS.fetch_sub(1, Ordering::Relaxed);
    TOTAL_REMOVES.fetch_add(1, Ordering::Relaxed);

    Ok(())
}

// ---------------------------------------------------------------------------
// Process membership
// ---------------------------------------------------------------------------

/// Add a process (by PID) to a cgroup.
pub fn add_process(cgroup_path: &str, pid: usize) -> Result<(), &'static str> {
    let mut mgr = MANAGER.lock();
    let idx = mgr.find_by_path(cgroup_path).ok_or("cgroup not found")?;

    if mgr.cgroups[idx].members.len() >= MAX_MEMBERS {
        return Err("cgroup member limit reached");
    }
    if mgr.cgroups[idx].members.contains(&pid) {
        return Err("process already in cgroup");
    }

    // Check pids limit
    let pids_max = mgr.cgroups[idx].pids.max;
    let pids_cur = mgr.cgroups[idx].pids.current;
    if pids_max > 0 && pids_cur >= pids_max {
        return Err("pids.max limit reached");
    }

    mgr.cgroups[idx].members.push(pid);
    mgr.cgroups[idx].pids.current += 1;

    Ok(())
}

/// Remove a process (by PID) from a cgroup.
pub fn remove_process(cgroup_path: &str, pid: usize) -> Result<(), &'static str> {
    let mut mgr = MANAGER.lock();
    let idx = mgr.find_by_path(cgroup_path).ok_or("cgroup not found")?;

    let pos = mgr.cgroups[idx].members.iter().position(|&p| p == pid)
        .ok_or("process not in cgroup")?;
    mgr.cgroups[idx].members.remove(pos);
    if mgr.cgroups[idx].pids.current > 0 {
        mgr.cgroups[idx].pids.current -= 1;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// CPU controller
// ---------------------------------------------------------------------------

/// Set CPU quota and period for a cgroup (in microseconds).
/// quota=0 means unlimited.
pub fn set_cpu_max(path: &str, quota: u64, period: u64) -> Result<(), &'static str> {
    let mut mgr = MANAGER.lock();
    let idx = mgr.find_by_path(path).ok_or("cgroup not found")?;
    mgr.cgroups[idx].cpu.quota = quota;
    mgr.cgroups[idx].cpu.period = if period == 0 { DEFAULT_CPU_PERIOD } else { period };
    Ok(())
}

/// Set CPU weight for a cgroup (1..10000).
pub fn set_cpu_weight(path: &str, weight: u16) -> Result<(), &'static str> {
    if weight == 0 || weight > 10000 {
        return Err("weight must be 1..10000");
    }
    let mut mgr = MANAGER.lock();
    let idx = mgr.find_by_path(path).ok_or("cgroup not found")?;
    mgr.cgroups[idx].cpu.weight = weight;
    Ok(())
}

/// Record CPU usage for a cgroup (called by the scheduler).
pub fn account_cpu(path: &str, usage_us: u64) {
    let mut mgr = MANAGER.lock();
    if let Some(idx) = mgr.find_by_path(path) {
        mgr.cgroups[idx].stats.cpu_usage_us += usage_us;
    }
}

/// Get CPU stats for a cgroup.
pub fn get_cpu_stats(path: &str) -> Result<String, &'static str> {
    let mgr = MANAGER.lock();
    let idx = mgr.find_by_path(path).ok_or("cgroup not found")?;
    let cg = &mgr.cgroups[idx];
    let quota_str = if cg.cpu.quota == 0 {
        "max".to_owned()
    } else {
        format!("{}", cg.cpu.quota)
    };
    Ok(format!(
        "cpu.max: {} {}\ncpu.weight: {}\ncpu.usage_usec: {}",
        quota_str, cg.cpu.period, cg.cpu.weight, cg.stats.cpu_usage_us
    ))
}

/// Check whether a cgroup has exhausted its CPU budget within the current period.
/// Returns true if over budget (should be throttled).
pub fn check_cpu_budget(path: &str) -> bool {
    let mgr = MANAGER.lock();
    if let Some(idx) = mgr.find_by_path(path) {
        let cg = &mgr.cgroups[idx];
        if cg.cpu.quota == 0 { return false; }
        // Simple check: if usage exceeds quota, throttle
        cg.stats.cpu_usage_us >= cg.cpu.quota
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Memory controller
// ---------------------------------------------------------------------------

/// Set memory maximum for a cgroup (bytes). 0 = unlimited.
pub fn set_memory_max(path: &str, bytes: u64) -> Result<(), &'static str> {
    let mut mgr = MANAGER.lock();
    let idx = mgr.find_by_path(path).ok_or("cgroup not found")?;
    mgr.cgroups[idx].memory.max = bytes;
    Ok(())
}

/// Set swap maximum for a cgroup (bytes). 0 = unlimited.
pub fn set_swap_max(path: &str, bytes: u64) -> Result<(), &'static str> {
    let mut mgr = MANAGER.lock();
    let idx = mgr.find_by_path(path).ok_or("cgroup not found")?;
    mgr.cgroups[idx].memory.swap_max = bytes;
    Ok(())
}

/// Update current memory usage for a cgroup (called by allocator).
pub fn account_memory(path: &str, current: u64) {
    let mut mgr = MANAGER.lock();
    if let Some(idx) = mgr.find_by_path(path) {
        mgr.cgroups[idx].memory.current = current;
        if current > mgr.cgroups[idx].stats.memory_peak {
            mgr.cgroups[idx].stats.memory_peak = current;
        }
        // Check if we hit the max limit
        let max = mgr.cgroups[idx].memory.max;
        if max > 0 && current >= max {
            mgr.cgroups[idx].mem_events.max += 1;
        }
    }
}

/// Get current memory usage for a cgroup.
pub fn get_memory_current(path: &str) -> Result<String, &'static str> {
    let mgr = MANAGER.lock();
    let idx = mgr.find_by_path(path).ok_or("cgroup not found")?;
    let cg = &mgr.cgroups[idx];
    let max_str = if cg.memory.max == 0 { "max".to_owned() } else { format!("{}", cg.memory.max) };
    Ok(format!(
        "memory.max: {}\nmemory.current: {}\nmemory.swap.max: {}\nmemory.peak: {}\nmemory.events: max={} oom={} oom_kill={}",
        max_str, cg.memory.current, cg.memory.swap_max,
        cg.stats.memory_peak,
        cg.mem_events.max, cg.mem_events.oom, cg.mem_events.oom_kill
    ))
}

/// Check whether a cgroup has exceeded its memory limit.
/// Returns true if over limit.
pub fn check_memory_limit(path: &str) -> bool {
    let mgr = MANAGER.lock();
    if let Some(idx) = mgr.find_by_path(path) {
        let cg = &mgr.cgroups[idx];
        if cg.memory.max == 0 { return false; }
        cg.memory.current >= cg.memory.max
    } else {
        false
    }
}

/// Record an OOM event for a cgroup.
pub fn record_oom(path: &str) {
    let mut mgr = MANAGER.lock();
    if let Some(idx) = mgr.find_by_path(path) {
        mgr.cgroups[idx].mem_events.oom += 1;
    }
}

/// Record an OOM kill event for a cgroup.
pub fn record_oom_kill(path: &str) {
    let mut mgr = MANAGER.lock();
    if let Some(idx) = mgr.find_by_path(path) {
        mgr.cgroups[idx].mem_events.oom_kill += 1;
    }
}

// ---------------------------------------------------------------------------
// I/O controller
// ---------------------------------------------------------------------------

/// Set I/O bandwidth limits for a cgroup (bytes/sec). 0 = unlimited.
pub fn set_io_max(path: &str, rbps: u64, wbps: u64) -> Result<(), &'static str> {
    let mut mgr = MANAGER.lock();
    let idx = mgr.find_by_path(path).ok_or("cgroup not found")?;
    mgr.cgroups[idx].io.rbps = rbps;
    mgr.cgroups[idx].io.wbps = wbps;
    Ok(())
}

/// Set I/O IOPS limits for a cgroup. 0 = unlimited.
pub fn set_io_iops(path: &str, riops: u64, wiops: u64) -> Result<(), &'static str> {
    let mut mgr = MANAGER.lock();
    let idx = mgr.find_by_path(path).ok_or("cgroup not found")?;
    mgr.cgroups[idx].io.riops = riops;
    mgr.cgroups[idx].io.wiops = wiops;
    Ok(())
}

/// Record I/O activity for a cgroup.
pub fn account_io(path: &str, read_bytes: u64, write_bytes: u64, read_ops: u64, write_ops: u64) {
    let mut mgr = MANAGER.lock();
    if let Some(idx) = mgr.find_by_path(path) {
        mgr.cgroups[idx].stats.io_read_bytes += read_bytes;
        mgr.cgroups[idx].stats.io_write_bytes += write_bytes;
        mgr.cgroups[idx].stats.io_read_ops += read_ops;
        mgr.cgroups[idx].stats.io_write_ops += write_ops;
    }
}

/// Get I/O stats for a cgroup.
pub fn get_io_stats(path: &str) -> Result<String, &'static str> {
    let mgr = MANAGER.lock();
    let idx = mgr.find_by_path(path).ok_or("cgroup not found")?;
    let cg = &mgr.cgroups[idx];
    let fmt_limit = |v: u64| if v == 0 { "max".to_owned() } else { format!("{}", v) };
    Ok(format!(
        "io.max: rbps={} wbps={} riops={} wiops={}\nio.stat: rbytes={} wbytes={} rios={} wios={}",
        fmt_limit(cg.io.rbps), fmt_limit(cg.io.wbps),
        fmt_limit(cg.io.riops), fmt_limit(cg.io.wiops),
        cg.stats.io_read_bytes, cg.stats.io_write_bytes,
        cg.stats.io_read_ops, cg.stats.io_write_ops
    ))
}

// ---------------------------------------------------------------------------
// PIDs controller
// ---------------------------------------------------------------------------

/// Set PIDs maximum for a cgroup. 0 = unlimited.
pub fn set_pids_max(path: &str, max: u32) -> Result<(), &'static str> {
    let mut mgr = MANAGER.lock();
    let idx = mgr.find_by_path(path).ok_or("cgroup not found")?;
    mgr.cgroups[idx].pids.max = max;
    Ok(())
}

/// Get current PID count for a cgroup.
pub fn get_pids_current(path: &str) -> Result<String, &'static str> {
    let mgr = MANAGER.lock();
    let idx = mgr.find_by_path(path).ok_or("cgroup not found")?;
    let cg = &mgr.cgroups[idx];
    let max_str = if cg.pids.max == 0 { "max".to_owned() } else { format!("{}", cg.pids.max) };
    Ok(format!("pids.max: {}\npids.current: {}", max_str, cg.pids.current))
}

/// Check whether a cgroup has reached its PIDs limit.
pub fn check_pids_limit(path: &str) -> bool {
    let mgr = MANAGER.lock();
    if let Some(idx) = mgr.find_by_path(path) {
        let cg = &mgr.cgroups[idx];
        if cg.pids.max == 0 { return false; }
        cg.pids.current >= cg.pids.max
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Freezer
// ---------------------------------------------------------------------------

/// Freeze all processes in a cgroup.
pub fn freeze(path: &str) -> Result<(), &'static str> {
    let mut mgr = MANAGER.lock();
    let idx = mgr.find_by_path(path).ok_or("cgroup not found")?;
    mgr.cgroups[idx].freezer = FreezerState::Frozen;
    Ok(())
}

/// Thaw (unfreeze) all processes in a cgroup.
pub fn thaw(path: &str) -> Result<(), &'static str> {
    let mut mgr = MANAGER.lock();
    let idx = mgr.find_by_path(path).ok_or("cgroup not found")?;
    mgr.cgroups[idx].freezer = FreezerState::Thawed;
    Ok(())
}

/// Check if a cgroup is frozen.
pub fn is_frozen(path: &str) -> bool {
    let mgr = MANAGER.lock();
    if let Some(idx) = mgr.find_by_path(path) {
        mgr.cgroups[idx].freezer == FreezerState::Frozen
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Listing and inspection
// ---------------------------------------------------------------------------

/// List all active cgroups.
pub fn list_cgroups() -> String {
    let mgr = MANAGER.lock();
    let mut out = String::new();
    out.push_str("PATH                                     PIDS  CPU_WT  MEM_MAX     FREEZER\n");
    out.push_str("---------------------------------------- ----- ------- ----------- -------\n");
    for cg in &mgr.cgroups {
        if !cg.active { continue; }
        let mem_max = if cg.memory.max == 0 { "unlimited".to_owned() } else { format!("{}", cg.memory.max) };
        let freezer = match cg.freezer {
            FreezerState::Thawed => "thawed",
            FreezerState::Frozen => "frozen",
        };
        out.push_str(&format!(
            "{:<40} {:<5} {:<7} {:<11} {}\n",
            cg.path, cg.pids.current, cg.cpu.weight, mem_max, freezer
        ));
    }
    let total = TOTAL_CGROUPS.load(Ordering::Relaxed);
    let creates = TOTAL_CREATES.load(Ordering::Relaxed);
    let removes = TOTAL_REMOVES.load(Ordering::Relaxed);
    out.push_str(&format!("\nTotal: {} active, {} created, {} removed\n", total, creates, removes));
    out
}

/// Show the cgroup hierarchy as a tree.
pub fn cgroup_tree() -> String {
    let mgr = MANAGER.lock();
    let mut out = String::new();
    // Find root
    if let Some(root_idx) = mgr.cgroups.iter().position(|c| c.active && c.parent.is_none()) {
        tree_recursive(&mgr.cgroups, root_idx, 0, &mut out);
    }
    out
}

fn tree_recursive(cgroups: &[Cgroup], idx: usize, depth: usize, out: &mut String) {
    let cg = &cgroups[idx];
    for _ in 0..depth {
        out.push_str("  ");
    }
    let pids_info = if cg.pids.current > 0 {
        format!(" ({} pids)", cg.pids.current)
    } else {
        String::new()
    };
    let frozen = if cg.freezer == FreezerState::Frozen { " [FROZEN]" } else { "" };
    out.push_str(&format!("{}{}{}\n", cg.name, pids_info, frozen));

    for &child_id in &cg.children {
        if let Some(child_idx) = cgroups.iter().position(|c| c.active && c.id == child_id) {
            tree_recursive(cgroups, child_idx, depth + 1, out);
        }
    }
}

/// Get detailed info for a specific cgroup.
pub fn cgroup_info(path: &str) -> Result<String, &'static str> {
    let mgr = MANAGER.lock();
    let idx = mgr.find_by_path(path).ok_or("cgroup not found")?;
    let cg = &mgr.cgroups[idx];

    let mut out = String::new();
    out.push_str(&format!("Cgroup: {}\n", cg.path));
    out.push_str(&format!("Name: {}\n", cg.name));
    out.push_str(&format!("ID: {}\n", cg.id));
    out.push_str(&format!("Freezer: {:?}\n", cg.freezer));
    out.push_str(&format!("Members ({}): {:?}\n", cg.members.len(), cg.members));
    out.push_str(&format!("Children: {}\n", cg.children.len()));

    // CPU
    let quota_str = if cg.cpu.quota == 0 { "max".to_owned() } else { format!("{}", cg.cpu.quota) };
    out.push_str(&format!("\n[cpu]\n  cpu.max: {} {}\n  cpu.weight: {}\n  usage_usec: {}\n",
        quota_str, cg.cpu.period, cg.cpu.weight, cg.stats.cpu_usage_us));

    // Memory
    let mem_max = if cg.memory.max == 0 { "max".to_owned() } else { format!("{}", cg.memory.max) };
    out.push_str(&format!("\n[memory]\n  memory.max: {}\n  memory.current: {}\n  memory.swap.max: {}\n  memory.peak: {}\n  events: max={} oom={} oom_kill={}\n",
        mem_max, cg.memory.current, cg.memory.swap_max,
        cg.stats.memory_peak,
        cg.mem_events.max, cg.mem_events.oom, cg.mem_events.oom_kill));

    // I/O
    let fmt_limit = |v: u64| if v == 0 { "max".to_owned() } else { format!("{}", v) };
    out.push_str(&format!("\n[io]\n  io.max: rbps={} wbps={} riops={} wiops={}\n  io.stat: rbytes={} wbytes={} rios={} wios={}\n",
        fmt_limit(cg.io.rbps), fmt_limit(cg.io.wbps),
        fmt_limit(cg.io.riops), fmt_limit(cg.io.wiops),
        cg.stats.io_read_bytes, cg.stats.io_write_bytes,
        cg.stats.io_read_ops, cg.stats.io_write_ops));

    // PIDs
    let pids_max = if cg.pids.max == 0 { "max".to_owned() } else { format!("{}", cg.pids.max) };
    out.push_str(&format!("\n[pids]\n  pids.max: {}\n  pids.current: {}\n", pids_max, cg.pids.current));

    Ok(out)
}
