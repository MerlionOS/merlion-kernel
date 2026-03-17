/// Traffic control (tc) for MerlionOS.
/// Implements token bucket rate limiting, queueing disciplines (FIFO, SFQ, HTB),
/// priority queuing, and per-flow bandwidth management.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of traffic classes per interface.
const MAX_CLASSES: usize = 64;

/// Maximum number of filters.
const MAX_FILTERS: usize = 128;

/// Maximum packets per queue.
const MAX_QUEUE_LEN: usize = 256;

/// Number of SFQ hash buckets.
const SFQ_BUCKETS: usize = 16;

/// Number of priority bands for PRIO qdisc.
const PRIO_BANDS: usize = 8;

/// Default token bucket size (bytes).
const DEFAULT_BUCKET_SIZE: u64 = 65536;

/// Tick rate (ticks per second) for token refill calculations.
const TICKS_PER_SEC: u64 = 100;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Queueing discipline type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QdiscType {
    /// Simple first-in-first-out, drop-tail when full.
    Fifo,
    /// Stochastic Fair Queueing — hash flows into buckets, round-robin.
    Sfq,
    /// Hierarchical Token Bucket — tree of classes with rate/ceil limits.
    Htb,
    /// Priority queueing — 8 bands, strict priority scheduling.
    Prio,
}

/// Per-class statistics.
#[derive(Debug, Clone)]
pub struct ClassStats {
    pub bytes_sent: u64,
    pub packets_sent: u64,
    pub bytes_dropped: u64,
    pub packets_dropped: u64,
    pub overlimits: u64,
}

impl ClassStats {
    const fn new() -> Self {
        Self {
            bytes_sent: 0,
            packets_sent: 0,
            bytes_dropped: 0,
            packets_dropped: 0,
            overlimits: 0,
        }
    }
}

/// Token bucket rate limiter.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    /// Current available tokens (bytes).
    pub tokens: u64,
    /// Maximum bucket capacity (bytes).
    pub bucket_size: u64,
    /// Refill rate in bytes per second.
    pub rate_bps: u64,
    /// Last refill tick.
    last_tick: u64,
}

impl TokenBucket {
    /// Create a new token bucket starting full.
    pub fn new(rate_bps: u64, bucket_size: u64) -> Self {
        Self {
            tokens: bucket_size,
            bucket_size,
            rate_bps,
            last_tick: crate::timer::ticks(),
        }
    }

    /// Try to consume `bytes` tokens. Returns `true` if sufficient tokens
    /// were available (and consumed), `false` if rate-limited.
    pub fn consume(&mut self, bytes: u64) -> bool {
        self.refill();
        if self.tokens >= bytes {
            self.tokens -= bytes;
            true
        } else {
            false
        }
    }

    /// Refill tokens based on elapsed time. Uses integer math only.
    pub fn refill(&mut self) {
        let now = crate::timer::ticks();
        if now <= self.last_tick {
            return;
        }
        let elapsed = now - self.last_tick;
        self.last_tick = now;
        // tokens += rate * elapsed_ticks / TICKS_PER_SEC
        let added = self.rate_bps * elapsed / TICKS_PER_SEC;
        self.tokens += added;
        if self.tokens > self.bucket_size {
            self.tokens = self.bucket_size;
        }
    }
}

/// A traffic class with bandwidth guarantee, ceiling, and queued packets.
#[derive(Debug, Clone)]
pub struct TrafficClass {
    /// Unique class identifier.
    pub id: u32,
    /// Parent class (None = root).
    pub parent: Option<u32>,
    /// Guaranteed rate (bytes per second).
    pub rate_bps: u64,
    /// Maximum rate / burst ceiling (bytes per second).
    pub ceil_bps: u64,
    /// Priority (0 = highest, 7 = lowest).
    pub priority: u8,
    /// Queueing discipline for this class.
    pub qdisc: QdiscType,
    /// Packet queue.
    pub queue: Vec<Vec<u8>>,
    /// Token bucket for rate enforcement.
    pub bucket: TokenBucket,
    /// Per-class statistics.
    pub stats: ClassStats,
}

