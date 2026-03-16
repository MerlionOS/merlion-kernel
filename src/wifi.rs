/// IEEE 802.11 WiFi driver for MerlionOS.
/// Implements WiFi scanning, authentication (WPA2-PSK), association,
/// and data frame handling for wireless network connectivity.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// 802.11 frame type/subtype constants
// ---------------------------------------------------------------------------

/// Frame type: Management (00)
const FRAME_TYPE_MGMT: u8 = 0x00;
/// Frame type: Control (01)
const FRAME_TYPE_CTRL: u8 = 0x01;
/// Frame type: Data (10)
const FRAME_TYPE_DATA: u8 = 0x02;

/// Management subtypes
const MGMT_ASSOC_REQ: u8 = 0x00;
const MGMT_ASSOC_RESP: u8 = 0x01;
const MGMT_PROBE_REQ: u8 = 0x04;
const MGMT_PROBE_RESP: u8 = 0x05;
const MGMT_BEACON: u8 = 0x08;
const MGMT_DISASSOC: u8 = 0x0A;
const MGMT_AUTH: u8 = 0x0B;
const MGMT_DEAUTH: u8 = 0x0C;

/// Control subtypes
const CTRL_ACK: u8 = 0x0D;
const CTRL_RTS: u8 = 0x0B;
const CTRL_CTS: u8 = 0x0C;

/// Data subtypes
const DATA_DATA: u8 = 0x00;
const DATA_QOS: u8 = 0x08;

/// Beacon interval in time units (1 TU = 1024 microseconds)
const BEACON_INTERVAL_TU: u16 = 100;

/// Maximum SSID length
const MAX_SSID_LEN: usize = 32;
/// Maximum scan results
const MAX_SCAN_RESULTS: usize = 16;
/// Maximum password length
const MAX_PASS_LEN: usize = 63;
/// PBKDF2 iteration count for WPA2-PSK
const PBKDF2_ITERATIONS: u32 = 4096;
/// PMK length (256 bits)
const PMK_LEN: usize = 32;
/// PTK length (512 bits)
const PTK_LEN: usize = 64;
/// Nonce length
const NONCE_LEN: usize = 32;

// ---------------------------------------------------------------------------
// Frequency / channel tables
// ---------------------------------------------------------------------------

/// 2.4 GHz channel frequencies (channels 1-14)
const CHANNELS_2G: [(u8, u16); 14] = [
    (1, 2412), (2, 2417), (3, 2422), (4, 2427), (5, 2432),
    (6, 2437), (7, 2442), (8, 2447), (9, 2452), (10, 2457),
    (11, 2462), (12, 2467), (13, 2472), (14, 2484),
];

/// 5 GHz channel frequencies (selected UNII bands)
const CHANNELS_5G: [(u8, u16); 12] = [
    (36, 5180), (40, 5200), (44, 5220), (48, 5240),
    (52, 5260), (56, 5280), (60, 5300), (64, 5320),
    (100, 5500), (149, 5745), (153, 5765), (165, 5825),
];

// ---------------------------------------------------------------------------
// Types and enums
// ---------------------------------------------------------------------------

/// Security type of a WiFi network.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SecurityType {
    Open,
    WEP,
    WPA2,
    WPA3,
}

impl SecurityType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SecurityType::Open => "Open",
            SecurityType::WEP => "WEP",
            SecurityType::WPA2 => "WPA2-PSK",
            SecurityType::WPA3 => "WPA3-SAE",
        }
    }
}

/// Frequency band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Band {
    Band2G,
    Band5G,
}

impl Band {
    pub fn as_str(&self) -> &'static str {
        match self {
            Band::Band2G => "2.4 GHz",
            Band::Band5G => "5 GHz",
        }
    }
}

/// Connection state machine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Scanning,
    Authenticating,
    Associating,
    Associated,
    Connected,
}

impl ConnectionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectionState::Disconnected => "Disconnected",
            ConnectionState::Scanning => "Scanning",
            ConnectionState::Authenticating => "Authenticating",
            ConnectionState::Associating => "Associating",
            ConnectionState::Associated => "Associated",
            ConnectionState::Connected => "Connected",
        }
    }
}

/// WPA2-PSK 4-way handshake state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HandshakeState {
    Idle,
    WaitMsg1,
    SentMsg2,
    WaitMsg3,
    Complete,
}

