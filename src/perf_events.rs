/// Performance events subsystem for MerlionOS.
/// Provides hardware-like performance counters, software events,
/// tracepoints, and flame graph generation.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_EVENTS: usize = 64;
const MAX_GROUPS: usize = 16;
const MAX_SAMPLES: usize = 512;
const RING_BUFFER_SIZE: usize = 1024;
const MAX_STACK_DEPTH: usize = 16;
const MAX_ANNOTATIONS: usize = 32;

// ---------------------------------------------------------------------------
// Hardware counter types (simulated)
// ---------------------------------------------------------------------------

/// Hardware performance counter type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwCounter {
    Cycles,
    Instructions,
    CacheHits,
    CacheMisses,
    BranchPredictions,
    BranchMisses,
    TlbMisses,
    BusAccesses,
}

/// Software event type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwEvent {
    ContextSwitches,
    PageFaults,
    TaskMigrations,
    AlignmentFaults,
    EmulationFaults,
    MinorFaults,
    MajorFaults,
}

/// Tracepoint category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tracepoint {
    SyscallEntry,
    SyscallExit,
    SchedSwitch,
    SchedWakeup,
    IrqHandler,
    BlockIo,
    NetworkTx,
    NetworkRx,
}

/// Event kind — hardware, software, or tracepoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Hardware(HwCounter),
    Software(SwEvent),
    Trace(Tracepoint),
}

// ---------------------------------------------------------------------------
// Event filter
// ---------------------------------------------------------------------------

/// Optional filter applied to an event.
#[derive(Debug, Clone, Copy)]
pub struct EventFilter {
    /// Only count for this PID (0 = any).
    pub pid: usize,
    /// Only count on this CPU (usize::MAX = any).
    pub cpu: usize,
    /// Only count for this cgroup id (0 = any).
    pub cgroup: usize,
}

impl EventFilter {
    pub const fn any() -> Self {
        Self { pid: 0, cpu: usize::MAX, cgroup: 0 }
    }

    fn matches(&self, pid: usize, cpu: usize, cgroup: usize) -> bool {
        (self.pid == 0 || self.pid == pid)
            && (self.cpu == usize::MAX || self.cpu == cpu)
            && (self.cgroup == 0 || self.cgroup == cgroup)
    }
}

// ---------------------------------------------------------------------------
// Core event structure
// ---------------------------------------------------------------------------

/// A single performance event descriptor.
#[derive(Debug, Clone)]
struct PerfEvent {
    id: usize,
    kind: EventKind,
    enabled: bool,
    count: u64,
    filter: EventFilter,
    group_id: Option<usize>,
    sample_period: u64,
    samples_taken: u64,
}

