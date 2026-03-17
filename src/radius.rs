/// RADIUS authentication client for MerlionOS (RFC 2865).
/// Provides network access control via RADIUS server communication.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default RADIUS authentication port.
const RADIUS_AUTH_PORT: u16 = 1812;

/// Default RADIUS accounting port.
const RADIUS_ACCT_PORT: u16 = 1813;

/// Maximum RADIUS packet size.
const MAX_PACKET_SIZE: usize = 4096;

/// Maximum attributes per packet.
const MAX_ATTRIBUTES: usize = 64;

/// Maximum sessions tracked.
const MAX_SESSIONS: usize = 256;

/// Authenticator length.
const AUTHENTICATOR_LEN: usize = 16;

// ---------------------------------------------------------------------------
// RADIUS codes
// ---------------------------------------------------------------------------

/// RADIUS packet codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RadiusCode {
    AccessRequest = 1,
    AccessAccept = 2,
    AccessReject = 3,
    AccountingRequest = 4,
    AccountingResponse = 5,
    AccessChallenge = 11,
}

impl RadiusCode {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(RadiusCode::AccessRequest),
            2 => Some(RadiusCode::AccessAccept),
            3 => Some(RadiusCode::AccessReject),
            4 => Some(RadiusCode::AccountingRequest),
            5 => Some(RadiusCode::AccountingResponse),
            11 => Some(RadiusCode::AccessChallenge),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            RadiusCode::AccessRequest => "Access-Request",
            RadiusCode::AccessAccept => "Access-Accept",
            RadiusCode::AccessReject => "Access-Reject",
            RadiusCode::AccountingRequest => "Accounting-Request",
            RadiusCode::AccountingResponse => "Accounting-Response",
            RadiusCode::AccessChallenge => "Access-Challenge",
        }
    }
}

// ---------------------------------------------------------------------------
// RADIUS attribute types
// ---------------------------------------------------------------------------

/// Well-known RADIUS attribute types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AttrType {
    UserName = 1,
    UserPassword = 2,
    NasIpAddress = 4,
    NasPort = 5,
    ServiceType = 6,
    FramedIpAddress = 8,
    ReplyMessage = 18,
    State = 24,
    SessionTimeout = 27,
    CalledStationId = 30,
    CallingStationId = 31,
}

impl AttrType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(AttrType::UserName),
            2 => Some(AttrType::UserPassword),
            4 => Some(AttrType::NasIpAddress),
            5 => Some(AttrType::NasPort),
            6 => Some(AttrType::ServiceType),
            8 => Some(AttrType::FramedIpAddress),
            18 => Some(AttrType::ReplyMessage),
            24 => Some(AttrType::State),
            27 => Some(AttrType::SessionTimeout),
            30 => Some(AttrType::CalledStationId),
            31 => Some(AttrType::CallingStationId),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            AttrType::UserName => "User-Name",
            AttrType::UserPassword => "User-Password",
            AttrType::NasIpAddress => "NAS-IP-Address",
            AttrType::NasPort => "NAS-Port",
            AttrType::ServiceType => "Service-Type",
            AttrType::FramedIpAddress => "Framed-IP-Address",
            AttrType::ReplyMessage => "Reply-Message",
            AttrType::State => "State",
            AttrType::SessionTimeout => "Session-Timeout",
            AttrType::CalledStationId => "Called-Station-Id",
            AttrType::CallingStationId => "Calling-Station-Id",
        }
    }
}

// ---------------------------------------------------------------------------
// RADIUS attribute
// ---------------------------------------------------------------------------

/// A RADIUS attribute (TLV).
#[derive(Debug, Clone)]
pub struct RadiusAttribute {
    pub attr_type: u8,
    pub value: Vec<u8>,
}

impl RadiusAttribute {
    pub fn new_string(attr_type: u8, value: &str) -> Self {
        Self {
            attr_type,
            value: value.as_bytes().to_vec(),
        }
    }

    pub fn new_u32(attr_type: u8, value: u32) -> Self {
        Self {
            attr_type,
            value: value.to_be_bytes().to_vec(),
        }
    }

    pub fn new_ip(attr_type: u8, ip: [u8; 4]) -> Self {
        Self {
            attr_type,
            value: ip.to_vec(),
        }
    }

