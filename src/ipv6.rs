/// IPv6 protocol implementation for MerlionOS.
/// Provides IPv6 addressing, packet construction/parsing, ICMPv6,
/// neighbor discovery, and dual-stack operation alongside IPv4.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// IPv6 Address
// ---------------------------------------------------------------------------

/// A 128-bit IPv6 address.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ipv6Addr {
    pub octets: [u8; 16],
}

impl Ipv6Addr {
    /// The all-zeros unspecified address (::).
    pub const UNSPECIFIED: Self = Self { octets: [0; 16] };

    /// The loopback address (::1).
    pub const LOOPBACK: Self = Self {
        octets: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
    };

    /// Link-local prefix bytes (fe80::).
    pub const LINK_LOCAL_PREFIX: [u8; 2] = [0xfe, 0x80];

    /// Create an address from raw octets.
    pub fn new(octets: [u8; 16]) -> Self {
        Self { octets }
    }

    /// Create an address from eight 16-bit segments.
    pub fn from_segments(segs: [u16; 8]) -> Self {
        let mut octets = [0u8; 16];
        for (i, seg) in segs.iter().enumerate() {
            octets[i * 2] = (*seg >> 8) as u8;
            octets[i * 2 + 1] = (*seg & 0xff) as u8;
        }
        Self { octets }
    }

    /// Returns `true` if this is the loopback address (::1).
    pub fn is_loopback(&self) -> bool {
        *self == Self::LOOPBACK
    }

    /// Returns `true` if this is a link-local address (fe80::/10).
    pub fn is_link_local(&self) -> bool {
        self.octets[0] == 0xfe && (self.octets[1] & 0xc0) == 0x80
    }

    /// Returns `true` if this is a multicast address (ff00::/8).
    pub fn is_multicast(&self) -> bool {
        self.octets[0] == 0xff
    }

    /// Returns `true` if this is the unspecified address (::).
    pub fn is_unspecified(&self) -> bool {
        *self == Self::UNSPECIFIED
    }

    /// Format the address as a colon-hex string with `::` compression.
    pub fn display(&self) -> String {
        let segs = self.segments();

        // Find the longest run of consecutive zero segments for :: compression.
        let mut best_start: usize = 8;
        let mut best_len: usize = 0;
        let mut cur_start: usize = 0;
        let mut cur_len: usize = 0;
        for i in 0..8 {
            if segs[i] == 0 {
                if cur_len == 0 {
                    cur_start = i;
                }
                cur_len += 1;
            } else {
                if cur_len > best_len && cur_len >= 2 {
                    best_start = cur_start;
                    best_len = cur_len;
                }
                cur_len = 0;
            }
        }
        if cur_len > best_len && cur_len >= 2 {
            best_start = cur_start;
            best_len = cur_len;
        }

        let mut out = String::new();
        let mut i = 0;
        while i < 8 {
            if i == best_start && best_len > 0 {
                if i == 0 {
                    out.push(':');
                }
                out.push(':');
                i += best_len;
                continue;
            }
            if i > 0 && !(i == best_start + best_len && best_len > 0) {
                // Only add colon if we didn't just emit ::
            }
            if !out.is_empty() && !out.ends_with(':') {
                out.push(':');
            }
            out.push_str(&format!("{:x}", segs[i]));
            i += 1;
        }
        out
    }

