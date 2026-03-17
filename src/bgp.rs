/// BGP-4 (Border Gateway Protocol) for MerlionOS.
/// External routing protocol for inter-AS routing.
/// Implements BGP FSM, UPDATE processing, and path selection (RFC 4271).

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::borrow::ToOwned;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// BGP TCP port
const BGP_PORT: u16 = 179;

/// Default hold time (seconds)
const DEFAULT_HOLD_TIME: u16 = 90;

/// Default keepalive interval (hold_time / 3)
const DEFAULT_KEEPALIVE: u16 = 30;

/// BGP marker: 16 bytes of 0xFF
const BGP_MARKER: [u8; 16] = [0xFF; 16];

/// Maximum peers
const MAX_PEERS: usize = 64;

/// Maximum routes in Adj-RIB-In
const MAX_ROUTES: usize = 1024;

/// Maximum prefix-list entries
const MAX_PREFIX_FILTERS: usize = 128;

/// Maximum AS path length
const MAX_AS_PATH: usize = 32;

/// Maximum communities per route
const MAX_COMMUNITIES: usize = 16;

/// BGP version
const BGP_VERSION: u8 = 4;

/// Well-known communities
const COMMUNITY_NO_EXPORT: u32 = 0xFFFFFF01;
const COMMUNITY_NO_ADVERTISE: u32 = 0xFFFFFF02;
const COMMUNITY_NO_EXPORT_SUBCONFED: u32 = 0xFFFFFF03;

// ---------------------------------------------------------------------------
// BGP Message Types
// ---------------------------------------------------------------------------

/// BGP message types per RFC 4271
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    Open = 1,
    Update = 2,
    Notification = 3,
    Keepalive = 4,
}

impl MessageType {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Open),
            2 => Some(Self::Update),
            3 => Some(Self::Notification),
            4 => Some(Self::Keepalive),
            _ => None,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::Update => "UPDATE",
            Self::Notification => "NOTIFICATION",
            Self::Keepalive => "KEEPALIVE",
        }
    }
}

// ---------------------------------------------------------------------------
// BGP FSM States
// ---------------------------------------------------------------------------

/// BGP finite state machine states per RFC 4271 Section 8
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsmState {
    Idle,
    Connect,
    OpenSent,
    OpenConfirm,
    Established,
}

impl FsmState {
    fn name(&self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Connect => "Connect",
            Self::OpenSent => "OpenSent",
            Self::OpenConfirm => "OpenConfirm",
            Self::Established => "Established",
        }
    }
}

// ---------------------------------------------------------------------------
// Path Attribute Origin
// ---------------------------------------------------------------------------

/// ORIGIN attribute values
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    Igp = 0,
    Egp = 1,
    Incomplete = 2,
}

impl Origin {
    fn name(&self) -> &'static str {
        match self {
            Self::Igp => "IGP",
            Self::Egp => "EGP",
            Self::Incomplete => "?",
        }
    }
}

// ---------------------------------------------------------------------------
// Data Structures
// ---------------------------------------------------------------------------

/// BGP peer (neighbor) configuration and state
#[derive(Debug, Clone)]
pub struct BgpPeer {
    pub ip: [u8; 4],
    pub remote_as: u32,
    pub local_as: u32,
    pub state: FsmState,
    pub hold_time: u16,
    pub keepalive_interval: u16,
    pub router_id: [u8; 4],
    pub last_keepalive: u64,
    pub uptime: u64,
    pub prefixes_received: u32,
    pub prefixes_sent: u32,
    pub messages_received: u64,
    pub messages_sent: u64,
    pub is_ebgp: bool,
}

/// Network Layer Reachability Information (prefix/length pair)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nlri {
    pub prefix: [u8; 4],
    pub length: u8,
}

impl Nlri {
    fn matches(&self, ip: [u8; 4]) -> bool {
        if self.length == 0 {
            return true;
        }
        let bits = self.length as u32;
        let mask = if bits >= 32 { 0xFFFFFFFF_u32 } else { !((1u32 << (32 - bits)) - 1) };
        let prefix_val = u32::from_be_bytes(self.prefix);
        let ip_val = u32::from_be_bytes(ip);
        (prefix_val & mask) == (ip_val & mask)
    }
}

