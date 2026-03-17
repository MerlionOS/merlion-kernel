/// DHCP server for MerlionOS.
/// Assigns IP addresses to clients on the local network,
/// manages lease database, and provides network configuration.
///
/// Implements the server side of DHCP (RFC 2131/2132):
/// - DISCOVER -> OFFER (allocate IP from pool)
/// - REQUEST  -> ACK/NAK (confirm or deny)
/// - RELEASE  -> free the IP
/// - INFORM   -> provide config without allocating IP
///
/// Uses `spin::Mutex` for thread-safety in `no_std` kernel context.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

use crate::net::Ipv4Addr;
use crate::timer;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const BOOTP_REQUEST: u8 = 1;
const BOOTP_REPLY: u8 = 2;
const HW_TYPE_ETHERNET: u8 = 1;
const HW_ADDR_LEN: u8 = 6;
const DHCP_MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];
const BOOTP_HEADER_LEN: usize = 236;

// DHCP message types
const DHCP_DISCOVER: u8 = 1;
const DHCP_OFFER: u8 = 2;
const DHCP_REQUEST: u8 = 3;
const DHCP_DECLINE: u8 = 4;
const DHCP_ACK: u8 = 5;
const DHCP_NAK: u8 = 6;
const DHCP_RELEASE: u8 = 7;
const DHCP_INFORM: u8 = 8;

// DHCP option codes
const OPT_SUBNET_MASK: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DNS: u8 = 6;
const OPT_LEASE_TIME: u8 = 51;
const OPT_MSG_TYPE: u8 = 53;
const OPT_SERVER_ID: u8 = 54;
const OPT_REQUESTED_IP: u8 = 50;
const OPT_END: u8 = 255;

/// Default lease duration: 1 hour (3600 seconds).
const DEFAULT_LEASE_SECS: u32 = 3600;

/// Maximum number of leases the server can track.
const MAX_LEASES: usize = 256;

/// Maximum number of static reservations.
const MAX_RESERVATIONS: usize = 64;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A 6-byte MAC address used as client identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacAddr(pub [u8; 6]);

impl MacAddr {
    pub const ZERO: MacAddr = MacAddr([0; 6]);

    /// Check if this MAC is all zeros.
    pub fn is_zero(&self) -> bool {
        self.0 == [0; 6]
    }
}

impl core::fmt::Display for MacAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5])
    }
}

/// Extract MAC from DHCP packet chaddr field (offset 28, 6 bytes).
fn mac_from_packet(data: &[u8]) -> MacAddr {
    if data.len() < 34 {
        return MacAddr::ZERO;
    }
    MacAddr([data[28], data[29], data[30], data[31], data[32], data[33]])
}

/// A lease binding a MAC address to an IP address.
#[derive(Debug, Clone)]
pub struct Lease {
    pub mac: MacAddr,
    pub ip: Ipv4Addr,
    /// Lease duration in seconds.
    pub duration: u32,
    /// Tick at which the lease was granted.
    pub granted_tick: u64,
    /// Whether this lease is currently active.
    pub active: bool,
}

impl Lease {
    const fn empty() -> Self {
        Self {
            mac: MacAddr([0; 6]),
            ip: Ipv4Addr([0, 0, 0, 0]),
            duration: 0,
            granted_tick: 0,
            active: false,
        }
    }

    /// Check if this lease has expired based on current tick count.
    fn is_expired(&self, now_ticks: u64) -> bool {
        if !self.active {
            return true;
        }
        // Convert duration (seconds) to ticks (100 Hz)
        let expiry_ticks = self.granted_tick + (self.duration as u64) * 100;
        now_ticks >= expiry_ticks
    }

    /// Remaining seconds on this lease.
    fn remaining_secs(&self, now_ticks: u64) -> u32 {
        if !self.active {
            return 0;
        }
        let expiry_ticks = self.granted_tick + (self.duration as u64) * 100;
        if now_ticks >= expiry_ticks {
            return 0;
        }
        ((expiry_ticks - now_ticks) / 100) as u32
    }
}

/// A static MAC -> IP reservation.
#[derive(Debug, Clone)]
pub struct Reservation {
    pub mac: MacAddr,
    pub ip: Ipv4Addr,
}

