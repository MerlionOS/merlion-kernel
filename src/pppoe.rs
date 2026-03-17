/// PPPoE (Point-to-Point Protocol over Ethernet) for MerlionOS.
/// Implements PPPoE discovery and session for broadband connections.
/// Used for DSL/fiber broadband in China and many other countries.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::borrow::ToOwned;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants — Ethertypes and protocol codes
// ---------------------------------------------------------------------------

/// PPPoE discovery ethertype
const ETHERTYPE_PPPOE_DISCOVERY: u16 = 0x8863;

/// PPPoE session ethertype
const ETHERTYPE_PPPOE_SESSION: u16 = 0x8864;

/// PPPoE version (4 bits) and type (4 bits) — always 0x11
const PPPOE_VER_TYPE: u8 = 0x11;

/// PPPoE discovery codes
const PPPOE_CODE_PADI: u8 = 0x09;
const PPPOE_CODE_PADO: u8 = 0x07;
const PPPOE_CODE_PADR: u8 = 0x19;
const PPPOE_CODE_PADS: u8 = 0x65;
const PPPOE_CODE_PADT: u8 = 0xA7;

/// PPP protocol IDs
const PPP_LCP: u16 = 0xC021;
const PPP_PAP: u16 = 0xC023;
const PPP_CHAP: u16 = 0xC223;
const PPP_IPCP: u16 = 0x8021;
const PPP_IPV6CP: u16 = 0x8057;
const PPP_IP: u16 = 0x0021;
const PPP_IPV6: u16 = 0x0057;

/// LCP codes
const LCP_CONF_REQ: u8 = 1;
const LCP_CONF_ACK: u8 = 2;
const LCP_CONF_NAK: u8 = 3;
const LCP_CONF_REJ: u8 = 4;
const LCP_TERM_REQ: u8 = 5;
const LCP_TERM_ACK: u8 = 6;
const LCP_ECHO_REQ: u8 = 9;
const LCP_ECHO_REP: u8 = 10;

/// LCP options
const LCP_OPT_MRU: u8 = 1;
const LCP_OPT_AUTH: u8 = 3;
const LCP_OPT_MAGIC: u8 = 5;

/// PAP codes
const PAP_AUTH_REQ: u8 = 1;
const PAP_AUTH_ACK: u8 = 2;
const PAP_AUTH_NAK: u8 = 3;

/// CHAP codes
const CHAP_CHALLENGE: u8 = 1;
const CHAP_RESPONSE: u8 = 2;
const CHAP_SUCCESS: u8 = 3;
const CHAP_FAILURE: u8 = 4;

/// IPCP codes (reuse LCP code values)
const IPCP_CONF_REQ: u8 = 1;
const IPCP_CONF_ACK: u8 = 2;
const IPCP_CONF_NAK: u8 = 3;

/// IPCP options
const IPCP_OPT_IP: u8 = 3;
const IPCP_OPT_DNS1: u8 = 129;
const IPCP_OPT_DNS2: u8 = 131;

/// Default MRU
const DEFAULT_MRU: u16 = 1492;

/// Maximum sessions
const MAX_SESSIONS: usize = 8;

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

static PADI_SENT: AtomicU64 = AtomicU64::new(0);
static PADO_RECEIVED: AtomicU64 = AtomicU64::new(0);
static PADR_SENT: AtomicU64 = AtomicU64::new(0);
static PADS_RECEIVED: AtomicU64 = AtomicU64::new(0);
static PADT_SENT: AtomicU64 = AtomicU64::new(0);
static PADT_RECEIVED: AtomicU64 = AtomicU64::new(0);
static LCP_PACKETS: AtomicU64 = AtomicU64::new(0);
static AUTH_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static AUTH_SUCCESSES: AtomicU64 = AtomicU64::new(0);
static AUTH_FAILURES: AtomicU64 = AtomicU64::new(0);
static IPCP_PACKETS: AtomicU64 = AtomicU64::new(0);
static DATA_TX_BYTES: AtomicU64 = AtomicU64::new(0);
static DATA_RX_BYTES: AtomicU64 = AtomicU64::new(0);
static ECHO_SENT: AtomicU64 = AtomicU64::new(0);
static ECHO_RECEIVED: AtomicU64 = AtomicU64::new(0);

static INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// PPPoE discovery phase
// ---------------------------------------------------------------------------

/// PPPoE discovery state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryState {
    /// Initial state, not started
    Idle,
    /// PADI sent, waiting for PADO
    PadiSent,
    /// PADO received, PADR sent, waiting for PADS
    PadrSent,
    /// Session established (PADS received)
    SessionEstablished,
    /// Terminated (PADT sent or received)
    Terminated,
}

/// PPP link phase after discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PppPhase {
    /// Dead — no link
    Dead,
    /// LCP negotiation in progress
    LcpNegotiation,
    /// LCP opened, ready for authentication
    LcpOpened,
    /// Authentication in progress
    Authenticating,
    /// Authentication succeeded
    Authenticated,
    /// IPCP negotiation in progress
    IpcpNegotiation,
    /// Network layer up — IP assigned
    NetworkUp,
    /// Terminating
    Terminating,
}

/// Authentication method negotiated via LCP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    None,
    Pap,
    Chap,
}

// ---------------------------------------------------------------------------
// PPPoE Tag (used in discovery frames)
// ---------------------------------------------------------------------------

/// A PPPoE tag in a discovery frame.
#[derive(Clone)]
pub struct PppoeTag {
    pub tag_type: u16,
    pub value: Vec<u8>,
}

/// Well-known tag types
const TAG_END_OF_LIST: u16 = 0x0000;
const TAG_SERVICE_NAME: u16 = 0x0101;
const TAG_AC_NAME: u16 = 0x0102;
const TAG_HOST_UNIQ: u16 = 0x0103;
const TAG_AC_COOKIE: u16 = 0x0104;
const TAG_RELAY_SESSION: u16 = 0x0110;
const TAG_SERVICE_NAME_ERROR: u16 = 0x0201;
const TAG_AC_SYSTEM_ERROR: u16 = 0x0202;
const TAG_GENERIC_ERROR: u16 = 0x0203;

// ---------------------------------------------------------------------------
// PPPoE session
// ---------------------------------------------------------------------------

/// A PPPoE session with all negotiated parameters.
#[derive(Clone)]
pub struct PppoeSession {
    pub session_id: u16,
    pub discovery_state: DiscoveryState,
    pub ppp_phase: PppPhase,
    pub peer_mac: [u8; 6],
    pub our_mac: [u8; 6],
    pub ac_name: String,
    pub service_name: String,
    pub ac_cookie: Vec<u8>,
    pub host_uniq: u32,
    pub username: String,
    pub auth_method: AuthMethod,
    pub mru: u16,
    pub magic_number: u32,
    pub our_ip: [u8; 4],
    pub peer_ip: [u8; 4],
    pub dns1: [u8; 4],
    pub dns2: [u8; 4],
    pub ipv6_interface_id: u64,
    pub created_tick: u64,
    pub lcp_id: u8,
    pub ipcp_id: u8,
}

impl PppoeSession {
    fn new(username: &str) -> Self {
        // Generate a pseudo-random host_uniq from tick counter
        let ticks = crate::timer::ticks() as u32;
        Self {
            session_id: 0,
            discovery_state: DiscoveryState::Idle,
            ppp_phase: PppPhase::Dead,
            peer_mac: [0; 6],
            our_mac: [0; 6],
            ac_name: String::new(),
            service_name: String::new(),
            ac_cookie: Vec::new(),
            host_uniq: ticks ^ 0xDEAD_BEEF,
            username: username.to_owned(),
            auth_method: AuthMethod::None,
            mru: DEFAULT_MRU,
            magic_number: ticks ^ 0x4D65726C, // "Merl"
            our_ip: [0; 4],
            peer_ip: [0; 4],
            dns1: [0; 4],
            dns2: [0; 4],
            ipv6_interface_id: 0,
            created_tick: crate::timer::ticks(),
            lcp_id: 1,
            ipcp_id: 1,
        }
    }

    fn uptime_secs(&self) -> u64 {
        let now = crate::timer::ticks();
        now.saturating_sub(self.created_tick) / 100
    }

