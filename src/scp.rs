/// SCP file transfer and SSH session management for MerlionOS.
/// Provides file upload/download over SSH sessions and tracks active connections.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

// --- SSH Session Management ---

const MAX_SESSIONS: usize = 8;

/// An active SSH session.
#[derive(Debug, Clone)]
pub struct SshSession {
    /// Session ID.
    pub id: u32,
    /// Connected client IP.
    pub client_ip: [u8; 4],
    /// Authenticated username.
    pub username: String,
    /// Timer tick when session started.
    pub start_tick: u64,
    /// Last activity tick.
    pub last_activity: u64,
    /// Commands executed in this session.
    pub command_count: usize,
    /// Whether session is active.
    pub active: bool,
    /// Terminal width (columns).
    pub term_cols: u16,
    /// Terminal height (rows).
    pub term_rows: u16,
}

static NEXT_SESSION_ID: AtomicU32 = AtomicU32::new(1);
static SESSIONS: Mutex<Vec<SshSession>> = Mutex::new(Vec::new());
static TOTAL_SESSIONS: AtomicUsize = AtomicUsize::new(0);
static TOTAL_COMMANDS: AtomicUsize = AtomicUsize::new(0);

/// Create a new SSH session.
pub fn create_session(client_ip: [u8; 4], username: &str) -> u32 {
    let id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
    let now = crate::timer::ticks();

    let mut sessions = SESSIONS.lock();
    // Remove inactive sessions if at capacity
    if sessions.len() >= MAX_SESSIONS {
        sessions.retain(|s| s.active);
    }

    sessions.push(SshSession {
        id,
        client_ip,
        username: username.to_owned(),
        start_tick: now,
        last_activity: now,
        command_count: 0,
        active: true,
        term_cols: 80,
        term_rows: 24,
    });

    TOTAL_SESSIONS.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[scp] session {} created for {}@{}.{}.{}.{}",
        id, username, client_ip[0], client_ip[1], client_ip[2], client_ip[3]);

    id
}

/// Close an SSH session.
pub fn close_session(id: u32) {
    let mut sessions = SESSIONS.lock();
    if let Some(session) = sessions.iter_mut().find(|s| s.id == id) {
        session.active = false;
        crate::serial_println!("[scp] session {} closed", id);
    }
}

/// Record a command execution in a session.
pub fn record_command(id: u32) {
    let mut sessions = SESSIONS.lock();
    if let Some(session) = sessions.iter_mut().find(|s| s.id == id && s.active) {
        session.command_count += 1;
        session.last_activity = crate::timer::ticks();
    }
    TOTAL_COMMANDS.fetch_add(1, Ordering::Relaxed);
}

/// Set terminal size for a session (e.g., from SSH window-change request).
pub fn set_terminal_size(id: u32, cols: u16, rows: u16) {
    let mut sessions = SESSIONS.lock();
    if let Some(session) = sessions.iter_mut().find(|s| s.id == id) {
        session.term_cols = cols;
        session.term_rows = rows;
    }
}

/// List active sessions.
pub fn list_sessions() -> String {
    let sessions = SESSIONS.lock();
    let active: Vec<&SshSession> = sessions.iter().filter(|s| s.active).collect();

    if active.is_empty() {
        return String::from("No active SSH sessions.\n");
    }

    let mut out = format!("Active SSH sessions ({}):\n", active.len());
    out.push_str(&format!("{:>4} {:<16} {:<12} {:>8} {:>6} {:>10}\n",
        "ID", "Client", "User", "Cmds", "Term", "Uptime"));

    let now = crate::timer::ticks();
    for s in &active {
        let ip = format!("{}.{}.{}.{}", s.client_ip[0], s.client_ip[1], s.client_ip[2], s.client_ip[3]);
        let term = format!("{}x{}", s.term_cols, s.term_rows);
        let uptime = (now - s.start_tick) / 100; // seconds
        out.push_str(&format!("{:>4} {:<16} {:<12} {:>8} {:>6} {:>8}s\n",
            s.id, ip, s.username, s.command_count, term, uptime));
    }
    out
}