/// Power saving mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PowerSaveMode {
    Active,
    PSM,
    /// DTIM-based wake: wake every N DTIMs
    DTIMWake(u8),
}

/// QoS Access Category.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AccessCategory {
    BestEffort,
    Background,
    Video,
    Voice,
}

// ---------------------------------------------------------------------------
// 802.11 frame header
// ---------------------------------------------------------------------------

/// Simplified 802.11 MAC header (24 bytes for data frames).
#[derive(Debug, Clone)]
pub struct FrameHeader {
    pub frame_type: u8,
    pub subtype: u8,
    pub to_ds: bool,
    pub from_ds: bool,
    pub retry: bool,
    pub protected: bool,
    pub duration: u16,
    pub addr1: [u8; 6],
    pub addr2: [u8; 6],
    pub addr3: [u8; 6],
    pub seq_ctrl: u16,
}

impl FrameHeader {
    pub fn new_data(bssid: &[u8; 6], src: &[u8; 6], dst: &[u8; 6], seq: u16) -> Self {
        Self {
            frame_type: FRAME_TYPE_DATA,
            subtype: DATA_DATA,
            to_ds: true,
            from_ds: false,
            retry: false,
            protected: false,
            duration: 0,
            addr1: *bssid,
            addr2: *src,
            addr3: *dst,
            seq_ctrl: seq << 4,
        }
    }

    pub fn new_mgmt(subtype: u8, bssid: &[u8; 6], src: &[u8; 6]) -> Self {
        Self {
            frame_type: FRAME_TYPE_MGMT,
            subtype,
            to_ds: false,
            from_ds: false,
            retry: false,
            protected: false,
            duration: 0,
            addr1: *bssid,
            addr2: *src,
            addr3: *bssid,
            seq_ctrl: 0,
        }
    }

    /// Encode the frame control field (2 bytes, little-endian).
    pub fn frame_control(&self) -> u16 {
        let mut fc: u16 = 0;
        fc |= (self.frame_type as u16 & 0x03) << 2;
        fc |= (self.subtype as u16 & 0x0F) << 4;
        if self.to_ds { fc |= 1 << 8; }
        if self.from_ds { fc |= 1 << 9; }
        if self.retry { fc |= 1 << 11; }
        if self.protected { fc |= 1 << 14; }
        fc
    }

    /// Serialize into bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(24);
        let fc = self.frame_control();
        buf.push((fc & 0xFF) as u8);
        buf.push((fc >> 8) as u8);
        buf.push((self.duration & 0xFF) as u8);
        buf.push((self.duration >> 8) as u8);
        buf.extend_from_slice(&self.addr1);
        buf.extend_from_slice(&self.addr2);
        buf.extend_from_slice(&self.addr3);
        buf.push((self.seq_ctrl & 0xFF) as u8);
        buf.push((self.seq_ctrl >> 8) as u8);
        buf
    }
}

// ---------------------------------------------------------------------------
// BSS (Basic Service Set) — represents a WiFi network
// ---------------------------------------------------------------------------

/// A discovered WiFi network (BSS).
#[derive(Debug, Clone)]
pub struct BssInfo {
    pub ssid: String,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub frequency: u16,
    pub band: Band,
    pub rssi: i16,
    pub security: SecurityType,
    pub beacon_interval: u16,
    pub supported_rates: Vec<u8>,
    pub last_seen_tick: u64,
}

impl BssInfo {
    fn format_bssid(&self) -> String {
        format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.bssid[0], self.bssid[1], self.bssid[2],
            self.bssid[3], self.bssid[4], self.bssid[5])
    }

    fn signal_bars(&self) -> u8 {
        if self.rssi >= -50 { 4 }
        else if self.rssi >= -60 { 3 }
        else if self.rssi >= -70 { 2 }
        else if self.rssi >= -80 { 1 }
        else { 0 }
    }
}

// ---------------------------------------------------------------------------
// Scan request / result
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScanType {
    Passive,
    Active,
}

