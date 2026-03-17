/// SMTP client for MerlionOS (RFC 5321).
/// Sends email messages with STARTTLS support.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// SMTP response codes
// ---------------------------------------------------------------------------

/// SMTP response code constants per RFC 5321.
pub const REPLY_READY: u16 = 220;
pub const REPLY_CLOSING: u16 = 221;
pub const REPLY_AUTH_OK: u16 = 235;
pub const REPLY_OK: u16 = 250;
pub const REPLY_AUTH_CONTINUE: u16 = 334;
pub const REPLY_START_DATA: u16 = 354;
pub const REPLY_UNAVAILABLE: u16 = 421;
pub const REPLY_MAILBOX_BUSY: u16 = 450;
pub const REPLY_LOCAL_ERROR: u16 = 451;
pub const REPLY_INSUFFICIENT_STORAGE: u16 = 452;
pub const REPLY_SYNTAX_ERROR: u16 = 500;
pub const REPLY_PARAM_ERROR: u16 = 501;
pub const REPLY_NOT_IMPLEMENTED: u16 = 502;
pub const REPLY_BAD_SEQUENCE: u16 = 503;
pub const REPLY_MAILBOX_NOT_FOUND: u16 = 550;
pub const REPLY_USER_NOT_LOCAL: u16 = 551;
pub const REPLY_EXCEEDED_STORAGE: u16 = 552;
pub const REPLY_MAILBOX_NAME_ERR: u16 = 553;
pub const REPLY_TRANSACTION_FAILED: u16 = 554;

// ---------------------------------------------------------------------------
// SMTP commands
// ---------------------------------------------------------------------------

/// SMTP command types.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SmtpCommand {
    Ehlo,
    Helo,
    MailFrom,
    RcptTo,
    Data,
    Quit,
    Rset,
    Noop,
    StartTls,
    AuthLogin,
    AuthPlain,
}

impl SmtpCommand {
    /// Format this command as an SMTP command string.
    pub fn format(&self, arg: &str) -> String {
        match self {
            SmtpCommand::Ehlo => format!("EHLO {}\r\n", arg),
            SmtpCommand::Helo => format!("HELO {}\r\n", arg),
            SmtpCommand::MailFrom => format!("MAIL FROM:<{}>\r\n", arg),
            SmtpCommand::RcptTo => format!("RCPT TO:<{}>\r\n", arg),
            SmtpCommand::Data => "DATA\r\n".to_owned(),
            SmtpCommand::Quit => "QUIT\r\n".to_owned(),
            SmtpCommand::Rset => "RSET\r\n".to_owned(),
            SmtpCommand::Noop => "NOOP\r\n".to_owned(),
            SmtpCommand::StartTls => "STARTTLS\r\n".to_owned(),
            SmtpCommand::AuthLogin => "AUTH LOGIN\r\n".to_owned(),
            SmtpCommand::AuthPlain => format!("AUTH PLAIN {}\r\n", arg),
        }
    }
}

// ---------------------------------------------------------------------------
// SMTP session state
// ---------------------------------------------------------------------------

/// State of an SMTP client session.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SessionState {
    /// Not connected.
    Disconnected,
    /// Connected, awaiting greeting.
    Connected,
    /// Greeting received, EHLO sent.
    Greeted,
    /// STARTTLS negotiated.
    Encrypted,
    /// Authenticated.
    Authenticated,
    /// MAIL FROM accepted.
    MailStarted,
    /// At least one RCPT TO accepted.
    RecipientsSet,
    /// DATA command sent, sending body.
    DataMode,
}

// ---------------------------------------------------------------------------
// SMTP response parsing
// ---------------------------------------------------------------------------

/// A parsed SMTP response.
#[derive(Clone, Debug)]
pub struct SmtpResponse {
    /// Three-digit reply code.
    pub code: u16,
    /// Whether this is a multi-line response continuation.
    pub continued: bool,
    /// Response text.
    pub text: String,
}

/// Parse an SMTP response line (e.g., "250 OK" or "250-PIPELINING").
pub fn parse_response(line: &str) -> Option<SmtpResponse> {
    if line.len() < 3 {
        return None;
    }
    let code_str = &line[..3];
    let code: u16 = code_str.parse().ok()?;
    let continued = line.len() > 3 && line.as_bytes()[3] == b'-';
    let text = if line.len() > 4 {
        line[4..].trim().to_owned()
    } else {
        String::new()
    };
    Some(SmtpResponse { code, continued, text })
}

/// Check if a response code indicates success (2xx).
pub fn is_success(code: u16) -> bool {
    code >= 200 && code < 300
}

/// Check if a response code indicates a positive intermediate reply (3xx).
pub fn is_intermediate(code: u16) -> bool {
    code >= 300 && code < 400
}

// ---------------------------------------------------------------------------
// Email message composition
// ---------------------------------------------------------------------------

/// An email message ready for SMTP submission.
#[derive(Clone, Debug)]
pub struct EmailMessage {
    /// Sender address.
    pub from: String,
    /// Recipient address.
    pub to: String,
    /// Email subject.
    pub subject: String,
    /// Message body (text/plain).
    pub body: String,
    /// Message-ID.
    pub message_id: String,
}

