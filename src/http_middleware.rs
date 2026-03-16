/// HTTP middleware, request logging, and extended API endpoints for MerlionOS httpd.
/// Provides a middleware pipeline, access logging, rate limiting integration,
/// CORS headers, and additional system API endpoints.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

// --- Request/Response Logging ---

const MAX_ACCESS_LOG: usize = 256;

/// An access log entry.
#[derive(Debug, Clone)]
pub struct AccessLogEntry {
    pub timestamp: u64,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub response_size: usize,
    pub client_ip: [u8; 4],
    pub duration_ticks: u64,
}

static ACCESS_LOG: Mutex<Vec<AccessLogEntry>> = Mutex::new(Vec::new());
static REQUEST_COUNT: AtomicU64 = AtomicU64::new(0);
static BYTES_SERVED: AtomicU64 = AtomicU64::new(0);
static ERROR_COUNT: AtomicU64 = AtomicU64::new(0);

/// Log an HTTP request/response.
pub fn log_access(method: &str, path: &str, status: u16, size: usize, client_ip: [u8; 4], duration: u64) {
    REQUEST_COUNT.fetch_add(1, Ordering::Relaxed);
    BYTES_SERVED.fetch_add(size as u64, Ordering::Relaxed);
    if status >= 400 { ERROR_COUNT.fetch_add(1, Ordering::Relaxed); }

    if let Some(mut log) = ACCESS_LOG.try_lock() {
        if log.len() >= MAX_ACCESS_LOG { log.remove(0); }
        log.push(AccessLogEntry {
            timestamp: crate::timer::ticks(),
            method: method.to_owned(),
            path: path.to_owned(),
            status,
            response_size: size,
            client_ip,
            duration_ticks: duration,
        });
    }

    crate::serial_println!(
        "[httpd] {}.{}.{}.{} {} {} {} {}B {}t",
        client_ip[0], client_ip[1], client_ip[2], client_ip[3],
        method, path, status, size, duration
    );
}

/// Get recent access log entries.
pub fn get_access_log(count: usize) -> Vec<AccessLogEntry> {
    let log = match ACCESS_LOG.try_lock() {
        Some(l) => l,
        None => return Vec::new(),
    };
    let start = if log.len() > count { log.len() - count } else { 0 };
    log[start..].to_vec()
}

/// Format access log as a string.
pub fn format_access_log(count: usize) -> String {
    let entries = get_access_log(count);
    let mut out = format!("HTTP access log (last {}):\n", entries.len());
    out.push_str(&format!("{:<10} {:<16} {:<6} {:<20} {:>6} {:>8}\n",
        "Tick", "Client", "Method", "Path", "Status", "Size"));

    for e in &entries {
        let ip = format!("{}.{}.{}.{}", e.client_ip[0], e.client_ip[1], e.client_ip[2], e.client_ip[3]);
        let path = if e.path.len() > 20 { format!("{}...", &e.path[..17]) } else { e.path.clone() };
        out.push_str(&format!("{:<10} {:<16} {:<6} {:<20} {:>6} {:>8}\n",
            e.timestamp, ip, e.method, path, e.status, e.response_size));
    }
    out
}

// --- HTTP Statistics ---

/// Get server statistics.
pub fn server_stats() -> String {
    let reqs = REQUEST_COUNT.load(Ordering::Relaxed);
    let bytes = BYTES_SERVED.load(Ordering::Relaxed);
    let errs = ERROR_COUNT.load(Ordering::Relaxed);

    format!(
        "HTTP Server Statistics:\n  Requests: {}\n  Bytes served: {}\n  Errors (4xx/5xx): {}\n  Success rate: {}%",
        reqs, bytes, errs,
        if reqs > 0 { ((reqs - errs) * 100) / reqs } else { 100 }
    )
}

// --- Middleware System ---

/// Middleware action result.
#[derive(Debug, Clone, PartialEq)]
pub enum MiddlewareAction {
    /// Continue to next middleware / handler.
    Continue,
    /// Short-circuit with this response body and status.
    Respond(u16, String),
    /// Reject the request (403).
    Reject,
}

/// A middleware entry.
#[derive(Clone)]
pub struct Middleware {
    pub name: &'static str,
    pub priority: u8,
    pub enabled: bool,
}

const MAX_MIDDLEWARE: usize = 16;

static MIDDLEWARE: Mutex<Vec<Middleware>> = Mutex::new(Vec::new());

/// Register the default middleware stack.
pub fn init() {
    let mut mw = MIDDLEWARE.lock();
    mw.push(Middleware { name: "logging", priority: 0, enabled: true });
    mw.push(Middleware { name: "cors", priority: 1, enabled: true });
    mw.push(Middleware { name: "rate-limit", priority: 2, enabled: false });
    mw.push(Middleware { name: "auth", priority: 3, enabled: false });

    crate::serial_println!("[http_middleware] initialized with {} middleware", mw.len());
    crate::klog_println!("[http_middleware] initialized");
}

