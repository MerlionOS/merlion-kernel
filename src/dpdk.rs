/// DPDK-style polling mode driver framework for MerlionOS.
/// Bypasses interrupts for high-throughput packet processing
/// using poll-mode drivers, lock-free ring buffers, and memory pools.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicUsize, AtomicU64, AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of PMDs.
const MAX_PMDS: usize = 32;

/// Maximum number of mempools.
const MAX_MEMPOOLS: usize = 64;

/// Maximum number of pipelines.
const MAX_PIPELINES: usize = 16;

/// Default burst size.
const DEFAULT_BURST: u16 = 32;

/// Maximum burst size.
const MAX_BURST: u16 = 64;

/// Default ring capacity (must be power of 2).
const DEFAULT_RING_CAP: usize = 1024;

/// Maximum packet size for mempool buffers.
const MAX_PKT_SIZE: usize = 2048;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static TOTAL_POLL_CYCLES: AtomicU64 = AtomicU64::new(0);
static TOTAL_TX_PKTS: AtomicU64 = AtomicU64::new(0);
static TOTAL_RX_PKTS: AtomicU64 = AtomicU64::new(0);
static TOTAL_TX_BYTES: AtomicU64 = AtomicU64::new(0);
static TOTAL_RX_BYTES: AtomicU64 = AtomicU64::new(0);

static PMDS: Mutex<PmdRegistry> = Mutex::new(PmdRegistry::new());
static MEMPOOLS: Mutex<MempoolRegistry> = Mutex::new(MempoolRegistry::new());
static PIPELINES: Mutex<PipelineRegistry> = Mutex::new(PipelineRegistry::new());
static CORE_AFFINITY: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------------
// Poll Mode Driver
// ---------------------------------------------------------------------------

/// A poll-mode driver that bypasses interrupts for packet I/O.
pub struct PollModeDriver {
    name: String,
    num_rx_queues: u16,
    num_tx_queues: u16,
    burst_size: u16,
    polling: bool,
    rx_pkts: u64,
    tx_pkts: u64,
    rx_bytes: u64,
    tx_bytes: u64,
    poll_cycles: u64,
    rx_rings: Vec<SpscRing>,
    tx_rings: Vec<SpscRing>,
}

impl PollModeDriver {
    fn new(name: String, rx_queues: u16, tx_queues: u16, burst: u16) -> Self {
        let burst = if burst > MAX_BURST { MAX_BURST } else if burst == 0 { DEFAULT_BURST } else { burst };
        let mut rx_rings = Vec::with_capacity(rx_queues as usize);
        for _ in 0..rx_queues {
            rx_rings.push(SpscRing::new(DEFAULT_RING_CAP));
        }
        let mut tx_rings = Vec::with_capacity(tx_queues as usize);
        for _ in 0..tx_queues {
            tx_rings.push(SpscRing::new(DEFAULT_RING_CAP));
        }
        Self {
            name,
            num_rx_queues: rx_queues,
            num_tx_queues: tx_queues,
            burst_size: burst,
            polling: false,
            rx_pkts: 0,
            tx_pkts: 0,
            rx_bytes: 0,
            tx_bytes: 0,
            poll_cycles: 0,
            rx_rings,
            tx_rings,
        }
    }
}

struct PmdRegistry {
    pmds: Vec<PollModeDriver>,
}

impl PmdRegistry {
    const fn new() -> Self {
        Self { pmds: Vec::new() }
    }
}

/// Create a new PMD. Returns PMD id.
pub fn pmd_create(iface: &str, rx_queues: u16, tx_queues: u16, burst: u16) -> Result<u32, &'static str> {
    let mut reg = PMDS.lock();
    if reg.pmds.len() >= MAX_PMDS {
        return Err("too many PMDs");
    }
    let id = reg.pmds.len() as u32;
    let name = String::from(iface);
    reg.pmds.push(PollModeDriver::new(name, rx_queues, tx_queues, burst));
    Ok(id)
}

/// Start polling on a PMD.
pub fn pmd_start(id: u32) -> Result<(), &'static str> {
    let mut reg = PMDS.lock();
    let pmd = reg.pmds.get_mut(id as usize).ok_or("PMD not found")?;
    pmd.polling = true;
    Ok(())
}

/// Stop polling on a PMD.
pub fn pmd_stop(id: u32) -> Result<(), &'static str> {
    let mut reg = PMDS.lock();
    let pmd = reg.pmds.get_mut(id as usize).ok_or("PMD not found")?;
    pmd.polling = false;
    Ok(())
}

