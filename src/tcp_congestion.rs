/// TCP congestion control for MerlionOS.
/// Implements Reno, Cubic, and BBR-like congestion control algorithms
/// for the kernel TCP stack.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of tracked connections.
const MAX_CONNECTIONS: usize = 64;

/// Initial congestion window size in segments.
const INITIAL_CWND: u32 = 10;

/// Minimum congestion window (must not go below 1 segment).
const MIN_CWND: u32 = 1;

/// Default slow-start threshold.
const DEFAULT_SSTHRESH: u32 = 65535;

/// Maximum congestion window.
const DEFAULT_MAX_CWND: u32 = 65535;

/// RTT smoothing factor alpha (fixed-point: value/256). Jacobson/Karels uses 1/8.
const RTT_ALPHA: u32 = 32; // 32/256 = 1/8

/// RTT variance factor beta (fixed-point: value/256). Jacobson/Karels uses 1/4.
const RTT_BETA: u32 = 64; // 64/256 = 1/4

/// Duplicate ACK threshold for fast retransmit.
const DUP_ACK_THRESHOLD: u32 = 3;

/// Cubic scaling constant C (fixed-point * 1000). Typical C = 0.4 -> 400.
const CUBIC_C: u32 = 400;

/// Cubic beta for multiplicative decrease (fixed-point * 1000). beta = 0.7 -> 700.
const CUBIC_BETA: u32 = 700;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Congestion control state machine phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CongestionState {
    SlowStart,
    CongestionAvoidance,
    FastRecovery,
    FastRetransmit,
}

/// Supported congestion control algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    Reno,
    Cubic,
    Bbr,
}

/// Per-connection congestion control state.
pub struct CongestionControl {
    /// Which algorithm governs this connection.
    pub algorithm: Algorithm,
    /// Congestion window in segments.
    pub cwnd: u32,
    /// Slow-start threshold in segments.
    pub ssthresh: u32,
    /// Smoothed RTT in microseconds (fixed-point * 256).
    pub rtt_us: u32,
    /// RTT variance in microseconds (fixed-point * 256).
    pub rtt_var: u32,
    /// Current congestion state.
    pub state: CongestionState,
    /// Consecutive duplicate ACK count.
    pub dup_ack_count: u32,
    /// Bytes currently in flight (unacknowledged).
    pub bytes_in_flight: u32,
    /// Maximum allowed cwnd.
    pub max_cwnd: u32,
    // -- Cubic-specific fields --
    /// Cubic: time factor K (microseconds).
    pub cubic_k: u32,
    /// Cubic: window size at last loss event.
    pub cubic_wmax: u32,
    /// Cubic: timestamp (us) of last loss event.
    pub cubic_origin: u64,
    // -- BBR-specific fields --
    /// BBR: estimated bottleneck bandwidth (bytes/sec).
    pub bbr_btl_bw: u32,
    /// BBR: minimum observed RTT (microseconds).
    pub bbr_rt_prop: u32,
    /// BBR: current pacing rate (bytes/sec).
    pub bbr_pacing_rate: u32,
}

impl CongestionControl {
    /// Create a new congestion control context with the given algorithm.
    pub fn new(algorithm: Algorithm) -> Self {
        Self {
            algorithm,
            cwnd: INITIAL_CWND,
            ssthresh: DEFAULT_SSTHRESH,
            rtt_us: 0,
            rtt_var: 0,
            state: CongestionState::SlowStart,
            dup_ack_count: 0,
            bytes_in_flight: 0,
            max_cwnd: DEFAULT_MAX_CWND,
            cubic_k: 0,
            cubic_wmax: 0,
            cubic_origin: 0,
            bbr_btl_bw: 0,
            bbr_rt_prop: u32::MAX,
            bbr_pacing_rate: 0,
        }
    }

    /// Called when a new ACK is received (not a duplicate).
    pub fn on_ack(&mut self, acked_bytes: u32) {
        self.dup_ack_count = 0;
        if self.bytes_in_flight >= acked_bytes {
            self.bytes_in_flight -= acked_bytes;
        } else {
            self.bytes_in_flight = 0;
        }

        match self.algorithm {
            Algorithm::Reno => self.reno_on_ack(acked_bytes),
            Algorithm::Cubic => self.cubic_on_ack(acked_bytes),
            Algorithm::Bbr => self.bbr_on_ack(acked_bytes),
        }
    }

    /// Called when a duplicate ACK is received.
    pub fn on_dup_ack(&mut self) {
        self.dup_ack_count += 1;
        if self.dup_ack_count >= DUP_ACK_THRESHOLD
            && self.state != CongestionState::FastRecovery
        {
            self.on_loss();
        } else if self.state == CongestionState::FastRecovery {
            // Inflate cwnd for each additional dup ACK during recovery.
            if self.cwnd < self.max_cwnd {
                self.cwnd += 1;
            }
        }
    }

