/// HTTP forward proxy for MerlionOS.
/// Supports HTTP CONNECT tunneling, request forwarding,
/// proxy authentication, and connection pooling.

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

/// Default HTTP proxy listen port
const PROXY_DEFAULT_PORT: u16 = 8080;

/// Maximum concurrent connections
const MAX_CONNECTIONS: usize = 256;

/// Maximum pooled idle connections
const MAX_IDLE_POOL: usize = 32;

/// Max idle time for pooled connections (seconds)
const POOL_IDLE_TIMEOUT: u64 = 60;

/// Maximum cached responses
const MAX_CACHE_ENTRIES: usize = 64;

/// Maximum blocked domains
const MAX_BLOCKED_DOMAINS: usize = 128;

/// Maximum allowed client IPs
const MAX_ALLOWED_CLIENTS: usize = 64;

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

static TOTAL_REQUESTS: AtomicU64 = AtomicU64::new(0);
static CONNECT_TUNNELS: AtomicU64 = AtomicU64::new(0);
static FORWARDED_REQUESTS: AtomicU64 = AtomicU64::new(0);
static ACTIVE_CONNECTIONS: AtomicU64 = AtomicU64::new(0);
static BYTES_TX: AtomicU64 = AtomicU64::new(0);
static BYTES_RX: AtomicU64 = AtomicU64::new(0);
static AUTH_FAILURES: AtomicU64 = AtomicU64::new(0);
static CACHE_HITS: AtomicU64 = AtomicU64::new(0);
static CACHE_MISSES: AtomicU64 = AtomicU64::new(0);
static BLOCKED_REQUESTS: AtomicU64 = AtomicU64::new(0);

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// HTTP method
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Head,
    Options,
    Patch,
    Connect,
}

impl HttpMethod {
    fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPTIONS",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Connect => "CONNECT",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "GET" => Some(HttpMethod::Get),
            "POST" => Some(HttpMethod::Post),
            "PUT" => Some(HttpMethod::Put),
            "DELETE" => Some(HttpMethod::Delete),
            "HEAD" => Some(HttpMethod::Head),
            "OPTIONS" => Some(HttpMethod::Options),
            "PATCH" => Some(HttpMethod::Patch),
            "CONNECT" => Some(HttpMethod::Connect),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Connection and pooling
// ---------------------------------------------------------------------------

/// State of a proxy connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    /// Waiting for client request
    Receiving,
    /// Tunnel established (CONNECT)
    Tunneling,
    /// Forwarding request to upstream
    Forwarding,
    /// Connection idle in pool
    Idle,
    /// Connection closed
    Closed,
}

/// A proxy connection tracking client-to-upstream link.
#[derive(Clone)]
pub struct ProxyConnection {
    pub id: u64,
    pub state: ConnState,
    pub client_ip: [u8; 4],
    pub client_port: u16,
    pub target_host: String,
    pub target_port: u16,
    pub method: Option<HttpMethod>,
    pub bytes_tx: u64,
    pub bytes_rx: u64,
    pub created_tick: u64,
    pub is_tunnel: bool,
}

/// A pooled idle connection to reuse.
#[derive(Clone)]
struct PooledConn {
    host: String,
    port: u16,
    idle_since: u64,
}

/// A cached HTTP response (basic).
#[derive(Clone)]
struct CacheEntry {
    url: String,
    status: u16,
    body_len: usize,
    expires_tick: u64,
}

// ---------------------------------------------------------------------------
// Proxy server state
// ---------------------------------------------------------------------------

struct HttpProxyServer {
    listen_port: u16,
    require_auth: bool,
    users: Vec<(String, String)>,
    connections: Vec<ProxyConnection>,
    next_conn_id: u64,
    pool: Vec<PooledConn>,
    cache: Vec<CacheEntry>,
    blocked_domains: Vec<String>,
    blocked_ips: Vec<[u8; 4]>,
    allowed_clients: Vec<[u8; 4]>,
    add_via_header: bool,
    add_xforwarded: bool,
    strip_proxy_headers: bool,
}

