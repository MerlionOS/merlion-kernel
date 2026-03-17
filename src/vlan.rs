/// IEEE 802.1Q VLAN support for MerlionOS.
/// Provides VLAN tagging/untagging, trunk/access port modes,
/// inter-VLAN routing, and VLAN-aware bridging.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// IEEE 802.1Q Tag Protocol Identifier.
const TPID_8021Q: u16 = 0x8100;

/// Maximum number of VLANs.
const MAX_VLANS: usize = 4094;

/// Maximum ports per VLAN.
const MAX_PORTS_PER_VLAN: usize = 64;

/// Maximum VLAN interfaces (sub-interfaces like eth0.10).
const MAX_VLAN_INTERFACES: usize = 128;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Port mode for VLAN assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortMode {
    /// Untagged traffic, single VLAN.
    Access,
    /// Tagged traffic, multiple VLANs.
    Trunk,
    /// Both tagged and untagged traffic.
    Hybrid,
}

impl PortMode {
    pub fn name(&self) -> &'static str {
        match self {
            PortMode::Access => "access",
            PortMode::Trunk => "trunk",
            PortMode::Hybrid => "hybrid",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "access" | "ACCESS" => Some(PortMode::Access),
            "trunk" | "TRUNK" => Some(PortMode::Trunk),
            "hybrid" | "HYBRID" => Some(PortMode::Hybrid),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// 802.1Q header
// ---------------------------------------------------------------------------

/// Parsed 802.1Q tag fields.
#[derive(Debug, Clone, Copy)]
pub struct VlanTag {
    /// Tag Protocol Identifier (should be 0x8100).
    pub tpid: u16,
    /// Priority Code Point (3 bits, 0-7).
    pub pcp: u8,
    /// Drop Eligible Indicator (1 bit).
    pub dei: bool,
    /// VLAN Identifier (12 bits, 0-4095).
    pub vid: u16,
}

impl VlanTag {
    /// Create a new VLAN tag with default PCP and DEI.
    pub fn new(vid: u16) -> Self {
        Self {
            tpid: TPID_8021Q,
            pcp: 0,
            dei: false,
            vid: vid & 0x0FFF,
        }
    }

    /// Create a VLAN tag with specified priority.
    pub fn with_priority(vid: u16, pcp: u8, dei: bool) -> Self {
        Self {
            tpid: TPID_8021Q,
            pcp: pcp & 0x07,
            dei,
            vid: vid & 0x0FFF,
        }
    }

    /// Encode the tag as 4 bytes (TPID + TCI).
    pub fn encode(&self) -> [u8; 4] {
        let tci: u16 = ((self.pcp as u16 & 0x07) << 13)
            | (if self.dei { 1u16 << 12 } else { 0 })
            | (self.vid & 0x0FFF);
        let tpid_bytes = self.tpid.to_be_bytes();
        let tci_bytes = tci.to_be_bytes();
        [tpid_bytes[0], tpid_bytes[1], tci_bytes[0], tci_bytes[1]]
    }

    /// Decode a VLAN tag from 4 bytes.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 { return None; }
        let tpid = u16::from_be_bytes([bytes[0], bytes[1]]);
        if tpid != TPID_8021Q { return None; }
        let tci = u16::from_be_bytes([bytes[2], bytes[3]]);
        Some(Self {
            tpid,
            pcp: ((tci >> 13) & 0x07) as u8,
            dei: (tci >> 12) & 1 == 1,
            vid: tci & 0x0FFF,
        })
    }
}

// ---------------------------------------------------------------------------
// VLAN interface (sub-interface)
// ---------------------------------------------------------------------------

/// A VLAN sub-interface (e.g., eth0.10).
#[derive(Debug, Clone)]
pub struct VlanInterface {
    /// Parent interface name (e.g., "eth0").
    pub parent: String,
    /// VLAN ID.
    pub vid: u16,
    /// IP address assigned to this interface.
    pub ip: [u8; 4],
    /// MAC address.
    pub mac: [u8; 6],
    /// Whether the interface is up.
    pub up: bool,
}

impl VlanInterface {
    /// The sub-interface name (e.g., "eth0.10").
    pub fn name(&self) -> String {
        format!("{}.{}", self.parent, self.vid)
    }
}

// ---------------------------------------------------------------------------
// Port configuration
// ---------------------------------------------------------------------------

/// Configuration for a switch port.
#[derive(Debug, Clone)]
pub struct PortConfig {
    /// Port name / identifier.
    pub name: String,
    /// Port mode.
    pub mode: PortMode,
    /// Port VLAN ID (native/untagged VLAN for trunk/hybrid).
    pub pvid: u16,
    /// VLANs this port is a member of (tagged).
    pub tagged_vlans: Vec<u16>,
    /// VLANs this port is a member of (untagged).
    pub untagged_vlans: Vec<u16>,
}

// ---------------------------------------------------------------------------
// VLAN entry in database
// ---------------------------------------------------------------------------

/// A VLAN entry in the VLAN database.
#[derive(Debug, Clone)]
struct VlanEntry {
    /// VLAN ID (1-4094).
    vid: u16,
    /// Human-readable name.
    name: String,
    /// Member ports (port name, tagged flag).
    members: Vec<(String, bool)>,
    /// Packet counter.
    rx_packets: u64,
    rx_bytes: u64,
    tx_packets: u64,
    tx_bytes: u64,
}

// ---------------------------------------------------------------------------
// VLAN manager
// ---------------------------------------------------------------------------

struct VlanManager {
    vlans: Vec<VlanEntry>,
    interfaces: Vec<VlanInterface>,
    ports: Vec<PortConfig>,
}

impl VlanManager {
    const fn new() -> Self {
        Self {
            vlans: Vec::new(),
            interfaces: Vec::new(),
            ports: Vec::new(),
        }
    }

