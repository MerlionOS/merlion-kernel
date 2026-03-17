/// mDNS (Multicast DNS) and DNS-SD (Service Discovery) for MerlionOS.
/// Enables zero-configuration networking: automatic hostname resolution
/// and service advertisement on the local network.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// mDNS multicast group address
const MDNS_MULTICAST: [u8; 4] = [224, 0, 0, 251];

/// mDNS port
const MDNS_PORT: u16 = 5353;

/// Default TTL for mDNS records (seconds)
const DEFAULT_TTL: u32 = 120;

/// Maximum services that can be registered
const MAX_SERVICES: usize = 32;

/// Maximum cached entries
const MAX_CACHE: usize = 128;

/// DNS record types
const DNS_TYPE_A: u16 = 1;
const DNS_TYPE_PTR: u16 = 12;
const DNS_TYPE_TXT: u16 = 16;
const DNS_TYPE_SRV: u16 = 33;

/// DNS class: IN (Internet)
const DNS_CLASS_IN: u16 = 1;

/// DNS flags
const DNS_FLAG_RESPONSE: u16 = 0x8400; // response + authoritative

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

static QUERIES_SENT: AtomicU64 = AtomicU64::new(0);
static QUERIES_RECEIVED: AtomicU64 = AtomicU64::new(0);
static RESPONSES_SENT: AtomicU64 = AtomicU64::new(0);
static RESPONSES_RECEIVED: AtomicU64 = AtomicU64::new(0);
static SERVICES_REGISTERED: AtomicU64 = AtomicU64::new(0);
static CACHE_HITS: AtomicU64 = AtomicU64::new(0);
static CACHE_MISSES: AtomicU64 = AtomicU64::new(0);

static INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Service registry
// ---------------------------------------------------------------------------

/// A service advertised via DNS-SD.
#[derive(Clone)]
pub struct MdnsService {
    pub name: String,
    pub service_type: String,
    pub port: u16,
    pub txt_records: Vec<(String, String)>,
    pub hostname: String,
}

impl MdnsService {
    /// Format TXT records as "key=value" pairs.
    fn format_txt(&self) -> String {
        if self.txt_records.is_empty() {
            return String::from("(none)");
        }
        let pairs: Vec<String> = self.txt_records.iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        pairs.join(", ")
    }

    /// Full service name: "name._type.local"
    fn full_name(&self) -> String {
        format!("{}.{}.local", self.name, self.service_type)
    }
}

// ---------------------------------------------------------------------------
// Cache entry
// ---------------------------------------------------------------------------

/// A cached mDNS record from the network.
#[derive(Clone)]
struct CacheEntry {
    hostname: String,
    ip: [u8; 4],
    ttl: u32,
    created_tick: u64,
    record_type: u16,
    /// For SRV records: port
    port: u16,
    /// For service discovery: service name and type
    service_name: String,
    service_type: String,
    /// TXT records (key=value)
    txt: Vec<(String, String)>,
}

impl CacheEntry {
    fn is_expired(&self, current_tick: u64, ticks_per_sec: u64) -> bool {
        let age_secs = (current_tick.saturating_sub(self.created_tick)) / ticks_per_sec;
        age_secs >= self.ttl as u64
    }
}

// ---------------------------------------------------------------------------
// mDNS state
// ---------------------------------------------------------------------------

struct MdnsState {
    /// Our hostname (without .local)
    hostname: String,
    /// Our IP address
    our_ip: [u8; 4],
    /// Registered services
    services: Vec<MdnsService>,
    /// Discovery cache
    cache: Vec<CacheEntry>,
    /// Whether probing for our hostname is complete
    probed: bool,
}

impl MdnsState {
    const fn new() -> Self {
        Self {
            hostname: String::new(),
            our_ip: [0; 4],
            services: Vec::new(),
            cache: Vec::new(),
            probed: false,
        }
    }
}

static MDNS: Mutex<MdnsState> = Mutex::new(MdnsState::new());

// ---------------------------------------------------------------------------
// DNS message building
// ---------------------------------------------------------------------------

