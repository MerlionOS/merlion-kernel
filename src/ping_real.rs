/// Real ICMP ping implementation for MerlionOS.
///
/// Sends genuine ICMP Echo Request packets through the network stack and
/// waits for Echo Reply packets from the remote host. Unlike the simulated
/// ping in [`crate::netproto`], this module exercises the full transmit and
/// receive paths via [`crate::netstack`].
///
/// # Usage
///
/// ```ignore
/// let results = ping_real::ping([8, 8, 8, 8], 4);
/// serial_println!("{}", ping_real::format_results(&results));
/// ```

use alloc::string::String;
use alloc::format;
use alloc::vec::Vec;

use crate::net;
use crate::netstack;
use crate::timer;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// IP protocol number for ICMP.
const IP_PROTO_ICMP: u8 = 1;

/// ICMP type: Echo Reply.
const ICMP_TYPE_ECHO_REPLY: u8 = 0;

/// Maximum number of ticks to wait for a single reply before declaring
/// a timeout. At 100 Hz PIT this gives roughly 5 seconds.
const REPLY_TIMEOUT_TICKS: u64 = 500;

/// Number of ticks to wait between consecutive pings (~1 second).
const INTER_PING_DELAY: u64 = 100;

// ---------------------------------------------------------------------------
// PingResult
// ---------------------------------------------------------------------------

/// Result of a single ICMP Echo Request / Reply exchange.
#[derive(Debug, Clone)]
pub struct PingResult {
    /// IPv4 address of the ping target.
    pub target_ip: [u8; 4],
    /// ICMP sequence number for this ping.
    pub seq: u16,
    /// Whether an Echo Reply was received before the timeout.
    pub success: bool,
    /// Round-trip time measured in PIT ticks (each tick is ~10 ms at 100 Hz).
    pub rtt_ticks: u64,
    /// Time-To-Live from the reply's IP header (0 if timed out).
    pub ttl: u8,
}

// ---------------------------------------------------------------------------
// Core ping function
// ---------------------------------------------------------------------------

/// Send `count` ICMP Echo Requests to `target_ip` and collect the results.
///
/// For each ping the function:
///   1. Records the start time via [`timer::ticks`].
///   2. Transmits an ICMP Echo Request via [`netstack::send_icmp_echo`].
///   3. Polls for incoming frames with [`netstack::poll_rx`], checking each
///      for an ICMP Echo Reply (type 0, code 0) that matches our target.
///   4. Records the round-trip time in ticks on success, or marks a timeout.
///   5. Waits [`INTER_PING_DELAY`] ticks before sending the next request.
///
/// Returns a `Vec<PingResult>` with one entry per sequence number.
pub fn ping(target_ip: [u8; 4], count: u16) -> Vec<PingResult> {
    let mut results = Vec::with_capacity(count as usize);

    for seq in 0..count {
        let start = timer::ticks();

        // Transmit the ICMP Echo Request.
        let sent = netstack::send_icmp_echo(target_ip, seq);
        if !sent {
            // NIC unavailable — record immediate failure.
            results.push(PingResult {
                target_ip,
                seq,
                success: false,
                rtt_ticks: 0,
                ttl: 0,
            });
            busy_wait_ticks(INTER_PING_DELAY);
            continue;
        }

        // Poll for the matching Echo Reply.
        let mut success = false;
        let mut rtt: u64 = 0;
        let mut ttl: u8 = 0;
        let deadline = start + REPLY_TIMEOUT_TICKS;

        while timer::ticks() < deadline {
            if let Some(frame) = netstack::poll_rx() {
                // We only care about IPv4 frames.
                if frame.ethertype != net::ETH_TYPE_IP {
                    continue;
                }

                let ip_payload = &frame.payload;
                if ip_payload.len() < 20 {
                    continue;
                }

                // Check that the IP protocol is ICMP.
                if ip_payload[9] != IP_PROTO_ICMP {
                    continue;
                }

                // Extract the TTL from the IP header.
                let reply_ttl = ip_payload[8];

                // Extract source IP from the IP header.
                let mut src_ip = [0u8; 4];
                src_ip.copy_from_slice(&ip_payload[12..16]);

                // Compute IP header length.
                let ihl = ((ip_payload[0] & 0x0F) as usize) * 4;
                if ip_payload.len() < ihl + 8 {
                    continue;
                }

                // Parse the ICMP header for type and sequence.
                if let Some((icmp_type, icmp_seq)) = parse_icmp_reply(ip_payload) {
                    if icmp_type == ICMP_TYPE_ECHO_REPLY
                        && icmp_seq == seq
                        && src_ip == target_ip
                    {
                        rtt = timer::ticks() - start;
                        ttl = reply_ttl;
                        success = true;
                        break;
                    }
                }
            }

            // Yield the CPU briefly between polls.
            x86_64::instructions::hlt();
        }

        if !success {
            rtt = 0;
            ttl = 0;
        }

        results.push(PingResult {
            target_ip,
            seq,
            success,
            rtt_ticks: rtt,
            ttl,
        });

        // Delay between pings.
        if seq + 1 < count {
            busy_wait_ticks(INTER_PING_DELAY);
        }
    }

    results
}

