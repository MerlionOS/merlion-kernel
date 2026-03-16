/// Multi-user session management for MerlionOS.
/// Supports multiple concurrent login sessions, virtual terminals,
/// session switching, and per-user resource accounting.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use alloc::borrow::ToOwned;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::{timer, rtc, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum concurrent login sessions.
const MAX_SESSIONS: usize = 16;

/// Number of virtual terminals (tty1 .. tty6).
const NUM_VTYS: usize = 6;

/// Auto-logout after this many seconds idle (default: 30 minutes).
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 1800;

/// Default message of the day.
const DEFAULT_MOTD: &str = "Welcome to MerlionOS — Born for AI. Built by AI.";

// ---------------------------------------------------------------------------
// Login source
// ---------------------------------------------------------------------------

/// How the user connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginSource {
    /// Local virtual terminal.
    Local,
    /// SSH remote session.
    Ssh,
    /// Serial console.
    Serial,
}

impl LoginSource {
    fn as_str(self) -> &'static str {
        match self {
            LoginSource::Local  => "local",
            LoginSource::Ssh    => "ssh",
            LoginSource::Serial => "serial",
        }
    }
}

// ---------------------------------------------------------------------------
// Per-user resource limits
// ---------------------------------------------------------------------------

/// Resource limits enforced per user.
#[derive(Debug, Clone, Copy)]
pub struct UserLimits {
    /// Maximum number of processes the user may spawn.
    pub max_processes: usize,
    /// Maximum number of open file descriptors.
    pub max_open_files: usize,
    /// Maximum memory in bytes.
    pub max_memory: usize,
    /// CPU time limit in seconds (0 = unlimited).
    pub cpu_time_limit: u64,
}