impl TrafficClass {
    /// Create a new traffic class.
    pub fn new(id: u32, parent: Option<u32>, rate_bps: u64, ceil_bps: u64,
               priority: u8, qdisc: QdiscType) -> Self {
        let bucket_size = if ceil_bps > 0 { ceil_bps } else { DEFAULT_BUCKET_SIZE };
        Self {
            id,
            parent,
            rate_bps,
            ceil_bps,
            priority,
            qdisc,
            queue: Vec::new(),
            bucket: TokenBucket::new(rate_bps, bucket_size),
            stats: ClassStats::new(),
        }
    }

    /// Enqueue a packet. Returns false if queue is full (drop-tail).
    pub fn enqueue(&mut self, packet: Vec<u8>) -> bool {
        if self.queue.len() >= MAX_QUEUE_LEN {
            self.stats.bytes_dropped += packet.len() as u64;
            self.stats.packets_dropped += 1;
            return false;
        }
        self.queue.push(packet);
        true
    }

    /// Dequeue next packet if tokens are available.
    pub fn dequeue(&mut self) -> Option<Vec<u8>> {
        if self.queue.is_empty() {
            return None;
        }
        let pkt_len = self.queue[0].len() as u64;
        if self.bucket.consume(pkt_len) {
            let pkt = self.queue.remove(0);
            self.stats.bytes_sent += pkt.len() as u64;
            self.stats.packets_sent += 1;
            Some(pkt)
        } else {
            self.stats.overlimits += 1;
            None
        }
    }

    /// Queue depth.
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }
}

/// Filter/classifier to match packets to traffic classes.
#[derive(Debug, Clone)]
pub struct TcFilter {
    /// Auto-assigned filter id.
    pub id: u32,
    /// Protocol number (6=TCP, 17=UDP, 1=ICMP).
    pub protocol: Option<u8>,
    /// Source port filter.
    pub src_port: Option<u16>,
    /// Destination port filter.
    pub dst_port: Option<u16>,
    /// Source IP filter.
    pub src_ip: Option<[u8; 4]>,
    /// DSCP value filter.
    pub dscp: Option<u8>,
    /// Target class id to enqueue matching packets.
    pub target_class: u32,
}

impl TcFilter {
    /// Check if a packet matches this filter. Extracts fields from raw IP packet.
    pub fn matches(&self, packet: &[u8]) -> bool {
        if packet.len() < 20 {
            return false;
        }
        // Check protocol
        let pkt_proto = packet[9];
        if let Some(proto) = self.protocol {
            if proto != pkt_proto {
                return false;
            }
        }
        // Check source IP (bytes 12..16)
        if let Some(sip) = self.src_ip {
            if packet.len() < 16 || packet[12..16] != sip {
                return false;
            }
        }
        // Check DSCP (TOS field byte 1, upper 6 bits)
        if let Some(dscp) = self.dscp {
            let tos = packet[1];
            if (tos >> 2) != dscp {
                return false;
            }
        }
        // Check ports (TCP/UDP: src=bytes 20..22, dst=bytes 22..24)
        if pkt_proto == 6 || pkt_proto == 17 {
            if packet.len() >= 24 {
                let s_port = u16::from_be_bytes([packet[20], packet[21]]);
                let d_port = u16::from_be_bytes([packet[22], packet[23]]);
                if let Some(sp) = self.src_port {
                    if sp != s_port { return false; }
                }
                if let Some(dp) = self.dst_port {
                    if dp != d_port { return false; }
                }
            } else {
                // Packet too short for port check but port filter set
                if self.src_port.is_some() || self.dst_port.is_some() {
                    return false;
                }
            }
        } else if self.src_port.is_some() || self.dst_port.is_some() {
            return false;
        }
        true
    }
}

/// SFQ (Stochastic Fair Queueing) state.
#[derive(Debug, Clone)]
struct SfqState {
    buckets: Vec<Vec<Vec<u8>>>,
    current_bucket: usize,
}

impl SfqState {
    fn new() -> Self {
        let mut buckets = Vec::with_capacity(SFQ_BUCKETS);
        for _ in 0..SFQ_BUCKETS {
            buckets.push(Vec::new());
        }
        Self { buckets, current_bucket: 0 }
    }

