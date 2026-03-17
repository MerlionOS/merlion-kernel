/// High-performance NIC framework for MerlionOS.
/// Provides multi-queue support, RSS (Receive Side Scaling),
/// interrupt coalescing, jumbo frames, and 25/100GbE NIC abstractions.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of registered NICs.
const MAX_NICS: usize = 8;

/// Maximum queues per NIC.
const MAX_QUEUES: usize = 64;

/// Maximum MTU (jumbo frames).
const MAX_MTU: u16 = 9000;

/// Default MTU.
const DEFAULT_MTU: u16 = 1500;

/// RSS indirection table size.
const RSS_INDIR_SIZE: usize = 128;

/// RSS key length in bytes.
const RSS_KEY_LEN: usize = 40;

/// Maximum ring descriptor entries per queue.
const RING_SIZE: usize = 256;

/// Default coalesce: max packets per interrupt.
const DEFAULT_COALESCE_PKTS: u32 = 64;

/// Default coalesce: max delay in microseconds.
const DEFAULT_COALESCE_DELAY_US: u32 = 50;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static TOTAL_TX: AtomicU64 = AtomicU64::new(0);
static TOTAL_RX: AtomicU64 = AtomicU64::new(0);
static TOTAL_TX_BYTES: AtomicU64 = AtomicU64::new(0);
static TOTAL_RX_BYTES: AtomicU64 = AtomicU64::new(0);

static NICS: Mutex<NicRegistry> = Mutex::new(NicRegistry::new());

// ---------------------------------------------------------------------------
// Queue descriptor
// ---------------------------------------------------------------------------

/// A single TX/RX descriptor in a ring.
#[derive(Clone)]
struct Descriptor {
    addr: u64,
    len: u32,
    flags: u16,
    status: u16,
}

impl Descriptor {
    const fn empty() -> Self {
        Self { addr: 0, len: 0, flags: 0, status: 0 }
    }
}

// ---------------------------------------------------------------------------
// Queue
// ---------------------------------------------------------------------------

/// Per-queue state: descriptors, head/tail, stats, coalescing, CPU affinity.
struct Queue {
    id: u16,
    descriptors: Vec<Descriptor>,
    head: u32,
    tail: u32,
    tx_packets: u64,
    tx_bytes: u64,
    rx_packets: u64,
    rx_bytes: u64,
    drops: u64,
    cpu_affinity: u16,
    coalesce_max_pkts: u32,
    coalesce_max_delay_us: u32,
    pending_irq_pkts: u32,
}

impl Queue {
    fn new(id: u16) -> Self {
        let mut descs = Vec::with_capacity(RING_SIZE);
        for _ in 0..RING_SIZE {
            descs.push(Descriptor::empty());
        }
        Self {
            id,
            descriptors: descs,
            head: 0,
            tail: 0,
            tx_packets: 0,
            tx_bytes: 0,
            rx_packets: 0,
            rx_bytes: 0,
            drops: 0,
            cpu_affinity: id,
            coalesce_max_pkts: DEFAULT_COALESCE_PKTS,
            coalesce_max_delay_us: DEFAULT_COALESCE_DELAY_US,
            pending_irq_pkts: 0,
        }
    }

    fn enqueue_tx(&mut self, data: &[u8]) -> bool {
        let next = (self.head + 1) % RING_SIZE as u32;
        if next == self.tail {
            self.drops += 1;
            return false;
        }
        let idx = self.head as usize;
        self.descriptors[idx].len = data.len() as u32;
        self.descriptors[idx].status = 1; // pending
        self.head = next;
        self.tx_packets += 1;
        self.tx_bytes += data.len() as u64;
        self.pending_irq_pkts += 1;
        true
    }