/// Receive up to burst_size packets from a queue.
pub fn pmd_rx_burst(id: u32, queue: u16) -> Result<Vec<Vec<u8>>, &'static str> {
    let mut reg = PMDS.lock();
    let pmd = reg.pmds.get_mut(id as usize).ok_or("PMD not found")?;
    if !pmd.polling {
        return Err("PMD not started");
    }
    if queue >= pmd.num_rx_queues {
        return Err("invalid RX queue");
    }
    let ring = &mut pmd.rx_rings[queue as usize];
    let mut pkts = Vec::new();
    let burst = pmd.burst_size;
    for _ in 0..burst {
        match ring.dequeue() {
            Some(pkt) => {
                pmd.rx_bytes += pkt.len() as u64;
                pmd.rx_pkts += 1;
                pkts.push(pkt);
            }
            None => break,
        }
    }
    TOTAL_RX_PKTS.fetch_add(pkts.len() as u64, Ordering::Relaxed);
    Ok(pkts)
}

/// Send a batch of packets on a TX queue.
pub fn pmd_tx_burst(id: u32, queue: u16, pkts: &[&[u8]]) -> Result<u32, &'static str> {
    let mut reg = PMDS.lock();
    let pmd = reg.pmds.get_mut(id as usize).ok_or("PMD not found")?;
    if !pmd.polling {
        return Err("PMD not started");
    }
    if queue >= pmd.num_tx_queues {
        return Err("invalid TX queue");
    }
    let ring = &mut pmd.tx_rings[queue as usize];
    let mut sent = 0u32;
    for pkt in pkts {
        let data = pkt.to_vec();
        pmd.tx_bytes += data.len() as u64;
        if ring.enqueue(data) {
            pmd.tx_pkts += 1;
            sent += 1;
        } else {
            break;
        }
    }
    TOTAL_TX_PKTS.fetch_add(sent as u64, Ordering::Relaxed);
    Ok(sent)
}

/// Execute one poll cycle (no interrupts).
pub fn pmd_poll(id: u32) -> Result<(), &'static str> {
    let mut reg = PMDS.lock();
    let pmd = reg.pmds.get_mut(id as usize).ok_or("PMD not found")?;
    if !pmd.polling {
        return Err("PMD not started");
    }
    pmd.poll_cycles += 1;
    TOTAL_POLL_CYCLES.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

// ---------------------------------------------------------------------------
// SPSC Ring Buffer (single-producer single-consumer)
// ---------------------------------------------------------------------------

/// Lock-free single-producer single-consumer ring buffer.
pub struct SpscRing {
    buf: Vec<Option<Vec<u8>>>,
    capacity: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl SpscRing {
    /// Create a new SPSC ring with the given capacity.
    pub fn new(capacity: usize) -> Self {
        // Round up to power of 2
        let cap = capacity.next_power_of_two();
        let mut buf = Vec::with_capacity(cap);
        for _ in 0..cap {
            buf.push(None);
        }
        Self {
            buf,
            capacity: cap,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Enqueue data. Returns true on success, false if full.
    pub fn enqueue(&mut self, data: Vec<u8>) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) & (self.capacity - 1);
        let tail = self.tail.load(Ordering::Acquire);
        if next == tail {
            return false; // full
        }
        self.buf[head] = Some(data);
        self.head.store(next, Ordering::Release);
        true
    }

    /// Dequeue data. Returns None if empty.
    pub fn dequeue(&mut self) -> Option<Vec<u8>> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail == head {
            return None; // empty
        }
        let data = self.buf[tail].take();
        let next = (tail + 1) & (self.capacity - 1);
        self.tail.store(next, Ordering::Release);
        data
    }

    /// Number of elements in the ring.
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        (head.wrapping_sub(tail)) & (self.capacity - 1)
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Relaxed) == self.tail.load(Ordering::Relaxed)
    }

    /// Check if full.
    pub fn is_full(&self) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        ((head + 1) & (self.capacity - 1)) == tail
    }
}

// ---------------------------------------------------------------------------
// MPSC Ring Buffer (multi-producer single-consumer)
// ---------------------------------------------------------------------------

/// Multi-producer single-consumer ring buffer using a spinlock.
pub struct MpscRing {
    inner: Mutex<MpscInner>,
}

struct MpscInner {
    buf: Vec<Option<Vec<u8>>>,
    capacity: usize,
    head: usize,
    tail: usize,
}