    /// Encode to wire format: type (1) + length (1) + value.
    pub fn encode(&self) -> Vec<u8> {
        let len = (2 + self.value.len()) as u8;
        let mut buf = Vec::with_capacity(len as usize);
        buf.push(self.attr_type);
        buf.push(len);
        buf.extend_from_slice(&self.value);
        buf
    }

    /// Decode from wire bytes. Returns (attribute, bytes_consumed).
    pub fn decode(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < 2 {
            return None;
        }
        let attr_type = data[0];
        let length = data[1] as usize;
        if length < 2 || data.len() < length {
            return None;
        }
        let value = data[2..length].to_vec();
        Some((Self { attr_type, value }, length))
    }
}

// ---------------------------------------------------------------------------
// RADIUS packet
// ---------------------------------------------------------------------------

/// A RADIUS packet.
#[derive(Debug, Clone)]
pub struct RadiusPacket {
    pub code: u8,
    pub identifier: u8,
    pub authenticator: [u8; AUTHENTICATOR_LEN],
    pub attributes: Vec<RadiusAttribute>,
}

impl RadiusPacket {
    /// Create a new Access-Request packet.
    pub fn access_request(identifier: u8, authenticator: [u8; AUTHENTICATOR_LEN]) -> Self {
        Self {
            code: RadiusCode::AccessRequest as u8,
            identifier,
            authenticator,
            attributes: Vec::new(),
        }
    }

    /// Add an attribute.
    pub fn add_attribute(&mut self, attr: RadiusAttribute) {
        if self.attributes.len() < MAX_ATTRIBUTES {
            self.attributes.push(attr);
        }
    }

    /// Encode to wire format.
    pub fn encode(&self) -> Vec<u8> {
        let mut attrs_buf = Vec::new();
        for attr in &self.attributes {
            attrs_buf.extend_from_slice(&attr.encode());
        }
        let length = (20 + attrs_buf.len()) as u16;
        let mut buf = Vec::with_capacity(length as usize);
        buf.push(self.code);
        buf.push(self.identifier);
        buf.extend_from_slice(&length.to_be_bytes());
        buf.extend_from_slice(&self.authenticator);
        buf.extend_from_slice(&attrs_buf);
        buf
    }

    /// Decode from wire bytes.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < 20 {
            return None;
        }
        let code = data[0];
        let identifier = data[1];
        let length = u16::from_be_bytes([data[2], data[3]]) as usize;
        if length < 20 || data.len() < length {
            return None;
        }
        let mut authenticator = [0u8; AUTHENTICATOR_LEN];
        authenticator.copy_from_slice(&data[4..20]);

        let mut attributes = Vec::new();
        let mut offset = 20;
        while offset < length {
            if let Some((attr, consumed)) = RadiusAttribute::decode(&data[offset..]) {
                attributes.push(attr);
                offset += consumed;
            } else {
                break;
            }
        }

        Some(Self {
            code,
            identifier,
            authenticator,
            attributes,
        })
    }
}

// ---------------------------------------------------------------------------
// Simple MD5 for password encryption (simplified)
// ---------------------------------------------------------------------------

/// Simplified password encryption: XOR password with MD5(secret + authenticator).
/// In a real implementation this would use proper MD5. Here we use a simple
/// hash to demonstrate the protocol flow.
fn encrypt_password(password: &[u8], secret: &[u8], authenticator: &[u8; 16]) -> Vec<u8> {
    // Simple hash of secret + authenticator (not real MD5)
    let mut hash = [0u8; 16];
    let mut idx = 0usize;
    for &b in secret.iter().chain(authenticator.iter()) {
        hash[idx % 16] ^= b;
        idx += 1;
    }
    // Pad password to 16-byte boundary
    let padded_len = ((password.len() + 15) / 16) * 16;
    let padded_len = if padded_len == 0 { 16 } else { padded_len };
    let mut result = Vec::with_capacity(padded_len);
    for i in 0..padded_len {
        let p = if i < password.len() { password[i] } else { 0 };
        result.push(p ^ hash[i % 16]);
    }
    result
}

// ---------------------------------------------------------------------------
// Authentication result
// ---------------------------------------------------------------------------

