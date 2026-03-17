/// SNMP (Simple Network Management Protocol) agent for MerlionOS.
/// Implements SNMPv2c with MIB-II for network monitoring.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default SNMP port.
const SNMP_PORT: u16 = 161;

/// SNMP trap port.
const SNMP_TRAP_PORT: u16 = 162;

/// Maximum OID depth.
const MAX_OID_LEN: usize = 32;

/// Maximum MIB entries.
const MAX_MIB_ENTRIES: usize = 256;

/// Maximum community strings.
const MAX_COMMUNITIES: usize = 8;

// ---------------------------------------------------------------------------
// ASN.1/BER tag types (simplified)
// ---------------------------------------------------------------------------

/// Simplified ASN.1/BER tags for SNMP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AsnTag {
    Integer = 0x02,
    OctetString = 0x04,
    Null = 0x05,
    ObjectIdentifier = 0x06,
    Sequence = 0x30,
    /// SNMP-specific types
    IpAddress = 0x40,
    Counter32 = 0x41,
    Gauge32 = 0x42,
    TimeTicks = 0x43,
    Counter64 = 0x46,
    /// PDU types
    GetRequest = 0xA0,
    GetNextRequest = 0xA1,
    GetResponse = 0xA2,
    SetRequest = 0xA3,
    TrapV2 = 0xA7,
}

impl AsnTag {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x02 => Some(AsnTag::Integer),
            0x04 => Some(AsnTag::OctetString),
            0x05 => Some(AsnTag::Null),
            0x06 => Some(AsnTag::ObjectIdentifier),
            0x30 => Some(AsnTag::Sequence),
            0x40 => Some(AsnTag::IpAddress),
            0x41 => Some(AsnTag::Counter32),
            0x42 => Some(AsnTag::Gauge32),
            0x43 => Some(AsnTag::TimeTicks),
            0x46 => Some(AsnTag::Counter64),
            0xA0 => Some(AsnTag::GetRequest),
            0xA1 => Some(AsnTag::GetNextRequest),
            0xA2 => Some(AsnTag::GetResponse),
            0xA3 => Some(AsnTag::SetRequest),
            0xA7 => Some(AsnTag::TrapV2),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// OID representation
// ---------------------------------------------------------------------------

/// An SNMP Object Identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Oid {
    pub components: Vec<u32>,
}

impl Oid {
    /// Parse from dotted notation (e.g. "1.3.6.1.2.1").
    pub fn from_str(s: &str) -> Option<Self> {
        let mut components = Vec::new();
        for part in s.split('.') {
            if part.is_empty() {
                continue;
            }
            match part.parse::<u32>() {
                Ok(n) => components.push(n),
                Err(_) => return None,
            }
        }
        if components.is_empty() {
            return None;
        }
        Some(Self { components })
    }

    /// Convert to dotted notation string.
    pub fn to_string(&self) -> String {
        let mut s = String::new();
        for (i, c) in self.components.iter().enumerate() {
            if i > 0 {
                s.push('.');
            }
            s.push_str(&format!("{}", c));
        }
        s
    }

    /// Check if this OID starts with the given prefix.
    pub fn starts_with(&self, prefix: &Oid) -> bool {
        if self.components.len() < prefix.components.len() {
            return false;
        }
        self.components[..prefix.components.len()] == prefix.components[..]
    }

    /// Compare for ordering (lexicographic on components).
    pub fn cmp_oid(&self, other: &Oid) -> core::cmp::Ordering {
        for (a, b) in self.components.iter().zip(other.components.iter()) {
            match a.cmp(b) {
                core::cmp::Ordering::Equal => continue,
                ord => return ord,
            }
        }
        self.components.len().cmp(&other.components.len())
    }
}

// ---------------------------------------------------------------------------
// MIB value
// ---------------------------------------------------------------------------

/// Value stored in the MIB.
#[derive(Debug, Clone)]
pub enum MibValue {
    Integer(i64),
    OctetString(String),
    OidValue(Oid),
    Counter32(u32),
    Counter64(u64),
    Gauge32(u32),
    TimeTicks(u32),
    IpAddress([u8; 4]),
    Null,
}

