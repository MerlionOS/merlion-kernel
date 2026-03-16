/// Capability-based security and seccomp-like syscall filtering for MerlionOS.
///
/// Provides three subsystems:
/// 1. **Capability system** — Linux-style capability bits replacing raw uid==0
///    checks. Each process carries effective, permitted, and inheritable sets.
/// 2. **Seccomp-like syscall filtering** — per-process bitmask controlling which
///    syscall numbers are allowed, with configurable default actions.
/// 3. **Audit logging** — security event logging to the kernel ring buffer and
///    serial console with atomic event counters.
///
/// Thread-safe via `spin::Mutex`; suitable for `#![no_std]` kernel use.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicUsize, Ordering};
use crate::{klog_println, serial_println};

// ---------------------------------------------------------------------------
// 1. Capability System
// ---------------------------------------------------------------------------

/// Bypass file permission checks.
pub const CAP_DAC_OVERRIDE: u64 = 1 << 0;
/// Send signals to any process.
pub const CAP_KILL: u64 = 1 << 1;
/// Network administration.
pub const CAP_NET_ADMIN: u64 = 1 << 2;
/// Use raw sockets.
pub const CAP_NET_RAW: u64 = 1 << 3;
/// General system administration.
pub const CAP_SYS_ADMIN: u64 = 1 << 4;
/// Reboot / shutdown.
pub const CAP_SYS_BOOT: u64 = 1 << 5;
/// Load / unload kernel modules.
pub const CAP_SYS_MODULE: u64 = 1 << 6;
/// Set system time.
pub const CAP_SYS_TIME: u64 = 1 << 7;
/// Set user ID.
pub const CAP_SETUID: u64 = 1 << 8;
/// Set group ID.
pub const CAP_SETGID: u64 = 1 << 9;
/// Change file ownership.
pub const CAP_CHOWN: u64 = 1 << 10;
/// Bypass ownership checks on files you own.
pub const CAP_FOWNER: u64 = 1 << 11;
/// Bind to privileged ports (< 1024).
pub const CAP_NET_BIND: u64 = 1 << 12;
/// Audit / logging administration.
pub const CAP_AUDIT: u64 = 1 << 13;

/// All capabilities combined.
pub const CAP_ALL: u64 = (1 << 14) - 1;

/// Maximum number of processes tracked in the capability table.
const MAX_PROC_CAPS: usize = 64;

/// Per-process capability sets.
#[derive(Clone)]
struct ProcCaps {
    pid: usize,
    /// Currently active capabilities.
    effective: u64,
    /// Maximum capabilities this process may activate.
    permitted: u64,
    /// Capabilities inherited by child processes.
    inheritable: u64,
}

/// Global capability table.
static CAPS_TABLE: Mutex<Vec<ProcCaps>> = Mutex::new(Vec::new());

/// Mapping from single-bit capability constants to human-readable names.
const CAP_NAMES: &[(u64, &str)] = &[
    (CAP_DAC_OVERRIDE, "DAC_OVERRIDE"),
    (CAP_KILL, "KILL"),
    (CAP_NET_ADMIN, "NET_ADMIN"),
    (CAP_NET_RAW, "NET_RAW"),
    (CAP_SYS_ADMIN, "SYS_ADMIN"),
    (CAP_SYS_BOOT, "SYS_BOOT"),
    (CAP_SYS_MODULE, "SYS_MODULE"),
    (CAP_SYS_TIME, "SYS_TIME"),
    (CAP_SETUID, "SETUID"),
    (CAP_SETGID, "SETGID"),
    (CAP_CHOWN, "CHOWN"),
    (CAP_FOWNER, "FOWNER"),
    (CAP_NET_BIND, "NET_BIND"),
    (CAP_AUDIT, "AUDIT"),
];

