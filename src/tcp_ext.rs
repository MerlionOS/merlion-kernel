/// TCP protocol extensions for MerlionOS.
/// Implements TCP Fast Open (TFO), Selective ACK (SACK),
/// Window Scaling, and Timestamps (RFC 7323).

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ===========================================================================
// TCP Extensions Configuration
// ===========================================================================

/// TCP extensions configuration.
pub struct TcpExtConfig {
    pub tfo_enabled: bool,
    pub tfo_mode: u8,           // 0=off, 1=client, 2=server, 3=both
    pub sack_enabled: bool,
    pub window_scaling: bool,
    pub timestamps_enabled: bool,
    pub default_window_scale: u8,
}

impl TcpExtConfig {
    const fn new() -> Self {
        Self {
            tfo_enabled: true,
            tfo_mode: 3,
            sack_enabled: true,
            window_scaling: true,
            timestamps_enabled: true,
            default_window_scale: 7,
        }
    }
}

static CONFIG: Mutex<TcpExtConfig> = Mutex::new(TcpExtConfig::new());

// ===========================================================================
// 1. TCP Fast Open (TFO)
// ===========================================================================

/// TCP Fast Open — allows data in SYN packets for 0-RTT connections.
/// Uses cookies to validate previously-seen clients.

/// Server secret for cookie generation (128-bit).
static TFO_SECRET: Mutex<[u8; 16]> = Mutex::new([
    0x4D, 0x65, 0x72, 0x6C, 0x69, 0x6F, 0x6E, 0x4F,
    0x53, 0x5F, 0x54, 0x46, 0x4F, 0x5F, 0x4B, 0x31,
]);

/// Maximum number of cached TFO cookies (client-side).
const TFO_CACHE_SIZE: usize = 64;

/// Cached TFO cookie entry: (client_ip, cookie).
struct TfoCacheEntry {
    ip: [u8; 4],
    cookie: [u8; 8],
    valid: bool,
}

impl TfoCacheEntry {
    const fn empty() -> Self {
        Self { ip: [0; 4], cookie: [0; 8], valid: false }
    }
}

static TFO_CACHE: Mutex<[TfoCacheEntry; TFO_CACHE_SIZE]> = Mutex::new(
    [const { TfoCacheEntry::empty() }; TFO_CACHE_SIZE]
);

/// TFO statistics.
struct TfoStats {
    cookies_generated: AtomicU64,
    cookies_validated: AtomicU64,
    cookies_invalid: AtomicU64,
    syn_data_sent: AtomicU64,
    syn_data_received: AtomicU64,
    fallbacks: AtomicU64,
}

static TFO_STATS: TfoStats = TfoStats {
    cookies_generated: AtomicU64::new(0),
    cookies_validated: AtomicU64::new(0),
    cookies_invalid: AtomicU64::new(0),
    syn_data_sent: AtomicU64::new(0),
    syn_data_received: AtomicU64::new(0),
    fallbacks: AtomicU64::new(0),
};

/// Simple HMAC-like hash: SipHash-inspired mixing of client_ip with server secret.
/// Returns an 8-byte cookie.
fn hmac_cookie(client_ip: &[u8; 4], secret: &[u8; 16]) -> [u8; 8] {
    let mut state: u64 = 0x736970_68617368; // "siphash" seed
    // Mix in secret
    for i in 0..16 {
        state = state.wrapping_mul(6364136223846793005)
            .wrapping_add(secret[i] as u64);
    }
    // Mix in client IP
    for i in 0..4 {
        state = state.wrapping_mul(6364136223846793005)
            .wrapping_add(client_ip[i] as u64);
        state ^= state >> 17;
    }
    // Final mix
    state ^= state >> 33;
    state = state.wrapping_mul(0xff51afd7ed558ccd);
    state ^= state >> 33;
    state.to_le_bytes()
}

/// Server generates a TFO cookie for a given client IP.
pub fn generate_cookie(client_ip: [u8; 4]) -> [u8; 8] {
    let secret = TFO_SECRET.lock();
    let cookie = hmac_cookie(&client_ip, &*secret);
    TFO_STATS.cookies_generated.fetch_add(1, Ordering::Relaxed);
    cookie
}

/// Server validates a TFO cookie from a client.
pub fn validate_cookie(client_ip: [u8; 4], cookie: &[u8; 8]) -> bool {
    let expected = generate_cookie(client_ip);
    // Constant-time comparison
    let mut diff: u8 = 0;
    for i in 0..8 {
        diff |= cookie[i] ^ expected[i];
    }
    if diff == 0 {
        TFO_STATS.cookies_validated.fetch_add(1, Ordering::Relaxed);
        true
    } else {
        TFO_STATS.cookies_invalid.fetch_add(1, Ordering::Relaxed);
        false
    }
}

