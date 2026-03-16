/// Structured logging framework with audit trail for MerlionOS.
///
/// Provides JSON-formatted structured log entries with key-value context fields,
/// severity levels compatible with syslog RFC 5424, query/filter capabilities,
/// and a dedicated audit trail for security-relevant events.

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// 1. Structured Log Entry
// ---------------------------------------------------------------------------

/// Monotonically increasing log sequence number.
static LOG_SEQ: AtomicU64 = AtomicU64::new(1);

/// Log severity levels compatible with syslog RFC 5424.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Severity {
    /// System is unusable.
    Emergency = 0,
    /// Action must be taken immediately.
    Alert = 1,
    /// Critical conditions.
    Critical = 2,
    /// Error conditions.
    Error = 3,
    /// Warning conditions.
    Warning = 4,
    /// Normal but significant condition.
    Notice = 5,
    /// Informational messages.
    Info = 6,
    /// Debug-level messages.
    Debug = 7,
}

impl Severity {
    /// Return a short human-readable label for this severity.
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Emergency => "EMERG",
            Severity::Alert => "ALERT",
            Severity::Critical => "CRIT",
            Severity::Error => "ERROR",
            Severity::Warning => "WARN",
            Severity::Notice => "NOTICE",
            Severity::Info => "INFO",
            Severity::Debug => "DEBUG",
        }
    }

    /// Convert a raw `u8` value to a `Severity`, clamping unknown values to `Debug`.
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Severity::Emergency,
            1 => Severity::Alert,
            2 => Severity::Critical,
            3 => Severity::Error,
            4 => Severity::Warning,
            5 => Severity::Notice,
            6 => Severity::Info,
            _ => Severity::Debug,
        }
    }
}

/// A context field (key-value pair) attached to a log entry.
#[derive(Debug, Clone)]
pub struct Field {
    pub key: String,
    pub value: String,
}

/// A structured log entry with metadata and context fields.
#[derive(Debug, Clone)]
pub struct StructuredEntry {
    /// Monotonically increasing sequence number.
    pub seq: u64,
    /// Timestamp in PIT timer ticks since boot.
    pub timestamp_ticks: u64,
    /// Log severity.
    pub severity: Severity,
    /// Syslog-style facility (e.g. "kern", "auth", "daemon", "user").
    pub facility: &'static str,
    /// Originating module (e.g. "security", "vfs", "net").
    pub module: &'static str,
    /// Human-readable message.
    pub message: String,
    /// Arbitrary key-value context fields.
    pub fields: Vec<Field>,
    /// UID of the user who triggered this event.
    pub uid: u32,
    /// PID of the process context.
    pub pid: usize,
}

// ---------------------------------------------------------------------------
// 2. Structured Log Buffer
// ---------------------------------------------------------------------------

/// Maximum number of structured log entries retained in memory.
const STRUCTURED_LOG_CAPACITY: usize = 512;

/// In-memory structured log buffer (bounded, drops oldest on overflow).
static STRUCTURED_LOG: Mutex<Vec<StructuredEntry>> = Mutex::new(Vec::new());

/// Total entries ever written (used for diagnostics).
static LOG_WRITE_POS: AtomicU64 = AtomicU64::new(0);

/// Minimum severity level to store. Entries with a numeric severity value
/// greater than this (i.e. less severe) are silently dropped.
static MIN_SEVERITY: Mutex<Severity> = Mutex::new(Severity::Debug);

/// Set the minimum severity filter. Only entries at this level or more severe
/// will be stored and echoed.
pub fn set_min_severity(sev: Severity) {
    *MIN_SEVERITY.lock() = sev;
}

/// Return the current minimum severity filter.
pub fn get_min_severity() -> Severity {
    *MIN_SEVERITY.lock()
}