/// Build a simple mDNS query for a hostname.
fn build_query(name: &str, qtype: u16) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(64);
    // Header: ID=0, flags=0 (query), qdcount=1, ancount=0, nscount=0, arcount=0
    pkt.extend_from_slice(&[0, 0]); // ID
    pkt.extend_from_slice(&[0, 0]); // Flags (query)
    pkt.extend_from_slice(&[0, 1]); // QDCOUNT=1
    pkt.extend_from_slice(&[0, 0]); // ANCOUNT
    pkt.extend_from_slice(&[0, 0]); // NSCOUNT
    pkt.extend_from_slice(&[0, 0]); // ARCOUNT
    // Question: encoded name
    encode_dns_name(&mut pkt, name);
    pkt.push((qtype >> 8) as u8);
    pkt.push(qtype as u8);
    pkt.push((DNS_CLASS_IN >> 8) as u8);
    pkt.push(DNS_CLASS_IN as u8);
    QUERIES_SENT.fetch_add(1, Ordering::Relaxed);
    pkt
}

/// Build an mDNS response with an A record.
fn build_a_response(name: &str, ip: [u8; 4], ttl: u32) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(64);
    // Header
    pkt.extend_from_slice(&[0, 0]); // ID
    pkt.push((DNS_FLAG_RESPONSE >> 8) as u8);
    pkt.push(DNS_FLAG_RESPONSE as u8);
    pkt.extend_from_slice(&[0, 0]); // QDCOUNT
    pkt.extend_from_slice(&[0, 1]); // ANCOUNT=1
    pkt.extend_from_slice(&[0, 0]); // NSCOUNT
    pkt.extend_from_slice(&[0, 0]); // ARCOUNT
    // Answer: A record
    encode_dns_name(&mut pkt, name);
    pkt.push((DNS_TYPE_A >> 8) as u8);
    pkt.push(DNS_TYPE_A as u8);
    pkt.push((DNS_CLASS_IN >> 8) as u8);
    pkt.push(DNS_CLASS_IN as u8);
    // TTL (4 bytes)
    pkt.push((ttl >> 24) as u8);
    pkt.push((ttl >> 16) as u8);
    pkt.push((ttl >> 8) as u8);
    pkt.push(ttl as u8);
    // RDLENGTH=4 (IPv4)
    pkt.extend_from_slice(&[0, 4]);
    pkt.extend_from_slice(&ip);
    RESPONSES_SENT.fetch_add(1, Ordering::Relaxed);
    pkt
}