/// Cache a TFO cookie for a destination (client-side).
fn cache_cookie(dst_ip: [u8; 4], cookie: [u8; 8]) {
    let mut cache = TFO_CACHE.lock();
    // Check if already cached; if so, update.
    for entry in cache.iter_mut() {
        if entry.valid && entry.ip == dst_ip {
            entry.cookie = cookie;
            return;
        }
    }
    // Find an empty slot or overwrite the first slot (simple eviction).
    for entry in cache.iter_mut() {
        if !entry.valid {
            entry.ip = dst_ip;
            entry.cookie = cookie;
            entry.valid = true;
            return;
        }
    }
    // Evict slot 0
    cache[0].ip = dst_ip;
    cache[0].cookie = cookie;
    cache[0].valid = true;
}

/// Look up a cached TFO cookie for a destination (client-side).
fn lookup_cookie(dst_ip: [u8; 4]) -> Option<[u8; 8]> {
    let cache = TFO_CACHE.lock();
    for entry in cache.iter() {
        if entry.valid && entry.ip == dst_ip {
            return Some(entry.cookie);
        }
    }
    None
}

/// TFO connect result.
pub struct TfoConnectResult {
    pub syn_with_data: bool,
    pub cookie_used: bool,
    pub data_len: usize,
}

/// Client-side TFO connect: sends SYN + data + cookie if available.
/// Returns result indicating whether TFO was used or fell back to normal SYN.
pub fn tfo_connect(dst_ip: [u8; 4], _dst_port: u16, data: &[u8]) -> Result<TfoConnectResult, &'static str> {
    let cfg = CONFIG.lock();
    if !cfg.tfo_enabled || (cfg.tfo_mode & 1) == 0 {
        return Err("TFO client mode disabled");
    }
    drop(cfg);

    if let Some(cookie) = lookup_cookie(dst_ip) {
        // We have a cached cookie — send SYN+data+cookie
        TFO_STATS.syn_data_sent.fetch_add(1, Ordering::Relaxed);
        let _ = cookie; // Would be included in TCP option kind=34
        Ok(TfoConnectResult {
            syn_with_data: true,
            cookie_used: true,
            data_len: data.len(),
        })
    } else {
        // No cookie — do a normal SYN but request a cookie (empty TFO option)
        TFO_STATS.fallbacks.fetch_add(1, Ordering::Relaxed);
        Ok(TfoConnectResult {
            syn_with_data: false,
            cookie_used: false,
            data_len: 0,
        })
    }
}

/// Server-side TFO accept: validates cookie and accepts SYN+data.
pub fn tfo_accept(client_ip: [u8; 4], cookie: &[u8; 8], data: &[u8]) -> Result<usize, &'static str> {
    let cfg = CONFIG.lock();
    if !cfg.tfo_enabled || (cfg.tfo_mode & 2) == 0 {
        return Err("TFO server mode disabled");
    }
    drop(cfg);

    if validate_cookie(client_ip, cookie) {
        TFO_STATS.syn_data_received.fetch_add(1, Ordering::Relaxed);
        Ok(data.len())
    } else {
        TFO_STATS.fallbacks.fetch_add(1, Ordering::Relaxed);
        Err("TFO cookie invalid, falling back to 3-way handshake")
    }
}

/// Handle incoming SYN-ACK with TFO cookie (client caches it for next time).
pub fn tfo_handle_synack_cookie(server_ip: [u8; 4], cookie: [u8; 8]) {
    cache_cookie(server_ip, cookie);
}

/// Return TFO statistics as a formatted string.
pub fn tfo_stats() -> String {
    format!(
        "TCP Fast Open (TFO)\n\
         \x20 cookies generated:  {}\n\
         \x20 cookies validated:  {}\n\
         \x20 cookies invalid:    {}\n\
         \x20 SYN+data sent:      {}\n\
         \x20 SYN+data received:  {}\n\
         \x20 fallbacks:          {}\n",
        TFO_STATS.cookies_generated.load(Ordering::Relaxed),
        TFO_STATS.cookies_validated.load(Ordering::Relaxed),
        TFO_STATS.cookies_invalid.load(Ordering::Relaxed),
        TFO_STATS.syn_data_sent.load(Ordering::Relaxed),
        TFO_STATS.syn_data_received.load(Ordering::Relaxed),
        TFO_STATS.fallbacks.load(Ordering::Relaxed),
    )
}