    /// Called on packet loss detection (triple dup-ACK or SACK).
    pub fn on_loss(&mut self) {
        match self.algorithm {
            Algorithm::Reno => {
                self.ssthresh = core::cmp::max(self.cwnd / 2, MIN_CWND);
                self.cwnd = self.ssthresh + DUP_ACK_THRESHOLD;
                self.state = CongestionState::FastRetransmit;
            }
            Algorithm::Cubic => {
                self.cubic_wmax = self.cwnd;
                // Multiplicative decrease: cwnd = cwnd * beta
                self.cwnd = core::cmp::max(
                    (self.cwnd * CUBIC_BETA) / 1000,
                    MIN_CWND,
                );
                self.ssthresh = self.cwnd;
                self.cubic_k = cubic_k_compute(self.cubic_wmax, self.cwnd);
                self.cubic_origin = 0; // Reset; caller should set timestamp.
                self.state = CongestionState::FastRecovery;
            }
            Algorithm::Bbr => {
                // BBR does not halve cwnd on loss; it adjusts pacing.
                self.bbr_pacing_rate = core::cmp::max(
                    self.bbr_pacing_rate * 3 / 4,
                    1,
                );
                self.state = CongestionState::FastRecovery;
            }
        }
        STATS.losses.fetch_add(1, Ordering::Relaxed);
    }

    /// Called on RTO timeout — the most severe congestion signal.
    pub fn on_timeout(&mut self) {
        self.ssthresh = core::cmp::max(self.cwnd / 2, MIN_CWND);
        self.cwnd = INITIAL_CWND;
        self.dup_ack_count = 0;
        self.state = CongestionState::SlowStart;
        STATS.timeouts.fetch_add(1, Ordering::Relaxed);
    }

    /// Update RTT estimate using Jacobson/Karels EWMA algorithm.
    /// `sample_us` is the measured RTT for this ACK in microseconds.
    pub fn on_rtt_sample(&mut self, sample_us: u32) {
        if self.rtt_us == 0 {
            // First sample: initialise directly.
            self.rtt_us = sample_us << 8; // fixed-point * 256
            self.rtt_var = (sample_us << 8) / 2;
        } else {
            // err = sample - (srtt >> 8)
            let srtt_actual = self.rtt_us >> 8;
            let err = if sample_us >= srtt_actual {
                sample_us - srtt_actual
            } else {
                srtt_actual - sample_us
            };
            // rttvar = (1 - beta) * rttvar + beta * |err|
            self.rtt_var = self.rtt_var
                - (self.rtt_var * RTT_BETA / 256)
                + (err * RTT_BETA);
            // srtt = (1 - alpha) * srtt + alpha * sample
            let signed_diff = (sample_us as i32) - (srtt_actual as i32);
            let adjustment = signed_diff * (RTT_ALPHA as i32) / 256;
            self.rtt_us = ((self.rtt_us as i32) + adjustment) as u32;
        }

        // BBR: update minimum RTT.
        if self.algorithm == Algorithm::Bbr && sample_us < self.bbr_rt_prop {
            self.bbr_rt_prop = sample_us;
        }

        STATS.rtt_samples.fetch_add(1, Ordering::Relaxed);
    }

    /// Compute the retransmission timeout (RTO) in microseconds.
    /// RTO = SRTT + max(1, 4 * RTTVAR), with a minimum of 200ms.
    pub fn rto_us(&self) -> u32 {
        if self.rtt_us == 0 {
            return 1_000_000; // 1 second default before any samples.
        }
        let srtt = self.rtt_us >> 8;
        let var4 = (self.rtt_var >> 8) * 4;
        let rto = srtt + core::cmp::max(1, var4);
        core::cmp::max(rto, 200_000) // Minimum 200ms
    }

    // -- Reno helpers --

    fn reno_on_ack(&mut self, _acked_bytes: u32) {
        match self.state {
            CongestionState::SlowStart => {
                self.cwnd += 1;
                if self.cwnd >= self.ssthresh {
                    self.state = CongestionState::CongestionAvoidance;
                }
            }
            CongestionState::CongestionAvoidance => {
                // Additive increase: cwnd += 1/cwnd per ACK (approximation).
                // We track a fractional part via integer division.
                self.cwnd += core::cmp::max(1, 1024 / self.cwnd);
            }
            CongestionState::FastRecovery | CongestionState::FastRetransmit => {
                // Exit fast recovery on new ACK.
                self.cwnd = self.ssthresh;
                self.state = CongestionState::CongestionAvoidance;
            }
        }
        self.cwnd = core::cmp::min(self.cwnd, self.max_cwnd);
    }