impl PerfEvent {
    const fn new(id: usize, kind: EventKind) -> Self {
        Self {
            id,
            kind,
            enabled: false,
            count: 0,
            filter: EventFilter::any(),
            group_id: None,
            sample_period: 0,
            samples_taken: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Event group (leader/follower)
// ---------------------------------------------------------------------------

/// Group of events measured simultaneously.
struct EventGroup {
    id: usize,
    leader: usize,       // event id of group leader
    followers: [usize; 8],
    follower_count: usize,
    enabled: bool,
}

impl EventGroup {
    const fn empty() -> Self {
        Self {
            id: 0,
            leader: 0,
            followers: [0; 8],
            follower_count: 0,
            enabled: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Sample record
// ---------------------------------------------------------------------------

/// A captured sample from statistical profiling.
#[derive(Clone)]
struct SampleRecord {
    timestamp: u64,
    pid: usize,
    cpu: usize,
    event_id: usize,
    /// Simulated instruction pointer.
    ip: u64,
    /// Simulated call stack (addresses).
    stack: [u64; MAX_STACK_DEPTH],
    stack_depth: usize,
}

impl SampleRecord {
    const fn empty() -> Self {
        Self {
            timestamp: 0,
            pid: 0,
            cpu: 0,
            event_id: 0,
            ip: 0,
            stack: [0; MAX_STACK_DEPTH],
            stack_depth: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Ring buffer for perf record
// ---------------------------------------------------------------------------

struct RingBuffer {
    entries: [RingEntry; RING_BUFFER_SIZE],
    head: usize,
    tail: usize,
    total_written: u64,
    total_lost: u64,
}

#[derive(Clone, Copy)]
struct RingEntry {
    timestamp: u64,
    event_id: usize,
    value: u64,
    pid: usize,
}

impl RingEntry {
    const fn empty() -> Self {
        Self { timestamp: 0, event_id: 0, value: 0, pid: 0 }
    }
}

impl RingBuffer {
    const fn new() -> Self {
        Self {
            entries: [RingEntry::empty(); RING_BUFFER_SIZE],
            head: 0,
            tail: 0,
            total_written: 0,
            total_lost: 0,
        }
    }

    fn push(&mut self, entry: RingEntry) {
        let next = (self.head + 1) % RING_BUFFER_SIZE;
        if next == self.tail {
            // Buffer full — advance tail (lose oldest)
            self.tail = (self.tail + 1) % RING_BUFFER_SIZE;
            self.total_lost += 1;
        }
        self.entries[self.head] = entry;
        self.head = next;
        self.total_written += 1;
    }

    fn len(&self) -> usize {
        if self.head >= self.tail {
            self.head - self.tail
        } else {
            RING_BUFFER_SIZE - self.tail + self.head
        }
    }

    fn drain(&mut self) -> Vec<RingEntry> {
        let mut out = Vec::new();
        while self.tail != self.head {
            out.push(self.entries[self.tail]);
            self.tail = (self.tail + 1) % RING_BUFFER_SIZE;
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Function annotation
// ---------------------------------------------------------------------------

struct FuncAnnotation {
    name: [u8; 64],
    name_len: usize,
    addr_start: u64,
    addr_end: u64,
    sample_count: u64,
    cycle_count: u64,
}

impl FuncAnnotation {
    const fn empty() -> Self {
        Self {
            name: [0; 64],
            name_len: 0,
            addr_start: 0,
            addr_end: 0,
            sample_count: 0,
            cycle_count: 0,
        }
    }

    fn set_name(&mut self, s: &str) {
        let bytes = s.as_bytes();
        let len = if bytes.len() > 64 { 64 } else { bytes.len() };
        self.name[..len].copy_from_slice(&bytes[..len]);
        self.name_len = len;
    }

    fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("???")
    }
}

// ---------------------------------------------------------------------------
// Top-down analysis model
// ---------------------------------------------------------------------------

/// Simplified top-down microarchitecture analysis.
pub struct TopDownMetrics {
    pub retiring: u64,
    pub bad_speculation: u64,
    pub frontend_bound: u64,
    pub backend_bound: u64,
    pub total_slots: u64,
}

impl TopDownMetrics {
    const fn zero() -> Self {
        Self {
            retiring: 0,
            bad_speculation: 0,
            frontend_bound: 0,
            backend_bound: 0,
            total_slots: 0,
        }
    }

    fn pct(&self, val: u64) -> u64 {
        if self.total_slots == 0 { 0 } else { val * 100 / self.total_slots }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

struct PerfState {
    events: [PerfEvent; MAX_EVENTS],
    event_count: usize,
    next_id: usize,

    groups: [EventGroup; MAX_GROUPS],
    group_count: usize,
    next_group_id: usize,

    samples: [SampleRecord; MAX_SAMPLES],
    sample_count: usize,

    ring: RingBuffer,
    recording: bool,

    annotations: [FuncAnnotation; MAX_ANNOTATIONS],
    annotation_count: usize,

    topdown: TopDownMetrics,

    /// Global tick counter used for timestamps.
    tick: u64,
}

impl PerfState {
    const fn new() -> Self {
        const EMPTY_EVENT: PerfEvent = PerfEvent::new(0, EventKind::Hardware(HwCounter::Cycles));
        const EMPTY_SAMPLE: SampleRecord = SampleRecord::empty();
        const EMPTY_GROUP: EventGroup = EventGroup::empty();
        const EMPTY_ANNOT: FuncAnnotation = FuncAnnotation::empty();
        Self {
            events: [EMPTY_EVENT; MAX_EVENTS],
            event_count: 0,
            next_id: 1,
            groups: [EMPTY_GROUP; MAX_GROUPS],
            group_count: 0,
            next_group_id: 1,
            samples: [EMPTY_SAMPLE; MAX_SAMPLES],
            sample_count: 0,
            ring: RingBuffer::new(),
            recording: false,
            annotations: [EMPTY_ANNOT; MAX_ANNOTATIONS],
            annotation_count: 0,
            topdown: TopDownMetrics::zero(),
            tick: 0,
        }
    }

    fn find_event(&self, id: usize) -> Option<usize> {
        for i in 0..self.event_count {
            if self.events[i].id == id {
                return Some(i);
            }
        }
        None
    }
}

static STATE: Mutex<PerfState> = Mutex::new(PerfState::new());

// Global atomic counters for fast-path recording
static TOTAL_CYCLES: AtomicU64 = AtomicU64::new(0);
static TOTAL_INSTRUCTIONS: AtomicU64 = AtomicU64::new(0);
static TOTAL_CACHE_HITS: AtomicU64 = AtomicU64::new(0);
static TOTAL_CACHE_MISSES: AtomicU64 = AtomicU64::new(0);
static TOTAL_CONTEXT_SWITCHES: AtomicU64 = AtomicU64::new(0);
static TOTAL_PAGE_FAULTS: AtomicU64 = AtomicU64::new(0);
static TOTAL_BRANCH_PREDICTIONS: AtomicU64 = AtomicU64::new(0);
static TOTAL_BRANCH_MISSES: AtomicU64 = AtomicU64::new(0);
static TOTAL_TLB_MISSES: AtomicU64 = AtomicU64::new(0);
static RECORDING_ACTIVE: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Public API — create / enable / read
// ---------------------------------------------------------------------------

/// Create a new performance event. Returns event id.
pub fn create_event(kind: EventKind, filter: EventFilter) -> Result<usize, &'static str> {
    let mut s = STATE.lock();
    if s.event_count >= MAX_EVENTS {
        return Err("perf: event table full");
    }
    let id = s.next_id;
    s.next_id += 1;
    let idx = s.event_count;
    s.events[idx] = PerfEvent::new(id, kind);
    s.events[idx].filter = filter;
    s.event_count += 1;
    Ok(id)
}

/// Create an event with a sampling period.
pub fn create_sampled_event(
    kind: EventKind,
    filter: EventFilter,
    sample_period: u64,
) -> Result<usize, &'static str> {
    let id = create_event(kind, filter)?;
    let mut s = STATE.lock();
    if let Some(idx) = s.find_event(id) {
        s.events[idx].sample_period = sample_period;
    }
    Ok(id)
}

/// Enable a performance event by id.
pub fn enable_event(id: usize) -> Result<(), &'static str> {
    let mut s = STATE.lock();
    match s.find_event(id) {
        Some(idx) => { s.events[idx].enabled = true; Ok(()) }
        None => Err("perf: event not found"),
    }
}

/// Disable a performance event by id.
pub fn disable_event(id: usize) -> Result<(), &'static str> {
    let mut s = STATE.lock();
    match s.find_event(id) {
        Some(idx) => { s.events[idx].enabled = false; Ok(()) }
        None => Err("perf: event not found"),
    }
}

/// Read the current count for an event.
pub fn read_event(id: usize) -> Result<u64, &'static str> {
    let s = STATE.lock();
    match s.find_event(id) {
        Some(idx) => Ok(s.events[idx].count),
        None => Err("perf: event not found"),
    }
}

/// Reset an event counter to zero.
pub fn reset_event(id: usize) -> Result<(), &'static str> {
    let mut s = STATE.lock();
    match s.find_event(id) {
        Some(idx) => { s.events[idx].count = 0; s.events[idx].samples_taken = 0; Ok(()) }
        None => Err("perf: event not found"),
    }
}

// ---------------------------------------------------------------------------
// Event groups
// ---------------------------------------------------------------------------

/// Create a group of events. The first event id is the leader.
pub fn create_group(leader_id: usize, follower_ids: &[usize]) -> Result<usize, &'static str> {
    let mut s = STATE.lock();
    if s.group_count >= MAX_GROUPS {
        return Err("perf: group table full");
    }
    let gid = s.next_group_id;
    s.next_group_id += 1;
    let gidx = s.group_count;
    s.groups[gidx].id = gid;
    s.groups[gidx].leader = leader_id;
    let fc = if follower_ids.len() > 8 { 8 } else { follower_ids.len() };
    for i in 0..fc {
        s.groups[gidx].followers[i] = follower_ids[i];
    }
    s.groups[gidx].follower_count = fc;

    // Tag events with group id
    if let Some(idx) = s.find_event(leader_id) {
        s.events[idx].group_id = Some(gid);
    }
    for i in 0..fc {
        if let Some(idx) = s.find_event(follower_ids[i]) {
            s.events[idx].group_id = Some(gid);
        }
    }

    s.group_count += 1;
    Ok(gid)
}

/// Enable all events in a group.
pub fn enable_group(gid: usize) -> Result<(), &'static str> {
    let mut s = STATE.lock();
    for g in 0..s.group_count {
        if s.groups[g].id == gid {
            s.groups[g].enabled = true;
            let leader = s.groups[g].leader;
            if let Some(idx) = s.find_event(leader) {
                s.events[idx].enabled = true;
            }
            for f in 0..s.groups[g].follower_count {
                let fid = s.groups[g].followers[f];
                if let Some(idx) = s.find_event(fid) {
                    s.events[idx].enabled = true;
                }
            }
            return Ok(());
        }
    }
    Err("perf: group not found")
}

// ---------------------------------------------------------------------------
// Recording (simulated event delivery)
// ---------------------------------------------------------------------------

/// Record a hardware/software event tick. Called from kernel subsystems.
pub fn record_event(kind: EventKind, pid: usize, cpu: usize, cgroup: usize, delta: u64) {
    // Update atomic counters for fast-path
    match kind {
        EventKind::Hardware(HwCounter::Cycles) => { TOTAL_CYCLES.fetch_add(delta, Ordering::Relaxed); }
        EventKind::Hardware(HwCounter::Instructions) => { TOTAL_INSTRUCTIONS.fetch_add(delta, Ordering::Relaxed); }
        EventKind::Hardware(HwCounter::CacheHits) => { TOTAL_CACHE_HITS.fetch_add(delta, Ordering::Relaxed); }
        EventKind::Hardware(HwCounter::CacheMisses) => { TOTAL_CACHE_MISSES.fetch_add(delta, Ordering::Relaxed); }
        EventKind::Hardware(HwCounter::BranchPredictions) => { TOTAL_BRANCH_PREDICTIONS.fetch_add(delta, Ordering::Relaxed); }
        EventKind::Hardware(HwCounter::BranchMisses) => { TOTAL_BRANCH_MISSES.fetch_add(delta, Ordering::Relaxed); }
        EventKind::Hardware(HwCounter::TlbMisses) => { TOTAL_TLB_MISSES.fetch_add(delta, Ordering::Relaxed); }
        EventKind::Software(SwEvent::ContextSwitches) => { TOTAL_CONTEXT_SWITCHES.fetch_add(delta, Ordering::Relaxed); }
        EventKind::Software(SwEvent::PageFaults) => { TOTAL_PAGE_FAULTS.fetch_add(delta, Ordering::Relaxed); }
        _ => {}
    }

    let mut s = STATE.lock();
    s.tick += 1;
    let ts = s.tick;

    for i in 0..s.event_count {
        if !s.events[i].enabled { continue; }
        if s.events[i].kind != kind { continue; }
        if !s.events[i].filter.matches(pid, cpu, cgroup) { continue; }
        s.events[i].count += delta;

        // Sampling
        if s.events[i].sample_period > 0 {
            s.events[i].samples_taken += delta;
            if s.events[i].samples_taken >= s.events[i].sample_period {
                s.events[i].samples_taken = 0;
                if s.sample_count < MAX_SAMPLES {
                    let si = s.sample_count;
                    s.samples[si].timestamp = ts;
                    s.samples[si].pid = pid;
                    s.samples[si].cpu = cpu;
                    s.samples[si].event_id = s.events[i].id;
                    // Simulated IP and stack
                    s.samples[si].ip = ts.wrapping_mul(0x1234_5678) & 0xFFFF_FFFF;
                    let depth = ((ts % (MAX_STACK_DEPTH as u64 - 1)) + 1) as usize;
                    for d in 0..depth {
                        s.samples[si].stack[d] = s.samples[si].ip.wrapping_add(d as u64 * 0x100);
                    }
                    s.samples[si].stack_depth = depth;
                    s.sample_count += 1;
                }
            }
        }

        // Ring buffer recording
        if s.recording {
            let eid = s.events[i].id;
            s.ring.push(RingEntry {
                timestamp: ts,
                event_id: eid,
                value: delta,
                pid,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Perf stat
// ---------------------------------------------------------------------------

/// Run a simulated workload measurement for `duration_ticks` and report counters.
pub fn perf_stat_report() -> String {
    let cycles = TOTAL_CYCLES.load(Ordering::Relaxed);
    let insns = TOTAL_INSTRUCTIONS.load(Ordering::Relaxed);
    let cache_h = TOTAL_CACHE_HITS.load(Ordering::Relaxed);
    let cache_m = TOTAL_CACHE_MISSES.load(Ordering::Relaxed);
    let branch_p = TOTAL_BRANCH_PREDICTIONS.load(Ordering::Relaxed);
    let branch_m = TOTAL_BRANCH_MISSES.load(Ordering::Relaxed);
    let tlb_m = TOTAL_TLB_MISSES.load(Ordering::Relaxed);
    let ctx_sw = TOTAL_CONTEXT_SWITCHES.load(Ordering::Relaxed);
    let pgfault = TOTAL_PAGE_FAULTS.load(Ordering::Relaxed);

    let ipc = if cycles > 0 { insns * 100 / cycles } else { 0 };
    let cache_rate = if cache_h + cache_m > 0 { cache_h * 100 / (cache_h + cache_m) } else { 0 };
    let branch_rate = if branch_p + branch_m > 0 {
        branch_p * 100 / (branch_p + branch_m)
    } else { 0 };

    let s = STATE.lock();
    let active = s.events.iter().take(s.event_count).filter(|e| e.enabled).count();

    format!(
        "Performance counters:\n\
         {:>14}  cycles\n\
         {:>14}  instructions          ({}.{:02} IPC)\n\
         {:>14}  cache-hits            ({}% hit rate)\n\
         {:>14}  cache-misses\n\
         {:>14}  branch-predictions    ({}% accuracy)\n\
         {:>14}  branch-misses\n\
         {:>14}  TLB-misses\n\
         {:>14}  context-switches\n\
         {:>14}  page-faults\n\n\
         Active events: {} / {}",
        cycles,
        insns, ipc / 100, ipc % 100,
        cache_h, cache_rate,
        cache_m,
        branch_p, branch_rate,
        branch_m,
        tlb_m,
        ctx_sw,
        pgfault,
        active, s.event_count,
    )
}

// ---------------------------------------------------------------------------
// Perf record — start / stop
// ---------------------------------------------------------------------------

/// Start continuous event recording into the ring buffer.
pub fn start_recording() {
    RECORDING_ACTIVE.store(true, Ordering::SeqCst);
    let mut s = STATE.lock();
    s.recording = true;
}

/// Stop recording and return collected entries count.
pub fn stop_recording() -> usize {
    RECORDING_ACTIVE.store(false, Ordering::SeqCst);
    let mut s = STATE.lock();
    s.recording = false;
    s.ring.len()
}

/// Drain recorded events from the ring buffer.
pub fn drain_records() -> Vec<(u64, usize, u64, usize)> {
    let mut s = STATE.lock();
    s.ring.drain().iter().map(|e| (e.timestamp, e.event_id, e.value, e.pid)).collect()
}

// ---------------------------------------------------------------------------
// Perf annotate — function-level performance
// ---------------------------------------------------------------------------

/// Register a function for annotation.
pub fn annotate_function(name: &str, addr_start: u64, addr_end: u64) -> Result<usize, &'static str> {
    let mut s = STATE.lock();
    if s.annotation_count >= MAX_ANNOTATIONS {
        return Err("perf: annotation table full");
    }
    let idx = s.annotation_count;
    s.annotations[idx].set_name(name);
    s.annotations[idx].addr_start = addr_start;
    s.annotations[idx].addr_end = addr_end;
    s.annotations[idx].sample_count = 0;
    s.annotations[idx].cycle_count = 0;
    s.annotation_count += 1;
    Ok(idx)
}

/// Attribute samples to annotated functions.
fn compute_annotations(s: &mut PerfState) {
    // Reset counts
    for i in 0..s.annotation_count {
        s.annotations[i].sample_count = 0;
        s.annotations[i].cycle_count = 0;
    }
    // Walk samples and match IPs to functions
    for si in 0..s.sample_count {
        let ip = s.samples[si].ip;
        for ai in 0..s.annotation_count {
            if ip >= s.annotations[ai].addr_start && ip < s.annotations[ai].addr_end {
                s.annotations[ai].sample_count += 1;
                s.annotations[ai].cycle_count += ip & 0xFF; // simulated cycle cost
                break;
            }
        }
    }
}

/// Format annotation report.
pub fn annotate_report() -> String {
    let mut s = STATE.lock();
    compute_annotations(&mut s);
    let total_samples: u64 = s.annotations[..s.annotation_count]
        .iter().map(|a| a.sample_count).sum();
    let mut out = String::from("Function annotation:\n");
    out.push_str(&format!("{:<32} {:>8} {:>8} {:>6}\n", "Function", "Samples", "Cycles", "Pct"));
    out.push_str(&format!("{:-<60}\n", ""));
    for i in 0..s.annotation_count {
        let a = &s.annotations[i];
        let pct = if total_samples > 0 { a.sample_count * 100 / total_samples } else { 0 };
        out.push_str(&format!(
            "{:<32} {:>8} {:>8} {:>5}%\n",
            a.name_str(), a.sample_count, a.cycle_count, pct
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Flame graph generation (folded stacks format)
// ---------------------------------------------------------------------------

/// Generate a text-based flame graph in folded stacks format.
/// Each line: `func_a;func_b;func_c count`
pub fn generate_flamegraph() -> String {
    let s = STATE.lock();
    if s.sample_count == 0 {
        return String::from("No samples collected. Enable sampled events first.\n\
            Usage: create a sampled event and record workload.");
    }

    // Build folded stacks from samples
    let mut stacks: Vec<(String, u64)> = Vec::new();

    for si in 0..s.sample_count {
        let sample = &s.samples[si];
        let mut stack_str = String::new();
        // Build stack from bottom to top
        for d in (0..sample.stack_depth).rev() {
            let addr = sample.stack[d];
            // Try to resolve against annotations
            let mut resolved = false;
            for ai in 0..s.annotation_count {
                if addr >= s.annotations[ai].addr_start && addr < s.annotations[ai].addr_end {
                    if !stack_str.is_empty() { stack_str.push(';'); }
                    stack_str.push_str(s.annotations[ai].name_str());
                    resolved = true;
                    break;
                }
            }
            if !resolved {
                if !stack_str.is_empty() { stack_str.push(';'); }
                stack_str.push_str(&format!("0x{:x}", addr));
            }
        }

        // Merge with existing or add new
        let mut found = false;
        for entry in stacks.iter_mut() {
            if entry.0 == stack_str {
                entry.1 += 1;
                found = true;
                break;
            }
        }
        if !found {
            stacks.push((stack_str, 1));
        }
    }

    let mut out = String::from("Flame graph (folded stacks):\n");
    out.push_str(&format!("Total samples: {}\n\n", s.sample_count));
    for (stack, count) in &stacks {
        out.push_str(&format!("{} {}\n", stack, count));
    }
    out
}

// ---------------------------------------------------------------------------
// Top-down analysis
// ---------------------------------------------------------------------------

/// Update top-down metrics from current counters.
fn compute_topdown(s: &mut PerfState) {
    let cycles = TOTAL_CYCLES.load(Ordering::Relaxed);
    let insns = TOTAL_INSTRUCTIONS.load(Ordering::Relaxed);
    let branch_m = TOTAL_BRANCH_MISSES.load(Ordering::Relaxed);
    let cache_m = TOTAL_CACHE_MISSES.load(Ordering::Relaxed);

    // Simplified model: total slots = cycles * 4 (simulated pipeline width)
    let total_slots = cycles * 4;
    if total_slots == 0 {
        s.topdown = TopDownMetrics::zero();
        return;
    }

    // Retiring: instructions that completed useful work
    let retiring = insns;
    // Bad speculation: branch mispredictions wasted work
    let bad_spec = branch_m * 20; // ~20 cycles penalty per mispredict
    // Backend bound: cache misses stall backend
    let backend = cache_m * 50; // ~50 cycles penalty per cache miss
    // Frontend bound: remainder
    let frontend = if total_slots > retiring + bad_spec + backend {
        total_slots - retiring - bad_spec - backend
    } else {
        0
    };

    s.topdown = TopDownMetrics {
        retiring,
        bad_speculation: bad_spec,
        frontend_bound: frontend,
        backend_bound: backend,
        total_slots,
    };
}

/// Generate top-down analysis report.
pub fn topdown_analysis() -> String {
    let mut s = STATE.lock();
    compute_topdown(&mut s);
    let td = &s.topdown;

    if td.total_slots == 0 {
        return String::from("Top-down analysis: no data (record some events first)");
    }

    let bar = |pct: u64| -> String {
        let filled = (pct / 5) as usize;
        let empty = 20usize.saturating_sub(filled);
        let mut b = String::new();
        for _ in 0..filled { b.push('#'); }
        for _ in 0..empty { b.push('.'); }
        b
    };

    let r_pct = td.pct(td.retiring);
    let bs_pct = td.pct(td.bad_speculation);
    let fe_pct = td.pct(td.frontend_bound);
    let be_pct = td.pct(td.backend_bound);

    format!(
        "Top-Down Microarchitecture Analysis\n\
         ====================================\n\
         Total pipeline slots: {}\n\n\
         Retiring:         {:>3}% [{}]\n\
         Bad Speculation:  {:>3}% [{}]\n\
         Frontend Bound:   {:>3}% [{}]\n\
         Backend Bound:    {:>3}% [{}]\n\n\
         Bottleneck: {}",
        td.total_slots,
        r_pct, bar(r_pct),
        bs_pct, bar(bs_pct),
        fe_pct, bar(fe_pct),
        be_pct, bar(be_pct),
        if be_pct >= fe_pct && be_pct >= bs_pct { "Backend (memory/cache)" }
        else if fe_pct >= bs_pct { "Frontend (instruction fetch/decode)" }
        else { "Bad Speculation (branch misprediction)" },
    )
}

// ---------------------------------------------------------------------------
// Info / summary
// ---------------------------------------------------------------------------

/// Summary of the performance events subsystem.
pub fn perf_events_info() -> String {
    let s = STATE.lock();
    let active = s.events.iter().take(s.event_count).filter(|e| e.enabled).count();
    let recording = if s.recording { "active" } else { "inactive" };

    let mut out = format!(
        "Performance Events Subsystem\n\
         =============================\n\
         Events:     {} configured, {} active\n\
         Groups:     {}\n\
         Samples:    {} / {}\n\
         Ring buf:   {} / {} entries (lost: {})\n\
         Recording:  {}\n\
         Annotations: {}\n\n",
        s.event_count, active,
        s.group_count,
        s.sample_count, MAX_SAMPLES,
        s.ring.len(), RING_BUFFER_SIZE, s.ring.total_lost,
        recording,
        s.annotation_count,
    );

    // List configured events
    if s.event_count > 0 {
        out.push_str("Configured events:\n");
        for i in 0..s.event_count {
            let e = &s.events[i];
            let kind_name = match e.kind {
                EventKind::Hardware(hw) => match hw {
                    HwCounter::Cycles => "hw:cycles",
                    HwCounter::Instructions => "hw:instructions",
                    HwCounter::CacheHits => "hw:cache-hits",
                    HwCounter::CacheMisses => "hw:cache-misses",
                    HwCounter::BranchPredictions => "hw:branch-pred",
                    HwCounter::BranchMisses => "hw:branch-miss",
                    HwCounter::TlbMisses => "hw:tlb-miss",
                    HwCounter::BusAccesses => "hw:bus-access",
                },
                EventKind::Software(sw) => match sw {
                    SwEvent::ContextSwitches => "sw:ctx-switch",
                    SwEvent::PageFaults => "sw:page-fault",
                    SwEvent::TaskMigrations => "sw:task-migrate",
                    SwEvent::AlignmentFaults => "sw:align-fault",
                    SwEvent::EmulationFaults => "sw:emu-fault",
                    SwEvent::MinorFaults => "sw:minor-fault",
                    SwEvent::MajorFaults => "sw:major-fault",
                },
                EventKind::Trace(tp) => match tp {
                    Tracepoint::SyscallEntry => "tp:syscall-enter",
                    Tracepoint::SyscallExit => "tp:syscall-exit",
                    Tracepoint::SchedSwitch => "tp:sched-switch",
                    Tracepoint::SchedWakeup => "tp:sched-wakeup",
                    Tracepoint::IrqHandler => "tp:irq-handler",
                    Tracepoint::BlockIo => "tp:block-io",
                    Tracepoint::NetworkTx => "tp:net-tx",
                    Tracepoint::NetworkRx => "tp:net-rx",
                },
            };
            let status = if e.enabled { "ON " } else { "OFF" };
            let group = match e.group_id {
                Some(g) => format!("grp:{}", g),
                None => String::from("  -  "),
            };
            out.push_str(&format!(
                "  [{:>3}] {} {:<20} count={:<12} {}\n",
                e.id, status, kind_name, e.count, group
            ));
        }
    }

    // Global counters
    out.push_str("\nGlobal counters:\n");
    out.push_str(&format!("  cycles:       {}\n", TOTAL_CYCLES.load(Ordering::Relaxed)));
    out.push_str(&format!("  instructions: {}\n", TOTAL_INSTRUCTIONS.load(Ordering::Relaxed)));
    out.push_str(&format!("  cache-hits:   {}\n", TOTAL_CACHE_HITS.load(Ordering::Relaxed)));
    out.push_str(&format!("  cache-misses: {}\n", TOTAL_CACHE_MISSES.load(Ordering::Relaxed)));
    out.push_str(&format!("  ctx-switches: {}\n", TOTAL_CONTEXT_SWITCHES.load(Ordering::Relaxed)));
    out.push_str(&format!("  page-faults:  {}\n", TOTAL_PAGE_FAULTS.load(Ordering::Relaxed)));

    out
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the performance events subsystem with default annotations.
pub fn init() {
    let mut s = STATE.lock();

    // Register some default function annotations for the kernel
    let defaults: &[(&str, u64, u64)] = &[
        ("kernel_main",     0x0010_0000, 0x0010_1000),
        ("scheduler",       0x0010_1000, 0x0010_2000),
        ("interrupt_handler", 0x0010_2000, 0x0010_3000),
        ("syscall_dispatch", 0x0010_3000, 0x0010_4000),
        ("memory_alloc",    0x0010_4000, 0x0010_5000),
        ("vfs_operations",  0x0010_5000, 0x0010_6000),
        ("network_stack",   0x0010_6000, 0x0010_7000),
        ("shell_dispatch",  0x0010_7000, 0x0010_8000),
    ];
    for &(name, start, end) in defaults {
        if s.annotation_count < MAX_ANNOTATIONS {
            let idx = s.annotation_count;
            s.annotations[idx].set_name(name);
            s.annotations[idx].addr_start = start;
            s.annotations[idx].addr_end = end;
            s.annotation_count += 1;
        }
    }

    // Seed top-down model with initial values
    s.topdown = TopDownMetrics::zero();
}