/// Scan configuration.
pub struct ScanConfig {
    pub scan_type: ScanType,
    pub dwell_time_ms: u32,
    pub band_filter: Option<Band>,
    pub channel_filter: Option<u8>,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            scan_type: ScanType::Active,
            dwell_time_ms: 100,
            band_filter: None,
            channel_filter: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Key derivation (simplified WPA2-PSK)
// ---------------------------------------------------------------------------

/// Simplified PBKDF2-SHA256 for WPA2-PSK PMK derivation.
/// In a real implementation this would use a proper HMAC-SHA256 PRF.
/// Here we use a deterministic mixing function for demonstration.
fn derive_pmk(passphrase: &[u8], ssid: &[u8]) -> [u8; PMK_LEN] {
    let mut pmk = [0u8; PMK_LEN];
    // Seed from passphrase and SSID
    let mut state: u64 = 0x5A17_B00F_CAFE_D00D;
    for &b in passphrase {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(b as u64);
    }
    for &b in ssid {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(b as u64);
    }
    // Iterate to simulate PBKDF2 cost
    for _ in 0..PBKDF2_ITERATIONS {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    }
    // Fill PMK
    for i in 0..PMK_LEN {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        pmk[i] = (state >> 33) as u8;
    }
    pmk
}

/// Derive PTK from PMK + nonces + MAC addresses (simplified).
fn derive_ptk(pmk: &[u8; PMK_LEN], anonce: &[u8; NONCE_LEN], snonce: &[u8; NONCE_LEN],
              bssid: &[u8; 6], sta_mac: &[u8; 6]) -> [u8; PTK_LEN] {
    let mut ptk = [0u8; PTK_LEN];
    let mut state: u64 = 0xDEAD_BEEF_1337_C0DE;
    for &b in pmk.iter() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(b as u64);
    }
    for &b in anonce.iter() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(b as u64);
    }
    for &b in snonce.iter() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(b as u64);
    }
    for &b in bssid.iter() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(b as u64);
    }
    for &b in sta_mac.iter() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(b as u64);
    }
    for i in 0..PTK_LEN {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        ptk[i] = (state >> 33) as u8;
    }
    ptk
}

/// Generate a pseudo-random nonce from a seed.
fn generate_nonce(seed: u64) -> [u8; NONCE_LEN] {
    let mut nonce = [0u8; NONCE_LEN];
    let mut state = seed;
    for i in 0..NONCE_LEN {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        nonce[i] = (state >> 33) as u8;
    }
    nonce
}

// ---------------------------------------------------------------------------
// Channel management
// ---------------------------------------------------------------------------

/// Regulatory domain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RegulatoryDomain {
    /// FCC (US): channels 1-11 (2.4 GHz), 36-165 (5 GHz)
    FCC,
    /// ETSI (EU): channels 1-13 (2.4 GHz), 36-64, 100-140 (5 GHz)
    ETSI,
    /// MKK (Japan): channels 1-14 (2.4 GHz), 36-64 (5 GHz)
    MKK,
    /// Singapore (SG): follows FCC
    SG,
}

impl RegulatoryDomain {
    pub fn max_2g_channel(&self) -> u8 {
        match self {
            RegulatoryDomain::FCC | RegulatoryDomain::SG => 11,
            RegulatoryDomain::ETSI => 13,
            RegulatoryDomain::MKK => 14,
        }
    }

    pub fn allowed_channel(&self, channel: u8, band: Band) -> bool {
        match band {
            Band::Band2G => channel >= 1 && channel <= self.max_2g_channel(),
            Band::Band5G => {
                // Check if channel is in the 5 GHz table
                CHANNELS_5G.iter().any(|&(ch, _)| ch == channel)
            }
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            RegulatoryDomain::FCC => "FCC (US)",
            RegulatoryDomain::ETSI => "ETSI (EU)",
            RegulatoryDomain::MKK => "MKK (JP)",
            RegulatoryDomain::SG => "SG",
        }
    }
}

fn channel_to_frequency(channel: u8) -> Option<u16> {
    for &(ch, freq) in CHANNELS_2G.iter() {
        if ch == channel { return Some(freq); }
    }
    for &(ch, freq) in CHANNELS_5G.iter() {
        if ch == channel { return Some(freq); }
    }
    None
}

fn channel_band(channel: u8) -> Band {
    if channel <= 14 { Band::Band2G } else { Band::Band5G }
}

// ---------------------------------------------------------------------------
// WiFi statistics
// ---------------------------------------------------------------------------