impl HttpProxyServer {
    const fn new() -> Self {
        Self {
            listen_port: PROXY_DEFAULT_PORT,
            require_auth: false,
            users: Vec::new(),
            connections: Vec::new(),
            next_conn_id: 1,
            pool: Vec::new(),
            cache: Vec::new(),
            blocked_domains: Vec::new(),
            blocked_ips: Vec::new(),
            allowed_clients: Vec::new(),
            add_via_header: true,
            add_xforwarded: true,
            strip_proxy_headers: true,
        }
    }

    fn authenticate(&self, auth_header: &str) -> bool {
        // Expect "Basic <base64(user:pass)>"
        if !auth_header.starts_with("Basic ") {
            return false;
        }
        // In a real implementation, we'd base64-decode here
        // For now, check against stored credentials literally
        let cred = &auth_header[6..];
        for (u, p) in &self.users {
            let expected = format!("{}:{}", u, p);
            if cred == expected {
                return true;
            }
        }
        false
    }

    fn is_blocked(&self, host: &str, ip: Option<[u8; 4]>) -> bool {
        for d in &self.blocked_domains {
            if host == d.as_str() || host.ends_with(&format!(".{}", d)) {
                return true;
            }
        }
        if let Some(target_ip) = ip {
            for blocked in &self.blocked_ips {
                if &target_ip == blocked {
                    return true;
                }
            }
        }
        false
    }

    fn is_client_allowed(&self, ip: [u8; 4]) -> bool {
        if self.allowed_clients.is_empty() {
            return true;
        }
        self.allowed_clients.contains(&ip)
    }

    fn get_pooled(&mut self, host: &str, port: u16) -> bool {
        let now = crate::timer::ticks();
        // Remove expired
        self.pool.retain(|c| {
            let age = now.saturating_sub(c.idle_since) / 100; // ticks to seconds approx
            age < POOL_IDLE_TIMEOUT
        });
        if let Some(pos) = self.pool.iter().position(|c| c.host == host && c.port == port) {
            self.pool.remove(pos);
            true
        } else {
            false
        }
    }

    fn return_to_pool(&mut self, host: String, port: u16) {
        if self.pool.len() >= MAX_IDLE_POOL {
            self.pool.remove(0);
        }
        self.pool.push(PooledConn {
            host,
            port,
            idle_since: crate::timer::ticks(),
        });
    }

    fn cache_lookup(&self, url: &str) -> bool {
        let now = crate::timer::ticks();
        self.cache.iter().any(|e| e.url == url && e.expires_tick > now)
    }

    fn create_connection(&mut self, client_ip: [u8; 4], client_port: u16,
                         host: &str, port: u16, method: HttpMethod) -> u64 {
        let id = self.next_conn_id;
        self.next_conn_id += 1;
        if self.connections.len() >= MAX_CONNECTIONS {
            if let Some(pos) = self.connections.iter().position(|c| c.state == ConnState::Closed) {
                self.connections.remove(pos);
            }
        }
        let is_tunnel = method == HttpMethod::Connect;
        self.connections.push(ProxyConnection {
            id,
            state: if is_tunnel { ConnState::Tunneling } else { ConnState::Forwarding },
            client_ip,
            client_port,
            target_host: host.to_owned(),
            target_port: port,
            method: Some(method),
            bytes_tx: 0,
            bytes_rx: 0,
            created_tick: crate::timer::ticks(),
            is_tunnel,
        });
        TOTAL_REQUESTS.fetch_add(1, Ordering::Relaxed);
        ACTIVE_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
        if is_tunnel {
            CONNECT_TUNNELS.fetch_add(1, Ordering::Relaxed);
        } else {
            FORWARDED_REQUESTS.fetch_add(1, Ordering::Relaxed);
        }
        id
    }

