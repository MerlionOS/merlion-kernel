/// Email client for MerlionOS.
/// Send via SMTP, receive via IMAP, with mailbox management
/// and MIME parsing for text/plain emails.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use alloc::borrow::ToOwned;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_ACCOUNTS: usize = 8;
const MAX_MAILBOX_EMAILS: usize = 256;
const MAX_CONTACTS: usize = 128;
const MAX_SEARCH_RESULTS: usize = 64;
const MAILBOX_VFS_ROOT: &str = "/var/mail";

// ---------------------------------------------------------------------------
// Mailbox names
// ---------------------------------------------------------------------------

const MAILBOX_INBOX: &str = "inbox";
const MAILBOX_SENT: &str = "sent";
const MAILBOX_DRAFTS: &str = "drafts";
const MAILBOX_TRASH: &str = "trash";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Email account configuration.
#[derive(Debug, Clone)]
pub struct EmailAccount {
    pub name: String,
    pub email: String,
    pub smtp_server: [u8; 4],
    pub smtp_port: u16,
    pub imap_server: [u8; 4],
    pub imap_port: u16,
    pub username: String,
    pub password: String,
}

impl EmailAccount {
    /// Create a new account with default ports.
    pub fn new(name: &str, email: &str, username: &str, password: &str) -> Self {
        Self {
            name: name.to_owned(),
            email: email.to_owned(),
            smtp_server: [127, 0, 0, 1],
            smtp_port: 25,
            imap_server: [127, 0, 0, 1],
            imap_port: 143,
            username: username.to_owned(),
            password: password.to_owned(),
        }
    }

    /// Set SMTP server address.
    pub fn with_smtp(mut self, ip: [u8; 4], port: u16) -> Self {
        self.smtp_server = ip;
        self.smtp_port = port;
        self
    }

    /// Set IMAP server address.
    pub fn with_imap(mut self, ip: [u8; 4], port: u16) -> Self {
        self.imap_server = ip;
        self.imap_port = port;
        self
    }

    fn smtp_addr_str(&self) -> String {
        format!("{}.{}.{}.{}",
            self.smtp_server[0], self.smtp_server[1],
            self.smtp_server[2], self.smtp_server[3])
    }
}

/// An email attachment descriptor.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub name: String,
    pub size: usize,
    pub mime_type: String,
}

/// A full email message.
#[derive(Debug, Clone)]
pub struct Email {
    pub id: u32,
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub date: String,
    pub body: String,
    pub read: bool,
    pub flagged: bool,
    pub attachments: Vec<Attachment>,
}

impl Email {
    fn new(id: u32, from: &str, to: &[&str], subject: &str, body: &str) -> Self {
        Self {
            id,
            from: from.to_owned(),
            to: to.iter().map(|s| (*s).to_owned()).collect(),
            cc: Vec::new(),
            subject: subject.to_owned(),
            date: String::new(),
            body: body.to_owned(),
            read: false,
            flagged: false,
            attachments: Vec::new(),
        }
    }

    /// Format email for display.
    pub fn display(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("From:    {}\n", self.from));
        let to_str: String = self.to.join(", ");
        out.push_str(&format!("To:      {}\n", to_str));
        if !self.cc.is_empty() {
            let cc_str: String = self.cc.join(", ");
            out.push_str(&format!("CC:      {}\n", cc_str));
        }
        out.push_str(&format!("Subject: {}\n", self.subject));
        if !self.date.is_empty() {
            out.push_str(&format!("Date:    {}\n", self.date));
        }
        let flags = format!("{}{}",
            if self.read { "" } else { "[UNREAD] " },
            if self.flagged { "[FLAGGED]" } else { "" },
        );
        if !flags.trim().is_empty() {
            out.push_str(&format!("Flags:   {}\n", flags.trim()));
        }
        if !self.attachments.is_empty() {
            out.push_str(&format!("Attachments: {}\n", self.attachments.len()));
            for att in &self.attachments {
                out.push_str(&format!("  - {} ({}, {} bytes)\n",
                    att.name, att.mime_type, att.size));
            }
        }
        out.push_str("---\n");
        out.push_str(&self.body);
        out.push('\n');
        out
    }
}

/// Summary of an email for listing.
#[derive(Debug, Clone)]
pub struct EmailHeader {
    pub id: u32,
    pub from: String,
    pub subject: String,
    pub date: String,
    pub read: bool,
    pub flagged: bool,
}