/// Result of a RADIUS authentication attempt.
#[derive(Debug, Clone)]
pub enum AuthResult {
    Accept {
        reply_message: Option<String>,
        session_timeout: Option<u32>,
        framed_ip: Option<[u8; 4]>,
    },
    Reject {
        reply_message: Option<String>,
    },
    Challenge {
        state: Vec<u8>,
        reply_message: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Accounting status
// ---------------------------------------------------------------------------

/// Accounting status types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcctStatus {
    Start,
    Stop,
    InterimUpdate,
}

// ---------------------------------------------------------------------------
// Session tracking
// ---------------------------------------------------------------------------

/// An active session.
#[derive(Debug, Clone)]
struct Session {
    username: String,
    session_id: u64,
    start_tick: u64,
    last_update_tick: u64,
    status: AcctStatus,
}

// ---------------------------------------------------------------------------
// RADIUS client state
// ---------------------------------------------------------------------------

struct RadiusClient {
    /// Server IP address.
    server_ip: [u8; 4],
    /// Shared secret.
    shared_secret: Vec<u8>,
    /// NAS IP address.
    nas_ip: [u8; 4],
    /// Next packet identifier.
    next_id: u8,
    /// Active sessions.
    sessions: Vec<Session>,
    /// Next session ID.
    next_session_id: u64,
    /// Configured flag.
    configured: bool,
}

impl RadiusClient {
    const fn new() -> Self {
        Self {
            server_ip: [0; 4],
            shared_secret: Vec::new(),
            nas_ip: [10, 0, 0, 1],
            next_id: 1,
            sessions: Vec::new(),
            next_session_id: 1,
            configured: false,
        }
    }

    fn next_identifier(&mut self) -> u8 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }
}

static RADIUS_CLIENT: Mutex<RadiusClient> = Mutex::new(RadiusClient::new());
static AUTH_REQUESTS: AtomicU64 = AtomicU64::new(0);
static AUTH_ACCEPTS: AtomicU64 = AtomicU64::new(0);
static AUTH_REJECTS: AtomicU64 = AtomicU64::new(0);
static AUTH_CHALLENGES: AtomicU64 = AtomicU64::new(0);
static ACCT_REQUESTS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Configure the RADIUS server.
pub fn set_server(ip: [u8; 4], secret: &str) {
    let mut client = RADIUS_CLIENT.lock();
    client.server_ip = ip;
    client.shared_secret = secret.as_bytes().to_vec();
    client.configured = true;
    crate::serial_println!("[radius] server set to {}.{}.{}.{}",
        ip[0], ip[1], ip[2], ip[3]);
}

/// Set the NAS IP address.
pub fn set_nas_ip(ip: [u8; 4]) {
    let mut client = RADIUS_CLIENT.lock();
    client.nas_ip = ip;
}

/// Build an Access-Request packet (for inspection/testing).
pub fn build_access_request(username: &str, password: &str) -> Result<RadiusPacket, &'static str> {
    let mut client = RADIUS_CLIENT.lock();
    if !client.configured {
        return Err("RADIUS server not configured");
    }
    let id = client.next_identifier();
    // Generate a simple authenticator (would be random in production)
    let mut authenticator = [0u8; 16];
    let tick = crate::timer::ticks() as u64;
    for i in 0..16 {
        authenticator[i] = ((tick >> (i % 8)) & 0xFF) as u8 ^ (id.wrapping_add(i as u8));
    }
    let mut pkt = RadiusPacket::access_request(id, authenticator);
    pkt.add_attribute(RadiusAttribute::new_string(AttrType::UserName as u8, username));
    let encrypted = encrypt_password(
        password.as_bytes(),
        &client.shared_secret,
        &authenticator,
    );
    pkt.add_attribute(RadiusAttribute {
        attr_type: AttrType::UserPassword as u8,
        value: encrypted,
    });
    pkt.add_attribute(RadiusAttribute::new_ip(AttrType::NasIpAddress as u8, client.nas_ip));
    pkt.add_attribute(RadiusAttribute::new_u32(AttrType::NasPort as u8, 0));
    AUTH_REQUESTS.fetch_add(1, Ordering::Relaxed);
    Ok(pkt)
}

/// Simulate authentication (since we cannot send real UDP packets yet).
/// In production, this would send the packet and wait for a response.
pub fn authenticate(username: &str, password: &str) -> Result<AuthResult, &'static str> {
    let _pkt = build_access_request(username, password)?;
    // Simulate: accept if username is not empty and password >= 4 chars
    if username.is_empty() {
        AUTH_REJECTS.fetch_add(1, Ordering::Relaxed);
        return Ok(AuthResult::Reject {
            reply_message: Some(String::from("empty username")),
        });
    }
    if password.len() < 4 {
        AUTH_REJECTS.fetch_add(1, Ordering::Relaxed);
        return Ok(AuthResult::Reject {
            reply_message: Some(String::from("password too short")),
        });
    }
    AUTH_ACCEPTS.fetch_add(1, Ordering::Relaxed);
    Ok(AuthResult::Accept {
        reply_message: Some(String::from("Welcome")),
        session_timeout: Some(3600),
        framed_ip: Some([10, 0, 1, 100]),
    })
}

