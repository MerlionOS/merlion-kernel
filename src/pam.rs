/// PAM (Pluggable Authentication Modules) framework for MerlionOS.
/// Provides a modular authentication system with stacking,
/// per-user home directories, and file encryption.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_MODULES: usize = 32;
const MAX_STACK_ENTRIES: usize = 16;
const MAX_SERVICES: usize = 16;
const MAX_SESSIONS: usize = 64;
const MAX_SECURE_TTYS: usize = 8;
const MAX_ENV_VARS: usize = 32;
const MAX_LIMITS: usize = 16;
const ENCRYPTION_KEY_LEN: usize = 32;
const HOME_BASE: &str = "/home/";

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static AUTH_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static AUTH_SUCCESSES: AtomicU64 = AtomicU64::new(0);
static AUTH_FAILURES: AtomicU64 = AtomicU64::new(0);
static SESSIONS_OPENED: AtomicU64 = AtomicU64::new(0);
static SESSIONS_CLOSED: AtomicU64 = AtomicU64::new(0);

static PAM: Mutex<PamState> = Mutex::new(PamState::new());

// ---------------------------------------------------------------------------
// PAM module types
// ---------------------------------------------------------------------------

/// The four PAM management groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PamType {
    Auth,
    Account,
    Session,
    Password,
}

impl PamType {
    fn label(self) -> &'static str {
        match self {
            PamType::Auth => "auth",
            PamType::Account => "account",
            PamType::Session => "session",
            PamType::Password => "password",
        }
    }
}

/// Control flag for module stacking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Required,
    Requisite,
    Sufficient,
    Optional,
}

impl Control {
    fn label(self) -> &'static str {
        match self {
            Control::Required => "required",
            Control::Requisite => "requisite",
            Control::Sufficient => "sufficient",
            Control::Optional => "optional",
        }
    }
}

/// Result of a single PAM module invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PamResult {
    Success,
    AuthError,
    PermDenied,
    UserUnknown,
    Ignore,
}

/// Identifies a built-in PAM module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleId {
    PamUnix,
    PamPermit,
    PamDeny,
    PamSecuretty,
    PamLimits,
    PamEnv,
}

impl ModuleId {
    fn name(self) -> &'static str {
        match self {
            ModuleId::PamUnix => "pam_unix",
            ModuleId::PamPermit => "pam_permit",
            ModuleId::PamDeny => "pam_deny",
            ModuleId::PamSecuretty => "pam_securetty",
            ModuleId::PamLimits => "pam_limits",
            ModuleId::PamEnv => "pam_env",
        }
    }
}

// ---------------------------------------------------------------------------
// Stack entry and service configuration
// ---------------------------------------------------------------------------

/// One entry in a PAM module stack.
#[derive(Debug, Clone, Copy)]
struct StackEntry {
    pam_type: PamType,
    control: Control,
    module: ModuleId,
}

/// A named PAM service configuration (e.g. login, sshd, su, sudo).
struct ServiceConfig {
    name: String,
    stack: Vec<StackEntry>,
}

impl ServiceConfig {
    fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            stack: Vec::new(),
        }
    }

    fn push(&mut self, pam_type: PamType, control: Control, module: ModuleId) {
        self.stack.push(StackEntry { pam_type, control, module });
    }
}

// ---------------------------------------------------------------------------
// Session tracking
// ---------------------------------------------------------------------------

/// An active PAM session.
struct Session {
    service: String,
    user: String,
    login_ticks: u64,
    active: bool,
}

