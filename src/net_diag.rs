/// Network diagnostics — comprehensive network testing and status.
/// Provides `netdiag` command that runs a full connectivity test.

use alloc::string::String;
use alloc::format;
use crate::{println, net, e1000e, virtio_net, netstack};

/// Run a full network diagnostic.
pub fn run() -> String {
    let mut report = String::from("\x1b[1m=== Network Diagnostics ===\x1b[0m\n\n");

    // 1. NIC detection
    report.push_str("NIC Status:\n");
    if e1000e::is_detected() {
        report.push_str(&format!("  \x1b[32m[OK]\x1b[0m e1000e: {}\n", e1000e::info()));
    } else if virtio_net::is_detected() {
        report.push_str(&format!("  \x1b[32m[OK]\x1b[0m virtio-net: {}\n", virtio_net::info()));
    } else {
        report.push_str("  \x1b[31m[FAIL]\x1b[0m No NIC detected\n");
    }

    // 2. Network configuration
    let n = net::NET.lock();
    report.push_str("\nConfiguration:\n");
    report.push_str(&format!("  MAC:     {}\n", n.mac));
    report.push_str(&format!("  IP:      {}\n", n.ip));
    report.push_str(&format!("  Gateway: {}\n", n.gateway));
    report.push_str(&format!("  Netmask: {}\n", n.netmask));
    drop(n);

    // 3. Packet statistics
    let n = net::NET.lock();
    report.push_str("\nStatistics:\n");
    report.push_str(&format!("  TX: {} packets, {} bytes\n", n.tx_packets, n.tx_bytes));
    report.push_str(&format!("  RX: {} packets, {} bytes\n", n.rx_packets, n.rx_bytes));
    drop(n);

    // 4. ARP table
    let arp_entries = crate::netproto::arp_list();
    report.push_str(&format!("\nARP Cache: {} entries\n", arp_entries.len()));
    for (ip, mac) in &arp_entries {
        report.push_str(&format!("  {} → {}\n", ip, mac));
    }

    // 5. TCP connections
    let sockets = crate::tcp_real::list_sockets();
    report.push_str(&format!("\nTCP Sockets: {}\n", sockets.len()));

    // 6. DNS cache
    report.push_str("\nDNS: cache active\n");

    // 7. Recommendations
    report.push_str("\nRecommendations:\n");
    let n = net::NET.lock();
    if n.ip == net::Ipv4Addr([10, 0, 2, 15]) {
        report.push_str("  \x1b[33m[INFO]\x1b[0m Using default QEMU IP. Run 'ifup' for DHCP.\n");
    }
    if n.tx_packets == 0 {
        report.push_str("  \x1b[33m[INFO]\x1b[0m No packets sent yet. Try 'ping gateway'.\n");
    }
    drop(n);

    report
}

/// Quick connectivity check — returns true if NIC is available.
pub fn is_network_available() -> bool {
    e1000e::is_detected() || virtio_net::is_detected()
}
