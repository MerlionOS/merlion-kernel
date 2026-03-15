/// Local DNS server for MerlionOS.
///
/// Authoritative DNS server listening on UDP port 53, responding to queries
/// for locally configured zones.  Supports A, AAAA, CNAME, MX, TXT, NS, and
/// SOA record types.  Ships with built-in records for `localhost` and
/// `merlion.local`.  Uses [`crate::netstack`] for UDP I/O.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::{netstack, serial_println};

const DNS_PORT: u16 = 53;
const DNS_HEADER_LEN: usize = 12;
const MAX_QUERY_LEN: usize = 512;
const RESPONSE_FLAGS: u16 = 0x8400;
const NXDOMAIN_FLAGS: u16 = 0x8403;
const CLASS_IN: u16 = 1;
const DEFAULT_TTL: u32 = 300;

/// DNS resource record types supported by the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum RecordType {
    A = 1, NS = 2, CNAME = 5, SOA = 6, MX = 15, TXT = 16, AAAA = 28,
}

impl RecordType {
    /// Convert a raw `u16` wire type to a known variant.
    fn from_u16(v: u16) -> Option<Self> {
        match v {
            1 => Some(Self::A), 2 => Some(Self::NS), 5 => Some(Self::CNAME),
            6 => Some(Self::SOA), 15 => Some(Self::MX), 16 => Some(Self::TXT),
            28 => Some(Self::AAAA), _ => None,
        }
    }
}

/// Payload carried by a DNS resource record.
#[derive(Debug, Clone)]
pub enum RecordData {
    /// 4-byte IPv4 address.
    A([u8; 4]),
    /// 16-byte IPv6 address.
    AAAA([u8; 16]),
    /// Canonical-name target.
    CNAME(String),
    /// Mail exchange: preference + server name.
    MX(u16, String),
    /// Arbitrary text.
    TXT(String),
    /// Name-server hostname.
    NS(String),
    /// Start-of-authority: mname, rname, serial, refresh, retry, expire, min.
    SOA(String, String, u32, u32, u32, u32, u32),
}

/// A single DNS resource record.
#[derive(Debug, Clone)]
pub struct DnsRecord {
    pub name: String,
    pub rtype: RecordType,
    pub ttl: u32,
    pub data: RecordData,
}

/// A DNS zone containing resource records.
#[derive(Debug, Clone)]
pub struct Zone {
    pub origin: String,
    pub records: Vec<DnsRecord>,
}

impl Zone {
    /// Create an empty zone.
    pub fn new(origin: &str) -> Self {
        Self { origin: String::from(origin), records: Vec::new() }
    }

    /// Add a record.
    pub fn add(&mut self, r: DnsRecord) { self.records.push(r); }

    /// Remove the first record matching `name` and `rtype`.
    pub fn remove(&mut self, name: &str, rtype: RecordType) -> bool {
        if let Some(i) = self.records.iter().position(|r| r.name == name && r.rtype == rtype) {
            self.records.remove(i);
            true
        } else { false }
    }

    /// Find all records matching `name` and `rtype`.
    fn lookup(&self, name: &str, rtype: RecordType) -> Vec<&DnsRecord> {
        self.records.iter().filter(|r| r.name == name && r.rtype == rtype).collect()
    }
}

/// A parsed incoming DNS query.
#[derive(Debug, Clone)]
pub struct DnsQuery {
    pub id: u16,
    pub name: String,
    pub qtype: u16,
    pub qclass: u16,
    pub src_ip: [u8; 4],
    pub src_port: u16,
}

/// Parse a raw DNS query packet into a [`DnsQuery`].
///
/// Returns `None` if the packet is too short, has zero questions, or the
/// name encoding is malformed.
pub fn parse_dns_query(data: &[u8], src_ip: [u8; 4], src_port: u16) -> Option<DnsQuery> {
    if data.len() < DNS_HEADER_LEN + 5 { return None; }
    let id = u16::from_be_bytes([data[0], data[1]]);
    if u16::from_be_bytes([data[4], data[5]]) == 0 { return None; }

    let mut pos = DNS_HEADER_LEN;
    let mut parts: Vec<&str> = Vec::new();
    loop {
        if pos >= data.len() { return None; }
        let len = data[pos] as usize;
        pos += 1;
        if len == 0 { break; }
        if pos + len > data.len() { return None; }
        parts.push(core::str::from_utf8(&data[pos..pos + len]).ok()?);
        pos += len;
    }
    if pos + 4 > data.len() { return None; }
    let qtype = u16::from_be_bytes([data[pos], data[pos + 1]]);
    let qclass = u16::from_be_bytes([data[pos + 2], data[pos + 3]]);

    let mut name = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 { name.push('.'); }
        name.push_str(p);
    }
    Some(DnsQuery { id, name, qtype, qclass, src_ip, src_port })
}