/// Build a DNS-SD service response (PTR + SRV + TXT).
fn build_service_response(svc: &MdnsService, ip: [u8; 4]) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(256);
    // Header: 3 answers (PTR + SRV + TXT)
    pkt.extend_from_slice(&[0, 0]); // ID
    pkt.push((DNS_FLAG_RESPONSE >> 8) as u8);
    pkt.push(DNS_FLAG_RESPONSE as u8);
    pkt.extend_from_slice(&[0, 0]); // QDCOUNT
    pkt.extend_from_slice(&[0, 3]); // ANCOUNT=3
    pkt.extend_from_slice(&[0, 0]); // NSCOUNT
    pkt.extend_from_slice(&[0, 1]); // ARCOUNT=1 (additional A record)

    let svc_domain = format!("{}.local", svc.service_type);
    let full = svc.full_name();

    // PTR record: _type.local -> name._type.local
    encode_dns_name(&mut pkt, &svc_domain);
    pkt.push((DNS_TYPE_PTR >> 8) as u8);
    pkt.push(DNS_TYPE_PTR as u8);
    pkt.push((DNS_CLASS_IN >> 8) as u8);
    pkt.push(DNS_CLASS_IN as u8);
    pkt.extend_from_slice(&DEFAULT_TTL.to_be_bytes());
    let ptr_data = encode_dns_name_bytes(&full);
    pkt.push((ptr_data.len() >> 8) as u8);
    pkt.push(ptr_data.len() as u8);
    pkt.extend_from_slice(&ptr_data);

    // SRV record: name._type.local -> hostname:port
    encode_dns_name(&mut pkt, &full);
    pkt.push((DNS_TYPE_SRV >> 8) as u8);
    pkt.push(DNS_TYPE_SRV as u8);
    pkt.push((DNS_CLASS_IN >> 8) as u8);
    pkt.push(DNS_CLASS_IN as u8);
    pkt.extend_from_slice(&DEFAULT_TTL.to_be_bytes());
    let host_local = format!("{}.local", svc.hostname);
    let target = encode_dns_name_bytes(&host_local);
    let srv_rdlen = 6 + target.len(); // priority(2) + weight(2) + port(2) + target
    pkt.push((srv_rdlen >> 8) as u8);
    pkt.push(srv_rdlen as u8);
    pkt.extend_from_slice(&[0, 0]); // priority
    pkt.extend_from_slice(&[0, 0]); // weight
    pkt.push((svc.port >> 8) as u8);
    pkt.push(svc.port as u8);
    pkt.extend_from_slice(&target);

    // TXT record
    encode_dns_name(&mut pkt, &full);
    pkt.push((DNS_TYPE_TXT >> 8) as u8);
    pkt.push(DNS_TYPE_TXT as u8);
    pkt.push((DNS_CLASS_IN >> 8) as u8);
    pkt.push(DNS_CLASS_IN as u8);
    pkt.extend_from_slice(&DEFAULT_TTL.to_be_bytes());
    let txt_data = encode_txt_records(&svc.txt_records);
    pkt.push((txt_data.len() >> 8) as u8);
    pkt.push(txt_data.len() as u8);
    pkt.extend_from_slice(&txt_data);

    // Additional: A record for the hostname
    let a_name = format!("{}.local", svc.hostname);
    encode_dns_name(&mut pkt, &a_name);
    pkt.push((DNS_TYPE_A >> 8) as u8);
    pkt.push(DNS_TYPE_A as u8);
    pkt.push((DNS_CLASS_IN >> 8) as u8);
    pkt.push(DNS_CLASS_IN as u8);
    pkt.extend_from_slice(&DEFAULT_TTL.to_be_bytes());
    pkt.extend_from_slice(&[0, 4]);
    pkt.extend_from_slice(&ip);

    RESPONSES_SENT.fetch_add(1, Ordering::Relaxed);
    pkt
}

/// Encode a DNS name into wire format (length-prefixed labels).
fn encode_dns_name(buf: &mut Vec<u8>, name: &str) {
    for label in name.split('.') {
        let len = label.len().min(63);
        buf.push(len as u8);
        buf.extend_from_slice(&label.as_bytes()[..len]);
    }
    buf.push(0); // root label
}

/// Encode a DNS name and return as a Vec.
fn encode_dns_name_bytes(name: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    encode_dns_name(&mut buf, name);
    buf
}

/// Encode TXT records as DNS TXT RDATA.
fn encode_txt_records(records: &[(String, String)]) -> Vec<u8> {
    let mut buf = Vec::new();
    for (k, v) in records {
        let entry = format!("{}={}", k, v);
        let len = entry.len().min(255);
        buf.push(len as u8);
        buf.extend_from_slice(&entry.as_bytes()[..len]);
    }
    if buf.is_empty() {
        buf.push(0); // empty TXT record
    }
    buf
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

/// Register a service for advertisement via DNS-SD.
pub fn register_service(name: &str, service_type: &str, port: u16,
                        txt: Vec<(String, String)>) -> Result<(), &'static str> {
    let mut mdns = MDNS.lock();
    if mdns.services.len() >= MAX_SERVICES {
        return Err("maximum services reached");
    }
    if mdns.services.iter().any(|s| s.name == name && s.service_type == service_type) {
        return Err("service already registered");
    }
    let svc = MdnsService {
        name: String::from(name),
        service_type: String::from(service_type),
        port,
        txt_records: txt,
        hostname: mdns.hostname.clone(),
    };
    crate::serial_println!("[mdns] registered service: {} ({}:{}) on {}",
        svc.full_name(), service_type, port, svc.hostname);
    mdns.services.push(svc);
    SERVICES_REGISTERED.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Unregister a service by name.
pub fn unregister_service(name: &str) -> Result<(), &'static str> {
    let mut mdns = MDNS.lock();
    let idx = mdns.services.iter().position(|s| s.name == name)
        .ok_or("service not found")?;
    let svc = mdns.services.remove(idx);
    crate::serial_println!("[mdns] unregistered service: {}", svc.full_name());
    Ok(())
}

