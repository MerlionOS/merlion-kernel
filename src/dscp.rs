/// DSCP (Differentiated Services Code Point) and ECN marking for MerlionOS.
/// Classifies and marks packets for Quality of Service handling.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// DSCP values (standard Per-Hop Behaviours)
// ---------------------------------------------------------------------------

pub const DSCP_CS0: u8 = 0;    // Best Effort
pub const DSCP_CS1: u8 = 8;    // Scavenger
pub const DSCP_AF11: u8 = 10;  // Assured Forwarding 1-1
pub const DSCP_AF12: u8 = 12;
pub const DSCP_AF13: u8 = 14;
pub const DSCP_AF21: u8 = 18;  // Assured Forwarding 2-1
pub const DSCP_AF22: u8 = 20;
pub const DSCP_AF23: u8 = 22;
pub const DSCP_AF31: u8 = 26;  // Assured Forwarding 3-1
pub const DSCP_AF32: u8 = 28;
pub const DSCP_AF33: u8 = 30;
pub const DSCP_AF41: u8 = 34;  // Assured Forwarding 4-1
pub const DSCP_AF42: u8 = 36;
pub const DSCP_AF43: u8 = 38;
pub const DSCP_CS5: u8 = 40;   // Signaling
pub const DSCP_EF: u8 = 46;    // Expedited Forwarding (VoIP)
pub const DSCP_CS6: u8 = 48;   // Network Control
pub const DSCP_CS7: u8 = 56;   // Network Control

// ---------------------------------------------------------------------------
// ECN (Explicit Congestion Notification)
// ---------------------------------------------------------------------------

pub const ECN_NOT_ECT: u8 = 0;  // Not ECN-Capable
pub const ECN_ECT1: u8 = 1;     // ECN-Capable Transport
pub const ECN_ECT0: u8 = 2;     // ECN-Capable Transport
pub const ECN_CE: u8 = 3;       // Congestion Experienced

// ---------------------------------------------------------------------------
// Marking operations
// ---------------------------------------------------------------------------

/// Extract the DSCP value from an IP header's TOS/DS field.
/// Returns the upper 6 bits of byte 1 (the TOS field).
pub fn get_dscp(ip_header: &[u8]) -> u8 {
    if ip_header.len() < 2 { return 0; }
    ip_header[1] >> 2
}

/// Set the DSCP value in an IP header, preserving the ECN bits.
pub fn set_dscp(ip_header: &mut [u8], dscp: u8) {
    if ip_header.len() < 2 { return; }
    let ecn = ip_header[1] & 0x03;
    ip_header[1] = (dscp << 2) | ecn;
}

/// Extract the ECN field from an IP header (lower 2 bits of TOS).
pub fn get_ecn(ip_header: &[u8]) -> u8 {
    if ip_header.len() < 2 { return 0; }
    ip_header[1] & 0x03
}

/// Set the ECN field in an IP header, preserving the DSCP bits.
pub fn set_ecn(ip_header: &mut [u8], ecn: u8) {
    if ip_header.len() < 2 { return; }
    let dscp_bits = ip_header[1] & 0xFC;
    ip_header[1] = dscp_bits | (ecn & 0x03);
}

/// Mark a packet with ECN Congestion Experienced (CE).
pub fn mark_ecn_ce(ip_header: &mut [u8]) {
    set_ecn(ip_header, ECN_CE);
}

/// Return a human-readable name for a DSCP value.
pub fn dscp_name(dscp: u8) -> &'static str {
    match dscp {
        0 => "CS0 (Best Effort)",
        8 => "CS1 (Scavenger)",
        10 => "AF11",
        12 => "AF12",
        14 => "AF13",
        16 => "CS2",
        18 => "AF21",
        20 => "AF22",
        22 => "AF23",
        24 => "CS3",
        26 => "AF31",
        28 => "AF32",
        30 => "AF33",
        32 => "CS4",
        34 => "AF41",
        36 => "AF42",
        38 => "AF43",
        40 => "CS5 (Signaling)",
        46 => "EF (Expedited)",
        48 => "CS6 (Network Ctrl)",
        56 => "CS7 (Network Ctrl)",
        _ => "Unknown",
    }
}