// ---------------------------------------------------------------------------
// ICMP reply parser
// ---------------------------------------------------------------------------

/// Extract the ICMP type and sequence number from a raw IPv4 packet.
///
/// Parses the IP header to locate the ICMP payload, then reads the type
/// byte (offset 0) and sequence number (offset 6..8). Returns `None` if
/// the packet is too short or the IP protocol is not ICMP.
pub fn parse_icmp_reply(data: &[u8]) -> Option<(u8, u16)> {
    if data.len() < 20 {
        return None;
    }

    // Verify IP protocol is ICMP.
    if data[9] != IP_PROTO_ICMP {
        return None;
    }

    let ihl = ((data[0] & 0x0F) as usize) * 4;
    if data.len() < ihl + 8 {
        return None;
    }

    let icmp = &data[ihl..];
    let icmp_type = icmp[0];
    let _icmp_code = icmp[1];
    let icmp_seq = u16::from_be_bytes([icmp[6], icmp[7]]);

    Some((icmp_type, icmp_seq))
}

// ---------------------------------------------------------------------------
// Result formatting
// ---------------------------------------------------------------------------

/// Format ping results into a human-readable string.
///
/// Each successful reply is shown as:
///   `Reply from X.X.X.X: seq=N ttl=64 time=Xms`
///
/// Timed-out requests are shown as:
///   `Request timeout for seq=N`
///
/// A summary line is appended:
///   `N packets sent, N received, N% loss`
pub fn format_results(results: &[PingResult]) -> String {
    let mut out = String::new();

    if let Some(first) = results.first() {
        let ip = first.target_ip;
        out.push_str(&format!(
            "PING {}.{}.{}.{} — {} requests\n",
            ip[0], ip[1], ip[2], ip[3],
            results.len()
        ));
    }

    for r in results {
        if r.success {
            // Each tick is ~10 ms at 100 Hz PIT.
            let ms = r.rtt_ticks * (1000 / timer::PIT_FREQUENCY_HZ);
            out.push_str(&format!(
                "Reply from {}.{}.{}.{}: seq={} ttl={} time={}ms\n",
                r.target_ip[0], r.target_ip[1], r.target_ip[2], r.target_ip[3],
                r.seq, r.ttl, ms,
            ));
        } else {
            out.push_str(&format!("Request timeout for seq={}\n", r.seq));
        }
    }

    // Summary statistics.
    let total = results.len();
    let received = results.iter().filter(|r| r.success).count();
    let loss_pct = if total > 0 {
        ((total - received) * 100) / total
    } else {
        0
    };

    if let Some(first) = results.first() {
        let ip = first.target_ip;
        out.push_str(&format!(
            "\n--- {}.{}.{}.{} ping statistics ---\n",
            ip[0], ip[1], ip[2], ip[3],
        ));
    }

    out.push_str(&format!(
        "{} packets sent, {} received, {}% loss\n",
        total, received, loss_pct,
    ));

    out
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

/// Busy-wait for `n` PIT ticks, yielding the CPU between checks.
fn busy_wait_ticks(n: u64) {
    let target = timer::ticks() + n;
    while timer::ticks() < target {
        x86_64::instructions::hlt();
    }
}
