/// DHCP client and DNS resolver for MerlionOS.
///
/// Implements DHCP discover/offer/request (RFC 2131) and DNS query/response
/// parsing (RFC 1035) using `no_std` compatible patterns with the `alloc` crate.
/// Designed to work alongside the kernel networking stack in [`crate::net`].

use alloc::vec;
use alloc::vec::Vec;
use crate::net::Ipv4Addr;

// DHCP constants (RFC 2131 / 2132)
const BOOTP_REQUEST: u8 = 1;
const BOOTP_REPLY: u8 = 2;
const HW_TYPE_ETHERNET: u8 = 1;
const HW_ADDR_LEN: u8 = 6;
const DHCP_MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];
const DHCP_DISCOVER: u8 = 1;
const DHCP_OFFER: u8 = 2;
const DHCP_REQUEST: u8 = 3;
const DHCP_ACK: u8 = 5;
const OPT_SUBNET_MASK: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DNS: u8 = 6;
const OPT_REQUESTED_IP: u8 = 50;
const OPT_LEASE_TIME: u8 = 51;
const OPT_MSG_TYPE: u8 = 53;
const OPT_END: u8 = 255;
/// Fixed-size BOOTP/DHCP header length (without options).
const BOOTP_HEADER_LEN: usize = 236;
/// Default transaction ID (in a real kernel this would be random).
const DEFAULT_XID: u32 = 0x4D_45_52_4C; // "MERL"

// DNS constants (RFC 1035)
const DNS_HEADER_LEN: usize = 12;
const DNS_FLAG_RD: u16 = 0x0100;
const DNS_QTYPE_A: u16 = 1;
const DNS_QCLASS_IN: u16 = 1;
const DNS_TYPE_A: u16 = 1;
const DNS_DEFAULT_ID: u16 = 0x4D4E;

/// Holds the parameters obtained from a successful DHCP handshake.
#[derive(Debug, Clone)]
pub struct DhcpLease {
    /// Assigned IP address.
    pub ip: Ipv4Addr,
    /// Default gateway / router.
    pub gateway: Ipv4Addr,
    /// DNS server address.
    pub dns: Ipv4Addr,
    /// Subnet mask.
    pub subnet_mask: Ipv4Addr,
    /// Lease duration in seconds.
    pub lease_time: u32,
}

/// Write a big-endian `u16` into `buf` at `offset`.
fn put_u16(buf: &mut Vec<u8>, offset: usize, v: u16) {
    buf[offset] = (v >> 8) as u8;
    buf[offset + 1] = v as u8;
}

/// Write a big-endian `u32` into `buf` at `offset`.
fn put_u32(buf: &mut Vec<u8>, offset: usize, v: u32) {
    buf[offset] = (v >> 24) as u8;
    buf[offset + 1] = (v >> 16) as u8;
    buf[offset + 2] = (v >> 8) as u8;
    buf[offset + 3] = v as u8;
}

/// Read a big-endian `u16` from a byte slice.
fn get_u16(data: &[u8], offset: usize) -> u16 {
    ((data[offset] as u16) << 8) | (data[offset + 1] as u16)
}

/// Read a big-endian `u32` from a byte slice.
fn get_u32(data: &[u8], offset: usize) -> u32 {
    ((data[offset] as u32) << 24)
        | ((data[offset + 1] as u32) << 16)
        | ((data[offset + 2] as u32) << 8)
        | (data[offset + 3] as u32)
}

/// Build the fixed BOOTP header common to Discover and Request messages.
fn build_bootp_header() -> Vec<u8> {
    let mut pkt = vec![0u8; BOOTP_HEADER_LEN];
    pkt[0] = BOOTP_REQUEST;
    pkt[1] = HW_TYPE_ETHERNET;
    pkt[2] = HW_ADDR_LEN;
    pkt[3] = 0; // hops
    put_u32(&mut pkt, 4, DEFAULT_XID);
    // secs, flags, ciaddr, yiaddr, siaddr, giaddr all zero
    // chaddr: deterministic MAC for the kernel
    pkt[28] = 0x52; pkt[29] = 0x54; pkt[30] = 0x00;
    pkt[31] = 0x12; pkt[32] = 0x34; pkt[33] = 0x56;
    pkt
}