/// Return a human-readable name for an ECN value.
pub fn ecn_name(ecn: u8) -> &'static str {
    match ecn & 0x03 {
        0 => "Not-ECT",
        1 => "ECT(1)",
        2 => "ECT(0)",
        3 => "CE (Congestion)",
        _ => "Unknown",
    }
}

// ---------------------------------------------------------------------------
// Classification rules
// ---------------------------------------------------------------------------

/// A rule that maps traffic (by protocol/port) to a DSCP value.
#[derive(Debug, Clone)]
pub struct DscpRule {
    /// Auto-assigned rule id.
    pub id: u32,
    /// IP protocol number (6=TCP, 17=UDP).
    pub protocol: Option<u8>,
    /// Destination port to match.
    pub port: Option<u16>,
    /// DSCP value to assign.
    pub dscp: u8,
    /// Human-readable description.
    pub description: &'static str,
}

/// Global classification rule table.
struct DscpState {
    rules: Vec<DscpRule>,
    next_id: u32,
}

impl DscpState {
    fn new() -> Self {
        Self { rules: Vec::new(), next_id: 1 }
    }

    fn add_rule(&mut self, protocol: Option<u8>, port: Option<u16>,
                dscp: u8, description: &'static str) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.rules.push(DscpRule { id, protocol, port, dscp, description });
        id
    }

    fn remove_rule(&mut self, id: u32) -> bool {
        if let Some(pos) = self.rules.iter().position(|r| r.id == id) {
            self.rules.remove(pos);
            true
        } else {
            false
        }
    }

    /// Classify a packet and return the DSCP value to apply.
    fn classify(&self, packet: &[u8]) -> u8 {
        if packet.len() < 24 {
            return DSCP_CS0;
        }
        let proto = packet[9];
        let dst_port = if proto == 6 || proto == 17 {
            u16::from_be_bytes([packet[22], packet[23]])
        } else {
            0
        };

        for rule in &self.rules {
            if let Some(rp) = rule.protocol {
                if rp != proto { continue; }
            }
            if let Some(rport) = rule.port {
                if rport != dst_port { continue; }
            }
            return rule.dscp;
        }
        DSCP_CS0
    }
}

static DSCP_STATE: Mutex<Option<DscpState>> = Mutex::new(None);

/// Statistics counters.
static PACKETS_CLASSIFIED: AtomicU64 = AtomicU64::new(0);
static PACKETS_MARKED: AtomicU64 = AtomicU64::new(0);
static ECN_CE_MARKED: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise DSCP subsystem with default classification rules.
pub fn init() {
    let mut state = DscpState::new();
    // Default rules
    state.add_rule(Some(6), Some(22), DSCP_AF21, "SSH");
    state.add_rule(Some(6), Some(80), DSCP_AF21, "HTTP");
    state.add_rule(Some(6), Some(443), DSCP_AF21, "HTTPS");
    state.add_rule(Some(17), Some(5060), DSCP_EF, "VoIP SIP");
    state.add_rule(Some(17), Some(53), DSCP_AF41, "DNS");
    state.add_rule(Some(6), Some(53), DSCP_AF41, "DNS/TCP");
    *DSCP_STATE.lock() = Some(state);
}

/// Add a classification rule. Returns the rule id.
pub fn add_rule(protocol: Option<u8>, port: Option<u16>,
                dscp: u8, description: &'static str) -> Option<u32> {
    let mut g = DSCP_STATE.lock();
    g.as_mut().map(|s| s.add_rule(protocol, port, dscp, description))
}