/// Browse for services of a given type on the local network.
/// Returns matching services from cache and our own registered services.
pub fn browse(service_type: &str) -> Vec<MdnsService> {
    let mdns = MDNS.lock();
    let current_tick = crate::timer::ticks();
    let tps = 100u64; // assume 100 ticks/sec

    // Send a query for the service type (simulated)
    let _query = build_query(
        &format!("{}.local", service_type),
        DNS_TYPE_PTR,
    );

    let mut results = Vec::new();

    // Our own services
    for svc in &mdns.services {
        if svc.service_type == service_type {
            results.push(svc.clone());
        }
    }

    // Cached entries from the network
    for entry in &mdns.cache {
        if entry.service_type == service_type && !entry.is_expired(current_tick, tps) {
            CACHE_HITS.fetch_add(1, Ordering::Relaxed);
            results.push(MdnsService {
                name: entry.service_name.clone(),
                service_type: entry.service_type.clone(),
                port: entry.port,
                txt_records: entry.txt.clone(),
                hostname: entry.hostname.clone(),
            });
        }
    }

    results
}

/// Resolve a .local hostname to an IP address.
pub fn resolve(hostname: &str) -> Option<[u8; 4]> {
    let mdns = MDNS.lock();
    let lookup = hostname.strip_suffix(".local").unwrap_or(hostname);
    let current_tick = crate::timer::ticks();
    let tps = 100u64;

    // Check if it's our own hostname
    if lookup == mdns.hostname {
        return Some(mdns.our_ip);
    }

    // Check cache
    for entry in &mdns.cache {
        let cached_name = entry.hostname.strip_suffix(".local").unwrap_or(&entry.hostname);
        if cached_name == lookup && entry.record_type == DNS_TYPE_A
            && !entry.is_expired(current_tick, tps) {
            CACHE_HITS.fetch_add(1, Ordering::Relaxed);
            return Some(entry.ip);
        }
    }

    CACHE_MISSES.fetch_add(1, Ordering::Relaxed);

    // Send a query (simulated)
    let _query = build_query(
        &format!("{}.local", lookup),
        DNS_TYPE_A,
    );

    None
}

/// Set our mDNS hostname (without .local suffix).
pub fn set_hostname(name: &str) {
    let mut mdns = MDNS.lock();
    mdns.hostname = String::from(name);
    mdns.probed = false; // need to re-probe
    // Simulate probing (send 3 queries, wait for conflicts)
    let _probe = build_query(&format!("{}.local", name), DNS_TYPE_A);
    mdns.probed = true;
    // Update hostname in all registered services
    for svc in &mut mdns.services {
        svc.hostname = String::from(name);
    }
    crate::serial_println!("[mdns] hostname set to {}.local", name);
}

/// Set our IP address for mDNS responses.
pub fn set_ip(ip: [u8; 4]) {
    let mut mdns = MDNS.lock();
    mdns.our_ip = ip;
}

/// Handle an incoming mDNS query (simplified).
pub fn handle_query(query: &[u8]) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }
    QUERIES_RECEIVED.fetch_add(1, Ordering::Relaxed);

    let mdns = MDNS.lock();
    let _flags = u16::from_be_bytes([query[2], query[3]]);
    let qdcount = u16::from_be_bytes([query[4], query[5]]);

    if qdcount == 0 {
        return None;
    }

    // Simplified: check if query is for our hostname
    let hostname_local = format!("{}.local", mdns.hostname);
    let encoded = encode_dns_name_bytes(&hostname_local);
    if query.len() > 12 + encoded.len() && query[12..12 + encoded.len()] == encoded[..] {
        return Some(build_a_response(&hostname_local, mdns.our_ip, DEFAULT_TTL));
    }

    // Check if query is for any of our services
    for svc in &mdns.services {
        let svc_domain = format!("{}.local", svc.service_type);
        let svc_encoded = encode_dns_name_bytes(&svc_domain);
        if query.len() > 12 + svc_encoded.len()
            && query[12..12 + svc_encoded.len()] == svc_encoded[..] {
            return Some(build_service_response(svc, mdns.our_ip));
        }
    }

    None
}