    fn dequeue_rx(&mut self) -> Option<usize> {
        if self.head == self.tail {
            return None;
        }
        let idx = self.tail as usize;
        let len = self.descriptors[idx].len as usize;
        self.descriptors[idx].status = 0;
        self.tail = (self.tail + 1) % RING_SIZE as u32;
        self.rx_packets += 1;
        self.rx_bytes += len as u64;
        self.pending_irq_pkts += 1;
        Some(len)
    }

    fn should_fire_interrupt(&self) -> bool {
        self.pending_irq_pkts >= self.coalesce_max_pkts
    }

    fn ack_interrupt(&mut self) {
        self.pending_irq_pkts = 0;
    }
}

// ---------------------------------------------------------------------------
// RSS
// ---------------------------------------------------------------------------

/// RSS (Receive Side Scaling) configuration.
struct RssConfig {
    enabled: bool,
    key: [u8; RSS_KEY_LEN],
    indirection_table: [u16; RSS_INDIR_SIZE],
    num_queues: u16,
}

impl RssConfig {
    const fn new() -> Self {
        Self {
            enabled: false,
            key: [0; RSS_KEY_LEN],
            indirection_table: [0; RSS_INDIR_SIZE],
            num_queues: 1,
        }
    }

    fn init(&mut self, num_queues: u16) {
        self.enabled = true;
        self.num_queues = num_queues;
        // Default key — Microsoft recommended Toeplitz key
        let default_key: [u8; RSS_KEY_LEN] = [
            0x6d, 0x5a, 0x56, 0xda, 0x25, 0x5b, 0x0e, 0xc2,
            0x41, 0x67, 0x25, 0x3d, 0x43, 0xa3, 0x8f, 0xb0,
            0xd0, 0xca, 0x2b, 0xcb, 0xae, 0x7b, 0x30, 0xb4,
            0x77, 0xcb, 0x2d, 0xa3, 0x80, 0x30, 0xf2, 0x0c,
            0x6a, 0x42, 0xb7, 0x3b, 0xbe, 0xac, 0x01, 0xfa,
        ];
        self.key = default_key;
        // Round-robin indirection table
        for i in 0..RSS_INDIR_SIZE {
            self.indirection_table[i] = (i as u16) % num_queues;
        }
    }
}

/// Toeplitz hash function (integer-only, no floats).
/// Hashes the 5-tuple {src_ip, dst_ip, src_port, dst_port, proto}.
pub fn rss_hash(key: &[u8; RSS_KEY_LEN], src_ip: u32, dst_ip: u32,
                src_port: u16, dst_port: u16, proto: u8) -> u32 {
    // Build input: 4+4+2+2+1 = 13 bytes
    let input: [u8; 13] = [
        (src_ip >> 24) as u8, (src_ip >> 16) as u8,
        (src_ip >> 8) as u8, src_ip as u8,
        (dst_ip >> 24) as u8, (dst_ip >> 16) as u8,
        (dst_ip >> 8) as u8, dst_ip as u8,
        (src_port >> 8) as u8, src_port as u8,
        (dst_port >> 8) as u8, dst_port as u8,
        proto,
    ];
    toeplitz_hash(key, &input)
}

/// Toeplitz hash over arbitrary input bytes using the RSS key.
fn toeplitz_hash(key: &[u8; RSS_KEY_LEN], input: &[u8]) -> u32 {
    let mut result: u32 = 0;
    // Build initial key value from first 4 bytes
    let mut key_val: u32 = ((key[0] as u32) << 24)
        | ((key[1] as u32) << 16)
        | ((key[2] as u32) << 8)
        | (key[3] as u32);

    for i in 0..input.len() {
        let next_key_byte = if i + 4 < RSS_KEY_LEN { key[i + 4] } else { 0 };
        for bit in 0..8u32 {
            if input[i] & (1u8 << (7 - bit)) != 0 {
                result ^= key_val;
            }
            // Shift key_val left by 1, bringing in the next bit
            key_val = (key_val << 1) | ((next_key_byte as u32 >> (7 - bit)) & 1);
        }
    }
    result
}

