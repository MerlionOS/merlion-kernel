/// NTP (Network Time Protocol) client for MerlionOS.
/// Synchronizes system time with NTP servers using simplified
/// SNTP (Simple NTP) protocol over UDP port 123.
///
/// Implements a subset of RFC 4330 (SNTP v4) with integer-only
/// arithmetic for clock offset calculation and drift tracking.
/// Uses `spin::Mutex` for thread-safety in `no_std` kernel context.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

use crate::net::Ipv4Addr;
use crate::timer;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// NTP uses UDP port 123.
const NTP_PORT: u16 = 123;

/// NTP packet size (48 bytes minimum).
const NTP_PACKET_LEN: usize = 48;

/// NTP epoch offset from Unix epoch: seconds between 1900-01-01 and 1970-01-01.
const NTP_EPOCH_OFFSET: u64 = 2_208_988_800;

/// Default poll interval in seconds (64s = 2^6).
const DEFAULT_POLL_INTERVAL: u32 = 64;

/// Maximum number of configured NTP servers.
const MAX_SERVERS: usize = 4;

/// LI=0 (no warning), VN=4 (NTPv4), Mode=3 (client)
const NTP_CLIENT_FLAGS: u8 = 0x23;

// ---------------------------------------------------------------------------
// NTP Packet
// ---------------------------------------------------------------------------

/// Represents an NTP packet (48 bytes).
///
/// Fields are stored in host byte order after parsing.
#[derive(Debug, Clone, Copy)]
pub struct NtpPacket {
    /// LI (2 bits) | VN (3 bits) | Mode (3 bits)
    pub flags: u8,
    /// Stratum level (0=unspecified, 1=primary, 2-15=secondary)
    pub stratum: u8,
    /// Poll interval (log2 seconds)
    pub poll: u8,
    /// Precision (log2 seconds, signed)
    pub precision: i8,
    /// Root delay (fixed-point seconds)
    pub root_delay: u32,
    /// Root dispersion (fixed-point seconds)
    pub root_dispersion: u32,
    /// Reference identifier
    pub ref_id: u32,
    /// Reference timestamp (seconds + fraction)
    pub ref_ts_sec: u32,
    pub ref_ts_frac: u32,
    /// Originate timestamp (seconds + fraction)
    pub orig_ts_sec: u32,
    pub orig_ts_frac: u32,
    /// Receive timestamp (seconds + fraction)
    pub recv_ts_sec: u32,
    pub recv_ts_frac: u32,
    /// Transmit timestamp (seconds + fraction)
    pub xmit_ts_sec: u32,
    pub xmit_ts_frac: u32,
}

impl NtpPacket {
    /// Create a client request packet.
    pub fn new_request() -> Self {
        Self {
            flags: NTP_CLIENT_FLAGS,
            stratum: 0,
            poll: 6, // 2^6 = 64 seconds
            precision: -6, // ~15ms
            root_delay: 0,
            root_dispersion: 0,
            ref_id: 0,
            ref_ts_sec: 0,
            ref_ts_frac: 0,
            orig_ts_sec: 0,
            orig_ts_frac: 0,
            recv_ts_sec: 0,
            recv_ts_frac: 0,
            xmit_ts_sec: 0,
            xmit_ts_frac: 0,
        }
    }

    /// Serialize the packet to 48 bytes (big-endian).
    pub fn to_bytes(&self) -> [u8; NTP_PACKET_LEN] {
        let mut buf = [0u8; NTP_PACKET_LEN];
        buf[0] = self.flags;
        buf[1] = self.stratum;
        buf[2] = self.poll;
        buf[3] = self.precision as u8;
        put_u32(&mut buf, 4, self.root_delay);
        put_u32(&mut buf, 8, self.root_dispersion);
        put_u32(&mut buf, 12, self.ref_id);
        put_u32(&mut buf, 16, self.ref_ts_sec);
        put_u32(&mut buf, 20, self.ref_ts_frac);
        put_u32(&mut buf, 24, self.orig_ts_sec);
        put_u32(&mut buf, 28, self.orig_ts_frac);
        put_u32(&mut buf, 32, self.recv_ts_sec);
        put_u32(&mut buf, 36, self.recv_ts_frac);
        put_u32(&mut buf, 40, self.xmit_ts_sec);
        put_u32(&mut buf, 44, self.xmit_ts_frac);
        buf
    }

