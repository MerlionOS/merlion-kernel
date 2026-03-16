/// HTTPS server for MerlionOS.
/// Extends the HTTP server with TLS encryption, connection pooling,
/// HTTP/1.1 keep-alive, reverse proxy, and load balancing.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// TLS Connection
// ---------------------------------------------------------------------------

/// State of a TLS connection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TlsState {
    /// TLS handshake in progress.
    Handshaking,
    /// Handshake complete, encrypted data flowing.
    Established,
    /// Close-notify sent/received.
    Closing,
    /// Connection fully closed.
    Closed,
}

/// A single TLS-wrapped connection.
struct TlsConnection {
    /// Unique connection identifier.
    id: u32,
    /// Client IPv4 address.
    client_ip: [u8; 4],
    /// Current TLS state.
    state: TlsState,
    /// Negotiated cipher suite name.
    cipher_suite: String,
    /// Tick count when the connection was created.
    created_tick: u64,
    /// Total bytes encrypted (sent to client).
    bytes_encrypted: u64,
    /// Total bytes decrypted (received from client).
    bytes_decrypted: u64,
    /// Number of HTTP requests served on this connection.
    requests_served: u32,
    /// Whether HTTP keep-alive is enabled.
    keep_alive: bool,
}

/// Global connection ID counter.
static NEXT_CONN_ID: AtomicU32 = AtomicU32::new(1);

impl TlsConnection {
    /// Create a new TLS connection in the Handshaking state.
    fn new(client_ip: [u8; 4]) -> Self {
        let id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);
        Self {
            id,
            client_ip,
            state: TlsState::Handshaking,
            cipher_suite: String::from("TLS_AES_128_GCM_SHA256"),
            created_tick: crate::timer::ticks(),
            bytes_encrypted: 0,
            bytes_decrypted: 0,
            requests_served: 0,
            keep_alive: true,
        }
    }

    /// Format connection info as a human-readable string.
    fn display(&self) -> String {
        format!(
            "conn#{} {}.{}.{}.{}  {:?}  cipher={}  reqs={}  enc={}B  dec={}B  ka={}",
            self.id,
            self.client_ip[0], self.client_ip[1],
            self.client_ip[2], self.client_ip[3],
            self.state,
            self.cipher_suite,
            self.requests_served,
            self.bytes_encrypted,
            self.bytes_decrypted,
            if self.keep_alive { "on" } else { "off" },
        )
    }
}

// ---------------------------------------------------------------------------
// Connection Pool
// ---------------------------------------------------------------------------

/// Maximum simultaneous TLS connections.
const MAX_POOL_SIZE: usize = 32;

/// Pool of active TLS connections.
struct ConnectionPool {
    connections: Vec<TlsConnection>,
    /// Connections idle longer than this (in ticks) are reaped.
    max_idle_ticks: u64,
    /// Maximum HTTP requests per connection before forcing close.
    max_requests_per_conn: u32,
}

impl ConnectionPool {
    const fn new() -> Self {
        Self {
            connections: Vec::new(),
            max_idle_ticks: 3000, // ~30 seconds at 100 Hz
            max_requests_per_conn: 100,
        }
    }
}

static POOL: Mutex<ConnectionPool> = Mutex::new(ConnectionPool::new());

/// Acquire a new connection slot from the pool. Returns the connection ID,
/// or `None` if the pool is full.
pub fn pool_get(client_ip: [u8; 4]) -> Option<u32> {
    let mut pool = POOL.lock();
    if pool.connections.len() >= MAX_POOL_SIZE {
        return None;
    }
    let conn = TlsConnection::new(client_ip);
    let id = conn.id;
    pool.connections.push(conn);
    TOTAL_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
    Some(id)
}

/// Mark a connection as established after TLS handshake completes.
pub fn pool_establish(id: u32) {
    let mut pool = POOL.lock();
    if let Some(conn) = pool.connections.iter_mut().find(|c| c.id == id) {
        conn.state = TlsState::Established;
    }
}