/// Look up queue from RSS hash via indirection table.
pub fn rss_queue(indir: &[u16; RSS_INDIR_SIZE], hash: u32) -> u16 {
    let idx = (hash as usize) % RSS_INDIR_SIZE;
    indir[idx]
}

// ---------------------------------------------------------------------------
// NIC Stats
// ---------------------------------------------------------------------------

/// Per-NIC statistics.
#[derive(Clone)]
pub struct NicStats {
    pub tx_packets: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub rx_bytes: u64,
    pub drops: u64,
}

// ---------------------------------------------------------------------------
// HighSpeedNic trait
// ---------------------------------------------------------------------------

/// Trait for high-speed NIC abstractions (25/100GbE).
pub trait HighSpeedNic {
    fn name(&self) -> &str;
    fn speed_gbps(&self) -> u32;
    fn num_queues(&self) -> u32;
    fn link_up(&self) -> bool;
    fn mac_address(&self) -> [u8; 6];
    fn send(&mut self, queue: u16, data: &[u8]) -> bool;
    fn recv(&mut self, queue: u16) -> Option<Vec<u8>>;
    fn stats(&self) -> NicStats;
}

// ---------------------------------------------------------------------------
// Simulated Mellanox ConnectX-5 (25GbE, 16 queues)
// ---------------------------------------------------------------------------

struct Mlx5Sim {
    mac: [u8; 6],
    mtu: u16,
    link: bool,
    queues: Vec<Queue>,
    rss: RssConfig,
    tx_csum_offload: bool,
    rx_csum_offload: bool,
}

impl Mlx5Sim {
    fn new() -> Self {
        let mut queues = Vec::with_capacity(16);
        for i in 0..16u16 {
            queues.push(Queue::new(i));
        }
        let mut rss = RssConfig::new();
        rss.init(16);
        Self {
            mac: [0x00, 0x02, 0xc9, 0x01, 0x02, 0x03],
            mtu: DEFAULT_MTU,
            link: true,
            queues,
            rss,
            tx_csum_offload: true,
            rx_csum_offload: true,
        }
    }
}

impl HighSpeedNic for Mlx5Sim {
    fn name(&self) -> &str { "mlx5_0 (ConnectX-5 25GbE)" }
    fn speed_gbps(&self) -> u32 { 25 }
    fn num_queues(&self) -> u32 { 16 }
    fn link_up(&self) -> bool { self.link }
    fn mac_address(&self) -> [u8; 6] { self.mac }

    fn send(&mut self, queue: u16, data: &[u8]) -> bool {
        if queue >= 16 || !self.link { return false; }
        if data.len() > self.mtu as usize { return false; }
        self.queues[queue as usize].enqueue_tx(data)
    }

    fn recv(&mut self, queue: u16) -> Option<Vec<u8>> {
        if queue >= 16 { return None; }
        if let Some(len) = self.queues[queue as usize].dequeue_rx() {
            // Simulated: return synthetic packet
            let pkt = vec![0u8; len.min(64)];
            Some(pkt)
        } else {
            None
        }
    }

