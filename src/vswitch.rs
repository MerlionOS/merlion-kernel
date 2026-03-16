/// Virtual network switch for MerlionOS container networking.
/// Provides Layer-2 switching between virtual ports, MAC learning,
/// VLAN tagging, and multi-port bridging for container isolation.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use spin::Mutex;
use crate::net::{MacAddr, Ipv4Addr};

/// Maximum number of entries in the MAC address table before eviction.
const MAC_TABLE_CAPACITY: usize = 256;

/// Maximum number of ports a single bridge group can contain.
const MAX_BRIDGE_PORTS: usize = 16;

/// Global virtual switch instance shared across the kernel.
pub static VSWITCH: Mutex<VSwitch> = Mutex::new(VSwitch::new());

/// A virtual port representing one end of a container's network connection.
#[derive(Debug, Clone)]
pub struct VPort {
    /// Unique port identifier assigned by the switch.
    pub id: usize,
    /// MAC address assigned to this port.
    pub mac: MacAddr,
    /// IPv4 address assigned to this port.
    pub ip: Ipv4Addr,
    /// Peer port id, if connected.
    pub connected_to: Option<usize>,
    /// Optional 802.1Q VLAN tag (1-4094).
    pub vlan_tag: Option<u16>,
    /// Frames transmitted from this port.
    pub tx_count: u64,
    /// Frames received on this port.
    pub rx_count: u64,
}

/// Per-port packet statistics returned by [`VSwitch::port_stats`].
#[derive(Debug, Clone)]
pub struct PortStats {
    pub port_id: usize,
    pub mac: MacAddr,
    pub tx_packets: u64,
    pub rx_packets: u64,
}

/// A bridge group tying multiple ports into one broadcast domain.
#[derive(Debug, Clone)]
pub struct Bridge {
    pub name: String,
    pub port_ids: Vec<usize>,
}

/// An Ethernet-like frame for internal switch forwarding.
#[derive(Debug, Clone)]
pub struct Frame {
    pub dst_mac: MacAddr,
    pub src_mac: MacAddr,
    pub ethertype: u16,
    pub payload: Vec<u8>,
}

/// Virtual Layer-2 switch with MAC learning, VLAN support, and bridging.
pub struct VSwitch {
    ports: Vec<VPort>,
    mac_table: Vec<(MacAddr, usize)>,
    bridges: Vec<Bridge>,
    next_port_id: usize,
}

impl VSwitch {
    /// Create a new empty virtual switch.
    const fn new() -> Self {
        Self {
            ports: Vec::new(),
            mac_table: Vec::new(),
            bridges: Vec::new(),
            next_port_id: 0,
        }
    }

    /// Create a new virtual port with the given MAC and IP addresses.
    /// Returns the assigned port id.
    pub fn create_port(&mut self, mac: MacAddr, ip: Ipv4Addr) -> usize {
        let id = self.next_port_id;
        self.next_port_id += 1;
        self.ports.push(VPort {
            id,
            mac,
            ip,
            connected_to: None,
            vlan_tag: None,
            tx_count: 0,
            rx_count: 0,
        });
        id
    }

    /// Create a port with an explicit VLAN tag.
    pub fn create_port_vlan(&mut self, mac: MacAddr, ip: Ipv4Addr, vlan: u16) -> usize {
        let id = self.create_port(mac, ip);
        if let Some(port) = self.port_mut(id) {
            port.vlan_tag = Some(vlan);
        }
        id
    }

    /// Establish a point-to-point link between two ports.
    pub fn connect(&mut self, port_a: usize, port_b: usize) {
        if let Some(p) = self.port_mut(port_a) {
            p.connected_to = Some(port_b);
        }
        if let Some(p) = self.port_mut(port_b) {
            p.connected_to = Some(port_a);
        }
    }

