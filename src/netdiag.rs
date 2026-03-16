/// Network diagnostics toolkit for MerlionOS.
/// Provides traceroute, netcat, port scanner, bandwidth test,
/// packet capture, and network health monitoring.
///
/// All timing uses integer microseconds (no floating point).
/// Thread-safe via `spin::Mutex`; suitable for `#![no_std]` kernel use.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const MAX_HOPS: u8 = 30;
const MAX_SCAN_PORTS: u16 = 1024;
const CAPTURE_RING_SIZE: usize = 512;
const MAX_ARP_ENTRIES: usize = 256;
const MAX_SOCKET_ENTRIES: usize = 128;
const LATENCY_SAMPLE_COUNT: usize = 256;
const MTU_CEILING: u16 = 1500;
const MTU_FLOOR: u16 = 68;
const HEALTH_INTERVAL_US: u64 = 10_000_000;

pub type Ipv4Addr = [u8; 4];
pub type MacAddr = [u8; 6];

fn fmt_ip(ip: Ipv4Addr) -> String { format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]) }
fn fmt_mac(m: MacAddr) -> String {
    format!("{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", m[0], m[1], m[2], m[3], m[4], m[5])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proto { Tcp, Udp, Icmp, Any }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortState { Open, Closed, Filtered }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketState { Listen, SynSent, SynReceived, Established, FinWait1, FinWait2,
    CloseWait, Closing, LastAck, TimeWait, Closed }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NcMode { Client, Server }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsRecordType { A, Aaaa, Cname, Mx, Ns, Ptr, Txt, Soa }

// ── 1. Traceroute ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TracerouteHop { pub ttl: u8, pub ip: Option<Ipv4Addr>, pub rtt_us: Option<u64> }
#[derive(Debug, Clone)]
pub struct TracerouteResult { pub target: Ipv4Addr, pub hops: Vec<TracerouteHop>, pub reached: bool }

pub fn traceroute(target: Ipv4Addr) -> TracerouteResult {
    let (gateway, base_rtt) = { let s = STATE.lock(); (s.gateway, s.simulated_rtt_us) };
    let mut hops = Vec::new();
    let mut reached = false;
    for ttl in 1..=MAX_HOPS {
        let (ip, rtt) = if ttl == 1 {
            (gateway, base_rtt)
        } else {
            ([10, 0, ttl, 1], base_rtt + (ttl as u64) * 500)
        };
        if ttl > 1 && (ip == target || ttl >= 5) {
            hops.push(TracerouteHop { ttl, ip: Some(target), rtt_us: Some(rtt) });
            reached = true;
            break;
        }
        hops.push(TracerouteHop { ttl, ip: Some(ip), rtt_us: Some(rtt) });
    }
    STATS.probes_sent.fetch_add(hops.len() as u64, Ordering::Relaxed);
    TracerouteResult { target, hops, reached }
}

pub fn format_traceroute(r: &TracerouteResult) -> String {
    let mut out = format!("traceroute to {}, {} hops max\n", fmt_ip(r.target), MAX_HOPS);
    for h in &r.hops {
        let ip = h.ip.map_or("*".into(), fmt_ip);
        let rtt = h.rtt_us.map_or("*".into(), |us| format!("{}.{} ms", us / 1000, (us % 1000) / 100));
        out.push_str(&format!(" {:>2}  {:<16}  {}\n", h.ttl, ip, rtt));
    }
    out.push_str(if r.reached { "-- destination reached --\n" } else { "-- destination not reached --\n" });
    out
}

// ── 2. Port Scanner ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PortScanEntry { pub port: u16, pub state: PortState, pub service: Option<&'static str> }
#[derive(Debug, Clone)]
pub struct PortScanResult { pub target: Ipv4Addr, pub start_port: u16, pub end_port: u16, pub entries: Vec<PortScanEntry> }

fn well_known_service(port: u16) -> Option<&'static str> {
    match port {
        21 => Some("ftp"), 22 => Some("ssh"), 23 => Some("telnet"), 25 => Some("smtp"),
        53 => Some("dns"), 80 => Some("http"), 110 => Some("pop3"), 143 => Some("imap"),
        443 => Some("https"), 993 => Some("imaps"), 3306 => Some("mysql"),
        5432 => Some("postgresql"), 6379 => Some("redis"), 8080 => Some("http-alt"), _ => None,
    }
}

pub fn port_scan(target: Ipv4Addr, start: u16, end: u16) -> PortScanResult {
    let end = if end.saturating_sub(start) > MAX_SCAN_PORTS { start + MAX_SCAN_PORTS } else { end };
    let entries: Vec<PortScanEntry> = (start..=end).map(|port| {
        let state = if well_known_service(port).is_some() { PortState::Open }
            else if port % 97 == 0 { PortState::Filtered } else { PortState::Closed };
        PortScanEntry { port, state, service: well_known_service(port) }
    }).collect();
    STATS.probes_sent.fetch_add(entries.len() as u64, Ordering::Relaxed);
    PortScanResult { target, start_port: start, end_port: end, entries }
}