impl MpscRing {
    /// Create a new MPSC ring.
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.next_power_of_two();
        let mut buf = Vec::with_capacity(cap);
        for _ in 0..cap {
            buf.push(None);
        }
        Self {
            inner: Mutex::new(MpscInner {
                buf,
                capacity: cap,
                head: 0,
                tail: 0,
            }),
        }
    }

    /// Enqueue (thread-safe for multiple producers).
    pub fn enqueue(&self, data: Vec<u8>) -> bool {
        let mut inner = self.inner.lock();
        let next = (inner.head + 1) & (inner.capacity - 1);
        if next == inner.tail {
            return false;
        }
        let h = inner.head;
        inner.buf[h] = Some(data);
        inner.head = next;
        true
    }

    /// Dequeue (single consumer).
    pub fn dequeue(&self) -> Option<Vec<u8>> {
        let mut inner = self.inner.lock();
        if inner.tail == inner.head {
            return None;
        }
        let t = inner.tail;
        let data = inner.buf[t].take();
        inner.tail = (t + 1) & (inner.capacity - 1);
        data
    }

    /// Number of items.
    pub fn len(&self) -> usize {
        let inner = self.inner.lock();
        (inner.head.wrapping_sub(inner.tail)) & (inner.capacity - 1)
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        let inner = self.inner.lock();
        inner.head == inner.tail
    }
}

// ---------------------------------------------------------------------------
// Memory Pool
// ---------------------------------------------------------------------------

/// Pre-allocated packet buffer pool to avoid allocation on hot path.
pub struct Mempool {
    name: String,
    buf_size: usize,
    count: usize,
    free_list: Vec<usize>,
    buffers: Vec<Vec<u8>>,
    alloc_count: u64,
    free_count: u64,
}

impl Mempool {
    fn new(name: String, buf_size: usize, count: usize) -> Self {
        let buf_size = if buf_size > MAX_PKT_SIZE { MAX_PKT_SIZE } else { buf_size };
        let mut buffers = Vec::with_capacity(count);
        let mut free_list = Vec::with_capacity(count);
        for i in 0..count {
            buffers.push(vec![0u8; buf_size]);
            free_list.push(i);
        }
        Self {
            name,
            buf_size,
            count,
            free_list,
            buffers,
            alloc_count: 0,
            free_count: 0,
        }
    }
}

struct MempoolRegistry {
    pools: Vec<Mempool>,
}

impl MempoolRegistry {
    const fn new() -> Self {
        Self { pools: Vec::new() }
    }
}

/// Create a new mempool. Returns pool id.
pub fn mempool_create(name: &str, buf_size: usize, count: usize) -> Result<u32, &'static str> {
    let mut reg = MEMPOOLS.lock();
    if reg.pools.len() >= MAX_MEMPOOLS {
        return Err("too many mempools");
    }
    let id = reg.pools.len() as u32;
    reg.pools.push(Mempool::new(String::from(name), buf_size, count));
    Ok(id)
}

/// Allocate a buffer from the mempool. Returns (pool_id, buf_idx, buf_size).
pub fn mempool_alloc(pool_id: u32) -> Result<(u32, usize, usize), &'static str> {
    let mut reg = MEMPOOLS.lock();
    let pool = reg.pools.get_mut(pool_id as usize).ok_or("mempool not found")?;
    match pool.free_list.pop() {
        Some(idx) => {
            pool.alloc_count += 1;
            Ok((pool_id, idx, pool.buf_size))
        }
        None => Err("mempool exhausted"),
    }
}

/// Free a buffer back to the mempool.
pub fn mempool_free(pool_id: u32, buf_idx: usize) -> Result<(), &'static str> {
    let mut reg = MEMPOOLS.lock();
    let pool = reg.pools.get_mut(pool_id as usize).ok_or("mempool not found")?;
    if buf_idx >= pool.count {
        return Err("invalid buffer index");
    }
    pool.free_list.push(buf_idx);
    pool.free_count += 1;
    // Zero out the buffer for reuse
    for b in pool.buffers[buf_idx].iter_mut() {
        *b = 0;
    }
    Ok(())
}

/// Get mempool statistics.
pub fn mempool_stats(pool_id: u32) -> Result<String, &'static str> {
    let reg = MEMPOOLS.lock();
    let pool = reg.pools.get(pool_id as usize).ok_or("mempool not found")?;
    let mut s = format!("Mempool '{}'\n", pool.name);
    s += &format!("  Buffer size:  {} bytes\n", pool.buf_size);
    s += &format!("  Total bufs:   {}\n", pool.count);
    s += &format!("  Free bufs:    {}\n", pool.free_list.len());
    s += &format!("  In use:       {}\n", pool.count - pool.free_list.len());
    s += &format!("  Alloc calls:  {}\n", pool.alloc_count);
    s += &format!("  Free calls:   {}\n", pool.free_count);
    Ok(s)
}

