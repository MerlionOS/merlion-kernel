/// Performance optimization subsystem for MerlionOS.
/// Implements IO schedulers, transparent huge pages,
/// memory compaction, and a benchmark suite.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_DEVICES: usize = 16;
const MAX_IO_QUEUE: usize = 256;
const MAX_THP_POOL: usize = 64;
const PAGE_SIZE_4K: usize = 4096;
const PAGE_SIZE_2M: usize = 2 * 1024 * 1024;
const THP_ALIGNMENT: usize = PAGE_SIZE_2M;
const DEADLINE_READ_MS: u64 = 500;
const DEADLINE_WRITE_MS: u64 = 5000;
const MAX_PROCESSES_CFQ: usize = 32;
const BFQ_DEFAULT_BUDGET: u32 = 16;
const LRU_MAX_PAGES: usize = 1024;
const BENCHMARK_ITERATIONS: u64 = 100_000;

// ---------------------------------------------------------------------------
// IO Scheduler types
// ---------------------------------------------------------------------------

/// Available IO scheduler algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoScheduler {
    Noop,
    Deadline,
    Cfq,
    Bfq,
}

impl IoScheduler {
    fn as_str(self) -> &'static str {
        match self {
            IoScheduler::Noop => "noop",
            IoScheduler::Deadline => "deadline",
            IoScheduler::Cfq => "cfq",
            IoScheduler::Bfq => "bfq",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "noop" => Some(IoScheduler::Noop),
            "deadline" => Some(IoScheduler::Deadline),
            "cfq" => Some(IoScheduler::Cfq),
            "bfq" => Some(IoScheduler::Bfq),
            _ => None,
        }
    }
}

/// Type of IO request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IoRequestType {
    Read,
    Write,
}

/// A single IO request in the scheduler queue.
#[derive(Debug, Clone)]
struct IoRequest {
    sector: u64,
    count: u32,
    req_type: IoRequestType,
    pid: u32,
    submit_tick: u64,
    deadline_tick: u64,
    budget_cost: u32,
}

/// Per-device scheduler state.
struct DeviceScheduler {
    name: String,
    scheduler: IoScheduler,
    queue: Vec<IoRequest>,
    dispatched: u64,
    merged: u64,
    // Deadline state
    read_deadline_ms: u64,
    write_deadline_ms: u64,
    // CFQ state
    cfq_current_pid: u32,
    cfq_round_robin_idx: usize,
    cfq_pid_list: Vec<u32>,
    // BFQ state
    bfq_budgets: Vec<(u32, u32)>, // (pid, remaining_budget)
}

impl DeviceScheduler {
    fn new(name: &str, sched: IoScheduler) -> Self {
        Self {
            name: String::from(name),
            scheduler: sched,
            queue: Vec::new(),
            dispatched: 0,
            merged: 0,
            read_deadline_ms: DEADLINE_READ_MS,
            write_deadline_ms: DEADLINE_WRITE_MS,
            cfq_current_pid: 0,
            cfq_round_robin_idx: 0,
            cfq_pid_list: Vec::new(),
            bfq_budgets: Vec::new(),
        }
    }

    fn submit(&mut self, req: IoRequest) {
        if self.queue.len() >= MAX_IO_QUEUE {
            // Drop oldest
            self.queue.remove(0);
        }
        self.queue.push(req);
    }

    fn dispatch_next(&mut self) -> Option<IoRequest> {
        if self.queue.is_empty() {
            return None;
        }

        let idx = match self.scheduler {
            IoScheduler::Noop => 0, // FIFO
            IoScheduler::Deadline => self.pick_deadline(),
            IoScheduler::Cfq => self.pick_cfq(),
            IoScheduler::Bfq => self.pick_bfq(),
        };

        let req = self.queue.remove(idx);
        self.dispatched += 1;
        Some(req)
    }

