/// SOCKS5 proxy server for MerlionOS (RFC 1928).
/// Supports TCP CONNECT, UDP ASSOCIATE, and username/password authentication.

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

/// Default SOCKS5 listen port
const SOCKS5_DEFAULT_PORT: u16 = 1080;

/// Maximum concurrent sessions
const MAX_SESSIONS: usize = 128;

/// Maximum access control entries
const MAX_ACL_ENTRIES: usize = 64;

/// SOCKS5 protocol version
const SOCKS_VERSION: u8 = 0x05;

/// Authentication methods
const AUTH_NO_AUTH: u8 = 0x00;
const AUTH_GSSAPI: u8 = 0x01;
const AUTH_USERNAME_PASSWORD: u8 = 0x02;
const AUTH_NO_ACCEPTABLE: u8 = 0xFF;

/// Commands
const CMD_CONNECT: u8 = 0x01;
const CMD_BIND: u8 = 0x02;
const CMD_UDP_ASSOCIATE: u8 = 0x03;

/// Address types
const ATYP_IPV4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_IPV6: u8 = 0x04;

/// Reply codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ReplyCode {
    Success = 0x00,
    GeneralFailure = 0x01,
    NotAllowed = 0x02,
    NetworkUnreachable = 0x03,
    HostUnreachable = 0x04,
    ConnectionRefused = 0x05,
    TtlExpired = 0x06,
    CommandNotSupported = 0x07,
    AddressNotSupported = 0x08,
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

static TOTAL_CONNECTIONS: AtomicU64 = AtomicU64::new(0);
static ACTIVE_SESSIONS: AtomicU64 = AtomicU64::new(0);
static BYTES_FORWARDED_TX: AtomicU64 = AtomicU64::new(0);
static BYTES_FORWARDED_RX: AtomicU64 = AtomicU64::new(0);
static AUTH_FAILURES: AtomicU64 = AtomicU64::new(0);
static CONNECT_REQUESTS: AtomicU64 = AtomicU64::new(0);
static BIND_REQUESTS: AtomicU64 = AtomicU64::new(0);
static UDP_ASSOCIATE_REQUESTS: AtomicU64 = AtomicU64::new(0);
static DENIED_REQUESTS: AtomicU64 = AtomicU64::new(0);

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Address representation
// ---------------------------------------------------------------------------

/// A SOCKS5 target address.
#[derive(Debug, Clone)]
pub enum Socks5Addr {
    /// IPv4 address [a, b, c, d]
    V4([u8; 4]),
    /// Domain name
    Domain(String),
    /// IPv6 address (16 bytes)
    V6([u8; 16]),
}

impl Socks5Addr {
    fn to_string_repr(&self) -> String {
        match self {
            Socks5Addr::V4(ip) => format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]),
            Socks5Addr::Domain(d) => d.clone(),
            Socks5Addr::V6(ip) => {
                let mut s = String::new();
                for i in 0..8 {
                    if i > 0 { s.push(':'); }
                    let w = ((ip[i * 2] as u16) << 8) | ip[i * 2 + 1] as u16;
                    s.push_str(&format!("{:x}", w));
                }
                s
            }
        }
    }

    fn atyp(&self) -> u8 {
        match self {
            Socks5Addr::V4(_) => ATYP_IPV4,
            Socks5Addr::Domain(_) => ATYP_DOMAIN,
            Socks5Addr::V6(_) => ATYP_IPV6,
        }
    }
}

// ---------------------------------------------------------------------------
// Proxy session
// ---------------------------------------------------------------------------

/// State of a SOCKS5 session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Handshake,
    Authenticating,
    Requesting,
    Connected,
    UdpAssociate,
    Closed,
}

/// A single proxy session tracking client-to-target connection.
#[derive(Clone)]
pub struct Socks5Session {
    pub id: u64,
    pub state: SessionState,
    pub client_ip: [u8; 4],
    pub client_port: u16,
    pub target_addr: Option<Socks5Addr>,
    pub target_port: u16,
    pub command: u8,
    pub auth_method: u8,
    pub username: Option<String>,
    pub bytes_tx: u64,
    pub bytes_rx: u64,
    pub created_tick: u64,
}