/// List all mempools info.
pub fn mempool_info() -> String {
    let reg = MEMPOOLS.lock();
    let mut s = String::from("=== Memory Pools ===\n");
    s += &format!("Pools: {}\n", reg.pools.len());
    for (i, pool) in reg.pools.iter().enumerate() {
        s += &format!("\n[Pool {}] '{}'\n", i, pool.name);
        s += &format!("  Buf size: {} bytes, Count: {}, Free: {}\n",
            pool.buf_size, pool.count, pool.free_list.len());
        s += &format!("  Allocs: {}, Frees: {}\n", pool.alloc_count, pool.free_count);
    }
    s
}

// ---------------------------------------------------------------------------
// Core Pinning
// ---------------------------------------------------------------------------

/// Pin current task to a specific CPU core.
pub fn pin_to_core(core_id: usize) {
    CORE_AFFINITY.store(core_id, Ordering::SeqCst);
}

/// Get the current core affinity.
pub fn get_core_id() -> usize {
    CORE_AFFINITY.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// Stage type for the packet processing pipeline.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PipelineStageKind {
    Rx,
    Classify,
    Process,
    Tx,
}

/// A pipeline stage with a type and core assignment.
pub struct PipelineStage {
    pub kind: PipelineStageKind,
    pub core_id: usize,
}

struct Pipeline {
    stages: Vec<PipelineStageEntry>,
    packets_processed: u64,
    active: bool,
}

struct PipelineStageEntry {
    kind: PipelineStageKind,
    core_id: usize,
    processed: u64,
}

struct PipelineRegistry {
    pipelines: Vec<Pipeline>,
}

impl PipelineRegistry {
    const fn new() -> Self {
        Self { pipelines: Vec::new() }
    }
}

/// Create a processing pipeline. Returns pipeline id.
pub fn pipeline_create(stages: &[PipelineStage]) -> Result<u32, &'static str> {
    let mut reg = PIPELINES.lock();
    if reg.pipelines.len() >= MAX_PIPELINES {
        return Err("too many pipelines");
    }
    let id = reg.pipelines.len() as u32;
    let entries: Vec<PipelineStageEntry> = stages.iter().map(|s| {
        PipelineStageEntry {
            kind: s.kind,
            core_id: s.core_id,
            processed: 0,
        }
    }).collect();
    reg.pipelines.push(Pipeline {
        stages: entries,
        packets_processed: 0,
        active: false,
    });
    Ok(id)
}

// ---------------------------------------------------------------------------
// Benchmark
// ---------------------------------------------------------------------------

/// Run a synthetic DPDK benchmark for the given number of ticks.
/// Returns a report string with PPS and throughput.
pub fn dpdk_benchmark(duration_ticks: u64) -> String {
    // Create a temporary PMD + mempool for benchmark
    let pmd_id = {
        let mut reg = PMDS.lock();
        let id = reg.pmds.len() as u32;
        reg.pmds.push(PollModeDriver::new(
            String::from("bench_pmd"),
            1, 1, MAX_BURST,
        ));
        id
    };

    // Start polling
    {
        let mut reg = PMDS.lock();
        if let Some(pmd) = reg.pmds.get_mut(pmd_id as usize) {
            pmd.polling = true;
        }
    }

    // Simulate packet processing
    let pkt_size: usize = 64; // minimum ethernet frame
    let pkts_per_cycle: u64 = MAX_BURST as u64;
    let total_cycles = duration_ticks;
    let total_pkts = total_cycles * pkts_per_cycle;
    let total_bytes = total_pkts * pkt_size as u64;

    // Update stats
    {
        let mut reg = PMDS.lock();
        if let Some(pmd) = reg.pmds.get_mut(pmd_id as usize) {
            pmd.tx_pkts += total_pkts;
            pmd.tx_bytes += total_bytes;
            pmd.poll_cycles += total_cycles;
            pmd.polling = false;
        }
    }

    TOTAL_TX_PKTS.fetch_add(total_pkts, Ordering::Relaxed);
    TOTAL_TX_BYTES.fetch_add(total_bytes, Ordering::Relaxed);
    TOTAL_POLL_CYCLES.fetch_add(total_cycles, Ordering::Relaxed);

    // Calculate PPS and throughput using integer math
    // Assume 100 ticks/sec (PIT frequency)
    let duration_sec = if duration_ticks >= 100 { duration_ticks / 100 } else { 1 };
    let pps = total_pkts / duration_sec;
    // Throughput in Mbps: (total_bytes * 8) / (duration_sec * 1_000_000)
    let throughput_mbps = (total_bytes * 8) / (duration_sec * 1_000_000);
    let throughput_gbps_int = throughput_mbps / 1000;
    let throughput_gbps_frac = (throughput_mbps % 1000) / 10; // 2 decimal digits

    let mut s = String::from("=== DPDK Benchmark Results ===\n");
    s += &format!("Duration:      {} ticks (~{} sec)\n", duration_ticks, duration_sec);
    s += &format!("Packet size:   {} bytes\n", pkt_size);
    s += &format!("Burst size:    {}\n", MAX_BURST);
    s += &format!("Total packets: {}\n", total_pkts);
    s += &format!("Total bytes:   {}\n", total_bytes);
    s += &format!("TX PPS:        {}\n", pps);
    s += &format!("Throughput:    {}.{:02} Gbps\n", throughput_gbps_int, throughput_gbps_frac);
    s += &format!("Poll cycles:   {}\n", total_cycles);
    s
}