    /// Parse an IPv6 address string (e.g. "2001:db8::1", "::1", "fe80::1").
    pub fn from_str(s: &str) -> Option<Self> {
        if s.is_empty() {
            return None;
        }

        // Split on "::" to handle compression.
        let parts: Vec<&str> = s.splitn(3, "::").collect();
        if parts.len() > 2 {
            // More than one :: is invalid... unless the split produced 3 from "::...::".
            return None;
        }

        let has_double_colon = s.contains("::");

        let left_segs = if parts[0].is_empty() {
            Vec::new()
        } else {
            parts[0].split(':').collect::<Vec<&str>>()
        };
        let right_segs = if has_double_colon && parts.len() == 2 && !parts[1].is_empty() {
            parts[1].split(':').collect::<Vec<&str>>()
        } else if !has_double_colon && parts.len() == 1 {
            // No ::, just split on :
            let all: Vec<&str> = s.split(':').collect();
            if all.len() != 8 {
                return None;
            }
            let mut segs = [0u16; 8];
            for (i, seg) in all.iter().enumerate() {
                segs[i] = parse_hex_u16(seg)?;
            }
            return Some(Self::from_segments(segs));
        } else {
            Vec::new()
        };

        if !has_double_colon {
            return None;
        }

        let total = left_segs.len() + right_segs.len();
        if total > 8 {
            return None;
        }
        let zero_fill = 8 - total;

        let mut segs = [0u16; 8];
        let mut idx = 0;
        for seg in &left_segs {
            segs[idx] = parse_hex_u16(seg)?;
            idx += 1;
        }
        idx += zero_fill; // skip zero-filled segments (already 0)
        for seg in &right_segs {
            segs[idx] = parse_hex_u16(seg)?;
            idx += 1;
        }

        Some(Self::from_segments(segs))
    }

    /// Generate a link-local address from a MAC address using EUI-64.
    pub fn from_mac(mac: &[u8; 6]) -> Self {
        let mut octets = [0u8; 16];
        octets[0] = 0xfe;
        octets[1] = 0x80;
        // octets[2..8] are zero (link-local prefix padding)
        octets[8] = mac[0] ^ 0x02; // flip universal/local bit
        octets[9] = mac[1];
        octets[10] = mac[2];
        octets[11] = 0xff;
        octets[12] = 0xfe;
        octets[13] = mac[3];
        octets[14] = mac[4];
        octets[15] = mac[5];
        Self { octets }
    }

    /// Extract the eight 16-bit segments.
    fn segments(&self) -> [u16; 8] {
        let mut segs = [0u16; 8];
        for i in 0..8 {
            segs[i] = ((self.octets[i * 2] as u16) << 8) | (self.octets[i * 2 + 1] as u16);
        }
        segs
    }
}

/// Parse a hex string (up to 4 digits) into a u16.
fn parse_hex_u16(s: &str) -> Option<u16> {
    if s.is_empty() || s.len() > 4 {
        return None;
    }
    let mut val: u16 = 0;
    for b in s.bytes() {
        let digit = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return None,
        };
        val = val.checked_mul(16)?.checked_add(digit as u16)?;
    }
    Some(val)
}

// ---------------------------------------------------------------------------
// IPv6 Header
// ---------------------------------------------------------------------------

/// An IPv6 packet header (40 bytes fixed).
pub struct Ipv6Header {
    /// IP version (always 6).
    pub version: u8,
    /// Traffic class (QoS).
    pub traffic_class: u8,
    /// Flow label (20 bits).
    pub flow_label: u32,
    /// Length of payload following this header, in bytes.
    pub payload_length: u16,
    /// Next header type (58 = ICMPv6, 6 = TCP, 17 = UDP).
    pub next_header: u8,
    /// Hop limit (analogous to IPv4 TTL).
    pub hop_limit: u8,
    /// Source address.
    pub src: Ipv6Addr,
    /// Destination address.
    pub dst: Ipv6Addr,
}

/// Build a 40-byte IPv6 header as raw bytes.
pub fn build_header(src: Ipv6Addr, dst: Ipv6Addr, next_header: u8,
                    payload_len: u16, hop_limit: u8) -> Vec<u8> {
    let mut buf = Vec::with_capacity(40);
    // Version (4 bits = 6), traffic class (8 bits = 0), flow label (20 bits = 0)
    buf.push(0x60); // version=6, tc high nibble=0
    buf.push(0x00); // tc low nibble + flow label high
    buf.push(0x00); // flow label mid
    buf.push(0x00); // flow label low
    // Payload length
    buf.push((payload_len >> 8) as u8);
    buf.push((payload_len & 0xff) as u8);
    // Next header
    buf.push(next_header);
    // Hop limit
    buf.push(hop_limit);
    // Source address (16 bytes)
    buf.extend_from_slice(&src.octets);
    // Destination address (16 bytes)
    buf.extend_from_slice(&dst.octets);
    buf
}

