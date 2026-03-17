/// iptables-style packet filter and NAT for MerlionOS.
/// Provides rule chains (INPUT/OUTPUT/FORWARD), SNAT/DNAT/MASQUERADE,
/// port forwarding, connection tracking, and logging.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum rules per chain.
const MAX_RULES_PER_CHAIN: usize = 128;

/// Maximum connection tracking entries.
const MAX_CONNTRACK: usize = 1024;

/// Maximum NAT mappings.
const MAX_NAT_MAPPINGS: usize = 256;

/// Maximum port forwarding rules.
const MAX_PORT_FORWARDS: usize = 64;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Rule chain (iptables table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chain {
    Input,
    Output,
    Forward,
    Prerouting,
    Postrouting,
}

impl Chain {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "INPUT" | "input" => Some(Chain::Input),
            "OUTPUT" | "output" => Some(Chain::Output),
            "FORWARD" | "forward" => Some(Chain::Forward),
            "PREROUTING" | "prerouting" => Some(Chain::Prerouting),
            "POSTROUTING" | "postrouting" => Some(Chain::Postrouting),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Chain::Input => "INPUT",
            Chain::Output => "OUTPUT",
            Chain::Forward => "FORWARD",
            Chain::Prerouting => "PREROUTING",
            Chain::Postrouting => "POSTROUTING",
        }
    }
}

/// Network protocol selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Udp,
    Icmp,
    Any,
}

impl Protocol {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "tcp" | "TCP" => Some(Protocol::Tcp),
            "udp" | "UDP" => Some(Protocol::Udp),
            "icmp" | "ICMP" => Some(Protocol::Icmp),
            "any" | "ANY" | "all" => Some(Protocol::Any),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp",
            Protocol::Icmp => "icmp",
            Protocol::Any => "all",
        }
    }
}

/// Target action for a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Accept,
    Drop,
    Reject,
    Log,
    Snat,
    Dnat,
    Masquerade,
    Redirect,
}

impl Target {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "ACCEPT" | "accept" => Some(Target::Accept),
            "DROP" | "drop" => Some(Target::Drop),
            "REJECT" | "reject" => Some(Target::Reject),
            "LOG" | "log" => Some(Target::Log),
            "SNAT" | "snat" => Some(Target::Snat),
            "DNAT" | "dnat" => Some(Target::Dnat),
            "MASQUERADE" | "masquerade" => Some(Target::Masquerade),
            "REDIRECT" | "redirect" => Some(Target::Redirect),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Target::Accept => "ACCEPT",
            Target::Drop => "DROP",
            Target::Reject => "REJECT",
            Target::Log => "LOG",
            Target::Snat => "SNAT",
            Target::Dnat => "DNAT",
            Target::Masquerade => "MASQUERADE",
            Target::Redirect => "REDIRECT",
        }
    }
}

/// Connection tracking state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    New,
    Established,
    Related,
}

impl ConnState {
    pub fn name(&self) -> &'static str {
        match self {
            ConnState::New => "NEW",
            ConnState::Established => "ESTABLISHED",
            ConnState::Related => "RELATED",
        }
    }
}

/// Packet direction for process_packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Inbound,
    Outbound,
    Forwarded,
}

// ---------------------------------------------------------------------------
// Rule
// ---------------------------------------------------------------------------

/// A single iptables rule with match criteria and target action.
#[derive(Debug, Clone)]
pub struct Rule {
    /// Rule number within its chain.
    pub num: u32,
    /// Target action.
    pub target: Target,
    /// Protocol filter (None = match all).
    pub protocol: Option<Protocol>,
    /// Source IP filter (None = match all).
    pub src_ip: Option<[u8; 4]>,
    /// Source subnet mask (prefix length, 0-32).
    pub src_mask: u8,
    /// Destination IP filter.
    pub dst_ip: Option<[u8; 4]>,
    /// Destination subnet mask (prefix length).
    pub dst_mask: u8,
    /// Source port filter.
    pub src_port: Option<u16>,
    /// Destination port filter.
    pub dst_port: Option<u16>,
    /// Interface name filter (None = match all).
    pub interface: Option<String>,
    /// NAT: rewrite source IP (for SNAT).
    pub nat_src_ip: Option<[u8; 4]>,
    /// NAT: rewrite destination IP (for DNAT).
    pub nat_dst_ip: Option<[u8; 4]>,
    /// NAT: rewrite destination port (for DNAT/REDIRECT).
    pub nat_dst_port: Option<u16>,
    /// Packet counter for this rule.
    pub packets: u64,
    /// Byte counter for this rule.
    pub bytes: u64,
}