// ---------------------------------------------------------------------------
// Init and info
// ---------------------------------------------------------------------------

/// Initialize the DPDK-style framework.
pub fn init() {
    // Create default mempool for packet buffers
    let _ = mempool_create("default_pktmbuf", 2048, 8192);

    // Create default PMDs for simulated NICs
    let _ = pmd_create("dpdk_mlx5", 4, 4, DEFAULT_BURST);
    let _ = pmd_create("dpdk_ice", 8, 8, DEFAULT_BURST);

    INITIALIZED.store(true, Ordering::SeqCst);
}

/// Return DPDK framework info.
pub fn dpdk_info() -> String {
    let pmd_reg = PMDS.lock();
    let pool_reg = MEMPOOLS.lock();
    let pipe_reg = PIPELINES.lock();

    let mut s = String::from("=== DPDK-Style Framework ===\n");
    s += &format!("PMDs:       {}\n", pmd_reg.pmds.len());
    s += &format!("Mempools:   {}\n", pool_reg.pools.len());
    s += &format!("Pipelines:  {}\n", pipe_reg.pipelines.len());
    s += &format!("Core pin:   CPU {}\n", CORE_AFFINITY.load(Ordering::Relaxed));
    s += "\nPoll Mode Drivers:\n";
    for (i, pmd) in pmd_reg.pmds.iter().enumerate() {
        s += &format!("  [{}] '{}' RXq={} TXq={} burst={} {}\n",
            i, pmd.name, pmd.num_rx_queues, pmd.num_tx_queues,
            pmd.burst_size, if pmd.polling { "POLLING" } else { "STOPPED" });
    }
    s
}

/// Return DPDK statistics.
pub fn dpdk_stats() -> String {
    let pmd_reg = PMDS.lock();
    let mut s = String::from("=== DPDK Statistics ===\n");

    for (i, pmd) in pmd_reg.pmds.iter().enumerate() {
        s += &format!("\n[PMD {}] '{}'\n", i, pmd.name);
        s += &format!("  TX pkts: {}  TX bytes: {}\n", pmd.tx_pkts, pmd.tx_bytes);
        s += &format!("  RX pkts: {}  RX bytes: {}\n", pmd.rx_pkts, pmd.rx_bytes);
        s += &format!("  Poll cycles: {}\n", pmd.poll_cycles);
    }

    s += &format!("\nGlobal: TX_pkts={} RX_pkts={} TX_bytes={} RX_bytes={} polls={}\n",
        TOTAL_TX_PKTS.load(Ordering::Relaxed),
        TOTAL_RX_PKTS.load(Ordering::Relaxed),
        TOTAL_TX_BYTES.load(Ordering::Relaxed),
        TOTAL_RX_BYTES.load(Ordering::Relaxed),
        TOTAL_POLL_CYCLES.load(Ordering::Relaxed));
    s
}

/// List all PMDs.
pub fn list_pmds() -> String {
    let reg = PMDS.lock();
    let mut s = String::from("Poll Mode Drivers:\n");
    for (i, pmd) in reg.pmds.iter().enumerate() {
        s += &format!("  [{}] '{}' - RX:{} TX:{} burst:{} [{}]\n",
            i, pmd.name, pmd.num_rx_queues, pmd.num_tx_queues,
            pmd.burst_size, if pmd.polling { "POLLING" } else { "STOPPED" });
    }
    s
}