static TX_FRAMES: AtomicU64 = AtomicU64::new(0);
static RX_FRAMES: AtomicU64 = AtomicU64::new(0);
static TX_BYTES: AtomicU64 = AtomicU64::new(0);
static RX_BYTES: AtomicU64 = AtomicU64::new(0);
static BEACONS_RX: AtomicU64 = AtomicU64::new(0);
static RETRIES: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Global WiFi state
// ---------------------------------------------------------------------------

static INITIALIZED: AtomicBool = AtomicBool::new(false);

pub static WIFI: Mutex<WifiState> = Mutex::new(WifiState::new());

pub struct WifiState {
    pub state: ConnectionState,
    pub sta_mac: [u8; 6],
    pub scan_results: Vec<BssInfo>,
    pub connected_bss: Option<BssInfo>,
    pub pmk: [u8; PMK_LEN],
    pub ptk: [u8; PTK_LEN],
    pub snonce: [u8; NONCE_LEN],
    pub anonce: [u8; NONCE_LEN],
    pub handshake: HandshakeState,
    pub seq_num: u16,
    pub auto_reconnect: bool,
    pub last_ssid: String,
    pub last_pass: String,
    pub power_save: PowerSaveMode,
    pub regulatory: RegulatoryDomain,
    pub current_channel: u8,
    pub simulated_aps: Vec<BssInfo>,
    pub dtim_period: u8,
    pub dtim_count: u8,
}

impl WifiState {
    pub const fn new() -> Self {
        Self {
            state: ConnectionState::Disconnected,
            sta_mac: [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE],
            scan_results: Vec::new(),
            connected_bss: None,
            pmk: [0u8; PMK_LEN],
            ptk: [0u8; PTK_LEN],
            snonce: [0u8; NONCE_LEN],
            anonce: [0u8; NONCE_LEN],
            handshake: HandshakeState::Idle,
            seq_num: 0,
            auto_reconnect: true,
            last_ssid: String::new(),
            last_pass: String::new(),
            power_save: PowerSaveMode::Active,
            regulatory: RegulatoryDomain::SG,
            current_channel: 0,
            simulated_aps: Vec::new(),
            dtim_period: 3,
            dtim_count: 0,
        }
    }

    fn next_seq(&mut self) -> u16 {
        let s = self.seq_num;
        self.seq_num = self.seq_num.wrapping_add(1) & 0x0FFF;
        s
    }
}

// ---------------------------------------------------------------------------
// Simulated AP creation
// ---------------------------------------------------------------------------

fn create_simulated_aps() -> Vec<BssInfo> {
    let mut aps = Vec::with_capacity(4);

    // SG-themed WiFi networks
    aps.push(BssInfo {
        ssid: String::from("MerlionNet"),
        bssid: [0xAA, 0xBB, 0xCC, 0x01, 0x02, 0x03],
        channel: 6,
        frequency: 2437,
        band: Band::Band2G,
        rssi: -45,
        security: SecurityType::WPA2,
        beacon_interval: BEACON_INTERVAL_TU,
        supported_rates: alloc::vec![6, 9, 12, 18, 24, 36, 48, 54],
        last_seen_tick: 0,
    });

    aps.push(BssInfo {
        ssid: String::from("Wireless@SGx"),
        bssid: [0x00, 0x1A, 0x2B, 0x3C, 0x4D, 0x5E],
        channel: 1,
        frequency: 2412,
        band: Band::Band2G,
        rssi: -62,
        security: SecurityType::Open,
        beacon_interval: BEACON_INTERVAL_TU,
        supported_rates: alloc::vec![6, 12, 24, 48],
        last_seen_tick: 0,
    });

    aps.push(BssInfo {
        ssid: String::from("SINGTEL-5G-HOME"),
        bssid: [0x10, 0x20, 0x30, 0x40, 0x50, 0x60],
        channel: 36,
        frequency: 5180,
        band: Band::Band5G,
        rssi: -58,
        security: SecurityType::WPA2,
        beacon_interval: BEACON_INTERVAL_TU,
        supported_rates: alloc::vec![6, 9, 12, 18, 24, 36, 48, 54],
        last_seen_tick: 0,
    });

    aps.push(BssInfo {
        ssid: String::from("StarHub-WiFi6"),
        bssid: [0xF0, 0xE1, 0xD2, 0xC3, 0xB4, 0xA5],
        channel: 11,
        frequency: 2462,
        band: Band::Band2G,
        rssi: -73,
        security: SecurityType::WPA3,
        beacon_interval: BEACON_INTERVAL_TU,
        supported_rates: alloc::vec![12, 24, 48, 54],
        last_seen_tick: 0,
    });

    aps
}