    fn pick_deadline(&self) -> usize {
        let now = TICK_COUNTER.load(Ordering::Relaxed);
        // First check for expired deadlines (reads first)
        let mut expired_read: Option<usize> = None;
        let mut expired_write: Option<usize> = None;

        for (i, req) in self.queue.iter().enumerate() {
            if now >= req.deadline_tick {
                match req.req_type {
                    IoRequestType::Read if expired_read.is_none() => {
                        expired_read = Some(i);
                    }
                    IoRequestType::Write if expired_write.is_none() => {
                        expired_write = Some(i);
                    }
                    _ => {}
                }
            }
        }

        if let Some(idx) = expired_read {
            return idx;
        }
        if let Some(idx) = expired_write {
            return idx;
        }

        // Sort by sector (pick lowest sector)
        let mut best = 0;
        let mut best_sector = self.queue[0].sector;
        for (i, req) in self.queue.iter().enumerate().skip(1) {
            if req.sector < best_sector {
                best_sector = req.sector;
                best = i;
            }
        }
        best
    }

    fn pick_cfq(&mut self) -> usize {
        // Rebuild pid list
        self.cfq_pid_list.clear();
        for req in &self.queue {
            if !self.cfq_pid_list.contains(&req.pid) {
                self.cfq_pid_list.push(req.pid);
            }
        }

        if self.cfq_pid_list.is_empty() {
            return 0;
        }

        // Round-robin through pids
        self.cfq_round_robin_idx %= self.cfq_pid_list.len();
        let target_pid = self.cfq_pid_list[self.cfq_round_robin_idx];
        self.cfq_round_robin_idx += 1;

        // Find first request from this pid
        for (i, req) in self.queue.iter().enumerate() {
            if req.pid == target_pid {
                return i;
            }
        }
        0
    }

    fn pick_bfq(&mut self) -> usize {
        // Assign budgets to new pids
        for req in &self.queue {
            if !self.bfq_budgets.iter().any(|(pid, _)| *pid == req.pid) {
                self.bfq_budgets.push((req.pid, BFQ_DEFAULT_BUDGET));
            }
        }

        // Find pid with highest remaining budget
        let mut best_pid = 0u32;
        let mut best_budget = 0u32;
        for (pid, budget) in &self.bfq_budgets {
            if *budget > best_budget {
                best_budget = *budget;
                best_pid = *pid;
            }
        }

        // Deduct budget
        for (pid, budget) in &mut self.bfq_budgets {
            if *pid == best_pid && *budget > 0 {
                *budget -= 1;
                break;
            }
        }

        // Refill budgets if all exhausted
        let total: u32 = self.bfq_budgets.iter().map(|(_, b)| *b).sum();
        if total == 0 {
            for (_, budget) in &mut self.bfq_budgets {
                *budget = BFQ_DEFAULT_BUDGET;
            }
        }

        // Remove pids with no queued requests
        self.bfq_budgets.retain(|(pid, _)| {
            self.queue.iter().any(|r| r.pid == *pid)
        });

        for (i, req) in self.queue.iter().enumerate() {
            if req.pid == best_pid {
                return i;
            }
        }
        0
    }
}

// ---------------------------------------------------------------------------
// Transparent Huge Pages
// ---------------------------------------------------------------------------

struct ThpRegion {
    base_addr: u64,
    allocated: bool,
}

struct ThpState {
    pool: Vec<ThpRegion>,
    allocated_count: u64,
    freed_count: u64,
    fallback_4k: u64,
    compaction_runs: u64,
}

impl ThpState {
    fn new() -> Self {
        // Create simulated 2MB-aligned free regions
        let mut pool = Vec::new();
        let base: u64 = 0x1_0000_0000; // 4GB mark
        for i in 0..MAX_THP_POOL {
            pool.push(ThpRegion {
                base_addr: base + (i as u64) * (PAGE_SIZE_2M as u64),
                allocated: false,
            });
        }
        Self {
            pool,
            allocated_count: 0,
            freed_count: 0,
            fallback_4k: 0,
            compaction_runs: 0,
        }
    }

    fn alloc(&mut self) -> Option<u64> {
        for region in &mut self.pool {
            if !region.allocated {
                region.allocated = true;
                self.allocated_count += 1;
                return Some(region.base_addr);
            }
        }
        self.fallback_4k += 1;
        None
    }

    fn free(&mut self, addr: u64) -> bool {
        for region in &mut self.pool {
            if region.base_addr == addr && region.allocated {
                region.allocated = false;
                self.freed_count += 1;
                return true;
            }
        }
        false
    }

