/// DNS client for MerlionOS.
///
/// Sends real DNS A-record queries over UDP through the kernel network stack
/// and parses the responses. Includes a simple in-memory cache with TTL-based
/// expiry to avoid redundant lookups.
///
/// Uses [`crate::dhcp`] for query building and response parsing,
/// [`crate::netstack`] for UDP send/receive, and [`crate::timer`] for
/// tick-based cache expiry.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::{dhcp, net, netstack, serial_println, timer};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// DNS server port.
const DNS_PORT: u16 = 53;

/// Source port for outgoing DNS queries.
const DNS_SRC_PORT: u16 = 10053;

/// Default DNS server for QEMU user-net (built-in forwarder).
const QEMU_DNS: [u8; 4] = [10, 0, 2, 3];

/// Maximum number of poll iterations while waiting for a DNS response.
const MAX_POLL_ITERS: usize = 200_000;

/// Cache TTL in timer ticks (100 Hz timer -> 3000 ticks = 30 seconds).
const CACHE_TTL_TICKS: u64 = 3000;

// ---------------------------------------------------------------------------
// DNS cache
// ---------------------------------------------------------------------------

/// A single cached DNS resolution result.
struct DnsCacheEntry {
    /// The hostname that was resolved.
    hostname: String,
    /// The resolved IPv4 address.
    ip: [u8; 4],
    /// Tick count at which this entry expires.
    expiry_tick: u64,
}

/// Global DNS cache protected by a spin mutex.
static DNS_CACHE: Mutex<Vec<DnsCacheEntry>> = Mutex::new(Vec::new());

// ---------------------------------------------------------------------------
// Well-known hosts
// ---------------------------------------------------------------------------

/// Try to resolve a hostname from the well-known hosts table.
///
/// Returns hard-coded addresses for common names such as `localhost`,
/// `gateway`, and `dns`, avoiding a network round-trip.
fn resolve_well_known(hostname: &str) -> Option<[u8; 4]> {
    match hostname {
        "localhost" | "loopback" => Some([127, 0, 0, 1]),
        "gateway" => {
            let gw = net::NET.lock().gateway.0;
            Some(gw)
        }
        "dns" => Some(dns_server_ip()),
        "self" | "me" => {
            let ip = net::NET.lock().ip.0;
            Some(ip)
        }
        _ => None,
    }
}

/// Try to parse `hostname` as a dotted-decimal IPv4 address (e.g. `1.2.3.4`).
fn try_parse_ip(hostname: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = hostname.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let a = parts[0].parse::<u8>().ok()?;
    let b = parts[1].parse::<u8>().ok()?;
    let c = parts[2].parse::<u8>().ok()?;
    let d = parts[3].parse::<u8>().ok()?;
    Some([a, b, c, d])
}

// ---------------------------------------------------------------------------
// DNS server selection
// ---------------------------------------------------------------------------

/// Determine the DNS server IP to use.
///
/// Prefers the gateway address from the current network state (which is
/// typically the DHCP-provided DNS server under QEMU user-net). Falls back
/// to the well-known QEMU DNS forwarder at `10.0.2.3`.
fn dns_server_ip() -> [u8; 4] {
    let ns = net::NET.lock();
    let gw = ns.gateway.0;
    if gw == [0, 0, 0, 0] {
        QEMU_DNS
    } else {
        // Under QEMU user-net the gateway (10.0.2.2) forwards DNS, but
        // the dedicated DNS forwarder at 10.0.2.3 is more reliable.
        QEMU_DNS
    }
}

// ---------------------------------------------------------------------------
// Core resolution
// ---------------------------------------------------------------------------