/// Parse a 40-byte IPv6 header from raw bytes.
/// Returns the parsed header and remaining payload slice.
pub fn parse_header(data: &[u8]) -> Result<(Ipv6Header, &[u8]), &'static str> {
    if data.len() < 40 {
        return Err("ipv6: packet too short for header");
    }
    let version = (data[0] >> 4) & 0x0f;
    if version != 6 {
        return Err("ipv6: version is not 6");
    }
    let traffic_class = ((data[0] & 0x0f) << 4) | ((data[1] >> 4) & 0x0f);
    let flow_label = ((data[1] as u32 & 0x0f) << 16)
        | ((data[2] as u32) << 8)
        | (data[3] as u32);
    let payload_length = ((data[4] as u16) << 8) | (data[5] as u16);
    let next_header = data[6];
    let hop_limit = data[7];

    let mut src_octets = [0u8; 16];
    src_octets.copy_from_slice(&data[8..24]);
    let mut dst_octets = [0u8; 16];
    dst_octets.copy_from_slice(&data[24..40]);

    let header = Ipv6Header {
        version,
        traffic_class,
        flow_label,
        payload_length,
        next_header,
        hop_limit,
        src: Ipv6Addr::new(src_octets),
        dst: Ipv6Addr::new(dst_octets),
    };

    let payload_end = 40 + (payload_length as usize);
    let payload = if data.len() >= payload_end {
        &data[40..payload_end]
    } else {
        &data[40..]
    };

    Ok((header, payload))
}

// ---------------------------------------------------------------------------
// ICMPv6
// ---------------------------------------------------------------------------

/// ICMPv6 Echo Request type.
pub const ICMPV6_ECHO_REQUEST: u8 = 128;
/// ICMPv6 Echo Reply type.
pub const ICMPV6_ECHO_REPLY: u8 = 129;
/// ICMPv6 Router Solicitation type.
pub const ICMPV6_ROUTER_SOLICIT: u8 = 133;
/// ICMPv6 Router Advertisement type.
pub const ICMPV6_ROUTER_ADVERT: u8 = 134;
/// ICMPv6 Neighbor Solicitation type.
pub const ICMPV6_NEIGHBOR_SOLICIT: u8 = 135;
/// ICMPv6 Neighbor Advertisement type.
pub const ICMPV6_NEIGHBOR_ADVERT: u8 = 136;

/// Parsed ICMPv6 message.
pub enum IcmpV6Message {
    EchoRequest { id: u16, seq: u16, data: Vec<u8> },
    EchoReply { id: u16, seq: u16, data: Vec<u8> },
    NeighborSolicit { target: Ipv6Addr },
    NeighborAdvert { target: Ipv6Addr },
    RouterSolicit,
    RouterAdvert,
    Unknown { msg_type: u8, code: u8 },
}

/// Build an ICMPv6 Echo Request payload (type 128).
pub fn build_echo_request(id: u16, seq: u16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8);
    buf.push(ICMPV6_ECHO_REQUEST); // type
    buf.push(0);                    // code
    buf.push(0); buf.push(0);      // checksum placeholder
    buf.push((id >> 8) as u8);
    buf.push((id & 0xff) as u8);
    buf.push((seq >> 8) as u8);
    buf.push((seq & 0xff) as u8);
    // Compute simple checksum over the ICMPv6 body.
    let cksum = icmpv6_checksum(&buf);
    buf[2] = (cksum >> 8) as u8;
    buf[3] = (cksum & 0xff) as u8;
    buf
}

/// Build an ICMPv6 Neighbor Solicitation message for `target`.
pub fn build_neighbor_solicit(target: Ipv6Addr) -> Vec<u8> {
    let mut buf = Vec::with_capacity(24);
    buf.push(ICMPV6_NEIGHBOR_SOLICIT); // type
    buf.push(0);                        // code
    buf.push(0); buf.push(0);          // checksum placeholder
    buf.push(0); buf.push(0); buf.push(0); buf.push(0); // reserved
    buf.extend_from_slice(&target.octets);
    let cksum = icmpv6_checksum(&buf);
    buf[2] = (cksum >> 8) as u8;
    buf[3] = (cksum & 0xff) as u8;
    buf
}