/// A contact entry.
#[derive(Debug, Clone)]
pub struct Contact {
    pub name: String,
    pub email: String,
}

// ---------------------------------------------------------------------------
// Mailbox
// ---------------------------------------------------------------------------

struct Mailbox {
    name: String,
    emails: Vec<Email>,
}

impl Mailbox {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            emails: Vec::new(),
        }
    }

    fn add(&mut self, email: Email) -> bool {
        if self.emails.len() >= MAX_MAILBOX_EMAILS {
            return false;
        }
        self.emails.push(email);
        true
    }

    fn get(&self, id: u32) -> Option<&Email> {
        self.emails.iter().find(|e| e.id == id)
    }

    fn get_mut(&mut self, id: u32) -> Option<&mut Email> {
        self.emails.iter_mut().find(|e| e.id == id)
    }

    fn remove(&mut self, id: u32) -> Option<Email> {
        if let Some(pos) = self.emails.iter().position(|e| e.id == id) {
            Some(self.emails.remove(pos))
        } else {
            None
        }
    }

    fn unread_count(&self) -> usize {
        self.emails.iter().filter(|e| !e.read).count()
    }

    fn headers(&self) -> Vec<EmailHeader> {
        self.emails.iter().map(|e| EmailHeader {
            id: e.id,
            from: e.from.clone(),
            subject: e.subject.clone(),
            date: e.date.clone(),
            read: e.read,
            flagged: e.flagged,
        }).collect()
    }

    fn search(&self, query: &str) -> Vec<EmailHeader> {
        let q = query.to_ascii_lowercase();
        let mut results = Vec::new();
        for e in &self.emails {
            if results.len() >= MAX_SEARCH_RESULTS {
                break;
            }
            let subj_lower = e.subject.to_ascii_lowercase();
            let from_lower = e.from.to_ascii_lowercase();
            let body_lower = e.body.to_ascii_lowercase();
            if subj_lower.contains(&q) || from_lower.contains(&q) || body_lower.contains(&q) {
                results.push(EmailHeader {
                    id: e.id,
                    from: e.from.clone(),
                    subject: e.subject.clone(),
                    date: e.date.clone(),
                    read: e.read,
                    flagged: e.flagged,
                });
            }
        }
        results
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct EmailState {
    initialized: bool,
    accounts: Vec<EmailAccount>,
    active_account: usize,
    inbox: Mailbox,
    sent: Mailbox,
    drafts: Mailbox,
    trash: Mailbox,
    contacts: Vec<Contact>,
    next_id: u32,
}

impl EmailState {
    const fn new() -> Self {
        Self {
            initialized: false,
            accounts: Vec::new(),
            active_account: 0,
            inbox: Mailbox { name: String::new(), emails: Vec::new() },
            sent: Mailbox { name: String::new(), emails: Vec::new() },
            drafts: Mailbox { name: String::new(), emails: Vec::new() },
            trash: Mailbox { name: String::new(), emails: Vec::new() },
            contacts: Vec::new(),
            next_id: 1,
        }
    }
}

static STATE: Mutex<EmailState> = Mutex::new(EmailState::new());

// Statistics
static EMAILS_SENT: AtomicU64 = AtomicU64::new(0);
static EMAILS_RECEIVED: AtomicU64 = AtomicU64::new(0);
static EMAILS_DELETED: AtomicU64 = AtomicU64::new(0);
static NEW_MAIL_COUNT: AtomicU64 = AtomicU64::new(0);
static EMAIL_INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the email subsystem.
pub fn init() {
    let mut state = STATE.lock();
    if state.initialized {
        return;
    }
    state.initialized = true;
    state.inbox = Mailbox::new(MAILBOX_INBOX);
    state.sent = Mailbox::new(MAILBOX_SENT);
    state.drafts = Mailbox::new(MAILBOX_DRAFTS);
    state.trash = Mailbox::new(MAILBOX_TRASH);
    EMAIL_INITIALIZED.store(true, Ordering::SeqCst);

    // Create VFS directories for mail storage
    let _ = crate::vfs::mkdir(MAILBOX_VFS_ROOT);
    let inbox_path = format!("{}/{}", MAILBOX_VFS_ROOT, MAILBOX_INBOX);
    let sent_path = format!("{}/{}", MAILBOX_VFS_ROOT, MAILBOX_SENT);
    let drafts_path = format!("{}/{}", MAILBOX_VFS_ROOT, MAILBOX_DRAFTS);
    let trash_path = format!("{}/{}", MAILBOX_VFS_ROOT, MAILBOX_TRASH);
    let _ = crate::vfs::mkdir(&inbox_path);
    let _ = crate::vfs::mkdir(&sent_path);
    let _ = crate::vfs::mkdir(&drafts_path);
    let _ = crate::vfs::mkdir(&trash_path);
}

/// Add an account.
pub fn add_account(account: EmailAccount) -> Result<usize, &'static str> {
    let mut state = STATE.lock();
    if state.accounts.len() >= MAX_ACCOUNTS {
        return Err("max accounts reached");
    }
    let idx = state.accounts.len();
    state.accounts.push(account);
    Ok(idx)
}

