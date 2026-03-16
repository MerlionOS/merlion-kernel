/// Network bridge for MerlionOS.
/// Connects multiple virtual (veth) and physical interfaces into a single
/// L2 broadcast domain, with MAC learning and packet forwarding.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use crate::net::MacAddr;

/// Maximum number of bridges.
const MAX_BRIDGES: usize = 16;

/// Maximum interfaces per bridge.
const MAX_BRIDGE_PORTS: usize = 16;

/// Maximum entries in the MAC forwarding table.
const MAX_FDB_ENTRIES: usize = 256;

/// Age-out threshold for MAC entries (in ticks, ~100 Hz -> 300 s).
const FDB_AGE_TICKS: u64 = 30_000;

/// Global bridge manager.
static BRIDGE_MANAGER: Mutex<BridgeManager> = Mutex::new(BridgeManager::new());

/// Next bridge ID counter.
static NEXT_BRIDGE_ID: AtomicU64 = AtomicU64::new(1);

/// Global tick counter snapshot source (updated externally).
static BRIDGE_TICK: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Spanning Tree Protocol (simplified)
// ---------------------------------------------------------------------------

/// Simplified STP port state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StpPortState {
    /// Port does not forward or learn.
    Blocking,
    /// Port is learning MAC addresses but not yet forwarding.
    Learning,
    /// Port forwards frames normally.
    Forwarding,
    /// Port is administratively disabled.
    Disabled,
}

/// Simplified STP bridge state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StpRole {
    /// This bridge believes it is the root.
    RootBridge,
    /// This bridge has elected another root.
    DesignatedBridge,
}

// ---------------------------------------------------------------------------
// MAC forwarding database (FDB)
// ---------------------------------------------------------------------------

/// A single entry in the MAC forwarding table.
#[derive(Clone)]
struct FdbEntry {
    /// Learned MAC address.
    mac: MacAddr,
    /// Port index the MAC was seen on.
    port: usize,
    /// Tick at which this entry was last refreshed.
    last_seen: u64,
}

/// Forwarding database.
struct Fdb {
    entries: Vec<FdbEntry>,
}

impl Fdb {
    const fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Learn (or refresh) a source MAC on the given port.
    fn learn(&mut self, mac: &MacAddr, port: usize, now: u64) {
        for entry in self.entries.iter_mut() {
            if macs_equal(&entry.mac, mac) {
                entry.port = port;
                entry.last_seen = now;
                return;
            }
        }
        if self.entries.len() < MAX_FDB_ENTRIES {
            self.entries.push(FdbEntry {
                mac: mac.clone(),
                port,
                last_seen: now,
            });
        }
    }

    /// Lookup which port a destination MAC is on.
    fn lookup(&self, mac: &MacAddr) -> Option<usize> {
        self.entries.iter()
            .find(|e| macs_equal(&e.mac, mac))
            .map(|e| e.port)
    }

    /// Remove entries older than the age-out threshold.
    fn age_out(&mut self, now: u64) {
        self.entries.retain(|e| now.saturating_sub(e.last_seen) < FDB_AGE_TICKS);
    }

    /// Number of entries currently in the table.
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Compare two MAC addresses byte-by-byte.
fn macs_equal(a: &MacAddr, b: &MacAddr) -> bool {
    a.0 == b.0
}

// ---------------------------------------------------------------------------
// Bridge port
// ---------------------------------------------------------------------------

/// A port attached to a bridge.
#[derive(Clone)]
pub struct BridgePort {
    /// Interface name (e.g. "veth0", "eth0").
    pub iface_name: String,
    /// STP state of this port.
    pub stp_state: StpPortState,
}

// ---------------------------------------------------------------------------
// Bridge
// ---------------------------------------------------------------------------

/// A network bridge instance.
pub struct Bridge {
    /// Unique bridge ID.
    pub id: u64,
    /// Human-readable bridge name (e.g. "br0").
    pub name: String,
    /// Ports attached to this bridge.
    pub ports: Vec<BridgePort>,
    /// MAC forwarding database.
    fdb: Fdb,
    /// STP role of this bridge.
    pub stp_role: StpRole,
    /// Bridge priority for STP root election (lower wins).
    pub stp_priority: u16,
    /// Total frames forwarded.
    pub frames_forwarded: u64,
    /// Total frames flooded (unknown unicast / broadcast).
    pub frames_flooded: u64,
}

impl Bridge {
    fn new(id: u64, name: String) -> Self {
        Self {
            id,
            name,
            ports: Vec::new(),
            fdb: Fdb::new(),
            stp_role: StpRole::RootBridge,
            stp_priority: 32768, // default STP priority
            frames_forwarded: 0,
            frames_flooded: 0,
        }
    }
}

/// Manages all active bridges.
struct BridgeManager {
    bridges: Vec<Bridge>,
}

impl BridgeManager {
    const fn new() -> Self {
        Self { bridges: Vec::new() }
    }
}

// ---------------------------------------------------------------------------
// Public API — bridge lifecycle
// ---------------------------------------------------------------------------

/// Create a new bridge with the given name.  Returns the bridge ID.
pub fn create_bridge(name: &str) -> Result<u64, &'static str> {
    let mut mgr = BRIDGE_MANAGER.lock();
    if mgr.bridges.len() >= MAX_BRIDGES {
        return Err("maximum bridges reached");
    }
    for br in mgr.bridges.iter() {
        if br.name == name {
            return Err("bridge name already exists");
        }
    }
    let id = NEXT_BRIDGE_ID.fetch_add(1, Ordering::Relaxed);
    let br = Bridge::new(id, String::from(name));
    crate::serial_println!("[bridge] created bridge {} ({})", name, id);
    mgr.bridges.push(br);
    Ok(id)
}