/// Parse an ICMPv6 message from raw payload bytes.
pub fn parse_icmpv6(data: &[u8]) -> Result<IcmpV6Message, &'static str> {
    if data.len() < 4 {
        return Err("icmpv6: too short");
    }
    let msg_type = data[0];
    let code = data[1];
    match msg_type {
        ICMPV6_ECHO_REQUEST | ICMPV6_ECHO_REPLY => {
            if data.len() < 8 {
                return Err("icmpv6: echo too short");
            }
            let id = ((data[4] as u16) << 8) | (data[5] as u16);
            let seq = ((data[6] as u16) << 8) | (data[7] as u16);
            let payload = if data.len() > 8 { data[8..].to_vec() } else { Vec::new() };
            if msg_type == ICMPV6_ECHO_REQUEST {
                Ok(IcmpV6Message::EchoRequest { id, seq, data: payload })
            } else {
                Ok(IcmpV6Message::EchoReply { id, seq, data: payload })
            }
        }
        ICMPV6_NEIGHBOR_SOLICIT => {
            if data.len() < 24 {
                return Err("icmpv6: neighbor solicit too short");
            }
            let mut target = [0u8; 16];
            target.copy_from_slice(&data[8..24]);
            Ok(IcmpV6Message::NeighborSolicit { target: Ipv6Addr::new(target) })
        }
        ICMPV6_NEIGHBOR_ADVERT => {
            if data.len() < 24 {
                return Err("icmpv6: neighbor advert too short");
            }
            let mut target = [0u8; 16];
            target.copy_from_slice(&data[8..24]);
            Ok(IcmpV6Message::NeighborAdvert { target: Ipv6Addr::new(target) })
        }
        ICMPV6_ROUTER_SOLICIT => Ok(IcmpV6Message::RouterSolicit),
        ICMPV6_ROUTER_ADVERT => Ok(IcmpV6Message::RouterAdvert),
        _ => Ok(IcmpV6Message::Unknown { msg_type, code }),
    }
}

/// Simple one's-complement checksum for ICMPv6 (without pseudo-header).
fn icmpv6_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        // Skip the checksum field itself (bytes 2-3).
        if i == 2 {
            i += 2;
            continue;
        }
        sum += ((data[i] as u32) << 8) | (data[i + 1] as u32);
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

// ---------------------------------------------------------------------------
// Neighbor Discovery Protocol (NDP)
// ---------------------------------------------------------------------------

/// State of a neighbor cache entry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NeighborState {
    /// Address resolution in progress — no MAC yet.
    Incomplete,
    /// Recently confirmed reachable.
    Reachable,
    /// Not recently confirmed; may still be reachable.
    Stale,
    /// Waiting before probing.
    Delay,
    /// Actively probing for reachability.
    Probe,
}

/// A single neighbor cache entry.
struct NeighborEntry {
    ipv6: Ipv6Addr,
    mac: [u8; 6],
    state: NeighborState,
    last_seen: u64,
}

const MAX_NEIGHBOR_ENTRIES: usize = 64;

static NDP_TABLE: Mutex<Vec<NeighborEntry>> = Mutex::new(Vec::new());

/// Look up the MAC address for an IPv6 neighbor.
pub fn ndp_lookup(addr: &Ipv6Addr) -> Option<[u8; 6]> {
    let table = NDP_TABLE.lock();
    table.iter()
        .find(|e| e.ipv6 == *addr && e.state != NeighborState::Incomplete)
        .map(|e| e.mac)
}

