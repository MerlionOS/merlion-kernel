/// Network stack integration layer for MerlionOS.
///
/// Bridges the raw NIC drivers ([`crate::e1000e`] and [`crate::virtio_net`]) to
/// the higher-level protocol logic. Provides functions to build and send
/// Ethernet, ARP, IPv4, UDP, and ICMP frames, as well as a polling receive
/// path that parses incoming Ethernet headers.
///
/// NIC selection priority: e1000e > virtio-net > usb-eth (CDC-ECM) > loopback (no real NIC).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, Ordering};

use crate::{e1000e, usb_eth, virtio_net, net, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Ethernet header size: 6 dst + 6 src + 2 ethertype.
const ETH_HEADER_LEN: usize = 14;

/// IPv4 header size (no options).
const IPV4_HEADER_LEN: usize = 20;

/// UDP header size.
const UDP_HEADER_LEN: usize = 8;

/// ICMP echo header size (type + code + checksum + id + seq).
const ICMP_ECHO_LEN: usize = 8;

/// ARP packet size (Ethernet hardware, IPv4 protocol).
const ARP_PACKET_LEN: usize = 28;

/// ICMP echo request identifier (arbitrary).
const ICMP_ECHO_ID: u16 = 0x4D4C; // "ML" for MerlionOS

// ---------------------------------------------------------------------------
// NIC backend selection
// ---------------------------------------------------------------------------

/// Which NIC backend is currently active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum NicBackend {
    /// No NIC detected; packets are silently dropped.
    None = 0,
    /// Intel e1000/e1000e driver.
    E1000e = 1,
    /// Virtio-net (legacy PCI transport).
    VirtioNet = 2,
    /// USB CDC-ECM/NCM Ethernet adapter.
    UsbEth = 3,
}

/// Active backend, set once during [`init`].
static BACKEND: AtomicU8 = AtomicU8::new(NicBackend::None as u8);

/// Return the currently selected NIC backend.
fn backend() -> NicBackend {
    match BACKEND.load(Ordering::Relaxed) {
        1 => NicBackend::E1000e,
        2 => NicBackend::VirtioNet,
        3 => NicBackend::UsbEth,
        _ => NicBackend::None,
    }
}

// ---------------------------------------------------------------------------
// ReceivedFrame
// ---------------------------------------------------------------------------