// ---------------------------------------------------------------------------
// Access control
// ---------------------------------------------------------------------------

/// An access control entry for filtering source or destination.
#[derive(Clone)]
pub struct AclEntry {
    pub ip: [u8; 4],
    pub mask: [u8; 4],
    pub port: Option<u16>,
    pub allow: bool,
}

impl AclEntry {
    fn matches_ip(&self, ip: [u8; 4]) -> bool {
        for i in 0..4 {
            if (ip[i] & self.mask[i]) != (self.ip[i] & self.mask[i]) {
                return false;
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Upstream proxy (chaining)
// ---------------------------------------------------------------------------

/// Configuration for chaining to an upstream SOCKS5 proxy.
#[derive(Clone)]
pub struct UpstreamProxy {
    pub addr: [u8; 4],
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

// ---------------------------------------------------------------------------
// Server state
// ---------------------------------------------------------------------------

struct Socks5Server {
    listen_port: u16,
    require_auth: bool,
    users: Vec<(String, String)>,
    sessions: Vec<Socks5Session>,
    next_session_id: u64,
    source_acl: Vec<AclEntry>,
    dest_acl: Vec<AclEntry>,
    upstream: Option<UpstreamProxy>,
}

impl Socks5Server {
    const fn new() -> Self {
        Self {
            listen_port: SOCKS5_DEFAULT_PORT,
            require_auth: false,
            users: Vec::new(),
            sessions: Vec::new(),
            next_session_id: 1,
            source_acl: Vec::new(),
            dest_acl: Vec::new(),
            upstream: None,
        }
    }

    fn select_auth_method(&self, client_methods: &[u8]) -> u8 {
        if !self.require_auth && client_methods.contains(&AUTH_NO_AUTH) {
            return AUTH_NO_AUTH;
        }
        if self.require_auth && client_methods.contains(&AUTH_USERNAME_PASSWORD) {
            return AUTH_USERNAME_PASSWORD;
        }
        if client_methods.contains(&AUTH_NO_AUTH) && !self.require_auth {
            return AUTH_NO_AUTH;
        }
        AUTH_NO_ACCEPTABLE
    }

    fn authenticate(&self, username: &str, password: &str) -> bool {
        for (u, p) in &self.users {
            if u == username && p == password {
                return true;
            }
        }
        false
    }

    fn check_source_acl(&self, ip: [u8; 4]) -> bool {
        if self.source_acl.is_empty() {
            return true;
        }
        for entry in &self.source_acl {
            if entry.matches_ip(ip) {
                return entry.allow;
            }
        }
        false
    }

    fn check_dest_acl(&self, ip: [u8; 4], port: u16) -> bool {
        if self.dest_acl.is_empty() {
            return true;
        }
        for entry in &self.dest_acl {
            if entry.matches_ip(ip) {
                if let Some(p) = entry.port {
                    if p == port { return entry.allow; }
                } else {
                    return entry.allow;
                }
            }
        }
        false
    }

    fn create_session(&mut self, client_ip: [u8; 4], client_port: u16) -> u64 {
        let id = self.next_session_id;
        self.next_session_id += 1;
        if self.sessions.len() >= MAX_SESSIONS {
            // Remove oldest closed session
            if let Some(pos) = self.sessions.iter().position(|s| s.state == SessionState::Closed) {
                self.sessions.remove(pos);
            }
        }
        self.sessions.push(Socks5Session {
            id,
            state: SessionState::Handshake,
            client_ip,
            client_port,
            target_addr: None,
            target_port: 0,
            command: 0,
            auth_method: AUTH_NO_AUTH,
            username: None,
            bytes_tx: 0,
            bytes_rx: 0,
            created_tick: crate::timer::ticks(),
        });
        TOTAL_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
        ACTIVE_SESSIONS.fetch_add(1, Ordering::Relaxed);
        id
    }

    fn close_session(&mut self, id: u64) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.state = SessionState::Closed;
            ACTIVE_SESSIONS.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

static SERVER: Mutex<Socks5Server> = Mutex::new(Socks5Server::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the SOCKS5 proxy subsystem.
pub fn init() {
    INITIALIZED.store(true, Ordering::SeqCst);
    crate::serial_println!("[socks5] SOCKS5 proxy subsystem initialized");
}

/// Start listening on the given port.
pub fn start(port: u16) {
    let mut srv = SERVER.lock();
    srv.listen_port = port;
    RUNNING.store(true, Ordering::SeqCst);
    crate::serial_println!("[socks5] SOCKS5 proxy started on port {}", port);
}

/// Stop the SOCKS5 proxy.
pub fn stop() {
    RUNNING.store(false, Ordering::SeqCst);
    let mut srv = SERVER.lock();
    // Close all active sessions
    let ids: Vec<u64> = srv.sessions.iter()
        .filter(|s| s.state != SessionState::Closed)
        .map(|s| s.id)
        .collect();
    for id in ids {
        srv.close_session(id);
    }
    crate::serial_println!("[socks5] SOCKS5 proxy stopped");
}

/// List all active sessions as a formatted string.
pub fn list_sessions() -> String {
    let srv = SERVER.lock();
    let active: Vec<&Socks5Session> = srv.sessions.iter()
        .filter(|s| s.state != SessionState::Closed)
        .collect();
    if active.is_empty() {
        return "No active SOCKS5 sessions".to_owned();
    }
    let mut out = format!("Active SOCKS5 sessions ({}):\n", active.len());
    for s in &active {
        let target = s.target_addr.as_ref()
            .map(|a| a.to_string_repr())
            .unwrap_or_else(|| "-".to_owned());
        let cmd_str = match s.command {
            CMD_CONNECT => "CONNECT",
            CMD_BIND => "BIND",
            CMD_UDP_ASSOCIATE => "UDP",
            _ => "?",
        };
        out.push_str(&format!("  #{}: {}.{}.{}.{}:{} -> {}:{} [{}] tx={} rx={}\n",
            s.id,
            s.client_ip[0], s.client_ip[1], s.client_ip[2], s.client_ip[3],
            s.client_port, target, s.target_port,
            cmd_str, s.bytes_tx, s.bytes_rx));
    }
    out
}

/// Return proxy information string.
pub fn socks5_info() -> String {
    let srv = SERVER.lock();
    let running = RUNNING.load(Ordering::Relaxed);
    let active = srv.sessions.iter().filter(|s| s.state != SessionState::Closed).count();
    let auth_mode = if srv.require_auth { "username/password" } else { "none" };
    let chained = srv.upstream.is_some();
    format!(
        "SOCKS5 Proxy:\n  Status: {}\n  Port: {}\n  Auth: {}\n  Active sessions: {}\n  Upstream chain: {}\n  Source ACL rules: {}\n  Dest ACL rules: {}",
        if running { "running" } else { "stopped" },
        srv.listen_port,
        auth_mode,
        active,
        if chained { "yes" } else { "no" },
        srv.source_acl.len(),
        srv.dest_acl.len(),
    )
}

/// Return proxy statistics string.
pub fn socks5_stats() -> String {
    format!(
        "SOCKS5 Stats:\n  Total connections: {}\n  Active sessions: {}\n  Bytes TX: {}\n  Bytes RX: {}\n  Auth failures: {}\n  CONNECT requests: {}\n  BIND requests: {}\n  UDP ASSOCIATE requests: {}\n  Denied requests: {}",
        TOTAL_CONNECTIONS.load(Ordering::Relaxed),
        ACTIVE_SESSIONS.load(Ordering::Relaxed),
        BYTES_FORWARDED_TX.load(Ordering::Relaxed),
        BYTES_FORWARDED_RX.load(Ordering::Relaxed),
        AUTH_FAILURES.load(Ordering::Relaxed),
        CONNECT_REQUESTS.load(Ordering::Relaxed),
        BIND_REQUESTS.load(Ordering::Relaxed),
        UDP_ASSOCIATE_REQUESTS.load(Ordering::Relaxed),
        DENIED_REQUESTS.load(Ordering::Relaxed),
    )
}