impl UserLimits {
    pub const fn default_limits() -> Self {
        Self {
            max_processes: 64,
            max_open_files: 256,
            max_memory: 64 * 1024 * 1024, // 64 MiB
            cpu_time_limit: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// User accounting
// ---------------------------------------------------------------------------

/// Accumulated accounting data for a user.
#[derive(Debug, Clone)]
pub struct UserAccounting {
    pub user: String,
    /// Total login time in seconds (across all sessions).
    pub total_login_secs: u64,
    /// Number of completed sessions.
    pub session_count: u64,
    /// CPU ticks consumed.
    pub cpu_ticks: u64,
    /// Commands executed.
    pub commands_executed: u64,
    /// Bytes read.
    pub bytes_read: u64,
    /// Bytes written.
    pub bytes_written: u64,
}

impl UserAccounting {
    fn new(user: &str) -> Self {
        Self {
            user: user.to_owned(),
            total_login_secs: 0,
            session_count: 0,
            cpu_ticks: 0,
            commands_executed: 0,
            bytes_read: 0,
            bytes_written: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Session environment
// ---------------------------------------------------------------------------

/// Per-session environment state.
#[derive(Debug, Clone)]
pub struct SessionEnv {
    /// Environment variables (key, value).
    pub vars: Vec<(String, String)>,
    /// Current working directory.
    pub cwd: String,
    /// File creation mask.
    pub umask: u16,
}

impl SessionEnv {
    fn new(user: &str) -> Self {
        let home = format!("/home/{}", user);
        let mut vars = Vec::new();
        vars.push(("USER".to_owned(), user.to_owned()));
        vars.push(("HOME".to_owned(), home.clone()));
        vars.push(("SHELL".to_owned(), "/bin/msh".to_owned()));
        vars.push(("PATH".to_owned(), "/bin:/usr/bin:/sbin".to_owned()));
        vars.push(("TERM".to_owned(), "merlion-vt".to_owned()));
        Self {
            vars,
            cwd: home,
            umask: 0o022,
        }
    }

    /// Get an environment variable.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// Set an environment variable.
    pub fn set(&mut self, key: &str, value: &str) {
        if let Some(entry) = self.vars.iter_mut().find(|(k, _)| k == key) {
            entry.1 = value.to_owned();
        } else {
            self.vars.push((key.to_owned(), value.to_owned()));
        }
    }
}

// ---------------------------------------------------------------------------
// Login session
// ---------------------------------------------------------------------------

/// A single login session.
#[derive(Debug, Clone)]
pub struct LoginSession {
    /// Unique session identifier.
    pub id: u64,
    /// Username.
    pub user: String,
    /// TTY name (e.g. "tty1", "pts/0").
    pub tty: String,
    /// Timer tick at login time.
    pub login_tick: u64,
    /// RTC timestamp string at login.
    pub login_time: String,
    /// Timer tick of last activity (for idle detection).
    pub last_activity_tick: u64,
    /// Source of the session.
    pub source: LoginSource,
    /// Session environment.
    pub env: SessionEnv,
    /// Per-user resource limits.
    pub limits: UserLimits,
    /// Whether session is active.
    pub active: bool,
}

// ---------------------------------------------------------------------------
// utmp / wtmp records
// ---------------------------------------------------------------------------

/// A record in the utmp/wtmp log.
#[derive(Debug, Clone)]
pub struct UtmpRecord {
    pub user: String,
    pub tty: String,
    pub source: LoginSource,
    pub login_tick: u64,
    pub login_time: String,
    /// None if still logged in.
    pub logout_time: Option<String>,
    pub logout_tick: Option<u64>,
}

// ---------------------------------------------------------------------------
// Virtual terminal
// ---------------------------------------------------------------------------

/// A virtual terminal slot.
#[derive(Debug, Clone)]
pub struct VirtualTerminal {
    /// TTY name (tty1..tty6).
    pub name: String,
    /// Session ID occupying this terminal, if any.
    pub session_id: Option<u64>,
    /// Output buffer (last N lines of output for redraw on switch).
    pub screen_lines: Vec<String>,
}

impl VirtualTerminal {
    fn new(n: usize) -> Self {
        Self {
            name: format!("tty{}", n + 1),
            session_id: None,
            screen_lines: Vec::new(),
        }
    }

    /// Append a line to the screen buffer (capped at 50 lines).
    pub fn push_line(&mut self, line: &str) {
        self.screen_lines.push(line.to_owned());
        if self.screen_lines.len() > 50 {
            self.screen_lines.remove(0);
        }
    }
}

// ---------------------------------------------------------------------------
// Session manager (global state)
// ---------------------------------------------------------------------------

/// Central session manager.
pub struct SessionManager {
    sessions: Vec<LoginSession>,
    vtys: [VirtualTerminal; NUM_VTYS],
    active_vty: usize,
    wtmp: Vec<UtmpRecord>,
    accounting: Vec<UserAccounting>,
    motd: String,
    idle_timeout_secs: u64,
    next_id: u64,
}

impl SessionManager {
    const fn placeholder_vty() -> VirtualTerminal {
        VirtualTerminal {
            name: String::new(),
            session_id: None,
            screen_lines: Vec::new(),
        }
    }

    pub const fn new() -> Self {
        Self {
            sessions: Vec::new(),
            vtys: [
                Self::placeholder_vty(),
                Self::placeholder_vty(),
                Self::placeholder_vty(),
                Self::placeholder_vty(),
                Self::placeholder_vty(),
                Self::placeholder_vty(),
            ],
            active_vty: 0,
            wtmp: Vec::new(),
            accounting: Vec::new(),
            motd: String::new(),
            idle_timeout_secs: DEFAULT_IDLE_TIMEOUT_SECS,
            next_id: 1,
        }
    }

    fn init_vtys(&mut self) {
        for i in 0..NUM_VTYS {
            self.vtys[i] = VirtualTerminal::new(i);
        }
        self.motd = DEFAULT_MOTD.to_owned();
    }

    /// Find or create accounting entry for a user.
    fn accounting_mut(&mut self, user: &str) -> &mut UserAccounting {
        if !self.accounting.iter().any(|a| a.user == user) {
            self.accounting.push(UserAccounting::new(user));
        }
        self.accounting.iter_mut().find(|a| a.user == user).unwrap()
    }
}

/// Global session manager.
pub static SESSION_MGR: Mutex<SessionManager> = Mutex::new(SessionManager::new());

/// Counter for total sessions created.
static TOTAL_SESSIONS: AtomicU64 = AtomicU64::new(0);

/// Currently active virtual terminal index (0-based).
static ACTIVE_VTY: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the multi-user session subsystem.
pub fn init() {
    let mut mgr = SESSION_MGR.lock();
    mgr.init_vtys();
    serial_println!("[multi_user] session manager initialised ({} virtual terminals)", NUM_VTYS);
}

/// Create a new login session.
///
/// Returns the session ID on success.
pub fn login(user: &str, tty: &str, source: LoginSource) -> u64 {
    let mut mgr = SESSION_MGR.lock();
    let now = timer::ticks();
    let dt = rtc::read();
    let time_str = format!("{}", dt);
    let id = mgr.next_id;
    mgr.next_id += 1;

    let session = LoginSession {
        id,
        user: user.to_owned(),
        tty: tty.to_owned(),
        login_tick: now,
        login_time: time_str.clone(),
        last_activity_tick: now,
        source,
        env: SessionEnv::new(user),
        limits: UserLimits::default_limits(),
        active: true,
    };

    // Record in utmp/wtmp
    mgr.wtmp.push(UtmpRecord {
        user: user.to_owned(),
        tty: tty.to_owned(),
        source,
        login_tick: now,
        login_time: time_str,
        logout_time: None,
        logout_tick: None,
    });

    // Assign to VTY if it matches ttyN
    if let Some(rest) = tty.strip_prefix("tty") {
        if let Ok(n) = rest.parse::<usize>() {
            if n >= 1 && n <= NUM_VTYS {
                mgr.vtys[n - 1].session_id = Some(id);
            }
        }
    }

    // Update accounting
    let acct = mgr.accounting_mut(user);
    acct.session_count += 1;

    mgr.sessions.push(session);
    TOTAL_SESSIONS.fetch_add(1, Ordering::Relaxed);

    serial_println!("[multi_user] login: {} on {} via {} (session {})", user, tty, source.as_str(), id);
    id
}

/// Destroy a login session by ID.
pub fn logout(session_id: u64) {
    let mut mgr = SESSION_MGR.lock();
    let now = timer::ticks();
    let dt = rtc::read();
    let time_str = format!("{}", dt);

    // Extract session data first to avoid double borrow.
    let session_data = mgr.sessions.iter_mut()
        .find(|s| s.id == session_id)
        .map(|session| {
            session.active = false;
            let elapsed = (now - session.login_tick) / 100;
            let user = session.user.clone();
            let tty = session.tty.clone();
            (user, tty, elapsed)
        });

    if let Some((user, tty, elapsed)) = session_data {
        // Update wtmp logout time
        if let Some(rec) = mgr.wtmp.iter_mut().rev().find(|r| {
            r.user == user && r.tty == tty && r.logout_time.is_none()
        }) {
            rec.logout_time = Some(time_str);
            rec.logout_tick = Some(now);
        }

        // Release VTY
        if let Some(rest) = tty.strip_prefix("tty") {
            if let Ok(n) = rest.parse::<usize>() {
                if n >= 1 && n <= NUM_VTYS {
                    mgr.vtys[n - 1].session_id = None;
                }
            }
        }

        // Update accounting
        let acct = mgr.accounting_mut(&user);
        acct.total_login_secs += elapsed;

        serial_println!("[multi_user] logout: {} (session {}, {} secs)", user, session_id, elapsed);
    }

    mgr.sessions.retain(|s| s.id != session_id);
}

/// Switch to a virtual terminal (1-based: 1..=6).
pub fn switch_session(tty_num: usize) {
    if tty_num < 1 || tty_num > NUM_VTYS {
        return;
    }
    ACTIVE_VTY.store(tty_num - 1, Ordering::SeqCst);
    let mgr = SESSION_MGR.lock();
    let vty = &mgr.vtys[tty_num - 1];
    serial_println!("[multi_user] switched to {} (session {:?})", vty.name, vty.session_id);
}

/// Record activity on a session (resets idle timer).
pub fn touch_session(session_id: u64) {
    let mut mgr = SESSION_MGR.lock();
    if let Some(s) = mgr.sessions.iter_mut().find(|s| s.id == session_id) {
        s.last_activity_tick = timer::ticks();
    }
}

/// Record a command execution for accounting.
pub fn record_command(session_id: u64) {
    let mut mgr = SESSION_MGR.lock();
    let user = mgr.sessions.iter()
        .find(|s| s.id == session_id)
        .map(|s| s.user.clone());
    if let Some(user) = user {
        let acct = mgr.accounting_mut(&user);
        acct.commands_executed += 1;
    }
}

/// Record bytes read/written for accounting.
pub fn record_io(session_id: u64, bytes_read: u64, bytes_written: u64) {
    let mut mgr = SESSION_MGR.lock();
    let user = mgr.sessions.iter()
        .find(|s| s.id == session_id)
        .map(|s| s.user.clone());
    if let Some(user) = user {
        let acct = mgr.accounting_mut(&user);
        acct.bytes_read += bytes_read;
        acct.bytes_written += bytes_written;
    }
}

/// Check for idle sessions and auto-logout if past the timeout.
/// Should be called periodically (e.g. from a timer task).
pub fn check_idle_sessions() {
    let mut to_logout = Vec::new();
    {
        let mgr = SESSION_MGR.lock();
        let now = timer::ticks();
        let timeout_ticks = mgr.idle_timeout_secs * 100; // 100 Hz timer
        for s in &mgr.sessions {
            if s.active && now - s.last_activity_tick > timeout_ticks {
                to_logout.push(s.id);
            }
        }
    }
    for id in to_logout {
        serial_println!("[multi_user] auto-logout session {} (idle timeout)", id);
        logout(id);
    }
}

/// Get the currently active virtual terminal index (0-based).
pub fn active_vty() -> usize {
    ACTIVE_VTY.load(Ordering::Relaxed)
}

/// Set the message of the day.
pub fn set_motd(msg: &str) {
    let mut mgr = SESSION_MGR.lock();
    mgr.motd = msg.to_owned();
}

/// Get the current message of the day.
pub fn get_motd() -> String {
    let mgr = SESSION_MGR.lock();
    mgr.motd.clone()
}

/// Set idle timeout in seconds.
pub fn set_idle_timeout(secs: u64) {
    let mut mgr = SESSION_MGR.lock();
    mgr.idle_timeout_secs = secs;
}

/// Set resource limits for a specific session.
pub fn set_session_limits(session_id: u64, limits: UserLimits) {
    let mut mgr = SESSION_MGR.lock();
    if let Some(s) = mgr.sessions.iter_mut().find(|s| s.id == session_id) {
        s.limits = limits;
    }
}

// ---------------------------------------------------------------------------
// who / w / last commands
// ---------------------------------------------------------------------------

/// Format output like Unix `who`: list currently logged-in users.
pub fn who() -> String {
    let mgr = SESSION_MGR.lock();
    let mut out = String::new();
    for s in &mgr.sessions {
        if !s.active {
            continue;
        }
        out.push_str(&format!(
            "{:<12} {:<8} {}  ({})\n",
            s.user, s.tty, s.login_time, s.source.as_str()
        ));
    }
    if out.is_empty() {
        out.push_str("No users logged in.\n");
    }
    out
}

/// Format output like Unix `w`: who + idle time + what they are doing.
pub fn w() -> String {
    let mgr = SESSION_MGR.lock();
    let now = timer::ticks();
    let mut out = String::new();
    out.push_str(&format!(" {:>8}  up {} secs,  {} users\n",
        "", now / 100, mgr.sessions.iter().filter(|s| s.active).count()));
    out.push_str(&format!("{:<12} {:<8} {:<20} {:>6}  {}\n",
        "USER", "TTY", "LOGIN@", "IDLE", "WHAT"));
    for s in &mgr.sessions {
        if !s.active {
            continue;
        }
        let idle_secs = (now - s.last_activity_tick) / 100;
        let idle_str = if idle_secs < 60 {
            format!("{}s", idle_secs)
        } else {
            format!("{}m", idle_secs / 60)
        };
        out.push_str(&format!(
            "{:<12} {:<8} {:<20} {:>6}  {}\n",
            s.user, s.tty, s.login_time, idle_str, s.env.cwd
        ));
    }
    out
}

/// Format output like Unix `last`: show login history from wtmp.
pub fn last() -> String {
    let mgr = SESSION_MGR.lock();
    let mut out = String::new();
    // Show most recent first, up to 20 entries.
    let entries: Vec<_> = mgr.wtmp.iter().rev().take(20).collect();
    for rec in entries {
        let logout_str = match &rec.logout_time {
            Some(t) => t.as_str(),
            None => "still logged in",
        };
        let duration = match rec.logout_tick {
            Some(lt) => {
                let secs = (lt - rec.login_tick) / 100;
                format!("({}:{:02})", secs / 3600, (secs % 3600) / 60)
            }
            None => String::new(),
        };
        out.push_str(&format!(
            "{:<12} {:<8} {:<8} {:<20} - {:<20} {}\n",
            rec.user, rec.tty, rec.source.as_str(),
            rec.login_time, logout_str, duration
        ));
    }
    if out.is_empty() {
        out.push_str("No login records.\n");
    }
    out
}

/// Return a summary of all active sessions.
pub fn sessions_info() -> String {
    let mgr = SESSION_MGR.lock();
    let mut out = String::new();
    out.push_str(&format!("Active sessions: {}\n", mgr.sessions.iter().filter(|s| s.active).count()));
    out.push_str(&format!("Total sessions created: {}\n", TOTAL_SESSIONS.load(Ordering::Relaxed)));
    out.push_str(&format!("Active VTY: tty{}\n", ACTIVE_VTY.load(Ordering::Relaxed) + 1));
    out.push_str(&format!("Idle timeout: {} secs\n\n", mgr.idle_timeout_secs));

    // Virtual terminal status
    out.push_str("Virtual Terminals:\n");
    for vty in &mgr.vtys {
        let status = match vty.session_id {
            Some(id) => format!("session {}", id),
            None => "free".to_owned(),
        };
        out.push_str(&format!("  {}: {}\n", vty.name, status));
    }

    // Accounting summary
    if !mgr.accounting.is_empty() {
        out.push_str("\nUser Accounting:\n");
        out.push_str(&format!("  {:<12} {:>8} {:>8} {:>10} {:>10} {:>10}\n",
            "USER", "SESSIONS", "LOGIN(s)", "CMDS", "READ(B)", "WRITE(B)"));
        for a in &mgr.accounting {
            out.push_str(&format!("  {:<12} {:>8} {:>8} {:>10} {:>10} {:>10}\n",
                a.user, a.session_count, a.total_login_secs,
                a.commands_executed, a.bytes_read, a.bytes_written));
        }
    }

    out
}