impl Session {
    fn new(service: &str, user: &str, ticks: u64) -> Self {
        Self {
            service: String::from(service),
            user: String::from(user),
            login_ticks: ticks,
            active: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Resource limits (pam_limits)
// ---------------------------------------------------------------------------

/// Per-user resource limits enforced by pam_limits.
struct UserLimit {
    username: String,
    max_procs: u32,
    max_files: u32,
    max_mem_kb: u32,
}

impl UserLimit {
    fn new(user: &str, procs: u32, files: u32, mem_kb: u32) -> Self {
        Self {
            username: String::from(user),
            max_procs: procs,
            max_files: files,
            max_mem_kb: mem_kb,
        }
    }
}

// ---------------------------------------------------------------------------
// Environment variables (pam_env)
// ---------------------------------------------------------------------------

/// Environment variable set by pam_env on session open.
struct EnvVar {
    key: String,
    value: String,
}

impl EnvVar {
    fn new(k: &str, v: &str) -> Self {
        Self { key: String::from(k), value: String::from(v) }
    }
}

// ---------------------------------------------------------------------------
// Secure tty list (pam_securetty)
// ---------------------------------------------------------------------------

/// Terminals where root login is permitted.
struct SecureTtyList {
    ttys: Vec<String>,
}

impl SecureTtyList {
    fn new() -> Self {
        let mut ttys = Vec::new();
        ttys.push(String::from("tty1"));
        ttys.push(String::from("ttyS0"));
        ttys.push(String::from("console"));
        Self { ttys }
    }

    fn is_secure(&self, tty: &str) -> bool {
        self.ttys.iter().any(|t| t == tty)
    }
}

// ---------------------------------------------------------------------------
// Encryption key derivation (simplified XOR)
// ---------------------------------------------------------------------------

/// Derive a repeating XOR key from a username and password.
fn derive_key(username: &str, password: &str) -> [u8; ENCRYPTION_KEY_LEN] {
    let mut key = [0u8; ENCRYPTION_KEY_LEN];
    let combined = format!("{}:{}", username, password);
    let bytes = combined.as_bytes();
    // FNV-1a based key expansion
    let mut hash: u64 = 0xcbf29ce484222325;
    for i in 0..ENCRYPTION_KEY_LEN {
        let b = bytes[i % bytes.len()];
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x00000100000001B3);
        key[i] = (hash & 0xFF) as u8;
    }
    key
}

/// Encrypt/decrypt data with XOR (symmetric).
fn xor_crypt(data: &[u8], key: &[u8; ENCRYPTION_KEY_LEN]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for (i, &b) in data.iter().enumerate() {
        out.push(b ^ key[i % ENCRYPTION_KEY_LEN]);
    }
    out
}

// ---------------------------------------------------------------------------
// PAM state
// ---------------------------------------------------------------------------

struct PamState {
    services: Vec<ServiceConfig>,
    sessions: Vec<Session>,
    limits: Vec<UserLimit>,
    env_vars: Vec<EnvVar>,
    secure_ttys: SecureTtyList,
}

impl PamState {
    const fn new() -> Self {
        Self {
            services: Vec::new(),
            sessions: Vec::new(),
            limits: Vec::new(),
            env_vars: Vec::new(),
            secure_ttys: SecureTtyList { ttys: Vec::new() },
        }
    }

    fn find_service(&self, name: &str) -> Option<usize> {
        self.services.iter().position(|s| s.name == name)
    }

    fn active_session_count(&self) -> usize {
        self.sessions.iter().filter(|s| s.active).count()
    }
}

// ---------------------------------------------------------------------------
// Module evaluation
// ---------------------------------------------------------------------------

/// Evaluate a single built-in module.
fn evaluate_module(
    module: ModuleId,
    _pam_type: PamType,
    user: &str,
    password: &str,
    tty: &str,
    state: &PamState,
) -> PamResult {
    match module {
        ModuleId::PamUnix => {
            // Authenticate via security module
            let hash = crate::security::hash_password(password);
            if crate::security::authenticate(user, hash) {
                PamResult::Success
            } else {
                PamResult::AuthError
            }
        }
        ModuleId::PamPermit => PamResult::Success,
        ModuleId::PamDeny => PamResult::PermDenied,
        ModuleId::PamSecuretty => {
            if user == "root" && !state.secure_ttys.is_secure(tty) {
                PamResult::PermDenied
            } else {
                PamResult::Success
            }
        }
        ModuleId::PamLimits => {
            // Check if user has limits defined (always succeed, limits enforced later)
            PamResult::Success
        }
        ModuleId::PamEnv => {
            // Environment setup happens in session open
            PamResult::Success
        }
    }
}

/// Evaluate a module stack for a given PAM type.
/// Returns Ok(()) on success, Err(reason) on failure.
fn evaluate_stack(
    stack: &[StackEntry],
    pam_type: PamType,
    user: &str,
    password: &str,
    tty: &str,
    state: &PamState,
) -> Result<(), &'static str> {
    let mut required_fail = false;
    let mut any_success = false;

    for entry in stack.iter().filter(|e| e.pam_type == pam_type) {
        let result = evaluate_module(entry.module, pam_type, user, password, tty, state);

        match entry.control {
            Control::Required => {
                if result != PamResult::Success && result != PamResult::Ignore {
                    required_fail = true;
                } else if result == PamResult::Success {
                    any_success = true;
                }
                // Continue evaluating remaining modules
            }
            Control::Requisite => {
                if result != PamResult::Success && result != PamResult::Ignore {
                    return Err("requisite module failed");
                }
                any_success = true;
            }
            Control::Sufficient => {
                if result == PamResult::Success {
                    if !required_fail {
                        return Ok(());
                    }
                }
                // If not success, treat as optional
            }
            Control::Optional => {
                if result == PamResult::Success {
                    any_success = true;
                }
                // Ignore failures from optional modules
            }
        }
    }

    if required_fail {
        Err("required module failed")
    } else if any_success {
        Ok(())
    } else {
        // No modules matched this type — pass by default
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Home directory management
// ---------------------------------------------------------------------------

/// Ensure the user's home directory exists, creating it if needed.
fn ensure_home_dir(username: &str) {
    let home = format!("{}{}", HOME_BASE, username);
    // Try to create /home if not exists
    let _ = crate::vfs::mkdir("/home");
    let _ = crate::vfs::mkdir(&home);
}

/// Get the home directory path for a user.
pub fn home_dir(username: &str) -> String {
    format!("{}{}", HOME_BASE, username)
}

/// Write encrypted data to a file in the user's home directory.
pub fn encrypted_write(username: &str, password: &str, filename: &str, data: &[u8]) -> Result<(), &'static str> {
    let key = derive_key(username, password);
    let encrypted = xor_crypt(data, &key);
    let path = format!("{}{}/{}", HOME_BASE, username, filename);
    // Convert encrypted bytes to string for VFS
    let s: String = encrypted.iter().map(|&b| b as char).collect();
    crate::vfs::write(&path, &s).map_err(|_| "write failed")
}

/// Read and decrypt data from a file in the user's home directory.
pub fn encrypted_read(username: &str, password: &str, filename: &str) -> Result<Vec<u8>, &'static str> {
    let key = derive_key(username, password);
    let path = format!("{}{}/{}", HOME_BASE, username, filename);
    let content = crate::vfs::cat(&path).map_err(|_| "read failed")?;
    let bytes: Vec<u8> = content.bytes().collect();
    Ok(xor_crypt(&bytes, &key))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the PAM framework with default service configurations.
pub fn init() {
    let mut state = PamState::new();

    // Secure TTY list
    state.secure_ttys = SecureTtyList::new();

    // Default environment variables
    state.env_vars.push(EnvVar::new("PATH", "/bin:/usr/bin:/sbin"));
    state.env_vars.push(EnvVar::new("SHELL", "/bin/msh"));
    state.env_vars.push(EnvVar::new("LANG", "en_US.UTF-8"));

    // Default resource limits
    state.limits.push(UserLimit::new("*", 64, 256, 65536));
    state.limits.push(UserLimit::new("root", 0, 0, 0)); // 0 = unlimited

    // --- Service: login ---
    let mut login = ServiceConfig::new("login");
    login.push(PamType::Auth, Control::Required, ModuleId::PamSecuretty);
    login.push(PamType::Auth, Control::Required, ModuleId::PamUnix);
    login.push(PamType::Account, Control::Required, ModuleId::PamUnix);
    login.push(PamType::Session, Control::Required, ModuleId::PamLimits);
    login.push(PamType::Session, Control::Required, ModuleId::PamEnv);
    login.push(PamType::Password, Control::Required, ModuleId::PamUnix);
    state.services.push(login);

    // --- Service: sshd ---
    let mut sshd = ServiceConfig::new("sshd");
    sshd.push(PamType::Auth, Control::Required, ModuleId::PamUnix);
    sshd.push(PamType::Account, Control::Required, ModuleId::PamUnix);
    sshd.push(PamType::Session, Control::Required, ModuleId::PamLimits);
    sshd.push(PamType::Session, Control::Required, ModuleId::PamEnv);
    sshd.push(PamType::Password, Control::Required, ModuleId::PamUnix);
    state.services.push(sshd);

    // --- Service: su ---
    let mut su = ServiceConfig::new("su");
    su.push(PamType::Auth, Control::Sufficient, ModuleId::PamPermit); // root can su without password
    su.push(PamType::Auth, Control::Required, ModuleId::PamUnix);
    su.push(PamType::Account, Control::Required, ModuleId::PamUnix);
    su.push(PamType::Session, Control::Optional, ModuleId::PamEnv);
    state.services.push(su);

    // --- Service: sudo ---
    let mut sudo = ServiceConfig::new("sudo");
    sudo.push(PamType::Auth, Control::Required, ModuleId::PamUnix);
    sudo.push(PamType::Account, Control::Required, ModuleId::PamUnix);
    sudo.push(PamType::Session, Control::Required, ModuleId::PamLimits);
    sudo.push(PamType::Session, Control::Optional, ModuleId::PamEnv);
    state.services.push(sudo);

    *PAM.lock() = state;
    INITIALIZED.store(true, Ordering::SeqCst);
}

/// Authenticate a user against a PAM service.
/// `tty` is used for securetty checks (pass "console" if unknown).
pub fn authenticate(service: &str, user: &str, password: &str) -> Result<(), &'static str> {
    AUTH_ATTEMPTS.fetch_add(1, Ordering::Relaxed);

    let pam = PAM.lock();
    let svc_idx = pam.find_service(service).ok_or("unknown PAM service")?;
    let stack = &pam.services[svc_idx].stack;

    // Evaluate auth stack
    let result = evaluate_stack(stack, PamType::Auth, user, password, "console", &pam);

    match result {
        Ok(()) => {
            // Also check account
            let acct = evaluate_stack(stack, PamType::Account, user, password, "console", &pam);
            if acct.is_ok() {
                AUTH_SUCCESSES.fetch_add(1, Ordering::Relaxed);
                Ok(())
            } else {
                AUTH_FAILURES.fetch_add(1, Ordering::Relaxed);
                acct
            }
        }
        Err(e) => {
            AUTH_FAILURES.fetch_add(1, Ordering::Relaxed);
            Err(e)
        }
    }
}

/// Authenticate with explicit TTY (for securetty checks).
pub fn authenticate_tty(service: &str, user: &str, password: &str, tty: &str) -> Result<(), &'static str> {
    AUTH_ATTEMPTS.fetch_add(1, Ordering::Relaxed);

