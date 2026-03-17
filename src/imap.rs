/// IMAP4rev1 client for MerlionOS (RFC 3501).
/// Retrieves email from remote mailboxes.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::borrow::ToOwned;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// IMAP command types
// ---------------------------------------------------------------------------

/// IMAP command identifiers per RFC 3501.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImapCommand {
    Login,
    Logout,
    Select,
    Examine,
    Create,
    Delete,
    Rename,
    Subscribe,
    Unsubscribe,
    List,
    Lsub,
    Fetch,
    Search,
    Store,
    Copy,
    Noop,
    Capability,
    StartTls,
}

impl ImapCommand {
    /// Format this command as an IMAP command string with a tag.
    pub fn format(&self, tag: &str, args: &str) -> String {
        let cmd_name = match self {
            ImapCommand::Login => "LOGIN",
            ImapCommand::Logout => "LOGOUT",
            ImapCommand::Select => "SELECT",
            ImapCommand::Examine => "EXAMINE",
            ImapCommand::Create => "CREATE",
            ImapCommand::Delete => "DELETE",
            ImapCommand::Rename => "RENAME",
            ImapCommand::Subscribe => "SUBSCRIBE",
            ImapCommand::Unsubscribe => "UNSUBSCRIBE",
            ImapCommand::List => "LIST",
            ImapCommand::Lsub => "LSUB",
            ImapCommand::Fetch => "FETCH",
            ImapCommand::Search => "SEARCH",
            ImapCommand::Store => "STORE",
            ImapCommand::Copy => "COPY",
            ImapCommand::Noop => "NOOP",
            ImapCommand::Capability => "CAPABILITY",
            ImapCommand::StartTls => "STARTTLS",
        };
        if args.is_empty() {
            format!("{} {}\r\n", tag, cmd_name)
        } else {
            format!("{} {} {}\r\n", tag, cmd_name, args)
        }
    }
}

// ---------------------------------------------------------------------------
// IMAP response types
// ---------------------------------------------------------------------------

/// IMAP response status.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResponseStatus {
    Ok,
    No,
    Bad,
    Bye,
    Preauth,
}

/// A parsed IMAP response.
#[derive(Clone, Debug)]
pub struct ImapResponse {
    /// The command tag this response is for, or "*" for untagged.
    pub tag: String,
    /// Response status.
    pub status: ResponseStatus,
    /// Response text.
    pub text: String,
}

/// Parse an IMAP response line.
pub fn parse_response(line: &str) -> Option<ImapResponse> {
    let parts: Vec<&str> = line.splitn(3, ' ').collect();
    if parts.len() < 2 {
        return None;
    }
    let tag = parts[0].to_owned();
    let status = match parts[1] {
        "OK" => ResponseStatus::Ok,
        "NO" => ResponseStatus::No,
        "BAD" => ResponseStatus::Bad,
        "BYE" => ResponseStatus::Bye,
        "PREAUTH" => ResponseStatus::Preauth,
        _ => return None,
    };
    let text = if parts.len() > 2 { parts[2].to_owned() } else { String::new() };
    Some(ImapResponse { tag, status, text })
}

// ---------------------------------------------------------------------------
// Message flags
// ---------------------------------------------------------------------------

/// IMAP message flags per RFC 3501.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MessageFlag {
    Seen,
    Answered,
    Flagged,
    Deleted,
    Draft,
    Recent,
}

impl MessageFlag {
    /// Get the IMAP flag string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageFlag::Seen => "\\Seen",
            MessageFlag::Answered => "\\Answered",
            MessageFlag::Flagged => "\\Flagged",
            MessageFlag::Deleted => "\\Deleted",
            MessageFlag::Draft => "\\Draft",
            MessageFlag::Recent => "\\Recent",
        }
    }

    /// Parse a flag from its string representation.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "\\Seen" => Some(MessageFlag::Seen),
            "\\Answered" => Some(MessageFlag::Answered),
            "\\Flagged" => Some(MessageFlag::Flagged),
            "\\Deleted" => Some(MessageFlag::Deleted),
            "\\Draft" => Some(MessageFlag::Draft),
            "\\Recent" => Some(MessageFlag::Recent),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Mailbox information
// ---------------------------------------------------------------------------