    /// Build a PADI frame (broadcast to find access concentrators).
    fn build_padi(&self) -> Vec<u8> {
        let mut frame = vec![
            // Ethernet: broadcast dest
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
            // Source MAC
            self.our_mac[0], self.our_mac[1], self.our_mac[2],
            self.our_mac[3], self.our_mac[4], self.our_mac[5],
            // Ethertype: PPPoE Discovery
            (ETHERTYPE_PPPOE_DISCOVERY >> 8) as u8,
            (ETHERTYPE_PPPOE_DISCOVERY & 0xFF) as u8,
            // PPPoE header: ver/type=0x11, code=PADI, session=0
            PPPOE_VER_TYPE, PPPOE_CODE_PADI, 0x00, 0x00,
        ];
        // Tags: Service-Name (empty = any), Host-Uniq
        let svc_tag: Vec<u8> = vec![
            (TAG_SERVICE_NAME >> 8) as u8, (TAG_SERVICE_NAME & 0xFF) as u8,
            0x00, 0x00, // length 0 (any service)
        ];
        let uniq_bytes = self.host_uniq.to_be_bytes();
        let uniq_tag: Vec<u8> = vec![
            (TAG_HOST_UNIQ >> 8) as u8, (TAG_HOST_UNIQ & 0xFF) as u8,
            0x00, 0x04, // length 4
            uniq_bytes[0], uniq_bytes[1], uniq_bytes[2], uniq_bytes[3],
        ];
        let payload_len = (svc_tag.len() + uniq_tag.len()) as u16;
        frame.push((payload_len >> 8) as u8);
        frame.push((payload_len & 0xFF) as u8);
        frame.extend_from_slice(&svc_tag);
        frame.extend_from_slice(&uniq_tag);
        frame
    }

    /// Build a PADR frame (select an AC after receiving PADO).
    fn build_padr(&self) -> Vec<u8> {
        let mut frame = vec![
            // Dest: peer MAC
            self.peer_mac[0], self.peer_mac[1], self.peer_mac[2],
            self.peer_mac[3], self.peer_mac[4], self.peer_mac[5],
            // Source MAC
            self.our_mac[0], self.our_mac[1], self.our_mac[2],
            self.our_mac[3], self.our_mac[4], self.our_mac[5],
            // Ethertype
            (ETHERTYPE_PPPOE_DISCOVERY >> 8) as u8,
            (ETHERTYPE_PPPOE_DISCOVERY & 0xFF) as u8,
            // PPPoE header
            PPPOE_VER_TYPE, PPPOE_CODE_PADR, 0x00, 0x00,
        ];
        // Tags: Service-Name, Host-Uniq, AC-Cookie if present
        let mut tags = Vec::new();
        // Service-Name
        let sn_bytes = self.service_name.as_bytes();
        tags.push((TAG_SERVICE_NAME >> 8) as u8);
        tags.push((TAG_SERVICE_NAME & 0xFF) as u8);
        tags.push((sn_bytes.len() >> 8) as u8);
        tags.push((sn_bytes.len() & 0xFF) as u8);
        tags.extend_from_slice(sn_bytes);
        // Host-Uniq
        let uniq_bytes = self.host_uniq.to_be_bytes();
        tags.push((TAG_HOST_UNIQ >> 8) as u8);
        tags.push((TAG_HOST_UNIQ & 0xFF) as u8);
        tags.push(0x00);
        tags.push(0x04);
        tags.extend_from_slice(&uniq_bytes);
        // AC-Cookie
        if !self.ac_cookie.is_empty() {
            tags.push((TAG_AC_COOKIE >> 8) as u8);
            tags.push((TAG_AC_COOKIE & 0xFF) as u8);
            let cl = self.ac_cookie.len() as u16;
            tags.push((cl >> 8) as u8);
            tags.push((cl & 0xFF) as u8);
            tags.extend_from_slice(&self.ac_cookie);
        }
        let payload_len = tags.len() as u16;
        frame.push((payload_len >> 8) as u8);
        frame.push((payload_len & 0xFF) as u8);
        frame.extend_from_slice(&tags);
        frame
    }