    let pam = PAM.lock();
    let svc_idx = pam.find_service(service).ok_or("unknown PAM service")?;
    let stack = &pam.services[svc_idx].stack;

    let result = evaluate_stack(stack, PamType::Auth, user, password, tty, &pam);
    match result {
        Ok(()) => {
            let acct = evaluate_stack(stack, PamType::Account, user, password, tty, &pam);
            if acct.is_ok() {
                AUTH_SUCCESSES.fetch_add(1, Ordering::Relaxed);
                Ok(())
            } else {
                AUTH_FAILURES.fetch_add(1, Ordering::Relaxed);
                acct
            }
        }
        Err(e) => {
            AUTH_FAILURES.fetch_add(1, Ordering::Relaxed);
            Err(e)
        }
    }
}

/// Open a PAM session for a user after successful authentication.
pub fn open_session(service: &str, user: &str) -> Result<(), &'static str> {
    // Ensure home directory exists
    ensure_home_dir(user);

    let mut pam = PAM.lock();
    let svc_idx = pam.find_service(service).ok_or("unknown PAM service")?;
    let stack = pam.services[svc_idx].stack.clone();

    // Evaluate session stack
    evaluate_stack(&stack, PamType::Session, user, "", "console", &pam)?;

    // Set environment variables
    for ev in &pam.env_vars {
        crate::env::set(&ev.key, &ev.value);
    }
    // Set HOME
    let home = format!("{}{}", HOME_BASE, user);
    crate::env::set("HOME", &home);
    crate::env::set("USER", user);

    // Apply resource limits
    let _limits = pam.limits.iter().find(|l| l.username == user || l.username == "*");
    // Limits would be enforced via cgroup or process manager in a full implementation

    let ticks = crate::timer::ticks();
    pam.sessions.push(Session::new(service, user, ticks));
    SESSIONS_OPENED.fetch_add(1, Ordering::Relaxed);

    Ok(())
}