    /// Parse an NTP packet from 48 bytes.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < NTP_PACKET_LEN {
            return None;
        }
        Some(Self {
            flags: data[0],
            stratum: data[1],
            poll: data[2],
            precision: data[3] as i8,
            root_delay: get_u32(data, 4),
            root_dispersion: get_u32(data, 8),
            ref_id: get_u32(data, 12),
            ref_ts_sec: get_u32(data, 16),
            ref_ts_frac: get_u32(data, 20),
            orig_ts_sec: get_u32(data, 24),
            orig_ts_frac: get_u32(data, 28),
            recv_ts_sec: get_u32(data, 32),
            recv_ts_frac: get_u32(data, 36),
            xmit_ts_sec: get_u32(data, 40),
            xmit_ts_frac: get_u32(data, 44),
        })
    }

    /// Extract the mode field (bits 0-2).
    pub fn mode(&self) -> u8 {
        self.flags & 0x07
    }

    /// Extract the version field (bits 3-5).
    pub fn version(&self) -> u8 {
        (self.flags >> 3) & 0x07
    }

    /// Extract the leap indicator (bits 6-7).
    pub fn leap_indicator(&self) -> u8 {
        (self.flags >> 6) & 0x03
    }
}

// ---------------------------------------------------------------------------
// Byte helpers
// ---------------------------------------------------------------------------

fn get_u32(data: &[u8], offset: usize) -> u32 {
    ((data[offset] as u32) << 24)
        | ((data[offset + 1] as u32) << 16)
        | ((data[offset + 2] as u32) << 8)
        | (data[offset + 3] as u32)
}

fn put_u32(buf: &mut [u8], offset: usize, v: u32) {
    buf[offset] = (v >> 24) as u8;
    buf[offset + 1] = (v >> 16) as u8;
    buf[offset + 2] = (v >> 8) as u8;
    buf[offset + 3] = v as u8;
}

// ---------------------------------------------------------------------------
// Client state
// ---------------------------------------------------------------------------

/// NTP server entry.
#[derive(Debug, Clone)]
struct NtpServer {
    ip: Ipv4Addr,
    active: bool,
}

/// Internal state of the NTP client.
struct NtpState {
    servers: [Option<Ipv4Addr>; MAX_SERVERS],
    server_count: usize,
    /// Current server index being used.
    current_server: usize,
    /// Our stratum (server stratum + 1).
    stratum: u8,
    /// Last server stratum seen.
    server_stratum: u8,
    /// Clock offset in milliseconds (signed, stored as i64).
    offset_ms: i64,
    /// Estimated drift in ms per hour (integer approximation).
    drift_ms_per_hour: i64,
    /// Poll interval in seconds.
    poll_interval: u32,
    /// Tick at which last successful sync occurred.
    last_sync_tick: u64,
    /// Number of samples used for drift estimation.
    drift_samples: u32,
    /// Previous offset for drift calculation.
    prev_offset_ms: i64,
    /// Tick of previous offset measurement.
    prev_offset_tick: u64,
}