// ===========================================================================
// 2. TCP SACK (Selective ACK)
// ===========================================================================

/// Selective Acknowledgment — allows receiver to inform sender about
/// non-contiguous received segments, avoiding unnecessary retransmissions.

/// Maximum SACK blocks tracked per connection.
const MAX_SACK_BLOCKS: usize = 32;

/// Maximum SACK blocks in a single TCP option (limited by option space).
const MAX_SACK_OPTION_BLOCKS: usize = 4;

/// TCP option kind for SACK permitted (SYN/SYN-ACK).
const TCP_OPT_SACK_PERMITTED: u8 = 4;

/// TCP option kind for SACK data.
const TCP_OPT_SACK: u8 = 5;

/// SACK scoreboard: tracks non-contiguous received segments.
pub struct SackScoreboard {
    blocks: Vec<(u32, u32)>,
}

impl SackScoreboard {
    /// Create a new empty scoreboard.
    pub fn new() -> Self {
        Self { blocks: Vec::new() }
    }

    /// Record a received segment [seq, seq+len). Merges adjacent/overlapping blocks.
    pub fn record_segment(&mut self, seq: u32, len: u32) {
        if len == 0 {
            return;
        }
        let right = seq.wrapping_add(len);

        // Try to merge with existing blocks
        let mut new_left = seq;
        let mut new_right = right;
        let mut merged = false;

        // Remove all overlapping/adjacent blocks and merge into one
        self.blocks.retain(|&(l, r)| {
            // Check overlap or adjacency (with wrapping arithmetic)
            if Self::overlaps_or_adjacent(new_left, new_right, l, r) {
                new_left = Self::seq_min(new_left, l);
                new_right = Self::seq_max(new_right, r);
                merged = true;
                false // remove this block, will be replaced by merged
            } else {
                true
            }
        });

        let _ = merged;
        self.blocks.push((new_left, new_right));

        // Sort by left edge
        self.blocks.sort_by(|a, b| a.0.cmp(&b.0));

        // Limit to MAX_SACK_BLOCKS
        while self.blocks.len() > MAX_SACK_BLOCKS {
            self.blocks.remove(0);
        }
    }

    /// Check if two ranges overlap or are adjacent.
    fn overlaps_or_adjacent(l1: u32, r1: u32, l2: u32, r2: u32) -> bool {
        // They overlap or are adjacent if neither is entirely before the other.
        !(r1 < l2 || r2 < l1)
    }

    /// Return the smaller sequence number (simple comparison, no wrapping).
    fn seq_min(a: u32, b: u32) -> u32 {
        if a <= b { a } else { b }
    }

    /// Return the larger sequence number.
    fn seq_max(a: u32, b: u32) -> u32 {
        if a >= b { a } else { b }
    }

    /// Get the top SACK blocks for inclusion in a TCP SACK option (max 4).
    /// Returns most recently added blocks first.
    pub fn get_sack_blocks(&self) -> Vec<(u32, u32)> {
        let len = self.blocks.len();
        if len <= MAX_SACK_OPTION_BLOCKS {
            return self.blocks.clone();
        }
        self.blocks[len - MAX_SACK_OPTION_BLOCKS..].to_vec()
    }

    /// Check if a sequence number is covered by any SACK block.
    pub fn is_sacked(&self, seq: u32) -> bool {
        for &(left, right) in &self.blocks {
            if seq >= left && seq < right {
                return true;
            }
        }
        false
    }

    /// Return number of tracked blocks.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Clear all blocks.
    pub fn clear(&mut self) {
        self.blocks.clear();
    }
}

/// Parse SACK blocks from TCP option data (kind=5).
/// Input: option data after kind and length bytes.
/// Each block is 8 bytes: 4-byte left_edge + 4-byte right_edge.
pub fn parse_sack_blocks(data: &[u8]) -> Vec<(u32, u32)> {
    let mut blocks = Vec::new();
    let mut i = 0;
    while i + 8 <= data.len() {
        let left = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        let right = u32::from_be_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]);
        blocks.push((left, right));
        i += 8;
    }
    blocks
}

/// Build SACK option bytes (kind=5, len, block pairs).
/// Returns the complete option including kind and length.
pub fn build_sack_option(blocks: &[(u32, u32)]) -> Vec<u8> {
    let count = core::cmp::min(blocks.len(), MAX_SACK_OPTION_BLOCKS);
    if count == 0 {
        return Vec::new();
    }
    let len = 2 + count * 8; // kind + len + blocks
    let mut out = Vec::with_capacity(len);
    out.push(TCP_OPT_SACK);
    out.push(len as u8);
    for i in 0..count {
        out.extend_from_slice(&blocks[i].0.to_be_bytes());
        out.extend_from_slice(&blocks[i].1.to_be_bytes());
    }
    out
}