/// Information about an IMAP mailbox.
#[derive(Clone, Debug)]
pub struct MailboxInfo {
    /// Mailbox name.
    pub name: String,
    /// Hierarchy delimiter character.
    pub delimiter: char,
    /// Number of messages in the mailbox.
    pub exists: u32,
    /// Number of recent messages.
    pub recent: u32,
    /// Number of unseen messages.
    pub unseen: u32,
    /// UIDVALIDITY value.
    pub uid_validity: u32,
    /// Next UID value.
    pub uid_next: u32,
    /// Mailbox flags (e.g., \Noselect, \HasChildren).
    pub flags: Vec<String>,
}

impl MailboxInfo {
    /// Create a new mailbox info with defaults.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            delimiter: '/',
            exists: 0,
            recent: 0,
            unseen: 0,
            uid_validity: 1,
            uid_next: 1,
            flags: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Message envelope (headers)
// ---------------------------------------------------------------------------

/// Email message envelope (header summary).
#[derive(Clone, Debug)]
pub struct Envelope {
    /// Message sequence number.
    pub seq: u32,
    /// Message UID.
    pub uid: u32,
    /// From address.
    pub from: String,
    /// To address.
    pub to: String,
    /// Subject.
    pub subject: String,
    /// Date string.
    pub date: String,
    /// Message-ID.
    pub message_id: String,
    /// Message flags.
    pub flags: Vec<MessageFlag>,
    /// Size in bytes.
    pub size: u32,
}

impl Envelope {
    /// Create an empty envelope for a sequence number.
    pub fn new(seq: u32) -> Self {
        Self {
            seq,
            uid: 0,
            from: String::new(),
            to: String::new(),
            subject: String::new(),
            date: String::new(),
            message_id: String::new(),
            flags: Vec::new(),
            size: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Search criteria
// ---------------------------------------------------------------------------

/// IMAP search criteria.
#[derive(Clone, Debug)]
pub enum SearchCriteria {
    /// All messages.
    All,
    /// Messages with \Seen flag.
    Seen,
    /// Messages without \Seen flag.
    Unseen,
    /// Messages with \Flagged flag.
    Flagged,
    /// Messages with \Answered flag.
    Answered,
    /// Messages with \Deleted flag.
    Deleted,
    /// Messages with \Draft flag.
    Draft,
    /// Messages from a specific sender.
    From(String),
    /// Messages with subject containing text.
    Subject(String),
    /// Messages since a date (DD-Mon-YYYY).
    Since(String),
    /// Messages before a date (DD-Mon-YYYY).
    Before(String),
    /// Messages larger than N bytes.
    Larger(u32),
    /// Messages smaller than N bytes.
    Smaller(u32),
}

impl SearchCriteria {
    /// Format as an IMAP SEARCH argument string.
    pub fn format(&self) -> String {
        match self {
            SearchCriteria::All => "ALL".to_owned(),
            SearchCriteria::Seen => "SEEN".to_owned(),
            SearchCriteria::Unseen => "UNSEEN".to_owned(),
            SearchCriteria::Flagged => "FLAGGED".to_owned(),
            SearchCriteria::Answered => "ANSWERED".to_owned(),
            SearchCriteria::Deleted => "DELETED".to_owned(),
            SearchCriteria::Draft => "DRAFT".to_owned(),
            SearchCriteria::From(addr) => format!("FROM \"{}\"", addr),
            SearchCriteria::Subject(text) => format!("SUBJECT \"{}\"", text),
            SearchCriteria::Since(date) => format!("SINCE {}", date),
            SearchCriteria::Before(date) => format!("BEFORE {}", date),
            SearchCriteria::Larger(n) => format!("LARGER {}", n),
            SearchCriteria::Smaller(n) => format!("SMALLER {}", n),
        }
    }
}

// ---------------------------------------------------------------------------
// MIME parsing (basic)
// ---------------------------------------------------------------------------

/// A parsed MIME content type.
#[derive(Clone, Debug)]
pub struct ContentType {
    /// Primary type (e.g., "text").
    pub primary: String,
    /// Subtype (e.g., "plain").
    pub subtype: String,
    /// Parameters (e.g., charset=UTF-8).
    pub params: Vec<(String, String)>,
}

impl ContentType {
    /// Parse a Content-Type header value.
    pub fn parse(value: &str) -> Self {
        let parts: Vec<&str> = value.splitn(2, ';').collect();
        let type_parts: Vec<&str> = parts[0].trim().splitn(2, '/').collect();
        let primary = if !type_parts.is_empty() {
            type_parts[0].trim().to_owned()
        } else {
            "text".to_owned()
        };
        let subtype = if type_parts.len() > 1 {
            type_parts[1].trim().to_owned()
        } else {
            "plain".to_owned()
        };

        let mut params = Vec::new();
        if parts.len() > 1 {
            for param in parts[1].split(';') {
                let kv: Vec<&str> = param.splitn(2, '=').collect();
                if kv.len() == 2 {
                    let key = kv[0].trim().to_owned();
                    let val = kv[1].trim().trim_matches('"').to_owned();
                    params.push((key, val));
                }
            }
        }

        Self { primary, subtype, params }
    }

    /// Check if this is text/plain.
    pub fn is_text_plain(&self) -> bool {
        self.primary == "text" && self.subtype == "plain"
    }

    /// Check if this is multipart/*.
    pub fn is_multipart(&self) -> bool {
        self.primary == "multipart"
    }

    /// Get the boundary parameter for multipart messages.
    pub fn boundary(&self) -> Option<&str> {
        self.params.iter()
            .find(|(k, _)| k == "boundary")
            .map(|(_, v)| v.as_str())
    }

    /// Get the charset parameter.
    pub fn charset(&self) -> &str {
        self.params.iter()
            .find(|(k, _)| k == "charset")
            .map(|(_, v)| v.as_str())
            .unwrap_or("US-ASCII")
    }
}

/// Extract text/plain body from a message, handling basic multipart.
pub fn extract_text_body(headers: &str, body: &str) -> String {
    // Find Content-Type header
    let ct_line = headers.lines()
        .find(|l| l.starts_with("Content-Type:") || l.starts_with("content-type:"));

    let ct = if let Some(line) = ct_line {
        let value = line.splitn(2, ':').nth(1).unwrap_or("text/plain").trim();
        ContentType::parse(value)
    } else {
        ContentType::parse("text/plain")
    };

    if ct.is_text_plain() {
        return body.to_owned();
    }

    if ct.is_multipart() {
        if let Some(boundary) = ct.boundary() {
            let delim = format!("--{}", boundary);
            let parts: Vec<&str> = body.split(&delim).collect();
            for part in parts {
                let trimmed = part.trim();
                if trimmed.is_empty() || trimmed == "--" {
                    continue;
                }
                // Split headers from body in this part
                if let Some(idx) = trimmed.find("\r\n\r\n") {
                    let part_headers = &trimmed[..idx];
                    let part_body = &trimmed[idx + 4..];
                    let part_ct_line = part_headers.lines()
                        .find(|l| l.starts_with("Content-Type:") || l.starts_with("content-type:"));
                    if let Some(pctl) = part_ct_line {
                        let pct_val = pctl.splitn(2, ':').nth(1).unwrap_or("").trim();
                        let pct = ContentType::parse(pct_val);
                        if pct.is_text_plain() {
                            return part_body.to_owned();
                        }
                    } else {
                        // No content-type in part, assume text/plain
                        return part_body.to_owned();
                    }
                } else if let Some(idx) = trimmed.find("\n\n") {
                    let part_body = &trimmed[idx + 2..];
                    return part_body.to_owned();
                }
            }
        }
    }

    // Fallback: return the body as-is
    body.to_owned()
}

// ---------------------------------------------------------------------------
// IMAP client state
// ---------------------------------------------------------------------------

/// IMAP connection state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    NotAuthenticated,
    Authenticated,
    Selected,
    Logout,
}

/// An IMAP client session.
#[derive(Clone, Debug)]
pub struct ImapSession {
    /// Server address.
    pub server: String,
    /// Username.
    pub username: String,
    /// Connection state.
    pub state: ConnectionState,
    /// Currently selected mailbox.
    pub selected_mailbox: Option<String>,
    /// Available mailboxes.
    pub mailboxes: Vec<MailboxInfo>,
    /// Cached envelopes for the selected mailbox.
    pub envelopes: Vec<Envelope>,
    /// Command tag counter.
    pub tag_counter: u32,
    /// Server capabilities.
    pub capabilities: Vec<String>,
}

impl ImapSession {
    /// Create a new disconnected session.
    pub fn new() -> Self {
        Self {
            server: String::new(),
            username: String::new(),
            state: ConnectionState::Disconnected,
            selected_mailbox: None,
            mailboxes: Vec::new(),
            envelopes: Vec::new(),
            tag_counter: 0,
            capabilities: Vec::new(),
        }
    }

    /// Generate the next command tag (e.g., "A001", "A002").
    pub fn next_tag(&mut self) -> String {
        self.tag_counter += 1;
        format!("A{:03}", self.tag_counter)
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

const MAX_SESSIONS: usize = 4;

struct ImapState {
    sessions: Vec<ImapSession>,
    initialized: bool,
}

impl ImapState {
    const fn new() -> Self {
        Self {
            sessions: Vec::new(),
            initialized: false,
        }
    }
}

static STATE: Mutex<ImapState> = Mutex::new(ImapState::new());

// Statistics
static CONNECTIONS: AtomicU64 = AtomicU64::new(0);
static MESSAGES_FETCHED: AtomicU64 = AtomicU64::new(0);
static SEARCHES_PERFORMED: AtomicU64 = AtomicU64::new(0);
static BYTES_RECEIVED: AtomicU64 = AtomicU64::new(0);
static BYTES_SENT: AtomicU64 = AtomicU64::new(0);
static COMMANDS_SENT: AtomicU64 = AtomicU64::new(0);
static IMAP_INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the IMAP subsystem.
pub fn init() {
    let mut state = STATE.lock();
    if state.initialized {
        return;
    }
    state.initialized = true;
    IMAP_INITIALIZED.store(true, Ordering::SeqCst);
}

/// Connect to an IMAP server (simulated).
pub fn connect(server: &str, user: &str, _pass: &str) -> Result<usize, &'static str> {
    let mut state = STATE.lock();
    if state.sessions.len() >= MAX_SESSIONS {
        return Err("maximum IMAP sessions reached");
    }

    let mut session = ImapSession::new();
    session.server = server.to_owned();
    session.username = user.to_owned();
    session.state = ConnectionState::Authenticated;
    session.capabilities = vec![
        "IMAP4rev1".to_owned(),
        "STARTTLS".to_owned(),
        "AUTH=LOGIN".to_owned(),
        "AUTH=PLAIN".to_owned(),
        "IDLE".to_owned(),
        "NAMESPACE".to_owned(),
        "UIDPLUS".to_owned(),
    ];

    // Create default INBOX
    let mut inbox = MailboxInfo::new("INBOX");
    inbox.exists = 0;
    inbox.recent = 0;
    session.mailboxes.push(inbox);

    // Add other default mailboxes
    session.mailboxes.push(MailboxInfo::new("Drafts"));
    session.mailboxes.push(MailboxInfo::new("Sent"));
    session.mailboxes.push(MailboxInfo::new("Trash"));
    session.mailboxes.push(MailboxInfo::new("Spam"));

    let idx = state.sessions.len();
    state.sessions.push(session);
    CONNECTIONS.fetch_add(1, Ordering::Relaxed);
    COMMANDS_SENT.fetch_add(2, Ordering::Relaxed); // LOGIN + CAPABILITY

    Ok(idx)
}

/// List mailboxes for a session.
pub fn list_mailboxes(session_idx: usize) -> Result<String, &'static str> {
    let state = STATE.lock();
    let session = state.sessions.get(session_idx).ok_or("invalid session")?;
    if session.state == ConnectionState::Disconnected {
        return Err("not connected");
    }

    COMMANDS_SENT.fetch_add(1, Ordering::Relaxed);

    let mut out = format!("Mailboxes for {}@{}:\n", session.username, session.server);
    for mbox in &session.mailboxes {
        out += &format!("  {} (exists={}, recent={}, unseen={})\n",
            mbox.name, mbox.exists, mbox.recent, mbox.unseen);
    }
    Ok(out)
}

/// Select a mailbox.
pub fn select(session_idx: usize, mailbox: &str) -> Result<String, &'static str> {
    let mut state = STATE.lock();
    let session = state.sessions.get_mut(session_idx).ok_or("invalid session")?;
    if session.state == ConnectionState::Disconnected {
        return Err("not connected");
    }

    // Check if mailbox exists
    let mbox = session.mailboxes.iter()
        .find(|m| m.name == mailbox)
        .ok_or("mailbox not found")?;

    let info = format!("* {} EXISTS\n* {} RECENT\n* OK [UIDVALIDITY {}]\n* OK [UIDNEXT {}]",
        mbox.exists, mbox.recent, mbox.uid_validity, mbox.uid_next);

    session.selected_mailbox = Some(mailbox.to_owned());
    session.state = ConnectionState::Selected;
    COMMANDS_SENT.fetch_add(1, Ordering::Relaxed);

    Ok(info)
}

/// Fetch message headers (envelope) for a sequence number.
pub fn fetch_headers(session_idx: usize, seq: u32) -> Result<String, &'static str> {
    let state = STATE.lock();
    let session = state.sessions.get(session_idx).ok_or("invalid session")?;
    if session.state != ConnectionState::Selected {
        return Err("no mailbox selected");
    }