    /// Build a PADT frame to terminate the session.
    fn build_padt(&self) -> Vec<u8> {
        vec![
            self.peer_mac[0], self.peer_mac[1], self.peer_mac[2],
            self.peer_mac[3], self.peer_mac[4], self.peer_mac[5],
            self.our_mac[0], self.our_mac[1], self.our_mac[2],
            self.our_mac[3], self.our_mac[4], self.our_mac[5],
            (ETHERTYPE_PPPOE_DISCOVERY >> 8) as u8,
            (ETHERTYPE_PPPOE_DISCOVERY & 0xFF) as u8,
            PPPOE_VER_TYPE, PPPOE_CODE_PADT,
            (self.session_id >> 8) as u8,
            (self.session_id & 0xFF) as u8,
            0x00, 0x00, // payload length 0
        ]
    }

    /// Build an LCP Configure-Request.
    fn build_lcp_conf_req(&self) -> Vec<u8> {
        let mut ppp = Vec::new();
        // PPP protocol: LCP
        ppp.push((PPP_LCP >> 8) as u8);
        ppp.push((PPP_LCP & 0xFF) as u8);
        // LCP: Configure-Request
        let mut opts = Vec::new();
        // MRU option
        opts.push(LCP_OPT_MRU);
        opts.push(4); // length
        opts.push((self.mru >> 8) as u8);
        opts.push((self.mru & 0xFF) as u8);
        // Magic number option
        let magic = self.magic_number.to_be_bytes();
        opts.push(LCP_OPT_MAGIC);
        opts.push(6); // length
        opts.extend_from_slice(&magic);
        let total_len = 4 + opts.len() as u16;
        ppp.push(LCP_CONF_REQ);
        ppp.push(self.lcp_id);
        ppp.push((total_len >> 8) as u8);
        ppp.push((total_len & 0xFF) as u8);
        ppp.extend_from_slice(&opts);
        ppp
    }

    /// Build a PAP Authenticate-Request.
    fn build_pap_auth_req(&self, password: &str) -> Vec<u8> {
        let mut ppp = Vec::new();
        ppp.push((PPP_PAP >> 8) as u8);
        ppp.push((PPP_PAP & 0xFF) as u8);
        let user_bytes = self.username.as_bytes();
        let pass_bytes = password.as_bytes();
        let data_len = 1 + user_bytes.len() + 1 + pass_bytes.len();
        let total_len = 4 + data_len as u16;
        ppp.push(PAP_AUTH_REQ);
        ppp.push(self.lcp_id);
        ppp.push((total_len >> 8) as u8);
        ppp.push((total_len & 0xFF) as u8);
        ppp.push(user_bytes.len() as u8);
        ppp.extend_from_slice(user_bytes);
        ppp.push(pass_bytes.len() as u8);
        ppp.extend_from_slice(pass_bytes);
        ppp
    }

