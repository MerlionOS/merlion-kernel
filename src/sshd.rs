/// Simplified SSH-2 server for MerlionOS (RFC 4253/4252/4254, stubbed crypto).
/// Authenticates via [`crate::security::authenticate`], provides an interactive
/// remote shell through [`crate::shell::dispatch`].

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use crate::net::{self, Ipv4Addr, ETH_TYPE_IP};
use crate::netstack;
use crate::security;
use crate::tcp_real;
use crate::timer;

// -- SSH message type constants (RFC 4253 / 4252 / 4254) --------------------
/// SSH_MSG_DISCONNECT.
const MSG_DISCONNECT: u8 = 1;
/// SSH_MSG_IGNORE.
const MSG_IGNORE: u8 = 2;
/// SSH_MSG_SERVICE_REQUEST.
const MSG_SERVICE_REQUEST: u8 = 5;
/// SSH_MSG_SERVICE_ACCEPT.
const MSG_SERVICE_ACCEPT: u8 = 6;
/// SSH_MSG_KEXINIT — key-exchange initialisation.
const MSG_KEXINIT: u8 = 20;
/// SSH_MSG_NEWKEYS — switch to new keys.
const MSG_NEWKEYS: u8 = 21;
/// SSH_MSG_USERAUTH_REQUEST.
const MSG_USERAUTH_REQUEST: u8 = 50;
/// SSH_MSG_USERAUTH_FAILURE.
const MSG_USERAUTH_FAILURE: u8 = 51;
/// SSH_MSG_USERAUTH_SUCCESS.
const MSG_USERAUTH_SUCCESS: u8 = 52;
/// SSH_MSG_CHANNEL_OPEN.
const MSG_CHANNEL_OPEN: u8 = 90;
/// SSH_MSG_CHANNEL_OPEN_CONFIRMATION.
const MSG_CHANNEL_OPEN_CONFIRMATION: u8 = 91;
/// SSH_MSG_CHANNEL_WINDOW_ADJUST.
const MSG_CHANNEL_WINDOW_ADJUST: u8 = 93;
/// SSH_MSG_CHANNEL_DATA.
const MSG_CHANNEL_DATA: u8 = 94;
/// SSH_MSG_CHANNEL_EOF.
const MSG_CHANNEL_EOF: u8 = 96;
/// SSH_MSG_CHANNEL_CLOSE.
const MSG_CHANNEL_CLOSE: u8 = 97;
/// SSH_MSG_CHANNEL_REQUEST.
const MSG_CHANNEL_REQUEST: u8 = 98;
/// SSH_MSG_CHANNEL_SUCCESS.
const MSG_CHANNEL_SUCCESS: u8 = 99;

const VERSION_STRING: &[u8] = b"SSH-2.0-MerlionOS\r\n";
const MAX_PAYLOAD: usize = 32768;
const DEFAULT_PORT: u16 = 22;
const RECV_POLL_LIMIT: usize = 500;
const MAX_AUTH_ATTEMPTS: usize = 3;
const CHANNEL_WINDOW: u32 = 0x20_0000;
const CHANNEL_MAX_PKT: u32 = 0x8000;

/// Phases of the SSH protocol as seen by the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Waiting for the client's version string.
    VersionExchange,
    /// Performing (stubbed) key exchange.
    KeyExchange,
    /// Authenticating the remote user.
    Authentication,
    /// Fully authenticated — interactive session.
    Interactive,
}

/// Per-connection SSH session.
pub struct SshSession {
    pub state: SessionState,
    pub socket_id: usize,
    pub username: String,
    pub channel_id: u32,
    pub auth_attempts: usize,
    recv_buf: Vec<u8>,
    keys_set: bool,
    /// DH keypair for this session.
    dh_keypair: Option<crate::crypto_ext::DhKeypair>,
    /// Crypto context (AES-CTR + HMAC) after key exchange.
    crypto: Option<crate::crypto_ext::SshCrypto>,
}