pub fn format_port_scan(r: &PortScanResult) -> String {
    let mut out = format!("PORT SCAN {} ports {}-{}\nPORT   STATE     SERVICE\n", fmt_ip(r.target), r.start_port, r.end_port);
    for e in &r.entries {
        if e.state == PortState::Closed { continue; }
        let st = match e.state { PortState::Open => "open     ", PortState::Filtered => "filtered ", _ => "closed   " };
        out.push_str(&format!("{:<6} {} {}\n", e.port, st, e.service.unwrap_or("-")));
    }
    out
}

// ── 3. Netcat (nc) ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NcSession {
    pub mode: NcMode, pub proto: Proto, pub remote_ip: Ipv4Addr, pub port: u16,
    pub bytes_sent: u64, pub bytes_received: u64, pub connected: bool,
}

pub fn nc_connect(mode: NcMode, proto: Proto, remote_ip: Ipv4Addr, port: u16) -> NcSession {
    NcSession { mode, proto, remote_ip, port, bytes_sent: 0, bytes_received: 0, connected: true }
}

pub fn nc_send(session: &mut NcSession, data: &[u8]) -> usize {
    if !session.connected { return 0; }
    session.bytes_sent += data.len() as u64;
    STATS.bytes_sent.fetch_add(data.len() as u64, Ordering::Relaxed);
    data.len()
}

pub fn nc_recv(session: &mut NcSession, buf: &mut [u8]) -> usize {
    if !session.connected { return 0; }
    let len = buf.len().min(64);
    for (i, b) in buf.iter_mut().enumerate().take(len) { *b = (i & 0xFF) as u8; }
    session.bytes_received += len as u64;
    STATS.bytes_received.fetch_add(len as u64, Ordering::Relaxed);
    len
}

pub fn nc_close(session: &mut NcSession) { session.connected = false; }

pub fn format_nc_session(s: &NcSession) -> String {
    let m = match s.mode { NcMode::Client => "client", NcMode::Server => "server" };
    let p = match s.proto { Proto::Tcp => "TCP", Proto::Udp => "UDP", _ => "???" };
    let st = if s.connected { "CONNECTED" } else { "CLOSED" };
    format!("nc {} {} {}:{} sent={} recv={} {}\n", m, p, fmt_ip(s.remote_ip), s.port, s.bytes_sent, s.bytes_received, st)
}

// ── 4. Bandwidth Test ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BandwidthResult { pub bytes_transferred: u64, pub elapsed_us: u64, pub throughput_kbps: u64 }

pub fn bandwidth_test(_target: Ipv4Addr, num_bytes: u64) -> BandwidthResult {
    let link_kbps = STATE.lock().simulated_link_kbps;
    let elapsed_us = if link_kbps > 0 { num_bytes * 1000 / link_kbps } else { 1 };
    let throughput_kbps = if elapsed_us > 0 { num_bytes * 1_000_000 / (elapsed_us * 1024) } else { 0 };
    STATS.bytes_sent.fetch_add(num_bytes, Ordering::Relaxed);
    BandwidthResult { bytes_transferred: num_bytes, elapsed_us, throughput_kbps }
}

pub fn format_bandwidth(r: &BandwidthResult) -> String {
    format!("Bandwidth: {} bytes in {} ms = {} KB/s\n", r.bytes_transferred, r.elapsed_us / 1000, r.throughput_kbps)
}

// ── 5. Packet Capture ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CapturedPacket {
    pub timestamp_us: u64, pub proto: Proto,
    pub src_ip: Ipv4Addr, pub dst_ip: Ipv4Addr,
    pub src_port: u16, pub dst_port: u16,
    pub length: u16, pub payload_head: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct CaptureFilter {
    pub proto: Option<Proto>, pub src_ip: Option<Ipv4Addr>,
    pub dst_ip: Option<Ipv4Addr>, pub port: Option<u16>,
}

impl CaptureFilter {
    pub fn any() -> Self { Self { proto: None, src_ip: None, dst_ip: None, port: None } }
    pub fn matches(&self, p: &CapturedPacket) -> bool {
        if let Some(pr) = self.proto { if pr != Proto::Any && pr != p.proto { return false; } }
        if self.src_ip.is_some_and(|ip| ip != p.src_ip) { return false; }
        if self.dst_ip.is_some_and(|ip| ip != p.dst_ip) { return false; }
        if let Some(port) = self.port { if p.src_port != port && p.dst_port != port { return false; } }
        true
    }
}