impl MibValue {
    pub fn display(&self) -> String {
        match self {
            MibValue::Integer(v) => format!("INTEGER: {}", v),
            MibValue::OctetString(s) => format!("STRING: \"{}\"", s),
            MibValue::OidValue(o) => format!("OID: {}", o.to_string()),
            MibValue::Counter32(v) => format!("Counter32: {}", v),
            MibValue::Counter64(v) => format!("Counter64: {}", v),
            MibValue::Gauge32(v) => format!("Gauge32: {}", v),
            MibValue::TimeTicks(v) => {
                // Convert hundredths of seconds to readable
                let secs = *v / 100;
                let days = secs / 86400;
                let hours = (secs % 86400) / 3600;
                let mins = (secs % 3600) / 60;
                let s = secs % 60;
                format!("Timeticks: ({}) {}d {}:{}:{}", v, days, hours, mins, s)
            }
            MibValue::IpAddress(ip) => format!("IpAddress: {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]),
            MibValue::Null => String::from("NULL"),
        }
    }
}

// ---------------------------------------------------------------------------
// MIB entry
// ---------------------------------------------------------------------------

/// A MIB entry: OID -> name + value.
#[derive(Debug, Clone)]
struct MibEntry {
    oid: Oid,
    name: String,
    value: MibValue,
    writable: bool,
}

// ---------------------------------------------------------------------------
// Community string
// ---------------------------------------------------------------------------

/// SNMP community string with access level.
#[derive(Debug, Clone)]
struct Community {
    name: String,
    read_only: bool,
}

// ---------------------------------------------------------------------------
// Trap target
// ---------------------------------------------------------------------------

/// SNMP trap destination.
#[derive(Debug, Clone)]
struct TrapTarget {
    ip: [u8; 4],
    community: String,
}

// ---------------------------------------------------------------------------
// SNMP agent state
// ---------------------------------------------------------------------------

struct SnmpAgent {
    /// MIB entries sorted by OID.
    mib: Vec<MibEntry>,
    /// Community strings.
    communities: Vec<Community>,
    /// Trap targets.
    trap_targets: Vec<TrapTarget>,
    /// Agent enabled.
    enabled: bool,
}

impl SnmpAgent {
    const fn new() -> Self {
        Self {
            mib: Vec::new(),
            communities: Vec::new(),
            trap_targets: Vec::new(),
            enabled: false,
        }
    }

    /// Insert or update a MIB entry, keeping sorted order.
    fn set_mib(&mut self, oid: Oid, name: &str, value: MibValue, writable: bool) {
        if let Some(entry) = self.mib.iter_mut().find(|e| e.oid == oid) {
            entry.value = value;
            entry.writable = writable;
            return;
        }
        if self.mib.len() >= MAX_MIB_ENTRIES {
            return;
        }
        let pos = self.mib.iter().position(|e| e.oid.cmp_oid(&oid) == core::cmp::Ordering::Greater)
            .unwrap_or(self.mib.len());
        self.mib.insert(pos, MibEntry {
            oid,
            name: String::from(name),
            value,
            writable,
        });
    }

    /// Get a MIB value by OID.
    fn get(&self, oid: &Oid) -> Option<&MibEntry> {
        self.mib.iter().find(|e| e.oid == *oid)
    }

    /// Get the next MIB entry after the given OID.
    fn get_next(&self, oid: &Oid) -> Option<&MibEntry> {
        for entry in &self.mib {
            if entry.oid.cmp_oid(oid) == core::cmp::Ordering::Greater {
                return Some(entry);
            }
        }
        None
    }

    /// Walk all entries under a prefix.
    fn walk(&self, prefix: &Oid) -> Vec<(&Oid, &str, &MibValue)> {
        self.mib.iter()
            .filter(|e| e.oid.starts_with(prefix))
            .map(|e| (&e.oid, e.name.as_str(), &e.value))
            .collect()
    }