    fn compact(&mut self) -> usize {
        self.compaction_runs += 1;
        // Simulated compaction: count fragmented regions
        let mut compacted = 0usize;
        let mut prev_free = false;
        for region in &self.pool {
            if !region.allocated {
                if prev_free {
                    compacted += 1;
                }
                prev_free = true;
            } else {
                prev_free = false;
            }
        }
        compacted
    }

    fn stats_string(&self) -> String {
        let in_use = self.pool.iter().filter(|r| r.allocated).count();
        let free = self.pool.len() - in_use;
        format!(
            "Transparent Huge Pages:\n  Pool size: {} x 2MB\n  In use: {}  Free: {}\n  Allocations: {}  Frees: {}\n  4KB fallbacks: {}\n  Compaction runs: {}",
            self.pool.len(), in_use, free,
            self.allocated_count, self.freed_count,
            self.fallback_4k,
            self.compaction_runs,
        )
    }
}

// ---------------------------------------------------------------------------
// Page Reclaim (LRU)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemPressure {
    Low,
    Medium,
    Critical,
}

impl MemPressure {
    fn as_str(self) -> &'static str {
        match self {
            MemPressure::Low => "low",
            MemPressure::Medium => "medium",
            MemPressure::Critical => "critical",
        }
    }
}

struct LruPage {
    addr: u64,
    active: bool,
    access_count: u32,
}

struct PageReclaim {
    pages: Vec<LruPage>,
    reclaimed_total: u64,
    pressure: MemPressure,
}

impl PageReclaim {
    fn new() -> Self {
        // Simulate some LRU pages
        let mut pages = Vec::new();
        for i in 0..LRU_MAX_PAGES {
            pages.push(LruPage {
                addr: 0x2_0000_0000 + (i as u64) * (PAGE_SIZE_4K as u64),
                active: i % 3 != 0, // 2/3 active, 1/3 inactive
                access_count: (i % 10) as u32,
            });
        }
        Self {
            pages,
            reclaimed_total: 0,
            pressure: MemPressure::Low,
        }
    }

    fn reclaim(&mut self, pages_needed: usize) -> usize {
        let mut reclaimed = 0;
        // Evict inactive pages first
        let mut i = 0;
        while i < self.pages.len() && reclaimed < pages_needed {
            if !self.pages[i].active {
                self.pages.remove(i);
                reclaimed += 1;
            } else {
                i += 1;
            }
        }
        // If still need more, demote active to inactive
        if reclaimed < pages_needed {
            for page in &mut self.pages {
                if page.active {
                    page.active = false;
                }
            }
        }
        self.reclaimed_total += reclaimed as u64;
        self.update_pressure();
        reclaimed
    }

    fn update_pressure(&mut self) {
        let total = LRU_MAX_PAGES;
        let current = self.pages.len();
        let used_pct = if total > 0 { current * 100 / total } else { 0 };
        self.pressure = if used_pct > 90 {
            MemPressure::Critical
        } else if used_pct > 70 {
            MemPressure::Medium
        } else {
            MemPressure::Low
        };
    }
}

// ---------------------------------------------------------------------------
// Benchmark suite
// ---------------------------------------------------------------------------

struct BenchResult {
    name: String,
    value: u64,
    unit: String,
}

fn bench_cpu() -> BenchResult {
    // Integer arithmetic throughput
    let mut sum: u64 = 0;
    let start = TICK_COUNTER.load(Ordering::Relaxed);
    for i in 0..BENCHMARK_ITERATIONS {
        sum = sum.wrapping_add(i.wrapping_mul(7).wrapping_add(13));
        sum ^= sum >> 3;
    }
    let end = TICK_COUNTER.load(Ordering::Relaxed);
    let elapsed = end.saturating_sub(start).max(1);
    // ops per tick * 100 (100Hz) = ops/sec
    let ops_per_sec = BENCHMARK_ITERATIONS * 100 / elapsed;
    // Prevent sum from being optimized out
    BENCH_SINK.store(sum, Ordering::Relaxed);
    BenchResult {
        name: String::from("CPU integer"),
        value: ops_per_sec,
        unit: String::from("ops/sec"),
    }
}