/// Build SACK Permitted option (kind=4, len=2) — sent in SYN/SYN-ACK.
pub fn build_sack_permitted_option() -> [u8; 2] {
    [TCP_OPT_SACK_PERMITTED, 2]
}

/// Sender-side SACK tracking: marks segments as SACKed and finds retransmit candidates.
pub struct SackSender {
    /// Segments sent: (seq, len, sacked).
    segments: Vec<(u32, u32, bool)>,
    /// Number of SACK blocks received.
    blocks_received: u64,
    /// Retransmissions avoided by SACK.
    retransmits_avoided: u64,
}

impl SackSender {
    pub fn new() -> Self {
        Self {
            segments: Vec::new(),
            blocks_received: 0,
            retransmits_avoided: 0,
        }
    }

    /// Register a sent segment.
    pub fn add_segment(&mut self, seq: u32, len: u32) {
        self.segments.push((seq, len, false));
        // Limit tracked segments
        while self.segments.len() > 256 {
            self.segments.remove(0);
        }
    }

    /// Mark segments as SACKed based on received SACK blocks.
    pub fn mark_sacked(&mut self, blocks: &[(u32, u32)]) {
        self.blocks_received += blocks.len() as u64;
        for seg in self.segments.iter_mut() {
            if seg.2 {
                continue; // already sacked
            }
            let seg_end = seg.0.wrapping_add(seg.1);
            for &(left, right) in blocks {
                if seg.0 >= left && seg_end <= right {
                    seg.2 = true;
                    self.retransmits_avoided += 1;
                    break;
                }
            }
        }
    }

    /// Get sequence numbers of segments NOT SACKed (retransmit candidates).
    pub fn get_retransmit_candidates(&self) -> Vec<u32> {
        let mut candidates = Vec::new();
        for &(seq, _len, sacked) in &self.segments {
            if !sacked {
                candidates.push(seq);
            }
        }
        candidates
    }

    /// Remove all segments up to (and including) the given ACK number.
    pub fn ack_segments(&mut self, ack: u32) {
        self.segments.retain(|&(seq, _len, _)| seq >= ack);
    }

    /// SACK-based loss detection: after 3+ dupacks with SACK info,
    /// segments not covered by any SACK block are considered lost.
    pub fn sack_based_loss_detection(&self, dup_ack_count: u32) -> Vec<u32> {
        if dup_ack_count < 3 {
            return Vec::new();
        }
        self.get_retransmit_candidates()
    }
}

/// SACK statistics.
struct SackStats {
    blocks_sent: AtomicU64,
    blocks_received: AtomicU64,
    retransmits_avoided: AtomicU64,
    sack_connections: AtomicU64,
}

static SACK_STATS: SackStats = SackStats {
    blocks_sent: AtomicU64::new(0),
    blocks_received: AtomicU64::new(0),
    retransmits_avoided: AtomicU64::new(0),
    sack_connections: AtomicU64::new(0),
};

/// Return SACK statistics as a formatted string.
pub fn sack_stats() -> String {
    format!(
        "TCP Selective ACK (SACK)\n\
         \x20 SACK blocks sent:      {}\n\
         \x20 SACK blocks received:  {}\n\
         \x20 retransmits avoided:   {}\n\
         \x20 SACK connections:      {}\n",
        SACK_STATS.blocks_sent.load(Ordering::Relaxed),
        SACK_STATS.blocks_received.load(Ordering::Relaxed),
        SACK_STATS.retransmits_avoided.load(Ordering::Relaxed),
        SACK_STATS.sack_connections.load(Ordering::Relaxed),
    )
}

// ===========================================================================
// 3. TCP Window Scaling
// ===========================================================================

/// Window Scaling — extends the 16-bit window field to support
/// windows larger than 64KB (up to 1GB).

/// TCP option kind for Window Scale.
const TCP_OPT_WINDOW_SCALE: u8 = 3;

/// Maximum window scale factor (RFC 7323: max 14).
const MAX_WINDOW_SCALE: u8 = 14;

/// Per-connection window scale state.
pub struct WindowScaleState {
    pub send_scale: u8,
    pub recv_scale: u8,
    pub scale_negotiated: bool,
}

impl WindowScaleState {
    pub fn new() -> Self {
        Self {
            send_scale: 0,
            recv_scale: 0,
            scale_negotiated: false,
        }
    }