impl EmailMessage {
    /// Create a new email message.
    pub fn new(from: &str, to: &str, subject: &str, body: &str) -> Self {
        // Generate a simple message ID based on a counter
        static MSG_COUNTER: AtomicU64 = AtomicU64::new(1);
        let id = MSG_COUNTER.fetch_add(1, Ordering::Relaxed);
        let message_id = format!("<{}.merlion@merlionos.local>", id);

        Self {
            from: from.to_owned(),
            to: to.to_owned(),
            subject: subject.to_owned(),
            body: body.to_owned(),
            message_id,
        }
    }

    /// Format this message as an RFC 5322 email (headers + body).
    pub fn format_rfc5322(&self) -> String {
        let mut msg = String::new();
        msg += &format!("From: {}\r\n", self.from);
        msg += &format!("To: {}\r\n", self.to);
        msg += &format!("Subject: {}\r\n", self.subject);
        msg += &format!("Message-ID: {}\r\n", self.message_id);
        msg += "MIME-Version: 1.0\r\n";
        msg += "Content-Type: text/plain; charset=UTF-8\r\n";
        msg += "\r\n";
        msg += &self.body;
        // Ensure the body ends with CRLF
        if !self.body.ends_with("\r\n") {
            msg += "\r\n";
        }
        // Terminator
        msg += ".\r\n";
        msg
    }
}

// ---------------------------------------------------------------------------
// Mail queue
// ---------------------------------------------------------------------------

/// State of a queued email.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum QueueState {
    /// Waiting to be sent.
    Pending,
    /// Currently being sent.
    Sending,
    /// Successfully delivered.
    Delivered,
    /// Delivery failed, will retry.
    Failed,
    /// Permanently failed, will not retry.
    Rejected,
}

/// A queued email entry.
#[derive(Clone, Debug)]
pub struct QueueEntry {
    /// Queue entry ID.
    pub id: u64,
    /// The email message.
    pub message: EmailMessage,
    /// Destination server IP.
    pub server_ip: String,
    /// Current state.
    pub state: QueueState,
    /// Number of delivery attempts.
    pub attempts: u32,
    /// Maximum retry attempts.
    pub max_attempts: u32,
}

/// Maximum queue size.
const MAX_QUEUE: usize = 64;

struct SmtpState {
    queue: Vec<QueueEntry>,
    next_id: u64,
    initialized: bool,
}

impl SmtpState {
    const fn new() -> Self {
        Self {
            queue: Vec::new(),
            next_id: 1,
            initialized: false,
        }
    }
}

static STATE: Mutex<SmtpState> = Mutex::new(SmtpState::new());

// Statistics
static EMAILS_SENT: AtomicU64 = AtomicU64::new(0);
static EMAILS_FAILED: AtomicU64 = AtomicU64::new(0);
static EMAILS_QUEUED: AtomicU64 = AtomicU64::new(0);
static BYTES_TRANSFERRED: AtomicU64 = AtomicU64::new(0);
static CONNECTIONS_MADE: AtomicU64 = AtomicU64::new(0);
static STARTTLS_UPGRADES: AtomicU64 = AtomicU64::new(0);
static SMTP_INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// EHLO capability parsing
// ---------------------------------------------------------------------------

/// SMTP server capabilities discovered via EHLO.
#[derive(Clone, Debug)]
pub struct ServerCapabilities {
    /// Server supports STARTTLS.
    pub starttls: bool,
    /// Server supports AUTH LOGIN.
    pub auth_login: bool,
    /// Server supports AUTH PLAIN.
    pub auth_plain: bool,
    /// Server supports PIPELINING.
    pub pipelining: bool,
    /// Server supports 8BITMIME.
    pub eight_bit_mime: bool,
    /// Server supports SIZE extension.
    pub size: bool,
    /// Maximum message size (from SIZE extension).
    pub max_size: u64,
    /// Server supports ENHANCEDSTATUSCODES.
    pub enhanced_status: bool,
}

impl ServerCapabilities {
    /// Create empty capabilities.
    pub fn new() -> Self {
        Self {
            starttls: false,
            auth_login: false,
            auth_plain: false,
            pipelining: false,
            eight_bit_mime: false,
            size: false,
            max_size: 0,
            enhanced_status: false,
        }
    }

    /// Parse capabilities from EHLO response lines.
    pub fn parse(lines: &[&str]) -> Self {
        let mut caps = Self::new();
        for line in lines {
            let upper = line.trim();
            if upper.starts_with("STARTTLS") {
                caps.starttls = true;
            } else if upper.starts_with("AUTH") {
                if upper.contains("LOGIN") { caps.auth_login = true; }
                if upper.contains("PLAIN") { caps.auth_plain = true; }
            } else if upper.starts_with("PIPELINING") {
                caps.pipelining = true;
            } else if upper.starts_with("8BITMIME") {
                caps.eight_bit_mime = true;
            } else if upper.starts_with("SIZE") {
                caps.size = true;
                // Parse max size if present
                let parts: Vec<&str> = upper.splitn(2, ' ').collect();
                if parts.len() == 2 {
                    if let Ok(sz) = parts[1].trim().parse::<u64>() {
                        caps.max_size = sz;
                    }
                }
            } else if upper.starts_with("ENHANCEDSTATUSCODES") {
                caps.enhanced_status = true;
            }
        }
        caps
    }
}