/// Convert a single capability bit to its name.
pub fn cap_name(cap: u64) -> &'static str {
    for &(bit, name) in CAP_NAMES {
        if cap == bit {
            return name;
        }
    }
    "UNKNOWN"
}

/// Initialize the capability subsystem. Adds pid 0 (kernel) with all caps.
pub fn init() {
    let mut table = CAPS_TABLE.lock();
    table.clear();
    table.push(ProcCaps {
        pid: 0,
        effective: CAP_ALL,
        permitted: CAP_ALL,
        inheritable: CAP_ALL,
    });
    serial_println!("[capability] initialized, pid 0 has CAP_ALL");
    klog_println!("[capability] initialized");
}

/// Set the full capability sets for a process.
pub fn set_caps(pid: usize, effective: u64, permitted: u64, inheritable: u64) {
    let mut table = CAPS_TABLE.lock();
    for entry in table.iter_mut() {
        if entry.pid == pid {
            entry.effective = effective & permitted; // effective must be subset of permitted
            entry.permitted = permitted;
            entry.inheritable = inheritable;
            return;
        }
    }
    if table.len() < MAX_PROC_CAPS {
        table.push(ProcCaps {
            pid,
            effective: effective & permitted,
            permitted,
            inheritable,
        });
    }
}

/// Get the (effective, permitted, inheritable) capability sets for a process.
/// Returns `(0, 0, 0)` if the process has no entry.
pub fn get_caps(pid: usize) -> (u64, u64, u64) {
    let table = CAPS_TABLE.lock();
    for entry in table.iter() {
        if entry.pid == pid {
            return (entry.effective, entry.permitted, entry.inheritable);
        }
    }
    (0, 0, 0)
}

/// Check whether `pid` has a specific capability in its effective set.
pub fn has_cap(pid: usize, cap: u64) -> bool {
    let table = CAPS_TABLE.lock();
    for entry in table.iter() {
        if entry.pid == pid {
            return (entry.effective & cap) == cap;
        }
    }
    false
}

/// Grant a capability to a process (adds to both effective and permitted sets).
/// The caller is assumed to hold `CAP_SYS_ADMIN`; this must be checked by the
/// calling code before invoking this function.
pub fn grant_cap(pid: usize, cap: u64) {
    let mut table = CAPS_TABLE.lock();
    for entry in table.iter_mut() {
        if entry.pid == pid {
            entry.effective |= cap;
            entry.permitted |= cap;
            return;
        }
    }
    // Process not in table yet — add it with only this cap.
    if table.len() < MAX_PROC_CAPS {
        table.push(ProcCaps {
            pid,
            effective: cap,
            permitted: cap,
            inheritable: 0,
        });
    }
}

/// Revoke a capability from a process (removes from effective set only).
/// The caller is assumed to hold `CAP_SYS_ADMIN`.
pub fn revoke_cap(pid: usize, cap: u64) {
    let mut table = CAPS_TABLE.lock();
    for entry in table.iter_mut() {
        if entry.pid == pid {
            entry.effective &= !cap;
            return;
        }
    }
}

/// Inherit capabilities from parent to child.
/// The child receives the parent's inheritable set as both its permitted and
/// effective sets; the child's inheritable set is also copied from the parent.
pub fn inherit_caps(parent_pid: usize, child_pid: usize) {
    let table = CAPS_TABLE.lock();
    let mut parent_inh = 0u64;
    for entry in table.iter() {
        if entry.pid == parent_pid {
            parent_inh = entry.inheritable;
            break;
        }
    }
    drop(table);
    set_caps(child_pid, parent_inh, parent_inh, parent_inh);
}

/// Remove the capability entry for a process (on exit).
pub fn drop_caps(pid: usize) {
    let mut table = CAPS_TABLE.lock();
    table.retain(|e| e.pid != pid);
}