    fn create_vlan(&mut self, vid: u16, name: &str) -> Result<(), &'static str> {
        if vid == 0 || vid > 4094 { return Err("VID must be 1-4094"); }
        if self.vlans.len() >= MAX_VLANS { return Err("VLAN table full"); }
        if self.vlans.iter().any(|v| v.vid == vid) { return Err("VLAN already exists"); }
        self.vlans.push(VlanEntry {
            vid,
            name: String::from(name),
            members: Vec::new(),
            rx_packets: 0,
            rx_bytes: 0,
            tx_packets: 0,
            tx_bytes: 0,
        });
        Ok(())
    }

    fn delete_vlan(&mut self, vid: u16) -> bool {
        if let Some(pos) = self.vlans.iter().position(|v| v.vid == vid) {
            self.vlans.remove(pos);
            // Remove VLAN interfaces for this VID
            self.interfaces.retain(|i| i.vid != vid);
            // Remove from port memberships
            for port in self.ports.iter_mut() {
                port.tagged_vlans.retain(|&v| v != vid);
                port.untagged_vlans.retain(|&v| v != vid);
            }
            true
        } else {
            false
        }
    }

    fn add_port_to_vlan(&mut self, vid: u16, port_name: &str, tagged: bool) -> Result<(), &'static str> {
        let vlan = self.vlans.iter_mut().find(|v| v.vid == vid)
            .ok_or("VLAN not found")?;
        if vlan.members.len() >= MAX_PORTS_PER_VLAN { return Err("too many ports"); }
        if vlan.members.iter().any(|(n, _)| n == port_name) {
            return Err("port already member of VLAN");
        }
        vlan.members.push((String::from(port_name), tagged));