fn bench_memory() -> BenchResult {
    // Sequential read/write bandwidth (integer math, no FP)
    let buf_size: usize = 4096;
    let mut buf: Vec<u8> = Vec::new();
    buf.resize(buf_size, 0xAA);

    let start = TICK_COUNTER.load(Ordering::Relaxed);
    let iterations = 10000u64;
    let mut checksum: u64 = 0;
    for _ in 0..iterations {
        // Write pass
        for j in 0..buf_size {
            buf[j] = (j & 0xFF) as u8;
        }
        // Read pass
        for j in 0..buf_size {
            checksum = checksum.wrapping_add(buf[j] as u64);
        }
    }
    let end = TICK_COUNTER.load(Ordering::Relaxed);
    let elapsed = end.saturating_sub(start).max(1);
    // bytes transferred = iterations * buf_size * 2 (read + write)
    let total_bytes = iterations * (buf_size as u64) * 2;
    // MB/s = total_bytes / (elapsed_sec) / 1024 / 1024
    // elapsed_sec = elapsed / 100
    let mb_per_sec = total_bytes * 100 / elapsed / 1024 / 1024;
    BENCH_SINK.store(checksum, Ordering::Relaxed);
    BenchResult {
        name: String::from("Memory bandwidth"),
        value: mb_per_sec,
        unit: String::from("MB/s"),
    }
}

fn bench_storage() -> BenchResult {
    // Simulated block device IOPS (random read/write)
    let mut iops: u64 = 0;
    let start = TICK_COUNTER.load(Ordering::Relaxed);
    let mut seed: u64 = 42;
    for _ in 0..BENCHMARK_ITERATIONS {
        // Simulate random sector calculation
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let _sector = seed % 1_000_000;
        iops += 1;
    }
    let end = TICK_COUNTER.load(Ordering::Relaxed);
    let elapsed = end.saturating_sub(start).max(1);
    let iops_sec = iops * 100 / elapsed;
    BenchResult {
        name: String::from("Storage IOPS"),
        value: iops_sec,
        unit: String::from("IOPS"),
    }
}

fn bench_network() -> BenchResult {
    // Simulated TCP throughput
    let packet_size: u64 = 1460; // MSS
    let start = TICK_COUNTER.load(Ordering::Relaxed);
    let packets = 50000u64;
    let mut total_bytes: u64 = 0;
    for _ in 0..packets {
        total_bytes += packet_size;
    }
    let end = TICK_COUNTER.load(Ordering::Relaxed);
    let elapsed = end.saturating_sub(start).max(1);
    let bytes_per_sec = total_bytes * 100 / elapsed;
    BenchResult {
        name: String::from("Network throughput"),
        value: bytes_per_sec,
        unit: String::from("bytes/sec"),
    }
}

fn bench_latency() -> String {
    // Simulated syscall latency percentiles
    let mut latencies: Vec<u64> = Vec::new();
    let mut seed: u64 = 12345;
    for _ in 0..1000 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        // Simulated latency: 1-100 microseconds
        let lat = (seed % 100) + 1;
        latencies.push(lat);
    }
    latencies.sort();
    let p50 = latencies[499];
    let p90 = latencies[899];
    let p99 = latencies[989];
    format!("Syscall latency: p50={}us p90={}us p99={}us", p50, p90, p99)
}

// ---------------------------------------------------------------------------
// Performance tuning recommendations
// ---------------------------------------------------------------------------