    /// Set negotiated scale factors after SYN exchange.
    pub fn set_negotiated(&mut self, send_scale: u8, recv_scale: u8) {
        self.send_scale = core::cmp::min(send_scale, MAX_WINDOW_SCALE);
        self.recv_scale = core::cmp::min(recv_scale, MAX_WINDOW_SCALE);
        self.scale_negotiated = true;
    }
}

/// Propose a window scale factor during SYN. Returns our desired scale.
pub fn negotiate_window_scale(our_scale: u8) -> u8 {
    core::cmp::min(our_scale, MAX_WINDOW_SCALE)
}

/// Parse Window Scale option from TCP option data.
/// Input: 3 bytes [kind=3, len=3, shift_count].
pub fn parse_window_scale_option(data: &[u8]) -> Option<u8> {
    if data.len() >= 3 && data[0] == TCP_OPT_WINDOW_SCALE && data[1] == 3 {
        let scale = core::cmp::min(data[2], MAX_WINDOW_SCALE);
        Some(scale)
    } else if data.len() >= 1 {
        // Just the shift count (after kind/len already parsed)
        Some(core::cmp::min(data[0], MAX_WINDOW_SCALE))
    } else {
        None
    }
}

/// Build Window Scale option: [kind=3, len=3, shift_count].
pub fn build_window_scale_option(scale: u8) -> [u8; 3] {
    [TCP_OPT_WINDOW_SCALE, 3, core::cmp::min(scale, MAX_WINDOW_SCALE)]
}

/// Calculate the effective window size from the advertised 16-bit value and scale factor.
pub fn effective_window(advertised: u16, scale: u8) -> u32 {
    (advertised as u32) << core::cmp::min(scale, MAX_WINDOW_SCALE)
}

/// Auto-tune window scale based on desired receive buffer size.
/// Returns the smallest scale factor that can represent the buffer.
pub fn auto_scale(buffer_size: usize) -> u8 {
    // Window = advertised << scale, max advertised = 65535
    // We need: buffer_size <= 65535 << scale
    let mut scale: u8 = 0;
    while scale < MAX_WINDOW_SCALE {
        let max_window = 65535u64 << (scale as u64);
        if max_window >= buffer_size as u64 {
            return scale;
        }
        scale += 1;
    }
    MAX_WINDOW_SCALE
}

/// Window scale statistics.
struct WscaleStats {
    negotiations: AtomicU64,
    max_scale_seen: AtomicU64,
}

static WSCALE_STATS: WscaleStats = WscaleStats {
    negotiations: AtomicU64::new(0),
    max_scale_seen: AtomicU64::new(0),
};

/// Record a window scale negotiation.
pub fn record_wscale_negotiation(scale: u8) {
    WSCALE_STATS.negotiations.fetch_add(1, Ordering::Relaxed);
    let prev = WSCALE_STATS.max_scale_seen.load(Ordering::Relaxed);
    if (scale as u64) > prev {
        WSCALE_STATS.max_scale_seen.store(scale as u64, Ordering::Relaxed);
    }
}

// ===========================================================================
// 4. TCP Timestamps
// ===========================================================================

/// TCP Timestamps — provides RTT measurement and
/// Protection Against Wrapped Sequences (PAWS).

/// TCP option kind for Timestamps.
const TCP_OPT_TIMESTAMP: u8 = 8;

/// Monotonic timestamp counter (milliseconds).
static TS_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Timestamp statistics.
struct TsStats {
    rtt_measurements: AtomicU64,
    paws_rejects: AtomicU64,
    total_rtt_ms: AtomicU64,
    min_rtt_ms: AtomicU64,
    max_rtt_ms: AtomicU64,
}

static TS_STATS: TsStats = TsStats {
    rtt_measurements: AtomicU64::new(0),
    paws_rejects: AtomicU64::new(0),
    total_rtt_ms: AtomicU64::new(0),
    min_rtt_ms: AtomicU64::new(u64::MAX),
    max_rtt_ms: AtomicU64::new(0),
};

/// Build TCP Timestamp option: [kind=8, len=10, TSval(4), TSecr(4)].
/// Returns 12 bytes: kind + len + 4-byte TSval + 4-byte TSecr + 2 NOP padding.
pub fn build_timestamp_option(tsval: u32, tsecr: u32) -> [u8; 12] {
    let mut out = [0u8; 12];
    // NOP NOP before timestamp for alignment (common practice)
    out[0] = 1; // NOP
    out[1] = 1; // NOP
    out[2] = TCP_OPT_TIMESTAMP;
    out[3] = 10;
    out[4..8].copy_from_slice(&tsval.to_be_bytes());
    out[8..12].copy_from_slice(&tsecr.to_be_bytes());
    out
}