// ---- wire-format helpers --------------------------------------------------

/// Encode a domain name into DNS wire format (length-prefixed labels + 0).
fn encode_name(name: &str) -> Vec<u8> {
    let mut b = Vec::new();
    for label in name.split('.') { b.push(label.len() as u8); b.extend_from_slice(label.as_bytes()); }
    b.push(0); b
}

/// Encode RDATA, returning `(type_code, bytes)`.
fn encode_rdata(d: &RecordData) -> (u16, Vec<u8>) {
    match d {
        RecordData::A(ip) => (1, ip.to_vec()),
        RecordData::AAAA(ip) => (28, ip.to_vec()),
        RecordData::CNAME(n) => (5, encode_name(n)),
        RecordData::NS(n) => (2, encode_name(n)),
        RecordData::MX(pref, n) => {
            let mut r = pref.to_be_bytes().to_vec(); r.extend_from_slice(&encode_name(n)); (15, r)
        }
        RecordData::TXT(t) => {
            let b = t.as_bytes();
            let mut r = Vec::new();
            let mut o = 0;
            while o < b.len() { let c = core::cmp::min(255, b.len() - o); r.push(c as u8); r.extend_from_slice(&b[o..o+c]); o += c; }
            if b.is_empty() { r.push(0); }
            (16, r)
        }
        RecordData::SOA(mn, rn, ser, ref_, ret, exp, min) => {
            let mut r = encode_name(mn); r.extend_from_slice(&encode_name(rn));
            for v in &[*ser, *ref_, *ret, *exp, *min] { r.extend_from_slice(&v.to_be_bytes()); }
            (6, r)
        }
    }
}

/// Build a complete DNS response packet for the given query and matching
/// records.  An empty `records` slice produces an NXDOMAIN response.
pub fn build_dns_response(query: &DnsQuery, records: &[&DnsRecord]) -> Vec<u8> {
    let mut r = Vec::with_capacity(MAX_QUERY_LEN);
    r.extend_from_slice(&query.id.to_be_bytes());
    let f = if records.is_empty() { NXDOMAIN_FLAGS } else { RESPONSE_FLAGS };
    r.extend_from_slice(&f.to_be_bytes());
    r.extend_from_slice(&1u16.to_be_bytes());                 // QDCOUNT
    r.extend_from_slice(&(records.len() as u16).to_be_bytes()); // ANCOUNT
    r.extend_from_slice(&0u16.to_be_bytes());                 // NSCOUNT
    r.extend_from_slice(&0u16.to_be_bytes());                 // ARCOUNT
    let qn = encode_name(&query.name);
    r.extend_from_slice(&qn);
    r.extend_from_slice(&query.qtype.to_be_bytes());
    r.extend_from_slice(&query.qclass.to_be_bytes());
    for rec in records {
        r.extend_from_slice(&encode_name(&rec.name));
        let (rt, rd) = encode_rdata(&rec.data);
        r.extend_from_slice(&rt.to_be_bytes());
        r.extend_from_slice(&CLASS_IN.to_be_bytes());
        r.extend_from_slice(&rec.ttl.to_be_bytes());
        r.extend_from_slice(&(rd.len() as u16).to_be_bytes());
        r.extend_from_slice(&rd);
    }
    r
}

// ---- DnsServer ------------------------------------------------------------

/// The MerlionOS local DNS server with an in-memory zone database.
pub struct DnsServer {
    pub zones: Vec<Zone>,
}

/// Global server instance.
pub static SERVER: Mutex<Option<DnsServer>> = Mutex::new(None);

impl DnsServer {
    /// Create a new server pre-loaded with built-in zones.
    pub fn new() -> Self { let mut s = Self { zones: Vec::new() }; s.load_builtin(); s }