/// Start accounting for a session.
pub fn acct_start(username: &str) -> Result<u64, &'static str> {
    let mut client = RADIUS_CLIENT.lock();
    if !client.configured {
        return Err("RADIUS server not configured");
    }
    if client.sessions.len() >= MAX_SESSIONS {
        return Err("session limit reached");
    }
    let sid = client.next_session_id;
    client.next_session_id += 1;
    let now = crate::timer::ticks() as u64;
    client.sessions.push(Session {
        username: String::from(username),
        session_id: sid,
        start_tick: now,
        last_update_tick: now,
        status: AcctStatus::Start,
    });
    ACCT_REQUESTS.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[radius] accounting start for {} (session {})", username, sid);
    Ok(sid)
}

/// Stop accounting for a session.
pub fn acct_stop(session_id: u64) -> Result<(), &'static str> {
    let mut client = RADIUS_CLIENT.lock();
    let pos = client.sessions.iter().position(|s| s.session_id == session_id)
        .ok_or("session not found")?;
    client.sessions[pos].status = AcctStatus::Stop;
    ACCT_REQUESTS.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[radius] accounting stop for session {}", session_id);
    // Remove stopped session
    client.sessions.remove(pos);
    Ok(())
}

/// Send interim accounting update.
pub fn acct_interim(session_id: u64) -> Result<(), &'static str> {
    let mut client = RADIUS_CLIENT.lock();
    let session = client.sessions.iter_mut().find(|s| s.session_id == session_id)
        .ok_or("session not found")?;
    session.status = AcctStatus::InterimUpdate;
    session.last_update_tick = crate::timer::ticks() as u64;
    ACCT_REQUESTS.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// RADIUS client info.
pub fn radius_info() -> String {
    let client = RADIUS_CLIENT.lock();
    let mut out = String::from("RADIUS Client Information\n");
    if client.configured {
        out.push_str(&format!("  Server: {}.{}.{}.{}\n",
            client.server_ip[0], client.server_ip[1],
            client.server_ip[2], client.server_ip[3]));
        out.push_str(&format!("  Auth port: {}\n", RADIUS_AUTH_PORT));
        out.push_str(&format!("  Acct port: {}\n", RADIUS_ACCT_PORT));
        out.push_str(&format!("  NAS IP: {}.{}.{}.{}\n",
            client.nas_ip[0], client.nas_ip[1],
            client.nas_ip[2], client.nas_ip[3]));
        out.push_str(&format!("  Shared secret: {} bytes\n", client.shared_secret.len()));
        out.push_str(&format!("  Active sessions: {}\n", client.sessions.len()));
    } else {
        out.push_str("  Not configured. Use set_server() to configure.\n");
    }
    out
}

/// RADIUS statistics.
pub fn radius_stats() -> String {
    let client = RADIUS_CLIENT.lock();
    let mut out = String::from("RADIUS Statistics\n");
    out.push_str(&format!("  Auth requests:    {}\n", AUTH_REQUESTS.load(Ordering::Relaxed)));
    out.push_str(&format!("  Auth accepts:     {}\n", AUTH_ACCEPTS.load(Ordering::Relaxed)));
    out.push_str(&format!("  Auth rejects:     {}\n", AUTH_REJECTS.load(Ordering::Relaxed)));
    out.push_str(&format!("  Auth challenges:  {}\n", AUTH_CHALLENGES.load(Ordering::Relaxed)));
    out.push_str(&format!("  Acct requests:    {}\n", ACCT_REQUESTS.load(Ordering::Relaxed)));
    out.push_str(&format!("  Active sessions:  {}\n", client.sessions.len()));
    if !client.sessions.is_empty() {
        out.push_str("  Sessions:\n");
        for s in &client.sessions {
            out.push_str(&format!("    sid={} user={} status={:?}\n",
                s.session_id, s.username, s.status));
        }
    }
    out
}

/// Initialise the RADIUS subsystem.
pub fn init() {
    crate::serial_println!("[radius] RADIUS authentication client initialised");
}