    fn stats(&self) -> NicStats {
        let mut s = NicStats { tx_packets: 0, tx_bytes: 0, rx_packets: 0, rx_bytes: 0, drops: 0 };
        for q in &self.queues {
            s.tx_packets += q.tx_packets;
            s.tx_bytes += q.tx_bytes;
            s.rx_packets += q.rx_packets;
            s.rx_bytes += q.rx_bytes;
            s.drops += q.drops;
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Simulated Intel E810 (100GbE, 32 queues)
// ---------------------------------------------------------------------------

struct IceSim {
    mac: [u8; 6],
    mtu: u16,
    link: bool,
    queues: Vec<Queue>,
    rss: RssConfig,
    tx_csum_offload: bool,
    rx_csum_offload: bool,
}

impl IceSim {
    fn new() -> Self {
        let mut queues = Vec::with_capacity(32);
        for i in 0..32u16 {
            queues.push(Queue::new(i));
        }
        let mut rss = RssConfig::new();
        rss.init(32);
        Self {
            mac: [0x68, 0x05, 0xca, 0x10, 0x20, 0x30],
            mtu: DEFAULT_MTU,
            link: true,
            queues,
            rss,
            tx_csum_offload: true,
            rx_csum_offload: true,
        }
    }
}

impl HighSpeedNic for IceSim {
    fn name(&self) -> &str { "ice_0 (E810 100GbE)" }
    fn speed_gbps(&self) -> u32 { 100 }
    fn num_queues(&self) -> u32 { 32 }
    fn link_up(&self) -> bool { self.link }
    fn mac_address(&self) -> [u8; 6] { self.mac }

    fn send(&mut self, queue: u16, data: &[u8]) -> bool {
        if queue >= 32 || !self.link { return false; }
        if data.len() > self.mtu as usize { return false; }
        self.queues[queue as usize].enqueue_tx(data)
    }

    fn recv(&mut self, queue: u16) -> Option<Vec<u8>> {
        if queue >= 32 { return None; }
        if let Some(len) = self.queues[queue as usize].dequeue_rx() {
            let pkt = vec![0u8; len.min(64)];
            Some(pkt)
        } else {
            None
        }
    }

    fn stats(&self) -> NicStats {
        let mut s = NicStats { tx_packets: 0, tx_bytes: 0, rx_packets: 0, rx_bytes: 0, drops: 0 };
        for q in &self.queues {
            s.tx_packets += q.tx_packets;
            s.tx_bytes += q.tx_bytes;
            s.rx_packets += q.rx_packets;
            s.rx_bytes += q.rx_bytes;
            s.drops += q.drops;
        }
        s
    }
}

// ---------------------------------------------------------------------------
// NIC Registry
// ---------------------------------------------------------------------------

struct NicEntry {
    nic_type: NicType,
}

enum NicType {
    Mlx5(Mlx5Sim),
    Ice(IceSim),
}

struct NicRegistry {
    entries: Vec<NicEntry>,
}

impl NicRegistry {
    const fn new() -> Self {
        Self { entries: Vec::new() }
    }

    fn register_mlx5(&mut self) -> usize {
        let id = self.entries.len();
        self.entries.push(NicEntry { nic_type: NicType::Mlx5(Mlx5Sim::new()) });
        id
    }

    fn register_ice(&mut self) -> usize {
        let id = self.entries.len();
        self.entries.push(NicEntry { nic_type: NicType::Ice(IceSim::new()) });
        id
    }
}

// ---------------------------------------------------------------------------
// MTU management
// ---------------------------------------------------------------------------

/// Set the MTU for a NIC.
pub fn set_mtu(nic_id: usize, mtu: u16) -> Result<(), &'static str> {
    if mtu < 68 || mtu > MAX_MTU {
        return Err("MTU must be between 68 and 9000");
    }
    let mut reg = NICS.lock();
    let entry = reg.entries.get_mut(nic_id).ok_or("NIC not found")?;
    match &mut entry.nic_type {
        NicType::Mlx5(nic) => nic.mtu = mtu,
        NicType::Ice(nic) => nic.mtu = mtu,
    }
    Ok(())
}

/// Get the MTU for a NIC.
pub fn get_mtu(nic_id: usize) -> Result<u16, &'static str> {
    let reg = NICS.lock();
    let entry = reg.entries.get(nic_id).ok_or("NIC not found")?;
    match &entry.nic_type {
        NicType::Mlx5(nic) => Ok(nic.mtu),
        NicType::Ice(nic) => Ok(nic.mtu),
    }
}

// ---------------------------------------------------------------------------
// Interrupt Coalescing
// ---------------------------------------------------------------------------

/// Set coalescing parameters for a specific queue on a NIC.
pub fn set_coalesce(nic_id: usize, queue: u16, max_pkts: u32, max_delay_us: u32) -> Result<(), &'static str> {
    let mut reg = NICS.lock();
    let entry = reg.entries.get_mut(nic_id).ok_or("NIC not found")?;
    let queues = match &mut entry.nic_type {
        NicType::Mlx5(nic) => &mut nic.queues,
        NicType::Ice(nic) => &mut nic.queues,
    };
    let q = queues.get_mut(queue as usize).ok_or("queue not found")?;
    q.coalesce_max_pkts = max_pkts;
    q.coalesce_max_delay_us = max_delay_us;
    Ok(())
}

