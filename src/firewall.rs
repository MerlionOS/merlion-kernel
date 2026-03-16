/// Packet firewall for MerlionOS.
/// Provides a priority-ordered rule table that matches incoming and outgoing
/// packets against configurable rules (source/destination IP and port,
/// protocol, direction) and returns an action (Allow, Deny, or Log).
/// Thread-safe via `spin::Mutex`; suitable for `#![no_std]` kernel use.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Maximum number of rules the table will hold.
const MAX_RULES: usize = 256;

/// Action to take when a rule matches a packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Allow the packet through.
    Allow,
    /// Silently drop the packet.
    Deny,
    /// Log the packet and allow it.
    Log,
}

/// Traffic direction a rule applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Inbound traffic only.
    In,
    /// Outbound traffic only.
    Out,
    /// Both inbound and outbound traffic.
    Both,
}

/// Network protocol selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Udp,
    Icmp,
    /// Matches every protocol.
    Any,
}

/// A single firewall rule describing which packets to match and what action
/// to take.  Fields set to `None` act as wildcards (match any value).
#[derive(Debug, Clone)]
pub struct FirewallRule {
    /// Unique identifier for this rule.
    pub id: u32,
    /// Action to perform on match.
    pub action: Action,
    /// Direction this rule applies to.
    pub direction: Direction,
    /// Protocol filter (`None` matches any protocol).
    pub protocol: Option<Protocol>,
    /// Source IP filter (`None` matches any source).
    pub src_ip: Option<[u8; 4]>,
    /// Destination IP filter (`None` matches any destination).
    pub dst_ip: Option<[u8; 4]>,
    /// Source port filter (`None` matches any source port).
    pub src_port: Option<u16>,
    /// Destination port filter (`None` matches any destination port).
    pub dst_port: Option<u16>,
    /// Priority (lower number = higher priority = evaluated first).
    pub priority: u8,
}

impl FirewallRule {
    /// Returns `true` when a packet's attributes satisfy every non-`None`
    /// field in this rule.
    fn matches(&self, dir: Direction, proto: Protocol,
               src_ip: [u8; 4], src_port: u16,
               dst_ip: [u8; 4], dst_port: u16) -> bool {
        if self.direction != Direction::Both && self.direction != dir { return false; }
        if let Some(p) = self.protocol {
            if p != Protocol::Any && p != proto { return false; }
        }
        if self.src_ip.is_some_and(|ip| ip != src_ip) { return false; }
        if self.dst_ip.is_some_and(|ip| ip != dst_ip) { return false; }
        if self.src_port.is_some_and(|p| p != src_port) { return false; }
        if self.dst_port.is_some_and(|p| p != dst_port) { return false; }
        true
    }
}

/// Counters for packets processed by the firewall (lock-free atomics).
pub struct PacketStats {
    /// Packets allowed through.
    pub allowed: AtomicU64,
    /// Packets denied / dropped.
    pub denied: AtomicU64,
    /// Packets logged.
    pub logged: AtomicU64,
}

impl PacketStats {
    const fn new() -> Self {
        Self {
            allowed: AtomicU64::new(0),
            denied: AtomicU64::new(0),
            logged: AtomicU64::new(0),
        }
    }

    /// Increment the counter matching `action`.
    fn record(&self, action: Action) {
        match action {
            Action::Allow => { self.allowed.fetch_add(1, Ordering::Relaxed); }
            Action::Deny  => { self.denied.fetch_add(1, Ordering::Relaxed); }
            Action::Log   => { self.logged.fetch_add(1, Ordering::Relaxed); }
        }
    }

    /// Return a snapshot as `(allowed, denied, logged)`.
    pub fn snapshot(&self) -> (u64, u64, u64) {
        (self.allowed.load(Ordering::Relaxed),
         self.denied.load(Ordering::Relaxed),
         self.logged.load(Ordering::Relaxed))
    }
}

/// Ordered collection of firewall rules with a configurable default policy.
/// Rules are kept sorted by priority (ascending) so the first match wins.
pub struct RuleTable {
    rules: Vec<FirewallRule>,
    /// Action applied when no rule matches.
    pub default_policy: Action,
    next_id: u32,
}

impl RuleTable {
    /// Create an empty rule table with the given default policy.
    pub fn new(default_policy: Action) -> Self {
        Self { rules: Vec::new(), default_policy, next_id: 1 }
    }

    /// Add a rule and return its assigned id.  The rule's `id` field is
    /// overwritten with an auto-generated unique value.  Returns `None` if
    /// the table is full.
    pub fn add_rule(&mut self, mut rule: FirewallRule) -> Option<u32> {
        if self.rules.len() >= MAX_RULES { return None; }
        let id = self.next_id;
        self.next_id += 1;
        rule.id = id;
        let pos = self.rules.iter().position(|r| r.priority > rule.priority)
            .unwrap_or(self.rules.len());
        self.rules.insert(pos, rule);
        Some(id)
    }