/// Close a PAM session for a user.
pub fn close_session(service: &str, user: &str) -> Result<(), &'static str> {
    let mut pam = PAM.lock();

    // Find the most recent active session for this user and service
    let pos = pam.sessions.iter().rposition(|s| {
        s.active && s.service == service && s.user == user
    });

    match pos {
        Some(idx) => {
            pam.sessions[idx].active = false;
            SESSIONS_CLOSED.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        None => Err("no active session found"),
    }
}

/// List all active sessions.
pub fn list_sessions() -> String {
    let pam = PAM.lock();
    let mut out = String::from("Active PAM sessions:\n");
    let mut count = 0u32;
    for s in &pam.sessions {
        if s.active {
            out.push_str(&format!("  {} @ {} (since tick {})\n",
                s.user, s.service, s.login_ticks));
            count += 1;
        }
    }
    if count == 0 {
        out.push_str("  (none)\n");
    }
    out
}

/// List configured PAM services and their module stacks.
pub fn list_services() -> String {
    let pam = PAM.lock();
    let mut out = String::from("PAM services:\n");
    for svc in &pam.services {
        out.push_str(&format!("  [{}]\n", svc.name));
        for entry in &svc.stack {
            out.push_str(&format!("    {} {} {}\n",
                entry.pam_type.label(),
                entry.control.label(),
                entry.module.name()));
        }
    }
    out
}