    /// Validate community string access.
    fn check_community(&self, name: &str, need_write: bool) -> bool {
        for c in &self.communities {
            if c.name == name {
                if need_write && c.read_only {
                    return false;
                }
                return true;
            }
        }
        false
    }
}

static SNMP_AGENT: Mutex<SnmpAgent> = Mutex::new(SnmpAgent::new());
static GET_REQUESTS: AtomicU64 = AtomicU64::new(0);
static GET_NEXT_REQUESTS: AtomicU64 = AtomicU64::new(0);
static SET_REQUESTS: AtomicU64 = AtomicU64::new(0);
static TRAPS_SENT: AtomicU64 = AtomicU64::new(0);
static AUTH_FAILURES: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Well-known OID prefixes
// ---------------------------------------------------------------------------

const OID_SYSTEM: &str = "1.3.6.1.2.1.1";
const OID_INTERFACES: &str = "1.3.6.1.2.1.2";
const OID_IP: &str = "1.3.6.1.2.1.4";
const OID_TCP: &str = "1.3.6.1.2.1.6";
const OID_UDP: &str = "1.3.6.1.2.1.17";

// ---------------------------------------------------------------------------
// MIB-II population
// ---------------------------------------------------------------------------

fn populate_mib(agent: &mut SnmpAgent) {
    // system group (1.3.6.1.2.1.1)
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.1.1.0").unwrap(),
        "sysDescr", MibValue::OctetString(String::from("MerlionOS kernel")), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.1.2.0").unwrap(),
        "sysObjectID", MibValue::OidValue(Oid::from_str("1.3.6.1.4.1.99999").unwrap()), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.1.3.0").unwrap(),
        "sysUpTime", MibValue::TimeTicks(0), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.1.4.0").unwrap(),
        "sysContact", MibValue::OctetString(String::from("admin@merlionos.local")), true);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.1.5.0").unwrap(),
        "sysName", MibValue::OctetString(String::from("merlion")), true);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.1.6.0").unwrap(),
        "sysLocation", MibValue::OctetString(String::from("Singapore")), true);

    // interfaces group (1.3.6.1.2.1.2)
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.1.0").unwrap(),
        "ifNumber", MibValue::Integer(2), false);

    // ifTable entry for interface 1 (eth0)
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.2.1.1.1").unwrap(),
        "ifIndex.1", MibValue::Integer(1), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.2.1.2.1").unwrap(),
        "ifDescr.1", MibValue::OctetString(String::from("eth0")), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.2.1.3.1").unwrap(),
        "ifType.1", MibValue::Integer(6), false); // ethernetCsmacd
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.2.1.4.1").unwrap(),
        "ifMtu.1", MibValue::Integer(1500), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.2.1.5.1").unwrap(),
        "ifSpeed.1", MibValue::Gauge32(1000000000), false); // 1 Gbps
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.2.1.8.1").unwrap(),
        "ifOperStatus.1", MibValue::Integer(1), false); // up
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.2.1.10.1").unwrap(),
        "ifInOctets.1", MibValue::Counter32(0), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.2.1.16.1").unwrap(),
        "ifOutOctets.1", MibValue::Counter32(0), false);

    // ifTable entry for interface 2 (lo)
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.2.1.1.2").unwrap(),
        "ifIndex.2", MibValue::Integer(2), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.2.1.2.2").unwrap(),
        "ifDescr.2", MibValue::OctetString(String::from("lo")), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.2.1.3.2").unwrap(),
        "ifType.2", MibValue::Integer(24), false); // softwareLoopback
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.2.1.4.2").unwrap(),
        "ifMtu.2", MibValue::Integer(65536), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.2.1.5.2").unwrap(),
        "ifSpeed.2", MibValue::Gauge32(0), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.2.1.8.2").unwrap(),
        "ifOperStatus.2", MibValue::Integer(1), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.2.1.10.2").unwrap(),
        "ifInOctets.2", MibValue::Counter32(0), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.2.2.1.16.2").unwrap(),
        "ifOutOctets.2", MibValue::Counter32(0), false);

    // ip group (1.3.6.1.2.1.4)
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.4.1.0").unwrap(),
        "ipForwarding", MibValue::Integer(1), true);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.4.3.0").unwrap(),
        "ipInReceives", MibValue::Counter32(0), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.4.10.0").unwrap(),
        "ipOutRequests", MibValue::Counter32(0), false);

    // tcp group (1.3.6.1.2.1.6)
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.6.5.0").unwrap(),
        "tcpActiveOpens", MibValue::Counter32(0), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.6.6.0").unwrap(),
        "tcpPassiveOpens", MibValue::Counter32(0), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.6.9.0").unwrap(),
        "tcpCurrEstab", MibValue::Gauge32(0), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.6.10.0").unwrap(),
        "tcpInSegs", MibValue::Counter32(0), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.6.11.0").unwrap(),
        "tcpOutSegs", MibValue::Counter32(0), false);

    // udp group (1.3.6.1.2.1.17)
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.17.1.0").unwrap(),
        "udpInDatagrams", MibValue::Counter32(0), false);
    agent.set_mib(Oid::from_str("1.3.6.1.2.1.17.4.0").unwrap(),
        "udpOutDatagrams", MibValue::Counter32(0), false);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Handle an SNMP GET request.