// ---------------------------------------------------------------------------
// Scanner
// ---------------------------------------------------------------------------

/// Perform a passive scan: listen for beacons from simulated APs.
fn scan_passive(wifi: &mut WifiState) -> Vec<BssInfo> {
    let mut results = Vec::new();
    for ap in wifi.simulated_aps.iter() {
        // Simulate receiving a beacon frame
        BEACONS_RX.fetch_add(1, Ordering::Relaxed);
        RX_FRAMES.fetch_add(1, Ordering::Relaxed);
        RX_BYTES.fetch_add(64, Ordering::Relaxed); // typical beacon size

        if wifi.regulatory.allowed_channel(ap.channel, ap.band) {
            results.push(ap.clone());
        }
    }
    results
}

/// Perform an active scan: send probe requests and collect responses.
fn scan_active(wifi: &mut WifiState) -> Vec<BssInfo> {
    let mut results = Vec::new();

    // Send probe request on each channel
    let max_ch = wifi.regulatory.max_2g_channel();
    for ch in 1..=max_ch {
        // Build probe request frame
        let _hdr = FrameHeader::new_mgmt(
            MGMT_PROBE_REQ,
            &[0xFF; 6], // broadcast
            &wifi.sta_mac,
        );
        TX_FRAMES.fetch_add(1, Ordering::Relaxed);
        TX_BYTES.fetch_add(28, Ordering::Relaxed);

        // Check if any simulated AP is on this channel
        for ap in wifi.simulated_aps.iter() {
            if ap.channel == ch {
                // Simulate probe response
                RX_FRAMES.fetch_add(1, Ordering::Relaxed);
                RX_BYTES.fetch_add(72, Ordering::Relaxed);
                results.push(ap.clone());
            }
        }
    }

    // Also probe 5 GHz channels
    for &(ch, _) in CHANNELS_5G.iter() {
        if wifi.regulatory.allowed_channel(ch, Band::Band5G) {
            TX_FRAMES.fetch_add(1, Ordering::Relaxed);
            TX_BYTES.fetch_add(28, Ordering::Relaxed);

            for ap in wifi.simulated_aps.iter() {
                if ap.channel == ch {
                    RX_FRAMES.fetch_add(1, Ordering::Relaxed);
                    RX_BYTES.fetch_add(72, Ordering::Relaxed);
                    results.push(ap.clone());
                }
            }
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Authentication (WPA2-PSK 4-way handshake)
// ---------------------------------------------------------------------------

/// Perform simplified WPA2-PSK authentication with a BSS.
fn authenticate_wpa2(wifi: &mut WifiState, bss: &BssInfo, passphrase: &str) -> bool {
    if bss.security == SecurityType::Open {
        // Open network: no authentication needed
        return true;
    }

    wifi.handshake = HandshakeState::WaitMsg1;

    // Step 1: Derive PMK from passphrase + SSID
    wifi.pmk = derive_pmk(passphrase.as_bytes(), bss.ssid.as_bytes());

    // Step 2: Receive ANonce from AP (simulated — Message 1)
    wifi.anonce = generate_nonce(
        (bss.bssid[0] as u64) << 40 | (bss.bssid[1] as u64) << 32 |
        (bss.bssid[2] as u64) << 24 | (bss.bssid[3] as u64) << 16 |
        (bss.bssid[4] as u64) << 8  | (bss.bssid[5] as u64)
    );
    RX_FRAMES.fetch_add(1, Ordering::Relaxed);
    RX_BYTES.fetch_add(128, Ordering::Relaxed);

    // Step 3: Generate SNonce and derive PTK
    wifi.snonce = generate_nonce(
        (wifi.sta_mac[0] as u64) << 40 | (wifi.sta_mac[1] as u64) << 32 |
        (wifi.sta_mac[2] as u64) << 24 | (wifi.sta_mac[3] as u64) << 16 |
        (wifi.sta_mac[4] as u64) << 8  | (wifi.sta_mac[5] as u64)
    );
    wifi.ptk = derive_ptk(&wifi.pmk, &wifi.anonce, &wifi.snonce, &bss.bssid, &wifi.sta_mac);

    // Step 4: Send Message 2 (SNonce + MIC)
    wifi.handshake = HandshakeState::SentMsg2;
    TX_FRAMES.fetch_add(1, Ordering::Relaxed);
    TX_BYTES.fetch_add(128, Ordering::Relaxed);

    // Step 5: Receive Message 3 (GTK + MIC) — simulated success
    wifi.handshake = HandshakeState::WaitMsg3;
    RX_FRAMES.fetch_add(1, Ordering::Relaxed);
    RX_BYTES.fetch_add(192, Ordering::Relaxed);

    // Step 6: Send Message 4 (ACK)
    TX_FRAMES.fetch_add(1, Ordering::Relaxed);
    TX_BYTES.fetch_add(64, Ordering::Relaxed);

    wifi.handshake = HandshakeState::Complete;
    true
}

/// Send an 802.11 authentication frame (Open System).
fn send_auth_frame(wifi: &mut WifiState, bssid: &[u8; 6]) {
    let _hdr = FrameHeader::new_mgmt(MGMT_AUTH, bssid, &wifi.sta_mac);
    TX_FRAMES.fetch_add(1, Ordering::Relaxed);
    TX_BYTES.fetch_add(30, Ordering::Relaxed);
    // Simulate auth response
    RX_FRAMES.fetch_add(1, Ordering::Relaxed);
    RX_BYTES.fetch_add(30, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Association
// ---------------------------------------------------------------------------

/// Send association request and process response.
fn associate(wifi: &mut WifiState, bss: &BssInfo) -> bool {
    wifi.state = ConnectionState::Associating;

    // Send association request
    let _hdr = FrameHeader::new_mgmt(MGMT_ASSOC_REQ, &bss.bssid, &wifi.sta_mac);
    TX_FRAMES.fetch_add(1, Ordering::Relaxed);
    TX_BYTES.fetch_add(48, Ordering::Relaxed);

    // Simulate association response (success, AID=1)
    RX_FRAMES.fetch_add(1, Ordering::Relaxed);
    RX_BYTES.fetch_add(36, Ordering::Relaxed);

    wifi.state = ConnectionState::Associated;
    wifi.current_channel = bss.channel;
    true
}

/// Handle deauthentication from AP.
fn handle_deauth(wifi: &mut WifiState) {
    wifi.state = ConnectionState::Disconnected;
    wifi.connected_bss = None;
    wifi.handshake = HandshakeState::Idle;
    wifi.current_channel = 0;
}

// ---------------------------------------------------------------------------
// Data path
// ---------------------------------------------------------------------------

/// Encapsulate a payload into an 802.11 data frame.
pub fn encapsulate_data(wifi: &mut WifiState, dst: &[u8; 6], payload: &[u8]) -> Vec<u8> {
    let bssid = match &wifi.connected_bss {
        Some(bss) => bss.bssid,
        None => return Vec::new(),
    };

    let seq = wifi.next_seq();
    let hdr = FrameHeader::new_data(&bssid, &wifi.sta_mac, dst, seq);
    let mut frame = hdr.to_bytes();

    // LLC/SNAP header (8 bytes)
    frame.extend_from_slice(&[0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00]);
    // EtherType: IPv4
    frame.push(0x08);
    frame.push(0x00);

    // Payload
    frame.extend_from_slice(payload);

    // Update stats
    TX_FRAMES.fetch_add(1, Ordering::Relaxed);
    TX_BYTES.fetch_add(frame.len() as u64, Ordering::Relaxed);

    frame
}

/// Decapsulate an 802.11 data frame, returning the payload.
pub fn decapsulate_data(frame: &[u8]) -> Option<Vec<u8>> {
    // Minimum: 24 (header) + 8 (LLC/SNAP) = 32 bytes
    if frame.len() < 32 {
        return None;
    }

    // Check frame type = Data
    let fc = (frame[1] as u16) << 8 | frame[0] as u16;
    let ftype = ((fc >> 2) & 0x03) as u8;
    if ftype != FRAME_TYPE_DATA {
        return None;
    }

    RX_FRAMES.fetch_add(1, Ordering::Relaxed);
    RX_BYTES.fetch_add(frame.len() as u64, Ordering::Relaxed);

    // Skip 24-byte header + 8-byte LLC/SNAP
    Some(frame[32..].to_vec())
}

/// Encapsulate with QoS header (adds 2-byte QoS control field).
pub fn encapsulate_qos(wifi: &mut WifiState, dst: &[u8; 6], payload: &[u8],
                       ac: AccessCategory) -> Vec<u8> {
    let bssid = match &wifi.connected_bss {
        Some(bss) => bss.bssid,
        None => return Vec::new(),
    };

    let seq = wifi.next_seq();
    let mut hdr = FrameHeader::new_data(&bssid, &wifi.sta_mac, dst, seq);
    hdr.subtype = DATA_QOS;
    let mut frame = hdr.to_bytes();

    // QoS control field (2 bytes): TID based on AC
    let tid: u8 = match ac {
        AccessCategory::BestEffort => 0,
        AccessCategory::Background => 1,
        AccessCategory::Video => 5,
        AccessCategory::Voice => 6,
    };
    frame.push(tid);
    frame.push(0x00);

    // LLC/SNAP
    frame.extend_from_slice(&[0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00, 0x08, 0x00]);
    frame.extend_from_slice(payload);

    TX_FRAMES.fetch_add(1, Ordering::Relaxed);
    TX_BYTES.fetch_add(frame.len() as u64, Ordering::Relaxed);

    frame
}

// ---------------------------------------------------------------------------
// Connection manager
// ---------------------------------------------------------------------------

/// Connect to a WiFi network by SSID and passphrase.
pub fn wifi_connect(ssid: &str, passphrase: &str) -> Result<(), &'static str> {
    let mut wifi = WIFI.lock();

    if wifi.state == ConnectionState::Connected {
        return Err("Already connected; disconnect first");
    }
    if ssid.is_empty() || ssid.len() > MAX_SSID_LEN {
        return Err("Invalid SSID");
    }

    // Step 1: Scan for the target network
    wifi.state = ConnectionState::Scanning;
    let results = scan_active(&mut wifi);

    // Find the target BSS
    let bss = match results.iter().find(|b| b.ssid == ssid) {
        Some(b) => b.clone(),
        None => {
            wifi.state = ConnectionState::Disconnected;
            return Err("Network not found");
        }
    };

    // Step 2: Check security requirements
    if bss.security == SecurityType::WPA2 || bss.security == SecurityType::WPA3 {
        if passphrase.is_empty() || passphrase.len() > MAX_PASS_LEN {
            wifi.state = ConnectionState::Disconnected;
            return Err("Invalid passphrase");
        }
    }

    // Step 3: Open System Authentication
    send_auth_frame(&mut wifi, &bss.bssid);

    // Step 4: WPA2-PSK 4-way handshake
    wifi.state = ConnectionState::Authenticating;
    if bss.security != SecurityType::Open {
        if !authenticate_wpa2(&mut wifi, &bss, passphrase) {
            wifi.state = ConnectionState::Disconnected;
            return Err("Authentication failed");
        }
    }

    // Step 5: Association
    if !associate(&mut wifi, &bss) {
        wifi.state = ConnectionState::Disconnected;
        return Err("Association failed");
    }

    // Step 6: Connected
    wifi.state = ConnectionState::Connected;
    wifi.connected_bss = Some(bss);
    wifi.last_ssid = String::from(ssid);
    wifi.last_pass = String::from(passphrase);

    Ok(())
}

/// Disconnect from the current WiFi network.
pub fn wifi_disconnect() {
    let mut wifi = WIFI.lock();

    if let Some(ref bss) = wifi.connected_bss {
        // Send deauthentication frame
        let _hdr = FrameHeader::new_mgmt(MGMT_DEAUTH, &bss.bssid, &wifi.sta_mac);
        TX_FRAMES.fetch_add(1, Ordering::Relaxed);
        TX_BYTES.fetch_add(26, Ordering::Relaxed);
    }

    handle_deauth(&mut wifi);
    wifi.last_ssid.clear();
    wifi.last_pass.clear();
}

/// Scan for available WiFi networks.
pub fn wifi_scan() -> Vec<BssInfo> {
    let mut wifi = WIFI.lock();
    let prev_state = wifi.state;

    wifi.state = ConnectionState::Scanning;
    let results = scan_active(&mut wifi);
    wifi.scan_results = results.clone();
    wifi.state = prev_state;

    results
}

/// Return current connection state as a string.
pub fn wifi_status() -> String {
    let wifi = WIFI.lock();
    let state_str = wifi.state.as_str();

    if let Some(ref bss) = wifi.connected_bss {
        format!("Status: {} | SSID: {} | BSSID: {} | Ch: {} | RSSI: {} dBm | Security: {}",
            state_str, bss.ssid, bss.format_bssid(), bss.channel, bss.rssi,
            bss.security.as_str())
    } else {
        format!("Status: {}", state_str)
    }
}

/// Return detailed WiFi interface info.
pub fn wifi_info() -> String {
    let wifi = WIFI.lock();
    let mac = format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        wifi.sta_mac[0], wifi.sta_mac[1], wifi.sta_mac[2],
        wifi.sta_mac[3], wifi.sta_mac[4], wifi.sta_mac[5]);

    let mut info = format!(
        "WiFi Interface: wlan0\n  MAC: {}\n  State: {}\n  Regulatory: {}\n  Power Save: {:?}\n",
        mac, wifi.state.as_str(), wifi.regulatory.as_str(), wifi.power_save);

    if let Some(ref bss) = wifi.connected_bss {
        info.push_str(&format!(
            "  Connected to:\n    SSID: {}\n    BSSID: {}\n    Channel: {} ({})\n    Band: {}\n    Signal: {} dBm ({} bars)\n    Security: {}\n",
            bss.ssid, bss.format_bssid(), bss.channel, bss.frequency,
            bss.band.as_str(), bss.rssi, bss.signal_bars(), bss.security.as_str()));
    }

    info
}

/// Return packet statistics.
pub fn wifi_stats() -> String {
    format!(
        "WiFi Statistics:\n  TX frames: {}\n  RX frames: {}\n  TX bytes: {}\n  RX bytes: {}\n  Beacons: {}\n  Retries: {}",
        TX_FRAMES.load(Ordering::Relaxed),
        RX_FRAMES.load(Ordering::Relaxed),
        TX_BYTES.load(Ordering::Relaxed),
        RX_BYTES.load(Ordering::Relaxed),
        BEACONS_RX.load(Ordering::Relaxed),
        RETRIES.load(Ordering::Relaxed))
}

/// List all networks found in the last scan.
pub fn list_networks() -> String {
    let wifi = WIFI.lock();
    if wifi.scan_results.is_empty() {
        return String::from("No scan results. Run wifi_scan() first.");
    }

    let mut out = String::from("SSID                             BSSID              CH  RSSI  SECURITY   BAND\n");
    out.push_str(          "-------------------------------  -----------------  --  ----  ---------  -------\n");

    for bss in wifi.scan_results.iter() {
        let ssid_padded = if bss.ssid.len() < 31 {
            let mut s = bss.ssid.clone();
            for _ in 0..(31 - s.len()) { s.push(' '); }
            s
        } else {
            bss.ssid.clone()
        };
        out.push_str(&format!("{}  {}  {:2}  {:4}  {:9}  {}\n",
            ssid_padded, bss.format_bssid(), bss.channel, bss.rssi,
            bss.security.as_str(), bss.band.as_str()));
    }

    out
}

/// Set power save mode.
pub fn set_power_save(mode: PowerSaveMode) {
    let mut wifi = WIFI.lock();
    wifi.power_save = mode;
}

/// Set regulatory domain.
pub fn set_regulatory(domain: RegulatoryDomain) {
    let mut wifi = WIFI.lock();
    wifi.regulatory = domain;
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the WiFi subsystem with simulated access points.
pub fn init() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }

    let mut wifi = WIFI.lock();
    wifi.simulated_aps = create_simulated_aps();
    wifi.state = ConnectionState::Disconnected;
    wifi.regulatory = RegulatoryDomain::SG;

    crate::serial_println!("[wifi] 802.11 driver initialized, {} simulated APs",
        wifi.simulated_aps.len());
    crate::serial_println!("[wifi] MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        wifi.sta_mac[0], wifi.sta_mac[1], wifi.sta_mac[2],
        wifi.sta_mac[3], wifi.sta_mac[4], wifi.sta_mac[5]);
    crate::serial_println!("[wifi] Regulatory domain: {}", wifi.regulatory.as_str());
}