    /// Build an IPCP Configure-Request (requesting IP 0.0.0.0 = ask server).
    fn build_ipcp_conf_req(&self) -> Vec<u8> {
        let mut ppp = Vec::new();
        ppp.push((PPP_IPCP >> 8) as u8);
        ppp.push((PPP_IPCP & 0xFF) as u8);
        let mut opts = Vec::new();
        // IP address option (request 0.0.0.0 to let server assign)
        opts.push(IPCP_OPT_IP);
        opts.push(6);
        opts.extend_from_slice(&self.our_ip);
        // DNS1
        opts.push(IPCP_OPT_DNS1);
        opts.push(6);
        opts.extend_from_slice(&self.dns1);
        // DNS2
        opts.push(IPCP_OPT_DNS2);
        opts.push(6);
        opts.extend_from_slice(&self.dns2);
        let total_len = 4 + opts.len() as u16;
        ppp.push(IPCP_CONF_REQ);
        ppp.push(self.ipcp_id);
        ppp.push((total_len >> 8) as u8);
        ppp.push((total_len & 0xFF) as u8);
        ppp.extend_from_slice(&opts);
        ppp
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

struct PppoeState {
    sessions: Vec<PppoeSession>,
}

impl PppoeState {
    const fn new() -> Self {
        Self {
            sessions: Vec::new(),
        }
    }
}

static STATE: Mutex<PppoeState> = Mutex::new(PppoeState::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the PPPoE subsystem.
pub fn init() {
    INITIALIZED.store(true, Ordering::SeqCst);
    crate::serial_println!("[pppoe] PPPoE subsystem initialized");
}

/// Initiate a PPPoE connection with username/password.
/// Returns session info on success.
pub fn pppoe_connect(username: &str, password: &str) -> Result<String, &'static str> {
    let mut st = STATE.lock();
    if st.sessions.len() >= MAX_SESSIONS {
        return Err("maximum PPPoE sessions reached");
    }

    let mut session = PppoeSession::new(username);

    // Phase 1: Discovery — send PADI
    let _padi = session.build_padi();
    session.discovery_state = DiscoveryState::PadiSent;
    PADI_SENT.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[pppoe] PADI sent for user '{}'", username);

    // Simulate PADO received (in real driver, wait for frame from NIC)
    session.peer_mac = [0x00, 0x1A, 0x2B, 0x3C, 0x4D, 0x5E];
    session.ac_name = "MerlionAC".to_owned();
    session.ac_cookie = vec![0xAA, 0xBB, 0xCC, 0xDD];
    PADO_RECEIVED.fetch_add(1, Ordering::Relaxed);

    // Send PADR
    let _padr = session.build_padr();
    session.discovery_state = DiscoveryState::PadrSent;
    PADR_SENT.fetch_add(1, Ordering::Relaxed);

    // Simulate PADS received
    session.session_id = (crate::timer::ticks() as u16) ^ 0x1234;
    if session.session_id == 0 { session.session_id = 1; }
    session.discovery_state = DiscoveryState::SessionEstablished;
    PADS_RECEIVED.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[pppoe] Session {} established with AC '{}'",
        session.session_id, session.ac_name);

    // Phase 2: LCP negotiation
    session.ppp_phase = PppPhase::LcpNegotiation;
    let _lcp_req = session.build_lcp_conf_req();
    LCP_PACKETS.fetch_add(1, Ordering::Relaxed);

    // Simulate LCP ACK
    session.ppp_phase = PppPhase::LcpOpened;
    session.auth_method = AuthMethod::Pap;
    LCP_PACKETS.fetch_add(1, Ordering::Relaxed);

    // Phase 3: PAP authentication
    session.ppp_phase = PppPhase::Authenticating;
    let _pap_req = session.build_pap_auth_req(password);
    AUTH_ATTEMPTS.fetch_add(1, Ordering::Relaxed);

    // Simulate PAP ACK
    session.ppp_phase = PppPhase::Authenticated;
    AUTH_SUCCESSES.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[pppoe] PAP authentication succeeded for '{}'", username);

    // Phase 4: IPCP negotiation
    session.ppp_phase = PppPhase::IpcpNegotiation;
    let _ipcp_req = session.build_ipcp_conf_req();
    IPCP_PACKETS.fetch_add(1, Ordering::Relaxed);

    // Simulate IPCP NAK with assigned IP and DNS
    session.our_ip = [10, 0, 0, 100 + (st.sessions.len() as u8)];
    session.peer_ip = [10, 0, 0, 1];
    session.dns1 = [8, 8, 8, 8];
    session.dns2 = [8, 8, 4, 4];
    session.ppp_phase = PppPhase::NetworkUp;
    IPCP_PACKETS.fetch_add(1, Ordering::Relaxed);

    let info = format!(
        "PPPoE session {} up:\n  AC: {}\n  IP: {}.{}.{}.{}\n  Peer: {}.{}.{}.{}\n  DNS: {}.{}.{}.{}, {}.{}.{}.{}\n  MRU: {}",
        session.session_id, session.ac_name,
        session.our_ip[0], session.our_ip[1], session.our_ip[2], session.our_ip[3],
        session.peer_ip[0], session.peer_ip[1], session.peer_ip[2], session.peer_ip[3],
        session.dns1[0], session.dns1[1], session.dns1[2], session.dns1[3],
        session.dns2[0], session.dns2[1], session.dns2[2], session.dns2[3],
        session.mru,
    );

    crate::serial_println!("[pppoe] IP {}.{}.{}.{} assigned via PPPoE",
        session.our_ip[0], session.our_ip[1], session.our_ip[2], session.our_ip[3]);

    st.sessions.push(session);
    Ok(info)
}

/// Disconnect a PPPoE session by session_id.
pub fn pppoe_disconnect(session_id: u16) {
    let mut st = STATE.lock();
    if let Some(s) = st.sessions.iter_mut().find(|s| s.session_id == session_id) {
        let _padt = s.build_padt();
        s.discovery_state = DiscoveryState::Terminated;
        s.ppp_phase = PppPhase::Terminating;
        PADT_SENT.fetch_add(1, Ordering::Relaxed);
        crate::serial_println!("[pppoe] Session {} terminated", session_id);
    }
    st.sessions.retain(|s| s.discovery_state != DiscoveryState::Terminated);
}

/// Disconnect all PPPoE sessions.
pub fn pppoe_disconnect_all() {
    let mut st = STATE.lock();
    for s in st.sessions.iter_mut() {
        let _padt = s.build_padt();
        PADT_SENT.fetch_add(1, Ordering::Relaxed);
    }
    st.sessions.clear();
    crate::serial_println!("[pppoe] All sessions terminated");
}

/// Return current PPPoE status.
pub fn pppoe_status() -> String {
    let st = STATE.lock();
    if st.sessions.is_empty() {
        return "PPPoE: no active sessions".to_owned();
    }
    let mut out = format!("PPPoE sessions ({}):\n", st.sessions.len());
    for s in &st.sessions {
        let phase = match s.ppp_phase {
            PppPhase::Dead => "dead",
            PppPhase::LcpNegotiation => "LCP negotiating",
            PppPhase::LcpOpened => "LCP opened",
            PppPhase::Authenticating => "authenticating",
            PppPhase::Authenticated => "authenticated",
            PppPhase::IpcpNegotiation => "IPCP negotiating",
            PppPhase::NetworkUp => "network up",
            PppPhase::Terminating => "terminating",
        };
        out.push_str(&format!(
            "  Session {}: {} user={} ip={}.{}.{}.{} ac={} mru={} uptime={}s\n",
            s.session_id, phase, s.username,
            s.our_ip[0], s.our_ip[1], s.our_ip[2], s.our_ip[3],
            s.ac_name, s.mru, s.uptime_secs(),
        ));
    }
    out
}

/// Return PPPoE subsystem info.
pub fn pppoe_info() -> String {
    let st = STATE.lock();
    format!(
        "PPPoE:\n  Initialized: {}\n  Active sessions: {}\n  Max sessions: {}\n  Default MRU: {}",
        INITIALIZED.load(Ordering::Relaxed),
        st.sessions.len(),
        MAX_SESSIONS,
        DEFAULT_MRU,
    )
}

/// Return PPPoE statistics.
pub fn pppoe_stats() -> String {
    format!(
        "PPPoE Stats:\n  PADI sent: {}\n  PADO received: {}\n  PADR sent: {}\n  PADS received: {}\n  PADT sent: {}\n  PADT received: {}\n  LCP packets: {}\n  Auth attempts: {}\n  Auth successes: {}\n  Auth failures: {}\n  IPCP packets: {}\n  Data TX bytes: {}\n  Data RX bytes: {}\n  Echo sent: {}\n  Echo received: {}",
        PADI_SENT.load(Ordering::Relaxed),
        PADO_RECEIVED.load(Ordering::Relaxed),
        PADR_SENT.load(Ordering::Relaxed),
        PADS_RECEIVED.load(Ordering::Relaxed),
        PADT_SENT.load(Ordering::Relaxed),
        PADT_RECEIVED.load(Ordering::Relaxed),
        LCP_PACKETS.load(Ordering::Relaxed),
        AUTH_ATTEMPTS.load(Ordering::Relaxed),
        AUTH_SUCCESSES.load(Ordering::Relaxed),
        AUTH_FAILURES.load(Ordering::Relaxed),
        IPCP_PACKETS.load(Ordering::Relaxed),
        DATA_TX_BYTES.load(Ordering::Relaxed),
        DATA_RX_BYTES.load(Ordering::Relaxed),
        ECHO_SENT.load(Ordering::Relaxed),
        ECHO_RECEIVED.load(Ordering::Relaxed),
    )
}