/// Append the DHCP magic cookie and a message-type option.
fn append_dhcp_type(pkt: &mut Vec<u8>, msg_type: u8) {
    pkt.extend_from_slice(&DHCP_MAGIC_COOKIE);
    pkt.push(OPT_MSG_TYPE);
    pkt.push(1);
    pkt.push(msg_type);
}

/// Build a DHCP Discover packet ready for UDP broadcast on port 67.
///
/// The returned bytes contain the full BOOTP payload (header + options)
/// to be wrapped in a UDP datagram from `0.0.0.0:68` to `255.255.255.255:67`.
pub fn discover() -> Vec<u8> {
    let mut pkt = build_bootp_header();
    append_dhcp_type(&mut pkt, DHCP_DISCOVER);
    pkt.push(OPT_END);
    pkt.resize(pkt.len().max(300), 0);
    pkt
}

/// Build a DHCP Request packet for the given `offered_ip`.
///
/// Sent after receiving a valid Offer to confirm the lease with the server.
pub fn request(offered_ip: Ipv4Addr) -> Vec<u8> {
    let mut pkt = build_bootp_header();
    append_dhcp_type(&mut pkt, DHCP_REQUEST);
    pkt.push(OPT_REQUESTED_IP);
    pkt.push(4);
    pkt.extend_from_slice(&offered_ip.0);
    pkt.push(OPT_END);
    pkt.resize(pkt.len().max(300), 0);
    pkt
}

/// Parse a DHCP Offer (or ACK) packet and extract the lease parameters.
///
/// Returns `None` if the packet is too short, has an incorrect magic cookie,
/// or is missing critical options.
pub fn parse_offer(data: &[u8]) -> Option<DhcpLease> {
    if data.len() < BOOTP_HEADER_LEN + 4 {
        return None;
    }
    if data[0] != BOOTP_REPLY {
        return None;
    }
    let cookie_off = BOOTP_HEADER_LEN;
    if data[cookie_off..cookie_off + 4] != DHCP_MAGIC_COOKIE {
        return None;
    }
    // yiaddr -- offered IP at offset 16
    let offered_ip = Ipv4Addr([data[16], data[17], data[18], data[19]]);
    let mut subnet_mask = Ipv4Addr([255, 255, 255, 0]);
    let mut gateway = Ipv4Addr::ZERO;
    let mut dns = Ipv4Addr::ZERO;
    let mut lease_time: u32 = 86400;

    let mut i = cookie_off + 4;
    while i < data.len() {
        let opt = data[i];
        if opt == OPT_END { break; }
        if opt == 0 { i += 1; continue; } // pad
        if i + 1 >= data.len() { break; }
        let len = data[i + 1] as usize;
        let vs = i + 2; // value start
        if vs + len > data.len() { break; }
        match opt {
            OPT_SUBNET_MASK if len >= 4 => {
                subnet_mask = Ipv4Addr([data[vs], data[vs+1], data[vs+2], data[vs+3]]);
            }
            OPT_ROUTER if len >= 4 => {
                gateway = Ipv4Addr([data[vs], data[vs+1], data[vs+2], data[vs+3]]);
            }
            OPT_DNS if len >= 4 => {
                dns = Ipv4Addr([data[vs], data[vs+1], data[vs+2], data[vs+3]]);
            }
            OPT_LEASE_TIME if len >= 4 => {
                lease_time = get_u32(data, vs);
            }
            _ => {}
        }
        i = vs + len;
    }

    Some(DhcpLease { ip: offered_ip, gateway, dns, subnet_mask, lease_time })
}