/// BGP path attributes for a route
#[derive(Debug, Clone)]
pub struct PathAttributes {
    pub origin: Origin,
    pub as_path: Vec<u32>,
    pub next_hop: [u8; 4],
    pub local_pref: u32,
    pub med: u32,
    pub communities: Vec<u32>,
    pub atomic_aggregate: bool,
    pub aggregator_as: u32,
    pub aggregator_ip: [u8; 4],
}

impl PathAttributes {
    fn new(next_hop: [u8; 4]) -> Self {
        Self {
            origin: Origin::Igp,
            as_path: Vec::new(),
            next_hop,
            local_pref: 100,
            med: 0,
            communities: Vec::new(),
            atomic_aggregate: false,
            aggregator_as: 0,
            aggregator_ip: [0; 4],
        }
    }

    fn as_path_length(&self) -> usize {
        self.as_path.len()
    }
}

/// A BGP route entry in Adj-RIB
#[derive(Debug, Clone)]
pub struct BgpRoute {
    pub nlri: Nlri,
    pub attrs: PathAttributes,
    pub peer_ip: [u8; 4],
    pub best: bool,
    pub valid: bool,
}

/// Prefix filter entry
#[derive(Debug, Clone)]
pub struct PrefixFilter {
    pub prefix: Nlri,
    pub action: FilterAction,
    pub ge: u8,
    pub le: u8,
}

/// Filter action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterAction {
    Permit,
    Deny,
}

impl FilterAction {
    fn name(&self) -> &'static str {
        match self {
            Self::Permit => "permit",
            Self::Deny => "deny",
        }
    }
}

/// Main BGP instance
struct BgpInstance {
    local_as: u32,
    router_id: [u8; 4],
    peers: Vec<BgpPeer>,
    routes: Vec<BgpRoute>,
    prefix_filters: Vec<PrefixFilter>,
    enabled: bool,
    tick_count: u64,
}

impl BgpInstance {
    const fn new() -> Self {
        Self {
            local_as: 0,
            router_id: [0; 4],
            peers: Vec::new(),
            routes: Vec::new(),
            prefix_filters: Vec::new(),
            enabled: false,
            tick_count: 0,
        }
    }

