/// Real TCP implementation for MerlionOS.
///
/// Provides a genuine TCP/IP stack capable of performing 3-way handshakes,
/// data transfer with sequence/acknowledgement tracking, and connection
/// teardown over the wire. Builds TCP segments with correct checksums
/// (including IPv4 pseudo-header) and drives a per-socket state machine
/// that follows the key transitions from RFC 793.
///
/// Packets are sent via [`crate::netstack::send_ipv4`] and received via
/// [`crate::netstack::poll_rx`].

use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::net::Ipv4Addr;
use crate::netstack;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// IP protocol number for TCP.
const IP_PROTO_TCP: u8 = 6;

/// TCP header length in bytes (without options).
const TCP_HEADER_LEN: usize = 20;

/// Maximum number of concurrent TCP sockets.
const MAX_SOCKETS: usize = 16;

/// Default receive window size advertised to the peer.
const DEFAULT_WINDOW: u16 = 8192;

/// Maximum retries when waiting for a reply during handshake or close.
const MAX_RETRIES: usize = 200;

/// Size of per-socket receive buffer before back-pressure.
const RECV_BUF_CAP: usize = 65536;

// ---------------------------------------------------------------------------
// TCP flags
// ---------------------------------------------------------------------------

/// FIN flag — sender has finished sending data.
pub const TCP_FIN: u8 = 0x01;
/// SYN flag — synchronise sequence numbers (connection setup).
pub const TCP_SYN: u8 = 0x02;
/// RST flag — reset the connection.
pub const TCP_RST: u8 = 0x04;
/// PSH flag — push buffered data to the receiving application.
pub const TCP_PSH: u8 = 0x08;
/// ACK flag — acknowledgement field is significant.
pub const TCP_ACK: u8 = 0x10;

// ---------------------------------------------------------------------------
// TCP header
// ---------------------------------------------------------------------------

/// On-wire TCP header (20 bytes, no options).
///
/// Laid out exactly as it appears on the network so that we can safely
/// transmute to/from byte slices when the platform is big-endian *or*
/// manually serialise on little-endian (which is what we do).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct TcpHeader {
    /// Source port (network byte order).
    pub src_port: u16,
    /// Destination port (network byte order).
    pub dst_port: u16,
    /// Sequence number (network byte order).
    pub seq: u32,
    /// Acknowledgement number (network byte order).
    pub ack: u32,
    /// Data offset (upper 4 bits) and flags (lower 6 bits of the second byte).
    ///
    /// The wire format packs the 4-bit data offset, 3 reserved bits, and
    /// 3+6 flag bits across two bytes. We store these two bytes as a single
    /// `u16` in network order for convenience during serialisation.
    pub data_offset_flags: u16,
    /// Receive window size (network byte order).
    pub window: u16,
    /// Checksum (network byte order).
    pub checksum: u16,
    /// Urgent pointer (network byte order).
    pub urgent: u16,
}

// ---------------------------------------------------------------------------
// TCP pseudo-header (for checksum calculation)
// ---------------------------------------------------------------------------

/// IPv4 pseudo-header prepended to the TCP segment for checksum computation
/// per RFC 793 section 3.1.
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct PseudoHeader {
    /// Source IPv4 address.
    src_ip: [u8; 4],
    /// Destination IPv4 address.
    dst_ip: [u8; 4],
    /// Zero byte (reserved).
    zero: u8,
    /// Protocol number (6 for TCP).
    protocol: u8,
    /// TCP segment length (header + payload), network byte order.
    tcp_len: u16,
}

// ---------------------------------------------------------------------------
// TCP connection states
// ---------------------------------------------------------------------------

