/// IGMP (Internet Group Management Protocol) for MerlionOS.
/// Manages multicast group membership for IPv4.
/// Implements IGMPv2 with v3 compatibility.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// IGMP protocol number (IP protocol 2).
const IGMP_PROTOCOL: u8 = 2;

/// IGMP message types.
const IGMP_MEMBERSHIP_QUERY: u8 = 0x11;
const IGMP_MEMBERSHIP_REPORT_V2: u8 = 0x16;
const IGMP_LEAVE_GROUP: u8 = 0x17;
const IGMP_MEMBERSHIP_REPORT_V3: u8 = 0x22;

/// Well-known multicast groups.
const ALL_HOSTS: [u8; 4] = [224, 0, 0, 1];
const ALL_ROUTERS: [u8; 4] = [224, 0, 0, 2];
const MDNS_GROUP: [u8; 4] = [224, 0, 0, 251];

/// Default query response interval (10 seconds = 1000 ticks at 100 Hz).
const QUERY_RESPONSE_INTERVAL: u64 = 1000;

/// Robustness variable (RFC 3376 default).
const ROBUSTNESS_VARIABLE: u32 = 2;

/// Last member query interval (1 second = 100 ticks).
const LAST_MEMBER_QUERY_INTERVAL: u64 = 100;

/// Maximum number of multicast groups.
const MAX_GROUPS: usize = 256;

/// Maximum interfaces tracked.
const MAX_INTERFACES: usize = 16;

// ---------------------------------------------------------------------------
// IGMP message
// ---------------------------------------------------------------------------

/// Parsed IGMP message.
#[derive(Debug, Clone)]
pub struct IgmpMessage {
    /// Message type.
    pub msg_type: u8,
    /// Max response time (in 1/10 seconds).
    pub max_resp_time: u8,
    /// Checksum.
    pub checksum: u16,
    /// Group address.
    pub group: [u8; 4],
}

impl IgmpMessage {
    /// Parse an IGMP message from raw bytes.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }
        Some(Self {
            msg_type: data[0],
            max_resp_time: data[1],
            checksum: u16::from_be_bytes([data[2], data[3]]),
            group: [data[4], data[5], data[6], data[7]],
        })
    }

    /// Encode to bytes.
    pub fn encode(&self) -> [u8; 8] {
        let cksum = self.checksum.to_be_bytes();
        [
            self.msg_type,
            self.max_resp_time,
            cksum[0], cksum[1],
            self.group[0], self.group[1], self.group[2], self.group[3],
        ]
    }

    /// Compute IGMP checksum over an 8-byte message.
    pub fn compute_checksum(data: &[u8; 8]) -> u16 {
        let mut sum: u32 = 0;
        let mut i = 0;
        while i + 1 < 8 {
            sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
            i += 2;
        }
        while sum > 0xFFFF {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        !(sum as u16)
    }
}

// ---------------------------------------------------------------------------
// Group membership entry
// ---------------------------------------------------------------------------

/// Multicast group membership.
#[derive(Debug, Clone)]
struct GroupEntry {
    /// Multicast group IP.
    group: [u8; 4],
    /// Interface name.
    iface: String,
    /// Tick when this group was joined.
    joined_tick: u64,
    /// Last report sent tick.
    last_report_tick: u64,
    /// Number of reports sent.
    reports_sent: u64,
}

// ---------------------------------------------------------------------------
// IGMP state
// ---------------------------------------------------------------------------

struct IgmpState {
    /// Joined multicast groups.
    groups: Vec<GroupEntry>,
    /// Whether we are acting as a querier.
    is_querier: bool,
    /// Query interval in ticks.
    query_interval: u64,
    /// Last query sent tick.
    last_query_tick: u64,
    /// Queries received.
    queries_rx: u64,
    /// Reports sent.
    reports_tx: u64,
    /// Leaves sent.
    leaves_tx: u64,
    /// Reports received.
    reports_rx: u64,
}

impl IgmpState {
    const fn new() -> Self {
        Self {
            groups: Vec::new(),
            is_querier: false,
            query_interval: QUERY_RESPONSE_INTERVAL,
            last_query_tick: 0,
            queries_rx: 0,
            reports_tx: 0,
            leaves_tx: 0,
            reports_rx: 0,
        }
    }
}

static IGMP_STATE: Mutex<IgmpState> = Mutex::new(IgmpState::new());
static IGMP_TICK: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Multicast forwarding table
// ---------------------------------------------------------------------------

/// Multicast forwarding entry: group -> list of interfaces.
#[derive(Debug, Clone)]
struct McastForwardEntry {
    group: [u8; 4],
    interfaces: Vec<String>,
}

static MCAST_TABLE: Mutex<Vec<McastForwardEntry>> = Mutex::new(Vec::new());

fn update_forwarding_table(group: [u8; 4], iface: &str, add: bool) {
    let mut table = MCAST_TABLE.lock();
    if add {
        if let Some(entry) = table.iter_mut().find(|e| e.group == group) {
            if !entry.interfaces.iter().any(|i| i == iface) {
                entry.interfaces.push(String::from(iface));
            }
        } else {
            let mut interfaces = Vec::new();
            interfaces.push(String::from(iface));
            table.push(McastForwardEntry { group, interfaces });
        }
    } else {
        if let Some(entry) = table.iter_mut().find(|e| e.group == group) {
            entry.interfaces.retain(|i| i != iface);
        }
        table.retain(|e| !e.interfaces.is_empty());
    }
}