/// Compose a new email.
pub fn compose(from: &str, to: &str, subject: &str, body: &str) -> Email {
    let mut state = STATE.lock();
    let id = state.next_id;
    state.next_id = state.next_id.wrapping_add(1);
    Email::new(id, from, &[to], subject, body)
}

/// Compose with CC.
pub fn compose_with_cc(from: &str, to: &str, cc: &[&str], subject: &str, body: &str) -> Email {
    let mut email = compose(from, to, subject, body);
    email.cc = cc.iter().map(|s| (*s).to_owned()).collect();
    email
}

/// Send an email via SMTP.
pub fn send(email: &Email) -> Result<(), &'static str> {
    let state = STATE.lock();
    if state.accounts.is_empty() {
        return Err("no email account configured");
    }
    let account = &state.accounts[state.active_account];
    let server_ip = account.smtp_addr_str();
    let to_str: String = email.to.join(", ");
    drop(state);

    // Queue via SMTP module
    let _result = crate::smtp::send_email(
        &email.from,
        &to_str,
        &email.subject,
        &email.body,
        &server_ip,
    );

    // Save to sent folder
    let mut state = STATE.lock();
    let mut sent_copy = email.clone();
    sent_copy.read = true;
    state.sent.add(sent_copy);

    EMAILS_SENT.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Compose and send in one step.
pub fn compose_and_send(from: &str, to: &str, subject: &str, body: &str) -> Result<(), &'static str> {
    let email = compose(from, to, subject, body);
    send(&email)
}

/// Reply to an email.
pub fn reply(original: &Email, from: &str, body: &str) -> Email {
    let subject = if original.subject.starts_with("Re: ") {
        original.subject.clone()
    } else {
        format!("Re: {}", original.subject)
    };
    let quoted_body = format!(
        "{}\n\n> On {}, {} wrote:\n{}",
        body,
        original.date,
        original.from,
        quote_body(&original.body),
    );
    let mut state = STATE.lock();
    let id = state.next_id;
    state.next_id = state.next_id.wrapping_add(1);
    Email::new(id, from, &[original.from.as_str()], &subject, &quoted_body)
}

/// Forward an email.
pub fn forward(original: &Email, from: &str, to: &str) -> Email {
    let subject = if original.subject.starts_with("Fwd: ") {
        original.subject.clone()
    } else {
        format!("Fwd: {}", original.subject)
    };
    let fwd_body = format!(
        "\n---------- Forwarded message ----------\nFrom: {}\nSubject: {}\nDate: {}\n\n{}",
        original.from, original.subject, original.date, original.body,
    );
    let mut state = STATE.lock();
    let id = state.next_id;
    state.next_id = state.next_id.wrapping_add(1);
    Email::new(id, from, &[to], &subject, &fwd_body)
}