/// States of the TCP finite state machine (simplified from RFC 793).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TcpState {
    /// No connection.
    Closed,
    /// Waiting for an incoming SYN (server side).
    Listen,
    /// SYN sent, waiting for SYN-ACK (client side).
    SynSent,
    /// SYN received, SYN-ACK sent, waiting for final ACK.
    SynReceived,
    /// Connection is open; data can flow both ways.
    Established,
    /// We sent FIN, waiting for ACK of our FIN.
    FinWait1,
    /// Our FIN was ACKed, waiting for peer's FIN.
    FinWait2,
    /// Peer sent FIN while we were in Established; we need to send FIN.
    CloseWait,
    /// Both sides sent FIN simultaneously.
    Closing,
    /// We sent our FIN (in CloseWait), waiting for ACK.
    LastAck,
    /// Waiting for enough time to pass to ensure the peer received our ACK.
    TimeWait,
}

// ---------------------------------------------------------------------------
// TcpSocket
// ---------------------------------------------------------------------------

/// A single TCP connection socket.
///
/// Tracks sequence numbers, connection state, local and remote endpoints,
/// and holds send/receive buffers.
pub struct TcpSocket {
    /// Current state of the TCP state machine.
    pub state: TcpState,
    /// Our next sequence number to send.
    pub seq_num: u32,
    /// Next sequence number we expect from the peer.
    pub ack_num: u32,
    /// Local IPv4 address.
    pub local_ip: Ipv4Addr,
    /// Local TCP port.
    pub local_port: u16,
    /// Remote IPv4 address.
    pub remote_ip: Ipv4Addr,
    /// Remote TCP port.
    pub remote_port: u16,
    /// Data waiting to be sent.
    pub send_buf: Vec<u8>,
    /// Data received from the peer, available to the application.
    pub recv_buf: Vec<u8>,
}

/// Global table of active TCP sockets protected by a spinlock.
static SOCKETS: Mutex<Vec<TcpSocket>> = Mutex::new(Vec::new());

// ---------------------------------------------------------------------------
// Checksum helpers
// ---------------------------------------------------------------------------

/// Compute the TCP checksum over the pseudo-header and full TCP segment.
///
/// The pseudo-header includes source/destination IP, a zero byte, the
/// protocol number (6), and the TCP segment length. The checksum is the
/// ones' complement of the ones' complement sum of all 16-bit words.
pub fn tcp_checksum(src_ip: [u8; 4], dst_ip: [u8; 4], tcp_data: &[u8]) -> u16 {
    let tcp_len = tcp_data.len() as u16;

    // Build pseudo-header + TCP data into a contiguous buffer.
    let mut buf = Vec::with_capacity(12 + tcp_data.len());
    buf.extend_from_slice(&src_ip);
    buf.extend_from_slice(&dst_ip);
    buf.push(0);
    buf.push(IP_PROTO_TCP);
    buf.extend_from_slice(&tcp_len.to_be_bytes());
    buf.extend_from_slice(tcp_data);

    netstack::ip_checksum(&buf)
}

// ---------------------------------------------------------------------------
// Packet building
// ---------------------------------------------------------------------------