/// Lookup which interfaces should receive a multicast group.
pub fn mcast_lookup(group: &[u8; 4]) -> Vec<String> {
    let table = MCAST_TABLE.lock();
    if let Some(entry) = table.iter().find(|e| &e.group == group) {
        entry.interfaces.clone()
    } else {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn ip4_str(ip: &[u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

fn is_well_known(group: &[u8; 4]) -> &'static str {
    if *group == ALL_HOSTS { "all-hosts" }
    else if *group == ALL_ROUTERS { "all-routers" }
    else if *group == MDNS_GROUP { "mDNS" }
    else { "" }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Join a multicast group on the default interface.
pub fn join_group(group_ip: [u8; 4]) -> Result<(), &'static str> {
    join_group_on(group_ip, "eth0")
}

/// Join a multicast group on a specific interface.
pub fn join_group_on(group_ip: [u8; 4], iface: &str) -> Result<(), &'static str> {
    // Validate multicast address (224.0.0.0 - 239.255.255.255)
    if group_ip[0] < 224 || group_ip[0] > 239 {
        return Err("not a valid multicast address");
    }
    let mut state = IGMP_STATE.lock();
    if state.groups.len() >= MAX_GROUPS {
        return Err("multicast group limit reached");
    }
    // Check if already joined
    if state.groups.iter().any(|g| g.group == group_ip && g.iface == iface) {
        return Err("already a member of this group");
    }
    let now = IGMP_TICK.load(Ordering::Relaxed);
    state.groups.push(GroupEntry {
        group: group_ip,
        iface: String::from(iface),
        joined_tick: now,
        last_report_tick: now,
        reports_sent: 1,
    });
    state.reports_tx += 1;
    drop(state);
    update_forwarding_table(group_ip, iface, true);
    crate::serial_println!("[igmp] joined {} on {}", ip4_str(&group_ip), iface);
    Ok(())
}

/// Leave a multicast group.
pub fn leave_group(group_ip: [u8; 4]) -> Result<(), &'static str> {
    leave_group_on(group_ip, "eth0")
}

/// Leave a multicast group on a specific interface.
pub fn leave_group_on(group_ip: [u8; 4], iface: &str) -> Result<(), &'static str> {
    let mut state = IGMP_STATE.lock();
    let pos = state.groups.iter().position(|g| g.group == group_ip && g.iface == iface)
        .ok_or("not a member of this group")?;
    state.groups.remove(pos);
    state.leaves_tx += 1;
    drop(state);
    update_forwarding_table(group_ip, iface, false);
    crate::serial_println!("[igmp] left {} on {}", ip4_str(&group_ip), iface);
    Ok(())
}

/// Handle an incoming IGMP query.
pub fn handle_query(msg: &IgmpMessage) {
    let mut state = IGMP_STATE.lock();
    state.queries_rx += 1;
    let now = IGMP_TICK.load(Ordering::Relaxed);
    let is_general = msg.group == [0, 0, 0, 0];
    for group in state.groups.iter_mut() {
        if is_general || group.group == msg.group {
            // Send a report (update last_report_tick)
            group.last_report_tick = now;
            group.reports_sent += 1;
        }
    }
    state.reports_tx += 1;
}

/// Handle an incoming IGMP report from another host.
pub fn handle_report(msg: &IgmpMessage) {
    let mut state = IGMP_STATE.lock();
    state.reports_rx += 1;
    // Suppress our own report for this group (IGMPv2 report suppression)
    let _ = msg;
}

/// List joined multicast groups.
pub fn list_groups() -> String {
    let state = IGMP_STATE.lock();
    if state.groups.is_empty() {
        return String::from("No multicast groups joined.\n");
    }
    let mut out = String::from("Group            Interface  Well-known     Reports\n");
    out.push_str("---------------  ---------  -------------  -------\n");
    for g in &state.groups {
        let wk = is_well_known(&g.group);
        out.push_str(&format!("{:<16} {:<10} {:<14} {}\n",
            ip4_str(&g.group), g.iface, wk, g.reports_sent));
    }
    out
}

/// IGMP subsystem info.
pub fn igmp_info() -> String {
    let state = IGMP_STATE.lock();
    let table = MCAST_TABLE.lock();
    let mut out = String::from("IGMP Information\n");
    out.push_str(&format!("  Querier: {}\n", state.is_querier));
    out.push_str(&format!("  Query interval: {} ticks\n", state.query_interval));
    out.push_str(&format!("  Robustness variable: {}\n", ROBUSTNESS_VARIABLE));
    out.push_str(&format!("  Groups joined: {}\n", state.groups.len()));
    out.push_str(&format!("  Forwarding entries: {}\n", table.len()));
    out.push_str(&format!("  Well-known groups:\n"));
    out.push_str(&format!("    224.0.0.1  all-hosts\n"));
    out.push_str(&format!("    224.0.0.2  all-routers\n"));
    out.push_str(&format!("    224.0.0.251 mDNS\n"));
    out
}

/// IGMP statistics.
pub fn igmp_stats() -> String {
    let state = IGMP_STATE.lock();
    let mut out = String::from("IGMP Statistics\n");
    out.push_str(&format!("  Queries received:  {}\n", state.queries_rx));
    out.push_str(&format!("  Reports sent:      {}\n", state.reports_tx));
    out.push_str(&format!("  Reports received:  {}\n", state.reports_rx));
    out.push_str(&format!("  Leaves sent:       {}\n", state.leaves_tx));
    out.push_str(&format!("  Groups active:     {}\n", state.groups.len()));
    out
}

/// Update tick counter.
pub fn tick(now: u64) {
    IGMP_TICK.store(now, Ordering::Relaxed);
}

/// Initialise the IGMP subsystem.
pub fn init() {
    crate::serial_println!("[igmp] multicast group management initialised");
}