/// Destroy a bridge by name.
pub fn destroy_bridge(name: &str) -> Result<(), &'static str> {
    let mut mgr = BRIDGE_MANAGER.lock();
    let pos = mgr.bridges.iter().position(|b| b.name == name).ok_or("bridge not found")?;
    mgr.bridges.remove(pos);
    crate::serial_println!("[bridge] destroyed bridge {}", name);
    Ok(())
}

/// Add an interface (by name) to a bridge.
pub fn add_interface(bridge_name: &str, iface_name: &str) -> Result<(), &'static str> {
    let mut mgr = BRIDGE_MANAGER.lock();
    let br = mgr.bridges.iter_mut()
        .find(|b| b.name == bridge_name)
        .ok_or("bridge not found")?;
    if br.ports.len() >= MAX_BRIDGE_PORTS {
        return Err("bridge port limit reached");
    }
    for port in br.ports.iter() {
        if port.iface_name == iface_name {
            return Err("interface already attached");
        }
    }
    br.ports.push(BridgePort {
        iface_name: String::from(iface_name),
        stp_state: StpPortState::Forwarding,
    });
    crate::serial_println!("[bridge] added {} to {}", iface_name, bridge_name);
    Ok(())
}

/// Remove an interface from a bridge.
pub fn remove_interface(bridge_name: &str, iface_name: &str) -> Result<(), &'static str> {
    let mut mgr = BRIDGE_MANAGER.lock();
    let br = mgr.bridges.iter_mut()
        .find(|b| b.name == bridge_name)
        .ok_or("bridge not found")?;
    let pos = br.ports.iter().position(|p| p.iface_name == iface_name)
        .ok_or("interface not attached")?;
    br.ports.remove(pos);
    crate::serial_println!("[bridge] removed {} from {}", iface_name, bridge_name);
    Ok(())
}

/// List all bridges with their ports.
pub fn list_bridges() -> Vec<String> {
    let mgr = BRIDGE_MANAGER.lock();
    mgr.bridges.iter().map(|br| {
        let ports: Vec<&str> = br.ports.iter().map(|p| p.iface_name.as_str()).collect();
        format!("{} (id={}, role={:?}, priority={}, ports=[{}], fdb={})",
            br.name, br.id, br.stp_role, br.stp_priority,
            ports.join(", "), br.fdb.len())
    }).collect()
}

// ---------------------------------------------------------------------------
// MAC learning & forwarding
// ---------------------------------------------------------------------------

/// Learn a source MAC address on the given bridge and port.
pub fn learn_mac(bridge_name: &str, src_mac: &MacAddr, port_index: usize) -> Result<(), &'static str> {
    let mut mgr = BRIDGE_MANAGER.lock();
    let br = mgr.bridges.iter_mut()
        .find(|b| b.name == bridge_name)
        .ok_or("bridge not found")?;
    if port_index >= br.ports.len() {
        return Err("port index out of range");
    }
    if br.ports[port_index].stp_state == StpPortState::Blocking
        || br.ports[port_index].stp_state == StpPortState::Disabled
    {
        return Err("port not in learning/forwarding state");
    }
    let now = BRIDGE_TICK.load(Ordering::Relaxed);
    br.fdb.learn(src_mac, port_index, now);
    Ok(())
}

/// Decide where to forward a frame.  Returns the port index if a specific
/// destination is known, or `None` to indicate the frame should be flooded
/// to all ports except the ingress port.
pub fn forward_lookup(bridge_name: &str, dst_mac: &MacAddr) -> Result<Option<usize>, &'static str> {
    let mgr = BRIDGE_MANAGER.lock();
    let br = mgr.bridges.iter()
        .find(|b| b.name == bridge_name)
        .ok_or("bridge not found")?;
    // Broadcast / multicast -> flood
    if dst_mac.0[0] & 0x01 != 0 {
        return Ok(None);
    }
    Ok(br.fdb.lookup(dst_mac))
}