/// Build a complete TCP segment (header + payload) ready for transmission.
///
/// The returned `Vec<u8>` contains the 20-byte TCP header followed by
/// `payload`. The checksum field is computed over a pseudo-header that
/// includes `src_ip` and `dst_ip`.
///
/// # Arguments
///
/// * `src_ip`   — source IPv4 address (4 bytes).
/// * `dst_ip`   — destination IPv4 address (4 bytes).
/// * `src_port` — source TCP port.
/// * `dst_port` — destination TCP port.
/// * `seq`      — sequence number.
/// * `ack`      — acknowledgement number.
/// * `flags`    — combination of `TCP_*` flag constants.
/// * `payload`  — application data (may be empty for control segments).
pub fn build_tcp_packet(
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    payload: &[u8],
) -> Vec<u8> {
    let total_len = TCP_HEADER_LEN + payload.len();
    let mut segment = vec![0u8; total_len];

    // Source port
    segment[0..2].copy_from_slice(&src_port.to_be_bytes());
    // Destination port
    segment[2..4].copy_from_slice(&dst_port.to_be_bytes());
    // Sequence number
    segment[4..8].copy_from_slice(&seq.to_be_bytes());
    // Acknowledgement number
    segment[8..12].copy_from_slice(&ack.to_be_bytes());
    // Data offset (5 words = 20 bytes => upper nibble = 5) + reserved + flags
    // Byte 12: data_offset(4 bits) | reserved(4 bits) = 0x50
    // Byte 13: flags
    segment[12] = (TCP_HEADER_LEN as u8 / 4) << 4; // 0x50
    segment[13] = flags;
    // Window size
    segment[14..16].copy_from_slice(&DEFAULT_WINDOW.to_be_bytes());
    // Checksum placeholder (zero)
    segment[16..18].copy_from_slice(&0u16.to_be_bytes());
    // Urgent pointer
    segment[18..20].copy_from_slice(&0u16.to_be_bytes());
    // Payload
    if !payload.is_empty() {
        segment[TCP_HEADER_LEN..].copy_from_slice(payload);
    }

    // Compute and insert checksum
    let cksum = tcp_checksum(src_ip, dst_ip, &segment);
    segment[16..18].copy_from_slice(&cksum.to_be_bytes());

    segment
}

// ---------------------------------------------------------------------------
// Packet parsing
// ---------------------------------------------------------------------------

/// Parse a TCP header from a raw byte slice.
///
/// Returns `None` if `data` is shorter than 20 bytes. Fields are converted
/// from network byte order to host byte order.
pub fn parse_tcp_header(data: &[u8]) -> Option<TcpHeader> {
    if data.len() < TCP_HEADER_LEN {
        return None;
    }

    let src_port = u16::from_be_bytes([data[0], data[1]]);
    let dst_port = u16::from_be_bytes([data[2], data[3]]);
    let seq = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let ack = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    let data_offset_flags = u16::from_be_bytes([data[12], data[13]]);
    let window = u16::from_be_bytes([data[14], data[15]]);
    let checksum = u16::from_be_bytes([data[16], data[17]]);
    let urgent = u16::from_be_bytes([data[18], data[19]]);

    Some(TcpHeader {
        src_port,
        dst_port,
        seq,
        ack,
        data_offset_flags,
        window,
        checksum,
        urgent,
    })
}

/// Extract the TCP flags byte from a parsed header.
pub fn header_flags(hdr: &TcpHeader) -> u8 {
    (hdr.data_offset_flags & 0x3F) as u8
}

/// Extract the data offset (header length in bytes) from a parsed header.
pub fn header_data_offset(hdr: &TcpHeader) -> usize {
    ((hdr.data_offset_flags >> 12) & 0x0F) as usize * 4
}

/// Extract the payload from a raw TCP segment, using the data offset field.
pub fn segment_payload(data: &[u8]) -> &[u8] {
    if data.len() < TCP_HEADER_LEN {
        return &[];
    }
    let offset = ((data[12] >> 4) as usize) * 4;
    if offset >= data.len() {
        &[]
    } else {
        &data[offset..]
    }
}

// ---------------------------------------------------------------------------
// Send helper
// ---------------------------------------------------------------------------

/// Send a TCP segment via the IP layer.
///
/// Builds the segment with [`build_tcp_packet`] and hands it to
/// [`netstack::send_ipv4`] with protocol number 6.
fn send_segment(
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    payload: &[u8],
) -> bool {
    let segment = build_tcp_packet(src_ip, dst_ip, src_port, dst_port, seq, ack, flags, payload);
    netstack::send_ipv4(dst_ip, IP_PROTO_TCP, &segment)
}

// ---------------------------------------------------------------------------
// Receive helper — poll for a TCP segment destined for a given port
// ---------------------------------------------------------------------------