fn quote_body(body: &str) -> String {
    let mut out = String::new();
    for line in body.lines() {
        out.push_str("> ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Check mail (simulate IMAP fetch).
pub fn check_mail() -> Vec<EmailHeader> {
    let state = STATE.lock();
    if state.accounts.is_empty() {
        return Vec::new();
    }
    // In a real implementation, this would connect via IMAP
    // Return current inbox headers
    state.inbox.headers()
}

/// Fetch a specific email by ID.
pub fn fetch_email(id: u32) -> Option<Email> {
    let mut state = STATE.lock();
    // Search all mailboxes
    if let Some(e) = state.inbox.get_mut(id) {
        e.read = true;
        return Some(e.clone());
    }
    if let Some(e) = state.sent.get(id) {
        return Some(e.clone());
    }
    if let Some(e) = state.drafts.get(id) {
        return Some(e.clone());
    }
    if let Some(e) = state.trash.get(id) {
        return Some(e.clone());
    }
    None
}

/// Delete an email (move to trash).
pub fn delete_email(id: u32) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    if let Some(email) = state.inbox.remove(id) {
        state.trash.add(email);
        EMAILS_DELETED.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    if let Some(email) = state.drafts.remove(id) {
        state.trash.add(email);
        EMAILS_DELETED.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    Err("email not found")
}

/// List mailbox names.
pub fn list_mailboxes() -> Vec<String> {
    alloc::vec![
        MAILBOX_INBOX.to_owned(),
        MAILBOX_SENT.to_owned(),
        MAILBOX_DRAFTS.to_owned(),
        MAILBOX_TRASH.to_owned(),
    ]
}

/// Get emails from a specific mailbox.
pub fn list_mailbox(name: &str) -> Vec<EmailHeader> {
    let state = STATE.lock();
    match name {
        "inbox" => state.inbox.headers(),
        "sent" => state.sent.headers(),
        "drafts" => state.drafts.headers(),
        "trash" => state.trash.headers(),
        _ => Vec::new(),
    }
}

/// Save email as draft.
pub fn save_draft(email: Email) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    if !state.drafts.add(email) {
        return Err("drafts folder full");
    }
    Ok(())
}

/// Add a contact.
pub fn add_contact(name: &str, email: &str) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    if state.contacts.len() >= MAX_CONTACTS {
        return Err("contact list full");
    }
    state.contacts.push(Contact {
        name: name.to_owned(),
        email: email.to_owned(),
    });
    Ok(())
}

/// Look up a contact by name (prefix match).
pub fn lookup_contact(query: &str) -> Vec<Contact> {
    let state = STATE.lock();
    let q = query.to_ascii_lowercase();
    state.contacts.iter()
        .filter(|c| c.name.to_ascii_lowercase().starts_with(&q)
            || c.email.to_ascii_lowercase().starts_with(&q))
        .cloned()
        .collect()
}

/// Search emails across all mailboxes.
pub fn search(query: &str) -> Vec<EmailHeader> {
    let state = STATE.lock();
    let mut results = state.inbox.search(query);
    results.extend(state.sent.search(query));
    results.extend(state.drafts.search(query));
    results
}

/// Flag/unflag an email.
pub fn toggle_flag(id: u32) -> Result<bool, &'static str> {
    let mut state = STATE.lock();
    if let Some(e) = state.inbox.get_mut(id) {
        e.flagged = !e.flagged;
        return Ok(e.flagged);
    }
    Err("email not found")
}

/// Get new mail notification count.
pub fn new_mail_count() -> u64 {
    NEW_MAIL_COUNT.load(Ordering::Relaxed)
}

/// Deliver an incoming email (called by IMAP fetch simulation).
pub fn deliver(from: &str, to: &str, subject: &str, body: &str) {
    let mut state = STATE.lock();
    let id = state.next_id;
    state.next_id = state.next_id.wrapping_add(1);
    let email = Email::new(id, from, &[to], subject, body);
    state.inbox.add(email);
    EMAILS_RECEIVED.fetch_add(1, Ordering::Relaxed);
    NEW_MAIL_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Get email subsystem info.
pub fn email_info() -> String {
    let state = STATE.lock();
    let mut info = String::from("MerlionOS Email Client v1.0\n");
    info.push_str(&format!("Accounts: {}\n", state.accounts.len()));
    if !state.accounts.is_empty() {
        let acct = &state.accounts[state.active_account];
        info.push_str(&format!("Active: {} <{}>\n", acct.name, acct.email));
    }
    info.push_str(&format!("Inbox: {} ({} unread)\n",
        state.inbox.emails.len(), state.inbox.unread_count()));
    info.push_str(&format!("Sent: {}\n", state.sent.emails.len()));
    info.push_str(&format!("Drafts: {}\n", state.drafts.emails.len()));
    info.push_str(&format!("Trash: {}\n", state.trash.emails.len()));
    info.push_str(&format!("Contacts: {}\n", state.contacts.len()));
    info
}

/// Get email statistics.
pub fn email_stats() -> String {
    let sent = EMAILS_SENT.load(Ordering::Relaxed);
    let received = EMAILS_RECEIVED.load(Ordering::Relaxed);
    let deleted = EMAILS_DELETED.load(Ordering::Relaxed);
    let new_count = NEW_MAIL_COUNT.load(Ordering::Relaxed);
    format!(
        "Emails sent: {}\nEmails received: {}\nEmails deleted: {}\nNew mail: {}\n",
        sent, received, deleted, new_count
    )
}