impl SshSession {
    /// Create a new session for the given TCP socket.
    pub fn new(socket_id: usize) -> Self {
        Self {
            state: SessionState::VersionExchange,
            socket_id, username: String::new(), channel_id: 0,
            auth_attempts: MAX_AUTH_ATTEMPTS, recv_buf: Vec::new(), keys_set: false,
            dh_keypair: None, crypto: None,
        }
    }

    /// Encrypt and send an SSH packet (uses AES-CTR if keys are established).
    fn send_packet(&mut self, msg_type: u8, payload: &[u8]) {
        let mut pkt = build_packet(msg_type, payload);
        if let Some(ref mut crypto) = self.crypto {
            // Encrypt the packet (skip first 4 bytes = length field in plaintext)
            let mac = crypto.compute_mac(crypto.seq_send, &pkt[4..]);
            crypto.encrypt_packet(&mut pkt[4..]);
            pkt.extend_from_slice(&mac[..16]); // truncated MAC (16 bytes)
        }
        let _ = tcp_real::send(self.socket_id, &pkt);
    }
}

/// Build an SSH binary packet: uint32 length + byte padding_len + byte type + payload + padding.
fn build_packet(msg_type: u8, payload: &[u8]) -> Vec<u8> {
    let payload_len = 1 + payload.len();
    let mut padding = 8 - ((1 + payload_len) % 8);
    if padding < 4 { padding += 8; }
    let packet_length = 1 + payload_len + padding;
    let mut buf = Vec::with_capacity(4 + packet_length);
    buf.extend_from_slice(&(packet_length as u32).to_be_bytes());
    buf.push(padding as u8);
    buf.push(msg_type);
    buf.extend_from_slice(payload);
    buf.resize(4 + packet_length, 0);
    buf
}

/// Parse one SSH packet from `data`. Returns `(msg_type, payload, bytes_consumed)`.
fn parse_packet(data: &[u8]) -> Option<(u8, Vec<u8>, usize)> {
    if data.len() < 6 { return None; }
    let pkt_len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let total = 4 + pkt_len;
    if data.len() < total || pkt_len < 2 || pkt_len > MAX_PAYLOAD + 256 { return None; }
    let pad_len = data[4] as usize;
    let msg_type = data[5];
    let payload_end = total - pad_len;
    let payload = if payload_end > 6 { data[6..payload_end].to_vec() } else { Vec::new() };
    Some((msg_type, payload, total))
}

/// Encode an SSH string (uint32 length-prefix + bytes).
fn ssh_string(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut v = Vec::with_capacity(4 + b.len());
    v.extend_from_slice(&(b.len() as u32).to_be_bytes());
    v.extend_from_slice(b);
    v
}

/// Decode an SSH string at `offset`. Returns `(string, new_offset)`.
fn read_ssh_string(data: &[u8], off: usize) -> Option<(String, usize)> {
    if off + 4 > data.len() { return None; }
    let len = u32::from_be_bytes([data[off], data[off+1], data[off+2], data[off+3]]) as usize;
    let end = off + 4 + len;
    if end > data.len() { return None; }
    Some((core::str::from_utf8(&data[off+4..end]).unwrap_or("").into(), end))
}

// -- Per-message handlers ---------------------------------------------------
/// SSH_MSG_KEX_DH_INIT / SSH_MSG_KEX_DH_REPLY message types (RFC 4253).
const MSG_KEX_DH_INIT: u8 = 30;
const MSG_KEX_DH_REPLY: u8 = 31;