struct CaptureRing { buf: Vec<CapturedPacket>, filter: CaptureFilter }
impl CaptureRing {
    fn new() -> Self { Self { buf: Vec::new(), filter: CaptureFilter::any() } }
    fn push(&mut self, pkt: CapturedPacket) {
        if !self.filter.matches(&pkt) { return; }
        if self.buf.len() >= CAPTURE_RING_SIZE { self.buf.remove(0); }
        self.buf.push(pkt);
    }
    fn drain(&mut self) -> Vec<CapturedPacket> { core::mem::replace(&mut self.buf, Vec::new()) }
}

static CAPTURE_ACTIVE: AtomicBool = AtomicBool::new(false);
static CAPTURE_RING: Mutex<Option<CaptureRing>> = Mutex::new(None);

pub fn capture_start(filter: CaptureFilter) {
    let mut cr = CaptureRing::new();
    cr.filter = filter;
    *CAPTURE_RING.lock() = Some(cr);
    CAPTURE_ACTIVE.store(true, Ordering::SeqCst);
}

pub fn capture_stop() -> Vec<CapturedPacket> {
    CAPTURE_ACTIVE.store(false, Ordering::SeqCst);
    CAPTURE_RING.lock().as_mut().map_or(Vec::new(), |cr| cr.drain())
}

/// Called from the network stack to feed a packet into the capture engine.
pub fn capture_ingest(pkt: CapturedPacket) {
    if !CAPTURE_ACTIVE.load(Ordering::Relaxed) { return; }
    if let Some(cr) = CAPTURE_RING.lock().as_mut() { cr.push(pkt); }
}

pub fn format_capture(packets: &[CapturedPacket]) -> String {
    let mut out = format!("{} packets captured\nTIMESTAMP     PROTO SRC              SPORT DST              DPORT LEN\n", packets.len());
    for p in packets {
        let pr = match p.proto { Proto::Tcp => "TCP ", Proto::Udp => "UDP ", Proto::Icmp => "ICMP", Proto::Any => "ANY " };
        out.push_str(&format!("{:>12}  {} {:<16} {:>5} {:<16} {:>5} {}\n",
            p.timestamp_us, pr, fmt_ip(p.src_ip), p.src_port, fmt_ip(p.dst_ip), p.dst_port, p.length));
    }
    out
}

// ── 6. DNS Lookup ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DnsRecord { pub name: String, pub rtype: DnsRecordType, pub value: String, pub ttl: u32 }
#[derive(Debug, Clone)]
pub struct DnsResult { pub query: String, pub records: Vec<DnsRecord>, pub elapsed_us: u64 }

pub fn dns_lookup(name: &str) -> DnsResult {
    let (records, elapsed_us) = match name {
        "localhost" => (vec![DnsRecord { name: "localhost".into(), rtype: DnsRecordType::A, value: "127.0.0.1".into(), ttl: 86400 }], 50),
        "merlionos.local" => (vec![
            DnsRecord { name: "merlionos.local".into(), rtype: DnsRecordType::A, value: "10.0.2.15".into(), ttl: 3600 },
            DnsRecord { name: "merlionos.local".into(), rtype: DnsRecordType::Mx, value: "mail.merlionos.local".into(), ttl: 3600 },
        ], 1200),
        _ => (vec![DnsRecord { name: String::from(name), rtype: DnsRecordType::A, value: "93.184.216.34".into(), ttl: 300 }], 25000),
    };
    STATS.probes_sent.fetch_add(1, Ordering::Relaxed);
    DnsResult { query: String::from(name), records, elapsed_us }
}

pub fn format_dns(r: &DnsResult) -> String {
    let mut out = format!("DNS lookup: {} ({} us)\nNAME                 TYPE   VALUE                     TTL\n", r.query, r.elapsed_us);
    for rec in &r.records {
        let t = match rec.rtype { DnsRecordType::A => "A", DnsRecordType::Aaaa => "AAAA", DnsRecordType::Cname => "CNAME",
            DnsRecordType::Mx => "MX", DnsRecordType::Ns => "NS", DnsRecordType::Ptr => "PTR",
            DnsRecordType::Txt => "TXT", DnsRecordType::Soa => "SOA" };
        out.push_str(&format!("{:<20} {:<6} {:<25} {}\n", rec.name, t, rec.value, rec.ttl));
    }
    out
}

// ── 7. HTTP Probe ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HttpProbeResult {
    pub url: String, pub status_code: u16, pub status_text: String,
    pub headers: Vec<(String, String)>, pub body_length: u32, pub elapsed_us: u64,
}

pub fn http_probe(url: &str) -> HttpProbeResult {
    STATS.probes_sent.fetch_add(1, Ordering::Relaxed);
    HttpProbeResult {
        url: String::from(url), status_code: 200, status_text: "OK".into(),
        headers: vec![("Content-Type".into(), "text/html".into()), ("Server".into(), "MerlionOS/1.0".into())],
        body_length: 1024, elapsed_us: 45000,
    }
}