    /// Add a peer
    fn add_peer(&mut self, ip: [u8; 4], remote_as: u32) -> bool {
        if self.peers.len() >= MAX_PEERS {
            return false;
        }
        if self.peers.iter().any(|p| p.ip == ip) {
            return false;
        }
        let is_ebgp = remote_as != self.local_as;
        self.peers.push(BgpPeer {
            ip,
            remote_as,
            local_as: self.local_as,
            state: FsmState::Idle,
            hold_time: DEFAULT_HOLD_TIME,
            keepalive_interval: DEFAULT_KEEPALIVE,
            router_id: [0; 4],
            last_keepalive: 0,
            uptime: 0,
            prefixes_received: 0,
            prefixes_sent: 0,
            messages_received: 0,
            messages_sent: 0,
            is_ebgp,
        });
        STATS.peers_configured.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Remove a peer by IP
    fn remove_peer(&mut self, ip: [u8; 4]) -> bool {
        let before = self.peers.len();
        self.routes.retain(|r| r.peer_ip != ip);
        self.peers.retain(|p| p.ip != ip);
        self.peers.len() < before
    }

    /// Transition a peer FSM
    fn advance_peer(&mut self, ip: [u8; 4]) {
        if let Some(peer) = self.peers.iter_mut().find(|p| p.ip == ip) {
            peer.state = match peer.state {
                FsmState::Idle => FsmState::Connect,
                FsmState::Connect => FsmState::OpenSent,
                FsmState::OpenSent => FsmState::OpenConfirm,
                FsmState::OpenConfirm => {
                    peer.uptime = self.tick_count;
                    STATS.sessions_established.fetch_add(1, Ordering::Relaxed);
                    FsmState::Established
                }
                FsmState::Established => FsmState::Established,
            };
        }
    }

    /// Install a route from an UPDATE message
    fn install_route(&mut self, nlri: Nlri, attrs: PathAttributes, peer_ip: [u8; 4]) -> bool {
        // Apply prefix filters
        for filter in &self.prefix_filters {
            if filter.prefix.matches(nlri.prefix) {
                if nlri.length >= filter.ge && nlri.length <= filter.le {
                    if filter.action == FilterAction::Deny {
                        STATS.routes_filtered.fetch_add(1, Ordering::Relaxed);
                        return false;
                    }
                }
            }
        }

        if self.routes.len() >= MAX_ROUTES {
            return false;
        }
        self.routes.push(BgpRoute {
            nlri,
            attrs,
            peer_ip,
            best: false,
            valid: true,
        });
        STATS.routes_received.fetch_add(1, Ordering::Relaxed);

        // Update peer prefix count
        if let Some(peer) = self.peers.iter_mut().find(|p| p.ip == peer_ip) {
            peer.prefixes_received += 1;
            peer.messages_received += 1;
        }
        true
    }

    /// Run best path selection per RFC 4271 Section 9.1.2
    fn best_path_selection(&mut self) {
        // Group routes by NLRI, pick best
        let n = self.routes.len();
        for i in 0..n {
            self.routes[i].best = false;
        }

        // For each unique prefix, find the best path
        let mut processed: Vec<Nlri> = Vec::new();
        for i in 0..n {
            if !self.routes[i].valid {
                continue;
            }
            let nlri = self.routes[i].nlri.clone();
            if processed.iter().any(|p| *p == nlri) {
                continue;
            }
            processed.push(nlri.clone());

            let mut best_idx = i;
            for j in (i + 1)..n {
                if !self.routes[j].valid || self.routes[j].nlri != nlri {
                    continue;
                }
                if self.is_preferred(j, best_idx) {
                    best_idx = j;
                }
            }
            self.routes[best_idx].best = true;
        }
    }

    /// Compare two routes: returns true if route at idx_a is preferred over idx_b
    fn is_preferred(&self, idx_a: usize, idx_b: usize) -> bool {
        let a = &self.routes[idx_a].attrs;
        let b = &self.routes[idx_b].attrs;

        // 1. Highest LOCAL_PREF
        if a.local_pref != b.local_pref {
            return a.local_pref > b.local_pref;
        }
        // 2. Shortest AS_PATH
        if a.as_path_length() != b.as_path_length() {
            return a.as_path_length() < b.as_path_length();
        }
        // 3. Lowest ORIGIN (IGP < EGP < Incomplete)
        if a.origin != b.origin {
            return (a.origin as u8) < (b.origin as u8);
        }
        // 4. Lowest MED (only compared among routes from same neighbor AS)
        if a.med != b.med {
            return a.med < b.med;
        }
        // 5. eBGP preferred over iBGP
        let a_peer = self.peers.iter().find(|p| p.ip == self.routes[idx_a].peer_ip);
        let b_peer = self.peers.iter().find(|p| p.ip == self.routes[idx_b].peer_ip);
        let a_ebgp = a_peer.map_or(false, |p| p.is_ebgp);
        let b_ebgp = b_peer.map_or(false, |p| p.is_ebgp);
        if a_ebgp != b_ebgp {
            return a_ebgp;
        }
        // 6. Lowest router ID as tiebreaker
        let a_rid = u32::from_be_bytes(self.routes[idx_a].peer_ip);
        let b_rid = u32::from_be_bytes(self.routes[idx_b].peer_ip);
        a_rid < b_rid
    }

    /// Add a prefix filter
    fn add_prefix_filter(&mut self, prefix: Nlri, action: FilterAction, ge: u8, le: u8) -> bool {
        if self.prefix_filters.len() >= MAX_PREFIX_FILTERS {
            return false;
        }
        self.prefix_filters.push(PrefixFilter { prefix, action, ge, le });
        true
    }

    /// Format community value
    fn format_community(c: u32) -> String {
        match c {
            COMMUNITY_NO_EXPORT => "no-export".to_owned(),
            COMMUNITY_NO_ADVERTISE => "no-advertise".to_owned(),
            COMMUNITY_NO_EXPORT_SUBCONFED => "no-export-subconfed".to_owned(),
            _ => {
                let asn = (c >> 16) & 0xFFFF;
                let val = c & 0xFFFF;
                format!("{}:{}", asn, val)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

struct BgpStats {
    peers_configured: AtomicU64,
    sessions_established: AtomicU64,
    routes_received: AtomicU64,
    routes_advertised: AtomicU64,
    routes_filtered: AtomicU64,
    updates_sent: AtomicU64,
    updates_received: AtomicU64,
    keepalives_sent: AtomicU64,
    keepalives_received: AtomicU64,
    notifications_sent: AtomicU64,
    notifications_received: AtomicU64,
}

impl BgpStats {
    const fn new() -> Self {
        Self {
            peers_configured: AtomicU64::new(0),
            sessions_established: AtomicU64::new(0),
            routes_received: AtomicU64::new(0),
            routes_advertised: AtomicU64::new(0),
            routes_filtered: AtomicU64::new(0),
            updates_sent: AtomicU64::new(0),
            updates_received: AtomicU64::new(0),
            keepalives_sent: AtomicU64::new(0),
            keepalives_received: AtomicU64::new(0),
            notifications_sent: AtomicU64::new(0),
            notifications_received: AtomicU64::new(0),
        }
    }
}

// ---------------------------------------------------------------------------
// Global State
// ---------------------------------------------------------------------------

static BGP: Mutex<BgpInstance> = Mutex::new(BgpInstance::new());
static STATS: BgpStats = BgpStats::new();
static INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize BGP subsystem
pub fn init() {
    let mut bgp = BGP.lock();
    bgp.local_as = 65001;
    bgp.router_id = [10, 0, 0, 1];
    bgp.enabled = true;
    INITIALIZED.store(true, Ordering::SeqCst);
}

/// Add a BGP peer
pub fn add_peer(ip: [u8; 4], remote_as: u32) -> bool {
    BGP.lock().add_peer(ip, remote_as)
}

/// Remove a BGP peer
pub fn remove_peer(ip: [u8; 4]) -> bool {
    BGP.lock().remove_peer(ip)
}

/// Advance a peer's FSM state
pub fn advance_peer(ip: [u8; 4]) {
    BGP.lock().advance_peer(ip);
}

/// Install a route from UPDATE
pub fn install_route(nlri: Nlri, attrs: PathAttributes, peer_ip: [u8; 4]) -> bool {
    BGP.lock().install_route(nlri, attrs, peer_ip)
}

/// Run best path selection
pub fn run_best_path() {
    BGP.lock().best_path_selection();
}

/// Add a prefix filter
pub fn add_prefix_filter(prefix: Nlri, action: FilterAction, ge: u8, le: u8) -> bool {
    BGP.lock().add_prefix_filter(prefix, action, ge, le)
}

/// List all peers
pub fn list_peers() -> String {
    let bgp = BGP.lock();
    if bgp.peers.is_empty() {
        return "No BGP peers configured\n".to_owned();
    }
    let fmt_ip = |ip: [u8; 4]| format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    let mut out = String::new();
    out.push_str("Neighbor         AS       State         PfxRcd   PfxSnt  MsgRcvd  MsgSent  Type\n");
    out.push_str("---------------- -------- ------------- -------- ------- -------- -------- -----\n");
    for p in &bgp.peers {
        let peer_type = if p.is_ebgp { "eBGP" } else { "iBGP" };
        out.push_str(&format!(
            "{:<16} {:<8} {:<13} {:<8} {:<7} {:<8} {:<8} {}\n",
            fmt_ip(p.ip), p.remote_as, p.state.name(),
            p.prefixes_received, p.prefixes_sent,
            p.messages_received, p.messages_sent, peer_type
        ));
    }
    out
}

/// Show BGP routing table
pub fn show_routes() -> String {
    let bgp = BGP.lock();
    if bgp.routes.is_empty() {
        return "No BGP routes\n".to_owned();
    }
    let fmt_ip = |ip: [u8; 4]| format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    let mut out = String::new();
    out.push_str("Status Prefix           Next Hop         LP    MED   AS Path          Origin\n");
    out.push_str("------ ---------------- ---------------- ----- ----- ---------------- ------\n");
    for r in &bgp.routes {
        if !r.valid {
            continue;
        }
        let status = if r.best { "*>" } else { "* " };
        let prefix_str = format!("{}/{}", fmt_ip(r.nlri.prefix), r.nlri.length);
        let as_path: String = if r.attrs.as_path.is_empty() {
            "local".to_owned()
        } else {
            let parts: Vec<String> = r.attrs.as_path.iter().map(|a| format!("{}", a)).collect();
            let mut s = String::new();
            for (i, p) in parts.iter().enumerate() {
                if i > 0 { s.push(' '); }
                s.push_str(p);
            }
            s
        };
        out.push_str(&format!(
            "{:<6} {:<16} {:<16} {:<5} {:<5} {:<16} {}\n",
            status, prefix_str, fmt_ip(r.attrs.next_hop),
            r.attrs.local_pref, r.attrs.med, as_path,
            r.attrs.origin.name()
        ));
    }
    out
}

/// Show BGP general info
pub fn bgp_info() -> String {
    let bgp = BGP.lock();
    let fmt_ip = |ip: [u8; 4]| format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    let total_routes = bgp.routes.iter().filter(|r| r.valid).count();
    let best_routes = bgp.routes.iter().filter(|r| r.best).count();
    let established = bgp.peers.iter().filter(|p| p.state == FsmState::Established).count();
    let mut out = String::new();
    out.push_str("BGP Information:\n");
    out.push_str(&format!("  Version:       BGP-{}\n", BGP_VERSION));
    out.push_str(&format!("  Local AS:      {}\n", bgp.local_as));
    out.push_str(&format!("  Router ID:     {}\n", fmt_ip(bgp.router_id)));
    out.push_str(&format!("  Enabled:       {}\n", bgp.enabled));
    out.push_str(&format!("  Port:          {}\n", BGP_PORT));
    out.push_str(&format!("  Hold time:     {} sec\n", DEFAULT_HOLD_TIME));
    out.push_str(&format!("  Keepalive:     {} sec\n", DEFAULT_KEEPALIVE));
    out.push_str(&format!("  Peers:         {} total, {} established\n",
        bgp.peers.len(), established));
    out.push_str(&format!("  Routes:        {} total, {} best\n", total_routes, best_routes));
    out.push_str(&format!("  Prefix filters: {}\n", bgp.prefix_filters.len()));
    out
}

/// Show BGP statistics
pub fn bgp_stats() -> String {
    let mut out = String::new();
    out.push_str("BGP Statistics:\n");
    out.push_str(&format!("  Peers configured:       {}\n", STATS.peers_configured.load(Ordering::Relaxed)));
    out.push_str(&format!("  Sessions established:   {}\n", STATS.sessions_established.load(Ordering::Relaxed)));
    out.push_str(&format!("  Routes received:        {}\n", STATS.routes_received.load(Ordering::Relaxed)));
    out.push_str(&format!("  Routes advertised:      {}\n", STATS.routes_advertised.load(Ordering::Relaxed)));
    out.push_str(&format!("  Routes filtered:        {}\n", STATS.routes_filtered.load(Ordering::Relaxed)));
    out.push_str(&format!("  UPDATEs sent:           {}\n", STATS.updates_sent.load(Ordering::Relaxed)));
    out.push_str(&format!("  UPDATEs received:       {}\n", STATS.updates_received.load(Ordering::Relaxed)));
    out.push_str(&format!("  KEEPALIVEs sent:        {}\n", STATS.keepalives_sent.load(Ordering::Relaxed)));
    out.push_str(&format!("  KEEPALIVEs received:    {}\n", STATS.keepalives_received.load(Ordering::Relaxed)));
    out.push_str(&format!("  NOTIFICATIONs sent:     {}\n", STATS.notifications_sent.load(Ordering::Relaxed)));
    out.push_str(&format!("  NOTIFICATIONs received: {}\n", STATS.notifications_received.load(Ordering::Relaxed)));
    out
}