        // Update port config
        if let Some(pc) = self.ports.iter_mut().find(|p| p.name == port_name) {
            if tagged {
                if !pc.tagged_vlans.contains(&vid) { pc.tagged_vlans.push(vid); }
            } else {
                if !pc.untagged_vlans.contains(&vid) { pc.untagged_vlans.push(vid); }
            }
        } else {
            // Auto-create port config
            let mode = if tagged { PortMode::Trunk } else { PortMode::Access };
            let mut pc = PortConfig {
                name: String::from(port_name),
                mode,
                pvid: if !tagged { vid } else { 1 },
                tagged_vlans: Vec::new(),
                untagged_vlans: Vec::new(),
            };
            if tagged { pc.tagged_vlans.push(vid); } else { pc.untagged_vlans.push(vid); }
            self.ports.push(pc);
        }
        Ok(())
    }

    fn remove_port_from_vlan(&mut self, vid: u16, port_name: &str) -> bool {
        if let Some(vlan) = self.vlans.iter_mut().find(|v| v.vid == vid) {
            let before = vlan.members.len();
            vlan.members.retain(|(n, _)| n != port_name);
            if vlan.members.len() < before {
                if let Some(pc) = self.ports.iter_mut().find(|p| p.name == port_name) {
                    pc.tagged_vlans.retain(|&v| v != vid);
                    pc.untagged_vlans.retain(|&v| v != vid);
                }
                return true;
            }
        }
        false
    }

    fn set_port_mode(&mut self, port_name: &str, mode: PortMode) -> bool {
        if let Some(pc) = self.ports.iter_mut().find(|p| p.name == port_name) {
            pc.mode = mode;
            true
        } else {
            false
        }
    }

    fn set_pvid(&mut self, port_name: &str, vid: u16) -> bool {
        if let Some(pc) = self.ports.iter_mut().find(|p| p.name == port_name) {
            pc.pvid = vid;
            true
        } else {
            false
        }
    }

    fn create_interface(&mut self, parent: &str, vid: u16, ip: [u8; 4], mac: [u8; 6]) -> Result<(), &'static str> {
        if self.interfaces.len() >= MAX_VLAN_INTERFACES { return Err("too many VLAN interfaces"); }
        if !self.vlans.iter().any(|v| v.vid == vid) { return Err("VLAN does not exist"); }
        if self.interfaces.iter().any(|i| i.parent == parent && i.vid == vid) {
            return Err("interface already exists");
        }
        self.interfaces.push(VlanInterface {
            parent: String::from(parent),
            vid,
            ip,
            mac,
            up: true,
        });
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Frame tagging / untagging
// ---------------------------------------------------------------------------

/// Insert an 802.1Q tag into an Ethernet frame.
/// The frame must start with dst MAC (6) + src MAC (6) + ethertype (2).
/// Returns a new frame with the 4-byte VLAN tag inserted after src MAC.
pub fn tag_frame(frame: &[u8], vid: u16) -> Vec<u8> {
    if frame.len() < 14 {
        return frame.to_vec();
    }
    let tag = VlanTag::new(vid);
    let encoded = tag.encode();
    let mut result = Vec::with_capacity(frame.len() + 4);
    // dst MAC + src MAC (12 bytes)
    result.extend_from_slice(&frame[..12]);
    // Insert VLAN tag
    result.extend_from_slice(&encoded);
    // Original ethertype + payload
    result.extend_from_slice(&frame[12..]);
    result
}

/// Remove an 802.1Q tag from an Ethernet frame.
/// Returns (untagged_frame, vid) or the original frame with VID 0 if no tag.
pub fn untag_frame(frame: &[u8]) -> (Vec<u8>, u16) {
    if frame.len() < 18 {
        return (frame.to_vec(), 0);
    }
    // Check for 802.1Q TPID at offset 12
    let tpid = u16::from_be_bytes([frame[12], frame[13]]);
    if tpid != TPID_8021Q {
        return (frame.to_vec(), 0);
    }
    if let Some(tag) = VlanTag::decode(&frame[12..16]) {
        let mut result = Vec::with_capacity(frame.len() - 4);
        result.extend_from_slice(&frame[..12]);
        result.extend_from_slice(&frame[16..]);
        (result, tag.vid)
    } else {
        (frame.to_vec(), 0)
    }
}