pub fn format_http_probe(r: &HttpProbeResult) -> String {
    let mut out = format!("HTTP GET {} => {} {} ({} us)\n", r.url, r.status_code, r.status_text, r.elapsed_us);
    for (k, v) in &r.headers { out.push_str(&format!("  {}: {}\n", k, v)); }
    out.push_str(&format!("  Body: {} bytes\n", r.body_length));
    out
}

// ── 8. Network Health Monitor ────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HealthReport {
    pub gateway_reachable: bool, pub gateway_latency_us: u64,
    pub dns_ok: bool, pub dns_latency_us: u64,
    pub checks_performed: u64, pub failures: u64, pub uptime_us: u64,
}

pub fn health_check() -> HealthReport {
    let (gw, rtt, checks, failures) = {
        let s = STATE.lock();
        (s.gateway, s.simulated_rtt_us, s.health_checks_done, s.health_failures)
    };
    let reachable = gw != [0, 0, 0, 0];
    let dns = dns_lookup("merlionos.local");
    let mut st = STATE.lock();
    st.health_checks_done += 1;
    if !reachable { st.health_failures += 1; }
    drop(st);
    STATS.probes_sent.fetch_add(1, Ordering::Relaxed);
    HealthReport {
        gateway_reachable: reachable, gateway_latency_us: rtt,
        dns_ok: !dns.records.is_empty(), dns_latency_us: dns.elapsed_us,
        checks_performed: checks + 1, failures, uptime_us: checks * HEALTH_INTERVAL_US,
    }
}

pub fn format_health(r: &HealthReport) -> String {
    format!("=== Network Health ===\nGateway: {} ({} us)\nDNS: {} ({} us)\nChecks: {} (failures: {})\nUptime: {} s\n",
        if r.gateway_reachable { "OK" } else { "DOWN" }, r.gateway_latency_us,
        if r.dns_ok { "OK" } else { "FAIL" }, r.dns_latency_us,
        r.checks_performed, r.failures, r.uptime_us / 1_000_000)
}

// ── 9. Connection Quality ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ConnectionQuality {
    pub probes_sent: u64, pub probes_received: u64,
    pub loss_permille: u32, pub jitter_us: u64,
    pub p50_us: u64, pub p90_us: u64, pub p99_us: u64,
    pub min_us: u64, pub max_us: u64,
}

struct LatencySamples { samples: Vec<u64> }
impl LatencySamples {
    fn new() -> Self { Self { samples: Vec::new() } }
    fn push(&mut self, val: u64) {
        if self.samples.len() >= LATENCY_SAMPLE_COUNT { self.samples.remove(0); }
        self.samples.push(val);
    }
    fn percentile(sorted: &[u64], pct: u32) -> u64 {
        if sorted.is_empty() { return 0; }
        sorted[((sorted.len() as u64 * pct as u64 / 100) as usize).min(sorted.len() - 1)]
    }
    fn compute(&self, sent: u64, received: u64) -> ConnectionQuality {
        let mut sorted = self.samples.clone();
        // Insertion sort (fine for <= 256 samples)
        for i in 1..sorted.len() {
            let key = sorted[i];
            let mut j = i;
            while j > 0 && sorted[j - 1] > key { sorted[j] = sorted[j - 1]; j -= 1; }
            sorted[j] = key;
        }
        let jitter = if self.samples.len() > 1 {
            let mut d: u64 = 0;
            for i in 1..self.samples.len() {
                let (a, b) = (self.samples[i - 1], self.samples[i]);
                d += if a > b { a - b } else { b - a };
            }
            d / (self.samples.len() as u64 - 1)
        } else { 0 };
        let loss = if sent > 0 { (sent.saturating_sub(received) * 1000 / sent) as u32 } else { 0 };
        ConnectionQuality {
            probes_sent: sent, probes_received: received, loss_permille: loss, jitter_us: jitter,
            p50_us: Self::percentile(&sorted, 50), p90_us: Self::percentile(&sorted, 90),
            p99_us: Self::percentile(&sorted, 99),
            min_us: sorted.first().copied().unwrap_or(0), max_us: sorted.last().copied().unwrap_or(0),
        }
    }
}

static LATENCY_RING: Mutex<Option<LatencySamples>> = Mutex::new(None);

/// Record a latency sample (called from the network stack).
pub fn record_latency(us: u64) {
    if let Some(ring) = LATENCY_RING.lock().as_mut() { ring.push(us); }
}

pub fn connection_quality() -> ConnectionQuality {
    let (sent, recv) = (STATS.probes_sent.load(Ordering::Relaxed), STATS.probes_received.load(Ordering::Relaxed));
    LATENCY_RING.lock().as_ref().map_or(
        ConnectionQuality { probes_sent: sent, probes_received: recv, loss_permille: 0,
            jitter_us: 0, p50_us: 0, p90_us: 0, p99_us: 0, min_us: 0, max_us: 0 },
        |ring| ring.compute(sent, recv))
}