/// Handle KEXINIT: perform Diffie-Hellman key exchange with AES-128-CTR.
fn handle_kexinit(s: &mut SshSession, _payload: &[u8]) {
    crate::serial_println!("[sshd] KEXINIT received (sock {})", s.socket_id);

    // Generate our DH keypair
    let dh = crate::crypto_ext::dh_generate_keypair();
    crate::serial_println!("[sshd] DH keypair generated (pub={:#x})", dh.public_key);

    // Send our KEXINIT reply with algorithm proposals
    let mut kp = Vec::with_capacity(64);
    // 16-byte random cookie
    let mut cookie = [0u8; 16];
    crate::crypto::random_bytes(&mut cookie);
    kp.extend_from_slice(&cookie);
    // Algorithm name-lists (simplified — advertise our supported algorithms)
    kp.extend_from_slice(&ssh_string("diffie-hellman-group14-sha256").as_slice()); // kex
    kp.extend_from_slice(&ssh_string("ssh-rsa").as_slice());                       // host key
    kp.extend_from_slice(&ssh_string("aes128-ctr").as_slice());                    // enc c→s
    kp.extend_from_slice(&ssh_string("aes128-ctr").as_slice());                    // enc s→c
    kp.extend_from_slice(&ssh_string("hmac-sha2-256").as_slice());                 // mac c→s
    kp.extend_from_slice(&ssh_string("hmac-sha2-256").as_slice());                 // mac s→c
    kp.extend_from_slice(&ssh_string("none").as_slice());                          // compression c→s
    kp.extend_from_slice(&ssh_string("none").as_slice());                          // compression s→c
    kp.extend_from_slice(&0u32.to_be_bytes());                                     // languages c→s
    kp.extend_from_slice(&0u32.to_be_bytes());                                     // languages s→c
    kp.push(0); // first_kex_packet_follows = false
    kp.extend_from_slice(&0u32.to_be_bytes()); // reserved
    let _ = tcp_real::send(s.socket_id, &build_packet(MSG_KEXINIT, &kp));

    // Send our DH public key as KEX_DH_REPLY
    // In proper SSH: server sends host key + DH public + signature
    // Simplified: send DH public key for key agreement
    let mut dh_reply = Vec::with_capacity(64);
    // host key blob (simplified RSA key placeholder)
    let host_key = ssh_string("ssh-rsa");
    dh_reply.extend_from_slice(&(host_key.len() as u32).to_be_bytes());
    dh_reply.extend_from_slice(&host_key);
    // Server DH public value (f)
    dh_reply.extend_from_slice(&(8u32).to_be_bytes());
    dh_reply.extend_from_slice(&dh.public_key.to_be_bytes());
    // Signature (simplified — hash of shared data)
    let sig = crate::crypto::sha256(&dh.public_key.to_be_bytes());
    dh_reply.extend_from_slice(&(sig.len() as u32).to_be_bytes());
    dh_reply.extend_from_slice(&sig);
    let _ = tcp_real::send(s.socket_id, &build_packet(MSG_KEX_DH_REPLY, &dh_reply));

    // Send NEWKEYS
    let _ = tcp_real::send(s.socket_id, &build_packet(MSG_NEWKEYS, &[]));

    // Store DH keypair — shared secret will be computed when we receive client's DH_INIT
    // For now, derive keys from our own public key as a demo
    // (Real SSH: wait for client's e value, compute shared = e^x mod p)
    let shared_secret = crate::crypto_ext::dh_shared_secret(dh.private_key, dh.public_key);
    let crypto = crate::crypto_ext::SshCrypto::from_shared_secret(shared_secret);
    crate::serial_println!("[sshd] DH shared secret derived, AES-128-CTR + HMAC-SHA256 ready");

    s.dh_keypair = Some(dh);
    s.crypto = Some(crypto);
    s.keys_set = true;
    s.state = SessionState::Authentication;
    crate::serial_println!("[sshd] key exchange complete (DH + AES-128-CTR)");
}

/// Handle SERVICE_REQUEST — accept any requested service.
fn handle_service_request(s: &mut SshSession, payload: &[u8]) {
    if let Some((name, _)) = read_ssh_string(payload, 0) {
        crate::serial_println!("[sshd] SERVICE_REQUEST \"{}\"", name);
        let _ = tcp_real::send(s.socket_id, &build_packet(MSG_SERVICE_ACCEPT, &ssh_string(&name)));
    }
}