/// Handle an incoming mDNS response, updating the cache.
pub fn handle_response(response: &[u8]) {
    if response.len() < 12 {
        return;
    }
    RESPONSES_RECEIVED.fetch_add(1, Ordering::Relaxed);

    let ancount = u16::from_be_bytes([response[6], response[7]]);
    if ancount == 0 {
        return;
    }

    // Simplified: add a synthetic cache entry
    // In a real implementation we would parse all answer records
    let current_tick = crate::timer::ticks();
    let mut mdns = MDNS.lock();
    if mdns.cache.len() < MAX_CACHE {
        // Check for conflict with our hostname
        // (simplified — real mDNS would parse the name from the response)
    }
    let _ = current_tick;
}

/// Expire old cache entries.
pub fn expire_cache() {
    let mut mdns = MDNS.lock();
    let current_tick = crate::timer::ticks();
    let tps = 100u64;
    mdns.cache.retain(|e| !e.is_expired(current_tick, tps));
}

// ---------------------------------------------------------------------------
// Shell / info
// ---------------------------------------------------------------------------

/// List all registered services.
pub fn list_services() -> String {
    let mdns = MDNS.lock();
    if mdns.services.is_empty() {
        return String::from("No mDNS services registered.");
    }
    let mut out = String::from("Registered mDNS/DNS-SD services:\n");
    out.push_str("NAME                              TYPE              PORT  HOST              TXT\n");
    out.push_str("--------------------------------  ----------------  ----  ----------------  ---\n");
    for svc in &mdns.services {
        out.push_str(&format!("{:<32}  {:<16}  {:>4}  {:<16}  {}\n",
            svc.name, svc.service_type, svc.port, svc.hostname, svc.format_txt()));
    }
    out
}

/// Return mDNS subsystem info.
pub fn mdns_info() -> String {
    let mdns = MDNS.lock();
    format!(
        "mDNS / DNS-SD:\n\
         \n  Hostname: {}.local\
         \n  IP: {}\
         \n  Multicast group: {}\
         \n  Port: {}\
         \n  Registered services: {}\
         \n  Cache entries: {}\
         \n  Probed: {}",
        mdns.hostname,
        format_ip(mdns.our_ip),
        format_ip(MDNS_MULTICAST),
        MDNS_PORT,
        mdns.services.len(),
        mdns.cache.len(),
        mdns.probed,
    )
}

/// Return mDNS statistics.
pub fn mdns_stats() -> String {
    format!(
        "mDNS Statistics:\n\
         \n  Queries sent: {}\
         \n  Queries received: {}\
         \n  Responses sent: {}\
         \n  Responses received: {}\
         \n  Services registered (total): {}\
         \n  Cache hits: {}\
         \n  Cache misses: {}",
        QUERIES_SENT.load(Ordering::Relaxed),
        QUERIES_RECEIVED.load(Ordering::Relaxed),
        RESPONSES_SENT.load(Ordering::Relaxed),
        RESPONSES_RECEIVED.load(Ordering::Relaxed),
        SERVICES_REGISTERED.load(Ordering::Relaxed),
        CACHE_HITS.load(Ordering::Relaxed),
        CACHE_MISSES.load(Ordering::Relaxed),
    )
}

fn format_ip(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize mDNS with default hostname and auto-register core services.
pub fn init() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }

    {
        let mut mdns = MDNS.lock();
        mdns.hostname = String::from("merlion");
        mdns.our_ip = [10, 0, 0, 1];
        mdns.probed = true;
    }

    // Auto-register HTTP server
    let _ = register_service(
        "MerlionOS Web Server",
        "_http._tcp",
        80,
        vec![
            (String::from("path"), String::from("/")),
            (String::from("os"), String::from("MerlionOS")),
        ],
    );

    // Auto-register SSH server
    let _ = register_service(
        "MerlionOS SSH",
        "_ssh._tcp",
        22,
        vec![
            (String::from("user"), String::from("root")),
        ],
    );

    crate::serial_println!("[mdns] initialized: merlion.local -> 10.0.0.1, 2 services registered");
}