    /// Hash a packet to a bucket index using source/dest IP+port.
    fn hash_packet(packet: &[u8]) -> usize {
        if packet.len() < 20 {
            return 0;
        }
        let mut h: u32 = 0;
        // Mix src IP
        for i in 12..16 {
            h = h.wrapping_mul(31).wrapping_add(packet[i] as u32);
        }
        // Mix dst IP
        for i in 16..20 {
            h = h.wrapping_mul(31).wrapping_add(packet[i] as u32);
        }
        // Mix ports if available
        if packet.len() >= 24 {
            for i in 20..24 {
                h = h.wrapping_mul(31).wrapping_add(packet[i] as u32);
            }
        }
        (h as usize) % SFQ_BUCKETS
    }

    fn enqueue(&mut self, packet: Vec<u8>) -> bool {
        let idx = Self::hash_packet(&packet);
        if self.buckets[idx].len() >= MAX_QUEUE_LEN / SFQ_BUCKETS {
            return false;
        }
        self.buckets[idx].push(packet);
        true
    }

    fn dequeue(&mut self) -> Option<Vec<u8>> {
        // Round-robin across non-empty buckets
        for _ in 0..SFQ_BUCKETS {
            let idx = self.current_bucket;
            self.current_bucket = (self.current_bucket + 1) % SFQ_BUCKETS;
            if !self.buckets[idx].is_empty() {
                return Some(self.buckets[idx].remove(0));
            }
        }
        None
    }

    fn total_queued(&self) -> usize {
        self.buckets.iter().map(|b| b.len()).sum()
    }
}

/// Priority queueing state (8 bands).
#[derive(Debug, Clone)]
struct PrioState {
    bands: Vec<Vec<Vec<u8>>>,
}

impl PrioState {
    fn new() -> Self {
        let mut bands = Vec::with_capacity(PRIO_BANDS);
        for _ in 0..PRIO_BANDS {
            bands.push(Vec::new());
        }
        Self { bands }
    }

    fn enqueue(&mut self, packet: Vec<u8>, priority: u8) -> bool {
        let band = (priority as usize).min(PRIO_BANDS - 1);
        if self.bands[band].len() >= MAX_QUEUE_LEN / PRIO_BANDS {
            return false;
        }
        self.bands[band].push(packet);
        true
    }

    /// Dequeue from highest priority (band 0) first.
    fn dequeue(&mut self) -> Option<Vec<u8>> {
        for band in self.bands.iter_mut() {
            if !band.is_empty() {
                return Some(band.remove(0));
            }
        }
        None
    }

    fn total_queued(&self) -> usize {
        self.bands.iter().map(|b| b.len()).sum()
    }
}

/// Per-interface traffic control attachment.
struct IfaceTc {
    /// Interface name.
    name: String,
    /// Root qdisc type.
    qdisc: QdiscType,
    /// Traffic classes.
    classes: Vec<TrafficClass>,
    /// Packet filters.
    filters: Vec<TcFilter>,
    /// SFQ state (if qdisc == Sfq).
    sfq: Option<SfqState>,
    /// Priority state (if qdisc == Prio).
    prio: Option<PrioState>,
    /// Next class id.
    next_class_id: u32,
    /// Next filter id.
    next_filter_id: u32,
}