/// Return a human-readable list of effective capabilities for a process.
pub fn list_caps(pid: usize) -> String {
    let (eff, _perm, _inh) = get_caps(pid);
    if eff == 0 {
        return String::from("(none)");
    }
    if eff == CAP_ALL {
        return String::from("ALL");
    }
    let mut names = Vec::new();
    for &(bit, name) in CAP_NAMES {
        if eff & bit != 0 {
            names.push(name);
        }
    }
    let mut out = String::new();
    for (i, name) in names.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(name);
    }
    out
}

// ---------------------------------------------------------------------------
// 2. Seccomp-like Syscall Filtering
// ---------------------------------------------------------------------------

/// Maximum number of distinct syscall numbers addressable by the bitmask.
const MAX_SYSCALLS: usize = 64;

/// Maximum number of per-process filters.
const MAX_FILTERS: usize = 32;

/// Action to take when a syscall is checked against the filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterAction {
    /// Allow the syscall to proceed.
    Allow,
    /// Kill the process immediately.
    Kill,
    /// Allow the syscall but log the event.
    Log,
}

/// Per-process syscall filter.
#[derive(Clone)]
struct SyscallFilter {
    pid: usize,
    /// Bitmask: bit N set means syscall N is allowed.
    allowed: u64,
    /// Action taken when a syscall is not in the allowed mask.
    default_action: FilterAction,
}

/// Global filter table.
static FILTERS: Mutex<Vec<SyscallFilter>> = Mutex::new(Vec::new());

/// Initialize the seccomp subsystem.
pub fn seccomp_init() {
    let mut filters = FILTERS.lock();
    filters.clear();
    serial_println!("[seccomp] initialized");
    klog_println!("[seccomp] initialized");
}

/// Set or update the syscall filter for a process.
pub fn seccomp_set(pid: usize, allowed_mask: u64, default_action: FilterAction) {
    let mut filters = FILTERS.lock();
    for f in filters.iter_mut() {
        if f.pid == pid {
            f.allowed = allowed_mask;
            f.default_action = default_action;
            return;
        }
    }
    if filters.len() < MAX_FILTERS {
        filters.push(SyscallFilter {
            pid,
            allowed: allowed_mask,
            default_action,
        });
    }
}

/// Check whether a syscall is allowed for a given pid.
/// Returns `FilterAction::Allow` if no filter is installed for the pid.
pub fn seccomp_check(pid: usize, syscall_num: u64) -> FilterAction {
    let filters = FILTERS.lock();
    for f in filters.iter() {
        if f.pid == pid {
            if syscall_num < MAX_SYSCALLS as u64 && (f.allowed & (1 << syscall_num)) != 0 {
                return FilterAction::Allow;
            }
            return f.default_action;
        }
    }
    FilterAction::Allow
}

/// Remove the syscall filter for a process (on exit).
pub fn seccomp_remove(pid: usize) {
    let mut filters = FILTERS.lock();
    filters.retain(|f| f.pid != pid);
}

/// Set strict mode for a process.
/// Only allows read (0), write (1), exit (2), and getpid (3).
pub fn seccomp_strict(pid: usize) {
    let allowed = (1u64 << 0) | (1u64 << 1) | (1u64 << 2) | (1u64 << 3);
    seccomp_set(pid, allowed, FilterAction::Kill);
}

/// Return a human-readable description of the filter for a pid.
pub fn seccomp_display(pid: usize) -> String {
    let filters = FILTERS.lock();
    for f in filters.iter() {
        if f.pid == pid {
            let action_str = match f.default_action {
                FilterAction::Allow => "allow",
                FilterAction::Kill => "kill",
                FilterAction::Log => "log",
            };
            let count = f.allowed.count_ones();
            return format!(
                "pid {} seccomp: {} syscalls allowed, default={}",
                pid, count, action_str
            );
        }
    }
    format!("pid {} seccomp: no filter (all allowed)", pid)
}

// ---------------------------------------------------------------------------
// 3. Audit Logging
// ---------------------------------------------------------------------------

