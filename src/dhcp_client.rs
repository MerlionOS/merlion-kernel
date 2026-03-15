/// DHCP client for MerlionOS.
///
/// Performs the full DORA (Discover-Offer-Request-Ack) handshake using the
/// real network stack and applies the resulting lease to the kernel's global
/// network configuration. Designed for `no_std` environments with the `alloc`
/// crate.
///
/// # Protocol flow
///
/// 1. **Discover** — broadcast to find DHCP servers on the local segment.
/// 2. **Offer** — server responds with an available IP and parameters.
/// 3. **Request** — client confirms it wants the offered address.
/// 4. **Ack** — server finalises the lease.
///
/// Each step is subject to a configurable tick-based timeout so the kernel
/// does not block indefinitely when no DHCP server is reachable.

use alloc::vec::Vec;

use crate::dhcp;
use crate::net;
use crate::netstack;
use crate::timer;

/// Re-export [`DhcpLease`] so callers can use `dhcp_client::DhcpLease`
/// without reaching into the lower-level `dhcp` module directly.
pub use crate::dhcp::DhcpLease;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// DHCP client port (source).
const DHCP_CLIENT_PORT: u16 = 68;

/// DHCP server port (destination).
const DHCP_SERVER_PORT: u16 = 67;

/// Broadcast IPv4 address used for DHCP messages.
const BROADCAST_IP: [u8; 4] = [255, 255, 255, 255];

/// Maximum number of ticks to wait for a DHCP Offer after sending Discover.
/// At 100 Hz this is ~5 seconds.
const OFFER_TIMEOUT_TICKS: u64 = 500;

/// Maximum number of ticks to wait for a DHCP Ack after sending Request.
/// At 100 Hz this is ~5 seconds.
const ACK_TIMEOUT_TICKS: u64 = 500;

/// EtherType for IPv4 frames.
const ETH_TYPE_IP: u16 = 0x0800;

/// IPv4 header length (no options).
const IPV4_HEADER_LEN: usize = 20;

/// UDP header length.
const UDP_HEADER_LEN: usize = 8;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the UDP payload from a raw IPv4 packet, filtering by destination
/// port.
///
/// Returns `None` if the frame is not IPv4/UDP or the destination port does
/// not match `expected_dst_port`.
fn extract_udp_payload(frame: &netstack::ReceivedFrame, expected_dst_port: u16) -> Option<Vec<u8>> {
    if frame.ethertype != ETH_TYPE_IP {
        return None;
    }

    let ip = &frame.payload;
    if ip.len() < IPV4_HEADER_LEN {
        return None;
    }

    // Check protocol == UDP (17)
    if ip[9] != 17 {
        return None;
    }

    // Compute actual IP header length from IHL field.
    let ihl = ((ip[0] & 0x0F) as usize) * 4;
    if ip.len() < ihl + UDP_HEADER_LEN {
        return None;
    }

    let udp = &ip[ihl..];
    let dst_port = u16::from_be_bytes([udp[2], udp[3]]);
    if dst_port != expected_dst_port {
        return None;
    }

    let udp_len = u16::from_be_bytes([udp[4], udp[5]]) as usize;
    if udp.len() < udp_len || udp_len < UDP_HEADER_LEN {
        return None;
    }

    Some(udp[UDP_HEADER_LEN..udp_len].to_vec())
}

// ---------------------------------------------------------------------------
// DORA flow
// ---------------------------------------------------------------------------