impl IfaceTc {
    fn new(name: String, qdisc: QdiscType) -> Self {
        let sfq = if qdisc == QdiscType::Sfq { Some(SfqState::new()) } else { None };
        let prio = if qdisc == QdiscType::Prio { Some(PrioState::new()) } else { None };
        Self {
            name,
            qdisc,
            classes: Vec::new(),
            filters: Vec::new(),
            sfq,
            prio,
            next_class_id: 1,
            next_filter_id: 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Global traffic control state, protected by spinlock.
static TC_STATE: Mutex<Option<TcGlobal>> = Mutex::new(None);

/// Global packet counters.
static TC_ENQUEUED: AtomicU64 = AtomicU64::new(0);
static TC_DEQUEUED: AtomicU64 = AtomicU64::new(0);
static TC_DROPPED: AtomicU64 = AtomicU64::new(0);

struct TcGlobal {
    interfaces: Vec<IfaceTc>,
}

impl TcGlobal {
    fn new() -> Self {
        Self { interfaces: Vec::new() }
    }

    fn find_iface(&self, name: &str) -> Option<usize> {
        self.interfaces.iter().position(|i| i.name == name)
    }

    fn find_iface_mut(&mut self, name: &str) -> Option<&mut IfaceTc> {
        self.interfaces.iter_mut().find(|i| i.name == name)
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the traffic control subsystem.
pub fn init() {
    *TC_STATE.lock() = Some(TcGlobal::new());
    // Create a default tc attachment for eth0
    tc_add_qdisc("eth0", QdiscType::Htb);
}

/// Attach a qdisc to a network interface.
pub fn tc_add_qdisc(iface: &str, qdisc: QdiscType) {
    let mut g = TC_STATE.lock();
    let g = match g.as_mut() { Some(g) => g, None => return };
    // Remove existing if any
    if let Some(idx) = g.find_iface(iface) {
        g.interfaces.remove(idx);
    }
    g.interfaces.push(IfaceTc::new(String::from(iface), qdisc));
}

/// Add a traffic class to an interface. Returns class id.
pub fn tc_add_class(iface: &str, parent: Option<u32>, rate_bps: u64,
                    ceil_bps: u64, priority: u8) -> Option<u32> {
    let mut g = TC_STATE.lock();
    let g = match g.as_mut() { Some(g) => g, None => return None };
    let itc = match g.find_iface_mut(iface) { Some(i) => i, None => return None };
    if itc.classes.len() >= MAX_CLASSES {
        return None;
    }
    let id = itc.next_class_id;
    itc.next_class_id += 1;
    let class = TrafficClass::new(id, parent, rate_bps, ceil_bps, priority, itc.qdisc);
    itc.classes.push(class);
    Some(id)
}

/// Add a filter to classify packets into a class. Returns filter id.
pub fn tc_add_filter(iface: &str, filter: TcFilter) -> Option<u32> {
    let mut g = TC_STATE.lock();
    let g = match g.as_mut() { Some(g) => g, None => return None };
    let itc = match g.find_iface_mut(iface) { Some(i) => i, None => return None };
    if itc.filters.len() >= MAX_FILTERS {
        return None;
    }
    let id = itc.next_filter_id;
    itc.next_filter_id += 1;
    let mut f = filter;
    f.id = id;
    itc.filters.push(f);
    Some(id)
}

/// Remove a qdisc (and all classes/filters) from an interface.
pub fn tc_del_qdisc(iface: &str) -> bool {
    let mut g = TC_STATE.lock();
    let g = match g.as_mut() { Some(g) => g, None => return false };
    if let Some(idx) = g.find_iface(iface) {
        g.interfaces.remove(idx);
        true
    } else {
        false
    }
}

/// Remove a traffic class by id.
pub fn tc_del_class(iface: &str, class_id: u32) -> bool {
    let mut g = TC_STATE.lock();
    let g = match g.as_mut() { Some(g) => g, None => return false };
    let itc = match g.find_iface_mut(iface) { Some(i) => i, None => return false };
    if let Some(pos) = itc.classes.iter().position(|c| c.id == class_id) {
        itc.classes.remove(pos);
        true
    } else {
        false
    }
}

/// Remove a filter by id.
pub fn tc_del_filter(iface: &str, filter_id: u32) -> bool {
    let mut g = TC_STATE.lock();
    let g = match g.as_mut() { Some(g) => g, None => return false };
    let itc = match g.find_iface_mut(iface) { Some(i) => i, None => return false };
    if let Some(pos) = itc.filters.iter().position(|f| f.id == filter_id) {
        itc.filters.remove(pos);
        true
    } else {
        false
    }
}

/// Classify and enqueue a packet on an interface.
pub fn tc_enqueue(iface: &str, packet: Vec<u8>) -> Result<(), &'static str> {
    let mut g = TC_STATE.lock();
    let g = match g.as_mut() { Some(g) => g, None => return Err("tc not initialised") };
    let itc = match g.find_iface_mut(iface) { Some(i) => i, None => return Err("interface not found") };

    // Find matching filter
    let target_class = itc.filters.iter().find(|f| f.matches(&packet)).map(|f| f.target_class);

    let ok = match itc.qdisc {
        QdiscType::Fifo => {
            // Enqueue to first class or default FIFO behaviour
            if let Some(cid) = target_class {
                if let Some(cls) = itc.classes.iter_mut().find(|c| c.id == cid) {
                    cls.enqueue(packet)
                } else {
                    // Class not found, try first class
                    if let Some(cls) = itc.classes.first_mut() {
                        cls.enqueue(packet)
                    } else {
                        false
                    }
                }
            } else if let Some(cls) = itc.classes.first_mut() {
                cls.enqueue(packet)
            } else {
                false
            }
        }
        QdiscType::Sfq => {
            if let Some(ref mut sfq) = itc.sfq {
                sfq.enqueue(packet)
            } else {
                false
            }
        }
        QdiscType::Htb => {
            // HTB: classify packet and enqueue to matching class
            if let Some(cid) = target_class {
                if let Some(cls) = itc.classes.iter_mut().find(|c| c.id == cid) {
                    cls.enqueue(packet)
                } else if let Some(cls) = itc.classes.first_mut() {
                    cls.enqueue(packet)
                } else {
                    false
                }
            } else {
                // No filter match — enqueue to lowest priority class
                let min_prio = itc.classes.iter().map(|c| c.priority).max();
                if let Some(mp) = min_prio {
                    if let Some(cls) = itc.classes.iter_mut().find(|c| c.priority == mp) {
                        cls.enqueue(packet)
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        }
        QdiscType::Prio => {
            if let Some(ref mut prio) = itc.prio {
                // Use DSCP to determine priority band
                let band = if packet.len() >= 2 {
                    let dscp = packet[1] >> 2;
                    // Map DSCP to priority band (higher DSCP = higher priority = lower band)
                    if dscp >= 46 { 0 }       // EF -> band 0
                    else if dscp >= 32 { 1 }   // CS4+ -> band 1
                    else if dscp >= 24 { 2 }   // AF3x -> band 2
                    else if dscp >= 16 { 3 }   // AF2x -> band 3
                    else if dscp >= 8 { 4 }    // AF1x/CS1 -> band 4
                    else { 5 }                 // BE -> band 5
                } else {
                    7 // Unknown -> lowest priority
                };
                prio.enqueue(packet, band)
            } else {
                false
            }
        }
    };

    if ok {
        TC_ENQUEUED.fetch_add(1, Ordering::Relaxed);
        Ok(())
    } else {
        TC_DROPPED.fetch_add(1, Ordering::Relaxed);
        Err("queue full or no class")
    }
}

/// Dequeue the next packet from an interface according to qdisc rules.
pub fn tc_dequeue(iface: &str) -> Option<Vec<u8>> {
    let mut g = TC_STATE.lock();
    let g = match g.as_mut() { Some(g) => g, None => return None };
    let itc = match g.find_iface_mut(iface) { Some(i) => i, None => return None };

    let pkt = match itc.qdisc {
        QdiscType::Fifo => {
            // Dequeue from first class with data
            itc.classes.iter_mut().find_map(|cls| cls.dequeue())
        }
        QdiscType::Sfq => {
            itc.sfq.as_mut().and_then(|sfq| sfq.dequeue())
        }
        QdiscType::Htb => {
            // HTB: serve classes by priority, check token bucket
            // Sort by priority (lower number = higher priority)
            let mut order: Vec<usize> = (0..itc.classes.len()).collect();
            order.sort_by_key(|&i| itc.classes[i].priority);
            let mut result = None;
            for idx in order {
                if let Some(pkt) = itc.classes[idx].dequeue() {
                    result = Some(pkt);
                    break;
                }
            }
            result
        }
        QdiscType::Prio => {
            itc.prio.as_mut().and_then(|prio| prio.dequeue())
        }
    };

    if pkt.is_some() {
        TC_DEQUEUED.fetch_add(1, Ordering::Relaxed);
    }
    pkt
}

/// Show tc qdisc and class configuration for an interface.
pub fn tc_show(iface: &str) -> String {
    let g = TC_STATE.lock();
    let g = match g.as_ref() { Some(g) => g, None => return String::from("(tc not initialised)\n") };
    let itc = match g.interfaces.iter().find(|i| i.name == iface) {
        Some(i) => i,
        None => return format!("(no tc attached to {})\n", iface),
    };

    let mut out = String::new();
    out.push_str(&format!("qdisc {:?} dev {}\n", itc.qdisc, itc.name));
    out.push_str(&format!("  classes: {}  filters: {}\n\n", itc.classes.len(), itc.filters.len()));

    if !itc.classes.is_empty() {
        out.push_str("ID   PARENT RATE(Bps)    CEIL(Bps)    PRI  QDISC  QUEUED\n");
        out.push_str("---- ------ ------------ ------------ ---- ------ ------\n");
        for cls in &itc.classes {
            let parent = match cls.parent {
                Some(p) => format!("{}", p),
                None => String::from("root"),
            };
            out.push_str(&format!(
                "{:<4} {:<6} {:<12} {:<12} {:<4} {:?}{}{}\n",
                cls.id, parent, cls.rate_bps, cls.ceil_bps,
                cls.priority, cls.qdisc,
                if cls.qdisc == QdiscType::Htb { "" } else { "" },
                format!("  {}", cls.queue_len()),
            ));
        }
    }

    if !itc.filters.is_empty() {
        out.push_str("\nFilters:\n");
        out.push_str("ID   PROTO  SRC_PORT DST_PORT SRC_IP           DSCP CLASS\n");
        out.push_str("---- ------ -------- -------- ---------------- ---- -----\n");
        for f in &itc.filters {
            let proto = match f.protocol {
                Some(6) => "TCP   ",
                Some(17) => "UDP   ",
                Some(1) => "ICMP  ",
                _ => "*     ",
            };
            let sp = match f.src_port { Some(p) => format!("{}", p), None => String::from("*") };
            let dp = match f.dst_port { Some(p) => format!("{}", p), None => String::from("*") };
            let sip = match f.src_ip {
                Some(ip) => format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]),
                None => String::from("*"),
            };
            let dscp = match f.dscp { Some(d) => format!("{}", d), None => String::from("*") };
            out.push_str(&format!(
                "{:<4} {} {:<8} {:<8} {:<16} {:<4} {}\n",
                f.id, proto, sp, dp, sip, dscp, f.target_class,
            ));
        }
    }

    out
}

/// Return global traffic control statistics.
pub fn tc_stats() -> String {
    let enq = TC_ENQUEUED.load(Ordering::Relaxed);
    let deq = TC_DEQUEUED.load(Ordering::Relaxed);
    let drp = TC_DROPPED.load(Ordering::Relaxed);

    let mut out = format!(
        "Traffic Control Statistics\n\
         ─────────────────────────\n\
         Enqueued:  {}\n\
         Dequeued:  {}\n\
         Dropped:   {}\n\n",
        enq, deq, drp,
    );

    let g = TC_STATE.lock();
    if let Some(ref g) = *g {
        for itc in &g.interfaces {
            out.push_str(&format!("Interface: {} (qdisc: {:?})\n", itc.name, itc.qdisc));
            for cls in &itc.classes {
                out.push_str(&format!(
                    "  class {}: sent={}/{} dropped={}/{} overlimits={} queued={}\n",
                    cls.id,
                    cls.stats.bytes_sent, cls.stats.packets_sent,
                    cls.stats.bytes_dropped, cls.stats.packets_dropped,
                    cls.stats.overlimits,
                    cls.queue_len(),
                ));
            }
            // SFQ stats
            if let Some(ref sfq) = itc.sfq {
                out.push_str(&format!("  SFQ: {} packets queued across {} buckets\n",
                    sfq.total_queued(), SFQ_BUCKETS));
            }
            // Prio stats
            if let Some(ref prio) = itc.prio {
                out.push_str(&format!("  PRIO: {} packets queued across {} bands\n",
                    prio.total_queued(), PRIO_BANDS));
            }
        }
    }

    out
}

/// Return summary info about the tc subsystem.
pub fn tc_info() -> String {
    let g = TC_STATE.lock();
    let g = match g.as_ref() {
        Some(g) => g,
        None => return String::from("Traffic control not initialised\n"),
    };

    let mut out = String::from("Traffic Control (tc) — MerlionOS QoS\n");
    out.push_str("─────────────────────────────────────\n");
    out.push_str(&format!("Interfaces with tc: {}\n", g.interfaces.len()));
    out.push_str("Supported qdiscs: FIFO, SFQ, HTB, PRIO\n\n");

    for itc in &g.interfaces {
        let total_classes = itc.classes.len();
        let total_filters = itc.filters.len();
        out.push_str(&format!(
            "  {} : qdisc={:?}  classes={}  filters={}\n",
            itc.name, itc.qdisc, total_classes, total_filters,
        ));
    }
    out
}