/// Pool configuration for the DHCP server.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// First IP in the pool (host byte order for the last octet).
    pub start: Ipv4Addr,
    /// Last IP in the pool.
    pub end: Ipv4Addr,
    /// Subnet mask.
    pub subnet: Ipv4Addr,
    /// Default gateway.
    pub gateway: Ipv4Addr,
    /// DNS server.
    pub dns: Ipv4Addr,
    /// Server's own IP (used as server-identifier option).
    pub server_ip: Ipv4Addr,
    /// Lease duration in seconds.
    pub lease_duration: u32,
}

impl PoolConfig {
    const fn empty() -> Self {
        Self {
            start: Ipv4Addr([0, 0, 0, 0]),
            end: Ipv4Addr([0, 0, 0, 0]),
            subnet: Ipv4Addr([255, 255, 255, 0]),
            gateway: Ipv4Addr([0, 0, 0, 0]),
            dns: Ipv4Addr([0, 0, 0, 0]),
            server_ip: Ipv4Addr([0, 0, 0, 0]),
            lease_duration: DEFAULT_LEASE_SECS,
        }
    }
}

/// Internal state of the DHCP server.
struct DhcpdState {
    config: PoolConfig,
    leases: [Lease; MAX_LEASES],
    lease_count: usize,
    reservations: Vec<Reservation>,
}

impl DhcpdState {
    const fn new() -> Self {
        const EMPTY_LEASE: Lease = Lease::empty();
        Self {
            config: PoolConfig::empty(),
            leases: [EMPTY_LEASE; MAX_LEASES],
            lease_count: 0,
            reservations: Vec::new(),
        }
    }

    /// Find an existing lease for this MAC address.
    fn find_lease_by_mac(&self, mac: &MacAddr) -> Option<usize> {
        for i in 0..self.lease_count {
            if self.leases[i].mac == *mac && self.leases[i].active {
                return Some(i);
            }
        }
        None
    }

    /// Find a lease by IP address.
    fn find_lease_by_ip(&self, ip: &Ipv4Addr) -> Option<usize> {
        for i in 0..self.lease_count {
            if self.leases[i].ip == *ip && self.leases[i].active {
                return Some(i);
            }
        }
        None
    }

    /// Check if an IP is in the configured pool range.
    fn ip_in_pool(&self, ip: &Ipv4Addr) -> bool {
        // Compare last octets (simple /24 pool assumption)
        ip.0[0] == self.config.start.0[0]
            && ip.0[1] == self.config.start.0[1]
            && ip.0[2] == self.config.start.0[2]
            && ip.0[3] >= self.config.start.0[3]
            && ip.0[3] <= self.config.end.0[3]
    }

    /// Find a reservation for the given MAC.
    fn find_reservation(&self, mac: &MacAddr) -> Option<Ipv4Addr> {
        for r in &self.reservations {
            if r.mac == *mac {
                return Some(r.ip);
            }
        }
        None
    }

    /// Allocate an IP from the pool for the given MAC.
    fn allocate_ip(&mut self, mac: &MacAddr) -> Option<Ipv4Addr> {
        // Check reservation first
        if let Some(ip) = self.find_reservation(mac) {
            return Some(ip);
        }

        // Check if client already has a non-expired lease
        let now = timer::ticks();
        if let Some(idx) = self.find_lease_by_mac(mac) {
            if !self.leases[idx].is_expired(now) {
                return Some(self.leases[idx].ip);
            }
        }

        // Find a free IP in the pool
        let base = [self.config.start.0[0], self.config.start.0[1], self.config.start.0[2]];
        let start_last = self.config.start.0[3];
        let end_last = self.config.end.0[3];

        for last in start_last..=end_last {
            let candidate = Ipv4Addr([base[0], base[1], base[2], last]);
            let mut in_use = false;
            for i in 0..self.lease_count {
                if self.leases[i].ip == candidate
                    && self.leases[i].active
                    && !self.leases[i].is_expired(now)
                {
                    in_use = true;
                    break;
                }
            }
            if !in_use {
                return Some(candidate);
            }
        }

        None // Pool exhausted
    }

