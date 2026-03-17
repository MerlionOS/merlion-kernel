/// OSPF (Open Shortest Path First) routing protocol for MerlionOS.
/// Link-state routing with Dijkstra SPF, area support, and neighbor management.
/// OSPFv2 for IPv4 (RFC 2328).

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use alloc::borrow::ToOwned;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// OSPF protocol number (IP protocol 89)
const OSPF_PROTOCOL: u8 = 89;

/// OSPF AllSPFRouters multicast: 224.0.0.5
const ALL_SPF_ROUTERS: [u8; 4] = [224, 0, 0, 5];

/// OSPF AllDRouters multicast: 224.0.0.6
const ALL_DR_ROUTERS: [u8; 4] = [224, 0, 0, 6];

/// Hello interval in seconds (broadcast/point-to-point)
const HELLO_INTERVAL: u32 = 10;

/// Dead interval = 4x hello interval
const DEAD_INTERVAL: u32 = 40;

/// Maximum number of neighbors
const MAX_NEIGHBORS: usize = 64;

/// Maximum number of interfaces
const MAX_INTERFACES: usize = 16;

/// Maximum LSAs in the LSDB per area
const MAX_LSAS: usize = 512;

/// Maximum areas
const MAX_AREAS: usize = 16;

/// Maximum routes in the routing table
const MAX_ROUTES: usize = 256;

/// Maximum nodes for Dijkstra SPF
const MAX_SPF_NODES: usize = 128;

/// Infinity cost
const INFINITY_COST: u32 = 0xFFFF;

/// LSA max age (seconds)
const LSA_MAX_AGE: u32 = 3600;

/// OSPF version
const OSPF_VERSION: u8 = 2;

// ---------------------------------------------------------------------------
// OSPF Packet Types
// ---------------------------------------------------------------------------

/// OSPF packet types per RFC 2328
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketType {
    Hello = 1,
    DatabaseDescription = 2,
    LinkStateRequest = 3,
    LinkStateUpdate = 4,
    LinkStateAck = 5,
}

impl PacketType {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Hello),
            2 => Some(Self::DatabaseDescription),
            3 => Some(Self::LinkStateRequest),
            4 => Some(Self::LinkStateUpdate),
            5 => Some(Self::LinkStateAck),
            _ => None,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Hello => "Hello",
            Self::DatabaseDescription => "DB Description",
            Self::LinkStateRequest => "LS Request",
            Self::LinkStateUpdate => "LS Update",
            Self::LinkStateAck => "LS Ack",
        }
    }
}

// ---------------------------------------------------------------------------
// Neighbor State Machine
// ---------------------------------------------------------------------------

/// OSPF neighbor states per RFC 2328 Section 10.1
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeighborState {
    Down,
    Init,
    TwoWay,
    ExStart,
    Exchange,
    Loading,
    Full,
}

impl NeighborState {
    fn name(&self) -> &'static str {
        match self {
            Self::Down => "Down",
            Self::Init => "Init",
            Self::TwoWay => "2-Way",
            Self::ExStart => "ExStart",
            Self::Exchange => "Exchange",
            Self::Loading => "Loading",
            Self::Full => "Full",
        }
    }
}

// ---------------------------------------------------------------------------
// Interface Types
// ---------------------------------------------------------------------------

/// OSPF interface network types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceType {
    Broadcast,
    PointToPoint,
    NBMA,
}

impl InterfaceType {
    fn name(&self) -> &'static str {
        match self {
            Self::Broadcast => "Broadcast",
            Self::PointToPoint => "Point-to-Point",
            Self::NBMA => "NBMA",
        }
    }
}

// ---------------------------------------------------------------------------
// Area Types
// ---------------------------------------------------------------------------

/// OSPF area types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaType {
    /// Normal area (can carry all LSA types)
    Normal,
    /// Stub area (no AS-external LSAs)
    Stub,
    /// Not-so-stubby area (can generate type-7 LSAs)
    NSSA,
    /// Backbone area (area 0.0.0.0)
    Backbone,
}

impl AreaType {
    fn name(&self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Stub => "Stub",
            Self::NSSA => "NSSA",
            Self::Backbone => "Backbone",
        }
    }
}

// ---------------------------------------------------------------------------
// LSA Types
// ---------------------------------------------------------------------------

/// Link State Advertisement types per RFC 2328
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LsaType {
    RouterLsa = 1,
    NetworkLsa = 2,
    SummaryLsaNetwork = 3,
    SummaryLsaAsbr = 4,
    AsExternalLsa = 5,
}