/// Add or update a neighbor cache entry.
pub fn ndp_add(ipv6: Ipv6Addr, mac: [u8; 6]) {
    let tick = crate::timer::ticks();
    let mut table = NDP_TABLE.lock();
    for entry in table.iter_mut() {
        if entry.ipv6 == ipv6 {
            entry.mac = mac;
            entry.state = NeighborState::Reachable;
            entry.last_seen = tick;
            return;
        }
    }
    if table.len() < MAX_NEIGHBOR_ENTRIES {
        table.push(NeighborEntry {
            ipv6,
            mac,
            state: NeighborState::Reachable,
            last_seen: tick,
        });
    }
}

/// Format the neighbor cache as a human-readable table.
pub fn ndp_table() -> String {
    let table = NDP_TABLE.lock();
    if table.is_empty() {
        return format!("(neighbor cache empty)\n");
    }
    let mut out = String::new();
    out.push_str("IPv6 Address                             MAC               State       Age\n");
    out.push_str("---------------------------------------- ----------------- ----------- ------\n");
    let now = crate::timer::ticks();
    for e in table.iter() {
        let addr = e.ipv6.display();
        let mac = format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            e.mac[0], e.mac[1], e.mac[2], e.mac[3], e.mac[4], e.mac[5]);
        let state = match e.state {
            NeighborState::Incomplete => "INCOMPLETE",
            NeighborState::Reachable  => "REACHABLE ",
            NeighborState::Stale      => "STALE     ",
            NeighborState::Delay      => "DELAY     ",
            NeighborState::Probe      => "PROBE     ",
        };
        let age = now.wrapping_sub(e.last_seen);
        out.push_str(&format!("{:<40} {} {} {}\n", addr, mac, state, age));
    }
    out
}

// ---------------------------------------------------------------------------
// IPv6 Routing
// ---------------------------------------------------------------------------

/// A single IPv6 routing table entry.
struct Ipv6Route {
    prefix: Ipv6Addr,
    prefix_len: u8,
    gateway: Ipv6Addr,
    interface: String,
}

const MAX_ROUTES: usize = 32;

static ROUTING_TABLE: Mutex<Vec<Ipv6Route>> = Mutex::new(Vec::new());

/// Add a route to the IPv6 routing table.
pub fn add_route(prefix: Ipv6Addr, prefix_len: u8, gateway: Ipv6Addr) {
    let mut table = ROUTING_TABLE.lock();
    if table.len() >= MAX_ROUTES {
        return;
    }
    table.push(Ipv6Route {
        prefix,
        prefix_len,
        gateway,
        interface: String::from("eth0"),
    });
}

/// Remove a route matching the given prefix and length.
pub fn remove_route(prefix: &Ipv6Addr, prefix_len: u8) {
    let mut table = ROUTING_TABLE.lock();
    table.retain(|r| !(r.prefix == *prefix && r.prefix_len == prefix_len));
}

/// Longest-prefix-match lookup. Returns the gateway address if found.
pub fn lookup_route(dst: &Ipv6Addr) -> Option<Ipv6Addr> {
    let table = ROUTING_TABLE.lock();
    let mut best: Option<&Ipv6Route> = None;
    for route in table.iter() {
        if prefix_matches(&route.prefix, route.prefix_len, dst) {
            if best.is_none() || route.prefix_len > best.unwrap().prefix_len {
                best = Some(route);
            }
        }
    }
    best.map(|r| r.gateway)
}

/// Check whether `addr` matches `prefix/prefix_len`.
fn prefix_matches(prefix: &Ipv6Addr, prefix_len: u8, addr: &Ipv6Addr) -> bool {
    let full_bytes = (prefix_len / 8) as usize;
    let remaining_bits = prefix_len % 8;
    if full_bytes > 16 {
        return false;
    }
    for i in 0..full_bytes {
        if prefix.octets[i] != addr.octets[i] {
            return false;
        }
    }
    if remaining_bits > 0 && full_bytes < 16 {
        let mask = 0xffu8 << (8 - remaining_bits);
        if (prefix.octets[full_bytes] & mask) != (addr.octets[full_bytes] & mask) {
            return false;
        }
    }
    true
}

