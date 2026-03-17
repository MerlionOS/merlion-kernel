/// FTP server for MerlionOS.
/// Implements the FTP protocol (RFC 959) for file transfer.
/// Supports login, directory browsing, upload/download, and passive mode.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default FTP control port.
const DEFAULT_PORT: u16 = 21;

/// Passive mode data port range start.
const PASV_PORT_MIN: u16 = 30000;

/// Passive mode data port range end.
const PASV_PORT_MAX: u16 = 30100;

/// Maximum concurrent sessions.
const MAX_SESSIONS: usize = 8;

/// Maximum command line length.
const MAX_CMD_LEN: usize = 512;

// ---------------------------------------------------------------------------
// FTP response codes
// ---------------------------------------------------------------------------

const REPLY_DATA_OPEN: u16 = 150;
const REPLY_OK: u16 = 200;
const REPLY_SYST: u16 = 215;
const REPLY_WELCOME: u16 = 220;
const REPLY_QUIT: u16 = 221;
const REPLY_TRANSFER_COMPLETE: u16 = 226;
const REPLY_LOGGED_IN: u16 = 230;
const REPLY_ACTION_OK: u16 = 250;
const REPLY_PWD: u16 = 257;
const REPLY_NEED_PASS: u16 = 331;
const REPLY_TIMEOUT: u16 = 421;
const REPLY_CANT_OPEN_DATA: u16 = 425;
const REPLY_NOT_AVAILABLE: u16 = 450;
const REPLY_SYNTAX_ERROR: u16 = 500;
const REPLY_NOT_LOGGED_IN: u16 = 530;
const REPLY_NOT_FOUND: u16 = 550;

// ---------------------------------------------------------------------------
// Transfer type
// ---------------------------------------------------------------------------

/// FTP transfer type: ASCII or Binary (Image).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferType {
    /// ASCII mode (text).
    Ascii,
    /// Binary/Image mode.
    Binary,
}

// ---------------------------------------------------------------------------
// FTP session
// ---------------------------------------------------------------------------

/// State for a single FTP client session.
pub struct FtpSession {
    /// Session identifier.
    pub id: u32,
    /// Client IP address.
    pub client_ip: [u8; 4],
    /// Whether the user has authenticated.
    pub authenticated: bool,
    /// Username of the logged-in user.
    pub username: String,
    /// Current working directory path.
    pub cwd: String,
    /// Current transfer type.
    pub transfer_type: TransferType,
    /// Passive mode data port (if PASV was issued).
    pub passive_port: Option<u16>,
    /// Data offset for resumed transfers.
    pub data_offset: usize,
}

impl FtpSession {
    /// Create a new unauthenticated session.
    fn new(id: u32, client_ip: [u8; 4]) -> Self {
        Self {
            id,
            client_ip,
            authenticated: false,
            username: String::new(),
            cwd: String::from("/"),
            transfer_type: TransferType::Binary,
            passive_port: None,
            data_offset: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Active FTP sessions.
static SESSIONS: Mutex<Vec<FtpSession>> = Mutex::new(Vec::new());

/// Next session ID counter.
static NEXT_SESSION_ID: AtomicU32 = AtomicU32::new(1);

/// Next passive port to allocate.
static NEXT_PASV_PORT: AtomicU32 = AtomicU32::new(PASV_PORT_MIN as u32);

/// Total sessions ever created.
static TOTAL_SESSIONS: AtomicU64 = AtomicU64::new(0);

/// Total files transferred (upload + download).
static FILES_TRANSFERRED: AtomicU64 = AtomicU64::new(0);

/// Total bytes uploaded.
static BYTES_UPLOADED: AtomicU64 = AtomicU64::new(0);

/// Total bytes downloaded.
static BYTES_DOWNLOADED: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// FTP reply formatting
// ---------------------------------------------------------------------------

/// Format a single-line FTP reply.
fn ftp_reply(code: u16, message: &str) -> String {
    format!("{} {}\r\n", code, message)
}

/// Format a multi-line FTP reply (for LIST, FEAT, etc.).
fn ftp_reply_multi(code: u16, lines: &[&str], end_msg: &str) -> String {
    let mut out = String::new();
    if let Some((first, rest)) = lines.split_first() {
        out.push_str(&format!("{}-{}\r\n", code, first));
        for line in rest {
            out.push_str(&format!(" {}\r\n", line));
        }
    }
    out.push_str(&format!("{} {}\r\n", code, end_msg));
    out
}

// ---------------------------------------------------------------------------
// Path resolution
// ---------------------------------------------------------------------------

/// Resolve a path relative to the session's current working directory.
fn resolve_path(cwd: &str, path: &str) -> String {
    if path.starts_with('/') {
        normalize_path(path)
    } else {
        let combined = if cwd.ends_with('/') {
            format!("{}{}", cwd, path)
        } else {
            format!("{}/{}", cwd, path)
        };
        normalize_path(&combined)
    }
}

/// Normalize a path, collapsing ".." and "." components.
fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => { parts.pop(); }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        String::from("/")
    } else {
        let mut result = String::new();
        for p in &parts {
            result.push('/');
            result.push_str(p);
        }
        result
    }
}

/// Get the parent directory of a path.
fn parent_path(path: &str) -> String {
    if path == "/" {
        return String::from("/");
    }
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => String::from("/"),
        Some(i) => String::from(&trimmed[..i]),
    }
}