    /// Remove the rule with the given id.  Returns `true` if found.
    pub fn remove_rule(&mut self, id: u32) -> bool {
        if let Some(pos) = self.rules.iter().position(|r| r.id == id) {
            self.rules.remove(pos);
            true
        } else {
            false
        }
    }

    /// Return a shared reference to the current ordered rule list.
    pub fn list_rules(&self) -> &[FirewallRule] {
        &self.rules
    }

    /// Evaluate the rule table against a packet.  The first rule whose
    /// fields all match wins; if none matches, `default_policy` is returned.
    pub fn check_packet(&self, direction: Direction, protocol: Protocol,
                        src_ip: [u8; 4], src_port: u16,
                        dst_ip: [u8; 4], dst_port: u16) -> Action {
        for rule in &self.rules {
            if rule.matches(direction, protocol, src_ip, src_port, dst_ip, dst_port) {
                return rule.action;
            }
        }
        self.default_policy
    }

    /// Produce a human-readable table of every rule in evaluation order.
    pub fn format_rules(&self) -> String {
        if self.rules.is_empty() {
            return format!("(no rules -- default policy: {:?})\n", self.default_policy);
        }
        let mut out = String::new();
        out.push_str("ID   PRI  ACTION  DIR   PROTO  SRC_IP           SPORT  DST_IP           DPORT\n");
        out.push_str("---- ---- ------- ----- ------ ---------------- ------ ---------------- -----\n");
        for r in &self.rules {
            let proto = match r.protocol {
                Some(Protocol::Tcp)  => "TCP   ",
                Some(Protocol::Udp)  => "UDP   ",
                Some(Protocol::Icmp) => "ICMP  ",
                Some(Protocol::Any)  => "ANY   ",
                None                 => "*     ",
            };
            let fmt_ip = |ip: Option<[u8; 4]>| match ip {
                Some(a) => format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3]),
                None => "*".into(),
            };
            let fmt_port = |p: Option<u16>| match p {
                Some(v) => format!("{}", v),
                None => "*".into(),
            };
            let dir = match r.direction {
                Direction::In => "IN   ", Direction::Out => "OUT  ",
                Direction::Both => "BOTH ",
            };
            let act = match r.action {
                Action::Allow => "ALLOW  ", Action::Deny => "DENY   ",
                Action::Log => "LOG    ",
            };
            out.push_str(&format!(
                "{:<4} {:<4} {} {} {} {:<16} {:<6} {:<16} {}\n",
                r.id, r.priority, act, dir, proto,
                fmt_ip(r.src_ip), fmt_port(r.src_port),
                fmt_ip(r.dst_ip), fmt_port(r.dst_port),
            ));
        }
        out.push_str(&format!("Default policy: {:?}\n", self.default_policy));
        out
    }
}

// ---------------------------------------------------------------------------
// Global firewall state
// ---------------------------------------------------------------------------

/// Global firewall rule table, protected by a spinlock.
pub static FIREWALL: Mutex<Option<RuleTable>> = Mutex::new(None);

/// Global packet statistics (lock-free atomics).
pub static STATS: PacketStats = PacketStats::new();

/// Initialise the global firewall with the given default policy.
pub fn init(default_policy: Action) {
    *FIREWALL.lock() = Some(RuleTable::new(default_policy));
}

/// Add a rule to the global firewall.
/// Returns the assigned rule id, or `None` if the table is full or
/// uninitialised.
pub fn add_rule(rule: FirewallRule) -> Option<u32> {
    FIREWALL.lock().as_mut().and_then(|t| t.add_rule(rule))
}

/// Remove a rule by id from the global firewall.
pub fn remove_rule(id: u32) -> bool {
    FIREWALL.lock().as_mut().map_or(false, |t| t.remove_rule(id))
}

/// Check a packet against the global firewall, update statistics, and
/// return the resulting action.  Returns `Deny` if the firewall has not
/// been initialised.
pub fn check_packet(direction: Direction, protocol: Protocol,
                    src_ip: [u8; 4], src_port: u16,
                    dst_ip: [u8; 4], dst_port: u16) -> Action {
    let action = {
        let fw = FIREWALL.lock();
        match fw.as_ref() {
            Some(t) => t.check_packet(direction, protocol, src_ip, src_port, dst_ip, dst_port),
            None => Action::Deny,
        }
    };
    STATS.record(action);
    action
}

/// Return current packet statistics as `(allowed, denied, logged)`.
pub fn packet_stats() -> (u64, u64, u64) {
    STATS.snapshot()
}

/// Return a formatted string of all rules in the global firewall.
pub fn format_rules() -> String {
    let fw = FIREWALL.lock();
    match fw.as_ref() {
        Some(t) => t.format_rules(),
        None => "(firewall not initialised)\n".into(),
    }
}