// ---------------------------------------------------------------------------
// Checksum offload
// ---------------------------------------------------------------------------

/// Enable/disable TX checksum offload.
pub fn offload_tx_csum(nic_id: usize, enabled: bool) -> Result<(), &'static str> {
    let mut reg = NICS.lock();
    let entry = reg.entries.get_mut(nic_id).ok_or("NIC not found")?;
    match &mut entry.nic_type {
        NicType::Mlx5(nic) => nic.tx_csum_offload = enabled,
        NicType::Ice(nic) => nic.tx_csum_offload = enabled,
    }
    Ok(())
}

/// Enable/disable RX checksum offload.
pub fn offload_rx_csum(nic_id: usize, enabled: bool) -> Result<(), &'static str> {
    let mut reg = NICS.lock();
    let entry = reg.entries.get_mut(nic_id).ok_or("NIC not found")?;
    match &mut entry.nic_type {
        NicType::Mlx5(nic) => nic.rx_csum_offload = enabled,
        NicType::Ice(nic) => nic.rx_csum_offload = enabled,
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Queue-to-CPU affinity
// ---------------------------------------------------------------------------

/// Set the CPU affinity for a queue.
pub fn set_queue_affinity(nic_id: usize, queue: u16, cpu: u16) -> Result<(), &'static str> {
    let mut reg = NICS.lock();
    let entry = reg.entries.get_mut(nic_id).ok_or("NIC not found")?;
    let queues = match &mut entry.nic_type {
        NicType::Mlx5(nic) => &mut nic.queues,
        NicType::Ice(nic) => &mut nic.queues,
    };
    let q = queues.get_mut(queue as usize).ok_or("queue not found")?;
    q.cpu_affinity = cpu;
    Ok(())
}

// ---------------------------------------------------------------------------
// Scatter-gather descriptor
// ---------------------------------------------------------------------------

/// A scatter-gather entry for jumbo frame support.
pub struct SgEntry {
    pub addr: u64,
    pub len: u32,
}

/// Build a scatter-gather list for a large frame.
pub fn build_sg_list(frame: &[u8], max_seg: usize) -> Vec<SgEntry> {
    let mut list = Vec::new();
    let mut offset = 0usize;
    while offset < frame.len() {
        let seg_len = (frame.len() - offset).min(max_seg);
        list.push(SgEntry {
            addr: (frame.as_ptr() as u64) + offset as u64,
            len: seg_len as u32,
        });
        offset += seg_len;
    }
    list
}

// ---------------------------------------------------------------------------
// Init and info
// ---------------------------------------------------------------------------

/// Initialize the HP networking subsystem.
pub fn init() {
    let mut reg = NICS.lock();
    reg.register_mlx5();
    reg.register_ice();
    INITIALIZED.store(true, Ordering::SeqCst);
}

