/// Token-bucket rate limiter for MerlionOS network and resource control.
/// Provides configurable per-resource rate limiting using the token bucket
/// algorithm, with specialised wrappers for bandwidth and connection limiting.
/// Thread-safe via `spin::Mutex`; suitable for `#![no_std]` kernel use.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use crate::timer;

/// A single token bucket that refills at a steady rate over time.
/// Tokens represent abstract units of permission (bytes, requests, etc.).
#[derive(Debug, Clone)]
pub struct TokenBucket {
    /// Maximum number of tokens the bucket can hold.
    pub capacity: u64,
    /// Current number of available tokens.
    pub tokens: u64,
    /// Tokens added per timer tick (tick = 1/100 s at PIT_FREQUENCY_HZ).
    pub refill_rate_per_tick: u64,
    /// Timer tick value when the bucket was last refilled.
    pub last_refill_tick: u64,
}

impl TokenBucket {
    /// Create a new token bucket.
    ///
    /// `capacity` is the burst size (max tokens stored at once).
    /// `rate_per_sec` is the sustained refill rate in tokens per second.
    /// The bucket starts full.
    pub fn new(capacity: u64, rate_per_sec: u64) -> Self {
        let refill_rate_per_tick = rate_per_sec / timer::PIT_FREQUENCY_HZ;
        Self {
            capacity,
            tokens: capacity,
            refill_rate_per_tick: if refill_rate_per_tick == 0 && rate_per_sec > 0 { 1 } else { refill_rate_per_tick },
            last_refill_tick: timer::ticks(),
        }
    }

    /// Refill the bucket based on elapsed time since the last refill.
    pub fn refill(&mut self) {
        let now = timer::ticks();
        if now <= self.last_refill_tick {
            return;
        }
        let elapsed = now - self.last_refill_tick;
        let add = elapsed.saturating_mul(self.refill_rate_per_tick);
        self.tokens = (self.tokens + add).min(self.capacity);
        self.last_refill_tick = now;
    }

    /// Attempt to consume `n` tokens.
    ///
    /// Calls `refill()` first, then removes tokens if enough are available.
    /// Returns `true` if the tokens were consumed, `false` if the request
    /// was denied (not enough tokens).
    pub fn try_consume(&mut self, n: u64) -> bool {
        self.refill();
        if self.tokens >= n {
            self.tokens -= n;
            true
        } else {
            false
        }
    }

    /// Number of tokens currently available (after refill).
    pub fn available(&mut self) -> u64 {
        self.refill();
        self.tokens
    }
}

// ---------------------------------------------------------------------------
// Named-bucket rate limiter
// ---------------------------------------------------------------------------

/// A collection of named token buckets for managing multiple rate limits.
pub struct RateLimiter {
    /// Named buckets: `(name, bucket)`.
    pub buckets: Vec<(String, TokenBucket)>,
}

impl RateLimiter {
    /// Create an empty rate limiter with no buckets.
    pub fn new() -> Self {
        Self { buckets: Vec::new() }
    }

    /// Create a new named bucket.
    ///
    /// `name` identifies the resource, `capacity` is burst size, and
    /// `rate` is the sustained token refill rate (tokens/sec).
    /// If a bucket with the same name already exists it is replaced.
    pub fn create_limiter(&mut self, name: &str, capacity: u64, rate: u64) {
        // Remove existing entry with same name.
        self.buckets.retain(|(n, _)| n != name);
        self.buckets.push((String::from(name), TokenBucket::new(capacity, rate)));
    }

    /// Check (and consume) `tokens` from the named bucket.
    ///
    /// Returns `true` if the tokens were available and consumed.
    /// Returns `false` if the bucket does not exist or has insufficient tokens.
    pub fn check(&mut self, name: &str, tokens: u64) -> bool {
        for (n, bucket) in self.buckets.iter_mut() {
            if n == name {
                return bucket.try_consume(tokens);
            }
        }
        false
    }

    /// Remove a named bucket.  Returns `true` if it existed.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.buckets.len();
        self.buckets.retain(|(n, _)| n != name);
        self.buckets.len() < before
    }

    /// Number of configured buckets.
    pub fn count(&self) -> usize {
        self.buckets.len()
    }
}

// ---------------------------------------------------------------------------
// Bandwidth limiter (bytes per second)
// ---------------------------------------------------------------------------

/// Network bandwidth limiter built on a `TokenBucket` where each token
/// represents one byte.
pub struct BandwidthLimiter {
    /// Underlying token bucket (1 token = 1 byte).
    bucket: TokenBucket,
    /// Total bytes allowed through since creation.
    pub total_passed: u64,
    /// Total bytes denied since creation.
    pub total_denied: u64,
}

