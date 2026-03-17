/// RIP (Routing Information Protocol) v2 for MerlionOS.
/// Simple distance-vector routing for small networks (RFC 2453).

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use alloc::borrow::ToOwned;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// RIPv2 multicast address: 224.0.0.9
const RIP_MULTICAST: [u8; 4] = [224, 0, 0, 9];

/// RIP UDP port
const RIP_PORT: u16 = 520;

/// RIP version
const RIP_VERSION: u8 = 2;

/// Maximum metric (infinity / unreachable)
const RIP_INFINITY: u8 = 16;

/// Valid metric range 1-15
const RIP_MAX_METRIC: u8 = 15;

/// Periodic update interval (seconds)
const UPDATE_INTERVAL: u32 = 30;

/// Route timeout (seconds) - route becomes invalid
const ROUTE_TIMEOUT: u32 = 180;

/// Garbage collection timer (seconds) - route removed after timeout + gc
const GARBAGE_TIMER: u32 = 120;

/// Maximum routes
const MAX_ROUTES: usize = 256;

/// Maximum configured networks
const MAX_NETWORKS: usize = 32;

/// Address family: IPv4
const AF_INET: u16 = 2;

/// Authentication type: simple password
const AUTH_SIMPLE_PASSWORD: u16 = 2;

// ---------------------------------------------------------------------------
// RIP Message Types
// ---------------------------------------------------------------------------

/// RIP command/message types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RipCommand {
    Request = 1,
    Response = 2,
}

impl RipCommand {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Request),
            2 => Some(Self::Response),
            _ => None,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Request => "Request",
            Self::Response => "Response",
        }
    }
}

// ---------------------------------------------------------------------------
// Data Structures
// ---------------------------------------------------------------------------

/// A RIP route entry (RTE) per RFC 2453 Section 3.6
#[derive(Debug, Clone)]
pub struct RipRoute {
    pub address_family: u16,
    pub route_tag: u16,
    pub ip: [u8; 4],
    pub subnet_mask: [u8; 4],
    pub next_hop: [u8; 4],
    pub metric: u8,
    pub source: [u8; 4],
    pub last_update: u64,
    pub timeout_timer: u32,
    pub garbage_timer: u32,
    pub changed: bool,
}

/// A configured network for RIP
#[derive(Debug, Clone)]
pub struct RipNetwork {
    pub prefix: [u8; 4],
    pub mask: [u8; 4],
}

/// Main RIP instance
struct RipInstance {
    routes: Vec<RipRoute>,
    networks: Vec<RipNetwork>,
    enabled: bool,
    tick_count: u64,
    split_horizon: bool,
    poison_reverse: bool,
    triggered_updates: bool,
    auth_enabled: bool,
    auth_password: [u8; 16],
}

impl RipInstance {
    const fn new() -> Self {
        Self {
            routes: Vec::new(),
            networks: Vec::new(),
            enabled: false,
            tick_count: 0,
            split_horizon: true,
            poison_reverse: true,
            triggered_updates: true,
            auth_enabled: false,
            auth_password: [0u8; 16],
        }
    }

    /// Add a network to participate in RIP
    fn add_network(&mut self, prefix: [u8; 4], mask: [u8; 4]) -> bool {
        if self.networks.len() >= MAX_NETWORKS {
            return false;
        }
        if self.networks.iter().any(|n| n.prefix == prefix && n.mask == mask) {
            return false;
        }
        self.networks.push(RipNetwork { prefix, mask });

        // Add a connected route for this network
        if self.routes.len() < MAX_ROUTES {
            self.routes.push(RipRoute {
                address_family: AF_INET,
                route_tag: 0,
                ip: prefix,
                subnet_mask: mask,
                next_hop: [0; 4],
                metric: 1,
                source: [0; 4],
                last_update: self.tick_count,
                timeout_timer: 0,
                garbage_timer: 0,
                changed: true,
            });
            STATS.routes_installed.fetch_add(1, Ordering::Relaxed);
        }
        true
    }