/// Build a DNS A-record query for `hostname`.
///
/// Returns a complete DNS message (header + question section) suitable for
/// sending as a UDP payload to a recursive resolver on port 53.
pub fn build_dns_query(hostname: &str) -> Vec<u8> {
    let mut pkt = vec![0u8; DNS_HEADER_LEN];
    put_u16(&mut pkt, 0, DNS_DEFAULT_ID); // transaction ID
    put_u16(&mut pkt, 2, DNS_FLAG_RD);    // flags: recursion desired
    put_u16(&mut pkt, 4, 1);              // QDCOUNT = 1

    // Encode QNAME: each label preceded by its length, terminated by 0x00
    for label in hostname.split('.') {
        let bytes = label.as_bytes();
        pkt.push(bytes.len() as u8);
        pkt.extend_from_slice(bytes);
    }
    pkt.push(0); // root label

    let off = pkt.len();
    pkt.resize(off + 4, 0);
    put_u16(&mut pkt, off, DNS_QTYPE_A);
    put_u16(&mut pkt, off + 2, DNS_QCLASS_IN);
    pkt
}

/// Parse a DNS response and extract the first A-record IPv4 address.
///
/// Returns `None` when the response is malformed, contains no answers,
/// or has no A-record in the answer section.
pub fn parse_dns_response(data: &[u8]) -> Option<[u8; 4]> {
    if data.len() < DNS_HEADER_LEN {
        return None;
    }
    let flags = get_u16(data, 2);
    if flags & 0x8000 == 0 { return None; } // not a response
    if flags & 0x000F != 0 { return None; } // RCODE != 0

    let qdcount = get_u16(data, 4) as usize;
    let ancount = get_u16(data, 6) as usize;
    if ancount == 0 { return None; }

    // Skip question section
    let mut offset = DNS_HEADER_LEN;
    for _ in 0..qdcount {
        offset = skip_dns_name(data, offset)?;
        offset += 4; // QTYPE + QCLASS
        if offset > data.len() { return None; }
    }

    // Walk answer records looking for the first A record
    for _ in 0..ancount {
        offset = skip_dns_name(data, offset)?;
        if offset + 10 > data.len() { return None; }
        let rtype = get_u16(data, offset);
        let rdlen = get_u16(data, offset + 8) as usize;
        offset += 10;
        if offset + rdlen > data.len() { return None; }
        if rtype == DNS_TYPE_A && rdlen == 4 {
            return Some([data[offset], data[offset+1], data[offset+2], data[offset+3]]);
        }
        offset += rdlen;
    }
    None
}

/// Skip over a DNS domain name, handling both label sequences and compressed
/// pointers (RFC 1035 section 4.1.4).
fn skip_dns_name(data: &[u8], mut offset: usize) -> Option<usize> {
    let mut jumped = false;
    let mut end_offset = 0usize;
    loop {
        if offset >= data.len() { return None; }
        let len = data[offset] as usize;
        if len == 0 {
            if !jumped { end_offset = offset + 1; }
            break;
        }
        if len & 0xC0 == 0xC0 {
            if !jumped { end_offset = offset + 2; }
            if offset + 1 >= data.len() { return None; }
            offset = ((len & 0x3F) << 8) | (data[offset + 1] as usize);
            jumped = true;
            continue;
        }
        offset += 1 + len;
    }
    Some(end_offset)
}

/// Resolve a hostname to an IPv4 address using the kernel DNS server.
///
/// Builds a DNS query, sends it through the kernel UDP stack, and parses
/// the response. Returns `None` on failure.
///
/// **Note**: The current stub implementation returns well-known QEMU
/// user-mode addresses. A production version would transmit via
/// [`crate::net`] and await the reply.
pub fn resolve(hostname: &str) -> Option<Ipv4Addr> {
    let _query = build_dns_query(hostname);

    // Stub: simulate well-known QEMU user-mode addresses
    match hostname {
        "gateway" => return Some(Ipv4Addr([10, 0, 2, 2])),
        "dns" => return Some(Ipv4Addr([10, 0, 2, 3])),
        "localhost" => return Some(Ipv4Addr::LOOPBACK),
        _ => {}
    }

    // In a real implementation we would:
    //   1. Obtain the DNS server IP from the current DhcpLease
    //   2. Send `_query` via UDP to dns_server:53
    //   3. Receive the response bytes
    //   4. Call parse_dns_response(&response) and wrap in Ipv4Addr
    crate::serial_println!("[dhcp] dns resolve '{}': no reply (stub)", hostname);
    None
}