pub fn format_quality(q: &ConnectionQuality) -> String {
    format!("=== Connection Quality ===\nProbes: {} sent, {} recv\nLoss: {}.{}%\nJitter: {} us\nLatency: min={} p50={} p90={} p99={} max={} (us)\n",
        q.probes_sent, q.probes_received, q.loss_permille / 10, q.loss_permille % 10,
        q.jitter_us, q.min_us, q.p50_us, q.p90_us, q.p99_us, q.max_us)
}

// ── 10. Network Topology (ARP Sweep) ─────────────────────────────────

#[derive(Debug, Clone)]
pub struct ArpEntry { pub ip: Ipv4Addr, pub mac: MacAddr, pub hostname: Option<String>, pub rtt_us: u64 }

pub fn arp_sweep(subnet: [u8; 3]) -> Vec<ArpEntry> {
    let hosts: &[(u8, MacAddr, Option<&str>)] = &[
        (1,   [0xAA, 0xBB, 0xCC, 0x00, 0x00, 0x01], Some("gateway")),
        (2,   [0xAA, 0xBB, 0xCC, 0x00, 0x00, 0x02], Some("dns-server")),
        (15,  [0x52, 0x54, 0x00, 0x12, 0x34, 0x56], Some("merlionos")),
        (100, [0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01], None),
        (101, [0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x02], None),
    ];
    let entries: Vec<ArpEntry> = hosts.iter().take(MAX_ARP_ENTRIES).map(|&(h, mac, name)| {
        ArpEntry { ip: [subnet[0], subnet[1], subnet[2], h], mac, hostname: name.map(String::from), rtt_us: 200 + (h as u64) * 10 }
    }).collect();
    STATS.probes_sent.fetch_add(254, Ordering::Relaxed);
    entries
}

pub fn format_arp_table(entries: &[ArpEntry]) -> String {
    let mut out = format!("{} hosts discovered\nIP               MAC                HOSTNAME         RTT\n", entries.len());
    for e in entries {
        out.push_str(&format!("{:<16} {} {:<16} {} us\n", fmt_ip(e.ip), fmt_mac(e.mac), e.hostname.as_deref().unwrap_or("-"), e.rtt_us));
    }
    out
}

// ── 11. MTU Discovery ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MtuResult { pub target: Ipv4Addr, pub path_mtu: u16, pub probes_sent: u16 }

pub fn mtu_discover(target: Ipv4Addr) -> MtuResult {
    let (mut low, mut high, mut probes) = (MTU_FLOOR, MTU_CEILING, 0u16);
    let sim_mtu: u16 = 1400;
    while low < high {
        let mid = low + (high - low + 1) / 2;
        probes += 1;
        if mid <= sim_mtu { low = mid; } else { high = mid - 1; }
    }
    STATS.probes_sent.fetch_add(probes as u64, Ordering::Relaxed);
    MtuResult { target, path_mtu: low, probes_sent: probes }
}

pub fn format_mtu(r: &MtuResult) -> String {
    format!("MTU to {}: {} bytes ({} probes)\n", fmt_ip(r.target), r.path_mtu, r.probes_sent)
}

// ── 12. Socket Statistics ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SocketInfo {
    pub proto: Proto, pub state: SocketState,
    pub local_ip: Ipv4Addr, pub local_port: u16,
    pub remote_ip: Ipv4Addr, pub remote_port: u16,
    pub recv_queue: u32, pub send_queue: u32, pub pid: u32,
}

static SOCKET_TABLE: Mutex<Option<Vec<SocketInfo>>> = Mutex::new(None);

pub fn socket_register(info: SocketInfo) {
    if let Some(t) = SOCKET_TABLE.lock().as_mut() { if t.len() < MAX_SOCKET_ENTRIES { t.push(info); } }
}
pub fn socket_unregister(local_port: u16) {
    if let Some(t) = SOCKET_TABLE.lock().as_mut() { t.retain(|s| s.local_port != local_port); }
}
pub fn socket_list() -> Vec<SocketInfo> { SOCKET_TABLE.lock().as_ref().map_or(Vec::new(), |t| t.clone()) }

pub fn format_sockets(sockets: &[SocketInfo]) -> String {
    let mut out = format!("{} sockets\nPROTO STATE        LOCAL            LPORT REMOTE           RPORT RECV SEND PID\n", sockets.len());
    for s in sockets {
        let p = match s.proto { Proto::Tcp => "TCP  ", Proto::Udp => "UDP  ", Proto::Icmp => "ICMP ", Proto::Any => "ANY  " };
        let st = match s.state {
            SocketState::Listen => "LISTEN      ", SocketState::SynSent => "SYN-SENT    ",
            SocketState::SynReceived => "SYN-RECV    ", SocketState::Established => "ESTABLISHED ",
            SocketState::FinWait1 => "FIN-WAIT-1  ", SocketState::FinWait2 => "FIN-WAIT-2  ",
            SocketState::CloseWait => "CLOSE-WAIT  ", SocketState::Closing => "CLOSING     ",
            SocketState::LastAck => "LAST-ACK    ", SocketState::TimeWait => "TIME-WAIT   ",
            SocketState::Closed => "CLOSED      ",
        };
        out.push_str(&format!("{} {} {:<16} {:>5} {:<16} {:>5} {:>4} {:>4} {}\n",
            p, st, fmt_ip(s.local_ip), s.local_port, fmt_ip(s.remote_ip), s.remote_port,
            s.recv_queue, s.send_queue, s.pid));
    }
    out
}

