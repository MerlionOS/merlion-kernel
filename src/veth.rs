/// Virtual Ethernet (veth) pairs for MerlionOS containers.
/// Creates paired virtual network interfaces where packets sent to one
/// end appear on the other, enabling container-to-host and container-to-container networking.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use crate::net::{Ipv4Addr, MacAddr};

/// Maximum number of veth pairs that can exist simultaneously.
const MAX_VETH_PAIRS: usize = 32;

/// Maximum packets buffered per interface before drops.
const MAX_QUEUE_DEPTH: usize = 256;

/// Maximum NAT table entries.
const MAX_NAT_ENTRIES: usize = 128;

/// Global next pair ID counter.
static NEXT_PAIR_ID: AtomicU64 = AtomicU64::new(1);

/// Global veth pair manager.
static VETH_MANAGER: Mutex<VethManager> = Mutex::new(VethManager::new());

/// Global NAT table for container egress.
static NAT: Mutex<NatTable> = Mutex::new(NatTable::new());

// ---------------------------------------------------------------------------
// Veth interface
// ---------------------------------------------------------------------------

/// A single end of a veth pair.
#[derive(Clone)]
pub struct VethInterface {
    /// Human-readable name (e.g. "veth0", "ceth0").
    pub name: String,
    /// MAC address assigned to this end.
    pub mac: MacAddr,
    /// IPv4 address assigned to this end.
    pub ip: Ipv4Addr,
    /// Packets waiting to be received on this interface.
    queue: Vec<VethPacket>,
    /// Total packets transmitted.
    pub tx_packets: u64,
    /// Total bytes transmitted.
    pub tx_bytes: u64,
    /// Total packets received (dequeued).
    pub rx_packets: u64,
    /// Total bytes received (dequeued).
    pub rx_bytes: u64,
    /// Packets dropped due to full queue.
    pub drops: u64,
    /// Whether the link is administratively up.
    pub link_up: bool,
}

/// A packet travelling through the veth pair.
#[derive(Clone, Debug)]
pub struct VethPacket {
    pub src_mac: MacAddr,
    pub dst_mac: MacAddr,
    pub src_ip: Ipv4Addr,
    pub dst_ip: Ipv4Addr,
    pub data: Vec<u8>,
}

impl VethInterface {
    fn new(name: String, mac: MacAddr, ip: Ipv4Addr) -> Self {
        Self {
            name,
            mac,
            ip,
            queue: Vec::new(),
            tx_packets: 0,
            tx_bytes: 0,
            rx_packets: 0,
            rx_bytes: 0,
            drops: 0,
            link_up: true,
        }
    }

    /// Enqueue a packet onto this interface's receive queue.
    fn enqueue(&mut self, pkt: VethPacket) -> bool {
        if self.queue.len() >= MAX_QUEUE_DEPTH {
            self.drops += 1;
            return false;
        }
        self.rx_bytes += pkt.data.len() as u64;
        self.rx_packets += 1;
        self.queue.push(pkt);
        true
    }

    /// Dequeue all pending packets.
    fn drain(&mut self) -> Vec<VethPacket> {
        core::mem::take(&mut self.queue)
    }
}

// ---------------------------------------------------------------------------
// Veth pair
// ---------------------------------------------------------------------------

/// A paired set of two virtual interfaces linked together.
pub struct VethPair {
    /// Unique pair identifier.
    pub id: u64,
    /// Side A of the pair.
    pub a: VethInterface,
    /// Side B of the pair.
    pub b: VethInterface,
}

/// Manages all active veth pairs.
struct VethManager {
    pairs: Vec<VethPair>,
}

impl VethManager {
    const fn new() -> Self {
        Self { pairs: Vec::new() }
    }
}

// ---------------------------------------------------------------------------
// MAC address generation
// ---------------------------------------------------------------------------

/// Generate a locally-administered MAC from an ID.
fn generate_mac(id: u64, side: u8) -> MacAddr {
    MacAddr([
        0x02, // locally administered
        side,
        ((id >> 24) & 0xFF) as u8,
        ((id >> 16) & 0xFF) as u8,
        ((id >> 8) & 0xFF) as u8,
        (id & 0xFF) as u8,
    ])
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new veth pair with the given endpoint names and IP addresses.
/// Returns the pair ID on success.
pub fn create_pair(name_a: &str, ip_a: Ipv4Addr, name_b: &str, ip_b: Ipv4Addr) -> Result<u64, &'static str> {
    let mut mgr = VETH_MANAGER.lock();
    if mgr.pairs.len() >= MAX_VETH_PAIRS {
        return Err("maximum veth pairs reached");
    }
    let id = NEXT_PAIR_ID.fetch_add(1, Ordering::Relaxed);
    let mac_a = generate_mac(id, 0xA0);
    let mac_b = generate_mac(id, 0xB0);
    let pair = VethPair {
        id,
        a: VethInterface::new(String::from(name_a), mac_a, ip_a),
        b: VethInterface::new(String::from(name_b), mac_b, ip_b),
    };
    crate::serial_println!("[veth] created pair {} ({} <-> {})", id, name_a, name_b);
    mgr.pairs.push(pair);
    Ok(id)
}