/// Poll the NIC for a TCP segment addressed to `local_port`.
///
/// Drains frames from [`netstack::poll_rx`], skipping anything that is not
/// an IPv4/TCP packet for our port. Returns the parsed header plus the raw
/// TCP segment bytes (header + payload) on success.
fn poll_tcp_segment(local_port: u16) -> Option<(TcpHeader, Vec<u8>, [u8; 4])> {
    let frame = netstack::poll_rx()?;

    // Must be IPv4 (ethertype 0x0800).
    if frame.ethertype != crate::net::ETH_TYPE_IP {
        return None;
    }

    let ip_payload = &frame.payload;
    if ip_payload.len() < 20 {
        return None;
    }

    // Check IP protocol == TCP (6).
    if ip_payload[9] != IP_PROTO_TCP {
        return None;
    }

    // Extract source IP from IP header.
    let mut src_ip = [0u8; 4];
    src_ip.copy_from_slice(&ip_payload[12..16]);

    // IP header length.
    let ihl = ((ip_payload[0] & 0x0F) as usize) * 4;
    if ip_payload.len() < ihl {
        return None;
    }

    let tcp_data = &ip_payload[ihl..];
    let hdr = parse_tcp_header(tcp_data)?;

    // Filter by destination port.
    if hdr.dst_port != local_port {
        return None;
    }

    Some((hdr, tcp_data.to_vec(), src_ip))
}

// ---------------------------------------------------------------------------
// Initial sequence number generator
// ---------------------------------------------------------------------------

/// Generate an initial sequence number.
///
/// Uses the PIT tick counter for a modicum of unpredictability. A real
/// implementation would use a cryptographic hash (RFC 6528).
fn generate_isn() -> u32 {
    let ticks = crate::timer::ticks();
    // Mix the tick count to spread values across the u32 range.
    (ticks.wrapping_mul(2654435761)) as u32 // Knuth multiplicative hash
}

/// Allocate an ephemeral source port.
///
/// Picks a port in the range 49152..65535 based on the current number of
/// active sockets and the tick counter.
fn alloc_ephemeral_port() -> u16 {
    let base = 49152u16;
    let ticks = crate::timer::ticks() as u16;
    let sockets = SOCKETS.lock().len() as u16;
    base.wrapping_add(ticks).wrapping_add(sockets) | 1 // ensure odd
}

// ---------------------------------------------------------------------------
// Connection API
// ---------------------------------------------------------------------------

/// Open a TCP connection to `dst_ip:dst_port` (active open).
///
/// Performs the full 3-way handshake:
///   1. Send SYN with our initial sequence number.
///   2. Wait for SYN-ACK from the peer.
///   3. Send ACK to complete the handshake.
///
/// Returns the socket index on success, or an error string on failure.
pub fn connect(dst_ip: Ipv4Addr, dst_port: u16) -> Result<usize, &'static str> {
    let src_ip = crate::net::NET.lock().ip;
    let src_port = alloc_ephemeral_port();
    let isn = generate_isn();

    crate::serial_println!(
        "[tcp_real] SYN -> {}:{} (seq={})",
        dst_ip, dst_port, isn
    );

    // --- Step 1: send SYN ---
    if !send_segment(src_ip.0, dst_ip.0, src_port, dst_port, isn, 0, TCP_SYN, &[]) {
        return Err("failed to send SYN");
    }

    // --- Step 2: wait for SYN-ACK ---
    let mut peer_isn: u32 = 0;
    let mut got_synack = false;

    for _ in 0..MAX_RETRIES {
        if let Some((hdr, _raw, _peer_ip)) = poll_tcp_segment(src_port) {
            let flags = header_flags(&hdr);
            if flags & (TCP_SYN | TCP_ACK) == (TCP_SYN | TCP_ACK) {
                // Verify the ACK acknowledges our SYN.
                if hdr.ack == isn.wrapping_add(1) {
                    peer_isn = hdr.seq;
                    got_synack = true;
                    let ack_val = hdr.ack;
                    crate::serial_println!(
                        "[tcp_real] SYN-ACK <- {}:{} (seq={}, ack={})",
                        dst_ip, dst_port, peer_isn, ack_val
                    );
                    break;
                }
            }
            // RST means connection refused.
            if flags & TCP_RST != 0 {
                return Err("connection refused (RST)");
            }
        }
        // Brief pause between polls (~10 ms at 100 Hz PIT).
        busy_wait_ticks(1);
    }

    if !got_synack {
        return Err("connection timed out waiting for SYN-ACK");
    }

    // --- Step 3: send ACK ---
    let our_seq = isn.wrapping_add(1);
    let our_ack = peer_isn.wrapping_add(1);

    send_segment(src_ip.0, dst_ip.0, src_port, dst_port, our_seq, our_ack, TCP_ACK, &[]);

    crate::serial_println!(
        "[tcp_real] ACK -> established (seq={}, ack={})",
        our_seq, our_ack
    );

    // Register the socket.
    let mut sockets = SOCKETS.lock();
    if sockets.len() >= MAX_SOCKETS {
        return Err("socket table full");
    }
    let idx = sockets.len();
    sockets.push(TcpSocket {
        state: TcpState::Established,
        seq_num: our_seq,
        ack_num: our_ack,
        local_ip: src_ip,
        local_port: src_port,
        remote_ip: dst_ip,
        remote_port: dst_port,
        send_buf: Vec::new(),
        recv_buf: Vec::new(),
    });

    crate::klog_println!("[tcp_real] connected {}:{} -> {}:{} (sock {})",
        src_ip, src_port, dst_ip, dst_port, idx);

    Ok(idx)
}