// ── Global State & Statistics ────────────────────────────────────────

pub struct DiagStats {
    pub probes_sent: AtomicU64, pub probes_received: AtomicU64,
    pub bytes_sent: AtomicU64, pub bytes_received: AtomicU64,
}
impl DiagStats {
    const fn new() -> Self {
        Self { probes_sent: AtomicU64::new(0), probes_received: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0), bytes_received: AtomicU64::new(0) }
    }
    pub fn snapshot(&self) -> (u64, u64, u64, u64) {
        (self.probes_sent.load(Ordering::Relaxed), self.probes_received.load(Ordering::Relaxed),
         self.bytes_sent.load(Ordering::Relaxed), self.bytes_received.load(Ordering::Relaxed))
    }
}

pub static STATS: DiagStats = DiagStats::new();

struct NetDiagState {
    gateway: Ipv4Addr, local_ip: Ipv4Addr,
    simulated_rtt_us: u64, simulated_link_kbps: u64,
    health_checks_done: u64, health_failures: u64, initialised: bool,
}
impl NetDiagState {
    const fn new() -> Self {
        Self { gateway: [0;4], local_ip: [0;4], simulated_rtt_us: 0, simulated_link_kbps: 0,
            health_checks_done: 0, health_failures: 0, initialised: false }
    }
}
static STATE: Mutex<NetDiagState> = Mutex::new(NetDiagState::new());

// ── 13. Public API ───────────────────────────────────────────────────

/// Initialise the network diagnostics subsystem.
pub fn init() {
    let mut s = STATE.lock();
    s.gateway = [10, 0, 2, 2];
    s.local_ip = [10, 0, 2, 15];
    s.simulated_rtt_us = 1500;
    s.simulated_link_kbps = 100_000;
    s.initialised = true;
    drop(s);
    *CAPTURE_RING.lock() = Some(CaptureRing::new());
    *LATENCY_RING.lock() = Some(LatencySamples::new());
    *SOCKET_TABLE.lock() = Some(Vec::new());
    // Seed latency ring with initial samples
    if let Some(ring) = LATENCY_RING.lock().as_mut() {
        for i in 0..16u64 { ring.push(1500 + i * 100); }
    }
}

/// Return a summary of the diagnostics subsystem.
pub fn netdiag_info() -> String {
    let s = STATE.lock();
    if !s.initialised { return "(netdiag not initialised)\n".into(); }
    let (gw, lip, rtt, link) = (s.gateway, s.local_ip, s.simulated_rtt_us, s.simulated_link_kbps);
    drop(s);
    let (ps, pr, bs, br) = STATS.snapshot();
    let nsock = socket_list().len();
    let cap = CAPTURE_ACTIVE.load(Ordering::Relaxed);
    let mut out = String::new();
    out.push_str("=== MerlionOS Network Diagnostics ===\n");
    out.push_str(&format!("Local IP:    {}\nGateway:     {}\n", fmt_ip(lip), fmt_ip(gw)));
    out.push_str(&format!("Base RTT:    {} us\nLink speed:  {} KB/s\n", rtt, link));
    out.push_str(&format!("Probes:      {} sent, {} received\n", ps, pr));
    out.push_str(&format!("Traffic:     {} bytes sent, {} bytes received\n", bs, br));
    out.push_str(&format!("Sockets:     {}\nCapture:     {}\n", nsock, if cap { "active" } else { "inactive" }));
    out.push_str("Tools: traceroute, port_scan, nc, bandwidth, capture, dns,\n");
    out.push_str("       http_probe, health, quality, arp, mtu, ss\n");
    out
}

// ── 14. Shell-friendly wrappers ──────────────────────────────────────

fn parse_ip(s: &str) -> Ipv4Addr {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() == 4 {
        [
            parts[0].parse().unwrap_or(0),
            parts[1].parse().unwrap_or(0),
            parts[2].parse().unwrap_or(0),
            parts[3].parse().unwrap_or(0),
        ]
    } else {
        [0, 0, 0, 0]
    }
}

/// Shell wrapper: `traceroute <ip>`
pub fn traceroute_cmd(ip_str: &str) -> String {
    let target = parse_ip(ip_str);
    format_traceroute(&traceroute(target))
}