impl LsaType {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::RouterLsa),
            2 => Some(Self::NetworkLsa),
            3 => Some(Self::SummaryLsaNetwork),
            4 => Some(Self::SummaryLsaAsbr),
            5 => Some(Self::AsExternalLsa),
            _ => None,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::RouterLsa => "Router",
            Self::NetworkLsa => "Network",
            Self::SummaryLsaNetwork => "Summary (Network)",
            Self::SummaryLsaAsbr => "Summary (ASBR)",
            Self::AsExternalLsa => "AS-External",
        }
    }
}

// ---------------------------------------------------------------------------
// Data Structures
// ---------------------------------------------------------------------------

/// OSPF neighbor entry
#[derive(Debug, Clone)]
pub struct Neighbor {
    pub router_id: [u8; 4],
    pub ip: [u8; 4],
    pub state: NeighborState,
    pub priority: u8,
    pub dr: [u8; 4],
    pub bdr: [u8; 4],
    pub dead_timer: u32,
    pub last_hello: u64,
    pub iface_index: usize,
}

/// OSPF interface
#[derive(Debug, Clone)]
pub struct OspfInterface {
    pub name: String,
    pub ip: [u8; 4],
    pub mask: [u8; 4],
    pub area_id: [u8; 4],
    pub iface_type: InterfaceType,
    pub cost: u32,
    pub hello_interval: u32,
    pub dead_interval: u32,
    pub dr: [u8; 4],
    pub bdr: [u8; 4],
    pub priority: u8,
    pub enabled: bool,
}

/// Link State Advertisement header + data
#[derive(Debug, Clone)]
pub struct Lsa {
    pub lsa_type: LsaType,
    pub link_state_id: [u8; 4],
    pub advertising_router: [u8; 4],
    pub seq_number: u32,
    pub age: u32,
    pub checksum: u16,
    pub metric: u32,
    pub mask: [u8; 4],
    pub area_id: [u8; 4],
}

/// OSPF area
#[derive(Debug, Clone)]
pub struct Area {
    pub area_id: [u8; 4],
    pub area_type: AreaType,
    pub lsdb: Vec<Lsa>,
    pub spf_runs: u64,
}

impl Area {
    fn new(area_id: [u8; 4], area_type: AreaType) -> Self {
        Self {
            area_id,
            area_type,
            lsdb: Vec::new(),
            spf_runs: 0,
        }
    }
}

/// SPF computed route
#[derive(Debug, Clone)]
pub struct OspfRoute {
    pub destination: [u8; 4],
    pub mask: [u8; 4],
    pub next_hop: [u8; 4],
    pub cost: u32,
    pub area_id: [u8; 4],
    pub route_type: &'static str,
}

/// Dijkstra SPF node (used during computation)
struct SpfNode {
    router_id: [u8; 4],
    cost: u32,
    next_hop: [u8; 4],
    visited: bool,
}

/// Main OSPF instance state
struct OspfInstance {
    router_id: [u8; 4],
    interfaces: Vec<OspfInterface>,
    neighbors: Vec<Neighbor>,
    areas: Vec<Area>,
    routes: Vec<OspfRoute>,
    enabled: bool,
    tick_count: u64,
}

impl OspfInstance {
    const fn new() -> Self {
        Self {
            router_id: [0; 4],
            interfaces: Vec::new(),
            neighbors: Vec::new(),
            areas: Vec::new(),
            routes: Vec::new(),
            enabled: false,
            tick_count: 0,
        }
    }

    /// Set the router ID
    fn set_router_id(&mut self, id: [u8; 4]) {
        self.router_id = id;
    }

    /// Add or get an area, returns area index
    fn ensure_area(&mut self, area_id: [u8; 4], area_type: AreaType) -> usize {
        if let Some(idx) = self.areas.iter().position(|a| a.area_id == area_id) {
            return idx;
        }
        if self.areas.len() >= MAX_AREAS {
            return 0;
        }
        let real_type = if area_id == [0, 0, 0, 0] { AreaType::Backbone } else { area_type };
        self.areas.push(Area::new(area_id, real_type));
        self.areas.len() - 1
    }