    /// Process an incoming RIP response (route update from neighbor)
    fn process_response(&mut self, source: [u8; 4], entries: &[(([u8; 4], [u8; 4], [u8; 4]), u8)]) {
        STATS.responses_received.fetch_add(1, Ordering::Relaxed);

        for &((ip, mask, nh), metric) in entries {
            if metric > RIP_INFINITY {
                continue;
            }
            // Add 1 to the received metric (distance-vector hop cost)
            let new_metric = if metric >= RIP_MAX_METRIC { RIP_INFINITY } else { metric + 1 };
            let next_hop = if nh == [0; 4] { source } else { nh };

            // Check if route exists
            if let Some(existing) = self.routes.iter_mut().find(|r| r.ip == ip && r.subnet_mask == mask) {
                // Update if better metric or same source
                if new_metric < existing.metric || existing.source == source {
                    existing.metric = new_metric;
                    existing.next_hop = next_hop;
                    existing.source = source;
                    existing.last_update = self.tick_count;
                    existing.timeout_timer = ROUTE_TIMEOUT;
                    existing.garbage_timer = 0;
                    existing.changed = true;
                }
            } else if new_metric < RIP_INFINITY && self.routes.len() < MAX_ROUTES {
                // New route
                self.routes.push(RipRoute {
                    address_family: AF_INET,
                    route_tag: 0,
                    ip,
                    subnet_mask: mask,
                    next_hop,
                    metric: new_metric,
                    source,
                    last_update: self.tick_count,
                    timeout_timer: ROUTE_TIMEOUT,
                    garbage_timer: 0,
                    changed: true,
                });
                STATS.routes_installed.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Age routes: decrement timers, mark expired, garbage collect
    fn age_routes(&mut self) {
        let tick = self.tick_count;
        for route in self.routes.iter_mut() {
            if route.source == [0; 4] {
                // Connected routes don't age
                continue;
            }
            let age = tick.saturating_sub(route.last_update) as u32;
            if age > ROUTE_TIMEOUT {
                if route.metric < RIP_INFINITY {
                    // Route timed out, set to infinity (poison)
                    route.metric = RIP_INFINITY;
                    route.changed = true;
                    route.garbage_timer = GARBAGE_TIMER;
                    STATS.routes_expired.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        // Remove garbage-collected routes
        self.routes.retain(|r| {
            if r.source == [0; 4] {
                return true;
            }
            let age = tick.saturating_sub(r.last_update) as u32;
            age <= ROUTE_TIMEOUT + GARBAGE_TIMER
        });
    }

    /// Build a response with split horizon / poison reverse
    fn build_response(&self, out_iface_source: [u8; 4]) -> Vec<(([u8; 4], [u8; 4], [u8; 4]), u8)> {
        let mut entries = Vec::new();
        for route in &self.routes {
            let metric = if self.split_horizon && route.next_hop == out_iface_source {
                if self.poison_reverse {
                    RIP_INFINITY
                } else {
                    continue;
                }
            } else {
                route.metric
            };
            entries.push(((route.ip, route.subnet_mask, route.next_hop), metric));
        }
        entries
    }
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

struct RipStats {
    requests_sent: AtomicU64,
    requests_received: AtomicU64,
    responses_sent: AtomicU64,
    responses_received: AtomicU64,
    routes_installed: AtomicU64,
    routes_expired: AtomicU64,
    triggered_updates: AtomicU64,
    bad_packets: AtomicU64,
}

impl RipStats {
    const fn new() -> Self {
        Self {
            requests_sent: AtomicU64::new(0),
            requests_received: AtomicU64::new(0),
            responses_sent: AtomicU64::new(0),
            responses_received: AtomicU64::new(0),
            routes_installed: AtomicU64::new(0),
            routes_expired: AtomicU64::new(0),
            triggered_updates: AtomicU64::new(0),
            bad_packets: AtomicU64::new(0),
        }
    }
}

// ---------------------------------------------------------------------------
// Global State
// ---------------------------------------------------------------------------

static RIP: Mutex<RipInstance> = Mutex::new(RipInstance::new());
static STATS: RipStats = RipStats::new();
static INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize RIP subsystem
pub fn init() {
    let mut rip = RIP.lock();
    rip.enabled = true;
    rip.split_horizon = true;
    rip.poison_reverse = true;
    rip.triggered_updates = true;
    INITIALIZED.store(true, Ordering::SeqCst);
}

/// Add a network to RIP
pub fn add_network(prefix: [u8; 4], mask: [u8; 4]) -> bool {
    RIP.lock().add_network(prefix, mask)
}

/// Show RIP routes
pub fn show_routes() -> String {
    let rip = RIP.lock();
    if rip.routes.is_empty() {
        return "No RIP routes\n".to_owned();
    }
    let fmt_ip = |ip: [u8; 4]| format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    let mut out = String::new();
    out.push_str("Network          Mask             Next Hop         Metric  Source           Tag\n");
    out.push_str("---------------- ---------------- ---------------- ------- ---------------- ----\n");
    for r in &rip.routes {
        let src = if r.source == [0; 4] { "connected".to_owned() } else { fmt_ip(r.source) };
        out.push_str(&format!(
            "{:<16} {:<16} {:<16} {:<7} {:<16} {}\n",
            fmt_ip(r.ip), fmt_ip(r.subnet_mask), fmt_ip(r.next_hop),
            r.metric, src, r.route_tag
        ));
    }
    out
}

/// Show RIP general info
pub fn rip_info() -> String {
    let rip = RIP.lock();
    let fmt_ip = |ip: [u8; 4]| format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    let active_routes = rip.routes.iter().filter(|r| r.metric < RIP_INFINITY).count();
    let mut out = String::new();
    out.push_str("RIP Information:\n");
    out.push_str(&format!("  Version:           RIPv{}\n", RIP_VERSION));
    out.push_str(&format!("  Enabled:           {}\n", rip.enabled));
    out.push_str(&format!("  Port:              {}\n", RIP_PORT));
    out.push_str(&format!("  Multicast:         {}\n", fmt_ip(RIP_MULTICAST)));
    out.push_str(&format!("  Update interval:   {} sec\n", UPDATE_INTERVAL));
    out.push_str(&format!("  Route timeout:     {} sec\n", ROUTE_TIMEOUT));
    out.push_str(&format!("  Garbage timer:     {} sec\n", GARBAGE_TIMER));
    out.push_str(&format!("  Max metric:        {} (infinity={})\n", RIP_MAX_METRIC, RIP_INFINITY));
    out.push_str(&format!("  Split horizon:     {}\n", rip.split_horizon));
    out.push_str(&format!("  Poison reverse:    {}\n", rip.poison_reverse));
    out.push_str(&format!("  Triggered updates: {}\n", rip.triggered_updates));
    out.push_str(&format!("  Authentication:    {}\n", if rip.auth_enabled { "simple password" } else { "none" }));
    out.push_str(&format!("  Networks:          {}\n", rip.networks.len()));
    out.push_str(&format!("  Routes:            {} total, {} active\n", rip.routes.len(), active_routes));
    if !rip.networks.is_empty() {
        out.push_str("  Configured networks:\n");
        for net in &rip.networks {
            out.push_str(&format!("    {}/{}\n", fmt_ip(net.prefix), fmt_ip(net.mask)));
        }
    }
    out
}

/// Show RIP statistics
pub fn rip_stats() -> String {
    let mut out = String::new();
    out.push_str("RIP Statistics:\n");
    out.push_str(&format!("  Requests sent:      {}\n", STATS.requests_sent.load(Ordering::Relaxed)));
    out.push_str(&format!("  Requests received:  {}\n", STATS.requests_received.load(Ordering::Relaxed)));
    out.push_str(&format!("  Responses sent:     {}\n", STATS.responses_sent.load(Ordering::Relaxed)));
    out.push_str(&format!("  Responses received: {}\n", STATS.responses_received.load(Ordering::Relaxed)));
    out.push_str(&format!("  Routes installed:   {}\n", STATS.routes_installed.load(Ordering::Relaxed)));
    out.push_str(&format!("  Routes expired:     {}\n", STATS.routes_expired.load(Ordering::Relaxed)));
    out.push_str(&format!("  Triggered updates:  {}\n", STATS.triggered_updates.load(Ordering::Relaxed)));
    out.push_str(&format!("  Bad packets:        {}\n", STATS.bad_packets.load(Ordering::Relaxed)));
    out
}