/// Parse TCP Timestamp option. Returns (TSval, TSecr) if valid.
/// Input: option data starting at kind byte, at least 10 bytes.
pub fn parse_timestamp_option(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() >= 10 && data[0] == TCP_OPT_TIMESTAMP && data[1] == 10 {
        let tsval = u32::from_be_bytes([data[2], data[3], data[4], data[5]]);
        let tsecr = u32::from_be_bytes([data[6], data[7], data[8], data[9]]);
        Some((tsval, tsecr))
    } else if data.len() >= 8 {
        // Data after kind/len already parsed
        let tsval = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        let tsecr = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        Some((tsval, tsecr))
    } else {
        None
    }
}

/// Measure RTT using timestamps.
/// sent_tsval: the TSval we originally sent (echoed back as TSecr).
/// received_tsecr: the TSecr field from the received segment.
/// current_ts: current local timestamp.
/// Returns RTT in milliseconds.
pub fn measure_rtt(sent_tsval: u32, _received_tsecr: u32, current_ts: u32) -> u32 {
    let rtt = current_ts.wrapping_sub(sent_tsval);
    TS_STATS.rtt_measurements.fetch_add(1, Ordering::Relaxed);
    TS_STATS.total_rtt_ms.fetch_add(rtt as u64, Ordering::Relaxed);

    // Update min RTT
    let prev_min = TS_STATS.min_rtt_ms.load(Ordering::Relaxed);
    if (rtt as u64) < prev_min {
        TS_STATS.min_rtt_ms.store(rtt as u64, Ordering::Relaxed);
    }

    // Update max RTT
    let prev_max = TS_STATS.max_rtt_ms.load(Ordering::Relaxed);
    if (rtt as u64) > prev_max {
        TS_STATS.max_rtt_ms.store(rtt as u64, Ordering::Relaxed);
    }

    rtt
}

/// PAWS (Protection Against Wrapped Sequences) check.
/// Returns true if the received timestamp is acceptable (not old duplicate).
/// received_tsval: TSval from the incoming segment.
/// last_tsval: the most recent TSval we've seen from this peer.
pub fn paws_check(received_tsval: u32, last_tsval: u32) -> bool {
    // A segment is acceptable if its TSval >= last seen TSval.
    // Use signed comparison to handle wraparound:
    // If received - last < 0 (as signed i32), the timestamp has gone backward.
    let diff = received_tsval.wrapping_sub(last_tsval) as i32;
    if diff < 0 {
        TS_STATS.paws_rejects.fetch_add(1, Ordering::Relaxed);
        false
    } else {
        true
    }
}

/// Get the current TCP timestamp (monotonic, millisecond granularity).
/// Uses the kernel timer tick count converted to milliseconds.
pub fn tcp_timestamp() -> u32 {
    // Use kernel uptime in ms as timestamp source
    let ticks = crate::timer::ticks();
    // PIT runs at 100 Hz, so each tick = 10ms
    let ms = ticks * 10;
    // Also advance our counter
    TS_COUNTER.store(ms, Ordering::Relaxed);
    ms as u32
}

/// Return timestamp statistics as a formatted string.
pub fn timestamp_stats() -> String {
    let measurements = TS_STATS.rtt_measurements.load(Ordering::Relaxed);
    let paws_rejects = TS_STATS.paws_rejects.load(Ordering::Relaxed);
    let total_rtt = TS_STATS.total_rtt_ms.load(Ordering::Relaxed);
    let min_rtt = TS_STATS.min_rtt_ms.load(Ordering::Relaxed);
    let max_rtt = TS_STATS.max_rtt_ms.load(Ordering::Relaxed);

    let avg_rtt = if measurements > 0 {
        total_rtt / measurements
    } else {
        0
    };

    let min_display = if min_rtt == u64::MAX { 0 } else { min_rtt };

    format!(
        "TCP Timestamps (RFC 7323)\n\
         \x20 RTT measurements:  {}\n\
         \x20 avg RTT:           {} ms\n\
         \x20 min RTT:           {} ms\n\
         \x20 max RTT:           {} ms\n\
         \x20 PAWS rejects:      {}\n\
         \x20 current timestamp: {}\n",
        measurements, avg_rtt, min_display, max_rtt, paws_rejects,
        tcp_timestamp(),
    )
}

// ===========================================================================
// 5. TCP Options Framework
// ===========================================================================

/// Unified TCP options structure parsed from/built into option bytes.
pub struct TcpOptions {
    pub mss: Option<u16>,
    pub window_scale: Option<u8>,
    pub sack_permitted: bool,
    pub sack_blocks: Vec<(u32, u32)>,
    pub timestamps: Option<(u32, u32)>,
    pub tfo_cookie: Option<Vec<u8>>,
}

