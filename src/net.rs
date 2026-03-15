/// Minimal networking stack.
/// Provides ARP and UDP over a simple Ethernet-like interface.
/// Uses QEMU's -netdev user for connectivity.
///
/// This is a stub/educational implementation — no actual NIC driver yet,
/// but provides the packet structures, protocol logic, and a loopback
/// interface for demonstrating the networking concepts.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use spin::Mutex;

// --- Ethernet ---

pub const ETH_TYPE_ARP: u16 = 0x0806;
pub const ETH_TYPE_IP: u16 = 0x0800;

#[derive(Debug, Clone)]
pub struct MacAddr(pub [u8; 6]);

impl MacAddr {
    pub const BROADCAST: MacAddr = MacAddr([0xFF; 6]);
    pub const ZERO: MacAddr = MacAddr([0; 6]);
}

impl core::fmt::Display for MacAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5])
    }
}

// --- IPv4 ---

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ipv4Addr(pub [u8; 4]);

impl Ipv4Addr {
    pub const ZERO: Ipv4Addr = Ipv4Addr([0; 4]);
    pub const LOOPBACK: Ipv4Addr = Ipv4Addr([127, 0, 0, 1]);
    pub const BROADCAST: Ipv4Addr = Ipv4Addr([255, 255, 255, 255]);
}

impl core::fmt::Display for Ipv4Addr {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}

// --- Network interface ---

pub static NET: Mutex<NetworkState> = Mutex::new(NetworkState::new());

pub struct NetworkState {
    pub mac: MacAddr,
    pub ip: Ipv4Addr,
    pub gateway: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    /// Loopback packet queue for demo purposes
    loopback_queue: Vec<Packet>,
}

#[derive(Debug, Clone)]
pub struct Packet {
    pub src_ip: Ipv4Addr,
    pub dst_ip: Ipv4Addr,
    pub src_port: u16,
    pub dst_port: u16,
    pub protocol: &'static str,
    pub data: Vec<u8>,
}

impl NetworkState {
    const fn new() -> Self {
        Self {
            mac: MacAddr([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]), // QEMU default
            ip: Ipv4Addr([10, 0, 2, 15]),       // QEMU user-net default
            gateway: Ipv4Addr([10, 0, 2, 2]),
            netmask: Ipv4Addr([255, 255, 255, 0]),
            rx_packets: 0,
            tx_packets: 0,
            rx_bytes: 0,
            tx_bytes: 0,
            loopback_queue: Vec::new(),
        }
    }

    /// Send a UDP packet (loopback only for now).
    pub fn send_udp(&mut self, dst_ip: Ipv4Addr, dst_port: u16, src_port: u16, data: &[u8]) {
        let packet = Packet {
            src_ip: self.ip,
            dst_ip,
            src_port,
            dst_port,
            protocol: "UDP",
            data: data.to_vec(),
        };

        self.tx_packets += 1;
        self.tx_bytes += data.len() as u64;

        // If sending to ourselves or loopback, deliver locally
        if dst_ip == self.ip || dst_ip == Ipv4Addr::LOOPBACK {
            self.rx_packets += 1;
            self.rx_bytes += data.len() as u64;
            self.loopback_queue.push(packet);
        }

        crate::serial_println!("[net] UDP {}:{} -> {}:{} ({} bytes)",
            self.ip, src_port, dst_ip, dst_port, data.len());
    }

    /// Receive queued packets.
    pub fn recv(&mut self) -> Vec<Packet> {
        core::mem::take(&mut self.loopback_queue)
    }

    /// Network interface info string.
    pub fn ifconfig(&self) -> String {
        format!(
            "eth0: {} ({})\n  inet {}\n  netmask {}\n  gateway {}\n  RX: {} packets, {} bytes\n  TX: {} packets, {} bytes",
            self.mac, "loopback",
            self.ip, self.netmask, self.gateway,
            self.rx_packets, self.rx_bytes,
            self.tx_packets, self.tx_bytes,
        )
    }
}

/// Simple DNS-like hostname resolution (hardcoded).
pub fn resolve(hostname: &str) -> Option<Ipv4Addr> {
    match hostname {
        "localhost" => Some(Ipv4Addr::LOOPBACK),
        "gateway" => Some(Ipv4Addr([10, 0, 2, 2])),
        "self" => Some(Ipv4Addr([10, 0, 2, 15])),
        _ => {
            // Try parsing as IP: "a.b.c.d"
            let parts: Vec<&str> = hostname.split('.').collect();
            if parts.len() == 4 {
                let a = parts[0].parse::<u8>().ok()?;
                let b = parts[1].parse::<u8>().ok()?;
                let c = parts[2].parse::<u8>().ok()?;
                let d = parts[3].parse::<u8>().ok()?;
                Some(Ipv4Addr([a, b, c, d]))
            } else {
                None
            }
        }
    }
}