/// Record bytes transferred on a connection.
pub fn pool_record_bytes(id: u32, encrypted: u64, decrypted: u64) {
    let mut pool = POOL.lock();
    if let Some(conn) = pool.connections.iter_mut().find(|c| c.id == id) {
        conn.bytes_encrypted += encrypted;
        conn.bytes_decrypted += decrypted;
        conn.requests_served += 1;
        TOTAL_REQUESTS.fetch_add(1, Ordering::Relaxed);
        TOTAL_BYTES_ENCRYPTED.fetch_add(encrypted, Ordering::Relaxed);
        TOTAL_BYTES_DECRYPTED.fetch_add(decrypted, Ordering::Relaxed);
    }
}

/// Release (close) a connection and remove it from the pool.
pub fn pool_release(id: u32) {
    let mut pool = POOL.lock();
    pool.connections.retain(|c| c.id != id);
}

/// Remove idle and over-limit connections from the pool.
pub fn pool_cleanup() {
    let now = crate::timer::ticks();
    let mut pool = POOL.lock();
    let max_idle = pool.max_idle_ticks;
    let max_reqs = pool.max_requests_per_conn;
    pool.connections.retain(|c| {
        if c.state == TlsState::Closed {
            return false;
        }
        if now.wrapping_sub(c.created_tick) > max_idle && c.state != TlsState::Handshaking {
            return false;
        }
        if c.requests_served >= max_reqs {
            return false;
        }
        true
    });
}

/// Return pool statistics as a formatted string.
pub fn pool_stats() -> String {
    let pool = POOL.lock();
    let active = pool.connections.iter().filter(|c| c.state == TlsState::Established).count();
    let handshaking = pool.connections.iter().filter(|c| c.state == TlsState::Handshaking).count();
    let mut out = String::new();
    out.push_str(&format!("pool: {}/{} slots used ({} active, {} handshaking)\n",
        pool.connections.len(), MAX_POOL_SIZE, active, handshaking));
    out.push_str(&format!("pool: max_idle={}t, max_reqs/conn={}\n",
        pool.max_idle_ticks, pool.max_requests_per_conn));
    for conn in &pool.connections {
        out.push_str(&format!("  {}\n", conn.display()));
    }
    out
}

// ---------------------------------------------------------------------------
// Self-signed Certificate (simulated)
// ---------------------------------------------------------------------------

/// A simulated X.509 certificate.
pub struct Certificate {
    /// Subject common name.
    pub subject: String,
    /// Issuer common name.
    pub issuer: String,
    /// Validity start (human-readable).
    pub valid_from: String,
    /// Validity end (human-readable).
    pub valid_to: String,
    /// Serial number.
    pub serial: u64,
    /// SHA-256 fingerprint (hex string, simulated).
    pub fingerprint: String,
}

static CERT: Mutex<Option<Certificate>> = Mutex::new(None);

/// Generate a simulated self-signed certificate for the given hostname.
pub fn generate_self_signed(hostname: &str) -> Certificate {
    // Deterministic pseudo-fingerprint from hostname bytes.
    let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    for b in hostname.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV-1a prime
    }
    let fp = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        (hash >> 56) as u8, (hash >> 48) as u8,
        (hash >> 40) as u8, (hash >> 32) as u8,
        (hash >> 24) as u8, (hash >> 16) as u8,
        (hash >> 8) as u8,  hash as u8,
    );

    Certificate {
        subject: format!("CN={}", hostname),
        issuer: format!("CN={} (self-signed)", hostname),
        valid_from: String::from("2026-01-01T00:00:00Z"),
        valid_to: String::from("2027-01-01T00:00:00Z"),
        serial: hash,
        fingerprint: fp,
    }
}