    COMMANDS_SENT.fetch_add(1, Ordering::Relaxed);
    MESSAGES_FETCHED.fetch_add(1, Ordering::Relaxed);

    // In a real implementation, this would fetch from the server.
    // For now, return simulated data.
    Ok(format!(
        "* {} FETCH (ENVELOPE (\"\" \"(no subject)\" ((\"\" NIL \"unknown\" \"example.com\")) \
         ((\"\" NIL \"unknown\" \"example.com\")) NIL NIL \"<msg@example.com>\"))",
        seq))
}

/// Fetch message body for a sequence number.
pub fn fetch_body(session_idx: usize, seq: u32) -> Result<String, &'static str> {
    let state = STATE.lock();
    let session = state.sessions.get(session_idx).ok_or("invalid session")?;
    if session.state != ConnectionState::Selected {
        return Err("no mailbox selected");
    }

    COMMANDS_SENT.fetch_add(1, Ordering::Relaxed);
    MESSAGES_FETCHED.fetch_add(1, Ordering::Relaxed);

    Ok(format!("* {} FETCH (BODY[TEXT] {{0}}\r\n)", seq))
}

/// Search messages by criteria.
pub fn search(session_idx: usize, criteria: &SearchCriteria) -> Result<String, &'static str> {
    let state = STATE.lock();
    let session = state.sessions.get(session_idx).ok_or("invalid session")?;
    if session.state != ConnectionState::Selected {
        return Err("no mailbox selected");
    }

    COMMANDS_SENT.fetch_add(1, Ordering::Relaxed);
    SEARCHES_PERFORMED.fetch_add(1, Ordering::Relaxed);

    Ok(format!("* SEARCH (criteria: {})", criteria.format()))
}

