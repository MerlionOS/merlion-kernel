/// Remote syslog client for MerlionOS.
/// Sends log messages to a remote syslog server over UDP (port 514).
/// Implements a simplified RFC 5424 message format.
///
/// # Usage
/// ```
/// remote_log::set_server([192, 168, 1, 100]);
/// remote_log::send(Severity::Info, "kern", "System booted");
/// ```

use alloc::string::String;
use alloc::borrow::ToOwned;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Default syslog UDP port.
const SYSLOG_PORT: u16 = 514;
/// Local source port for syslog.
const LOCAL_PORT: u16 = 50514;
/// Max message size (UDP payload).
const MAX_MSG_SIZE: usize = 1024;
/// Buffer for queued messages when server is not yet set.
const QUEUE_SIZE: usize = 32;

/// Whether remote logging is enabled.
static ENABLED: AtomicBool = AtomicBool::new(false);
/// Remote server IP (packed as u32: a.b.c.d -> [a, b, c, d]).
static SERVER_IP: Mutex<[u8; 4]> = Mutex::new([0; 4]);
/// Custom port override (0 = use default 514).
static SERVER_PORT: Mutex<u16> = Mutex::new(SYSLOG_PORT);
/// Hostname to include in messages.
static HOSTNAME: Mutex<Option<String>> = Mutex::new(None);

/// Messages sent counter.
static SENT_COUNT: AtomicU32 = AtomicU32::new(0);
/// Messages failed counter.
static FAIL_COUNT: AtomicU32 = AtomicU32::new(0);

/// Syslog facility codes (RFC 5424).
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum Facility {
    Kern     = 0,   // kernel messages
    User     = 1,   // user-level messages
    Daemon   = 3,   // system daemons
    Auth     = 4,   // security/authorization
    Syslog   = 5,   // syslogd internal
    Local0   = 16,  // local use 0
    Local1   = 17,  // local use 1
    Local7   = 23,  // local use 7
}

/// Severity levels matching RFC 5424.
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum Severity {
    Emergency = 0,
    Alert     = 1,
    Critical  = 2,
    Error     = 3,
    Warning   = 4,
    Notice    = 5,
    Info      = 6,
    Debug     = 7,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Emergency => "emerg",
            Severity::Alert     => "alert",
            Severity::Critical  => "crit",
            Severity::Error     => "err",
            Severity::Warning   => "warning",
            Severity::Notice    => "notice",
            Severity::Info      => "info",
            Severity::Debug     => "debug",
        }
    }
}

/// Initialize remote logging (disabled by default).
pub fn init() {
    set_hostname("merlion");
    crate::serial_println!("[remote_log] initialized (disabled, set server to enable)");
    crate::klog_println!("[remote_log] initialized");
}

/// Set the remote syslog server IP address and enable remote logging.
pub fn set_server(ip: [u8; 4]) {
    *SERVER_IP.lock() = ip;
    ENABLED.store(true, Ordering::SeqCst);
    crate::serial_println!(
        "[remote_log] server set to {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]
    );
}

/// Disable remote logging.
pub fn disable() {
    ENABLED.store(false, Ordering::SeqCst);
    crate::serial_println!("[remote_log] disabled");
}

/// Enable remote logging (server must be set first).
pub fn enable() {
    let ip = *SERVER_IP.lock();
    if ip == [0, 0, 0, 0] {
        crate::serial_println!("[remote_log] cannot enable: no server set");
        return;
    }
    ENABLED.store(true, Ordering::SeqCst);
}

/// Set custom server port (default is 514).
pub fn set_port(port: u16) {
    *SERVER_PORT.lock() = port;
}

/// Set the hostname used in syslog messages.
pub fn set_hostname(name: &str) {
    *HOSTNAME.lock() = Some(name.to_owned());
}

/// Build an RFC 5424-style syslog message.
///
/// Format: <PRI>VERSION TIMESTAMP HOSTNAME APP-NAME PROCID MSGID MSG
/// Simplified: <PRI>1 - HOSTNAME merlion PID - MSG
fn build_message(facility: Facility, severity: Severity, module: &str, message: &str) -> String {
    // PRI = facility * 8 + severity
    let pri = (facility as u8) * 8 + (severity as u8);

    let hostname = HOSTNAME.lock().clone().unwrap_or_else(|| "merlion".to_owned());
    let pid = crate::task::current_pid();

    // Get uptime as timestamp substitute (no RTC formatting needed for syslog)
    let ticks = crate::timer::ticks();
    let secs = ticks / 100;

    // RFC 5424 format (simplified)
    format!(
        "<{}>1 {}s {} {} {} - - {}",
        pri, secs, hostname, module, pid, message
    )
}

/// Send a syslog message to the remote server.
pub fn send(severity: Severity, module: &str, message: &str) -> bool {
    if !ENABLED.load(Ordering::SeqCst) {
        return false;
    }

    send_with_facility(Facility::Kern, severity, module, message)
}

/// Send a syslog message with a specific facility.
pub fn send_with_facility(facility: Facility, severity: Severity, module: &str, message: &str) -> bool {
    if !ENABLED.load(Ordering::SeqCst) {
        return false;
    }

    let msg = build_message(facility, severity, module, message);
    let bytes = msg.as_bytes();

    if bytes.len() > MAX_MSG_SIZE {
        // Truncate
        let truncated = &bytes[..MAX_MSG_SIZE];
        send_udp_packet(truncated)
    } else {
        send_udp_packet(bytes)
    }
}

/// Actually send the UDP packet.
fn send_udp_packet(data: &[u8]) -> bool {
    let server_ip = *SERVER_IP.lock();
    let port = *SERVER_PORT.lock();

    let ok = crate::netstack::send_udp(server_ip, LOCAL_PORT, port, data);

    if ok {
        SENT_COUNT.fetch_add(1, Ordering::Relaxed);
    } else {
        FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    ok
}

/// Send an auth event to remote syslog.
pub fn send_auth(severity: Severity, message: &str) -> bool {
    send_with_facility(Facility::Auth, severity, "auth", message)
}

/// Send a daemon event to remote syslog.
pub fn send_daemon(severity: Severity, message: &str) -> bool {
    send_with_facility(Facility::Daemon, severity, "daemon", message)
}

/// Get remote syslog status as a formatted string.
pub fn status() -> String {
    let enabled = ENABLED.load(Ordering::SeqCst);
    let ip = *SERVER_IP.lock();
    let port = *SERVER_PORT.lock();
    let sent = SENT_COUNT.load(Ordering::Relaxed);
    let failed = FAIL_COUNT.load(Ordering::Relaxed);

    if enabled {
        format!(
            "Remote syslog: enabled\nServer: {}.{}.{}.{}:{}\nSent: {} | Failed: {}",
            ip[0], ip[1], ip[2], ip[3], port, sent, failed
        )
    } else {
        format!("Remote syslog: disabled\nSent: {} | Failed: {}", sent, failed)
    }
}

/// Check if remote logging is enabled.
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}