/// Create and store a structured log entry.
///
/// Automatically fills in the sequence number, timestamp (from `crate::timer`),
/// uid (currently always 0), and pid (from `crate::task`). The entry is also
/// echoed to the serial console for early-boot and headless debugging.
///
/// Entries whose severity exceeds (is less urgent than) the configured minimum
/// are silently discarded.
pub fn log_structured(
    severity: Severity,
    facility: &'static str,
    module: &'static str,
    message: String,
    fields: Vec<Field>,
) {
    // Filter: severity numerically higher means *less* severe.
    if severity > get_min_severity() {
        return;
    }

    let seq = LOG_SEQ.fetch_add(1, Ordering::Relaxed);
    let timestamp_ticks = crate::timer::ticks();
    let pid = crate::task::current_pid();
    let uid: u32 = 0; // TODO: retrieve from process credentials when available

    let entry = StructuredEntry {
        seq,
        timestamp_ticks,
        severity,
        facility,
        module,
        message,
        fields,
        uid,
        pid,
    };

    // Echo to serial console.
    crate::serial_println!("{}", to_text(&entry));

    // Store in ring buffer.
    let mut log = STRUCTURED_LOG.lock();
    if log.len() >= STRUCTURED_LOG_CAPACITY {
        log.remove(0);
    }
    log.push(entry);
    LOG_WRITE_POS.fetch_add(1, Ordering::Relaxed);
}

/// Return the last `count` log entries (most recent last).
pub fn query(count: usize) -> Vec<StructuredEntry> {
    let log = STRUCTURED_LOG.lock();
    let start = log.len().saturating_sub(count);
    log[start..].to_vec()
}

/// Return the last `count` entries whose severity is at least as severe as `sev`.
pub fn query_by_severity(sev: Severity, count: usize) -> Vec<StructuredEntry> {
    let log = STRUCTURED_LOG.lock();
    let filtered: Vec<StructuredEntry> = log
        .iter()
        .filter(|e| e.severity <= sev)
        .cloned()
        .collect();
    let start = filtered.len().saturating_sub(count);
    filtered[start..].to_vec()
}

/// Return the last `count` entries matching the given facility string.
pub fn query_by_facility(facility: &str, count: usize) -> Vec<StructuredEntry> {
    let log = STRUCTURED_LOG.lock();
    let filtered: Vec<StructuredEntry> = log
        .iter()
        .filter(|e| e.facility == facility)
        .cloned()
        .collect();
    let start = filtered.len().saturating_sub(count);
    filtered[start..].to_vec()
}

/// Return the last `count` entries matching the given module name.
pub fn query_by_module(module: &str, count: usize) -> Vec<StructuredEntry> {
    let log = STRUCTURED_LOG.lock();
    let filtered: Vec<StructuredEntry> = log
        .iter()
        .filter(|e| e.module == module)
        .cloned()
        .collect();
    let start = filtered.len().saturating_sub(count);
    filtered[start..].to_vec()
}

/// Clear all entries from the structured log buffer.
pub fn clear() {
    STRUCTURED_LOG.lock().clear();
}

// ---------------------------------------------------------------------------
// 3. JSON Formatter
// ---------------------------------------------------------------------------

/// Escape a string for safe inclusion in a JSON value.
fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

/// Format a structured entry as a JSON object string.
pub fn to_json(entry: &StructuredEntry) -> String {
    let mut json = format!(
        "{{\"seq\":{},\"ts\":{},\"sev\":\"{}\",\"fac\":\"{}\",\"mod\":\"{}\",\"uid\":{},\"pid\":{},\"msg\":\"{}\"",
        entry.seq,
        entry.timestamp_ticks,
        entry.severity.as_str(),
        entry.facility,
        entry.module,
        entry.uid,
        entry.pid,
        escape_json(&entry.message)
    );

    if !entry.fields.is_empty() {
        json.push_str(",\"ctx\":{");
        for (i, field) in entry.fields.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            json.push_str(&format!(
                "\"{}\":\"{}\"",
                escape_json(&field.key),
                escape_json(&field.value)
            ));
        }
        json.push('}');
    }

    json.push('}');
    json
}

/// Format a structured entry as a human-readable single line.
pub fn to_text(entry: &StructuredEntry) -> String {
    let mut text = format!(
        "[{:>8}] {} {}/{}: {}",
        entry.timestamp_ticks,
        entry.severity.as_str(),
        entry.facility,
        entry.module,
        entry.message
    );
    for field in &entry.fields {
        text.push_str(&format!(" {}={}", field.key, field.value));
    }
    text
}

/// Format multiple entries as human-readable text, one per line.
pub fn format_text(entries: &[StructuredEntry]) -> String {
    let mut out = String::new();
    for entry in entries {
        out.push_str(&to_text(entry));
        out.push('\n');
    }
    out
}