    fn close_connection(&mut self, id: u64) {
        if let Some(c) = self.connections.iter_mut().find(|c| c.id == id) {
            if c.state != ConnState::Closed {
                c.state = ConnState::Closed;
                ACTIVE_CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
}

static PROXY: Mutex<HttpProxyServer> = Mutex::new(HttpProxyServer::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the HTTP proxy subsystem.
pub fn init() {
    INITIALIZED.store(true, Ordering::SeqCst);
    crate::serial_println!("[http-proxy] HTTP proxy subsystem initialized");
}

/// Start the HTTP proxy on the given port.
pub fn start(port: u16) {
    let mut srv = PROXY.lock();
    srv.listen_port = port;
    RUNNING.store(true, Ordering::SeqCst);
    crate::serial_println!("[http-proxy] HTTP proxy started on port {}", port);
}

/// Stop the HTTP proxy.
pub fn stop() {
    RUNNING.store(false, Ordering::SeqCst);
    let mut srv = PROXY.lock();
    let ids: Vec<u64> = srv.connections.iter()
        .filter(|c| c.state != ConnState::Closed)
        .map(|c| c.id)
        .collect();
    for id in ids {
        srv.close_connection(id);
    }
    srv.pool.clear();
    crate::serial_println!("[http-proxy] HTTP proxy stopped");
}

/// Return proxy information string.
pub fn proxy_info() -> String {
    let srv = PROXY.lock();
    let running = RUNNING.load(Ordering::Relaxed);
    let active = srv.connections.iter().filter(|c| c.state != ConnState::Closed).count();
    let auth_mode = if srv.require_auth { "Basic" } else { "none" };
    format!(
        "HTTP Proxy:\n  Status: {}\n  Port: {}\n  Auth: {}\n  Active connections: {}\n  Pooled idle: {}\n  Cached entries: {}\n  Blocked domains: {}\n  Via header: {}\n  X-Forwarded-For: {}",
        if running { "running" } else { "stopped" },
        srv.listen_port,
        auth_mode,
        active,
        srv.pool.len(),
        srv.cache.len(),
        srv.blocked_domains.len(),
        srv.add_via_header,
        srv.add_xforwarded,
    )
}

/// Return proxy statistics string.
pub fn proxy_stats() -> String {
    format!(
        "HTTP Proxy Stats:\n  Total requests: {}\n  CONNECT tunnels: {}\n  Forwarded requests: {}\n  Active connections: {}\n  Bytes TX: {}\n  Bytes RX: {}\n  Auth failures: {}\n  Cache hits: {}\n  Cache misses: {}\n  Blocked requests: {}",
        TOTAL_REQUESTS.load(Ordering::Relaxed),
        CONNECT_TUNNELS.load(Ordering::Relaxed),
        FORWARDED_REQUESTS.load(Ordering::Relaxed),
        ACTIVE_CONNECTIONS.load(Ordering::Relaxed),
        BYTES_TX.load(Ordering::Relaxed),
        BYTES_RX.load(Ordering::Relaxed),
        AUTH_FAILURES.load(Ordering::Relaxed),
        CACHE_HITS.load(Ordering::Relaxed),
        CACHE_MISSES.load(Ordering::Relaxed),
        BLOCKED_REQUESTS.load(Ordering::Relaxed),
    )
}

/// List active connections as a formatted string.
pub fn list_connections() -> String {
    let srv = PROXY.lock();
    let active: Vec<&ProxyConnection> = srv.connections.iter()
        .filter(|c| c.state != ConnState::Closed)
        .collect();
    if active.is_empty() {
        return "No active HTTP proxy connections".to_owned();
    }
    let mut out = format!("Active HTTP proxy connections ({}):\n", active.len());
    for c in &active {
        let method_str = c.method.map(|m| m.as_str()).unwrap_or("?");
        let state_str = match c.state {
            ConnState::Receiving => "RECV",
            ConnState::Tunneling => "TUNNEL",
            ConnState::Forwarding => "FWD",
            ConnState::Idle => "IDLE",
            ConnState::Closed => "CLOSED",
        };
        out.push_str(&format!("  #{}: {}.{}.{}.{}:{} -> {}:{} [{}] {} tx={} rx={}\n",
            c.id,
            c.client_ip[0], c.client_ip[1], c.client_ip[2], c.client_ip[3],
            c.client_port, c.target_host, c.target_port,
            method_str, state_str, c.bytes_tx, c.bytes_rx));
    }
    out
}