/// Handle USERAUTH_REQUEST — password method via `crate::security::authenticate`.
fn handle_userauth_request(s: &mut SshSession, payload: &[u8]) {
    let (user, o1) = match read_ssh_string(payload, 0) { Some(v) => v, None => return };
    let (_svc, o2) = match read_ssh_string(payload, o1) { Some(v) => v, None => return };
    let (method, o3) = match read_ssh_string(payload, o2) { Some(v) => v, None => return };
    crate::serial_println!("[sshd] USERAUTH user=\"{}\" method=\"{}\"", user, method);

    if method == "password" {
        let pw_off = if o3 < payload.len() { o3 + 1 } else { o3 };
        if let Some((pw, _)) = read_ssh_string(payload, pw_off) {
            if security::authenticate(&user, security::hash_password(&pw)) {
                crate::serial_println!("[sshd] auth SUCCESS for \"{}\"", user);
                s.username = user;
                s.state = SessionState::Interactive;
                let _ = tcp_real::send(s.socket_id, &build_packet(MSG_USERAUTH_SUCCESS, &[]));
                return;
            }
        }
    }
    s.auth_attempts -= 1;
    crate::serial_println!("[sshd] auth FAILED ({} left)", s.auth_attempts);
    let mut fp = ssh_string("password");
    fp.push(0);
    let _ = tcp_real::send(s.socket_id, &build_packet(MSG_USERAUTH_FAILURE, &fp));
}

/// Handle CHANNEL_OPEN — allocate channel and confirm.
fn handle_channel_open(s: &mut SshSession, payload: &[u8]) {
    let (ctype, o1) = match read_ssh_string(payload, 0) { Some(v) => v, None => return };
    if o1 + 4 > payload.len() { return; }
    let sender_ch = u32::from_be_bytes([payload[o1], payload[o1+1], payload[o1+2], payload[o1+3]]);
    crate::serial_println!("[sshd] CHANNEL_OPEN \"{}\" ch={}", ctype, sender_ch);
    s.channel_id = sender_ch;
    let mut cp = Vec::with_capacity(16);
    cp.extend_from_slice(&sender_ch.to_be_bytes());
    cp.extend_from_slice(&0u32.to_be_bytes());
    cp.extend_from_slice(&CHANNEL_WINDOW.to_be_bytes());
    cp.extend_from_slice(&CHANNEL_MAX_PKT.to_be_bytes());
    let _ = tcp_real::send(s.socket_id, &build_packet(MSG_CHANNEL_OPEN_CONFIRMATION, &cp));
}

/// Handle CHANNEL_REQUEST (pty-req, shell, exec, etc.).
fn handle_channel_request(s: &mut SshSession, payload: &[u8]) {
    if payload.len() < 4 { return; }
    let (rtype, off) = match read_ssh_string(payload, 4) { Some(v) => v, None => return };
    let want_reply = off < payload.len() && payload[off] != 0;
    crate::serial_println!("[sshd] CHANNEL_REQUEST \"{}\"", rtype);
    if want_reply {
        let mut sp = Vec::with_capacity(4);
        sp.extend_from_slice(&s.channel_id.to_be_bytes());
        let _ = tcp_real::send(s.socket_id, &build_packet(MSG_CHANNEL_SUCCESS, &sp));
    }
    if rtype == "shell" {
        let banner = format!("Welcome to MerlionOS SSH ({}@merlion)\r\n$ ", s.username);
        send_channel_data(s, banner.as_bytes());
    }
}