/// Execute the full DHCP DORA handshake and return a [`DhcpLease`].
///
/// 1. Builds and broadcasts a DHCP Discover packet.
/// 2. Polls the NIC for a DHCP Offer (with timeout).
/// 3. Builds and broadcasts a DHCP Request for the offered IP.
/// 4. Polls the NIC for a DHCP Ack (with timeout).
///
/// Returns `Err` with a human-readable reason if any step fails or times out.
pub fn run_dhcp() -> Result<DhcpLease, &'static str> {
    // ---- Step 1: Discover ------------------------------------------------
    let discover_pkt = dhcp::discover();
    crate::serial_println!("[dhcp_client] sending DHCP Discover");

    if !netstack::send_udp(BROADCAST_IP, DHCP_CLIENT_PORT, DHCP_SERVER_PORT, &discover_pkt) {
        return Err("failed to send DHCP Discover");
    }

    // ---- Step 2: wait for Offer ------------------------------------------
    let offer_lease = wait_for_dhcp_reply(OFFER_TIMEOUT_TICKS, "Offer")?;
    crate::serial_println!(
        "[dhcp_client] received Offer: ip={} gw={} mask={} dns={} lease={}s",
        offer_lease.ip, offer_lease.gateway, offer_lease.subnet_mask,
        offer_lease.dns, offer_lease.lease_time
    );

    // ---- Step 3: Request -------------------------------------------------
    let request_pkt = dhcp::request(offer_lease.ip);
    crate::serial_println!("[dhcp_client] sending DHCP Request for {}", offer_lease.ip);

    if !netstack::send_udp(BROADCAST_IP, DHCP_CLIENT_PORT, DHCP_SERVER_PORT, &request_pkt) {
        return Err("failed to send DHCP Request");
    }

    // ---- Step 4: wait for Ack --------------------------------------------
    let ack_lease = wait_for_dhcp_reply(ACK_TIMEOUT_TICKS, "Ack")?;
    crate::serial_println!(
        "[dhcp_client] received Ack: ip={} gw={} mask={} dns={} lease={}s",
        ack_lease.ip, ack_lease.gateway, ack_lease.subnet_mask,
        ack_lease.dns, ack_lease.lease_time
    );

    Ok(ack_lease)
}

/// Poll the NIC until a valid DHCP reply (Offer or Ack) arrives or the
/// timeout expires.
///
/// `label` is used only for the error message (e.g. "Offer" or "Ack").
fn wait_for_dhcp_reply(timeout_ticks: u64, label: &'static str) -> Result<DhcpLease, &'static str> {
    let deadline = timer::ticks() + timeout_ticks;

    while timer::ticks() < deadline {
        if let Some(frame) = netstack::poll_rx() {
            if let Some(udp_payload) = extract_udp_payload(&frame, DHCP_CLIENT_PORT) {
                if let Some(lease) = dhcp::parse_offer(&udp_payload) {
                    return Ok(lease);
                }
            }
            // Not a DHCP reply — keep polling.
        }

        // Yield the CPU briefly so we don't spin at full speed. On bare
        // metal the HLT instruction waits for the next interrupt (timer or
        // NIC), which is both power-friendly and responsive.
        x86_64::instructions::hlt();
    }

    match label {
        "Offer" => Err("timeout waiting for DHCP Offer"),
        "Ack" => Err("timeout waiting for DHCP Ack"),
        _ => Err("timeout waiting for DHCP reply"),
    }
}

// ---------------------------------------------------------------------------
// Lease application
// ---------------------------------------------------------------------------

/// Apply a [`DhcpLease`] to the kernel's global network configuration.
///
/// Updates the IP address, gateway, and subnet mask stored in
/// [`net::NET`] so that all subsequent outbound packets use the
/// DHCP-assigned parameters. Logs the new configuration over serial.
pub fn apply_lease(lease: &DhcpLease) {
    let mut state = net::NET.lock();
    state.ip = lease.ip;
    state.gateway = lease.gateway;
    state.netmask = lease.subnet_mask;
    drop(state);

    crate::serial_println!(
        "[dhcp_client] lease applied: ip={} gw={} mask={} dns={} ttl={}s",
        lease.ip, lease.gateway, lease.subnet_mask, lease.dns, lease.lease_time
    );
}

// ---------------------------------------------------------------------------
// High-level entry point
// ---------------------------------------------------------------------------

/// Bring up the network interface via DHCP.
///
/// Runs the full DORA handshake, applies the resulting lease to the global
/// network state, and logs the result. This is the single function that
/// higher-level code (e.g. the shell `ifup` command) should call.
///
/// # Errors
///
/// Returns `Err` with a human-readable message if the DHCP exchange fails
/// (e.g. no server responded within the timeout window).
pub fn ifup() -> Result<(), &'static str> {
    crate::serial_println!("[dhcp_client] starting DHCP on eth0...");

    let lease = run_dhcp()?;
    apply_lease(&lease);

    crate::serial_println!(
        "[dhcp_client] eth0 is up — {} via {}",
        lease.ip, lease.gateway
    );

    Ok(())
}