/// Format the IPv6 routing table as a human-readable string.
pub fn routing_table() -> String {
    let table = ROUTING_TABLE.lock();
    if table.is_empty() {
        return format!("(no IPv6 routes)\n");
    }
    let mut out = String::new();
    out.push_str("Prefix                                    Len  Gateway                                  Iface\n");
    out.push_str("----------------------------------------- ---- ---------------------------------------- -----\n");
    for r in table.iter() {
        out.push_str(&format!("{:<41} /{:<3} {:<40} {}\n",
            r.prefix.display(), r.prefix_len, r.gateway.display(), r.interface));
    }
    out
}

// ---------------------------------------------------------------------------
// Global state & public API
// ---------------------------------------------------------------------------

/// Our link-local IPv6 address.
static OUR_ADDR: Mutex<Ipv6Addr> = Mutex::new(Ipv6Addr::UNSPECIFIED);

/// Default MAC used when no real NIC is available.
const DEFAULT_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

/// Packet counters.
static PACKETS_SENT: AtomicU64 = AtomicU64::new(0);
static PACKETS_RECV: AtomicU64 = AtomicU64::new(0);
static PINGS_SENT: AtomicU64 = AtomicU64::new(0);

/// Initialise the IPv6 subsystem: generate a link-local address from the
/// default MAC and add a link-local route.
pub fn init() {
    let ll = Ipv6Addr::from_mac(&DEFAULT_MAC);
    *OUR_ADDR.lock() = ll;

    // Add link-local route (fe80::/10 -> on-link).
    let prefix = Ipv6Addr::from_segments([0xfe80, 0, 0, 0, 0, 0, 0, 0]);
    add_route(prefix, 10, Ipv6Addr::UNSPECIFIED);

    // Add loopback route.
    add_route(Ipv6Addr::LOOPBACK, 128, Ipv6Addr::LOOPBACK);

    crate::serial_println!("[ipv6] initialised, link-local: {}", ll.display());
}

/// Return our current IPv6 address.
pub fn our_ipv6() -> Ipv6Addr {
    *OUR_ADDR.lock()
}

/// Return a summary of IPv6 subsystem state.
pub fn ipv6_info() -> String {
    let addr = our_ipv6();
    let mut out = String::new();
    out.push_str(&format!("ipv6: address {}\n", addr.display()));
    out.push_str(&format!("ipv6: loopback {}\n", if addr.is_loopback() { "yes" } else { "no" }));
    out.push_str(&format!("ipv6: link-local {}\n", if addr.is_link_local() { "yes" } else { "no" }));
    out.push_str(&format!("ipv6: neighbor cache entries: {}\n", NDP_TABLE.lock().len()));
    out.push_str(&format!("ipv6: routing table entries: {}\n", ROUTING_TABLE.lock().len()));
    out
}

/// Return packet statistics.
pub fn ipv6_stats() -> String {
    format!(
        "ipv6: packets sent: {}, received: {}, pings: {}\n",
        PACKETS_SENT.load(Ordering::Relaxed),
        PACKETS_RECV.load(Ordering::Relaxed),
        PINGS_SENT.load(Ordering::Relaxed),
    )
}

/// Send an IPv6 packet to `dst` with the given next-header and payload.
/// Returns `true` if the packet was built and queued (simulated).
pub fn send_ipv6(dst: Ipv6Addr, next_header: u8, payload: &[u8]) -> bool {
    let src = our_ipv6();
    if src.is_unspecified() {
        return false;
    }
    let _header = build_header(src, dst, next_header, payload.len() as u16, 64);
    // In a full implementation this would be handed to the NIC driver.
    PACKETS_SENT.fetch_add(1, Ordering::Relaxed);
    true
}

/// Send an ICMPv6 echo request (ping6) and return a status string.
pub fn ping6(dst: &Ipv6Addr) -> String {
    let seq = PINGS_SENT.fetch_add(1, Ordering::Relaxed) as u16;
    let echo = build_echo_request(1, seq);
    if send_ipv6(*dst, 58, &echo) {
        format!("ping6 {} seq={}: sent ({} bytes)", dst.display(), seq, echo.len())
    } else {
        format!("ping6 {}: failed (no source address)", dst.display())
    }
}