/// Shell wrapper: `portscan <ip> <start> <end>`
pub fn port_scan_cmd(args: &str) -> String {
    let parts: Vec<&str> = args.split_whitespace().collect();
    if parts.len() < 3 {
        return String::from("Usage: portscan <ip> <start> <end>");
    }
    let target = parse_ip(parts[0]);
    let start = parts[1].parse::<u16>().unwrap_or(1);
    let end = parts[2].parse::<u16>().unwrap_or(1024);
    format_port_scan(&port_scan(target, start, end))
}

/// Shell wrapper: `dns <name>` — returns formatted string.
pub fn dns_lookup_cmd(name: &str) -> String {
    format_dns(&dns_lookup(name))
}

/// Shell wrapper: capture status.
pub fn capture_status() -> String {
    let active = CAPTURE_ACTIVE.load(Ordering::Relaxed);
    let count = CAPTURE_RING.lock().as_ref().map_or(0, |cr| cr.buf.len());
    format!("Packet capture: {} ({} packets buffered)", if active { "active" } else { "inactive" }, count)
}

/// Shell wrapper: health check returning formatted string.
pub fn health_check_cmd() -> String {
    format_health(&health_check())
}

// ── 15. ss command ───────────────────────────────────────────────────

/// ss command — display socket statistics with flag parsing.
/// Flags: -t (TCP), -u (UDP), -l (listening), -n (numeric), -p (process)
pub fn ss_command(flags: &str) -> String {
    let show_tcp = flags.contains('t') || (!flags.contains('u') && !flags.contains('t'));
    let show_udp = flags.contains('u') || (!flags.contains('u') && !flags.contains('t'));
    let listen_only = flags.contains('l');
    let _numeric = flags.contains('n');
    let _show_pid = flags.contains('p');

    let mut out = String::from("State      Recv-Q Send-Q  Local Address:Port       Peer Address:Port\n");

    if show_tcp {
        let sockets = socket_list();
        for sock in &sockets {
            if sock.proto != Proto::Tcp && (flags.contains('t') || flags.contains('u')) {
                continue;
            }
            let state_str = match sock.state {
                SocketState::Listen => "LISTEN",
                SocketState::SynSent => "SYN-SENT",
                SocketState::SynReceived => "SYN-RECV",
                SocketState::Established => "ESTAB",
                SocketState::FinWait1 => "FIN-WAIT-1",
                SocketState::FinWait2 => "FIN-WAIT-2",
                SocketState::CloseWait => "CLOSE-WAIT",
                SocketState::Closing => "CLOSING",
                SocketState::LastAck => "LAST-ACK",
                SocketState::TimeWait => "TIME-WAIT",
                SocketState::Closed => "CLOSED",
            };
            if listen_only && sock.state != SocketState::Listen { continue; }
            out.push_str(&format!("{:<10} {:>6} {:>6}  {:<22} {}\n",
                state_str, sock.recv_queue, sock.send_queue,
                format!("{}:{}", fmt_ip(sock.local_ip), sock.local_port),
                format!("{}:{}", fmt_ip(sock.remote_ip), sock.remote_port),
            ));
        }
    }

    if show_udp {
        out.push_str(&format!("{:<10} {:>6} {:>6}  {:<22} {}\n",
            "UNCONN", 0, 0, "0.0.0.0:*", "0.0.0.0:*"));
    }

    out
}

// ── 16. nc (netcat) command ──────────────────────────────────────────

/// nc (netcat) command — parse arguments and execute.
pub fn nc_command(args: &str) -> String {
    let parts: Vec<&str> = args.split_whitespace().collect();

    // Parse flags
    let mut zero_io = false;
    let mut verbose = false;
    let mut listen = false;
    let mut remaining = Vec::new();

    for part in &parts {
        if part.starts_with('-') {
            for ch in part[1..].chars() {
                match ch {
                    'z' => zero_io = true,
                    'v' => verbose = true,
                    'l' => listen = true,
                    _ => {}
                }
            }
        } else {
            remaining.push(*part);
        }
    }

    if listen {
        if let Some(port) = remaining.first().and_then(|s| s.parse::<u16>().ok()) {
            return format!("Listening on 0.0.0.0:{} ...\n(use Ctrl+C to stop)", port);
        }
        return String::from("Usage: nc -l <port>");
    }

    if remaining.len() < 2 {
        return String::from("Usage: nc [-zv] <host> <port|start-end>\n  nc -l <port>");
    }

    let host = remaining[0];
    let port_arg = remaining[1];

    // Parse port or port range
    if let Some((start_s, end_s)) = port_arg.split_once('-') {
        let start = start_s.parse::<u16>().unwrap_or(1);
        let end = end_s.parse::<u16>().unwrap_or(start);
        return nc_port_scan_range(host, start, end, verbose);
    }

    if let Ok(port) = port_arg.parse::<u16>() {
        if zero_io {
            // Port scan single port
            let open = port % 3 == 0; // simulated
            if verbose {
                if open {
                    format!("Connection to {} {} port [tcp/*] succeeded!", host, port)
                } else {
                    format!("nc: connect to {} port {} (tcp) failed: Connection refused", host, port)
                }
            } else if open {
                format!("{}: open", port)
            } else {
                String::new()
            }
        } else {
            format!("Connected to {} port {}\n(interactive mode not available, use -z for scan)", host, port)
        }
    } else {
        String::from("nc: invalid port")
    }
}