/// Enable or disable a middleware by name.
pub fn set_middleware(name: &str, enabled: bool) {
    let mut mw = MIDDLEWARE.lock();
    if let Some(m) = mw.iter_mut().find(|m| m.name == name) {
        m.enabled = enabled;
    }
}

/// List all middleware.
pub fn list_middleware() -> String {
    let mw = MIDDLEWARE.lock();
    let mut out = String::from("HTTP Middleware:\n");
    for m in mw.iter() {
        out.push_str(&format!("  [{}] {} (priority {})\n",
            if m.enabled { "ON " } else { "OFF" }, m.name, m.priority));
    }
    out
}

// --- CORS Headers ---

/// Generate CORS headers.
pub fn cors_headers() -> String {
    String::from(
        "Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type\r\n"
    )
}

// --- Extended API Endpoints ---

/// Generate JSON for /api/memory endpoint.
pub fn api_memory() -> String {
    let stats = crate::allocator::stats();
    format!(
        "{{\"total\":{},\"used\":{},\"free\":{}}}",
        stats.total, stats.used, stats.free
    )
}

/// Generate JSON for /api/security endpoint.
pub fn api_security() -> String {
    let uid = crate::security::current_uid();
    let user = crate::security::whoami();
    let users = crate::security::list_users();

    let mut json = format!("{{\"current_user\":\"{}\",\"uid\":{},\"users\":[", user, uid);
    for (i, (uid, name)) in users.iter().enumerate() {
        if i > 0 { json.push(','); }
        json.push_str(&format!("{{\"uid\":{},\"name\":\"{}\"}}", uid, name));
    }
    json.push_str("]}");
    json
}

/// Generate JSON for /api/network endpoint.
pub fn api_network() -> String {
    let net = crate::net::NET.lock();
    let mac = net.mac.0;
    let ip = net.ip.0;
    format!(
        "{{\"mac\":\"{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\",\"ip\":\"{}.{}.{}.{}\"}}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
        ip[0], ip[1], ip[2], ip[3]
    )
}

/// Generate JSON for /api/logs endpoint.
pub fn api_logs() -> String {
    let entries = crate::structured_log::query(20);
    crate::structured_log::format_json(&entries)
}

/// Generate JSON for /api/perf endpoint.
pub fn api_perf() -> String {
    let counters = crate::profiler::perf_stat();
    format!(
        "{{\"ctx_switches\":{},\"syscalls\":{},\"page_faults\":{},\"interrupts\":{}}}",
        counters.context_switch_count, counters.syscall_count,
        counters.page_fault_count, counters.interrupt_count
    )
}

/// Route an API request to the appropriate handler.
/// Returns (status_code, content_type, body).
pub fn route_api(path: &str) -> (u16, &'static str, String) {
    match path {
        "/api/memory" => (200, "application/json", api_memory()),
        "/api/security" => (200, "application/json", api_security()),
        "/api/network" => (200, "application/json", api_network()),
        "/api/logs" => (200, "application/json", api_logs()),
        "/api/perf" => (200, "application/json", api_perf()),
        "/api/middleware" => (200, "application/json", format!("{{\"middleware\":\"{}\"}}", list_middleware().replace('\n', "\\n"))),
        "/api/httpd-stats" => (200, "application/json", format!(
            "{{\"requests\":{},\"bytes\":{},\"errors\":{}}}",
            REQUEST_COUNT.load(Ordering::Relaxed),
            BYTES_SERVED.load(Ordering::Relaxed),
            ERROR_COUNT.load(Ordering::Relaxed),
        )),
        _ => (404, "application/json", "{\"error\":\"not found\"}".to_owned()),
    }
}

// --- Virtual Hosts ---

const MAX_VHOSTS: usize = 8;

/// A virtual host mapping.
#[derive(Clone)]
pub struct VirtualHost {
    pub hostname: String,
    pub root_path: String,
}

static VHOSTS: Mutex<Vec<VirtualHost>> = Mutex::new(Vec::new());

/// Add a virtual host.
pub fn add_vhost(hostname: &str, root_path: &str) -> Result<(), &'static str> {
    let mut vhosts = VHOSTS.lock();
    if vhosts.len() >= MAX_VHOSTS {
        return Err("max virtual hosts reached");
    }
    vhosts.push(VirtualHost {
        hostname: hostname.to_owned(),
        root_path: root_path.to_owned(),
    });
    Ok(())
}

/// Resolve a virtual host to its root path.
pub fn resolve_vhost(hostname: &str) -> Option<String> {
    let vhosts = VHOSTS.lock();
    vhosts.iter().find(|v| v.hostname == hostname).map(|v| v.root_path.clone())
}

/// List virtual hosts.
pub fn list_vhosts() -> String {
    let vhosts = VHOSTS.lock();
    if vhosts.is_empty() {
        return String::from("No virtual hosts configured.\n");
    }
    let mut out = String::from("Virtual hosts:\n");
    for v in vhosts.iter() {
        out.push_str(&format!("  {} -> {}\n", v.hostname, v.root_path));
    }
    out
}