/// Send data on an established TCP connection.
///
/// Transmits `data` with the PSH+ACK flags so the peer delivers it to the
/// application promptly. Updates the socket's sequence number.
///
/// Returns the number of bytes sent, or an error if the socket is not in
/// the Established state.
pub fn send(sock_id: usize, data: &[u8]) -> Result<usize, &'static str> {
    let mut sockets = SOCKETS.lock();
    let sock = sockets.get_mut(sock_id).ok_or("invalid socket id")?;

    if sock.state != TcpState::Established {
        return Err("socket not established");
    }

    let flags = TCP_PSH | TCP_ACK;
    let ok = send_segment(
        sock.local_ip.0,
        sock.remote_ip.0,
        sock.local_port,
        sock.remote_port,
        sock.seq_num,
        sock.ack_num,
        flags,
        data,
    );

    if !ok {
        return Err("failed to transmit segment");
    }

    sock.seq_num = sock.seq_num.wrapping_add(data.len() as u32);

    crate::serial_println!(
        "[tcp_real] sent {} bytes on sock {} (seq now {})",
        data.len(), sock_id, sock.seq_num
    );

    Ok(data.len())
}

/// Read data from the socket's receive buffer.
///
/// Returns whatever data has been accumulated so far and clears the buffer.
/// An empty `Vec` means no data is available yet.
pub fn recv(sock_id: usize) -> Result<Vec<u8>, &'static str> {
    let mut sockets = SOCKETS.lock();
    let sock = sockets.get_mut(sock_id).ok_or("invalid socket id")?;

    if sock.state == TcpState::Closed {
        return Err("socket closed");
    }

    let data = core::mem::take(&mut sock.recv_buf);
    Ok(data)
}