    /// Populate localhost and merlion.local zones.
    fn load_builtin(&mut self) {
        let mut lo = Zone::new("localhost");
        lo.add(DnsRecord { name: String::from("localhost"), rtype: RecordType::A,
            ttl: DEFAULT_TTL, data: RecordData::A([127, 0, 0, 1]) });
        lo.add(DnsRecord { name: String::from("localhost"), rtype: RecordType::AAAA,
            ttl: DEFAULT_TTL, data: RecordData::AAAA([0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1]) });
        self.zones.push(lo);

        let mut ml = Zone::new("merlion.local");
        ml.add(DnsRecord { name: String::from("merlion.local"), rtype: RecordType::SOA,
            ttl: DEFAULT_TTL, data: RecordData::SOA(
                String::from("ns.merlion.local"), String::from("admin.merlion.local"),
                2026031601, 3600, 900, 604800, 300) });
        ml.add(DnsRecord { name: String::from("merlion.local"), rtype: RecordType::NS,
            ttl: DEFAULT_TTL, data: RecordData::NS(String::from("ns.merlion.local")) });
        ml.add(DnsRecord { name: String::from("merlion.local"), rtype: RecordType::A,
            ttl: DEFAULT_TTL, data: RecordData::A([10, 0, 2, 15]) });
        ml.add(DnsRecord { name: String::from("ns.merlion.local"), rtype: RecordType::A,
            ttl: DEFAULT_TTL, data: RecordData::A([10, 0, 2, 15]) });
        ml.add(DnsRecord { name: String::from("merlion.local"), rtype: RecordType::MX,
            ttl: DEFAULT_TTL, data: RecordData::MX(10, String::from("mail.merlion.local")) });
        ml.add(DnsRecord { name: String::from("mail.merlion.local"), rtype: RecordType::A,
            ttl: DEFAULT_TTL, data: RecordData::A([10, 0, 2, 15]) });
        ml.add(DnsRecord { name: String::from("merlion.local"), rtype: RecordType::TXT,
            ttl: DEFAULT_TTL, data: RecordData::TXT(String::from("v=merlionos born-for-ai built-by-ai")) });
        self.zones.push(ml);
    }

    /// Add a record to the zone matching `zone_origin`, creating it if needed.
    pub fn add_record(&mut self, zone_origin: &str, record: DnsRecord) {
        if let Some(z) = self.zones.iter_mut().find(|z| z.origin == zone_origin) {
            z.add(record);
        } else {
            let mut z = Zone::new(zone_origin); z.add(record); self.zones.push(z);
        }
    }

    /// Remove the first record matching `name` and `rtype` from any zone.
    pub fn remove_record(&mut self, name: &str, rtype: RecordType) -> bool {
        self.zones.iter_mut().any(|z| z.remove(name, rtype))
    }

    /// Resolve a query against all loaded zones.
    ///
    /// Returns matching records from the first zone that has them.  Performs
    /// one level of CNAME chasing when the direct lookup yields no results.
    pub fn resolve_query(&self, query: &DnsQuery) -> Vec<&DnsRecord> {
        let rtype = match RecordType::from_u16(query.qtype) {
            Some(rt) => rt,
            None => return Vec::new(),
        };
        for zone in &self.zones {
            let hits = zone.lookup(&query.name, rtype);
            if !hits.is_empty() { return hits; }
        }
        if rtype != RecordType::CNAME {
            for zone in &self.zones {
                if let Some(cn) = zone.lookup(&query.name, RecordType::CNAME).first() {
                    if let RecordData::CNAME(ref target) = cn.data {
                        for z2 in &self.zones {
                            let h = z2.lookup(target, rtype);
                            if !h.is_empty() { return h; }
                        }
                    }
                }
            }
        }
        Vec::new()
    }

    /// Handle a single incoming DNS request: parse, resolve, respond via UDP.
    pub fn handle_request(&self, data: &[u8], src_ip: [u8; 4], src_port: u16) {
        if data.len() < DNS_HEADER_LEN || data.len() > MAX_QUERY_LEN { return; }
        let query = match parse_dns_query(data, src_ip, src_port) {
            Some(q) => q,
            None => return,
        };
        serial_println!("[dnsd] query id={:#06x} name={} type={} from {}.{}.{}.{}:{}",
            query.id, query.name, query.qtype,
            src_ip[0], src_ip[1], src_ip[2], src_ip[3], src_port);
        let records = self.resolve_query(&query);
        let response = build_dns_response(&query, &records);
        serial_println!("[dnsd] responding with {} answer(s), {} bytes",
            records.len(), response.len());
        netstack::send_udp(src_ip, DNS_PORT, src_port, &response);
    }
}

// ---- public init / dispatch -----------------------------------------------

/// Initialise the global DNS server with built-in zones.
pub fn init() {
    let mut srv = SERVER.lock();
    if srv.is_none() {
        *srv = Some(DnsServer::new());
        serial_println!("[dnsd] DNS server initialised on UDP port {}", DNS_PORT);
    }
}

/// Process an incoming UDP packet destined for port 53.
pub fn handle_packet(data: &[u8], src_ip: [u8; 4], src_port: u16) {
    let srv = SERVER.lock();
    if let Some(ref s) = *srv { s.handle_request(data, src_ip, src_port); }
}

/// Add a record to the global server at runtime.
pub fn add_record(zone_origin: &str, record: DnsRecord) {
    let mut srv = SERVER.lock();
    if let Some(ref mut s) = *srv { s.add_record(zone_origin, record); }
}

/// Remove a record from the global server at runtime.
pub fn remove_record(name: &str, rtype: RecordType) -> bool {
    let mut srv = SERVER.lock();
    if let Some(ref mut s) = *srv { s.remove_record(name, rtype) } else { false }
}