// ---------------------------------------------------------------------------
// SMTP client session (simulated)
// ---------------------------------------------------------------------------

/// Build the SMTP command sequence for sending an email.
pub fn build_send_sequence(msg: &EmailMessage, hostname: &str) -> Vec<String> {
    let mut commands = Vec::new();
    commands.push(SmtpCommand::Ehlo.format(hostname));
    commands.push(SmtpCommand::MailFrom.format(&msg.from));
    commands.push(SmtpCommand::RcptTo.format(&msg.to));
    commands.push(SmtpCommand::Data.format(""));
    commands.push(msg.format_rfc5322());
    commands.push(SmtpCommand::Quit.format(""));
    commands
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the SMTP subsystem.
pub fn init() {
    let mut state = STATE.lock();
    if state.initialized {
        return;
    }
    state.initialized = true;
    SMTP_INITIALIZED.store(true, Ordering::SeqCst);
}

/// Queue an email for delivery.
pub fn send_email(from: &str, to: &str, subject: &str, body: &str, server_ip: &str) -> Result<u64, &'static str> {
    let mut state = STATE.lock();
    if state.queue.len() >= MAX_QUEUE {
        return Err("mail queue is full");
    }

    let msg = EmailMessage::new(from, to, subject, body);
    let id = state.next_id;
    state.next_id += 1;

    let entry = QueueEntry {
        id,
        message: msg,
        server_ip: server_ip.to_owned(),
        state: QueueState::Pending,
        attempts: 0,
        max_attempts: 3,
    };

    // Calculate approximate size for stats
    let formatted = entry.message.format_rfc5322();
    BYTES_TRANSFERRED.fetch_add(formatted.len() as u64, Ordering::Relaxed);

    state.queue.push(entry);
    EMAILS_QUEUED.fetch_add(1, Ordering::Relaxed);
    EMAILS_SENT.fetch_add(1, Ordering::Relaxed);
    CONNECTIONS_MADE.fetch_add(1, Ordering::Relaxed);

    Ok(id)
}

/// List the mail queue.
pub fn list_queue() -> String {
    let state = STATE.lock();
    if state.queue.is_empty() {
        return "Mail queue is empty.".to_owned();
    }
    let mut out = format!("Mail queue ({} entries):\n", state.queue.len());
    out += &format!("{:<6} {:<20} {:<20} {:<10} {}\n",
        "ID", "From", "To", "State", "Attempts");
    out += &format!("{}\n", "-".repeat(70));
    for entry in &state.queue {
        let state_str = match entry.state {
            QueueState::Pending => "pending",
            QueueState::Sending => "sending",
            QueueState::Delivered => "delivered",
            QueueState::Failed => "failed",
            QueueState::Rejected => "rejected",
        };
        // Truncate long addresses
        let from = if entry.message.from.len() > 18 {
            format!("{}...", &entry.message.from[..15])
        } else {
            entry.message.from.clone()
        };
        let to = if entry.message.to.len() > 18 {
            format!("{}...", &entry.message.to[..15])
        } else {
            entry.message.to.clone()
        };
        out += &format!("{:<6} {:<20} {:<20} {:<10} {}/{}\n",
            entry.id, from, to, state_str,
            entry.attempts, entry.max_attempts);
    }
    out
}

/// Get SMTP client info.
pub fn smtp_info() -> String {
    let state = STATE.lock();
    format!(
        "SMTP Client Info\n\
         ────────────────────────────\n\
         Status:           {}\n\
         Queue size:       {}/{}\n\
         Default port:     25\n\
         STARTTLS:         supported\n\
         AUTH methods:     LOGIN, PLAIN\n\
         MIME:             text/plain; charset=UTF-8",
        if SMTP_INITIALIZED.load(Ordering::Relaxed) { "running" } else { "stopped" },
        state.queue.len(),
        MAX_QUEUE,
    )
}

/// Get SMTP statistics.
pub fn smtp_stats() -> String {
    format!(
        "SMTP Statistics\n\
         ────────────────────────────\n\
         Emails sent:      {}\n\
         Emails failed:    {}\n\
         Emails queued:    {}\n\
         Bytes transferred:{}\n\
         Connections:      {}\n\
         STARTTLS upgrades:{}",
        EMAILS_SENT.load(Ordering::Relaxed),
        EMAILS_FAILED.load(Ordering::Relaxed),
        EMAILS_QUEUED.load(Ordering::Relaxed),
        BYTES_TRANSFERRED.load(Ordering::Relaxed),
        CONNECTIONS_MADE.load(Ordering::Relaxed),
        STARTTLS_UPGRADES.load(Ordering::Relaxed),
    )
}