/// Process a frame on a bridge: learn the source, decide forwarding.
/// Returns the list of port indices the frame should be sent out on.
pub fn process_frame(
    bridge_name: &str,
    src_mac: &MacAddr,
    dst_mac: &MacAddr,
    ingress_port: usize,
) -> Result<Vec<usize>, &'static str> {
    let mut mgr = BRIDGE_MANAGER.lock();
    let br = mgr.bridges.iter_mut()
        .find(|b| b.name == bridge_name)
        .ok_or("bridge not found")?;
    if ingress_port >= br.ports.len() {
        return Err("port index out of range");
    }
    // Only process on forwarding ports
    if br.ports[ingress_port].stp_state != StpPortState::Forwarding {
        return Ok(Vec::new());
    }
    // Learn source
    let now = BRIDGE_TICK.load(Ordering::Relaxed);
    br.fdb.learn(src_mac, ingress_port, now);

    // Forward decision
    let is_broadcast = dst_mac.0[0] & 0x01 != 0;
    if is_broadcast {
        // Flood to all forwarding ports except ingress
        let ports: Vec<usize> = (0..br.ports.len())
            .filter(|&i| i != ingress_port && br.ports[i].stp_state == StpPortState::Forwarding)
            .collect();
        br.frames_flooded += 1;
        Ok(ports)
    } else if let Some(egress) = br.fdb.lookup(dst_mac) {
        if egress == ingress_port {
            // Destination is on the same port; do not forward
            Ok(Vec::new())
        } else if br.ports[egress].stp_state == StpPortState::Forwarding {
            br.frames_forwarded += 1;
            Ok(alloc::vec![egress])
        } else {
            Ok(Vec::new())
        }
    } else {
        // Unknown unicast -> flood
        let ports: Vec<usize> = (0..br.ports.len())
            .filter(|&i| i != ingress_port && br.ports[i].stp_state == StpPortState::Forwarding)
            .collect();
        br.frames_flooded += 1;
        Ok(ports)
    }
}

/// Age out stale FDB entries on all bridges.
pub fn age_fdb() {
    let mut mgr = BRIDGE_MANAGER.lock();
    let now = BRIDGE_TICK.load(Ordering::Relaxed);
    for br in mgr.bridges.iter_mut() {
        br.fdb.age_out(now);
    }
}

// ---------------------------------------------------------------------------
// STP (simplified root election)
// ---------------------------------------------------------------------------

/// Run a simplified STP root election across all bridges.
/// The bridge with the lowest priority becomes RootBridge; all others
/// become DesignatedBridge.
pub fn stp_elect_root() {
    let mut mgr = BRIDGE_MANAGER.lock();
    if mgr.bridges.is_empty() {
        return;
    }
    // Find the bridge with the lowest (priority, id) tuple
    let mut root_idx = 0usize;
    for (i, br) in mgr.bridges.iter().enumerate() {
        let current_best = &mgr.bridges[root_idx];
        if br.stp_priority < current_best.stp_priority
            || (br.stp_priority == current_best.stp_priority && br.id < current_best.id)
        {
            root_idx = i;
        }
    }
    for (i, br) in mgr.bridges.iter_mut().enumerate() {
        if i == root_idx {
            br.stp_role = StpRole::RootBridge;
        } else {
            br.stp_role = StpRole::DesignatedBridge;
        }
    }
    crate::serial_println!("[bridge] STP root elected: {} (id={})",
        mgr.bridges[root_idx].name, mgr.bridges[root_idx].id);
}

/// Set STP port state on a specific port of a bridge.
pub fn set_port_state(bridge_name: &str, port_index: usize, state: StpPortState) -> Result<(), &'static str> {
    let mut mgr = BRIDGE_MANAGER.lock();
    let br = mgr.bridges.iter_mut()
        .find(|b| b.name == bridge_name)
        .ok_or("bridge not found")?;
    if port_index >= br.ports.len() {
        return Err("port index out of range");
    }
    br.ports[port_index].stp_state = state;
    crate::serial_println!("[bridge] {}: port {} -> {:?}", bridge_name, port_index, state);
    Ok(())
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

/// Get statistics for a bridge.
pub fn bridge_stats(bridge_name: &str) -> Result<String, &'static str> {
    let mgr = BRIDGE_MANAGER.lock();
    let br = mgr.bridges.iter()
        .find(|b| b.name == bridge_name)
        .ok_or("bridge not found")?;
    Ok(format!(
        "{}: id={} role={:?} priority={}\n  ports: {}\n  FDB entries: {}\n  forwarded: {} flooded: {}",
        br.name, br.id, br.stp_role, br.stp_priority,
        br.ports.len(), br.fdb.len(),
        br.frames_forwarded, br.frames_flooded,
    ))
}

/// Update the bridge tick counter (call from timer interrupt or periodic task).
pub fn tick(now: u64) {
    BRIDGE_TICK.store(now, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialise the bridge subsystem.
pub fn init() {
    crate::serial_println!("[bridge] network bridge subsystem initialised");
}