/// Atomic event counters for audit summary.
static DENIED_COUNT: AtomicUsize = AtomicUsize::new(0);
static AUTH_FAIL_COUNT: AtomicUsize = AtomicUsize::new(0);
static AUTH_OK_COUNT: AtomicUsize = AtomicUsize::new(0);
static CAP_DENIED_COUNT: AtomicUsize = AtomicUsize::new(0);
static SECCOMP_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Security events that can be audited.
pub enum AuditEvent {
    /// A permission check denied access to a path.
    PermissionDenied { uid: u32, path: String, access: &'static str },
    /// A user authenticated successfully.
    AuthSuccess { username: String },
    /// A user authentication attempt failed.
    AuthFailed { username: String },
    /// A user switched identity.
    UserSwitch { from_uid: u32, to_uid: u32 },
    /// A capability check denied an operation.
    CapabilityDenied { pid: usize, cap: &'static str },
    /// A syscall was blocked by the filter.
    SyscallBlocked { pid: usize, syscall: u64 },
    /// A seccomp violation occurred.
    SeccompViolation { pid: usize, syscall: u64 },
}

/// Log a security event to the kernel ring buffer and serial console.
pub fn audit_log(event: AuditEvent) {
    match event {
        AuditEvent::PermissionDenied { uid, ref path, access } => {
            DENIED_COUNT.fetch_add(1, Ordering::Relaxed);
            let msg = format!("[audit] DENIED uid={} access={} path={}", uid, access, path);
            klog_println!("{}", msg);
            serial_println!("{}", msg);
        }
        AuditEvent::AuthSuccess { ref username } => {
            AUTH_OK_COUNT.fetch_add(1, Ordering::Relaxed);
            let msg = format!("[audit] AUTH_OK user={}", username);
            klog_println!("{}", msg);
            serial_println!("{}", msg);
        }
        AuditEvent::AuthFailed { ref username } => {
            AUTH_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
            let msg = format!("[audit] AUTH_FAIL user={}", username);
            klog_println!("{}", msg);
            serial_println!("{}", msg);
        }
        AuditEvent::UserSwitch { from_uid, to_uid } => {
            let msg = format!("[audit] USER_SWITCH from={} to={}", from_uid, to_uid);
            klog_println!("{}", msg);
            serial_println!("{}", msg);
        }
        AuditEvent::CapabilityDenied { pid, cap } => {
            CAP_DENIED_COUNT.fetch_add(1, Ordering::Relaxed);
            let msg = format!("[audit] CAP_DENIED pid={} cap={}", pid, cap);
            klog_println!("{}", msg);
            serial_println!("{}", msg);
        }
        AuditEvent::SyscallBlocked { pid, syscall } => {
            SECCOMP_COUNT.fetch_add(1, Ordering::Relaxed);
            let msg = format!("[audit] SYSCALL_BLOCKED pid={} syscall={}", pid, syscall);
            klog_println!("{}", msg);
            serial_println!("{}", msg);
        }
        AuditEvent::SeccompViolation { pid, syscall } => {
            SECCOMP_COUNT.fetch_add(1, Ordering::Relaxed);
            let msg = format!("[audit] SECCOMP_VIOLATION pid={} syscall={}", pid, syscall);
            klog_println!("{}", msg);
            serial_println!("{}", msg);
        }
    }
}

/// Return a summary of audit event counts.
pub fn audit_summary() -> String {
    format!(
        "Audit summary: denied={} auth_ok={} auth_fail={} cap_denied={} seccomp={}",
        DENIED_COUNT.load(Ordering::Relaxed),
        AUTH_OK_COUNT.load(Ordering::Relaxed),
        AUTH_FAIL_COUNT.load(Ordering::Relaxed),
        CAP_DENIED_COUNT.load(Ordering::Relaxed),
        SECCOMP_COUNT.load(Ordering::Relaxed),
    )
}