fn analyze_workload(state: &PerfOptState) -> String {
    let mut recs = String::from("Tuning Recommendations:\n");

    // IO scheduler recommendation
    let total_io: u64 = state.devices.iter().map(|d| d.dispatched).sum();
    if total_io > 1000 {
        recs.push_str("  - High IO workload detected: consider 'bfq' scheduler\n");
    }

    // THP recommendation
    if state.thp.fallback_4k > state.thp.allocated_count {
        recs.push_str("  - Many THP fallbacks: run memory compaction\n");
    }

    // Memory pressure
    match state.reclaim.pressure {
        MemPressure::Critical => {
            recs.push_str("  - CRITICAL memory pressure: increase RAM or reduce workload\n");
        }
        MemPressure::Medium => {
            recs.push_str("  - Medium memory pressure: consider freeing caches\n");
        }
        MemPressure::Low => {
            recs.push_str("  - Memory pressure is low: system is healthy\n");
        }
    }

    // General
    recs.push_str("  - Use 'deadline' scheduler for latency-sensitive workloads\n");
    recs.push_str("  - Use 'bfq' scheduler for desktop/interactive workloads\n");

    recs
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

struct PerfOptState {
    devices: Vec<DeviceScheduler>,
    thp: ThpState,
    reclaim: PageReclaim,
    bench_results: Vec<BenchResult>,
}

impl PerfOptState {
    fn new() -> Self {
        Self {
            devices: Vec::new(),
            thp: ThpState::new(),
            reclaim: PageReclaim::new(),
            bench_results: Vec::new(),
        }
    }
}

static STATE: Mutex<Option<PerfOptState>> = Mutex::new(None);
static TICK_COUNTER: AtomicU64 = AtomicU64::new(0);
static BENCH_SINK: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the performance optimization subsystem.
pub fn init() {
    let mut state = STATE.lock();
    let mut s = PerfOptState::new();
    // Register default devices
    s.devices.push(DeviceScheduler::new("sda", IoScheduler::Deadline));
    s.devices.push(DeviceScheduler::new("nvme0n1", IoScheduler::Bfq));
    *state = Some(s);
}

/// Set the IO scheduler for a device.
pub fn set_scheduler(device: &str, sched_name: &str) -> Result<(), &'static str> {
    let sched = IoScheduler::from_str(sched_name).ok_or("unknown scheduler")?;
    let mut state = STATE.lock();
    let s = state.as_mut().ok_or("perf_opt not initialized")?;

    for dev in &mut s.devices {
        if dev.name == device {
            dev.scheduler = sched;
            return Ok(());
        }
    }

    // Device not found, create it
    if s.devices.len() >= MAX_DEVICES {
        return Err("too many devices");
    }
    s.devices.push(DeviceScheduler::new(device, sched));
    Ok(())
}

/// Get the current IO scheduler for a device.
pub fn get_scheduler(device: &str) -> String {
    let state = STATE.lock();
    let s = match state.as_ref() {
        Some(s) => s,
        None => return String::from("not initialized"),
    };

    for dev in &s.devices {
        if dev.name == device {
            return String::from(dev.scheduler.as_str());
        }
    }
    String::from("device not found")
}

/// Submit an IO request.
pub fn submit_io(device: &str, sector: u64, count: u32, is_write: bool, pid: u32) {
    let mut state = STATE.lock();
    let s = match state.as_mut() {
        Some(s) => s,
        None => return,
    };

    let now = TICK_COUNTER.load(Ordering::Relaxed);
    let req_type = if is_write { IoRequestType::Write } else { IoRequestType::Read };
    let deadline = now + if is_write {
        DEADLINE_WRITE_MS / 10
    } else {
        DEADLINE_READ_MS / 10
    };

    let req = IoRequest {
        sector,
        count,
        req_type,
        pid,
        submit_tick: now,
        deadline_tick: deadline,
        budget_cost: 1,
    };

    for dev in &mut s.devices {
        if dev.name == device {
            dev.submit(req);
            return;
        }
    }
}

/// Allocate a transparent huge page (2MB).
pub fn thp_alloc() -> Option<u64> {
    let mut state = STATE.lock();
    let s = state.as_mut()?;
    s.thp.alloc()
}

/// Free a transparent huge page.
pub fn thp_free(addr: u64) -> bool {
    let mut state = STATE.lock();
    let s = match state.as_mut() {
        Some(s) => s,
        None => return false,
    };
    s.thp.free(addr)
}

/// Run memory compaction.
pub fn compact_memory() -> usize {
    let mut state = STATE.lock();
    let s = match state.as_mut() {
        Some(s) => s,
        None => return 0,
    };
    s.thp.compact()
}

/// Reclaim memory pages.
pub fn reclaim_pages(needed: usize) -> usize {
    let mut state = STATE.lock();
    let s = match state.as_mut() {
        Some(s) => s,
        None => return 0,
    };
    s.reclaim.reclaim(needed)
}