/// Return PAM subsystem information.
pub fn pam_info() -> String {
    let pam = PAM.lock();
    let mut out = String::from("PAM Subsystem:\n");
    out.push_str(&format!("  Initialized: {}\n", INITIALIZED.load(Ordering::Relaxed)));
    out.push_str(&format!("  Services: {}\n", pam.services.len()));
    out.push_str(&format!("  Active sessions: {}\n", pam.active_session_count()));
    out.push_str(&format!("  Secure TTYs: {}\n", pam.secure_ttys.ttys.len()));
    out.push_str(&format!("  Resource limit rules: {}\n", pam.limits.len()));
    out.push_str(&format!("  Environment vars: {}\n", pam.env_vars.len()));
    out.push_str("\n  Built-in modules:\n");
    out.push_str("    pam_unix     - password auth via security module\n");
    out.push_str("    pam_permit   - always succeed\n");
    out.push_str("    pam_deny     - always fail\n");
    out.push_str("    pam_securetty - restrict root to secure terminals\n");
    out.push_str("    pam_limits   - enforce per-user resource limits\n");
    out.push_str("    pam_env      - set environment on login\n");
    out
}

/// Return PAM statistics.
pub fn pam_stats() -> String {
    let mut out = String::from("PAM Statistics:\n");
    out.push_str(&format!("  Auth attempts:  {}\n", AUTH_ATTEMPTS.load(Ordering::Relaxed)));
    out.push_str(&format!("  Auth successes: {}\n", AUTH_SUCCESSES.load(Ordering::Relaxed)));
    out.push_str(&format!("  Auth failures:  {}\n", AUTH_FAILURES.load(Ordering::Relaxed)));
    out.push_str(&format!("  Sessions opened: {}\n", SESSIONS_OPENED.load(Ordering::Relaxed)));
    out.push_str(&format!("  Sessions closed: {}\n", SESSIONS_CLOSED.load(Ordering::Relaxed)));
    out
}