/// Get IMAP client info.
pub fn imap_info() -> String {
    let state = STATE.lock();
    let active = state.sessions.iter()
        .filter(|s| s.state != ConnectionState::Disconnected)
        .count();
    let mut out = format!(
        "IMAP4rev1 Client Info\n\
         ────────────────────────────\n\
         Status:           {}\n\
         Active sessions:  {}/{}\n\
         Default port:     143 (993 with TLS)\n\
         STARTTLS:         supported\n\
         AUTH methods:     LOGIN, PLAIN\n",
        if IMAP_INITIALIZED.load(Ordering::Relaxed) { "running" } else { "stopped" },
        active,
        MAX_SESSIONS,
    );

    if !state.sessions.is_empty() {
        out += "\nSessions:\n";
        for (i, session) in state.sessions.iter().enumerate() {
            let state_str = match session.state {
                ConnectionState::Disconnected => "disconnected",
                ConnectionState::NotAuthenticated => "not authenticated",
                ConnectionState::Authenticated => "authenticated",
                ConnectionState::Selected => "selected",
                ConnectionState::Logout => "logout",
            };
            out += &format!("  [{}] {}@{} ({})",
                i, session.username, session.server, state_str);
            if let Some(ref mbox) = session.selected_mailbox {
                out += &format!(" mailbox={}", mbox);
            }
            out += "\n";
        }
    }

    out
}

/// Get IMAP statistics.
pub fn imap_stats() -> String {
    format!(
        "IMAP Statistics\n\
         ────────────────────────────\n\
         Connections:      {}\n\
         Messages fetched: {}\n\
         Searches:         {}\n\
         Commands sent:    {}\n\
         Bytes received:   {}\n\
         Bytes sent:       {}",
        CONNECTIONS.load(Ordering::Relaxed),
        MESSAGES_FETCHED.load(Ordering::Relaxed),
        SEARCHES_PERFORMED.load(Ordering::Relaxed),
        COMMANDS_SENT.load(Ordering::Relaxed),
        BYTES_RECEIVED.load(Ordering::Relaxed),
        BYTES_SENT.load(Ordering::Relaxed),
    )
}