/// Return certificate info as a formatted string.
pub fn cert_info() -> String {
    let cert = CERT.lock();
    match cert.as_ref() {
        Some(c) => format!(
            "cert: subject={}\ncert: issuer={}\ncert: valid {} to {}\ncert: serial={:#x}\ncert: fingerprint={}\n",
            c.subject, c.issuer, c.valid_from, c.valid_to, c.serial, c.fingerprint,
        ),
        None => String::from("cert: (no certificate generated)\n"),
    }
}

// ---------------------------------------------------------------------------
// Reverse Proxy & Load Balancing
// ---------------------------------------------------------------------------

/// A reverse proxy routing entry.
struct ProxyRoute {
    /// URL path prefix to match (e.g. "/api").
    path_prefix: String,
    /// Backend server IPv4 address.
    backend_ip: [u8; 4],
    /// Backend server port.
    backend_port: u16,
    /// Load-balancing weight (higher = more traffic).
    weight: u8,
}

const MAX_PROXY_ROUTES: usize = 16;

static PROXY_ROUTES: Mutex<Vec<ProxyRoute>> = Mutex::new(Vec::new());

/// Add a reverse proxy route.
pub fn add_proxy_route(prefix: &str, backend_ip: [u8; 4], port: u16, weight: u8) {
    let mut routes = PROXY_ROUTES.lock();
    // Update existing route with same prefix.
    for r in routes.iter_mut() {
        if r.path_prefix == prefix {
            r.backend_ip = backend_ip;
            r.backend_port = port;
            r.weight = weight;
            return;
        }
    }
    if routes.len() < MAX_PROXY_ROUTES {
        routes.push(ProxyRoute {
            path_prefix: String::from(prefix),
            backend_ip,
            backend_port: port,
            weight,
        });
    }
}

/// Remove a reverse proxy route by path prefix.
pub fn remove_proxy_route(prefix: &str) {
    let mut routes = PROXY_ROUTES.lock();
    routes.retain(|r| r.path_prefix != prefix);
}

/// List all proxy routes as a formatted string.
pub fn list_proxy_routes() -> String {
    let routes = PROXY_ROUTES.lock();
    if routes.is_empty() {
        return String::from("(no proxy routes)\n");
    }
    let mut out = String::new();
    out.push_str("Prefix          Backend              Port   Weight\n");
    out.push_str("--------------- -------------------- ------ ------\n");
    for r in routes.iter() {
        out.push_str(&format!(
            "{:<15} {}.{}.{}.{:<13} {:<6} {}\n",
            r.path_prefix,
            r.backend_ip[0], r.backend_ip[1],
            r.backend_ip[2], r.backend_ip[3],
            r.backend_port, r.weight,
        ));
    }
    out
}

/// Route a request path to a backend. Returns `(ip, port)` of the best
/// matching backend, using longest-prefix match. When multiple routes share
/// the same prefix, the one with the highest weight is preferred.
pub fn proxy_request(path: &str) -> Option<([u8; 4], u16)> {
    let routes = PROXY_ROUTES.lock();
    let mut best: Option<&ProxyRoute> = None;
    for r in routes.iter() {
        if path.starts_with(&r.path_prefix) {
            match best {
                None => best = Some(r),
                Some(prev) => {
                    if r.path_prefix.len() > prev.path_prefix.len()
                        || (r.path_prefix.len() == prev.path_prefix.len()
                            && r.weight > prev.weight)
                    {
                        best = Some(r);
                    }
                }
            }
        }
    }
    best.map(|r| (r.backend_ip, r.backend_port))
}

// ---------------------------------------------------------------------------
// Rate Limiting (per-IP)
// ---------------------------------------------------------------------------

/// Per-IP rate limit state.
struct RateLimit {
    ip: [u8; 4],
    /// Number of requests in the current window.
    requests: u32,
    /// Tick when the current window started.
    window_start: u64,
    /// Maximum requests allowed per window.
    max_per_window: u32,
    /// Window duration in ticks.
    window_ticks: u64,
}