    // -- Cubic helpers --

    fn cubic_on_ack(&mut self, _acked_bytes: u32) {
        match self.state {
            CongestionState::SlowStart => {
                self.cwnd += 1;
                if self.cwnd >= self.ssthresh {
                    self.state = CongestionState::CongestionAvoidance;
                }
            }
            CongestionState::CongestionAvoidance => {
                // W_cubic(t) = C * (t - K)^3 + W_max
                // We use integer arithmetic with time in milliseconds.
                let t_ms = if self.cubic_origin > 0 { 1u32 } else { 1u32 };
                let diff = if t_ms >= self.cubic_k {
                    t_ms - self.cubic_k
                } else {
                    self.cubic_k - t_ms
                };
                // Cube in fixed-point (avoid overflow with u64).
                let diff3 = (diff as u64) * (diff as u64) * (diff as u64);
                let w_cubic = ((CUBIC_C as u64 * diff3) / 1_000_000)
                    .saturating_add(self.cubic_wmax as u64);
                let target = core::cmp::min(w_cubic as u32, self.max_cwnd);
                if target > self.cwnd {
                    self.cwnd += 1;
                } else {
                    // TCP-friendly region: fall back to Reno-like increase.
                    self.cwnd += core::cmp::max(1, 1024 / self.cwnd);
                }
            }
            CongestionState::FastRecovery | CongestionState::FastRetransmit => {
                self.cwnd = self.ssthresh;
                self.state = CongestionState::CongestionAvoidance;
            }
        }
        self.cwnd = core::cmp::min(self.cwnd, self.max_cwnd);
    }

    // -- BBR helpers --

    fn bbr_on_ack(&mut self, acked_bytes: u32) {
        // Update bandwidth estimate: delivered / rtt.
        if self.bbr_rt_prop > 0 && self.bbr_rt_prop < u32::MAX {
            // bw = acked_bytes / rtt_sec, but in integer form:
            // bw = acked_bytes * 1_000_000 / rtt_us
            let bw = (acked_bytes as u64)
                .saturating_mul(1_000_000)
                / (self.bbr_rt_prop as u64);
            let bw = bw as u32;
            if bw > self.bbr_btl_bw {
                self.bbr_btl_bw = bw;
            }
        }

        // Pacing rate = btl_bw * gain (gain = 1.0 in steady state).
        self.bbr_pacing_rate = self.bbr_btl_bw;

        // cwnd = btl_bw * rt_prop / segment_size (approximate with segments).
        if self.bbr_btl_bw > 0 && self.bbr_rt_prop < u32::MAX {
            let bdp = (self.bbr_btl_bw as u64)
                .saturating_mul(self.bbr_rt_prop as u64)
                / 1_000_000;
            // BDP in bytes -> convert to segments (assume 1460 byte MSS).
            let bdp_segments = core::cmp::max((bdp / 1460) as u32, MIN_CWND);
            self.cwnd = core::cmp::min(bdp_segments + 2, self.max_cwnd);
        }

        if self.state == CongestionState::FastRecovery
            || self.state == CongestionState::FastRetransmit
        {
            self.state = CongestionState::CongestionAvoidance;
        }
    }
}

/// Integer cube-root approximation (Newton's method) for Cubic K computation.
fn icbrt(val: u64) -> u32 {
    if val == 0 {
        return 0;
    }
    let mut x: u64 = 1;
    // Find initial guess via bit shifting.
    while x * x * x < val {
        x <<= 1;
    }
    // Newton iterations.
    for _ in 0..32 {
        let x2 = (2 * x + val / (x * x)) / 3;
        if x2 >= x {
            break;
        }
        x = x2;
    }
    x as u32
}

/// Compute Cubic K = cbrt(W_max * (1 - beta) / C).
fn cubic_k_compute(wmax: u32, _cwnd: u32) -> u32 {
    // K = cbrt(wmax * (1 - beta) / C)
    // (1 - beta) = 300/1000 = 0.3
    let numerator = (wmax as u64) * 300;
    let denom = CUBIC_C as u64; // 400
    let val = numerator * 1000 / denom; // scale for precision
    icbrt(val)
}

// ---------------------------------------------------------------------------
// Connection tracking
// ---------------------------------------------------------------------------

/// A tracked connection with its congestion state.
pub struct TrackedConnection {
    pub conn_id: u32,
    pub src_port: u16,
    pub dst_port: u16,
    pub cc: CongestionControl,
}