    /// Record a lease for mac -> ip.
    fn grant_lease(&mut self, mac: MacAddr, ip: Ipv4Addr) {
        let now = timer::ticks();

        // Update existing lease if present
        if let Some(idx) = self.find_lease_by_mac(&mac) {
            self.leases[idx].ip = ip;
            self.leases[idx].duration = self.config.lease_duration;
            self.leases[idx].granted_tick = now;
            self.leases[idx].active = true;
            return;
        }

        // Try to reuse an expired slot
        for i in 0..self.lease_count {
            if !self.leases[i].active || self.leases[i].is_expired(now) {
                self.leases[i] = Lease {
                    mac,
                    ip,
                    duration: self.config.lease_duration,
                    granted_tick: now,
                    active: true,
                };
                return;
            }
        }

        // Add new lease
        if self.lease_count < MAX_LEASES {
            self.leases[self.lease_count] = Lease {
                mac,
                ip,
                duration: self.config.lease_duration,
                granted_tick: now,
                active: true,
            };
            self.lease_count += 1;
        }
    }

    /// Release a lease for the given MAC.
    fn release_lease(&mut self, mac: &MacAddr) -> bool {
        if let Some(idx) = self.find_lease_by_mac(mac) {
            self.leases[idx].active = false;
            return true;
        }
        false
    }

