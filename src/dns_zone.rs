/// DNS zone management and resolver cache for MerlionOS.
/// Extends the DNS server (dnsd.rs) with zone file management,
/// a resolver cache with TTL expiration, and DNS statistics.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// --- DNS Record Types ---

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecordType {
    A,      // IPv4 address
    AAAA,   // IPv6 address (stored as string)
    CNAME,  // Canonical name
    MX,     // Mail exchange
    TXT,    // Text record
    NS,     // Name server
    SOA,    // Start of authority
    PTR,    // Pointer (reverse DNS)
    SRV,    // Service locator
}

impl RecordType {
    pub fn as_str(&self) -> &'static str {
        match self {
            RecordType::A => "A",
            RecordType::AAAA => "AAAA",
            RecordType::CNAME => "CNAME",
            RecordType::MX => "MX",
            RecordType::TXT => "TXT",
            RecordType::NS => "NS",
            RecordType::SOA => "SOA",
            RecordType::PTR => "PTR",
            RecordType::SRV => "SRV",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() { // note: won't work in no_std without alloc
            _ => {
                // Manual case-insensitive matching
                match s {
                    "A" | "a" => Some(RecordType::A),
                    "AAAA" | "aaaa" => Some(RecordType::AAAA),
                    "CNAME" | "cname" => Some(RecordType::CNAME),
                    "MX" | "mx" => Some(RecordType::MX),
                    "TXT" | "txt" => Some(RecordType::TXT),
                    "NS" | "ns" => Some(RecordType::NS),
                    "SOA" | "soa" => Some(RecordType::SOA),
                    "PTR" | "ptr" => Some(RecordType::PTR),
                    "SRV" | "srv" => Some(RecordType::SRV),
                    _ => None,
                }
            }
        }
    }
}

// --- DNS Zone ---

#[derive(Debug, Clone)]
pub struct DnsRecord {
    pub name: String,
    pub record_type: RecordType,
    pub value: String,
    pub ttl: u32,        // Time to live in seconds
    pub priority: u16,   // For MX/SRV records
}

#[derive(Debug, Clone)]
pub struct DnsZone {
    pub domain: String,
    pub soa_primary: String,
    pub soa_admin: String,
    pub serial: u32,
    pub refresh: u32,
    pub retry: u32,
    pub expire: u32,
    pub minimum_ttl: u32,
    pub records: Vec<DnsRecord>,
}

const MAX_ZONES: usize = 16;
const MAX_RECORDS_PER_ZONE: usize = 64;

static ZONES: Mutex<Vec<DnsZone>> = Mutex::new(Vec::new());

/// Initialize with a default zone for merlionos.local
pub fn init() {
    let mut zones = ZONES.lock();

    let mut default_zone = DnsZone {
        domain: "merlionos.local".to_owned(),
        soa_primary: "ns1.merlionos.local".to_owned(),
        soa_admin: "admin.merlionos.local".to_owned(),
        serial: 2026031601,
        refresh: 3600,
        retry: 900,
        expire: 604800,
        minimum_ttl: 86400,
        records: Vec::new(),
    };

    // Default records
    default_zone.records.push(DnsRecord {
        name: "@".to_owned(), record_type: RecordType::A,
        value: "10.0.2.15".to_owned(), ttl: 3600, priority: 0,
    });
    default_zone.records.push(DnsRecord {
        name: "ns1".to_owned(), record_type: RecordType::NS,
        value: "ns1.merlionos.local".to_owned(), ttl: 3600, priority: 0,
    });
    default_zone.records.push(DnsRecord {
        name: "@".to_owned(), record_type: RecordType::TXT,
        value: "Born for AI. Built by AI.".to_owned(), ttl: 3600, priority: 0,
    });

    zones.push(default_zone);

    crate::serial_println!("[dns_zone] initialized with 1 zone");
    crate::klog_println!("[dns_zone] initialized");
}

/// Add a record to a zone.
pub fn add_record(domain: &str, name: &str, rtype: RecordType, value: &str, ttl: u32) -> Result<(), &'static str> {
    let mut zones = ZONES.lock();
    let zone = zones.iter_mut().find(|z| z.domain == domain)
        .ok_or("dns_zone: zone not found")?;
    if zone.records.len() >= MAX_RECORDS_PER_ZONE {
        return Err("dns_zone: max records reached");
    }
    zone.records.push(DnsRecord {
        name: name.to_owned(),
        record_type: rtype,
        value: value.to_owned(),
        ttl,
        priority: 0,
    });
    zone.serial += 1;
    Ok(())
}

/// Remove a record from a zone.
pub fn remove_record(domain: &str, name: &str, rtype: RecordType) -> Result<(), &'static str> {
    let mut zones = ZONES.lock();
    let zone = zones.iter_mut().find(|z| z.domain == domain)
        .ok_or("dns_zone: zone not found")?;
    let len_before = zone.records.len();
    zone.records.retain(|r| !(r.name == name && r.record_type == rtype));
    if zone.records.len() == len_before {
        return Err("dns_zone: record not found");
    }
    zone.serial += 1;
    Ok(())
}