pub fn handle_get(community: &str, oid_str: &str) -> Result<String, &'static str> {
    let agent = SNMP_AGENT.lock();
    if !agent.check_community(community, false) {
        AUTH_FAILURES.fetch_add(1, Ordering::Relaxed);
        return Err("authentication failure");
    }
    GET_REQUESTS.fetch_add(1, Ordering::Relaxed);
    let oid = Oid::from_str(oid_str).ok_or("invalid OID")?;
    match agent.get(&oid) {
        Some(entry) => Ok(format!("{} = {}", entry.name, entry.value.display())),
        None => Err("no such object"),
    }
}

/// Handle an SNMP GET-NEXT request.
pub fn handle_get_next(community: &str, oid_str: &str) -> Result<String, &'static str> {
    let agent = SNMP_AGENT.lock();
    if !agent.check_community(community, false) {
        AUTH_FAILURES.fetch_add(1, Ordering::Relaxed);
        return Err("authentication failure");
    }
    GET_NEXT_REQUESTS.fetch_add(1, Ordering::Relaxed);
    let oid = Oid::from_str(oid_str).ok_or("invalid OID")?;
    match agent.get_next(&oid) {
        Some(entry) => Ok(format!("{} = {} = {}", entry.oid.to_string(), entry.name, entry.value.display())),
        None => Err("end of MIB"),
    }
}

/// Handle an SNMP SET request.
pub fn handle_set(community: &str, oid_str: &str, value: MibValue) -> Result<(), &'static str> {
    let mut agent = SNMP_AGENT.lock();
    if !agent.check_community(community, true) {
        AUTH_FAILURES.fetch_add(1, Ordering::Relaxed);
        return Err("authentication failure (read-only)");
    }
    SET_REQUESTS.fetch_add(1, Ordering::Relaxed);
    let oid = Oid::from_str(oid_str).ok_or("invalid OID")?;
    let entry = agent.mib.iter_mut().find(|e| e.oid == oid)
        .ok_or("no such object")?;
    if !entry.writable {
        return Err("object is read-only");
    }
    entry.value = value;
    Ok(())
}

/// Walk the MIB subtree under a given OID prefix.
pub fn snmp_walk(oid_prefix: &str) -> Vec<(String, String)> {
    let agent = SNMP_AGENT.lock();
    let prefix = match Oid::from_str(oid_prefix) {
        Some(o) => o,
        None => return Vec::new(),
    };
    agent.walk(&prefix).iter().map(|(oid, name, value)| {
        (format!("{} ({})", oid.to_string(), name), value.display())
    }).collect()
}

/// Walk command for shell: returns formatted string.
pub fn snmp_walk_cmd(oid_prefix: &str) -> String {
    let results = snmp_walk(oid_prefix);
    if results.is_empty() {
        return format!("No entries under {}\n", oid_prefix);
    }
    let mut out = String::new();
    for (oid, value) in &results {
        out.push_str(&format!("{} = {}\n", oid, value));
    }
    out
}