/// Format multiple entries as a JSON array.
pub fn format_json(entries: &[StructuredEntry]) -> String {
    let mut out = String::from("[");
    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&to_json(entry));
    }
    out.push(']');
    out
}

// ---------------------------------------------------------------------------
// 4. Audit Trail
// ---------------------------------------------------------------------------

/// Audit trail categories for security-relevant events.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AuditCategory {
    /// Login, logout, authentication events.
    Auth,
    /// File access events.
    Access,
    /// Administrative actions (user add/remove, chmod, etc.).
    Admin,
    /// Process lifecycle (spawn, kill, exit).
    Process,
    /// Network events (connect, listen, etc.).
    Network,
    /// Security policy events (capability, seccomp).
    Security,
}

impl AuditCategory {
    /// Short label for the audit category.
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditCategory::Auth => "AUTH",
            AuditCategory::Access => "ACCESS",
            AuditCategory::Admin => "ADMIN",
            AuditCategory::Process => "PROCESS",
            AuditCategory::Network => "NETWORK",
            AuditCategory::Security => "SECURITY",
        }
    }
}

/// Per-category audit event counters.
static AUDIT_COUNTS: Mutex<[usize; 6]> = Mutex::new([0; 6]);

/// Increment the counter for a given audit category.
fn increment_audit(cat: AuditCategory) {
    let mut counts = AUDIT_COUNTS.lock();
    let idx = cat as usize;
    if idx < 6 {
        counts[idx] += 1;
    }
}

/// Log an audit event.
///
/// This is a convenience wrapper that writes to the structured log with
/// facility `"auth"`, attaches an `audit_cat` context field, and increments
/// the per-category counter.
pub fn audit(
    category: AuditCategory,
    module: &'static str,
    message: String,
    extra_fields: Vec<Field>,
) {
    increment_audit(category);

    let mut fields = alloc::vec![
        Field { key: "audit_cat".to_owned(), value: category.as_str().to_owned() },
    ];
    fields.extend(extra_fields);

    let severity = match category {
        AuditCategory::Auth | AuditCategory::Security => Severity::Notice,
        _ => Severity::Info,
    };

    log_structured(severity, "auth", module, message, fields);
}

/// Query audit trail entries (entries with facility `"auth"`).
pub fn audit_trail(count: usize) -> Vec<StructuredEntry> {
    query_by_facility("auth", count)
}

/// Return a human-readable summary of audit event counts.
pub fn audit_stats() -> String {
    let counts = AUDIT_COUNTS.lock();
    format!(
        "Audit stats: auth={} access={} admin={} process={} network={} security={}",
        counts[0], counts[1], counts[2], counts[3], counts[4], counts[5]
    )
}

// ---------------------------------------------------------------------------
// 5. Convenience Functions
// ---------------------------------------------------------------------------

/// Log an informational message from the `"kern"` facility.
pub fn info(module: &'static str, msg: impl Into<String>) {
    log_structured(Severity::Info, "kern", module, msg.into(), Vec::new());
}

/// Log a warning from the `"kern"` facility.
pub fn warn(module: &'static str, msg: impl Into<String>) {
    log_structured(Severity::Warning, "kern", module, msg.into(), Vec::new());
}

/// Log an error from the `"kern"` facility.
pub fn error(module: &'static str, msg: impl Into<String>) {
    log_structured(Severity::Error, "kern", module, msg.into(), Vec::new());
}

/// Log a debug message from the `"kern"` facility.
pub fn debug(module: &'static str, msg: impl Into<String>) {
    log_structured(Severity::Debug, "kern", module, msg.into(), Vec::new());
}

/// Log an informational message with context fields.
pub fn info_with(module: &'static str, msg: impl Into<String>, fields: Vec<Field>) {
    log_structured(Severity::Info, "kern", module, msg.into(), fields);
}

/// Log a warning with context fields.
pub fn warn_with(module: &'static str, msg: impl Into<String>, fields: Vec<Field>) {
    log_structured(Severity::Warning, "kern", module, msg.into(), fields);
}

/// Create a [`Field`] from a key-value pair.
pub fn field(key: &str, value: &str) -> Field {
    Field {
        key: key.to_owned(),
        value: value.to_owned(),
    }
}

/// Initialize the structured logging subsystem.
pub fn init() {
    info("structured_log", "Structured logging initialized");
}