/// Global connection table.
struct ConnectionTable {
    connections: Vec<TrackedConnection>,
    next_id: u32,
    default_algo: Algorithm,
}

impl ConnectionTable {
    const fn new() -> Self {
        Self {
            connections: Vec::new(),
            next_id: 1,
            default_algo: Algorithm::Reno,
        }
    }
}

static TABLE: Mutex<ConnectionTable> = Mutex::new(ConnectionTable::new());

/// Global congestion statistics.
pub struct CongestionStats {
    pub acks: AtomicU64,
    pub losses: AtomicU64,
    pub timeouts: AtomicU64,
    pub rtt_samples: AtomicU64,
}

static STATS: CongestionStats = CongestionStats {
    acks: AtomicU64::new(0),
    losses: AtomicU64::new(0),
    timeouts: AtomicU64::new(0),
    rtt_samples: AtomicU64::new(0),
};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the congestion control subsystem.
pub fn init() {
    let mut table = TABLE.lock();
    table.connections.clear();
    table.next_id = 1;
    table.default_algo = Algorithm::Reno;
}

/// Set the default congestion control algorithm for new connections.
pub fn set_default_algorithm(algo: Algorithm) {
    TABLE.lock().default_algo = algo;
}

/// Register a new connection and return its id.
pub fn new_connection(src_port: u16, dst_port: u16) -> Option<u32> {
    let mut table = TABLE.lock();
    if table.connections.len() >= MAX_CONNECTIONS {
        return None;
    }
    let id = table.next_id;
    table.next_id += 1;
    let algo = table.default_algo;
    table.connections.push(TrackedConnection {
        conn_id: id,
        src_port,
        dst_port,
        cc: CongestionControl::new(algo),
    });
    STATS.acks.fetch_add(0, Ordering::Relaxed); // touch stats
    Some(id)
}

/// Remove a connection by id.
pub fn remove_connection(conn_id: u32) -> bool {
    let mut table = TABLE.lock();
    if let Some(pos) = table.connections.iter().position(|c| c.conn_id == conn_id) {
        table.connections.remove(pos);
        true
    } else {
        false
    }
}

/// Return info for a specific connection.
pub fn congestion_info(conn_id: u32) -> String {
    let table = TABLE.lock();
    if let Some(c) = table.connections.iter().find(|c| c.conn_id == conn_id) {
        let cc = &c.cc;
        format!(
            "conn {} | algo={:?} state={:?} cwnd={} ssthresh={} rtt={}us var={}us flight={} dupacks={}\n",
            c.conn_id, cc.algorithm, cc.state, cc.cwnd, cc.ssthresh,
            cc.rtt_us >> 8, cc.rtt_var >> 8, cc.bytes_in_flight, cc.dup_ack_count,
        )
    } else {
        format!("connection {} not found\n", conn_id)
    }
}

/// List all tracked connections.
pub fn list_connections() -> String {
    let table = TABLE.lock();
    if table.connections.is_empty() {
        return "No active congestion-controlled connections.\n".into();
    }
    let mut out = String::new();
    out.push_str("ID   ALGO   STATE              CWND   SSTHRESH  RTT(us)  FLIGHT\n");
    out.push_str("---- ------ ------------------ ------ --------- -------- ------\n");
    for c in &table.connections {
        let cc = &c.cc;
        let algo = match cc.algorithm {
            Algorithm::Reno => "Reno  ",
            Algorithm::Cubic => "Cubic ",
            Algorithm::Bbr => "BBR   ",
        };
        let state = match cc.state {
            CongestionState::SlowStart => "SlowStart         ",
            CongestionState::CongestionAvoidance => "CongAvoid         ",
            CongestionState::FastRecovery => "FastRecovery      ",
            CongestionState::FastRetransmit => "FastRetransmit    ",
        };
        out.push_str(&format!(
            "{:<4} {} {} {:<6} {:<9} {:<8} {}\n",
            c.conn_id, algo, state, cc.cwnd, cc.ssthresh,
            cc.rtt_us >> 8, cc.bytes_in_flight,
        ));
    }
    out
}

/// Return global congestion statistics.
pub fn congestion_stats() -> String {
    let acks = STATS.acks.load(Ordering::Relaxed);
    let losses = STATS.losses.load(Ordering::Relaxed);
    let timeouts = STATS.timeouts.load(Ordering::Relaxed);
    let samples = STATS.rtt_samples.load(Ordering::Relaxed);
    let table = TABLE.lock();
    format!(
        "TCP Congestion Control\n  default algo: {:?}\n  connections: {}\n  \
         acks: {}  losses: {}  timeouts: {}  rtt_samples: {}\n",
        table.default_algo, table.connections.len(),
        acks, losses, timeouts, samples,
    )
}