// ---------------------------------------------------------------------------
// Passive port allocation
// ---------------------------------------------------------------------------

/// Allocate the next passive mode data port in the 30000-30100 range.
fn allocate_passive_port() -> u16 {
    let port = NEXT_PASV_PORT.fetch_add(1, Ordering::SeqCst) as u16;
    if port > PASV_PORT_MAX {
        NEXT_PASV_PORT.store(PASV_PORT_MIN as u32, Ordering::SeqCst);
        PASV_PORT_MIN
    } else {
        port
    }
}

// ---------------------------------------------------------------------------
// FTP command processing
// ---------------------------------------------------------------------------

/// Parse an FTP command line into (command, argument).
fn parse_ftp_command(line: &str) -> (&str, &str) {
    let trimmed = line.trim();
    match trimmed.find(' ') {
        Some(i) => (&trimmed[..i], trimmed[i + 1..].trim()),
        None => (trimmed, ""),
    }
}

/// Process a single FTP command for the given session.
/// Returns the reply string to send to the client.
pub fn process_command(session: &mut FtpSession, line: &str) -> String {
    let (cmd, arg) = parse_ftp_command(line);
    let cmd_upper: String = cmd.chars().map(|c| {
        if c >= 'a' && c <= 'z' { (c as u8 - 32) as char } else { c }
    }).collect();

    match cmd_upper.as_str() {
        "USER" => cmd_user(session, arg),
        "PASS" => cmd_pass(session, arg),
        "SYST" => ftp_reply(REPLY_SYST, "UNIX Type: L8"),
        "FEAT" => cmd_feat(),
        "PWD" | "XPWD" => cmd_pwd(session),
        "CWD" | "XCWD" => cmd_cwd(session, arg),
        "CDUP" | "XCUP" => cmd_cdup(session),
        "TYPE" => cmd_type(session, arg),
        "PASV" => cmd_pasv(session),
        "LIST" => cmd_list(session, arg),
        "NLST" => cmd_nlst(session, arg),
        "RETR" => cmd_retr(session, arg),
        "STOR" => cmd_stor(session, arg),
        "DELE" => cmd_dele(session, arg),
        "MKD" | "XMKD" => cmd_mkd(session, arg),
        "RMD" | "XRMD" => cmd_rmd(session, arg),
        "SIZE" => cmd_size(session, arg),
        "NOOP" => ftp_reply(REPLY_OK, "NOOP ok"),
        "QUIT" => cmd_quit(session),
        _ => ftp_reply(REPLY_SYNTAX_ERROR, "Command not recognized"),
    }
}

// ---------------------------------------------------------------------------
// Command implementations
// ---------------------------------------------------------------------------

fn cmd_user(session: &mut FtpSession, username: &str) -> String {
    session.username = String::from(username);
    session.authenticated = false;
    crate::serial_println!("[ftpd] USER {} from {}.{}.{}.{}",
        username, session.client_ip[0], session.client_ip[1],
        session.client_ip[2], session.client_ip[3]);
    ftp_reply(REPLY_NEED_PASS, "Please specify the password.")
}

fn cmd_pass(session: &mut FtpSession, password: &str) -> String {
    if session.username.is_empty() {
        return ftp_reply(REPLY_SYNTAX_ERROR, "Login with USER first.");
    }
    let hash = crate::security::hash_password(password);
    if crate::security::authenticate(&session.username, hash) {
        session.authenticated = true;
        crate::serial_println!("[ftpd] session {} authenticated as {}",
            session.id, session.username);
        ftp_reply(REPLY_LOGGED_IN, "Login successful.")
    } else {
        crate::serial_println!("[ftpd] session {} auth failed for {}",
            session.id, session.username);
        ftp_reply(REPLY_NOT_LOGGED_IN, "Login incorrect.")
    }
}