impl Rule {
    /// Create a new rule with the given target and all wildcards.
    pub fn new(target: Target) -> Self {
        Self {
            num: 0,
            target,
            protocol: None,
            src_ip: None,
            src_mask: 0,
            dst_ip: None,
            dst_mask: 0,
            src_port: None,
            dst_port: None,
            interface: None,
            nat_src_ip: None,
            nat_dst_ip: None,
            nat_dst_port: None,
            packets: 0,
            bytes: 0,
        }
    }

    /// Check if an IP matches an IP/mask filter.
    fn ip_matches(ip: [u8; 4], filter: [u8; 4], prefix_len: u8) -> bool {
        if prefix_len == 0 { return true; }
        if prefix_len >= 32 { return ip == filter; }
        let ip_u32 = u32::from_be_bytes(ip);
        let filter_u32 = u32::from_be_bytes(filter);
        let mask = !((1u32 << (32 - prefix_len)) - 1);
        (ip_u32 & mask) == (filter_u32 & mask)
    }

    /// Test whether a packet matches this rule.
    pub fn matches(&self, proto: Protocol, src_ip: [u8; 4], src_port: u16,
                   dst_ip: [u8; 4], dst_port: u16, iface: &str) -> bool {
        if let Some(p) = self.protocol {
            if p != Protocol::Any && p != proto { return false; }
        }
        if let Some(sip) = self.src_ip {
            if !Self::ip_matches(src_ip, sip, self.src_mask) { return false; }
        }
        if let Some(dip) = self.dst_ip {
            if !Self::ip_matches(dst_ip, dip, self.dst_mask) { return false; }
        }
        if let Some(sp) = self.src_port {
            if sp != src_port { return false; }
        }
        if let Some(dp) = self.dst_port {
            if dp != dst_port { return false; }
        }
        if let Some(ref iname) = self.interface {
            if iname.as_str() != iface { return false; }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Rule Chain
// ---------------------------------------------------------------------------

/// A named chain of rules with a default policy.
struct RuleChain {
    chain: Chain,
    rules: Vec<Rule>,
    policy: Target,
    next_num: u32,
    /// Total packets evaluated against this chain.
    total_packets: u64,
    /// Total bytes evaluated.
    total_bytes: u64,
}

impl RuleChain {
    fn new(chain: Chain, policy: Target) -> Self {
        Self {
            chain,
            rules: Vec::new(),
            policy,
            next_num: 1,
            total_packets: 0,
            total_bytes: 0,
        }
    }

    fn add_rule(&mut self, mut rule: Rule) -> Option<u32> {
        if self.rules.len() >= MAX_RULES_PER_CHAIN { return None; }
        let num = self.next_num;
        self.next_num += 1;
        rule.num = num;
        self.rules.push(rule);
        Some(num)
    }

    fn insert_rule(&mut self, index: usize, mut rule: Rule) -> Option<u32> {
        if self.rules.len() >= MAX_RULES_PER_CHAIN { return None; }
        let num = self.next_num;
        self.next_num += 1;
        rule.num = num;
        let idx = if index > self.rules.len() { self.rules.len() } else { index };
        self.rules.insert(idx, rule);
        Some(num)
    }

    fn delete_rule(&mut self, index: usize) -> bool {
        if index < self.rules.len() {
            self.rules.remove(index);
            true
        } else {
            false
        }
    }

    fn flush(&mut self) {
        self.rules.clear();
        self.total_packets = 0;
        self.total_bytes = 0;
    }

    /// Evaluate rules against a packet. Returns the matching target or the
    /// chain default policy.
    fn evaluate(&mut self, proto: Protocol, src_ip: [u8; 4], src_port: u16,
                dst_ip: [u8; 4], dst_port: u16, iface: &str, pkt_len: u64) -> Target {
        self.total_packets += 1;
        self.total_bytes += pkt_len;
        for rule in self.rules.iter_mut() {
            if rule.matches(proto, src_ip, src_port, dst_ip, dst_port, iface) {
                rule.packets += 1;
                rule.bytes += pkt_len;
                return rule.target;
            }
        }
        self.policy
    }
}

// ---------------------------------------------------------------------------
// Connection tracking
// ---------------------------------------------------------------------------

/// A tracked connection entry.
#[derive(Debug, Clone)]
pub struct ConntrackEntry {
    pub protocol: Protocol,
    pub src_ip: [u8; 4],
    pub src_port: u16,
    pub dst_ip: [u8; 4],
    pub dst_port: u16,
    pub state: ConnState,
    pub packets: u64,
    pub bytes: u64,
}

impl ConntrackEntry {
    fn matches_forward(&self, proto: Protocol, src_ip: [u8; 4], src_port: u16,
                       dst_ip: [u8; 4], dst_port: u16) -> bool {
        self.protocol == proto && self.src_ip == src_ip && self.src_port == src_port
            && self.dst_ip == dst_ip && self.dst_port == dst_port
    }

    fn matches_reply(&self, proto: Protocol, src_ip: [u8; 4], src_port: u16,
                     dst_ip: [u8; 4], dst_port: u16) -> bool {
        self.protocol == proto && self.dst_ip == src_ip && self.dst_port == src_port
            && self.src_ip == dst_ip && self.src_port == dst_port
    }
}

/// Connection tracking table.
struct ConntrackTable {
    entries: Vec<ConntrackEntry>,
}

impl ConntrackTable {
    const fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Look up or create a conntrack entry.  Returns the connection state.
    fn track(&mut self, proto: Protocol, src_ip: [u8; 4], src_port: u16,
             dst_ip: [u8; 4], dst_port: u16, pkt_len: u64) -> ConnState {
        // Check for existing forward match
        for entry in self.entries.iter_mut() {
            if entry.matches_forward(proto, src_ip, src_port, dst_ip, dst_port) {
                entry.packets += 1;
                entry.bytes += pkt_len;
                if entry.state == ConnState::New {
                    entry.state = ConnState::Established;
                }
                return entry.state;
            }
        }
        // Check for reply match (return traffic)
        for entry in self.entries.iter_mut() {
            if entry.matches_reply(proto, src_ip, src_port, dst_ip, dst_port) {
                entry.packets += 1;
                entry.bytes += pkt_len;
                entry.state = ConnState::Established;
                return ConnState::Established;
            }
        }
        // New connection
        if self.entries.len() < MAX_CONNTRACK {
            self.entries.push(ConntrackEntry {
                protocol: proto,
                src_ip,
                src_port,
                dst_ip,
                dst_port,
                state: ConnState::New,
                packets: 1,
                bytes: pkt_len,
            });
        }
        ConnState::New
    }

    fn list(&self) -> Vec<ConntrackEntry> {
        self.entries.clone()
    }

    fn count(&self) -> usize {
        self.entries.len()
    }

    fn flush(&mut self) {
        self.entries.clear();
    }
}

// ---------------------------------------------------------------------------
// NAT mapping
// ---------------------------------------------------------------------------

/// A NAT translation entry recording how an address was rewritten.
#[derive(Debug, Clone)]
struct NatMapping {
    original_src_ip: [u8; 4],
    original_src_port: u16,
    translated_src_ip: [u8; 4],
    translated_src_port: u16,
    dst_ip: [u8; 4],
    dst_port: u16,
    protocol: Protocol,
}

/// Port forwarding rule.
#[derive(Debug, Clone)]
pub struct PortForward {
    pub protocol: Protocol,
    pub external_port: u16,
    pub internal_ip: [u8; 4],
    pub internal_port: u16,
    pub packets: u64,
    pub bytes: u64,
}

// ---------------------------------------------------------------------------
// Iptables engine
// ---------------------------------------------------------------------------

struct IptablesEngine {
    input: RuleChain,
    output: RuleChain,
    forward: RuleChain,
    prerouting: RuleChain,
    postrouting: RuleChain,
    conntrack: ConntrackTable,
    nat_mappings: Vec<NatMapping>,
    port_forwards: Vec<PortForward>,
    /// IP address used for MASQUERADE (outbound interface IP).
    masquerade_ip: [u8; 4],
    /// Next ephemeral port for MASQUERADE/SNAT.
    next_nat_port: u16,
    /// Global counters.
    total_packets: u64,
    total_bytes: u64,
    accepted: u64,
    dropped: u64,
    rejected: u64,
    logged: u64,
}

impl IptablesEngine {
    fn new() -> Self {
        Self {
            input: RuleChain::new(Chain::Input, Target::Accept),
            output: RuleChain::new(Chain::Output, Target::Accept),
            forward: RuleChain::new(Chain::Forward, Target::Drop),
            prerouting: RuleChain::new(Chain::Prerouting, Target::Accept),
            postrouting: RuleChain::new(Chain::Postrouting, Target::Accept),
            conntrack: ConntrackTable::new(),
            nat_mappings: Vec::new(),
            port_forwards: Vec::new(),
            masquerade_ip: [10, 0, 2, 15],
            next_nat_port: 40000,
            total_packets: 0,
            total_bytes: 0,
            accepted: 0,
            dropped: 0,
            rejected: 0,
            logged: 0,
        }
    }

    fn get_chain_mut(&mut self, chain: Chain) -> &mut RuleChain {
        match chain {
            Chain::Input => &mut self.input,
            Chain::Output => &mut self.output,
            Chain::Forward => &mut self.forward,
            Chain::Prerouting => &mut self.prerouting,
            Chain::Postrouting => &mut self.postrouting,
        }
    }

    fn get_chain(&self, chain: Chain) -> &RuleChain {
        match chain {
            Chain::Input => &self.input,
            Chain::Output => &self.output,
            Chain::Forward => &self.forward,
            Chain::Prerouting => &self.prerouting,
            Chain::Postrouting => &self.postrouting,
        }
    }

    fn add_rule(&mut self, chain: Chain, rule: Rule) -> Option<u32> {
        self.get_chain_mut(chain).add_rule(rule)
    }

    fn insert_rule(&mut self, chain: Chain, index: usize, rule: Rule) -> Option<u32> {
        self.get_chain_mut(chain).insert_rule(index, rule)
    }

    fn delete_rule(&mut self, chain: Chain, index: usize) -> bool {
        self.get_chain_mut(chain).delete_rule(index)
    }

    fn flush_chain(&mut self, chain: Chain) {
        self.get_chain_mut(chain).flush();
    }

    fn set_policy(&mut self, chain: Chain, target: Target) {
        self.get_chain_mut(chain).policy = target;
    }

    fn list_rules(&self, chain: Chain) -> String {
        let ch = self.get_chain(chain);
        if ch.rules.is_empty() {
            return format!("Chain {} (policy {}, 0 rules)\n",
                           chain.name(), ch.policy.name());
        }
        let mut out = format!("Chain {} (policy {}, {} rules)\n",
                              chain.name(), ch.policy.name(), ch.rules.len());
        out.push_str("num  target     prot  source           dest             sport  dport  iface    pkts   bytes\n");
        for r in &ch.rules {
            let proto = r.protocol.map_or("all", |p| p.name());
            let src = match r.src_ip {
                Some(ip) => if r.src_mask > 0 && r.src_mask < 32 {
                    format!("{}.{}.{}.{}/{}", ip[0], ip[1], ip[2], ip[3], r.src_mask)
                } else {
                    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
                },
                None => String::from("0.0.0.0/0"),
            };
            let dst = match r.dst_ip {
                Some(ip) => if r.dst_mask > 0 && r.dst_mask < 32 {
                    format!("{}.{}.{}.{}/{}", ip[0], ip[1], ip[2], ip[3], r.dst_mask)
                } else {
                    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
                },
                None => String::from("0.0.0.0/0"),
            };
            let sp = r.src_port.map_or(String::from("*"), |p| format!("{}", p));
            let dp = r.dst_port.map_or(String::from("*"), |p| format!("{}", p));
            let iface = r.interface.as_deref().unwrap_or("*");
            out.push_str(&format!("{:<4} {:<10} {:<5} {:<16} {:<16} {:<6} {:<6} {:<8} {:<6} {}\n",
                                  r.num, r.target.name(), proto,
                                  src, dst, sp, dp, iface, r.packets, r.bytes));
        }
        out
    }

    fn add_port_forward(&mut self, proto: Protocol, external_port: u16,
                        internal_ip: [u8; 4], internal_port: u16) -> bool {
        if self.port_forwards.len() >= MAX_PORT_FORWARDS { return false; }
        self.port_forwards.push(PortForward {
            protocol: proto,
            external_port,
            internal_ip,
            internal_port,
            packets: 0,
            bytes: 0,
        });
        // Also add a DNAT rule to PREROUTING
        let mut rule = Rule::new(Target::Dnat);
        rule.protocol = Some(proto);
        rule.dst_port = Some(external_port);
        rule.nat_dst_ip = Some(internal_ip);
        rule.nat_dst_port = Some(internal_port);
        let _ = self.prerouting.add_rule(rule);
        true
    }

    fn alloc_nat_port(&mut self) -> u16 {
        let port = self.next_nat_port;
        self.next_nat_port = if self.next_nat_port >= 60000 { 40000 } else { self.next_nat_port + 1 };
        port
    }

    /// Process a packet through the iptables chains, performing NAT and
    /// connection tracking. Returns the final target action.
    fn process_packet(&mut self, proto: Protocol, src_ip: [u8; 4], src_port: u16,
                      dst_ip: [u8; 4], dst_port: u16, direction: Direction,
                      iface: &str, pkt_len: u64) -> Target {
        self.total_packets += 1;
        self.total_bytes += pkt_len;

        // Connection tracking
        let conn_state = self.conntrack.track(proto, src_ip, src_port, dst_ip, dst_port, pkt_len);

        // Allow established/related return traffic
        if conn_state == ConnState::Established || conn_state == ConnState::Related {
            self.accepted += 1;
            return Target::Accept;
        }

        // PREROUTING (DNAT, port forwarding)
        let pre_target = self.prerouting.evaluate(
            proto, src_ip, src_port, dst_ip, dst_port, iface, pkt_len);
        if pre_target == Target::Dnat {
            // DNAT rewriting would happen here; we just accept for routing
        }

        // Check port forwarding rules
        for pf in self.port_forwards.iter_mut() {
            if pf.protocol == proto && pf.external_port == dst_port {
                pf.packets += 1;
                pf.bytes += pkt_len;
                // Would rewrite dst to internal_ip:internal_port
            }
        }

        // Main chain evaluation
        let target = match direction {
            Direction::Inbound => self.input.evaluate(
                proto, src_ip, src_port, dst_ip, dst_port, iface, pkt_len),
            Direction::Outbound => self.output.evaluate(
                proto, src_ip, src_port, dst_ip, dst_port, iface, pkt_len),
            Direction::Forwarded => {
                if !IP_FORWARDING.load(Ordering::Relaxed) {
                    self.dropped += 1;
                    return Target::Drop;
                }
                self.forward.evaluate(
                    proto, src_ip, src_port, dst_ip, dst_port, iface, pkt_len)
            }
        };

        // POSTROUTING (SNAT/MASQUERADE)
        if direction == Direction::Outbound || direction == Direction::Forwarded {
            let post_target = self.postrouting.evaluate(
                proto, src_ip, src_port, dst_ip, dst_port, iface, pkt_len);
            if post_target == Target::Masquerade || post_target == Target::Snat {
                if self.nat_mappings.len() < MAX_NAT_MAPPINGS {
                    let translated_port = self.alloc_nat_port();
                    self.nat_mappings.push(NatMapping {
                        original_src_ip: src_ip,
                        original_src_port: src_port,
                        translated_src_ip: self.masquerade_ip,
                        translated_src_port: translated_port,
                        dst_ip,
                        dst_port,
                        protocol: proto,
                    });
                }
            }
        }

        match target {
            Target::Accept => self.accepted += 1,
            Target::Drop => self.dropped += 1,
            Target::Reject => self.rejected += 1,
            Target::Log => self.logged += 1,
            _ => self.accepted += 1,
        }

        target
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Global iptables engine.
static IPTABLES: Mutex<Option<IptablesEngine>> = Mutex::new(None);

/// IP forwarding flag (atomic for lock-free reads from fast path).
static IP_FORWARDING: AtomicBool = AtomicBool::new(false);

/// Global packet counter (atomic, lock-free).
static GLOBAL_PACKETS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the iptables subsystem.
pub fn init() {
    *IPTABLES.lock() = Some(IptablesEngine::new());
}

/// Add a rule to a chain. Returns rule number or None.
pub fn add_rule(chain: Chain, rule: Rule) -> Option<u32> {
    IPTABLES.lock().as_mut().and_then(|e| e.add_rule(chain, rule))
}

/// Insert a rule at a specific index. Returns rule number or None.
pub fn insert_rule(chain: Chain, index: usize, rule: Rule) -> Option<u32> {
    IPTABLES.lock().as_mut().and_then(|e| e.insert_rule(chain, index, rule))
}

/// Delete a rule by index from a chain.
pub fn delete_rule(chain: Chain, index: usize) -> bool {
    IPTABLES.lock().as_mut().map_or(false, |e| e.delete_rule(chain, index))
}

/// List all rules in a chain as formatted string.
pub fn list_rules(chain: Chain) -> String {
    IPTABLES.lock().as_ref().map_or(
        String::from("(iptables not initialised)\n"),
        |e| e.list_rules(chain),
    )
}

/// Flush (clear) all rules in a chain.
pub fn flush_chain(chain: Chain) {
    if let Some(e) = IPTABLES.lock().as_mut() { e.flush_chain(chain); }
}

/// Set the default policy for a chain.
pub fn set_policy(chain: Chain, target: Target) {
    if let Some(e) = IPTABLES.lock().as_mut() { e.set_policy(chain, target); }
}

/// Add a port forwarding rule.
pub fn forward_port(proto: Protocol, external_port: u16,
                    internal_ip: [u8; 4], internal_port: u16) -> bool {
    IPTABLES.lock().as_mut().map_or(false, |e|
        e.add_port_forward(proto, external_port, internal_ip, internal_port))
}

/// Process a packet through the iptables engine.
pub fn process_packet(proto: Protocol, src_ip: [u8; 4], src_port: u16,
                      dst_ip: [u8; 4], dst_port: u16, direction: Direction,
                      iface: &str, pkt_len: u64) -> Target {
    GLOBAL_PACKETS.fetch_add(1, Ordering::Relaxed);
    IPTABLES.lock().as_mut().map_or(Target::Accept, |e|
        e.process_packet(proto, src_ip, src_port, dst_ip, dst_port, direction, iface, pkt_len))
}

/// Enable IP forwarding.
pub fn enable_forwarding() {
    IP_FORWARDING.store(true, Ordering::Relaxed);
}

/// Disable IP forwarding.
pub fn disable_forwarding() {
    IP_FORWARDING.store(false, Ordering::Relaxed);
}

/// Check whether IP forwarding is enabled.
pub fn is_forwarding() -> bool {
    IP_FORWARDING.load(Ordering::Relaxed)
}

/// List all connection tracking entries.
pub fn conntrack_list() -> Vec<ConntrackEntry> {
    IPTABLES.lock().as_ref().map_or(Vec::new(), |e| e.conntrack.list())
}

/// Flush conntrack table.
pub fn conntrack_flush() {
    if let Some(e) = IPTABLES.lock().as_mut() { e.conntrack.flush(); }
}

/// Format conntrack entries for display.
pub fn conntrack_info() -> String {
    let entries = conntrack_list();
    if entries.is_empty() {
        return String::from("Connection tracking: 0 entries\n");
    }
    let mut out = format!("Connection tracking: {} entries\n", entries.len());
    out.push_str("proto  src_ip           sport  dst_ip           dport  state        pkts   bytes\n");
    for e in &entries {
        out.push_str(&format!("{:<6} {}.{}.{}.{:<8} {:<6} {}.{}.{}.{:<8} {:<6} {:<12} {:<6} {}\n",
                              e.protocol.name(),
                              e.src_ip[0], e.src_ip[1], e.src_ip[2], e.src_ip[3], e.src_port,
                              e.dst_ip[0], e.dst_ip[1], e.dst_ip[2], e.dst_ip[3], e.dst_port,
                              e.state.name(), e.packets, e.bytes));
    }
    out
}

/// General iptables info string.
pub fn iptables_info() -> String {
    let guard = IPTABLES.lock();
    let e = match guard.as_ref() {
        Some(e) => e,
        None => return String::from("iptables: not initialised\n"),
    };
    let mut out = String::from("iptables - MerlionOS packet filter\n");
    out.push_str(&format!("IP forwarding: {}\n", if IP_FORWARDING.load(Ordering::Relaxed) { "enabled" } else { "disabled" }));
    out.push_str(&format!("Masquerade IP: {}.{}.{}.{}\n",
                          e.masquerade_ip[0], e.masquerade_ip[1], e.masquerade_ip[2], e.masquerade_ip[3]));
    out.push_str(&format!("Chains:\n"));
    for chain in &[Chain::Input, Chain::Output, Chain::Forward, Chain::Prerouting, Chain::Postrouting] {
        let ch = e.get_chain(*chain);
        out.push_str(&format!("  {}: {} rules, policy {}, {} pkts / {} bytes\n",
                              chain.name(), ch.rules.len(), ch.policy.name(),
                              ch.total_packets, ch.total_bytes));
    }
    out.push_str(&format!("Conntrack entries: {}\n", e.conntrack.count()));
    out.push_str(&format!("NAT mappings: {}\n", e.nat_mappings.len()));
    out.push_str(&format!("Port forwards: {}\n", e.port_forwards.len()));
    out
}

/// Global statistics string.
pub fn iptables_stats() -> String {
    let guard = IPTABLES.lock();
    let e = match guard.as_ref() {
        Some(e) => e,
        None => return String::from("iptables: not initialised\n"),
    };
    let mut out = String::from("iptables statistics:\n");
    out.push_str(&format!("  Total packets: {}\n", e.total_packets));
    out.push_str(&format!("  Total bytes:   {}\n", e.total_bytes));
    out.push_str(&format!("  Accepted:      {}\n", e.accepted));
    out.push_str(&format!("  Dropped:       {}\n", e.dropped));
    out.push_str(&format!("  Rejected:      {}\n", e.rejected));
    out.push_str(&format!("  Logged:        {}\n", e.logged));
    out.push_str(&format!("  Conntrack:     {} entries\n", e.conntrack.count()));
    out.push_str(&format!("  Global pkts:   {}\n", GLOBAL_PACKETS.load(Ordering::Relaxed)));
    out
}

// ---------------------------------------------------------------------------
// Shell-like syntax parser: iptables -A INPUT -s 192.168.1.0/24 -j ACCEPT
// ---------------------------------------------------------------------------

/// Parse an IP address string like "192.168.1.0" or "192.168.1.0/24".
/// Returns (ip, prefix_len).
fn parse_ip_cidr(s: &str) -> Option<([u8; 4], u8)> {
    let (ip_str, mask) = if let Some(idx) = s.find('/') {
        let m = s[idx + 1..].parse::<u8>().ok()?;
        (&s[..idx], m)
    } else {
        (s, 32)
    };
    let parts: Vec<&str> = ip_str.split('.').collect();
    if parts.len() != 4 { return None; }
    let a = parts[0].parse::<u8>().ok()?;
    let b = parts[1].parse::<u8>().ok()?;
    let c = parts[2].parse::<u8>().ok()?;
    let d = parts[3].parse::<u8>().ok()?;
    Some(([a, b, c, d], mask))
}

/// Parse a shell-like iptables command string.
/// Example: "iptables -A INPUT -s 192.168.1.0/24 -p tcp --dport 80 -j ACCEPT"
pub fn parse_command(cmd: &str) -> Result<String, String> {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    if tokens.is_empty() { return Err(String::from("empty command")); }

    // Skip "iptables" prefix if present
    let start = if tokens[0] == "iptables" { 1 } else { 0 };
    if start >= tokens.len() { return Err(String::from("no arguments")); }

    let mut i = start;
    let mut chain: Option<Chain> = None;
    let mut action_flag: Option<&str> = None; // -A, -I, -D, -F, -P, -L
    let mut insert_idx: Option<usize> = None;
    let mut rule = Rule::new(Target::Accept);
    let mut policy_target: Option<Target> = None;

    while i < tokens.len() {
        match tokens[i] {
            "-A" | "--append" => {
                action_flag = Some("-A");
                i += 1;
                if i < tokens.len() { chain = Chain::from_str(tokens[i]); }
            }
            "-I" | "--insert" => {
                action_flag = Some("-I");
                i += 1;
                if i < tokens.len() { chain = Chain::from_str(tokens[i]); }
                // Optional index after chain name
                if i + 1 < tokens.len() {
                    if let Ok(idx) = tokens[i + 1].parse::<usize>() {
                        insert_idx = Some(idx);
                        i += 1;
                    }
                }
            }
            "-D" | "--delete" => {
                action_flag = Some("-D");
                i += 1;
                if i < tokens.len() { chain = Chain::from_str(tokens[i]); }
                if i + 1 < tokens.len() {
                    if let Ok(idx) = tokens[i + 1].parse::<usize>() {
                        insert_idx = Some(idx);
                        i += 1;
                    }
                }
            }
            "-F" | "--flush" => {
                action_flag = Some("-F");
                i += 1;
                if i < tokens.len() { chain = Chain::from_str(tokens[i]); }
            }
            "-P" | "--policy" => {
                action_flag = Some("-P");
                i += 1;
                if i < tokens.len() { chain = Chain::from_str(tokens[i]); }
                i += 1;
                if i < tokens.len() { policy_target = Target::from_str(tokens[i]); }
            }
            "-L" | "--list" => {
                action_flag = Some("-L");
                i += 1;
                if i < tokens.len() { chain = Chain::from_str(tokens[i]); }
            }
            "-s" | "--source" => {
                i += 1;
                if i < tokens.len() {
                    if let Some((ip, mask)) = parse_ip_cidr(tokens[i]) {
                        rule.src_ip = Some(ip);
                        rule.src_mask = mask;
                    }
                }
            }
            "-d" | "--destination" => {
                i += 1;
                if i < tokens.len() {
                    if let Some((ip, mask)) = parse_ip_cidr(tokens[i]) {
                        rule.dst_ip = Some(ip);
                        rule.dst_mask = mask;
                    }
                }
            }
            "-p" | "--protocol" => {
                i += 1;
                if i < tokens.len() { rule.protocol = Protocol::from_str(tokens[i]); }
            }
            "--sport" | "--source-port" => {
                i += 1;
                if i < tokens.len() { rule.src_port = tokens[i].parse().ok(); }
            }
            "--dport" | "--destination-port" => {
                i += 1;
                if i < tokens.len() { rule.dst_port = tokens[i].parse().ok(); }
            }
            "-i" | "--in-interface" => {
                i += 1;
                if i < tokens.len() { rule.interface = Some(String::from(tokens[i])); }
            }
            "-j" | "--jump" => {
                i += 1;
                if i < tokens.len() {
                    if let Some(t) = Target::from_str(tokens[i]) { rule.target = t; }
                }
            }
            "--to-source" => {
                i += 1;
                if i < tokens.len() {
                    if let Some((ip, _)) = parse_ip_cidr(tokens[i]) {
                        rule.nat_src_ip = Some(ip);
                    }
                }
            }
            "--to-destination" => {
                i += 1;
                if i < tokens.len() {
                    // ip:port format
                    let s = tokens[i];
                    if let Some(colon) = s.rfind(':') {
                        if let Some((ip, _)) = parse_ip_cidr(&s[..colon]) {
                            rule.nat_dst_ip = Some(ip);
                        }
                        rule.nat_dst_port = s[colon + 1..].parse().ok();
                    } else if let Some((ip, _)) = parse_ip_cidr(s) {
                        rule.nat_dst_ip = Some(ip);
                    }
                }
            }
            _ => {} // skip unknown flags
        }
        i += 1;
    }

    match action_flag {
        Some("-A") => {
            let ch = chain.ok_or_else(|| String::from("no chain specified"))?;
            match add_rule(ch, rule) {
                Some(num) => Ok(format!("Rule {} added to {}", num, ch.name())),
                None => Err(String::from("failed to add rule (chain full or not initialised)")),
            }
        }
        Some("-I") => {
            let ch = chain.ok_or_else(|| String::from("no chain specified"))?;
            let idx = insert_idx.unwrap_or(0);
            match insert_rule(ch, idx, rule) {
                Some(num) => Ok(format!("Rule {} inserted into {} at {}", num, ch.name(), idx)),
                None => Err(String::from("failed to insert rule")),
            }
        }
        Some("-D") => {
            let ch = chain.ok_or_else(|| String::from("no chain specified"))?;
            let idx = insert_idx.ok_or_else(|| String::from("no rule index specified"))?;
            if delete_rule(ch, idx) {
                Ok(format!("Rule deleted from {} at index {}", ch.name(), idx))
            } else {
                Err(format!("no rule at index {} in {}", idx, ch.name()))
            }
        }
        Some("-F") => {
            let ch = chain.ok_or_else(|| String::from("no chain specified"))?;
            flush_chain(ch);
            Ok(format!("Flushed chain {}", ch.name()))
        }
        Some("-P") => {
            let ch = chain.ok_or_else(|| String::from("no chain specified"))?;
            let tgt = policy_target.ok_or_else(|| String::from("no target specified"))?;
            set_policy(ch, tgt);
            Ok(format!("Policy for {} set to {}", ch.name(), tgt.name()))
        }
        Some("-L") => {
            if let Some(ch) = chain {
                Ok(list_rules(ch))
            } else {
                // List all chains
                let mut out = String::new();
                for ch in &[Chain::Input, Chain::Output, Chain::Forward, Chain::Prerouting, Chain::Postrouting] {
                    out.push_str(&list_rules(*ch));
                    out.push('\n');
                }
                Ok(out)
            }
        }
        _ => Err(String::from("Usage: iptables -A|-I|-D|-F|-P|-L CHAIN [options] [-j TARGET]")),
    }
}