/// Handle CHANNEL_DATA — interactive command processing.
fn handle_channel_data(s: &mut SshSession, payload: &[u8]) {
    if payload.len() < 4 { return; }
    let (data, _) = match read_ssh_string(payload, 4) { Some(v) => v, None => return };
    for ch in data.chars() {
        if ch == '\r' || ch == '\n' {
            let line: String = s.recv_buf.iter().map(|&b| b as char).collect();
            s.recv_buf.clear();
            send_channel_data(s, b"\r\n");
            if !line.is_empty() {
                crate::serial_println!("[sshd] exec: {}", line);
                crate::shell::dispatch(&line);
                let ack = format!("[executed: {}]\r\n$ ", line);
                send_channel_data(s, ack.as_bytes());
            } else {
                send_channel_data(s, b"$ ");
            }
        } else {
            s.recv_buf.push(ch as u8);
            send_channel_data(s, &[ch as u8]);
        }
    }
}

/// Send data to the client over the channel (encrypted if keys established).
fn send_channel_data(s: &mut SshSession, data: &[u8]) {
    let mut p = Vec::with_capacity(8 + data.len());
    p.extend_from_slice(&s.channel_id.to_be_bytes());
    p.extend_from_slice(&(data.len() as u32).to_be_bytes());
    p.extend_from_slice(data);
    s.send_packet(MSG_CHANNEL_DATA, &p);
}

// -- Connection handler -----------------------------------------------------
/// Process a full SSH session on an established TCP socket, driving the
/// state machine from version exchange through interactive shell.
pub fn handle_client(socket_id: usize) {
    crate::serial_println!("[sshd] new connection on socket {}", socket_id);
    let mut session = SshSession::new(socket_id);

    // Version exchange: server speaks first.
    let _ = tcp_real::send(socket_id, VERSION_STRING);
    let mut vbuf = Vec::new();
    for _ in 0..RECV_POLL_LIMIT {
        if let Ok(c) = tcp_real::recv(socket_id) {
            if !c.is_empty() {
                vbuf.extend_from_slice(&c);
                if vbuf.windows(2).any(|w| w == b"\r\n") { break; }
            }
        }
        crate::task::yield_now();
    }
    if let Ok(v) = core::str::from_utf8(&vbuf) {
        crate::serial_println!("[sshd] client version: {}", v.trim());
        if !v.trim().starts_with("SSH-2.0") {
            let _ = tcp_real::close(socket_id);
            return;
        }
    } else {
        let _ = tcp_real::close(socket_id);
        return;
    }
    session.state = SessionState::KeyExchange;

    // Binary packet loop.
    let mut pkt_buf: Vec<u8> = Vec::new();
    let mut idle: usize = 0;
    loop {
        if let Ok(c) = tcp_real::recv(session.socket_id) {
            if !c.is_empty() { pkt_buf.extend_from_slice(&c); idle = 0; }
        }
        while let Some((mt, payload, consumed)) = parse_packet(&pkt_buf) {
            pkt_buf = pkt_buf[consumed..].to_vec();
            match mt {
                MSG_DISCONNECT => {
                    crate::serial_println!("[sshd] client disconnected");
                    let _ = tcp_real::close(session.socket_id); return;
                }
                MSG_IGNORE => {}
                MSG_KEXINIT => handle_kexinit(&mut session, &payload),
                MSG_NEWKEYS => { session.keys_set = true; }
                MSG_SERVICE_REQUEST => handle_service_request(&mut session, &payload),
                MSG_USERAUTH_REQUEST => {
                    handle_userauth_request(&mut session, &payload);
                    if session.auth_attempts == 0 {
                        let _ = tcp_real::send(session.socket_id, &build_packet(MSG_DISCONNECT, &[]));
                        let _ = tcp_real::close(session.socket_id); return;
                    }
                }
                MSG_CHANNEL_OPEN => handle_channel_open(&mut session, &payload),
                MSG_CHANNEL_REQUEST => handle_channel_request(&mut session, &payload),
                MSG_CHANNEL_DATA if session.state == SessionState::Interactive => {
                    handle_channel_data(&mut session, &payload);
                }
                MSG_CHANNEL_EOF | MSG_CHANNEL_CLOSE => {
                    crate::serial_println!("[sshd] channel closed");
                    let mut cp = Vec::with_capacity(4);
                    cp.extend_from_slice(&session.channel_id.to_be_bytes());
                    let _ = tcp_real::send(session.socket_id, &build_packet(MSG_CHANNEL_CLOSE, &cp));
                    let _ = tcp_real::close(session.socket_id); return;
                }
                MSG_CHANNEL_WINDOW_ADJUST => {}
                other => { crate::serial_println!("[sshd] unhandled msg {}", other); }
            }
        }
        idle += 1;
        if idle > RECV_POLL_LIMIT * 4 {
            crate::serial_println!("[sshd] timeout (sock {})", session.socket_id);
            let _ = tcp_real::close(session.socket_id); return;
        }
        crate::task::yield_now();
    }
}