/// Return information about the HP NIC framework.
pub fn hpnet_info() -> String {
    let reg = NICS.lock();
    let mut s = String::from("=== High-Performance NIC Framework ===\n");
    s += &format!("Registered NICs: {}\n", reg.entries.len());
    for (i, entry) in reg.entries.iter().enumerate() {
        let (name, speed, nq, link, mac, mtu, tx_csum, rx_csum) = match &entry.nic_type {
            NicType::Mlx5(nic) => (
                nic.name(), nic.speed_gbps(), nic.num_queues(), nic.link_up(),
                nic.mac_address(), nic.mtu, nic.tx_csum_offload, nic.rx_csum_offload,
            ),
            NicType::Ice(nic) => (
                nic.name(), nic.speed_gbps(), nic.num_queues(), nic.link_up(),
                nic.mac_address(), nic.mtu, nic.tx_csum_offload, nic.rx_csum_offload,
            ),
        };
        s += &format!("\n[NIC {}] {}\n", i, name);
        s += &format!("  Speed:     {} Gbps\n", speed);
        s += &format!("  Queues:    {}\n", nq);
        s += &format!("  Link:      {}\n", if link { "UP" } else { "DOWN" });
        s += &format!("  MAC:       {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\n",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
        s += &format!("  MTU:       {}\n", mtu);
        s += &format!("  TX csum:   {}\n", if tx_csum { "offload" } else { "software" });
        s += &format!("  RX csum:   {}\n", if rx_csum { "offload" } else { "software" });
    }
    s
}

/// Return RSS configuration info.
pub fn rss_info() -> String {
    let reg = NICS.lock();
    let mut s = String::from("=== RSS (Receive Side Scaling) ===\n");
    for (i, entry) in reg.entries.iter().enumerate() {
        let (name, rss) = match &entry.nic_type {
            NicType::Mlx5(nic) => (nic.name(), &nic.rss),
            NicType::Ice(nic) => (nic.name(), &nic.rss),
        };
        s += &format!("\n[NIC {}] {}\n", i, name);
        s += &format!("  RSS enabled:  {}\n", rss.enabled);
        s += &format!("  Num queues:   {}\n", rss.num_queues);
        s += &format!("  Key (first 8): {:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}\n",
            rss.key[0], rss.key[1], rss.key[2], rss.key[3],
            rss.key[4], rss.key[5], rss.key[6], rss.key[7]);
        s += &format!("  Indir[0..7]:  ");
        for j in 0..8 {
            s += &format!("{} ", rss.indirection_table[j]);
        }
        s += "\n";
    }
    s
}

/// Return NIC statistics.
pub fn hpnet_stats() -> String {
    let reg = NICS.lock();
    let mut s = String::from("=== HP NIC Statistics ===\n");
    for (i, entry) in reg.entries.iter().enumerate() {
        let (name, stats) = match &entry.nic_type {
            NicType::Mlx5(nic) => (nic.name(), nic.stats()),
            NicType::Ice(nic) => (nic.name(), nic.stats()),
        };
        s += &format!("\n[NIC {}] {}\n", i, name);
        s += &format!("  TX packets: {}  TX bytes: {}\n", stats.tx_packets, stats.tx_bytes);
        s += &format!("  RX packets: {}  RX bytes: {}\n", stats.rx_packets, stats.rx_bytes);
        s += &format!("  Drops:      {}\n", stats.drops);
    }
    s += &format!("\nGlobal: TX={} RX={} TX_bytes={} RX_bytes={}\n",
        TOTAL_TX.load(Ordering::Relaxed),
        TOTAL_RX.load(Ordering::Relaxed),
        TOTAL_TX_BYTES.load(Ordering::Relaxed),
        TOTAL_RX_BYTES.load(Ordering::Relaxed));
    s
}

/// List all registered NICs.
pub fn list_nics() -> String {
    let reg = NICS.lock();
    let mut s = String::from("Registered NICs:\n");
    for (i, entry) in reg.entries.iter().enumerate() {
        let (name, speed, link) = match &entry.nic_type {
            NicType::Mlx5(nic) => (nic.name(), nic.speed_gbps(), nic.link_up()),
            NicType::Ice(nic) => (nic.name(), nic.speed_gbps(), nic.link_up()),
        };
        s += &format!("  [{}] {} - {} Gbps [{}]\n", i, name, speed,
            if link { "UP" } else { "DOWN" });
    }
    s
}