fn nc_port_scan_range(host: &str, start: u16, end: u16, verbose: bool) -> String {
    let mut out = String::new();
    for port in start..=end {
        let open = port == 22 || port == 80 || port == 443 || port == 8080 || port % 100 == 0;
        if open {
            out.push_str(&format!("Connection to {} {} port [tcp/*] succeeded!\n", host, port));
        } else if verbose {
            out.push_str(&format!("nc: connect to {} port {} (tcp) failed: Connection refused\n", host, port));
        }
    }
    if out.is_empty() { out.push_str("No open ports found.\n"); }
    out
}

// ---------------------------------------------------------------------------
// ip command — iproute2-style network configuration
// ---------------------------------------------------------------------------

/// `ip` command dispatcher — supports `ip address`, `ip route`, `ip link`, `ip neigh`.
pub fn ip_command(args: &str) -> String {
    let parts: Vec<&str> = args.split_whitespace().collect();
    let subcmd = parts.first().map(|s| *s).unwrap_or("help");

    match subcmd {
        "address" | "addr" | "a" => ip_address(),
        "route" | "r" => ip_route(),
        "link" | "l" => ip_link(),
        "neigh" | "neighbour" | "n" => ip_neigh(),
        "help" | "-h" => String::from(
            "Usage: ip <subcommand>\n  ip address  - show IP addresses\n  ip route    - show routing table\n  ip link     - show link state\n  ip neigh    - show ARP/NDP neighbors\n"
        ),
        _ => alloc::format!("ip: unknown subcommand '{}'. Try 'ip help'.\n", subcmd),
    }
}

/// `ip address` — show all interface addresses (like `ip addr show`).
fn ip_address() -> String {
    let net = crate::net::NET.lock();
    let mac = net.mac.0;
    let ip = net.ip.0;
    drop(net);

    let mut out = String::from("1: lo: <LOOPBACK,UP> mtu 65536\n");
    out.push_str("    inet 127.0.0.1/8 scope host lo\n");
    out.push_str("    inet6 ::1/128 scope host\n");

    out.push_str(&alloc::format!(
        "2: eth0: <BROADCAST,MULTICAST,UP> mtu 1500\n    link/ether {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\n",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    ));
    out.push_str(&alloc::format!(
        "    inet {}.{}.{}.{}/24 brd {}.{}.{}.255 scope global eth0\n",
        ip[0], ip[1], ip[2], ip[3], ip[0], ip[1], ip[2]
    ));

    // IPv6 link-local
    let v6 = crate::ipv6::our_ipv6();
    out.push_str(&alloc::format!("    inet6 {}/64 scope link\n", v6.display()));

    out
}

/// `ip route` — show routing table.
fn ip_route() -> String {
    let net = crate::net::NET.lock();
    let ip = net.ip.0;
    let gw = net.gateway.0;
    drop(net);

    let mut out = alloc::format!(
        "default via {}.{}.{}.{} dev eth0\n",
        gw[0], gw[1], gw[2], gw[3]
    );
    out.push_str(&alloc::format!(
        "{}.{}.{}.0/24 dev eth0 proto kernel scope link src {}.{}.{}.{}\n",
        ip[0], ip[1], ip[2], ip[0], ip[1], ip[2], ip[3]
    ));

    // IPv6 routes
    out.push_str("fe80::/64 dev eth0 proto kernel scope link\n");

    out
}

/// `ip link` — show link layer info.
fn ip_link() -> String {
    let net = crate::net::NET.lock();
    let mac = net.mac.0;
    drop(net);

    let mut out = String::from("1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UP\n");
    out.push_str("    link/loopback 00:00:00:00:00:00\n");

    out.push_str(&alloc::format!(
        "2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc fq_codel state UP\n    link/ether {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\n",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    ));

    out
}

/// `ip neigh` — show ARP/NDP neighbor table.
fn ip_neigh() -> String {
    let mut out = String::new();

    // IPv4 ARP entries
    let net = crate::net::NET.lock();
    let gw = net.gateway.0;
    let gw_mac = net.mac.0; // approximate
    drop(net);

    out.push_str(&alloc::format!(
        "{}.{}.{}.{} dev eth0 lladdr {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} REACHABLE\n",
        gw[0], gw[1], gw[2], gw[3],
        gw_mac[0], gw_mac[1], gw_mac[2], gw_mac[3], gw_mac[4], gw_mac[5]
    ));

    // IPv6 NDP
    out.push_str(&crate::ipv6::ndp_table());

    out
}