/// Check if a frame has an 802.1Q tag and return the VID.
pub fn get_frame_vid(frame: &[u8]) -> Option<u16> {
    if frame.len() < 16 { return None; }
    let tpid = u16::from_be_bytes([frame[12], frame[13]]);
    if tpid != TPID_8021Q { return None; }
    VlanTag::decode(&frame[12..16]).map(|t| t.vid)
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static VLAN_MANAGER: Mutex<VlanManager> = Mutex::new(VlanManager::new());

/// Global VLAN packet counter.
static VLAN_PACKETS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the VLAN subsystem.
pub fn init() {
    let mut mgr = VLAN_MANAGER.lock();
    // Create default VLAN 1
    let _ = mgr.create_vlan(1, "default");
}

/// Create a new VLAN.
pub fn create_vlan(vid: u16, name: &str) -> Result<(), &'static str> {
    VLAN_MANAGER.lock().create_vlan(vid, name)
}

/// Delete a VLAN.
pub fn delete_vlan(vid: u16) -> bool {
    VLAN_MANAGER.lock().delete_vlan(vid)
}

/// Add a port to a VLAN.
pub fn add_port(vid: u16, port: &str, tagged: bool) -> Result<(), &'static str> {
    VLAN_MANAGER.lock().add_port_to_vlan(vid, port, tagged)
}

/// Remove a port from a VLAN.
pub fn remove_port(vid: u16, port: &str) -> bool {
    VLAN_MANAGER.lock().remove_port_from_vlan(vid, port)
}

/// Set the mode for a port.
pub fn set_port_mode(port: &str, mode: PortMode) -> bool {
    VLAN_MANAGER.lock().set_port_mode(port, mode)
}

/// Set the PVID (native VLAN) for a port.
pub fn set_pvid(port: &str, vid: u16) -> bool {
    VLAN_MANAGER.lock().set_pvid(port, vid)
}

/// Create a VLAN sub-interface.
pub fn create_interface(parent: &str, vid: u16, ip: [u8; 4], mac: [u8; 6]) -> Result<(), &'static str> {
    VLAN_MANAGER.lock().create_interface(parent, vid, ip, mac)
}

/// List all VLANs as formatted strings.
pub fn list_vlans() -> String {
    let mgr = VLAN_MANAGER.lock();
    if mgr.vlans.is_empty() {
        return String::from("No VLANs configured.\n");
    }
    let mut out = format!("VID   Name             Ports              RX pkts    TX pkts\n");
    out.push_str("----  ---------------  -----------------  ---------  ---------\n");
    for v in &mgr.vlans {
        let ports: Vec<String> = v.members.iter().map(|(n, tagged)| {
            if *tagged { format!("{}(T)", n) } else { format!("{}(U)", n) }
        }).collect();
        let port_str = if ports.is_empty() {
            String::from("-")
        } else {
            let mut s = String::new();
            for (i, p) in ports.iter().enumerate() {
                if i > 0 { s.push(','); }
                s.push_str(p);
            }
            s
        };
        out.push_str(&format!("{:<5} {:<16} {:<18} {:<10} {}\n",
                              v.vid, v.name, port_str, v.rx_packets, v.tx_packets));
    }
    out
}