    /// Add an interface to OSPF
    fn add_interface(&mut self, name: &str, ip: [u8; 4], mask: [u8; 4],
                     area_id: [u8; 4], cost: u32, iface_type: InterfaceType) -> bool {
        if self.interfaces.len() >= MAX_INTERFACES {
            return false;
        }
        self.ensure_area(area_id, AreaType::Normal);
        self.interfaces.push(OspfInterface {
            name: name.to_owned(),
            ip,
            mask,
            area_id,
            iface_type,
            cost,
            hello_interval: HELLO_INTERVAL,
            dead_interval: DEAD_INTERVAL,
            dr: [0; 4],
            bdr: [0; 4],
            priority: 1,
            enabled: true,
        });

        // Generate a Router LSA for this interface
        let area_idx = self.areas.iter().position(|a| a.area_id == area_id).unwrap_or(0);
        if area_idx < self.areas.len() && self.areas[area_idx].lsdb.len() < MAX_LSAS {
            self.areas[area_idx].lsdb.push(Lsa {
                lsa_type: LsaType::RouterLsa,
                link_state_id: self.router_id,
                advertising_router: self.router_id,
                seq_number: 1,
                age: 0,
                checksum: 0,
                metric: cost,
                mask,
                area_id,
            });
        }
        true
    }