impl TcpOptions {
    pub fn new() -> Self {
        Self {
            mss: None,
            window_scale: None,
            sack_permitted: false,
            sack_blocks: Vec::new(),
            timestamps: None,
            tfo_cookie: None,
        }
    }
}

/// TCP option kinds.
const TCP_OPT_END: u8 = 0;
const TCP_OPT_NOP: u8 = 1;
const TCP_OPT_MSS: u8 = 2;
// kind 3 = window scale (defined above)
// kind 4 = SACK permitted (defined above)
// kind 5 = SACK (defined above)
// kind 8 = timestamps (defined above)
const TCP_OPT_TFO: u8 = 34;

/// Parse all TCP options from raw option bytes.
pub fn parse_tcp_options(data: &[u8]) -> TcpOptions {
    let mut opts = TcpOptions::new();
    let mut i = 0;

    while i < data.len() {
        let kind = data[i];
        match kind {
            TCP_OPT_END => break,
            TCP_OPT_NOP => {
                i += 1;
                continue;
            }
            _ => {
                if i + 1 >= data.len() {
                    break;
                }
                let len = data[i + 1] as usize;
                if len < 2 || i + len > data.len() {
                    break;
                }

                match kind {
                    TCP_OPT_MSS => {
                        if len == 4 && i + 4 <= data.len() {
                            let mss = u16::from_be_bytes([data[i + 2], data[i + 3]]);
                            opts.mss = Some(mss);
                        }
                    }
                    TCP_OPT_WINDOW_SCALE => {
                        if len == 3 && i + 3 <= data.len() {
                            opts.window_scale = Some(
                                core::cmp::min(data[i + 2], MAX_WINDOW_SCALE)
                            );
                        }
                    }
                    TCP_OPT_SACK_PERMITTED => {
                        opts.sack_permitted = true;
                    }
                    TCP_OPT_SACK => {
                        let block_data = &data[i + 2..i + len];
                        opts.sack_blocks = parse_sack_blocks(block_data);
                    }
                    TCP_OPT_TIMESTAMP => {
                        if len == 10 && i + 10 <= data.len() {
                            let tsval = u32::from_be_bytes([
                                data[i + 2], data[i + 3], data[i + 4], data[i + 5],
                            ]);
                            let tsecr = u32::from_be_bytes([
                                data[i + 6], data[i + 7], data[i + 8], data[i + 9],
                            ]);
                            opts.timestamps = Some((tsval, tsecr));
                        }
                    }
                    TCP_OPT_TFO => {
                        if len > 2 {
                            opts.tfo_cookie = Some(data[i + 2..i + len].to_vec());
                        } else {
                            // Empty TFO option = cookie request
                            opts.tfo_cookie = Some(Vec::new());
                        }
                    }
                    _ => {} // Unknown option, skip
                }

                i += len;
                continue;
            }
        }
    }

    opts
}

/// Build TCP options into raw bytes.
pub fn build_tcp_options(opts: &TcpOptions) -> Vec<u8> {
    let mut out = Vec::new();

    // MSS
    if let Some(mss) = opts.mss {
        out.push(TCP_OPT_MSS);
        out.push(4);
        out.extend_from_slice(&mss.to_be_bytes());
    }

    // Window Scale
    if let Some(scale) = opts.window_scale {
        out.push(1); // NOP for alignment
        let ws = build_window_scale_option(scale);
        out.extend_from_slice(&ws);
    }

    // SACK Permitted
    if opts.sack_permitted {
        let sp = build_sack_permitted_option();
        out.extend_from_slice(&sp);
    }

    // SACK blocks
    if !opts.sack_blocks.is_empty() {
        let sack = build_sack_option(&opts.sack_blocks);
        out.extend_from_slice(&sack);
    }

    // Timestamps
    if let Some((tsval, tsecr)) = opts.timestamps {
        let ts = build_timestamp_option(tsval, tsecr);
        out.extend_from_slice(&ts);
    }

    // TFO cookie
    if let Some(ref cookie) = opts.tfo_cookie {
        out.push(TCP_OPT_TFO);
        out.push((2 + cookie.len()) as u8);
        out.extend_from_slice(cookie);
    }

    // Pad to 4-byte boundary with NOPs, then END
    while out.len() % 4 != 0 {
        out.push(TCP_OPT_NOP);
    }

    out
}

// ===========================================================================
// 6. Global API
// ===========================================================================