/// Get info for a specific VLAN.
pub fn vlan_info(vid: u16) -> String {
    let mgr = VLAN_MANAGER.lock();
    if let Some(v) = mgr.vlans.iter().find(|v| v.vid == vid) {
        let mut out = format!("VLAN {} ({})\n", v.vid, v.name);
        out.push_str(&format!("  Members: {}\n", v.members.len()));
        for (name, tagged) in &v.members {
            out.push_str(&format!("    {} ({})\n", name, if *tagged { "tagged" } else { "untagged" }));
        }
        out.push_str(&format!("  RX: {} packets, {} bytes\n", v.rx_packets, v.rx_bytes));
        out.push_str(&format!("  TX: {} packets, {} bytes\n", v.tx_packets, v.tx_bytes));
        // Show VLAN interfaces
        let ifaces: Vec<&VlanInterface> = mgr.interfaces.iter()
            .filter(|i| i.vid == vid).collect();
        if !ifaces.is_empty() {
            out.push_str("  Interfaces:\n");
            for iface in ifaces {
                out.push_str(&format!("    {} ip={}.{}.{}.{} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} {}\n",
                                      iface.name(),
                                      iface.ip[0], iface.ip[1], iface.ip[2], iface.ip[3],
                                      iface.mac[0], iface.mac[1], iface.mac[2],
                                      iface.mac[3], iface.mac[4], iface.mac[5],
                                      if iface.up { "UP" } else { "DOWN" }));
            }
        }
        out
    } else {
        format!("VLAN {} not found\n", vid)
    }
}

/// Per-VLAN and global statistics.
pub fn vlan_stats() -> String {
    let mgr = VLAN_MANAGER.lock();
    let total = VLAN_PACKETS.load(Ordering::Relaxed);
    let mut out = format!("VLAN statistics (total tagged packets: {})\n", total);
    out.push_str(&format!("  VLANs configured: {}\n", mgr.vlans.len()));
    out.push_str(&format!("  VLAN interfaces:  {}\n", mgr.interfaces.len()));
    out.push_str(&format!("  Switch ports:     {}\n", mgr.ports.len()));
    if !mgr.vlans.is_empty() {
        out.push_str("\nPer-VLAN:\n");
        for v in &mgr.vlans {
            out.push_str(&format!("  VLAN {:>4} ({}): RX {} pkts/{} bytes, TX {} pkts/{} bytes\n",
                                  v.vid, v.name, v.rx_packets, v.rx_bytes,
                                  v.tx_packets, v.tx_bytes));
        }
    }
    out
}

/// Process a frame through VLAN logic: classify and optionally tag/untag.
/// Updates per-VLAN statistics.
pub fn process_frame(frame: &[u8], ingress_port: &str) -> Option<(Vec<u8>, u16)> {
    VLAN_PACKETS.fetch_add(1, Ordering::Relaxed);
    let mut mgr = VLAN_MANAGER.lock();

    // Determine VLAN
    let (clean_frame, vid) = if let Some(fvid) = get_frame_vid(frame) {
        // Tagged frame: use embedded VID
        let (uf, v) = untag_frame(frame);
        (uf, v)
    } else {
        // Untagged frame: use port PVID
        let pvid = mgr.ports.iter().find(|p| p.name == ingress_port)
            .map(|p| p.pvid).unwrap_or(1);
        (frame.to_vec(), pvid)
    };

    // Update VLAN stats
    let pkt_len = clean_frame.len() as u64;
    if let Some(v) = mgr.vlans.iter_mut().find(|v| v.vid == vid) {
        v.rx_packets += 1;
        v.rx_bytes += pkt_len;
    }

    Some((clean_frame, vid))
}

/// Route a frame between VLANs (inter-VLAN routing).
/// Requires IP forwarding to be enabled (checks iptables module).
pub fn route_between_vlans(frame: &[u8], src_vid: u16, dst_vid: u16) -> Option<Vec<u8>> {
    if !crate::iptables::is_forwarding() { return None; }

    let mgr = VLAN_MANAGER.lock();
    // Verify both VLANs exist
    if !mgr.vlans.iter().any(|v| v.vid == src_vid) { return None; }
    if !mgr.vlans.iter().any(|v| v.vid == dst_vid) { return None; }

    // Re-tag frame with destination VLAN
    Some(tag_frame(frame, dst_vid))
}