/// Get session statistics.
pub fn session_stats() -> String {
    let active = SESSIONS.lock().iter().filter(|s| s.active).count();
    format!(
        "SSH stats: {} active sessions, {} total, {} commands executed",
        active,
        TOTAL_SESSIONS.load(Ordering::Relaxed),
        TOTAL_COMMANDS.load(Ordering::Relaxed),
    )
}

// --- SCP File Transfer ---

/// SCP transfer direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScpDirection {
    Upload,   // client -> server
    Download, // server -> client
}

/// An SCP transfer record.
#[derive(Debug, Clone)]
pub struct ScpTransfer {
    pub id: u32,
    pub session_id: u32,
    pub direction: ScpDirection,
    pub filename: String,
    pub size: usize,
    pub transferred: usize,
    pub completed: bool,
    pub timestamp: u64,
}

const MAX_TRANSFERS: usize = 32;
static TRANSFERS: Mutex<Vec<ScpTransfer>> = Mutex::new(Vec::new());
static NEXT_TRANSFER_ID: AtomicU32 = AtomicU32::new(1);

/// Start an SCP upload (client sends file to server).
/// The file is written to the VFS.
pub fn scp_upload(session_id: u32, filename: &str, data: &[u8]) -> Result<u32, &'static str> {
    let id = NEXT_TRANSFER_ID.fetch_add(1, Ordering::Relaxed);

    // Write to VFS
    let path = if filename.starts_with('/') {
        filename.to_owned()
    } else {
        format!("/tmp/{}", filename)
    };

    let content = core::str::from_utf8(data).unwrap_or("(binary data)");
    crate::vfs::write(&path, content)?;

    let mut transfers = TRANSFERS.lock();
    if transfers.len() >= MAX_TRANSFERS { transfers.remove(0); }
    transfers.push(ScpTransfer {
        id,
        session_id,
        direction: ScpDirection::Upload,
        filename: filename.to_owned(),
        size: data.len(),
        transferred: data.len(),
        completed: true,
        timestamp: crate::timer::ticks(),
    });

    crate::serial_println!("[scp] upload #{}: {} ({} bytes) -> {}", id, filename, data.len(), path);
    Ok(id)
}

/// Start an SCP download (server sends file to client).
/// Returns the file contents.
pub fn scp_download(session_id: u32, filename: &str) -> Result<(u32, String), &'static str> {
    let id = NEXT_TRANSFER_ID.fetch_add(1, Ordering::Relaxed);

    let path = if filename.starts_with('/') {
        filename.to_owned()
    } else {
        format!("/tmp/{}", filename)
    };

    let content = crate::vfs::cat(&path)?;
    let size = content.len();

    let mut transfers = TRANSFERS.lock();
    if transfers.len() >= MAX_TRANSFERS { transfers.remove(0); }
    transfers.push(ScpTransfer {
        id,
        session_id,
        direction: ScpDirection::Download,
        filename: filename.to_owned(),
        size,
        transferred: size,
        completed: true,
        timestamp: crate::timer::ticks(),
    });

    crate::serial_println!("[scp] download #{}: {} ({} bytes)", id, filename, size);
    Ok((id, content))
}

/// List recent SCP transfers.
pub fn list_transfers() -> String {
    let transfers = TRANSFERS.lock();
    if transfers.is_empty() {
        return String::from("No SCP transfers recorded.\n");
    }

    let mut out = format!("SCP transfers ({}):\n", transfers.len());
    out.push_str(&format!("{:>4} {:>6} {:<8} {:<20} {:>8} {}\n",
        "ID", "Sess", "Dir", "File", "Size", "Status"));

    for t in transfers.iter() {
        let dir = match t.direction {
            ScpDirection::Upload => "upload",
            ScpDirection::Download => "download",
        };
        let status = if t.completed { "done" } else { "partial" };
        out.push_str(&format!("{:>4} {:>6} {:<8} {:<20} {:>8} {}\n",
            t.id, t.session_id, dir, t.filename, t.size, status));
    }
    out
}

/// Initialize SCP/session module.
pub fn init() {
    crate::serial_println!("[scp] SSH session manager initialized");
    crate::klog_println!("[scp] initialized");
}