impl NtpState {
    const fn new() -> Self {
        Self {
            servers: [None; MAX_SERVERS],
            server_count: 0,
            current_server: 0,
            stratum: 16, // 16 = unsynchronized
            server_stratum: 0,
            offset_ms: 0,
            drift_ms_per_hour: 0,
            poll_interval: DEFAULT_POLL_INTERVAL,
            last_sync_tick: 0,
            drift_samples: 0,
            prev_offset_ms: 0,
            prev_offset_tick: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static STATE: Mutex<NtpState> = Mutex::new(NtpState::new());
static ENABLED: AtomicBool = AtomicBool::new(false);

// Statistics
static SYNCS_ATTEMPTED: AtomicU64 = AtomicU64::new(0);
static SYNCS_SUCCEEDED: AtomicU64 = AtomicU64::new(0);
static SYNCS_FAILED: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the NTP subsystem with default pool.ntp.org servers.
pub fn init() {
    let mut state = STATE.lock();
    // Default NTP servers (well-known pool.ntp.org addresses)
    let defaults: [[u8; 4]; 4] = [
        [129, 6, 15, 28],   // time-a-g.nist.gov
        [129, 6, 15, 29],   // time-b-g.nist.gov
        [132, 163, 96, 1],  // time-a-wwv.nist.gov
        [132, 163, 96, 2],  // time-b-wwv.nist.gov
    ];
    for (i, addr) in defaults.iter().enumerate() {
        state.servers[i] = Some(Ipv4Addr(*addr));
    }
    state.server_count = defaults.len();
    drop(state);
    crate::serial_println!("[ntp] initialized with {} default servers", defaults.len());
}

/// Set or add an NTP server by IP address.
pub fn set_server(ip: [u8; 4]) {
    let mut state = STATE.lock();
    if state.server_count < MAX_SERVERS {
        let idx = state.server_count;
        state.servers[idx] = Some(Ipv4Addr(ip));
        state.server_count += 1;
        crate::serial_println!(
            "[ntp] added server {}.{}.{}.{}",
            ip[0], ip[1], ip[2], ip[3]
        );
    } else {
        // Replace current server
        let idx = state.current_server;
        state.servers[idx] = Some(Ipv4Addr(ip));
        crate::serial_println!(
            "[ntp] replaced server slot {} with {}.{}.{}.{}",
            idx, ip[0], ip[1], ip[2], ip[3]
        );
    }
}

/// Set the poll interval (in seconds).
pub fn set_poll_interval(seconds: u32) {
    let secs = if seconds < 16 { 16 } else if seconds > 1024 { 1024 } else { seconds };
    let mut state = STATE.lock();
    state.poll_interval = secs;
    crate::serial_println!("[ntp] poll interval set to {}s", secs);
}

/// Build an NTP request packet for sending to a server.
pub fn build_request() -> [u8; NTP_PACKET_LEN] {
    let mut pkt = NtpPacket::new_request();
    // Set transmit timestamp to current system time estimate
    let uptime = timer::uptime_secs();
    // Use a base NTP timestamp (approximate: 2024-01-01 in NTP seconds)
    let ntp_base: u32 = 3_913_056_000; // approx 2024-01-01 00:00:00 NTP
    pkt.xmit_ts_sec = ntp_base.wrapping_add(uptime as u32);
    pkt.xmit_ts_frac = 0;
    pkt.to_bytes()
}

/// Process an NTP response and update clock offset.
///
/// `server_ip` is the IP of the server that sent this response.
/// `data` is the raw 48-byte NTP response.
/// `send_tick` is the timer tick when the request was sent.
/// `recv_tick` is the timer tick when the response was received.
///
/// Returns the calculated offset in milliseconds, or None on error.
pub fn process_response(
    data: &[u8],
    send_tick: u64,
    recv_tick: u64,
) -> Option<i64> {
    let pkt = NtpPacket::from_bytes(data)?;

    // Validate: must be server mode (4) or broadcast (5)
    let mode = pkt.mode();
    if mode != 4 && mode != 5 {
        return None;
    }

    // Reject stratum 0 (kiss-o-death) or stratum >= 16
    if pkt.stratum == 0 || pkt.stratum >= 16 {
        return None;
    }

    // Calculate round-trip delay and offset using integer math
    // All timestamps in seconds (we ignore fractions for simplicity)
    //
    // T1 = originate (our send time in NTP seconds)
    // T2 = receive (server receive time)
    // T3 = transmit (server send time)
    // T4 = our receive time in NTP seconds
    //
    // offset = ((T2 - T1) + (T3 - T4)) / 2
    // delay  = (T4 - T1) - (T3 - T2)

    let t1 = pkt.orig_ts_sec as i64;
    let t2 = pkt.recv_ts_sec as i64;
    let t3 = pkt.xmit_ts_sec as i64;

    // Convert our ticks to approximate NTP seconds
    let rtt_ticks = recv_tick.saturating_sub(send_tick);
    let rtt_ms = (rtt_ticks * 10) as i64; // 100 Hz ticks to ms

    let ntp_base: i64 = 3_913_056_000;
    let t4 = ntp_base + (timer::uptime_secs() as i64);

    // Offset in seconds, then convert to milliseconds
    let offset_sec = ((t2 - t1) + (t3 - t4)) / 2;
    let offset_ms = offset_sec * 1000;

    // Update state
    let mut state = STATE.lock();
    let prev_offset = state.offset_ms;
    let prev_tick = state.prev_offset_tick;

    state.offset_ms = offset_ms;
    state.server_stratum = pkt.stratum;
    state.stratum = pkt.stratum + 1;
    state.last_sync_tick = recv_tick;

    // Update drift estimate if we have a previous measurement
    if state.drift_samples > 0 && prev_tick > 0 {
        let elapsed_ticks = recv_tick.saturating_sub(prev_tick);
        if elapsed_ticks > 100 {
            // At least 1 second between samples
            let elapsed_hours_x100 = (elapsed_ticks * 100) / (100 * 3600); // hundredths of hours
            if elapsed_hours_x100 > 0 {
                let delta_ms = offset_ms - prev_offset;
                let new_drift = (delta_ms * 100) / (elapsed_hours_x100 as i64);
                // Exponential moving average: 7/8 old + 1/8 new
                state.drift_ms_per_hour =
                    (state.drift_ms_per_hour * 7 + new_drift) / 8;
            }
        }
    }

    state.prev_offset_ms = offset_ms;
    state.prev_offset_tick = recv_tick;
    state.drift_samples += 1;
    drop(state);

    SYNCS_SUCCEEDED.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!(
        "[ntp] sync ok: offset={}ms rtt={}ms stratum={}",
        offset_ms, rtt_ms, pkt.stratum
    );

    Some(offset_ms)
}

/// Perform a synchronization attempt with the given server IP.
///
/// This is a high-level function that builds the request, sends it,
/// and processes the response. In a real implementation, this would
/// use the UDP stack. Currently returns a simulated result.
pub fn sync(server_ip: [u8; 4]) -> Result<i64, &'static str> {
    SYNCS_ATTEMPTED.fetch_add(1, Ordering::Relaxed);

    let _request = build_request();
    let send_tick = timer::ticks();

    crate::serial_println!(
        "[ntp] sending request to {}.{}.{}.{}",
        server_ip[0], server_ip[1], server_ip[2], server_ip[3]
    );

    // In a real implementation we would:
    // 1. Send _request via UDP to server_ip:123
    // 2. Wait for response
    // 3. Call process_response() with the response data

    // Simulated response: pretend server is in sync (offset ~0)
    let recv_tick = send_tick + 5; // ~50ms simulated RTT

    // Update state with simulated zero offset
    let mut state = STATE.lock();
    state.offset_ms = 0;
    state.server_stratum = 1;
    state.stratum = 2;
    state.last_sync_tick = recv_tick;
    state.drift_samples += 1;
    drop(state);

    SYNCS_SUCCEEDED.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[ntp] sync complete: offset=0ms (simulated)");
    Ok(0)
}

/// Get the current clock offset in milliseconds.
pub fn offset_ms() -> i64 {
    STATE.lock().offset_ms
}

/// Get the current NTP time as seconds since NTP epoch (1900-01-01).
pub fn get_ntp_time() -> u64 {
    let uptime = timer::uptime_secs();
    let offset = STATE.lock().offset_ms;
    let ntp_base: u64 = 3_913_056_000;
    let ntp_secs = ntp_base + uptime;
    if offset >= 0 {
        ntp_secs + ((offset as u64) / 1000)
    } else {
        ntp_secs.saturating_sub(((-offset) as u64) / 1000)
    }
}

/// Get current time as Unix timestamp (seconds since 1970-01-01).
pub fn get_unix_time() -> u64 {
    get_ntp_time().saturating_sub(NTP_EPOCH_OFFSET)
}

// ---------------------------------------------------------------------------
// Info / stats
// ---------------------------------------------------------------------------

/// Return NTP client status information.
pub fn ntp_info() -> String {
    let state = STATE.lock();
    let mut out = String::from("=== MerlionOS NTP Client ===\n");
    out.push_str(&format!("Stratum:        {}\n", state.stratum));
    out.push_str(&format!("Server stratum: {}\n", state.server_stratum));
    out.push_str(&format!("Clock offset:   {}ms\n", state.offset_ms));
    out.push_str(&format!("Drift:          {}ms/hour\n", state.drift_ms_per_hour));
    out.push_str(&format!("Poll interval:  {}s\n", state.poll_interval));

    let now = timer::ticks();
    if state.last_sync_tick > 0 {
        let ago = (now - state.last_sync_tick) / 100;
        out.push_str(&format!("Last sync:      {}s ago\n", ago));
    } else {
        out.push_str("Last sync:      never\n");
    }

    out.push_str(&format!("Drift samples:  {}\n", state.drift_samples));

    out.push_str("\nConfigured servers:\n");
    for i in 0..state.server_count {
        if let Some(ip) = &state.servers[i] {
            let marker = if i == state.current_server { " *" } else { "" };
            out.push_str(&format!("  {}{}\n", ip, marker));
        }
    }

    let ntp_time = get_ntp_time();
    let unix_time = ntp_time.saturating_sub(NTP_EPOCH_OFFSET);
    out.push_str(&format!("\nNTP time:  {} (NTP epoch)\n", ntp_time));
    out.push_str(&format!("Unix time: {} (Unix epoch)\n", unix_time));

    out
}

/// Return NTP statistics.
pub fn ntp_stats() -> String {
    let state = STATE.lock();
    let mut out = String::from("=== NTP Statistics ===\n");
    out.push_str(&format!(
        "Syncs attempted:  {}\n",
        SYNCS_ATTEMPTED.load(Ordering::Relaxed)
    ));
    out.push_str(&format!(
        "Syncs succeeded:  {}\n",
        SYNCS_SUCCEEDED.load(Ordering::Relaxed)
    ));
    out.push_str(&format!(
        "Syncs failed:     {}\n",
        SYNCS_FAILED.load(Ordering::Relaxed)
    ));
    out.push_str(&format!("Current offset:   {}ms\n", state.offset_ms));
    out.push_str(&format!("Current drift:    {}ms/hour\n", state.drift_ms_per_hour));
    out.push_str(&format!("Drift samples:    {}\n", state.drift_samples));
    out.push_str(&format!("Server stratum:   {}\n", state.server_stratum));
    out.push_str(&format!("Our stratum:      {}\n", state.stratum));
    out
}