    /// Count active (non-expired) leases.
    fn active_lease_count(&self) -> usize {
        let now = timer::ticks();
        let mut count = 0;
        for i in 0..self.lease_count {
            if self.leases[i].active && !self.leases[i].is_expired(now) {
                count += 1;
            }
        }
        count
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static STATE: Mutex<DhcpdState> = Mutex::new(DhcpdState::new());
static RUNNING: AtomicBool = AtomicBool::new(false);

// Statistics (atomic for lock-free reads)
static DISCOVERS: AtomicU64 = AtomicU64::new(0);
static OFFERS: AtomicU64 = AtomicU64::new(0);
static REQUESTS: AtomicU64 = AtomicU64::new(0);
static ACKS: AtomicU64 = AtomicU64::new(0);
static NAKS: AtomicU64 = AtomicU64::new(0);
static RELEASES: AtomicU64 = AtomicU64::new(0);
static INFORMS: AtomicU64 = AtomicU64::new(0);
static TOTAL_LEASES: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Packet parsing helpers
// ---------------------------------------------------------------------------

/// Read a big-endian u32 from a byte slice.
fn get_u32(data: &[u8], offset: usize) -> u32 {
    ((data[offset] as u32) << 24)
        | ((data[offset + 1] as u32) << 16)
        | ((data[offset + 2] as u32) << 8)
        | (data[offset + 3] as u32)
}

/// Write a big-endian u32 into a buffer.
fn put_u32(buf: &mut [u8], offset: usize, v: u32) {
    buf[offset] = (v >> 24) as u8;
    buf[offset + 1] = (v >> 16) as u8;
    buf[offset + 2] = (v >> 8) as u8;
    buf[offset + 3] = v as u8;
}

/// Extract DHCP message type from options.
fn extract_msg_type(data: &[u8]) -> Option<u8> {
    let cookie_off = BOOTP_HEADER_LEN;
    if data.len() < cookie_off + 4 {
        return None;
    }
    if data[cookie_off..cookie_off + 4] != DHCP_MAGIC_COOKIE {
        return None;
    }
    let mut i = cookie_off + 4;
    while i < data.len() {
        let opt = data[i];
        if opt == OPT_END { break; }
        if opt == 0 { i += 1; continue; }
        if i + 1 >= data.len() { break; }
        let len = data[i + 1] as usize;
        if i + 2 + len > data.len() { break; }
        if opt == OPT_MSG_TYPE && len >= 1 {
            return Some(data[i + 2]);
        }
        i += 2 + len;
    }
    None
}

/// Extract the requested IP from DHCP options.
fn extract_requested_ip(data: &[u8]) -> Option<Ipv4Addr> {
    let cookie_off = BOOTP_HEADER_LEN;
    if data.len() < cookie_off + 4 {
        return None;
    }
    let mut i = cookie_off + 4;
    while i < data.len() {
        let opt = data[i];
        if opt == OPT_END { break; }
        if opt == 0 { i += 1; continue; }
        if i + 1 >= data.len() { break; }
        let len = data[i + 1] as usize;
        if i + 2 + len > data.len() { break; }
        if opt == OPT_REQUESTED_IP && len >= 4 {
            let vs = i + 2;
            return Some(Ipv4Addr([data[vs], data[vs + 1], data[vs + 2], data[vs + 3]]));
        }
        i += 2 + len;
    }
    None
}

// ---------------------------------------------------------------------------
// Packet builder
// ---------------------------------------------------------------------------

/// Build a DHCP response packet.
fn build_response(
    request: &[u8],
    msg_type: u8,
    offered_ip: Ipv4Addr,
    config: &PoolConfig,
) -> Vec<u8> {
    let mut pkt = vec![0u8; BOOTP_HEADER_LEN];

    // BOOTP header
    pkt[0] = BOOTP_REPLY;
    pkt[1] = HW_TYPE_ETHERNET;
    pkt[2] = HW_ADDR_LEN;
    pkt[3] = 0; // hops

    // Copy xid from request (bytes 4..8)
    if request.len() >= 8 {
        pkt[4..8].copy_from_slice(&request[4..8]);
    }

    // yiaddr (your IP address) at offset 16
    pkt[16] = offered_ip.0[0];
    pkt[17] = offered_ip.0[1];
    pkt[18] = offered_ip.0[2];
    pkt[19] = offered_ip.0[3];

    // siaddr (server IP) at offset 20
    pkt[20] = config.server_ip.0[0];
    pkt[21] = config.server_ip.0[1];
    pkt[22] = config.server_ip.0[2];
    pkt[23] = config.server_ip.0[3];

    // Copy chaddr from request (offset 28, 16 bytes)
    if request.len() >= 44 {
        pkt[28..44].copy_from_slice(&request[28..44]);
    }

    // DHCP magic cookie
    pkt.extend_from_slice(&DHCP_MAGIC_COOKIE);

    // Option 53: DHCP Message Type
    pkt.push(OPT_MSG_TYPE);
    pkt.push(1);
    pkt.push(msg_type);

    // Option 54: Server Identifier
    pkt.push(OPT_SERVER_ID);
    pkt.push(4);
    pkt.extend_from_slice(&config.server_ip.0);

    // Option 51: Lease Time
    pkt.push(OPT_LEASE_TIME);
    pkt.push(4);
    let lt = config.lease_duration;
    pkt.push((lt >> 24) as u8);
    pkt.push((lt >> 16) as u8);
    pkt.push((lt >> 8) as u8);
    pkt.push(lt as u8);

    // Option 1: Subnet Mask
    pkt.push(OPT_SUBNET_MASK);
    pkt.push(4);
    pkt.extend_from_slice(&config.subnet.0);

    // Option 3: Router
    pkt.push(OPT_ROUTER);
    pkt.push(4);
    pkt.extend_from_slice(&config.gateway.0);

    // Option 6: DNS Server
    pkt.push(OPT_DNS);
    pkt.push(4);
    pkt.extend_from_slice(&config.dns.0);

    // End option
    pkt.push(OPT_END);

    // Pad to minimum 300 bytes
    if pkt.len() < 300 {
        pkt.resize(300, 0);
    }

    pkt
}

/// Build a NAK response.
fn build_nak(request: &[u8], config: &PoolConfig) -> Vec<u8> {
    let mut pkt = vec![0u8; BOOTP_HEADER_LEN];
    pkt[0] = BOOTP_REPLY;
    pkt[1] = HW_TYPE_ETHERNET;
    pkt[2] = HW_ADDR_LEN;

    if request.len() >= 8 {
        pkt[4..8].copy_from_slice(&request[4..8]);
    }
    if request.len() >= 44 {
        pkt[28..44].copy_from_slice(&request[28..44]);
    }

    pkt.extend_from_slice(&DHCP_MAGIC_COOKIE);
    pkt.push(OPT_MSG_TYPE);
    pkt.push(1);
    pkt.push(DHCP_NAK);
    pkt.push(OPT_SERVER_ID);
    pkt.push(4);
    pkt.extend_from_slice(&config.server_ip.0);
    pkt.push(OPT_END);

    if pkt.len() < 300 {
        pkt.resize(300, 0);
    }
    pkt
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the DHCP server subsystem.
pub fn init() {
    crate::serial_println!("[dhcpd] initialized");
}

/// Start the DHCP server with the given pool configuration.
///
/// `pool_start` / `pool_end` define the assignable IP range.
/// The server will use `gateway` as its own IP (server identifier).
pub fn start(
    pool_start: [u8; 4],
    pool_end: [u8; 4],
    subnet: [u8; 4],
    gateway: [u8; 4],
    dns: [u8; 4],
) {
    let mut state = STATE.lock();
    state.config = PoolConfig {
        start: Ipv4Addr(pool_start),
        end: Ipv4Addr(pool_end),
        subnet: Ipv4Addr(subnet),
        gateway: Ipv4Addr(gateway),
        dns: Ipv4Addr(dns),
        server_ip: Ipv4Addr(gateway),
        lease_duration: DEFAULT_LEASE_SECS,
    };
    drop(state);
    RUNNING.store(true, Ordering::SeqCst);
    crate::serial_println!(
        "[dhcpd] started: pool {}.{}.{}.{}-{}.{}.{}.{}",
        pool_start[0], pool_start[1], pool_start[2], pool_start[3],
        pool_end[0], pool_end[1], pool_end[2], pool_end[3],
    );
}

/// Stop the DHCP server.
pub fn stop() {
    RUNNING.store(false, Ordering::SeqCst);
    crate::serial_println!("[dhcpd] stopped");
}

/// Add a static MAC -> IP reservation.
pub fn add_reservation(mac: [u8; 6], ip: [u8; 4]) {
    let mut state = STATE.lock();
    if state.reservations.len() < MAX_RESERVATIONS {
        state.reservations.push(Reservation {
            mac: MacAddr(mac),
            ip: Ipv4Addr(ip),
        });
    }
}

/// Process an incoming DHCP packet and return a response (if any).
///
/// The caller is responsible for sending the response back via UDP.
/// Returns `None` if the server is stopped or the packet is invalid.
pub fn process_packet(data: &[u8]) -> Option<Vec<u8>> {
    if !RUNNING.load(Ordering::SeqCst) {
        return None;
    }
    if data.len() < BOOTP_HEADER_LEN + 4 {
        return None;
    }
    // Must be a BOOTP request
    if data[0] != BOOTP_REQUEST {
        return None;
    }

    let msg_type = extract_msg_type(data)?;
    let client_mac = mac_from_packet(data);

    match msg_type {
        DHCP_DISCOVER => {
            DISCOVERS.fetch_add(1, Ordering::Relaxed);
            handle_discover(data, &client_mac)
        }
        DHCP_REQUEST => {
            REQUESTS.fetch_add(1, Ordering::Relaxed);
            handle_request(data, &client_mac)
        }
        DHCP_RELEASE => {
            RELEASES.fetch_add(1, Ordering::Relaxed);
            handle_release(&client_mac);
            None // No response for RELEASE
        }
        DHCP_INFORM => {
            INFORMS.fetch_add(1, Ordering::Relaxed);
            handle_inform(data)
        }
        DHCP_DECLINE => {
            // Client is telling us the IP is in use; release it
            let mut state = STATE.lock();
            state.release_lease(&client_mac);
            None
        }
        _ => None,
    }
}

fn handle_discover(data: &[u8], mac: &MacAddr) -> Option<Vec<u8>> {
    let mut state = STATE.lock();
    let ip = state.allocate_ip(mac)?;
    let config = state.config.clone();
    drop(state);

    OFFERS.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[dhcpd] OFFER {} to {}", ip, mac);
    Some(build_response(data, DHCP_OFFER, ip, &config))
}

fn handle_request(data: &[u8], mac: &MacAddr) -> Option<Vec<u8>> {
    let requested_ip = extract_requested_ip(data).or_else(|| {
        // ciaddr (offset 12)
        if data.len() >= 16 {
            let ip = Ipv4Addr([data[12], data[13], data[14], data[15]]);
            if ip != Ipv4Addr::ZERO { Some(ip) } else { None }
        } else {
            None
        }
    })?;

    let mut state = STATE.lock();

    // Verify IP is in our pool
    if !state.ip_in_pool(&requested_ip) {
        let config = state.config.clone();
        drop(state);
        NAKS.fetch_add(1, Ordering::Relaxed);
        crate::serial_println!("[dhcpd] NAK {} to {} (not in pool)", requested_ip, mac);
        return Some(build_nak(data, &config));
    }

    // Check if someone else holds this IP
    if let Some(idx) = state.find_lease_by_ip(&requested_ip) {
        let now = timer::ticks();
        if state.leases[idx].mac != *mac && !state.leases[idx].is_expired(now) {
            let config = state.config.clone();
            drop(state);
            NAKS.fetch_add(1, Ordering::Relaxed);
            crate::serial_println!("[dhcpd] NAK {} to {} (IP in use)", requested_ip, mac);
            return Some(build_nak(data, &config));
        }
    }

    // Grant the lease
    state.grant_lease(*mac, requested_ip);
    let config = state.config.clone();
    drop(state);

    ACKS.fetch_add(1, Ordering::Relaxed);
    TOTAL_LEASES.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[dhcpd] ACK {} to {}", requested_ip, mac);
    Some(build_response(data, DHCP_ACK, requested_ip, &config))
}

fn handle_release(mac: &MacAddr) {
    let mut state = STATE.lock();
    if state.release_lease(mac) {
        crate::serial_println!("[dhcpd] RELEASE from {}", mac);
    }
}

fn handle_inform(data: &[u8]) -> Option<Vec<u8>> {
    let state = STATE.lock();
    let config = state.config.clone();
    drop(state);

    // Respond with network config but yiaddr = 0 (client keeps its IP)
    let ciaddr = if data.len() >= 16 {
        Ipv4Addr([data[12], data[13], data[14], data[15]])
    } else {
        Ipv4Addr::ZERO
    };
    Some(build_response(data, DHCP_ACK, ciaddr, &config))
}

// ---------------------------------------------------------------------------
// Query API
// ---------------------------------------------------------------------------

/// Return a list of all active leases as formatted strings.
pub fn list_leases() -> String {
    let state = STATE.lock();
    let now = timer::ticks();
    let mut out = String::from("MAC Address        IP Address       Remaining\n");
    out.push_str("------------------ ---------------- ---------\n");

    for i in 0..state.lease_count {
        let lease = &state.leases[i];
        if lease.active && !lease.is_expired(now) {
            let rem = lease.remaining_secs(now);
            out.push_str(&format!(
                "{} {:>16} {}s\n",
                lease.mac, lease.ip, rem
            ));
        }
    }

    let active = state.active_lease_count();
    out.push_str(&format!("\nActive leases: {}\n", active));
    out
}

/// Return DHCP server status information.
pub fn dhcpd_info() -> String {
    let running = RUNNING.load(Ordering::SeqCst);
    let state = STATE.lock();
    let active = state.active_lease_count();
    let config = &state.config;

    let mut out = String::from("=== MerlionOS DHCP Server ===\n");
    out.push_str(&format!("Status:      {}\n", if running { "running" } else { "stopped" }));
    out.push_str(&format!("Pool start:  {}\n", config.start));
    out.push_str(&format!("Pool end:    {}\n", config.end));
    out.push_str(&format!("Subnet:      {}\n", config.subnet));
    out.push_str(&format!("Gateway:     {}\n", config.gateway));
    out.push_str(&format!("DNS:         {}\n", config.dns));
    out.push_str(&format!("Server IP:   {}\n", config.server_ip));
    out.push_str(&format!("Lease time:  {}s\n", config.lease_duration));
    out.push_str(&format!("Active:      {} leases\n", active));
    out.push_str(&format!("Reservations: {}\n", state.reservations.len()));
    out
}

/// Return DHCP server statistics.
pub fn dhcpd_stats() -> String {
    let state = STATE.lock();
    let active = state.active_lease_count();
    drop(state);

    let mut out = String::from("=== DHCP Server Statistics ===\n");
    out.push_str(&format!("Discovers:    {}\n", DISCOVERS.load(Ordering::Relaxed)));
    out.push_str(&format!("Offers:       {}\n", OFFERS.load(Ordering::Relaxed)));
    out.push_str(&format!("Requests:     {}\n", REQUESTS.load(Ordering::Relaxed)));
    out.push_str(&format!("ACKs:         {}\n", ACKS.load(Ordering::Relaxed)));
    out.push_str(&format!("NAKs:         {}\n", NAKS.load(Ordering::Relaxed)));
    out.push_str(&format!("Releases:     {}\n", RELEASES.load(Ordering::Relaxed)));
    out.push_str(&format!("Informs:      {}\n", INFORMS.load(Ordering::Relaxed)));
    out.push_str(&format!("Total leases: {}\n", TOTAL_LEASES.load(Ordering::Relaxed)));
    out.push_str(&format!("Active now:   {}\n", active));
    out
}