/// Query records matching name and type within a zone.
pub fn query(domain: &str, name: &str, rtype: RecordType) -> Vec<DnsRecord> {
    let zones = ZONES.lock();
    if let Some(zone) = zones.iter().find(|z| z.domain == domain) {
        zone.records.iter()
            .filter(|r| (r.name == name || name == "*") && r.record_type == rtype)
            .cloned()
            .collect()
    } else {
        Vec::new()
    }
}

/// Create a new zone.
pub fn create_zone(domain: &str) -> Result<(), &'static str> {
    let mut zones = ZONES.lock();
    if zones.len() >= MAX_ZONES {
        return Err("dns_zone: max zones reached");
    }
    if zones.iter().any(|z| z.domain == domain) {
        return Err("dns_zone: zone already exists");
    }
    zones.push(DnsZone {
        domain: domain.to_owned(),
        soa_primary: format!("ns1.{}", domain),
        soa_admin: format!("admin.{}", domain),
        serial: 1,
        refresh: 3600, retry: 900, expire: 604800, minimum_ttl: 86400,
        records: Vec::new(),
    });
    Ok(())
}

/// List all zones.
pub fn list_zones() -> String {
    let zones = ZONES.lock();
    let mut out = format!("DNS zones ({}):\n", zones.len());
    for z in zones.iter() {
        out.push_str(&format!("  {} — {} records, serial {}\n",
            z.domain, z.records.len(), z.serial));
    }
    out
}

/// Show zone details.
pub fn zone_info(domain: &str) -> String {
    let zones = ZONES.lock();
    let zone = match zones.iter().find(|z| z.domain == domain) {
        Some(z) => z,
        None => return format!("Zone '{}' not found.\n", domain),
    };

    let mut out = format!("Zone: {}\n", zone.domain);
    out.push_str(&format!("  SOA: {} {}\n", zone.soa_primary, zone.soa_admin));
    out.push_str(&format!("  Serial: {}\n", zone.serial));
    out.push_str(&format!("  TTL: {}s\n\n", zone.minimum_ttl));
    out.push_str(&format!("{:<16} {:<8} {:>6} {}\n", "Name", "Type", "TTL", "Value"));

    for r in &zone.records {
        out.push_str(&format!("{:<16} {:<8} {:>6} {}\n",
            r.name, r.record_type.as_str(), r.ttl, r.value));
    }
    out
}

// --- Resolver Cache ---

const CACHE_SIZE: usize = 128;

#[derive(Debug, Clone)]
struct CacheEntry {
    name: String,
    record_type: RecordType,
    value: String,
    ttl: u32,
    inserted_tick: u64,
}

static CACHE: Mutex<Vec<CacheEntry>> = Mutex::new(Vec::new());
static CACHE_HITS: AtomicU64 = AtomicU64::new(0);
static CACHE_MISSES: AtomicU64 = AtomicU64::new(0);

/// Add an entry to the resolver cache.
pub fn cache_put(name: &str, rtype: RecordType, value: &str, ttl: u32) {
    let mut cache = CACHE.lock();
    // Remove existing entry for same name+type
    cache.retain(|e| !(e.name == name && e.record_type == rtype));
    if cache.len() >= CACHE_SIZE { cache.remove(0); }
    cache.push(CacheEntry {
        name: name.to_owned(),
        record_type: rtype,
        value: value.to_owned(),
        ttl,
        inserted_tick: crate::timer::ticks(),
    });
}

/// Look up a cached DNS entry. Returns None if not found or expired.
pub fn cache_get(name: &str, rtype: RecordType) -> Option<String> {
    let now = crate::timer::ticks();
    let cache = CACHE.lock();

    for entry in cache.iter() {
        if entry.name == name && entry.record_type == rtype {
            let elapsed_secs = (now - entry.inserted_tick) / 100;
            if elapsed_secs < entry.ttl as u64 {
                CACHE_HITS.fetch_add(1, Ordering::Relaxed);
                return Some(entry.value.clone());
            }
        }
    }

    CACHE_MISSES.fetch_add(1, Ordering::Relaxed);
    None
}

/// Flush expired entries from the cache.
pub fn cache_flush() {
    let now = crate::timer::ticks();
    let mut cache = CACHE.lock();
    cache.retain(|e| {
        let elapsed = (now - e.inserted_tick) / 100;
        elapsed < e.ttl as u64
    });
}

/// Clear the entire cache.
pub fn cache_clear() {
    CACHE.lock().clear();
}

/// Get cache statistics.
pub fn cache_stats() -> String {
    let size = CACHE.lock().len();
    let hits = CACHE_HITS.load(Ordering::Relaxed);
    let misses = CACHE_MISSES.load(Ordering::Relaxed);
    let total = hits + misses;
    let rate = if total > 0 { (hits * 100) / total } else { 0 };
    format!(
        "DNS cache: {} entries, {} hits, {} misses ({}% hit rate)",
        size, hits, misses, rate
    )
}