/// Send a trap (simplified: just log + increment counter).
pub fn send_trap(trap_oid: &str, message: &str) {
    TRAPS_SENT.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[snmp] TRAP {}: {}", trap_oid, message);
}

/// Send link-up trap.
pub fn trap_link_up(iface: &str) {
    send_trap("1.3.6.1.6.3.1.1.5.4", &format!("linkUp: {}", iface));
}

/// Send link-down trap.
pub fn trap_link_down(iface: &str) {
    send_trap("1.3.6.1.6.3.1.1.5.3", &format!("linkDown: {}", iface));
}

/// Send authentication failure trap.
pub fn trap_auth_failure(source: &str) {
    send_trap("1.3.6.1.6.3.1.1.5.5", &format!("authenticationFailure from {}", source));
}

/// Add a community string.
pub fn add_community(name: &str, read_only: bool) {
    let mut agent = SNMP_AGENT.lock();
    if agent.communities.len() >= MAX_COMMUNITIES {
        return;
    }
    if agent.communities.iter().any(|c| c.name == name) {
        return;
    }
    agent.communities.push(Community {
        name: String::from(name),
        read_only,
    });
}

/// Add a trap target.
pub fn add_trap_target(ip: [u8; 4], community: &str) {
    let mut agent = SNMP_AGENT.lock();
    agent.trap_targets.push(TrapTarget {
        ip,
        community: String::from(community),
    });
}

/// Update sysUpTime from timer ticks.
pub fn update_uptime() {
    let mut agent = SNMP_AGENT.lock();
    let ticks = crate::timer::ticks() as u32;
    // Convert 100 Hz ticks to centiseconds (1:1 since PIT is 100 Hz)
    if let Some(entry) = agent.mib.iter_mut().find(|e| e.name == "sysUpTime") {
        entry.value = MibValue::TimeTicks(ticks);
    }
}

/// SNMP agent info.
pub fn snmp_info() -> String {
    let agent = SNMP_AGENT.lock();
    let mut out = String::from("SNMP Agent Information\n");
    out.push_str(&format!("  Enabled: {}\n", agent.enabled));
    out.push_str(&format!("  Port: {}\n", SNMP_PORT));
    out.push_str(&format!("  Trap port: {}\n", SNMP_TRAP_PORT));
    out.push_str(&format!("  MIB entries: {}\n", agent.mib.len()));
    out.push_str(&format!("  Communities: {}\n", agent.communities.len()));
    for c in &agent.communities {
        out.push_str(&format!("    {} ({})\n", c.name, if c.read_only { "RO" } else { "RW" }));
    }
    out.push_str(&format!("  Trap targets: {}\n", agent.trap_targets.len()));
    for t in &agent.trap_targets {
        out.push_str(&format!("    {}.{}.{}.{} ({})\n",
            t.ip[0], t.ip[1], t.ip[2], t.ip[3], t.community));
    }
    out
}

/// SNMP statistics.
pub fn snmp_stats() -> String {
    let mut out = String::from("SNMP Statistics\n");
    out.push_str(&format!("  GET requests:      {}\n", GET_REQUESTS.load(Ordering::Relaxed)));
    out.push_str(&format!("  GET-NEXT requests: {}\n", GET_NEXT_REQUESTS.load(Ordering::Relaxed)));
    out.push_str(&format!("  SET requests:      {}\n", SET_REQUESTS.load(Ordering::Relaxed)));
    out.push_str(&format!("  Traps sent:        {}\n", TRAPS_SENT.load(Ordering::Relaxed)));
    out.push_str(&format!("  Auth failures:     {}\n", AUTH_FAILURES.load(Ordering::Relaxed)));
    out
}

/// Initialise the SNMP agent.
pub fn init() {
    let mut agent = SNMP_AGENT.lock();
    populate_mib(&mut agent);
    // Default community strings
    agent.communities.push(Community {
        name: String::from("public"),
        read_only: true,
    });
    agent.communities.push(Community {
        name: String::from("private"),
        read_only: false,
    });
    agent.enabled = true;
    drop(agent);
    crate::serial_println!("[snmp] SNMP agent initialised ({} MIB entries)", {
        SNMP_AGENT.lock().mib.len()
    });
}
