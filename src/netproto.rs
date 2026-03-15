/// Network protocol implementations: ARP table, ICMP echo (ping).
/// Builds on top of the net.rs loopback interface.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use spin::Mutex;
use crate::net::{MacAddr, Ipv4Addr};

// --- ARP Table ---

const ARP_TABLE_SIZE: usize = 16;

static ARP_TABLE: Mutex<Vec<ArpEntry>> = Mutex::new(Vec::new());

#[derive(Clone)]
struct ArpEntry {
    ip: Ipv4Addr,
    mac: MacAddr,
}

/// Add or update an ARP entry.
pub fn arp_insert(ip: Ipv4Addr, mac: MacAddr) {
    let mut table = ARP_TABLE.lock();
    for entry in table.iter_mut() {
        if entry.ip == ip {
            entry.mac = mac.clone();
            return;
        }
    }
    if table.len() < ARP_TABLE_SIZE {
        table.push(ArpEntry { ip, mac });
    }
}

/// Look up a MAC address by IP.
pub fn arp_lookup(ip: &Ipv4Addr) -> Option<MacAddr> {
    let table = ARP_TABLE.lock();
    table.iter().find(|e| e.ip == *ip).map(|e| e.mac.clone())
}

/// List all ARP entries.
pub fn arp_list() -> Vec<(Ipv4Addr, MacAddr)> {
    ARP_TABLE.lock().iter().map(|e| (e.ip, e.mac.clone())).collect()
}

// --- ICMP Ping ---

/// Ping result.
pub struct PingResult {
    pub target: Ipv4Addr,
    pub seq: u16,
    pub ttl: u8,
    pub rtt_ticks: u64,
    pub success: bool,
}

/// Simulate a ping to an IP address.
/// For loopback/self addresses, responds immediately.
/// For other addresses, simulates a timeout.
pub fn ping(target: Ipv4Addr, count: u16) -> Vec<PingResult> {
    let self_ip = crate::net::NET.lock().ip;
    let mut results = Vec::new();

    for seq in 0..count {
        let start = crate::timer::ticks();

        let success = target == self_ip
            || target == Ipv4Addr::LOOPBACK
            || target == Ipv4Addr([10, 0, 2, 2]); // gateway always "responds"

        // Simulate RTT
        if success {
            // Busy-wait a tiny bit for realism
            while crate::timer::ticks() < start + 1 {}
        }

        let rtt = crate::timer::ticks() - start;

        results.push(PingResult {
            target,
            seq,
            ttl: if success { 64 } else { 0 },
            rtt_ticks: if success { rtt } else { 0 },
            success,
        });

        // Small delay between pings
        let pause_until = crate::timer::ticks() + 10; // 100ms at 100Hz
        while crate::timer::ticks() < pause_until {
            x86_64::instructions::hlt();
        }
    }

    // Add ARP entry for successful pings
    if results.iter().any(|r| r.success) {
        arp_insert(target, MacAddr([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]));
    }

    results
}

/// Format ping results as a summary string.
pub fn format_ping(results: &[PingResult]) -> String {
    let mut out = String::new();
    let total = results.len();
    let success = results.iter().filter(|r| r.success).count();

    for r in results {
        if r.success {
            out.push_str(&format!(
                "Reply from {}: seq={} ttl={} time={}ms\n",
                r.target, r.seq, r.ttl, r.rtt_ticks * 10
            ));
        } else {
            out.push_str(&format!("Request timeout for seq={}\n", r.seq));
        }
    }

    out.push_str(&format!(
        "\n--- {} ping statistics ---\n{} packets sent, {} received, {}% loss\n",
        results.first().map(|r| format!("{}", r.target)).unwrap_or_default(),
        total, success,
        if total > 0 { ((total - success) * 100) / total } else { 0 }
    ));

    out
}

// --- TCP State (stub for Phase 30) ---

/// TCP connection states (simplified).
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

/// TCP connection info (placeholder for future implementation).
#[allow(dead_code)]
pub struct TcpConnection {
    pub local_ip: Ipv4Addr,
    pub local_port: u16,
    pub remote_ip: Ipv4Addr,
    pub remote_port: u16,
    pub state: TcpState,
}