/// Resolve a hostname to an IPv4 address by sending a real DNS query.
///
/// Builds a DNS A-record query with [`dhcp::build_dns_query`], transmits it
/// as a UDP datagram to the configured DNS server on port 53, then polls
/// the NIC for a matching response. The first A-record address found in the
/// response is returned.
///
/// Well-known hostnames (`localhost`, `gateway`, `dns`, `self`) and literal
/// IPv4 addresses are resolved immediately without a network round-trip.
///
/// # Errors
///
/// Returns a static error string if no response is received within the
/// polling window, or if the response does not contain a valid A record.
pub fn resolve(hostname: &str) -> Result<[u8; 4], &'static str> {
    // Fast path: well-known names.
    if let Some(ip) = resolve_well_known(hostname) {
        return Ok(ip);
    }

    // Fast path: literal IP address.
    if let Some(ip) = try_parse_ip(hostname) {
        return Ok(ip);
    }

    // Build the DNS query packet.
    let query = dhcp::build_dns_query(hostname);
    let dns_server = dns_server_ip();

    serial_println!(
        "[dns] query {} -> {}.{}.{}.{}:{}",
        hostname,
        dns_server[0], dns_server[1], dns_server[2], dns_server[3],
        DNS_PORT
    );

    // Send the query via UDP.
    if !netstack::send_udp(dns_server, DNS_SRC_PORT, DNS_PORT, &query) {
        return Err("dns: failed to send UDP query");
    }

    // Poll for a DNS response.
    for _ in 0..MAX_POLL_ITERS {
        let frame = match netstack::poll_rx() {
            Some(f) => f,
            None => continue,
        };

        // We only care about IPv4 frames.
        if frame.ethertype != net::ETH_TYPE_IP {
            continue;
        }

        let ip_payload = &frame.payload;
        if ip_payload.len() < 20 {
            continue;
        }

        // Check for UDP (protocol 17).
        if ip_payload[9] != 17 {
            continue;
        }

        // Extract IP header length.
        let ihl = ((ip_payload[0] & 0x0F) as usize) * 4;
        if ip_payload.len() < ihl + 8 {
            continue;
        }

        let udp_data = &ip_payload[ihl..];

        // Check source port == 53 (DNS response).
        let src_port = u16::from_be_bytes([udp_data[0], udp_data[1]]);
        if src_port != DNS_PORT {
            continue;
        }

        // Extract the UDP payload (skip 8-byte UDP header).
        let dns_response = &udp_data[8..];

        if let Some(ip) = dhcp::parse_dns_response(dns_response) {
            serial_println!(
                "[dns] resolved {} -> {}.{}.{}.{}",
                hostname, ip[0], ip[1], ip[2], ip[3]
            );
            return Ok(ip);
        }
    }

    serial_println!("[dns] resolution failed for '{}'", hostname);
    Err("dns: no response or no A record")
}

// ---------------------------------------------------------------------------
// Cached resolution
// ---------------------------------------------------------------------------

/// Resolve a hostname with caching.
///
/// Checks the in-memory DNS cache first. If a non-expired entry exists for
/// `hostname`, it is returned immediately. Otherwise, [`resolve`] is called
/// and the result is stored in the cache with a TTL of
/// [`CACHE_TTL_TICKS`] (~30 seconds at 100 Hz).
///
/// Expired entries are pruned lazily on each lookup.
pub fn resolve_with_cache(hostname: &str) -> Result<[u8; 4], &'static str> {
    let now = timer::ticks();

    // Check the cache (and prune expired entries while we are at it).
    {
        let mut cache = DNS_CACHE.lock();

        // Remove expired entries.
        cache.retain(|e| e.expiry_tick > now);

        // Look for a hit.
        for entry in cache.iter() {
            if entry.hostname == hostname {
                serial_println!(
                    "[dns] cache hit for {} -> {}.{}.{}.{}",
                    hostname, entry.ip[0], entry.ip[1], entry.ip[2], entry.ip[3]
                );
                return Ok(entry.ip);
            }
        }
    } // release lock before doing network I/O

    // Cache miss — perform a real resolution.
    let ip = resolve(hostname)?;

    // Store in cache.
    {
        let mut cache = DNS_CACHE.lock();
        cache.push(DnsCacheEntry {
            hostname: String::from(hostname),
            ip,
            expiry_tick: now + CACHE_TTL_TICKS,
        });
    }

    Ok(ip)
}

/// Flush all entries from the DNS cache.
///
/// Useful after a network reconfiguration (e.g. a new DHCP lease) to
/// ensure stale records are not served.
pub fn flush_cache() {
    let mut cache = DNS_CACHE.lock();
    cache.clear();
    serial_println!("[dns] cache flushed");
}