/// Remove a classification rule by id.
pub fn remove_rule(id: u32) -> bool {
    let mut g = DSCP_STATE.lock();
    g.as_mut().map_or(false, |s| s.remove_rule(id))
}

/// Classify a packet and return the appropriate DSCP value.
pub fn classify(packet: &[u8]) -> u8 {
    PACKETS_CLASSIFIED.fetch_add(1, Ordering::Relaxed);
    let g = DSCP_STATE.lock();
    match g.as_ref() {
        Some(s) => s.classify(packet),
        None => DSCP_CS0,
    }
}

/// Classify a packet and mark it with the resulting DSCP value in-place.
pub fn classify_and_mark(packet: &mut [u8]) {
    let dscp = classify(packet);
    set_dscp(packet, dscp);
    PACKETS_MARKED.fetch_add(1, Ordering::Relaxed);
}

/// Return summary info about the DSCP subsystem.
pub fn dscp_info() -> String {
    let g = DSCP_STATE.lock();
    let mut out = String::from("DSCP / ECN — MerlionOS QoS Marking\n");
    out.push_str("───────────────────────────────────\n");

    out.push_str("Standard DSCP values:\n");
    for &(val, name) in &[
        (DSCP_CS0, "CS0  Best Effort"),
        (DSCP_CS1, "CS1  Scavenger"),
        (DSCP_AF11, "AF11"), (DSCP_AF12, "AF12"), (DSCP_AF13, "AF13"),
        (DSCP_AF21, "AF21"), (DSCP_AF22, "AF22"), (DSCP_AF23, "AF23"),
        (DSCP_AF31, "AF31"), (DSCP_AF32, "AF32"), (DSCP_AF33, "AF33"),
        (DSCP_AF41, "AF41"), (DSCP_AF42, "AF42"), (DSCP_AF43, "AF43"),
        (DSCP_CS5, "CS5  Signaling"),
        (DSCP_EF, "EF   Expedited Forwarding"),
        (DSCP_CS6, "CS6  Network Control"),
        (DSCP_CS7, "CS7  Network Control"),
    ] {
        out.push_str(&format!("  DSCP {:>2} = {}\n", val, name));
    }

    out.push_str("\nECN values:\n");
    out.push_str("  0 = Not-ECT  1 = ECT(1)  2 = ECT(0)  3 = CE\n");

    if let Some(ref s) = *g {
        out.push_str(&format!("\nClassification rules: {}\n", s.rules.len()));
    }
    out
}

/// List all classification rules.
pub fn list_rules() -> String {
    let g = DSCP_STATE.lock();
    let g = match g.as_ref() {
        Some(g) => g,
        None => return String::from("(DSCP not initialised)\n"),
    };

    if g.rules.is_empty() {
        return String::from("(no classification rules)\n");
    }

    let mut out = String::from("ID   PROTO  PORT   DSCP  DESCRIPTION\n");
    out.push_str("---- ------ ------ ----- -----------\n");
    for r in &g.rules {
        let proto = match r.protocol {
            Some(6) => "TCP   ",
            Some(17) => "UDP   ",
            Some(1) => "ICMP  ",
            _ => "*     ",
        };
        let port = match r.port {
            Some(p) => format!("{:<6}", p),
            None => String::from("*     "),
        };
        out.push_str(&format!(
            "{:<4} {} {} {:<5} {}\n",
            r.id, proto, port, r.dscp, r.description,
        ));
    }
    out
}

/// Return DSCP statistics.
pub fn dscp_stats() -> String {
    let classified = PACKETS_CLASSIFIED.load(Ordering::Relaxed);
    let marked = PACKETS_MARKED.load(Ordering::Relaxed);
    let ce = ECN_CE_MARKED.load(Ordering::Relaxed);

    format!(
        "DSCP/ECN Statistics\n\
         ───────────────────\n\
         Packets classified: {}\n\
         Packets marked:     {}\n\
         ECN CE marked:      {}\n",
        classified, marked, ce,
    )
}