fn cmd_feat() -> String {
    ftp_reply_multi(211, &["Features:"], "End")
}

fn cmd_pwd(session: &FtpSession) -> String {
    if !session.authenticated {
        return ftp_reply(REPLY_NOT_LOGGED_IN, "Please login with USER and PASS.");
    }
    ftp_reply(REPLY_PWD, &format!("\"{}\" is the current directory", session.cwd))
}

fn cmd_cwd(session: &mut FtpSession, path: &str) -> String {
    if !session.authenticated {
        return ftp_reply(REPLY_NOT_LOGGED_IN, "Please login with USER and PASS.");
    }
    let target = resolve_path(&session.cwd, path);
    // Verify directory exists via VFS ls
    match crate::vfs::ls(&target) {
        Ok(_) => {
            session.cwd = target;
            ftp_reply(REPLY_ACTION_OK, "Directory changed.")
        }
        Err(_) => ftp_reply(REPLY_NOT_FOUND, "Failed to change directory."),
    }
}

fn cmd_cdup(session: &mut FtpSession) -> String {
    if !session.authenticated {
        return ftp_reply(REPLY_NOT_LOGGED_IN, "Please login with USER and PASS.");
    }
    session.cwd = parent_path(&session.cwd);
    ftp_reply(REPLY_ACTION_OK, "Directory changed.")
}

fn cmd_type(session: &mut FtpSession, arg: &str) -> String {
    match arg.chars().next() {
        Some('A') | Some('a') => {
            session.transfer_type = TransferType::Ascii;
            ftp_reply(REPLY_OK, "Switching to ASCII mode.")
        }
        Some('I') | Some('i') => {
            session.transfer_type = TransferType::Binary;
            ftp_reply(REPLY_OK, "Switching to Binary mode.")
        }
        _ => ftp_reply(REPLY_SYNTAX_ERROR, "Unrecognized TYPE command."),
    }
}

fn cmd_pasv(session: &mut FtpSession) -> String {
    if !session.authenticated {
        return ftp_reply(REPLY_NOT_LOGGED_IN, "Please login with USER and PASS.");
    }
    let port = allocate_passive_port();
    session.passive_port = Some(port);
    let ip = crate::net::NET.lock().ip;
    let p1 = (port >> 8) as u8;
    let p2 = (port & 0xFF) as u8;
    ftp_reply(227, &format!("Entering Passive Mode ({},{},{},{},{},{}).",
        ip.0[0], ip.0[1], ip.0[2], ip.0[3], p1, p2))
}

fn cmd_list(session: &mut FtpSession, arg: &str) -> String {
    if !session.authenticated {
        return ftp_reply(REPLY_NOT_LOGGED_IN, "Please login with USER and PASS.");
    }
    let target = if arg.is_empty() || arg.starts_with('-') {
        session.cwd.clone()
    } else {
        resolve_path(&session.cwd, arg)
    };
    match crate::vfs::ls(&target) {
        Ok(entries) => {
            let mut listing = String::new();
            for (name, type_char) in &entries {
                let perm = if *type_char == 'd' {
                    "drwxr-xr-x"
                } else {
                    "-rw-r--r--"
                };
                let size = if *type_char == 'f' {
                    let full = resolve_path(&target, name);
                    crate::vfs::cat(&full).map(|c| c.len()).unwrap_or(0)
                } else {
                    0
                };
                listing.push_str(&format!(
                    "{} 1 root root {:>8} Jan 01 00:00 {}\r\n",
                    perm, size, name
                ));
            }
            let mut reply = ftp_reply(REPLY_DATA_OPEN, "Here comes the directory listing.");
            reply.push_str(&listing);
            reply.push_str(&ftp_reply(REPLY_TRANSFER_COMPLETE, "Directory send OK."));
            reply
        }
        Err(_) => ftp_reply(REPLY_NOT_FOUND, "Failed to list directory."),
    }
}

fn cmd_nlst(session: &mut FtpSession, arg: &str) -> String {
    if !session.authenticated {
        return ftp_reply(REPLY_NOT_LOGGED_IN, "Please login with USER and PASS.");
    }
    let target = if arg.is_empty() || arg.starts_with('-') {
        session.cwd.clone()
    } else {
        resolve_path(&session.cwd, arg)
    };
    match crate::vfs::ls(&target) {
        Ok(entries) => {
            let mut listing = String::new();
            for (name, _) in &entries {
                listing.push_str(name);
                listing.push_str("\r\n");
            }
            let mut reply = ftp_reply(REPLY_DATA_OPEN, "Here comes the directory listing.");
            reply.push_str(&listing);
            reply.push_str(&ftp_reply(REPLY_TRANSFER_COMPLETE, "Directory send OK."));
            reply
        }
        Err(_) => ftp_reply(REPLY_NOT_FOUND, "Failed to list directory."),
    }
}