/// Run all benchmarks and return a formatted report.
pub fn run_all_benchmarks() -> String {
    let cpu = bench_cpu();
    let mem = bench_memory();
    let storage = bench_storage();
    let net = bench_network();
    let latency = bench_latency();

    let mut report = String::from("=== MerlionOS Benchmark Suite ===\n\n");
    report.push_str(&format!("  {}: {} {}\n", cpu.name, cpu.value, cpu.unit));
    report.push_str(&format!("  {}: {} {}\n", mem.name, mem.value, mem.unit));
    report.push_str(&format!("  {}: {} {}\n", storage.name, storage.value, storage.unit));
    report.push_str(&format!("  {}: {} {}\n", net.name, net.value, net.unit));
    report.push_str(&format!("  {}\n", latency));

    // Store results
    let mut state = STATE.lock();
    if let Some(s) = state.as_mut() {
        s.bench_results.clear();
        s.bench_results.push(cpu);
        s.bench_results.push(mem);
        s.bench_results.push(storage);
        s.bench_results.push(net);
    }

    report.push_str("\n=== Benchmark Complete ===\n");
    report
}

/// Run a single benchmark by name.
pub fn run_benchmark(name: &str) -> String {
    match name {
        "cpu" => {
            let r = bench_cpu();
            format!("{}: {} {}", r.name, r.value, r.unit)
        }
        "mem" | "memory" => {
            let r = bench_memory();
            format!("{}: {} {}", r.name, r.value, r.unit)
        }
        "io" | "storage" => {
            let r = bench_storage();
            format!("{}: {} {}", r.name, r.value, r.unit)
        }
        "net" | "network" => {
            let r = bench_network();
            format!("{}: {} {}", r.name, r.value, r.unit)
        }
        "latency" => bench_latency(),
        _ => format!("Unknown benchmark: {}. Available: cpu, mem, io, net, latency", name),
    }
}

/// Show IO scheduler info for all devices.
pub fn io_sched_info() -> String {
    let state = STATE.lock();
    let s = match state.as_ref() {
        Some(s) => s,
        None => return String::from("perf_opt not initialized"),
    };

    let mut out = String::from("IO Schedulers:\n");
    for dev in &s.devices {
        out.push_str(&format!(
            "  {}: {}  (queued: {}, dispatched: {}, merged: {})\n",
            dev.name, dev.scheduler.as_str(),
            dev.queue.len(), dev.dispatched, dev.merged,
        ));
    }
    out.push_str(&format!("  Available: [noop] [deadline] [cfq] [bfq]\n"));
    out
}

/// THP information.
pub fn thp_info() -> String {
    let state = STATE.lock();
    let s = match state.as_ref() {
        Some(s) => s,
        None => return String::from("perf_opt not initialized"),
    };
    s.thp.stats_string()
}

/// Performance optimization subsystem info.
pub fn perf_opt_info() -> String {
    let state = STATE.lock();
    let s = match state.as_ref() {
        Some(s) => s,
        None => return String::from("perf_opt not initialized"),
    };

    let devices = s.devices.len();
    let thp_used = s.thp.pool.iter().filter(|r| r.allocated).count();
    let thp_free = s.thp.pool.len() - thp_used;
    let lru_pages = s.reclaim.pages.len();
    let pressure = s.reclaim.pressure.as_str();

    format!(
        "Performance Optimization Subsystem\n  Devices: {}\n  THP: {} used, {} free\n  LRU pages: {}\n  Memory pressure: {}\n  Benchmarks stored: {}",
        devices, thp_used, thp_free, lru_pages, pressure, s.bench_results.len(),
    )
}

/// Performance optimization statistics.
pub fn perf_opt_stats() -> String {
    let state = STATE.lock();
    let s = match state.as_ref() {
        Some(s) => s,
        None => return String::from("perf_opt not initialized"),
    };

    let total_dispatched: u64 = s.devices.iter().map(|d| d.dispatched).sum();
    let total_queued: usize = s.devices.iter().map(|d| d.queue.len()).sum();

    let mut out = format!(
        "Performance Stats:\n  IO requests dispatched: {}\n  IO requests queued: {}\n  THP allocations: {}\n  THP frees: {}\n  THP fallbacks (4KB): {}\n  Compaction runs: {}\n  Pages reclaimed: {}\n  Memory pressure: {}\n",
        total_dispatched, total_queued,
        s.thp.allocated_count, s.thp.freed_count, s.thp.fallback_4k,
        s.thp.compaction_runs,
        s.reclaim.reclaimed_total,
        s.reclaim.pressure.as_str(),
    );

    out.push_str(&analyze_workload(s));
    out
}

/// Tick the performance subsystem (called from timer).
pub fn tick() {
    TICK_COUNTER.fetch_add(1, Ordering::Relaxed);
}