/// A received Ethernet frame with parsed header fields.
#[derive(Debug, Clone)]
pub struct ReceivedFrame {
    /// Source MAC address.
    pub src_mac: [u8; 6],
    /// Destination MAC address.
    pub dst_mac: [u8; 6],
    /// EtherType (e.g. 0x0800 for IPv4, 0x0806 for ARP).
    pub ethertype: u16,
    /// Frame payload (everything after the 14-byte Ethernet header).
    pub payload: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Detect which NIC is available and select the backend.
///
/// Probes in priority order: e1000e, virtio-net, then falls back to a
/// no-op loopback sink. This should be called after both NIC drivers have
/// had a chance to initialise (i.e. after `e1000e::init()` and
/// `virtio_net::init()`).
pub fn init() {
    if e1000e::is_detected() {
        BACKEND.store(NicBackend::E1000e as u8, Ordering::SeqCst);
        serial_println!("[netstack] backend: e1000e");
    } else if virtio_net::is_detected() {
        BACKEND.store(NicBackend::VirtioNet as u8, Ordering::SeqCst);
        serial_println!("[netstack] backend: virtio-net");
    } else if usb_eth::is_detected() {
        BACKEND.store(NicBackend::UsbEth as u8, Ordering::SeqCst);
        serial_println!("[netstack] backend: usb-eth (CDC-ECM)");
    } else {
        BACKEND.store(NicBackend::None as u8, Ordering::SeqCst);
        serial_println!("[netstack] backend: loopback (no NIC)");
    }
}

// ---------------------------------------------------------------------------
// Low-level send / receive
// ---------------------------------------------------------------------------

/// Transmit a raw Ethernet frame through whichever NIC is active.
///
/// Returns `true` on success, `false` if no NIC is available or the driver
/// rejected the frame.
fn nic_send(frame: &[u8]) -> bool {
    match backend() {
        NicBackend::E1000e => e1000e::send_frame(frame),
        NicBackend::VirtioNet => virtio_net::send_frame(frame).is_ok(),
        NicBackend::UsbEth => usb_eth::send_frame(frame).is_ok(),
        NicBackend::None => false,
    }
}

/// Poll the active NIC for a single received frame.
///
/// Returns the raw frame bytes (starting at the destination MAC) or `None`
/// if no frame is pending.
fn nic_recv() -> Option<Vec<u8>> {
    match backend() {
        NicBackend::E1000e => e1000e::recv_frame(),
        // virtio-net does not yet expose recv_frame; return None.
        NicBackend::VirtioNet => None,
        NicBackend::UsbEth => usb_eth::recv_frame(),
        NicBackend::None => None,
    }
}

/// Return our MAC address from the global network state.
fn our_mac() -> [u8; 6] {
    net::NET.lock().mac.0
}

/// Return our IPv4 address from the global network state.
fn our_ip() -> [u8; 4] {
    net::NET.lock().ip.0
}

// ---------------------------------------------------------------------------
// Checksum
// ---------------------------------------------------------------------------

/// Compute the standard Internet checksum (ones' complement of the ones'
/// complement sum of 16-bit words).
///
/// Used for IPv4 headers, ICMP, and UDP. Handles odd-length data by
/// treating the trailing byte as the high byte of a final 16-bit word.
pub fn ip_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

// ---------------------------------------------------------------------------
// Ethernet
// ---------------------------------------------------------------------------

/// Build an Ethernet frame and transmit it.
///
/// Constructs a 14-byte Ethernet header (dst MAC, src MAC, ethertype) followed
/// by `payload`, then hands the complete frame to the active NIC driver. The
/// NIC hardware appends the FCS/CRC automatically.
///
/// Returns `true` if the frame was successfully queued for transmission.
pub fn send_ethernet(dst_mac: &[u8; 6], ethertype: u16, payload: &[u8]) -> bool {
    let total = ETH_HEADER_LEN + payload.len();
    let mut frame = Vec::with_capacity(total);

    // Destination MAC
    frame.extend_from_slice(dst_mac);
    // Source MAC
    frame.extend_from_slice(&our_mac());
    // EtherType
    frame.extend_from_slice(&ethertype.to_be_bytes());
    // Payload
    frame.extend_from_slice(payload);

    let ok = nic_send(&frame);
    if ok {
        let mut ns = net::NET.lock();
        ns.tx_packets += 1;
        ns.tx_bytes += frame.len() as u64;
    }
    ok
}

// ---------------------------------------------------------------------------
// ARP
// ---------------------------------------------------------------------------

/// Construct and send an ARP "who-has" request for `target_ip`.
///
/// Broadcasts an ARP request on the local Ethernet segment asking which
/// host owns `target_ip`. The sender fields are filled from the current
/// interface configuration in [`net::NET`].
pub fn send_arp_request(target_ip: [u8; 4]) -> bool {
    let src_mac = our_mac();
    let src_ip = our_ip();

    let mut arp = [0u8; ARP_PACKET_LEN];

    // Hardware type: Ethernet (1)
    arp[0..2].copy_from_slice(&1u16.to_be_bytes());
    // Protocol type: IPv4 (0x0800)
    arp[2..4].copy_from_slice(&0x0800u16.to_be_bytes());
    // Hardware address length
    arp[4] = 6;
    // Protocol address length
    arp[5] = 4;
    // Operation: request (1)
    arp[6..8].copy_from_slice(&1u16.to_be_bytes());
    // Sender hardware address
    arp[8..14].copy_from_slice(&src_mac);
    // Sender protocol address
    arp[14..18].copy_from_slice(&src_ip);
    // Target hardware address (unknown — zeroed)
    arp[18..24].copy_from_slice(&[0u8; 6]);
    // Target protocol address
    arp[24..28].copy_from_slice(&target_ip);

    serial_println!(
        "[netstack] ARP who-has {}.{}.{}.{} tell {}.{}.{}.{}",
        target_ip[0], target_ip[1], target_ip[2], target_ip[3],
        src_ip[0], src_ip[1], src_ip[2], src_ip[3]
    );

    send_ethernet(&[0xFF; 6], net::ETH_TYPE_ARP, &arp)
}

// ---------------------------------------------------------------------------
// IPv4
// ---------------------------------------------------------------------------

/// Build an IPv4 packet and send it inside an Ethernet frame.
///
/// Constructs a minimal 20-byte IPv4 header (no options) with the given
/// `protocol` number (e.g. 1 = ICMP, 17 = UDP), computes the header
/// checksum, and wraps everything in an Ethernet frame.
///
/// The destination MAC is resolved in order: ARP cache lookup, broadcast
/// for link-local / broadcast addresses, or gateway MAC. If no MAC is
/// known, a broadcast is used (suitable for QEMU user-net).
pub fn send_ipv4(dst_ip: [u8; 4], protocol: u8, payload: &[u8]) -> bool {
    let src_ip = our_ip();
    let total_len = (IPV4_HEADER_LEN + payload.len()) as u16;

    let mut ip_hdr = [0u8; IPV4_HEADER_LEN];

    // Version (4) + IHL (5 words = 20 bytes)
    ip_hdr[0] = 0x45;
    // DSCP / ECN
    ip_hdr[1] = 0;
    // Total length
    ip_hdr[2..4].copy_from_slice(&total_len.to_be_bytes());
    // Identification (static counter would be better; use 0 for now)
    ip_hdr[4..6].copy_from_slice(&1u16.to_be_bytes());
    // Flags + Fragment Offset (Don't Fragment)
    ip_hdr[6..8].copy_from_slice(&0x4000u16.to_be_bytes());
    // TTL
    ip_hdr[8] = 64;
    // Protocol
    ip_hdr[9] = protocol;
    // Header checksum — zeroed first, filled after computation
    ip_hdr[10..12].copy_from_slice(&[0, 0]);
    // Source IP
    ip_hdr[12..16].copy_from_slice(&src_ip);
    // Destination IP
    ip_hdr[16..20].copy_from_slice(&dst_ip);

    // Compute and insert header checksum
    let cksum = ip_checksum(&ip_hdr);
    ip_hdr[10..12].copy_from_slice(&cksum.to_be_bytes());

    // Build the full IP packet (header + payload)
    let mut packet = Vec::with_capacity(IPV4_HEADER_LEN + payload.len());
    packet.extend_from_slice(&ip_hdr);
    packet.extend_from_slice(payload);

    // Resolve destination MAC
    let dst_mac = resolve_dst_mac(&dst_ip);

    send_ethernet(&dst_mac, net::ETH_TYPE_IP, &packet)
}

/// Resolve a destination MAC address for the given IPv4 address.
///
/// Checks the ARP cache first, falls back to broadcast for broadcast /
/// link-local destinations, and uses broadcast as a last resort (works
/// fine under QEMU user-net where the virtual switch delivers everything).
fn resolve_dst_mac(dst_ip: &[u8; 4]) -> [u8; 6] {
    // Broadcast IP → broadcast MAC
    if *dst_ip == [255, 255, 255, 255] {
        return [0xFF; 6];
    }

    // Try ARP table
    let ip = net::Ipv4Addr(*dst_ip);
    if let Some(mac) = crate::netproto::arp_lookup(&ip) {
        return mac.0;
    }

    // Fallback: broadcast (QEMU user-net handles it)
    [0xFF; 6]
}

// ---------------------------------------------------------------------------
// UDP
// ---------------------------------------------------------------------------

/// Send a UDP datagram to `dst_ip:dst_port` from `src_port`.
///
/// Builds a UDP header (source port, dest port, length, checksum) around
/// `data`, then wraps it in an IPv4 packet with protocol 17. The UDP
/// checksum is computed over a pseudo-header per RFC 768.
pub fn send_udp(dst_ip: [u8; 4], src_port: u16, dst_port: u16, data: &[u8]) -> bool {
    let udp_len = (UDP_HEADER_LEN + data.len()) as u16;

    let mut udp = Vec::with_capacity(UDP_HEADER_LEN + data.len());

    // Source port
    udp.extend_from_slice(&src_port.to_be_bytes());
    // Destination port
    udp.extend_from_slice(&dst_port.to_be_bytes());
    // Length
    udp.extend_from_slice(&udp_len.to_be_bytes());
    // Checksum placeholder (zero = no checksum, valid for UDP over IPv4)
    udp.extend_from_slice(&0u16.to_be_bytes());
    // Data
    udp.extend_from_slice(data);

    // Compute UDP checksum over pseudo-header + UDP segment
    let src_ip = our_ip();
    let cksum = udp_checksum(&src_ip, &dst_ip, &udp);
    udp[6..8].copy_from_slice(&cksum.to_be_bytes());

    serial_println!(
        "[netstack] UDP :{} -> {}.{}.{}.{}:{} ({} bytes)",
        src_port, dst_ip[0], dst_ip[1], dst_ip[2], dst_ip[3],
        dst_port, data.len()
    );

    send_ipv4(dst_ip, 17, &udp)
}

/// Compute UDP checksum including the IPv4 pseudo-header.
fn udp_checksum(src_ip: &[u8; 4], dst_ip: &[u8; 4], udp_segment: &[u8]) -> u16 {
    let mut pseudo = Vec::with_capacity(12 + udp_segment.len());
    pseudo.extend_from_slice(src_ip);
    pseudo.extend_from_slice(dst_ip);
    pseudo.push(0); // zero
    pseudo.push(17); // protocol
    pseudo.extend_from_slice(&(udp_segment.len() as u16).to_be_bytes());
    pseudo.extend_from_slice(udp_segment);
    let ck = ip_checksum(&pseudo);
    // RFC 768: if computed checksum is 0, transmit 0xFFFF
    if ck == 0 { 0xFFFF } else { ck }
}

// ---------------------------------------------------------------------------
// ICMP
// ---------------------------------------------------------------------------

/// Send an ICMP Echo Request (ping) to `dst_ip` with the given sequence
/// number.
///
/// Builds a type-8 ICMP message with a fixed identifier and the provided
/// `seq`, computes the ICMP checksum, and sends it inside an IPv4 packet
/// with protocol 1.
pub fn send_icmp_echo(dst_ip: [u8; 4], seq: u16) -> bool {
    // 8 bytes header + 56 bytes payload (standard ping size)
    let payload_len = 56;
    let total = ICMP_ECHO_LEN + payload_len;
    let mut icmp = Vec::with_capacity(total);

    // Type: echo request (8)
    icmp.push(8);
    // Code: 0
    icmp.push(0);
    // Checksum placeholder
    icmp.extend_from_slice(&0u16.to_be_bytes());
    // Identifier
    icmp.extend_from_slice(&ICMP_ECHO_ID.to_be_bytes());
    // Sequence number
    icmp.extend_from_slice(&seq.to_be_bytes());
    // Payload: ascending bytes for easy identification in captures
    for i in 0..payload_len {
        icmp.push(i as u8);
    }

    // Compute and fill ICMP checksum
    let cksum = ip_checksum(&icmp);
    icmp[2..4].copy_from_slice(&cksum.to_be_bytes());

    serial_println!(
        "[netstack] ICMP echo -> {}.{}.{}.{} seq={}",
        dst_ip[0], dst_ip[1], dst_ip[2], dst_ip[3], seq
    );

    send_ipv4(dst_ip, 1, &icmp)
}

// ---------------------------------------------------------------------------
// Receive path
// ---------------------------------------------------------------------------

/// Poll the NIC for a received frame and parse its Ethernet header.
///
/// Returns `Some(ReceivedFrame)` with the source/destination MACs,
/// ethertype, and payload extracted, or `None` if no frame is available.
/// Updates the global RX statistics on success.
pub fn poll_rx() -> Option<ReceivedFrame> {
    let raw = nic_recv()?;

    if raw.len() < ETH_HEADER_LEN {
        return None;
    }

    let mut dst_mac = [0u8; 6];
    let mut src_mac = [0u8; 6];
    dst_mac.copy_from_slice(&raw[0..6]);
    src_mac.copy_from_slice(&raw[6..12]);
    let ethertype = u16::from_be_bytes([raw[12], raw[13]]);
    let payload = raw[ETH_HEADER_LEN..].to_vec();

    // Update RX stats
    {
        let mut ns = net::NET.lock();
        ns.rx_packets += 1;
        ns.rx_bytes += raw.len() as u64;
    }

    Some(ReceivedFrame {
        src_mac,
        dst_mac,
        ethertype,
        payload,
    })
}