fn cmd_retr(session: &mut FtpSession, arg: &str) -> String {
    if !session.authenticated {
        return ftp_reply(REPLY_NOT_LOGGED_IN, "Please login with USER and PASS.");
    }
    if session.passive_port.is_none() {
        return ftp_reply(REPLY_CANT_OPEN_DATA, "Use PASV first.");
    }
    let path = resolve_path(&session.cwd, arg);
    match crate::vfs::cat(&path) {
        Ok(content) => {
            let bytes = content.len() as u64;
            BYTES_DOWNLOADED.fetch_add(bytes, Ordering::Relaxed);
            FILES_TRANSFERRED.fetch_add(1, Ordering::Relaxed);
            crate::serial_println!("[ftpd] RETR {} ({} bytes) session {}",
                path, bytes, session.id);
            let mut reply = ftp_reply(REPLY_DATA_OPEN,
                &format!("Opening {} mode data connection for {}.",
                    if session.transfer_type == TransferType::Ascii { "ASCII" } else { "BINARY" },
                    arg));
            reply.push_str(&content);
            reply.push_str(&ftp_reply(REPLY_TRANSFER_COMPLETE, "Transfer complete."));
            reply
        }
        Err(_) => ftp_reply(REPLY_NOT_FOUND, "File not found."),
    }
}

fn cmd_stor(session: &mut FtpSession, arg: &str) -> String {
    if !session.authenticated {
        return ftp_reply(REPLY_NOT_LOGGED_IN, "Please login with USER and PASS.");
    }
    if session.passive_port.is_none() {
        return ftp_reply(REPLY_CANT_OPEN_DATA, "Use PASV first.");
    }
    let path = resolve_path(&session.cwd, arg);
    // In a real implementation, data would come via the data connection.
    // Here we create an empty file as a placeholder.
    match crate::vfs::write(&path, "") {
        Ok(()) => {
            FILES_TRANSFERRED.fetch_add(1, Ordering::Relaxed);
            crate::serial_println!("[ftpd] STOR {} session {}", path, session.id);
            let mut reply = ftp_reply(REPLY_DATA_OPEN,
                &format!("Opening {} mode data connection for {}.",
                    if session.transfer_type == TransferType::Ascii { "ASCII" } else { "BINARY" },
                    arg));
            reply.push_str(&ftp_reply(REPLY_TRANSFER_COMPLETE, "Transfer complete."));
            reply
        }
        Err(_) => ftp_reply(REPLY_NOT_AVAILABLE, "File write failed."),
    }
}

fn cmd_dele(session: &FtpSession, arg: &str) -> String {
    if !session.authenticated {
        return ftp_reply(REPLY_NOT_LOGGED_IN, "Please login with USER and PASS.");
    }
    let path = resolve_path(&session.cwd, arg);
    match crate::vfs::rm(&path) {
        Ok(()) => {
            crate::serial_println!("[ftpd] DELE {} session {}", path, session.id);
            ftp_reply(REPLY_ACTION_OK, "Delete operation successful.")
        }
        Err(_) => ftp_reply(REPLY_NOT_FOUND, "Delete operation failed."),
    }
}

fn cmd_mkd(session: &FtpSession, arg: &str) -> String {
    if !session.authenticated {
        return ftp_reply(REPLY_NOT_LOGGED_IN, "Please login with USER and PASS.");
    }
    let path = resolve_path(&session.cwd, arg);
    match crate::vfs::mkdir(&path) {
        Ok(()) => {
            crate::serial_println!("[ftpd] MKD {} session {}", path, session.id);
            ftp_reply(REPLY_PWD, &format!("\"{}\" created", path))
        }
        Err(_) => ftp_reply(REPLY_NOT_FOUND, "Create directory operation failed."),
    }
}

fn cmd_rmd(session: &FtpSession, arg: &str) -> String {
    if !session.authenticated {
        return ftp_reply(REPLY_NOT_LOGGED_IN, "Please login with USER and PASS.");
    }
    let path = resolve_path(&session.cwd, arg);
    match crate::vfs::rm(&path) {
        Ok(()) => {
            crate::serial_println!("[ftpd] RMD {} session {}", path, session.id);
            ftp_reply(REPLY_ACTION_OK, "Remove directory operation successful.")
        }
        Err(_) => ftp_reply(REPLY_NOT_FOUND, "Remove directory operation failed."),
    }
}