/// Destroy a veth pair by ID.
pub fn destroy_pair(id: u64) -> Result<(), &'static str> {
    let mut mgr = VETH_MANAGER.lock();
    let pos = mgr.pairs.iter().position(|p| p.id == id).ok_or("pair not found")?;
    let pair = mgr.pairs.remove(pos);
    crate::serial_println!("[veth] destroyed pair {} ({} <-> {})", id, pair.a.name, pair.b.name);
    Ok(())
}

/// Send a packet into the named interface; it will appear on the peer's queue.
pub fn send(iface_name: &str, pkt: VethPacket) -> Result<(), &'static str> {
    let mut mgr = VETH_MANAGER.lock();
    for pair in mgr.pairs.iter_mut() {
        if pair.a.name == iface_name {
            if !pair.a.link_up || !pair.b.link_up {
                return Err("link down");
            }
            pair.a.tx_packets += 1;
            pair.a.tx_bytes += pkt.data.len() as u64;
            pair.b.enqueue(pkt);
            return Ok(());
        }
        if pair.b.name == iface_name {
            if !pair.a.link_up || !pair.b.link_up {
                return Err("link down");
            }
            pair.b.tx_packets += 1;
            pair.b.tx_bytes += pkt.data.len() as u64;
            pair.a.enqueue(pkt);
            return Ok(());
        }
    }
    Err("interface not found")
}

/// Receive (drain) all pending packets from the named interface.
pub fn recv(iface_name: &str) -> Result<Vec<VethPacket>, &'static str> {
    let mut mgr = VETH_MANAGER.lock();
    for pair in mgr.pairs.iter_mut() {
        if pair.a.name == iface_name {
            return Ok(pair.a.drain());
        }
        if pair.b.name == iface_name {
            return Ok(pair.b.drain());
        }
    }
    Err("interface not found")
}

/// List all active veth pairs.
pub fn list_pairs() -> Vec<String> {
    let mgr = VETH_MANAGER.lock();
    mgr.pairs.iter().map(|p| {
        format!("veth{}: {} ({}) <-> {} ({})",
            p.id, p.a.name, p.a.ip, p.b.name, p.b.ip)
    }).collect()
}

/// Get link status for a named interface.
pub fn link_status(iface_name: &str) -> Result<bool, &'static str> {
    let mgr = VETH_MANAGER.lock();
    for pair in mgr.pairs.iter() {
        if pair.a.name == iface_name { return Ok(pair.a.link_up); }
        if pair.b.name == iface_name { return Ok(pair.b.link_up); }
    }
    Err("interface not found")
}

/// Get stats for a named interface.
pub fn iface_stats(iface_name: &str) -> Result<String, &'static str> {
    let mgr = VETH_MANAGER.lock();
    for pair in mgr.pairs.iter() {
        for iface in [&pair.a, &pair.b] {
            if iface.name == iface_name {
                return Ok(format!(
                    "{}: mac={} ip={} link={}\n  TX: {} pkts {} bytes\n  RX: {} pkts {} bytes\n  drops: {}",
                    iface.name, iface.mac, iface.ip,
                    if iface.link_up { "up" } else { "down" },
                    iface.tx_packets, iface.tx_bytes,
                    iface.rx_packets, iface.rx_bytes,
                    iface.drops,
                ));
            }
        }
    }
    Err("interface not found")
}

// ---------------------------------------------------------------------------
// NAT — simple source NAT for container egress
// ---------------------------------------------------------------------------

/// A single NAT mapping (container internal -> external).
#[derive(Clone)]
struct NatEntry {
    /// Container's internal (source) IP.
    internal_ip: Ipv4Addr,
    /// External (translated) IP seen on the wire.
    external_ip: Ipv4Addr,
    /// Number of packets translated.
    packets: u64,
}

/// Source NAT table.
struct NatTable {
    entries: Vec<NatEntry>,
}

impl NatTable {
    const fn new() -> Self {
        Self { entries: Vec::new() }
    }
}

/// Perform source NAT: rewrite `src_ip` to `external_ip` for outbound traffic.
/// Creates the mapping if it does not exist.
pub fn nat_outbound(src_ip: Ipv4Addr, external_ip: Ipv4Addr) -> Result<(), &'static str> {
    let mut table = NAT.lock();
    for entry in table.entries.iter_mut() {
        if entry.internal_ip == src_ip {
            entry.external_ip = external_ip;
            entry.packets += 1;
            return Ok(());
        }
    }
    if table.entries.len() >= MAX_NAT_ENTRIES {
        return Err("NAT table full");
    }
    table.entries.push(NatEntry {
        internal_ip: src_ip,
        external_ip,
        packets: 1,
    });
    crate::serial_println!("[veth] NAT {} -> {}", src_ip, external_ip);
    Ok(())
}

/// List all NAT table entries.
pub fn nat_table() -> Vec<String> {
    let table = NAT.lock();
    table.entries.iter().map(|e| {
        format!("{} -> {} ({} pkts)", e.internal_ip, e.external_ip, e.packets)
    }).collect()
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialise the veth subsystem.
pub fn init() {
    crate::serial_println!("[veth] virtual ethernet subsystem initialised");
}