// -- Server listener --------------------------------------------------------
/// Start the SSH daemon on `port`. Accepts connections and hands each to
/// [`handle_client`], yielding between attempts.
pub fn sshd_start(port: u16) {
    let p = if port == 0 { DEFAULT_PORT } else { port };
    let ip = net::NET.lock().ip;
    crate::serial_println!("[sshd] starting on {}:{}", ip, p);
    crate::println!("[sshd] listening on {}:{}", ip, p);
    loop {
        if let Some(sid) = accept_tcp(p) { handle_client(sid); }
        crate::task::yield_now();
    }
}

/// Accept one inbound TCP connection: SYN → SYN-ACK → ACK, then register socket.
fn accept_tcp(port: u16) -> Option<usize> {
    let frame = netstack::poll_rx()?;
    if frame.ethertype != ETH_TYPE_IP { return None; }
    let ip = &frame.payload;
    if ip.len() < 20 || ip[9] != 6 { return None; }
    let ihl = ((ip[0] & 0x0F) as usize) * 4;
    if ip.len() < ihl + 20 { return None; }
    let tcp_data = &ip[ihl..];
    let hdr = tcp_real::parse_tcp_header(tcp_data)?;
    let flags = tcp_real::header_flags(&hdr);
    if hdr.dst_port != port || flags != tcp_real::TCP_SYN { return None; }

    let mut peer_ip = [0u8; 4];
    peer_ip.copy_from_slice(&ip[12..16]);
    let local_ip = net::NET.lock().ip;
    let isn = (timer::ticks().wrapping_mul(2654435761)) as u32;
    let ack_num = hdr.seq.wrapping_add(1);

    let syn_ack = tcp_real::build_tcp_packet(
        local_ip.0, peer_ip, port, hdr.src_port, isn, ack_num,
        tcp_real::TCP_SYN | tcp_real::TCP_ACK, &[],
    );
    netstack::send_ipv4(peer_ip, 6, &syn_ack);

    // Wait for final ACK.
    for _ in 0..200 {
        if let Some(f2) = netstack::poll_rx() {
            if f2.ethertype != ETH_TYPE_IP { continue; }
            let ip2 = &f2.payload;
            if ip2.len() < 20 || ip2[9] != 6 { continue; }
            let ihl2 = ((ip2[0] & 0x0F) as usize) * 4;
            if ip2.len() < ihl2 + 20 { continue; }
            if let Some(h2) = tcp_real::parse_tcp_header(&ip2[ihl2..]) {
                let f = tcp_real::header_flags(&h2);
                if h2.dst_port == port && f & tcp_real::TCP_ACK != 0 && f & tcp_real::TCP_SYN == 0 {
                    let sid = tcp_real::register_established(
                        local_ip, port, Ipv4Addr(peer_ip), hdr.src_port,
                        isn.wrapping_add(1), ack_num,
                    );
                    crate::serial_println!("[sshd] TCP established — sock {}", sid);
                    return Some(sid);
                }
            }
        }
        let t = timer::ticks() + 1;
        while timer::ticks() < t { core::hint::spin_loop(); }
    }
    crate::serial_println!("[sshd] handshake timeout");
    None
}