fn cmd_size(session: &FtpSession, arg: &str) -> String {
    if !session.authenticated {
        return ftp_reply(REPLY_NOT_LOGGED_IN, "Please login with USER and PASS.");
    }
    let path = resolve_path(&session.cwd, arg);
    match crate::vfs::cat(&path) {
        Ok(content) => ftp_reply(213, &format!("{}", content.len())),
        Err(_) => ftp_reply(REPLY_NOT_FOUND, "Could not get file size."),
    }
}

fn cmd_quit(session: &mut FtpSession) -> String {
    crate::serial_println!("[ftpd] session {} disconnected (QUIT)", session.id);
    // Remove from active sessions
    let mut sessions = SESSIONS.lock();
    sessions.retain(|s| s.id != session.id);
    ftp_reply(REPLY_QUIT, "Goodbye.")
}

// ---------------------------------------------------------------------------
// Session management
// ---------------------------------------------------------------------------

/// Create a new FTP session for an incoming connection.
pub fn create_session(client_ip: [u8; 4]) -> u32 {
    let id = NEXT_SESSION_ID.fetch_add(1, Ordering::SeqCst);
    TOTAL_SESSIONS.fetch_add(1, Ordering::Relaxed);
    let session = FtpSession::new(id, client_ip);
    let mut sessions = SESSIONS.lock();
    // Enforce max sessions
    if sessions.len() >= MAX_SESSIONS {
        sessions.remove(0);
    }
    sessions.push(session);
    crate::serial_println!("[ftpd] new session {} from {}.{}.{}.{}",
        id, client_ip[0], client_ip[1], client_ip[2], client_ip[3]);
    id
}

/// Get the welcome banner for a new FTP connection.
pub fn welcome_banner() -> String {
    ftp_reply(REPLY_WELCOME, "MerlionOS FTP server ready.")
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the FTP server subsystem.
pub fn init() {
    crate::serial_println!("[ftpd] FTP server initialized (port {})", DEFAULT_PORT);
    crate::klog_println!("[ftpd] initialized");
}

/// Return FTP server information string.
pub fn ftpd_info() -> String {
    let sessions = SESSIONS.lock();
    let active = sessions.len();
    let ip = crate::net::NET.lock().ip;
    format!(
        "MerlionOS FTP Server (RFC 959)\n\
         Status:         running\n\
         Listen address: {}:{}\n\
         Active sessions: {}\n\
         Max sessions:   {}\n\
         Passive ports:  {}-{}\n\
         Transfer types: ASCII, Binary\n\
         Commands:       USER PASS PWD CWD CDUP LIST NLST RETR STOR\n\
                         DELE MKD RMD PASV TYPE SIZE SYST FEAT NOOP QUIT\n",
        ip, DEFAULT_PORT, active, MAX_SESSIONS,
        PASV_PORT_MIN, PASV_PORT_MAX,
    )
}

/// Return FTP server statistics.
pub fn ftpd_stats() -> String {
    let total = TOTAL_SESSIONS.load(Ordering::Relaxed);
    let active = SESSIONS.lock().len();
    let files = FILES_TRANSFERRED.load(Ordering::Relaxed);
    let up = BYTES_UPLOADED.load(Ordering::Relaxed);
    let down = BYTES_DOWNLOADED.load(Ordering::Relaxed);
    format!(
        "FTP Server Statistics\n\
         Total sessions:    {}\n\
         Active sessions:   {}\n\
         Files transferred: {}\n\
         Bytes uploaded:    {}\n\
         Bytes downloaded:  {}\n\
         Bytes total:       {}\n",
        total, active, files, up, down, up + down,
    )
}

/// List all active FTP sessions.
pub fn list_sessions() -> String {
    let sessions = SESSIONS.lock();
    if sessions.is_empty() {
        return String::from("No active FTP sessions.\n");
    }
    let mut out = format!("{:<6} {:<16} {:<10} {:<8} {:<6}\n",
        "ID", "Client IP", "User", "Type", "CWD");
    out.push_str(&format!("{}\n", "-".repeat(50)));
    for s in sessions.iter() {
        let ip_str = format!("{}.{}.{}.{}",
            s.client_ip[0], s.client_ip[1], s.client_ip[2], s.client_ip[3]);
        let user = if s.authenticated { s.username.as_str() } else { "(none)" };
        let ttype = if s.transfer_type == TransferType::Ascii { "ASCII" } else { "BIN" };
        out.push_str(&format!("{:<6} {:<16} {:<10} {:<8} {}\n",
            s.id, ip_str, user, ttype, s.cwd));
    }
    out
}