    /// Process a received Hello packet from a neighbor
    fn process_hello(&mut self, from_ip: [u8; 4], router_id: [u8; 4],
                     priority: u8, dr: [u8; 4], bdr: [u8; 4], iface_idx: usize) {
        if let Some(n) = self.neighbors.iter_mut().find(|n| n.router_id == router_id) {
            // Existing neighbor: update
            n.state = match n.state {
                NeighborState::Down => NeighborState::Init,
                NeighborState::Init => NeighborState::TwoWay,
                _ => n.state,
            };
            n.dead_timer = DEAD_INTERVAL;
            n.last_hello = self.tick_count;
            n.dr = dr;
            n.bdr = bdr;
            STATS.hellos_received.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if self.neighbors.len() >= MAX_NEIGHBORS {
            return;
        }
        self.neighbors.push(Neighbor {
            router_id,
            ip: from_ip,
            state: NeighborState::Init,
            priority,
            dr,
            bdr,
            dead_timer: DEAD_INTERVAL,
            last_hello: self.tick_count,
            iface_index: iface_idx,
        });
        STATS.hellos_received.fetch_add(1, Ordering::Relaxed);
        STATS.adjacencies_formed.fetch_add(1, Ordering::Relaxed);
    }

    /// Advance a neighbor to Full state (after exchange completes)
    fn advance_neighbor(&mut self, router_id: [u8; 4]) {
        if let Some(n) = self.neighbors.iter_mut().find(|n| n.router_id == router_id) {
            n.state = match n.state {
                NeighborState::TwoWay => NeighborState::ExStart,
                NeighborState::ExStart => NeighborState::Exchange,
                NeighborState::Exchange => NeighborState::Loading,
                NeighborState::Loading => NeighborState::Full,
                other => other,
            };
        }
    }

    /// Run Dijkstra SPF on a specific area's LSDB
    fn run_spf(&mut self, area_idx: usize) {
        if area_idx >= self.areas.len() {
            return;
        }

        let area_id = self.areas[area_idx].area_id;
        let lsdb = &self.areas[area_idx].lsdb;

        // Build node list from Router LSAs
        let mut nodes: Vec<SpfNode> = Vec::new();
        for lsa in lsdb.iter() {
            if lsa.lsa_type != LsaType::RouterLsa {
                continue;
            }
            if nodes.len() >= MAX_SPF_NODES {
                break;
            }
            let cost = if lsa.advertising_router == self.router_id { 0 } else { INFINITY_COST };
            nodes.push(SpfNode {
                router_id: lsa.advertising_router,
                cost,
                next_hop: lsa.advertising_router,
                visited: false,
            });
        }

        // Dijkstra main loop
        loop {
            // Find unvisited node with minimum cost
            let mut min_cost = INFINITY_COST + 1;
            let mut min_idx: Option<usize> = None;
            for (i, node) in nodes.iter().enumerate() {
                if !node.visited && node.cost < min_cost {
                    min_cost = node.cost;
                    min_idx = Some(i);
                }
            }
            let current = match min_idx {
                Some(i) => i,
                None => break,
            };
            nodes[current].visited = true;

            let current_rid = nodes[current].router_id;
            let current_cost = nodes[current].cost;
            let current_nh = nodes[current].next_hop;

            // Relax neighbors: find LSAs that this router advertises links to
            for lsa in lsdb.iter() {
                if lsa.lsa_type != LsaType::RouterLsa {
                    continue;
                }
                if lsa.advertising_router != current_rid {
                    continue;
                }
                let new_cost = current_cost.saturating_add(lsa.metric);
                // Update the target node (link_state_id)
                for node in nodes.iter_mut() {
                    if node.router_id == lsa.link_state_id && !node.visited {
                        if new_cost < node.cost {
                            node.cost = new_cost;
                            node.next_hop = if current_rid == self.router_id {
                                lsa.link_state_id
                            } else {
                                current_nh
                            };
                        }
                    }
                }
            }
        }

        // Remove old routes for this area
        self.routes.retain(|r| r.area_id != area_id);

        // Install new routes from SPF results
        for node in &nodes {
            if node.router_id == self.router_id || node.cost >= INFINITY_COST {
                continue;
            }
            if self.routes.len() >= MAX_ROUTES {
                break;
            }
            // Find the next-hop IP from neighbor table
            let nh_ip = self.neighbors.iter()
                .find(|n| n.router_id == node.next_hop)
                .map(|n| n.ip)
                .unwrap_or(node.next_hop);

            self.routes.push(OspfRoute {
                destination: node.router_id,
                mask: [255, 255, 255, 0],
                next_hop: nh_ip,
                cost: node.cost,
                area_id,
                route_type: "intra-area",
            });
        }

        self.areas[area_idx].spf_runs += 1;
        STATS.spf_runs.fetch_add(1, Ordering::Relaxed);
    }

    /// Run SPF on all areas
    fn run_all_spf(&mut self) {
        let n = self.areas.len();
        for i in 0..n {
            self.run_spf(i);
        }
    }

    /// Age out dead neighbors
    fn age_neighbors(&mut self) {
        self.neighbors.retain(|n| {
            let age = self.tick_count.saturating_sub(n.last_hello);
            age < DEAD_INTERVAL as u64
        });
    }
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

struct OspfStats {
    hellos_sent: AtomicU64,
    hellos_received: AtomicU64,
    lsa_originated: AtomicU64,
    lsa_received: AtomicU64,
    spf_runs: AtomicU64,
    adjacencies_formed: AtomicU64,
}

impl OspfStats {
    const fn new() -> Self {
        Self {
            hellos_sent: AtomicU64::new(0),
            hellos_received: AtomicU64::new(0),
            lsa_originated: AtomicU64::new(0),
            lsa_received: AtomicU64::new(0),
            spf_runs: AtomicU64::new(0),
            adjacencies_formed: AtomicU64::new(0),
        }
    }
}

// ---------------------------------------------------------------------------
// Global State
// ---------------------------------------------------------------------------

static OSPF: Mutex<OspfInstance> = Mutex::new(OspfInstance::new());
static STATS: OspfStats = OspfStats::new();
static INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize OSPF subsystem
pub fn init() {
    let mut ospf = OSPF.lock();
    ospf.set_router_id([10, 0, 0, 1]);
    ospf.ensure_area([0, 0, 0, 0], AreaType::Backbone);
    ospf.enabled = true;
    INITIALIZED.store(true, Ordering::SeqCst);
}

/// Add an interface to OSPF
pub fn add_interface(name: &str, ip: [u8; 4], mask: [u8; 4],
                     area_id: [u8; 4], cost: u32) -> bool {
    OSPF.lock().add_interface(name, ip, mask, area_id, cost, InterfaceType::Broadcast)
}

/// Process an incoming Hello from a neighbor
pub fn process_hello(from_ip: [u8; 4], router_id: [u8; 4],
                     priority: u8, dr: [u8; 4], bdr: [u8; 4], iface_idx: usize) {
    OSPF.lock().process_hello(from_ip, router_id, priority, dr, bdr, iface_idx);
}

/// Advance neighbor state machine
pub fn advance_neighbor(router_id: [u8; 4]) {
    OSPF.lock().advance_neighbor(router_id);
}

/// Trigger SPF recalculation on all areas
pub fn run_spf() {
    OSPF.lock().run_all_spf();
}

/// List all OSPF neighbors
pub fn list_neighbors() -> String {
    let ospf = OSPF.lock();
    if ospf.neighbors.is_empty() {
        return "No OSPF neighbors\n".to_owned();
    }
    let mut out = String::new();
    out.push_str("Router ID        State      Priority  DR               BDR              Interface\n");
    out.push_str("---------------- ---------- --------- ---------------- ---------------- ---------\n");
    for n in &ospf.neighbors {
        let fmt_ip = |ip: [u8; 4]| format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
        let iface = ospf.interfaces.get(n.iface_index)
            .map(|i| i.name.as_str()).unwrap_or("?");
        out.push_str(&format!(
            "{:<16} {:<10} {:<9} {:<16} {:<16} {}\n",
            fmt_ip(n.router_id), n.state.name(), n.priority,
            fmt_ip(n.dr), fmt_ip(n.bdr), iface
        ));
    }
    out
}

/// Show the Link State Database
pub fn show_lsdb() -> String {
    let ospf = OSPF.lock();
    let mut out = String::new();
    for area in &ospf.areas {
        let fmt_ip = |ip: [u8; 4]| format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
        out.push_str(&format!("Area {} ({}):\n", fmt_ip(area.area_id), area.area_type.name()));
        if area.lsdb.is_empty() {
            out.push_str("  (empty)\n");
            continue;
        }
        out.push_str("  Type             LS ID            Adv Router       Seq#       Age  Metric\n");
        out.push_str("  ---------------- ---------------- ---------------- ---------- ---- ------\n");
        for lsa in &area.lsdb {
            out.push_str(&format!(
                "  {:<16} {:<16} {:<16} 0x{:08x} {:<4} {}\n",
                lsa.lsa_type.name(), fmt_ip(lsa.link_state_id),
                fmt_ip(lsa.advertising_router), lsa.seq_number,
                lsa.age, lsa.metric
            ));
        }
        out.push_str(&format!("  SPF runs: {}\n", area.spf_runs));
    }
    out
}

/// Show OSPF computed routes
pub fn show_routes() -> String {
    let ospf = OSPF.lock();
    if ospf.routes.is_empty() {
        return "No OSPF routes\n".to_owned();
    }
    let mut out = String::new();
    out.push_str("Destination      Mask             Next Hop         Cost  Area             Type\n");
    out.push_str("---------------- ---------------- ---------------- ----- ---------------- ----------\n");
    for r in &ospf.routes {
        let fmt_ip = |ip: [u8; 4]| format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
        out.push_str(&format!(
            "{:<16} {:<16} {:<16} {:<5} {:<16} {}\n",
            fmt_ip(r.destination), fmt_ip(r.mask), fmt_ip(r.next_hop),
            r.cost, fmt_ip(r.area_id), r.route_type
        ));
    }
    out
}

/// Show OSPF general info
pub fn ospf_info() -> String {
    let ospf = OSPF.lock();
    let fmt_ip = |ip: [u8; 4]| format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    let mut out = String::new();
    out.push_str("OSPF Information:\n");
    out.push_str(&format!("  Router ID:     {}\n", fmt_ip(ospf.router_id)));
    out.push_str(&format!("  Version:       OSPFv{}\n", OSPF_VERSION));
    out.push_str(&format!("  Enabled:       {}\n", ospf.enabled));
    out.push_str(&format!("  Areas:         {}\n", ospf.areas.len()));
    out.push_str(&format!("  Interfaces:    {}\n", ospf.interfaces.len()));
    out.push_str(&format!("  Neighbors:     {}\n", ospf.neighbors.len()));
    out.push_str(&format!("  Routes:        {}\n", ospf.routes.len()));
    out.push_str(&format!("  Hello interval: {} sec\n", HELLO_INTERVAL));
    out.push_str(&format!("  Dead interval:  {} sec\n", DEAD_INTERVAL));
    out.push_str(&format!("  Multicast:     {}\n", fmt_ip(ALL_SPF_ROUTERS)));
    out.push_str("  Areas:\n");
    for area in &ospf.areas {
        out.push_str(&format!("    {} ({}) - {} LSAs\n",
            fmt_ip(area.area_id), area.area_type.name(), area.lsdb.len()));
    }
    out.push_str("  Interfaces:\n");
    for iface in &ospf.interfaces {
        out.push_str(&format!("    {} ({}) area {} cost {} {}\n",
            iface.name, fmt_ip(iface.ip), fmt_ip(iface.area_id),
            iface.cost, iface.iface_type.name()));
    }
    out
}

/// Show OSPF statistics
pub fn ospf_stats() -> String {
    let mut out = String::new();
    out.push_str("OSPF Statistics:\n");
    out.push_str(&format!("  Hellos sent:         {}\n", STATS.hellos_sent.load(Ordering::Relaxed)));
    out.push_str(&format!("  Hellos received:     {}\n", STATS.hellos_received.load(Ordering::Relaxed)));
    out.push_str(&format!("  LSAs originated:     {}\n", STATS.lsa_originated.load(Ordering::Relaxed)));
    out.push_str(&format!("  LSAs received:       {}\n", STATS.lsa_received.load(Ordering::Relaxed)));
    out.push_str(&format!("  SPF runs:            {}\n", STATS.spf_runs.load(Ordering::Relaxed)));
    out.push_str(&format!("  Adjacencies formed:  {}\n", STATS.adjacencies_formed.load(Ordering::Relaxed)));
    out
}