/// Initialise all TCP extensions.
pub fn init() {
    // Reset statistics
    TFO_STATS.cookies_generated.store(0, Ordering::Relaxed);
    TFO_STATS.cookies_validated.store(0, Ordering::Relaxed);
    TFO_STATS.cookies_invalid.store(0, Ordering::Relaxed);
    TFO_STATS.syn_data_sent.store(0, Ordering::Relaxed);
    TFO_STATS.syn_data_received.store(0, Ordering::Relaxed);
    TFO_STATS.fallbacks.store(0, Ordering::Relaxed);

    SACK_STATS.blocks_sent.store(0, Ordering::Relaxed);
    SACK_STATS.blocks_received.store(0, Ordering::Relaxed);
    SACK_STATS.retransmits_avoided.store(0, Ordering::Relaxed);
    SACK_STATS.sack_connections.store(0, Ordering::Relaxed);

    WSCALE_STATS.negotiations.store(0, Ordering::Relaxed);
    WSCALE_STATS.max_scale_seen.store(0, Ordering::Relaxed);

    TS_STATS.rtt_measurements.store(0, Ordering::Relaxed);
    TS_STATS.paws_rejects.store(0, Ordering::Relaxed);
    TS_STATS.total_rtt_ms.store(0, Ordering::Relaxed);
    TS_STATS.min_rtt_ms.store(u64::MAX, Ordering::Relaxed);
    TS_STATS.max_rtt_ms.store(0, Ordering::Relaxed);

    // Clear TFO cache
    let mut cache = TFO_CACHE.lock();
    for entry in cache.iter_mut() {
        entry.valid = false;
    }
    drop(cache);
}

/// Show all TCP extension status/info.
pub fn tcp_ext_info() -> String {
    let cfg = CONFIG.lock();
    let tfo_mode_str = match cfg.tfo_mode {
        0 => "disabled",
        1 => "client-only",
        2 => "server-only",
        3 => "client+server",
        _ => "unknown",
    };

    format!(
        "TCP Protocol Extensions\n\
         ═══════════════════════\n\
         TCP Fast Open (TFO)\n\
         \x20 enabled:       {}\n\
         \x20 mode:          {} ({})\n\
         \n\
         TCP Selective ACK (SACK)\n\
         \x20 enabled:       {}\n\
         \n\
         TCP Window Scaling\n\
         \x20 enabled:       {}\n\
         \x20 default scale: {} (max window: {} KB)\n\
         \n\
         TCP Timestamps (RFC 7323)\n\
         \x20 enabled:       {}\n\
         \x20 current ts:    {}\n",
        cfg.tfo_enabled,
        cfg.tfo_mode, tfo_mode_str,
        cfg.sack_enabled,
        cfg.window_scaling,
        cfg.default_window_scale,
        (65535u64 << cfg.default_window_scale as u64) / 1024,
        cfg.timestamps_enabled,
        tcp_timestamp(),
    )
}

/// Combined statistics from all TCP extensions.
pub fn tcp_ext_stats() -> String {
    let mut out = String::new();
    out.push_str(&tfo_stats());
    out.push('\n');
    out.push_str(&sack_stats());
    out.push('\n');

    // Window scaling stats
    let negotiations = WSCALE_STATS.negotiations.load(Ordering::Relaxed);
    let max_scale = WSCALE_STATS.max_scale_seen.load(Ordering::Relaxed);
    out.push_str(&format!(
        "TCP Window Scaling\n\
         \x20 negotiations:   {}\n\
         \x20 max scale seen: {}\n\n",
        negotiations, max_scale,
    ));

    out.push_str(&timestamp_stats());
    out
}

/// Set TCP extensions configuration.
pub fn set_config(config: TcpExtConfig) {
    let mut cfg = CONFIG.lock();
    cfg.tfo_enabled = config.tfo_enabled;
    cfg.tfo_mode = config.tfo_mode;
    cfg.sack_enabled = config.sack_enabled;
    cfg.window_scaling = config.window_scaling;
    cfg.timestamps_enabled = config.timestamps_enabled;
    cfg.default_window_scale = core::cmp::min(config.default_window_scale, MAX_WINDOW_SCALE);
}

/// Get a copy of the current TCP extensions configuration.
pub fn get_config() -> TcpExtConfig {
    let cfg = CONFIG.lock();
    TcpExtConfig {
        tfo_enabled: cfg.tfo_enabled,
        tfo_mode: cfg.tfo_mode,
        sack_enabled: cfg.sack_enabled,
        window_scaling: cfg.window_scaling,
        timestamps_enabled: cfg.timestamps_enabled,
        default_window_scale: cfg.default_window_scale,
    }
}