    /// Forward a frame from `src_port`. Learns source MAC, then delivers
    /// to the destination port (unicast hit) or floods (broadcast/unknown).
    /// VLAN filtering is applied when tags are present.
    pub fn forward_frame(&mut self, src_port: usize, frame: &Frame) -> Vec<usize> {
        if let Some(p) = self.port_mut(src_port) { p.tx_count += 1; }
        self.mac_learn(&frame.src_mac, src_port);
        let src_vlan = self.port_vlan(src_port);
        let is_broadcast = frame.dst_mac.0 == MacAddr::BROADCAST.0;
        let dst_port_id = if is_broadcast { None } else { self.mac_lookup(&frame.dst_mac) };
        let mut delivered_to = Vec::new();

        match dst_port_id {
            Some(dst_id) if dst_id != src_port => {
                if self.vlans_match(src_vlan, self.port_vlan(dst_id)) {
                    if let Some(p) = self.port_mut(dst_id) {
                        p.rx_count += 1;
                    }
                    delivered_to.push(dst_id);
                }
            }
            _ => {
                let eligible: Vec<usize> = self
                    .ports
                    .iter()
                    .filter(|p| p.id != src_port)
                    .filter(|p| self.vlans_match(src_vlan, p.vlan_tag))
                    .map(|p| p.id)
                    .collect();

                for pid in &eligible {
                    if let Some(p) = self.port_mut(*pid) {
                        p.rx_count += 1;
                    }
                    delivered_to.push(*pid);
                }
            }
        }

        delivered_to
    }

    /// Return per-port packet statistics for every registered port.
    pub fn port_stats(&self) -> Vec<PortStats> {
        self.ports
            .iter()
            .map(|p| PortStats {
                port_id: p.id,
                mac: p.mac.clone(),
                tx_packets: p.tx_count,
                rx_packets: p.rx_count,
            })
            .collect()
    }

    /// Create a named bridge group spanning the given ports. Returns the
    /// bridge index. Bridged ports share one broadcast domain.
    pub fn create_bridge(&mut self, name: &str, port_ids: &[usize]) -> usize {
        let mut ids = Vec::new();
        for &pid in port_ids {
            if pid < self.next_port_id && ids.len() < MAX_BRIDGE_PORTS {
                ids.push(pid);
            }
        }
        let idx = self.bridges.len();
        self.bridges.push(Bridge {
            name: String::from(name),
            port_ids: ids,
        });
        idx
    }

    /// List all bridge groups.
    pub fn bridges(&self) -> &[Bridge] { &self.bridges }

    /// Return the total number of ports on the switch.
    pub fn port_count(&self) -> usize { self.ports.len() }

    /// Get a shared reference to a port by id.
    pub fn port(&self, id: usize) -> Option<&VPort> {
        self.ports.iter().find(|p| p.id == id)
    }

    /// Get a mutable reference to a port by id.
    fn port_mut(&mut self, id: usize) -> Option<&mut VPort> {
        self.ports.iter_mut().find(|p| p.id == id)
    }

    /// Learn or update a MAC-to-port mapping.
    fn mac_learn(&mut self, mac: &MacAddr, port_id: usize) {
        for entry in self.mac_table.iter_mut() {
            if entry.0 .0 == mac.0 { entry.1 = port_id; return; }
        }
        if self.mac_table.len() >= MAC_TABLE_CAPACITY {
            self.mac_table.remove(0);
        }
        self.mac_table.push((mac.clone(), port_id));
    }

    /// Look up which port a MAC address lives on.
    fn mac_lookup(&self, mac: &MacAddr) -> Option<usize> {
        for entry in &self.mac_table {
            if entry.0 .0 == mac.0 {
                return Some(entry.1);
            }
        }
        None
    }

    /// Get the VLAN tag of a port, if any.
    fn port_vlan(&self, id: usize) -> Option<u16> {
        self.port(id).and_then(|p| p.vlan_tag)
    }

    /// Two ports match if both untagged or sharing the same VLAN tag.
    fn vlans_match(&self, a: Option<u16>, b: Option<u16>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(va), Some(vb)) => va == vb,
            _ => false,
        }
    }
}

/// Format a one-line summary of switch state for the shell.
pub fn status_line() -> String {
    let sw = VSWITCH.lock();
    format!(
        "vswitch: {} ports, {} MAC entries, {} bridges",
        sw.ports.len(),
        sw.mac_table.len(),
        sw.bridges.len(),
    )
}