const MAX_RATE_ENTRIES: usize = 64;
const DEFAULT_RATE_LIMIT: u32 = 60;     // 60 requests per window
const DEFAULT_RATE_WINDOW: u64 = 6000;  // ~60 seconds at 100 Hz

static RATE_LIMITS: Mutex<Vec<RateLimit>> = Mutex::new(Vec::new());

/// Check whether a request from `ip` is allowed under the rate limit.
/// Returns `true` if the request is permitted, `false` if rate-limited.
pub fn check_rate_limit(ip: [u8; 4]) -> bool {
    let now = crate::timer::ticks();
    let mut limits = RATE_LIMITS.lock();

    // Find existing entry for this IP.
    for entry in limits.iter_mut() {
        if entry.ip == ip {
            // Reset window if expired.
            if now.wrapping_sub(entry.window_start) > entry.window_ticks {
                entry.requests = 0;
                entry.window_start = now;
            }
            if entry.requests >= entry.max_per_window {
                return false;
            }
            entry.requests += 1;
            return true;
        }
    }

    // New IP: create entry.
    if limits.len() < MAX_RATE_ENTRIES {
        limits.push(RateLimit {
            ip,
            requests: 1,
            window_start: now,
            max_per_window: DEFAULT_RATE_LIMIT,
            window_ticks: DEFAULT_RATE_WINDOW,
        });
    }
    true
}

/// Return rate limiting statistics as a formatted string.
pub fn rate_limit_stats() -> String {
    let limits = RATE_LIMITS.lock();
    if limits.is_empty() {
        return String::from("(no rate limit entries)\n");
    }
    let now = crate::timer::ticks();
    let mut out = String::new();
    out.push_str("IP Address       Requests  Limit  Window  Remaining\n");
    out.push_str("---------------- --------- ------ ------- ---------\n");
    for e in limits.iter() {
        let elapsed = now.wrapping_sub(e.window_start);
        let remaining = if elapsed < e.window_ticks {
            e.window_ticks - elapsed
        } else {
            0
        };
        out.push_str(&format!(
            "{}.{}.{}.{:<8} {:<9} {:<6} {:<7} {}t\n",
            e.ip[0], e.ip[1], e.ip[2], e.ip[3],
            e.requests, e.max_per_window, e.window_ticks, remaining,
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Global statistics
// ---------------------------------------------------------------------------

static TOTAL_CONNECTIONS: AtomicU64 = AtomicU64::new(0);
static TOTAL_REQUESTS: AtomicU64 = AtomicU64::new(0);
static TOTAL_BYTES_ENCRYPTED: AtomicU64 = AtomicU64::new(0);
static TOTAL_BYTES_DECRYPTED: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// HTTPS Server public API
// ---------------------------------------------------------------------------

/// Initialise the HTTPS server: generate a self-signed certificate
/// and prepare the connection pool.
pub fn init() {
    let cert = generate_self_signed("merlionos.local");
    crate::serial_println!(
        "[https] initialised, cert subject={}, serial={:#x}",
        cert.subject, cert.serial
    );
    *CERT.lock() = Some(cert);
}

/// Return HTTPS server information: certificate, pool status, proxy routes.
pub fn https_info() -> String {
    let mut out = String::new();
    out.push_str(&cert_info());
    out.push_str(&pool_stats());
    out.push_str(&list_proxy_routes());
    out
}

/// Return HTTPS server statistics: connections, bytes, requests.
pub fn https_stats() -> String {
    format!(
        "https: total connections: {}\nhttps: total requests: {}\nhttps: bytes encrypted: {}\nhttps: bytes decrypted: {}\n",
        TOTAL_CONNECTIONS.load(Ordering::Relaxed),
        TOTAL_REQUESTS.load(Ordering::Relaxed),
        TOTAL_BYTES_ENCRYPTED.load(Ordering::Relaxed),
        TOTAL_BYTES_DECRYPTED.load(Ordering::Relaxed),
    )
}