impl BandwidthLimiter {
    /// Create a bandwidth limiter.
    ///
    /// `bytes_per_sec` is the sustained rate.  `burst` is how many bytes
    /// can be sent in a single burst above the sustained rate.
    pub fn new(bytes_per_sec: u64, burst: u64) -> Self {
        Self {
            bucket: TokenBucket::new(burst, bytes_per_sec),
            total_passed: 0,
            total_denied: 0,
        }
    }

    /// Try to send `bytes` through the limiter.
    /// Returns `true` if the transfer is allowed.
    pub fn try_send(&mut self, bytes: u64) -> bool {
        if self.bucket.try_consume(bytes) {
            self.total_passed += bytes;
            true
        } else {
            self.total_denied += bytes;
            false
        }
    }

    /// Bytes currently available in the burst window.
    pub fn available(&mut self) -> u64 {
        self.bucket.available()
    }
}

// ---------------------------------------------------------------------------
// Connection limiter (max concurrent per IP)
// ---------------------------------------------------------------------------

/// Limits the number of concurrent connections from each source IP address.
pub struct ConnectionLimiter {
    /// Maximum concurrent connections allowed per IP.
    pub max_per_ip: u32,
    /// Active connection counts: `(ip, count)`.
    active: Vec<([u8; 4], u32)>,
}

impl ConnectionLimiter {
    /// Create a new connection limiter with the given per-IP maximum.
    pub fn new(max_per_ip: u32) -> Self {
        Self { max_per_ip, active: Vec::new() }
    }

    /// Try to open a connection from `ip`.
    /// Returns `true` if the connection is allowed (under the limit).
    pub fn try_connect(&mut self, ip: [u8; 4]) -> bool {
        for (addr, count) in self.active.iter_mut() {
            if *addr == ip {
                if *count >= self.max_per_ip {
                    return false;
                }
                *count += 1;
                return true;
            }
        }
        self.active.push((ip, 1));
        true
    }

    /// Record a disconnection from `ip`, freeing a slot.
    pub fn disconnect(&mut self, ip: [u8; 4]) {
        for (addr, count) in self.active.iter_mut() {
            if *addr == ip && *count > 0 {
                *count -= 1;
                return;
            }
        }
    }

    /// Current connection count for `ip`.
    pub fn connections_from(&self, ip: [u8; 4]) -> u32 {
        self.active.iter()
            .find(|(a, _)| *a == ip)
            .map_or(0, |(_, c)| *c)
    }

    /// Total active connections across all IPs.
    pub fn total_active(&self) -> u32 {
        self.active.iter().map(|(_, c)| *c).sum()
    }

    /// Remove entries with zero connections to reclaim memory.
    pub fn gc(&mut self) {
        self.active.retain(|(_, c)| *c > 0);
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Global rate limiter instance, protected by a spinlock.
pub static RATE_LIMITER: Mutex<Option<RateLimiter>> = Mutex::new(None);

/// Initialise the global rate limiter.
pub fn init() {
    *RATE_LIMITER.lock() = Some(RateLimiter::new());
}

/// Create a named bucket in the global rate limiter.
pub fn create_limiter(name: &str, capacity: u64, rate: u64) {
    if let Some(ref mut rl) = *RATE_LIMITER.lock() {
        rl.create_limiter(name, capacity, rate);
    }
}

/// Check and consume tokens from a named bucket in the global rate limiter.
pub fn check(name: &str, tokens: u64) -> bool {
    RATE_LIMITER.lock().as_mut().map_or(false, |rl| rl.check(name, tokens))
}

/// Format a human-readable status of all rate limiter buckets.
pub fn format_status() -> String {
    let mut rl = RATE_LIMITER.lock();
    let rl = match rl.as_mut() {
        Some(r) => r,
        None => return "(rate limiter not initialised)\n".into(),
    };
    if rl.buckets.is_empty() {
        return "(no rate limit buckets configured)\n".into();
    }
    let mut out = String::new();
    out.push_str("NAME                 CAPACITY    TOKENS      RATE/TICK\n");
    out.push_str("-------------------- ----------- ----------- -----------\n");
    for (name, bucket) in rl.buckets.iter_mut() {
        bucket.refill();
        out.push_str(&format!(
            "{:<20} {:<11} {:<11} {}\n",
            name, bucket.capacity, bucket.tokens, bucket.refill_rate_per_tick,
        ));
    }
    out
}