/// Gracefully close a TCP connection.
///
/// Sends a FIN segment and waits for the peer to acknowledge it. The
/// socket transitions through FinWait1 -> FinWait2 -> TimeWait -> Closed.
pub fn close(sock_id: usize) -> Result<(), &'static str> {
    // Scope the first lock to send FIN.
    {
        let mut sockets = SOCKETS.lock();
        let sock = sockets.get_mut(sock_id).ok_or("invalid socket id")?;

        if sock.state == TcpState::Closed {
            return Ok(());
        }

        crate::serial_println!(
            "[tcp_real] FIN -> {}:{} (sock {})",
            sock.remote_ip, sock.remote_port, sock_id
        );

        send_segment(
            sock.local_ip.0,
            sock.remote_ip.0,
            sock.local_port,
            sock.remote_port,
            sock.seq_num,
            sock.ack_num,
            TCP_FIN | TCP_ACK,
            &[],
        );

        sock.seq_num = sock.seq_num.wrapping_add(1); // FIN consumes one seq
        sock.state = TcpState::FinWait1;
    }

    // Wait for FIN-ACK (or just ACK + peer FIN).
    let local_port = {
        let sockets = SOCKETS.lock();
        sockets[sock_id].local_port
    };

    for _ in 0..MAX_RETRIES {
        if let Some((hdr, _raw, _peer_ip)) = poll_tcp_segment(local_port) {
            let mut sockets = SOCKETS.lock();
            let sock = sockets.get_mut(sock_id).ok_or("invalid socket id")?;
            let flags = header_flags(&hdr);

            match sock.state {
                TcpState::FinWait1 => {
                    if flags & TCP_ACK != 0 && flags & TCP_FIN != 0 {
                        // Simultaneous close or combined FIN-ACK.
                        sock.ack_num = hdr.seq.wrapping_add(1);
                        send_segment(
                            sock.local_ip.0, sock.remote_ip.0,
                            sock.local_port, sock.remote_port,
                            sock.seq_num, sock.ack_num,
                            TCP_ACK, &[],
                        );
                        sock.state = TcpState::TimeWait;
                        crate::serial_println!("[tcp_real] -> TimeWait (sock {})", sock_id);
                        break;
                    } else if flags & TCP_ACK != 0 {
                        sock.state = TcpState::FinWait2;
                        crate::serial_println!("[tcp_real] -> FinWait2 (sock {})", sock_id);
                    }
                }
                TcpState::FinWait2 => {
                    if flags & TCP_FIN != 0 {
                        sock.ack_num = hdr.seq.wrapping_add(1);
                        send_segment(
                            sock.local_ip.0, sock.remote_ip.0,
                            sock.local_port, sock.remote_port,
                            sock.seq_num, sock.ack_num,
                            TCP_ACK, &[],
                        );
                        sock.state = TcpState::TimeWait;
                        crate::serial_println!("[tcp_real] -> TimeWait (sock {})", sock_id);
                        break;
                    }
                }
                _ => {}
            }
        }
        busy_wait_ticks(1);
    }

    // Transition TimeWait -> Closed (abbreviated; real TCP waits 2*MSL).
    {
        let mut sockets = SOCKETS.lock();
        if let Some(sock) = sockets.get_mut(sock_id) {
            sock.state = TcpState::Closed;
            crate::serial_println!("[tcp_real] closed (sock {})", sock_id);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Incoming segment processing
// ---------------------------------------------------------------------------

/// Process an incoming TCP segment for a given socket.
///
/// Handles state transitions and data delivery. This is the core of the
/// receive-side state machine and should be called whenever a segment
/// arrives that matches an existing socket.
///
/// `segment` is the raw TCP segment bytes (header + payload) as extracted
/// from the IP payload.
pub fn process_incoming(sock_id: usize, segment: &[u8]) {
    let hdr = match parse_tcp_header(segment) {
        Some(h) => h,
        None => return,
    };

    let flags = header_flags(&hdr);
    let payload = segment_payload(segment);

    let mut sockets = SOCKETS.lock();
    let sock = match sockets.get_mut(sock_id) {
        Some(s) => s,
        None => return,
    };

    // RST handling — unconditionally tear down.
    if flags & TCP_RST != 0 {
        crate::serial_println!("[tcp_real] RST received on sock {}", sock_id);
        sock.state = TcpState::Closed;
        return;
    }

    match sock.state {
        TcpState::SynSent => {
            // Expecting SYN-ACK.
            if flags & (TCP_SYN | TCP_ACK) == (TCP_SYN | TCP_ACK) {
                sock.ack_num = hdr.seq.wrapping_add(1);
                sock.state = TcpState::Established;
                // Send ACK.
                send_segment(
                    sock.local_ip.0, sock.remote_ip.0,
                    sock.local_port, sock.remote_port,
                    sock.seq_num, sock.ack_num,
                    TCP_ACK, &[],
                );
                crate::serial_println!(
                    "[tcp_real] SynSent -> Established (sock {})", sock_id
                );
            }
        }

        TcpState::SynReceived => {
            // Expecting ACK of our SYN-ACK.
            if flags & TCP_ACK != 0 {
                sock.state = TcpState::Established;
                crate::serial_println!(
                    "[tcp_real] SynReceived -> Established (sock {})", sock_id
                );
            }
        }

        TcpState::Established => {
            // Peer FIN — begin passive close.
            if flags & TCP_FIN != 0 {
                sock.ack_num = hdr.seq.wrapping_add(payload.len() as u32).wrapping_add(1);
                sock.state = TcpState::CloseWait;
                // ACK the FIN.
                send_segment(
                    sock.local_ip.0, sock.remote_ip.0,
                    sock.local_port, sock.remote_port,
                    sock.seq_num, sock.ack_num,
                    TCP_ACK, &[],
                );
                crate::serial_println!(
                    "[tcp_real] Established -> CloseWait (sock {})", sock_id
                );
                return;
            }

            // Data segment.
            if !payload.is_empty() && flags & TCP_ACK != 0 {
                // Verify sequence number matches what we expect.
                if hdr.seq == sock.ack_num {
                    if sock.recv_buf.len() + payload.len() <= RECV_BUF_CAP {
                        sock.recv_buf.extend_from_slice(payload);
                    }
                    sock.ack_num = sock.ack_num.wrapping_add(payload.len() as u32);
                }
                // Send ACK for the received data.
                send_segment(
                    sock.local_ip.0, sock.remote_ip.0,
                    sock.local_port, sock.remote_port,
                    sock.seq_num, sock.ack_num,
                    TCP_ACK, &[],
                );
            }
        }

        TcpState::FinWait1 => {
            if flags & TCP_FIN != 0 && flags & TCP_ACK != 0 {
                // Peer ACKs our FIN and sends its own FIN simultaneously.
                sock.ack_num = hdr.seq.wrapping_add(1);
                sock.state = TcpState::TimeWait;
                send_segment(
                    sock.local_ip.0, sock.remote_ip.0,
                    sock.local_port, sock.remote_port,
                    sock.seq_num, sock.ack_num,
                    TCP_ACK, &[],
                );
            } else if flags & TCP_ACK != 0 {
                sock.state = TcpState::FinWait2;
            } else if flags & TCP_FIN != 0 {
                // Simultaneous close — both sides sent FIN.
                sock.ack_num = hdr.seq.wrapping_add(1);
                sock.state = TcpState::Closing;
                send_segment(
                    sock.local_ip.0, sock.remote_ip.0,
                    sock.local_port, sock.remote_port,
                    sock.seq_num, sock.ack_num,
                    TCP_ACK, &[],
                );
            }
        }

        TcpState::FinWait2 => {
            if flags & TCP_FIN != 0 {
                sock.ack_num = hdr.seq.wrapping_add(1);
                sock.state = TcpState::TimeWait;
                send_segment(
                    sock.local_ip.0, sock.remote_ip.0,
                    sock.local_port, sock.remote_port,
                    sock.seq_num, sock.ack_num,
                    TCP_ACK, &[],
                );
            }
        }

        TcpState::CloseWait => {
            // Application should call close() to send our FIN.
            // Nothing to do here until then.
        }

        TcpState::Closing => {
            if flags & TCP_ACK != 0 {
                sock.state = TcpState::TimeWait;
            }
        }

        TcpState::LastAck => {
            if flags & TCP_ACK != 0 {
                sock.state = TcpState::Closed;
                crate::serial_println!(
                    "[tcp_real] LastAck -> Closed (sock {})", sock_id
                );
            }
        }

        TcpState::TimeWait => {
            // Ignore segments in TimeWait (simplified — real TCP re-ACKs).
        }

        TcpState::Listen | TcpState::Closed => {
            // Unexpected segment; ignore.
        }
    }
}

// ---------------------------------------------------------------------------
// Background receive pump
// ---------------------------------------------------------------------------

/// Poll the NIC and dispatch any incoming TCP segments to matching sockets.
///
/// Call this periodically (e.g. from the main loop or a timer callback) so
/// that incoming data and control segments are processed. Returns the
/// number of segments dispatched.
pub fn poll_incoming() -> usize {
    let mut dispatched = 0;

    while let Some(frame) = netstack::poll_rx() {
        if frame.ethertype != crate::net::ETH_TYPE_IP {
            continue;
        }
        let ip = &frame.payload;
        if ip.len() < 20 || ip[9] != IP_PROTO_TCP {
            continue;
        }
        let ihl = ((ip[0] & 0x0F) as usize) * 4;
        if ip.len() < ihl + TCP_HEADER_LEN {
            continue;
        }
        let tcp_data = &ip[ihl..];
        let dst_port = u16::from_be_bytes([tcp_data[2], tcp_data[3]]);

        // Find the matching socket.
        let mut src_ip = [0u8; 4];
        src_ip.copy_from_slice(&ip[12..16]);

        let sock_id = {
            let sockets = SOCKETS.lock();
            sockets.iter().enumerate().find(|(_, s)| {
                s.local_port == dst_port
                    && s.remote_ip.0 == src_ip
                    && s.state != TcpState::Closed
            }).map(|(i, _)| i)
        };

        if let Some(id) = sock_id {
            process_incoming(id, tcp_data);
            dispatched += 1;
        }
    }

    dispatched
}

// ---------------------------------------------------------------------------
// Socket query helpers
// ---------------------------------------------------------------------------

/// Return the current state of a socket.
pub fn socket_state(sock_id: usize) -> Option<TcpState> {
    SOCKETS.lock().get(sock_id).map(|s| s.state)
}

/// List all active (non-Closed) sockets with summary info.
pub fn list_sockets() -> Vec<(usize, Ipv4Addr, u16, Ipv4Addr, u16, TcpState)> {
    let sockets = SOCKETS.lock();
    sockets
        .iter()
        .enumerate()
        .filter(|(_, s)| s.state != TcpState::Closed)
        .map(|(i, s)| (i, s.local_ip, s.local_port, s.remote_ip, s.remote_port, s.state))
        .collect()
}

// ---------------------------------------------------------------------------
// Server-side socket registration
// ---------------------------------------------------------------------------

/// Register an already-established TCP connection in the socket table.
///
/// Used by the built-in HTTP server ([`crate::httpd`]) which performs the
/// 3-way handshake manually and then needs a socket index to send data
/// and close the connection through the normal [`send`]/[`close`] API.
///
/// Returns the socket index.
pub fn register_established(
    local_ip: Ipv4Addr,
    local_port: u16,
    remote_ip: Ipv4Addr,
    remote_port: u16,
    seq_num: u32,
    ack_num: u32,
) -> usize {
    let mut sockets = SOCKETS.lock();
    let idx = sockets.len();
    sockets.push(TcpSocket {
        state: TcpState::Established,
        seq_num,
        ack_num,
        local_ip,
        local_port,
        remote_ip,
        remote_port,
        send_buf: Vec::new(),
        recv_buf: Vec::new(),
    });
    crate::serial_println!(
        "[tcp_real] registered server socket {} ({}:{} <- {}:{})",
        idx, local_ip, local_port, remote_ip, remote_port
    );
    idx
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

/// Busy-wait for `n` PIT ticks (~10 ms each at 100 Hz).
fn busy_wait_ticks(n: u64) {
    let target = crate::timer::ticks() + n;
    while crate::timer::ticks() < target {
        x86_64::instructions::hlt();
    }
}
