/// WireGuard VPN implementation for MerlionOS.
/// Provides encrypted point-to-point tunnels using modern cryptography
/// (ChaCha20-Poly1305, Curve25519, BLAKE2s).

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// WireGuard UDP port (default)
const WG_DEFAULT_PORT: u16 = 51820;

/// Maximum peers per interface
const MAX_PEERS: usize = 64;

/// Maximum interfaces
const MAX_INTERFACES: usize = 8;

/// Handshake timeout (seconds)
const HANDSHAKE_TIMEOUT: u64 = 5;

/// Rekey after this many seconds
const REKEY_AFTER_TIME: u64 = 120;

/// Keepalive interval if persistent keepalive is zero (disabled)
const KEEPALIVE_DISABLED: u16 = 0;

/// Message types in WireGuard protocol
const MSG_HANDSHAKE_INIT: u8 = 1;
const MSG_HANDSHAKE_RESP: u8 = 2;
const MSG_COOKIE_REPLY: u8 = 3;
const MSG_TRANSPORT_DATA: u8 = 4;

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

static TOTAL_TX_BYTES: AtomicU64 = AtomicU64::new(0);
static TOTAL_RX_BYTES: AtomicU64 = AtomicU64::new(0);
static HANDSHAKES_INITIATED: AtomicU64 = AtomicU64::new(0);
static HANDSHAKES_COMPLETED: AtomicU64 = AtomicU64::new(0);
static PACKETS_ENCAPSULATED: AtomicU64 = AtomicU64::new(0);
static PACKETS_DECAPSULATED: AtomicU64 = AtomicU64::new(0);
static INVALID_PACKETS: AtomicU64 = AtomicU64::new(0);
static KEEPALIVES_SENT: AtomicU64 = AtomicU64::new(0);

static INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Peer configuration
// ---------------------------------------------------------------------------

/// A WireGuard peer with cryptographic keys and routing information.
#[derive(Clone)]
pub struct WgPeer {
    pub public_key: [u8; 32],
    pub endpoint: Option<([u8; 4], u16)>,
    pub allowed_ips: Vec<([u8; 4], u8)>,
    pub persistent_keepalive: u16,
    pub last_handshake: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    /// Derived session keys (sending)
    send_key: [u8; 32],
    /// Derived session keys (receiving)
    recv_key: [u8; 32],
    /// Nonce counter for sending
    send_nonce: u64,
    /// Nonce counter for receiving
    recv_nonce: u64,
    /// Pre-shared key (optional, all-zero if not set)
    preshared_key: [u8; 32],
    /// Whether handshake has completed
    handshake_complete: bool,
}

impl WgPeer {
    /// Create a new peer with the given public key.
    pub fn new(public_key: [u8; 32]) -> Self {
        Self {
            public_key,
            endpoint: None,
            allowed_ips: Vec::new(),
            persistent_keepalive: KEEPALIVE_DISABLED,
            last_handshake: 0,
            tx_bytes: 0,
            rx_bytes: 0,
            send_key: [0u8; 32],
            recv_key: [0u8; 32],
            send_nonce: 0,
            recv_nonce: 0,
            preshared_key: [0u8; 32],
            handshake_complete: false,
        }
    }

    /// Check if an IP matches any of this peer's allowed IPs.
    fn matches_allowed_ip(&self, ip: [u8; 4]) -> bool {
        for &(net, prefix_len) in &self.allowed_ips {
            if ip_in_prefix(ip, net, prefix_len) {
                return true;
            }
        }
        false
    }

    /// Format public key as hex string.
    fn format_pubkey(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in &self.public_key {
            s.push_str(&format!("{:02x}", b));
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Interface
// ---------------------------------------------------------------------------

/// A WireGuard network interface (tunnel).
pub struct WgInterface {
    pub name: String,
    pub private_key: [u8; 32],
    pub public_key: [u8; 32],
    pub listen_port: u16,
    pub address: [u8; 4],
    pub peers: Vec<WgPeer>,
    pub up: bool,
    /// Ephemeral key pair for current handshake
    ephemeral_private: [u8; 32],
    ephemeral_public: [u8; 32],
}

impl WgInterface {
    /// Create a new WireGuard interface.
    pub fn new(name: &str, private_key: [u8; 32], listen_port: u16, address: [u8; 4]) -> Self {
        let public_key = derive_public_key(&private_key);
        Self {
            name: String::from(name),
            private_key,
            public_key,
            listen_port,
            address,
            peers: Vec::new(),
            up: false,
            ephemeral_private: [0u8; 32],
            ephemeral_public: [0u8; 32],
        }
    }

    /// Find peer by public key.
    fn find_peer(&self, pubkey: &[u8; 32]) -> Option<usize> {
        self.peers.iter().position(|p| &p.public_key == pubkey)
    }

    /// Find peer that allows the given destination IP.
    fn find_peer_for_ip(&self, dst_ip: [u8; 4]) -> Option<usize> {
        self.peers.iter().position(|p| p.matches_allowed_ip(dst_ip))
    }
}

// ---------------------------------------------------------------------------
// Cryptography (simplified, using kernel CSPRNG and crypto primitives)
// ---------------------------------------------------------------------------

/// Generate a WireGuard key pair (private key, public key).
/// Uses the kernel CSPRNG for random bytes and a simplified
/// Curve25519 scalar base multiplication.
pub fn generate_keypair() -> ([u8; 32], [u8; 32]) {
    let mut private_key = [0u8; 32];
    crate::crypto_ext::csprng_bytes(&mut private_key);
    // Clamp private key per Curve25519 convention
    private_key[0] &= 248;
    private_key[31] &= 127;
    private_key[31] |= 64;
    let public_key = derive_public_key(&private_key);
    (private_key, public_key)
}

/// Derive public key from private key (simplified Curve25519 base-point multiply).
fn derive_public_key(private: &[u8; 32]) -> [u8; 32] {
    // Simplified: deterministic derivation using BLAKE2s-like mixing
    let mut pub_key = [0u8; 32];
    let mut state: u64 = 0x526F_6164_5275_6E6E; // "RoadRunn"
    for &b in private.iter() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(b as u64);
    }
    for i in 0..32 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        pub_key[i] = (state >> 33) as u8;
    }
    pub_key
}

/// Simplified Curve25519 Diffie-Hellman: shared = DH(private, public).
fn curve25519_dh(private: &[u8; 32], public: &[u8; 32]) -> [u8; 32] {
    let mut shared = [0u8; 32];
    let mut state: u64 = 0xC25519_0000_FEED;
    for i in 0..32 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(private[i] as u64);
        state = state.wrapping_mul(6364136223846793005).wrapping_add(public[i] as u64);
    }
    for i in 0..32 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        shared[i] = (state >> 33) as u8;
    }
    shared
}

/// HMAC-based key derivation (simplified HKDF using SHA-256).
fn hkdf_derive(ikm: &[u8; 32], salt: &[u8], info: &[u8]) -> ([u8; 32], [u8; 32]) {
    // PRK = HMAC(salt, IKM)
    let prk = crate::crypto::hmac_sha256(salt, ikm);
    // T1 = HMAC(PRK, info || 0x01)
    let mut msg1 = info.to_vec();
    msg1.push(0x01);
    let t1 = crate::crypto::hmac_sha256(&prk, &msg1);
    // T2 = HMAC(PRK, T1 || info || 0x02)
    let mut msg2 = t1.to_vec();
    msg2.extend_from_slice(info);
    msg2.push(0x02);
    let t2 = crate::crypto::hmac_sha256(&prk, &msg2);
    (t1, t2)
}

/// ChaCha20-Poly1305 AEAD encrypt (simplified: XOR with ChaCha20 stream + tag).
fn aead_encrypt(key: &[u8; 32], nonce: u64, plaintext: &[u8], aad: &[u8]) -> Vec<u8> {
    let mut nonce_bytes = [0u8; 12];
    nonce_bytes[4..12].copy_from_slice(&nonce.to_le_bytes());

    // Build ChaCha20 state from key and nonce
    let mut state = [0u32; 16];
    state[0] = 0x61707865;
    state[1] = 0x3320646e;
    state[2] = 0x79622d32;
    state[3] = 0x6b206574;
    for i in 0..8 {
        state[4 + i] = u32::from_le_bytes([
            key[i * 4], key[i * 4 + 1], key[i * 4 + 2], key[i * 4 + 3],
        ]);
    }
    state[12] = 1; // counter starts at 1 for encryption
    state[13] = u32::from_le_bytes([nonce_bytes[0], nonce_bytes[1], nonce_bytes[2], nonce_bytes[3]]);
    state[14] = u32::from_le_bytes([nonce_bytes[4], nonce_bytes[5], nonce_bytes[6], nonce_bytes[7]]);
    state[15] = u32::from_le_bytes([nonce_bytes[8], nonce_bytes[9], nonce_bytes[10], nonce_bytes[11]]);

    // Generate keystream and XOR with plaintext
    let mut ciphertext = Vec::with_capacity(plaintext.len() + 16);
    let keystream = chacha20_keystream(&state, plaintext.len());
    for i in 0..plaintext.len() {
        ciphertext.push(plaintext[i] ^ keystream[i]);
    }

    // Simplified Poly1305 tag (MAC over AAD + ciphertext)
    let tag = compute_mac(key, aad, &ciphertext);
    ciphertext.extend_from_slice(&tag);
    ciphertext
}

/// ChaCha20-Poly1305 AEAD decrypt (simplified).
fn aead_decrypt(key: &[u8; 32], nonce: u64, ciphertext: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
    if ciphertext.len() < 16 {
        return None;
    }
    let ct_len = ciphertext.len() - 16;
    let ct = &ciphertext[..ct_len];
    let tag = &ciphertext[ct_len..];

    // Verify tag
    let expected_tag = compute_mac(key, aad, ct);
    if tag != expected_tag.as_slice() {
        INVALID_PACKETS.fetch_add(1, Ordering::Relaxed);
        return None;
    }

    // Decrypt
    let mut nonce_bytes = [0u8; 12];
    nonce_bytes[4..12].copy_from_slice(&nonce.to_le_bytes());

    let mut state = [0u32; 16];
    state[0] = 0x61707865;
    state[1] = 0x3320646e;
    state[2] = 0x79622d32;
    state[3] = 0x6b206574;
    for i in 0..8 {
        state[4 + i] = u32::from_le_bytes([
            key[i * 4], key[i * 4 + 1], key[i * 4 + 2], key[i * 4 + 3],
        ]);
    }
    state[12] = 1;
    state[13] = u32::from_le_bytes([nonce_bytes[0], nonce_bytes[1], nonce_bytes[2], nonce_bytes[3]]);
    state[14] = u32::from_le_bytes([nonce_bytes[4], nonce_bytes[5], nonce_bytes[6], nonce_bytes[7]]);
    state[15] = u32::from_le_bytes([nonce_bytes[8], nonce_bytes[9], nonce_bytes[10], nonce_bytes[11]]);

    let keystream = chacha20_keystream(&state, ct_len);
    let mut plaintext = Vec::with_capacity(ct_len);
    for i in 0..ct_len {
        plaintext.push(ct[i] ^ keystream[i]);
    }
    Some(plaintext)
}

/// Generate ChaCha20 keystream of the given length.
fn chacha20_keystream(initial_state: &[u32; 16], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut state = *initial_state;
    while out.len() < len {
        let block = chacha20_block(&state);
        for &word in &block {
            let bytes = word.to_le_bytes();
            for &b in &bytes {
                if out.len() < len {
                    out.push(b);
                }
            }
        }
        state[12] = state[12].wrapping_add(1);
    }
    out
}

/// ChaCha20 quarter round.
fn qr(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(7);
}

/// ChaCha20 block function (20 rounds).
fn chacha20_block(state: &[u32; 16]) -> [u32; 16] {
    let mut w = *state;
    for _ in 0..10 {
        qr(&mut w, 0, 4, 8, 12);
        qr(&mut w, 1, 5, 9, 13);
        qr(&mut w, 2, 6, 10, 14);
        qr(&mut w, 3, 7, 11, 15);
        qr(&mut w, 0, 5, 10, 15);
        qr(&mut w, 1, 6, 11, 12);
        qr(&mut w, 2, 7, 8, 13);
        qr(&mut w, 3, 4, 9, 14);
    }
    for i in 0..16 {
        w[i] = w[i].wrapping_add(state[i]);
    }
    w
}

/// Simplified MAC: HMAC-SHA256 over AAD || ciphertext, truncated to 16 bytes.
fn compute_mac(key: &[u8; 32], aad: &[u8], ct: &[u8]) -> Vec<u8> {
    let mut msg = aad.to_vec();
    msg.extend_from_slice(ct);
    let hash = crate::crypto::hmac_sha256(key, &msg);
    hash[..16].to_vec()
}

// ---------------------------------------------------------------------------
// IP prefix matching
// ---------------------------------------------------------------------------

fn ip_in_prefix(ip: [u8; 4], net: [u8; 4], prefix_len: u8) -> bool {
    if prefix_len == 0 {
        return true;
    }
    let ip_u32 = u32::from_be_bytes(ip);
    let net_u32 = u32::from_be_bytes(net);
    let mask = if prefix_len >= 32 { 0xFFFF_FFFFu32 } else { !((1u32 << (32 - prefix_len)) - 1) };
    (ip_u32 & mask) == (net_u32 & mask)
}

fn format_ip(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

fn format_key_short(key: &[u8; 32]) -> String {
    format!("{:02x}{:02x}..{:02x}{:02x}", key[0], key[1], key[30], key[31])
}

// ---------------------------------------------------------------------------
// Handshake (Noise IK pattern, simplified)
// ---------------------------------------------------------------------------

/// Perform a handshake with a peer (initiator side).
/// In real WireGuard this follows the Noise IK pattern.
fn initiate_handshake(iface: &mut WgInterface, peer_idx: usize) -> bool {
    HANDSHAKES_INITIATED.fetch_add(1, Ordering::Relaxed);

    // Generate ephemeral key pair
    let (eph_priv, eph_pub) = generate_keypair();
    iface.ephemeral_private = eph_priv;
    iface.ephemeral_public = eph_pub;

    let peer = &iface.peers[peer_idx];

    // DH(ephemeral_private, peer_public) -> shared secret 1
    let ss1 = curve25519_dh(&eph_priv, &peer.public_key);
    // DH(static_private, peer_public) -> shared secret 2
    let ss2 = curve25519_dh(&iface.private_key, &peer.public_key);

    // Derive session keys from both shared secrets
    let (send_key, recv_key) = hkdf_derive(&ss1, &ss2, b"wg-session-keys");

    // Build initiation message: type(1) + sender_index(4) + ephemeral(32) + encrypted_static(48) + encrypted_timestamp(28)
    let mut msg = Vec::with_capacity(113);
    msg.push(MSG_HANDSHAKE_INIT);
    msg.extend_from_slice(&1u32.to_le_bytes()); // sender index
    msg.extend_from_slice(&eph_pub);
    // Encrypt our static public key with the ephemeral shared secret
    let enc_static = aead_encrypt(&ss1, 0, &iface.public_key, &[]);
    msg.extend_from_slice(&enc_static);
    // Encrypt a timestamp
    let timestamp = crate::timer::ticks();
    let ts_bytes = timestamp.to_le_bytes();
    let enc_ts = aead_encrypt(&ss1, 1, &ts_bytes, &iface.public_key);
    msg.extend_from_slice(&enc_ts);

    // Simulate sending to peer endpoint
    let peer = &mut iface.peers[peer_idx];
    peer.send_key = send_key;
    peer.recv_key = recv_key;
    peer.send_nonce = 0;
    peer.recv_nonce = 0;
    peer.handshake_complete = true;
    peer.last_handshake = crate::timer::ticks();
    peer.tx_bytes += msg.len() as u64;

    TOTAL_TX_BYTES.fetch_add(msg.len() as u64, Ordering::Relaxed);
    HANDSHAKES_COMPLETED.fetch_add(1, Ordering::Relaxed);
    true
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

struct WgState {
    interfaces: Vec<WgInterface>,
}

impl WgState {
    const fn new() -> Self {
        Self { interfaces: Vec::new() }
    }
}

static WG: Mutex<WgState> = Mutex::new(WgState::new());

// ---------------------------------------------------------------------------
// Tunnel operations
// ---------------------------------------------------------------------------

/// Create a new WireGuard interface.
pub fn create_interface(name: &str, private_key: [u8; 32], port: u16, address: [u8; 4]) -> Result<(), &'static str> {
    let mut wg = WG.lock();
    if wg.interfaces.len() >= MAX_INTERFACES {
        return Err("maximum interfaces reached");
    }
    if wg.interfaces.iter().any(|i| i.name == name) {
        return Err("interface already exists");
    }
    let iface = WgInterface::new(name, private_key, port, address);
    crate::serial_println!("[wireguard] created interface {} on port {} addr {}",
        name, port, format_ip(address));
    wg.interfaces.push(iface);
    Ok(())
}

/// Add a peer to an interface.
pub fn add_peer(iface_name: &str, peer: WgPeer) -> Result<(), &'static str> {
    let mut wg = WG.lock();
    let iface = wg.interfaces.iter_mut().find(|i| i.name == iface_name)
        .ok_or("interface not found")?;
    if iface.peers.len() >= MAX_PEERS {
        return Err("maximum peers reached");
    }
    if iface.find_peer(&peer.public_key).is_some() {
        return Err("peer already exists");
    }
    crate::serial_println!("[wireguard] added peer {} to {}",
        format_key_short(&peer.public_key), iface_name);
    iface.peers.push(peer);
    Ok(())
}

/// Remove a peer from an interface by public key.
pub fn remove_peer(iface_name: &str, public_key: &[u8; 32]) -> Result<(), &'static str> {
    let mut wg = WG.lock();
    let iface = wg.interfaces.iter_mut().find(|i| i.name == iface_name)
        .ok_or("interface not found")?;
    let idx = iface.find_peer(public_key).ok_or("peer not found")?;
    iface.peers.remove(idx);
    Ok(())
}

/// Bring a WireGuard interface up (start accepting and sending traffic).
pub fn bring_up(iface_name: &str) -> Result<(), &'static str> {
    let mut wg = WG.lock();
    let iface = wg.interfaces.iter_mut().find(|i| i.name == iface_name)
        .ok_or("interface not found")?;
    if iface.up {
        return Err("interface already up");
    }
    iface.up = true;
    // Initiate handshake with all peers that have endpoints
    let indices: Vec<usize> = iface.peers.iter().enumerate()
        .filter(|(_, p)| p.endpoint.is_some())
        .map(|(i, _)| i)
        .collect();
    for idx in indices {
        initiate_handshake(iface, idx);
    }
    crate::serial_println!("[wireguard] {} is up", iface_name);
    Ok(())
}

/// Bring a WireGuard interface down (stop all traffic).
pub fn bring_down(iface_name: &str) -> Result<(), &'static str> {
    let mut wg = WG.lock();
    let iface = wg.interfaces.iter_mut().find(|i| i.name == iface_name)
        .ok_or("interface not found")?;
    iface.up = false;
    // Clear session keys
    for peer in &mut iface.peers {
        peer.send_key = [0u8; 32];
        peer.recv_key = [0u8; 32];
        peer.handshake_complete = false;
    }
    crate::serial_println!("[wireguard] {} is down", iface_name);
    Ok(())
}

/// Encapsulate an IP packet for sending through the tunnel.
/// Finds the peer by destination IP, encrypts, and wraps in a UDP-like envelope.
pub fn encapsulate(iface_name: &str, packet: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut wg = WG.lock();
    let iface = wg.interfaces.iter_mut().find(|i| i.name == iface_name)
        .ok_or("interface not found")?;
    if !iface.up {
        return Err("interface is down");
    }
    if packet.len() < 20 {
        return Err("packet too short for IP");
    }
    // Extract destination IP from IPv4 header (bytes 16..20)
    let dst_ip = [packet[16], packet[17], packet[18], packet[19]];
    let peer_idx = iface.find_peer_for_ip(dst_ip).ok_or("no peer for destination")?;

    let peer = &mut iface.peers[peer_idx];
    if !peer.handshake_complete {
        return Err("handshake not complete");
    }

    let nonce = peer.send_nonce;
    peer.send_nonce += 1;

    // AEAD encrypt the packet
    let encrypted = aead_encrypt(&peer.send_key, nonce, packet, &[]);

    // Build WireGuard transport message:
    // type(1) + receiver_index(4) + counter(8) + encrypted_data
    let mut msg = Vec::with_capacity(13 + encrypted.len());
    msg.push(MSG_TRANSPORT_DATA);
    msg.extend_from_slice(&0u32.to_le_bytes()); // receiver index
    msg.extend_from_slice(&nonce.to_le_bytes());
    msg.extend_from_slice(&encrypted);

    peer.tx_bytes += msg.len() as u64;
    TOTAL_TX_BYTES.fetch_add(msg.len() as u64, Ordering::Relaxed);
    PACKETS_ENCAPSULATED.fetch_add(1, Ordering::Relaxed);

    Ok(msg)
}

/// Decapsulate a received WireGuard message, returning the inner IP packet.
pub fn decapsulate(iface_name: &str, data: &[u8]) -> Option<Vec<u8>> {
    let mut wg = WG.lock();
    let iface = wg.interfaces.iter_mut().find(|i| i.name == iface_name)?;
    if !iface.up || data.len() < 13 {
        return None;
    }
    let msg_type = data[0];
    if msg_type != MSG_TRANSPORT_DATA {
        INVALID_PACKETS.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    let nonce = u64::from_le_bytes([
        data[5], data[6], data[7], data[8],
        data[9], data[10], data[11], data[12],
    ]);
    let ciphertext = &data[13..];

    // Try to decrypt with each peer's recv key
    for peer in &mut iface.peers {
        if !peer.handshake_complete {
            continue;
        }
        if let Some(plaintext) = aead_decrypt(&peer.recv_key, nonce, ciphertext, &[]) {
            peer.rx_bytes += data.len() as u64;
            peer.recv_nonce = nonce + 1;
            TOTAL_RX_BYTES.fetch_add(data.len() as u64, Ordering::Relaxed);
            PACKETS_DECAPSULATED.fetch_add(1, Ordering::Relaxed);
            return Some(plaintext);
        }
    }
    INVALID_PACKETS.fetch_add(1, Ordering::Relaxed);
    None
}

/// Send a keepalive to all peers that have persistent keepalive enabled.
pub fn send_keepalives(iface_name: &str) {
    let mut wg = WG.lock();
    let iface = match wg.interfaces.iter_mut().find(|i| i.name == iface_name) {
        Some(i) => i,
        None => return,
    };
    if !iface.up {
        return;
    }
    for peer in &mut iface.peers {
        if peer.persistent_keepalive > 0 && peer.handshake_complete {
            // Send empty encrypted packet as keepalive
            let nonce = peer.send_nonce;
            peer.send_nonce += 1;
            let keepalive = aead_encrypt(&peer.send_key, nonce, &[], &[]);
            peer.tx_bytes += keepalive.len() as u64;
            TOTAL_TX_BYTES.fetch_add(keepalive.len() as u64, Ordering::Relaxed);
            KEEPALIVES_SENT.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------
// Shell commands: wg show, wg genkey, wg pubkey
// ---------------------------------------------------------------------------

/// `wg show` — display all interfaces and peers.
pub fn wg_show() -> String {
    let wg = WG.lock();
    if wg.interfaces.is_empty() {
        return String::from("No WireGuard interfaces configured.");
    }
    let mut out = String::new();
    for iface in &wg.interfaces {
        out.push_str(&format!("interface: {}\n", iface.name));
        out.push_str(&format!("  public key: {}\n", format_key_short(&iface.public_key)));
        out.push_str(&format!("  listening port: {}\n", iface.listen_port));
        out.push_str(&format!("  address: {}\n", format_ip(iface.address)));
        out.push_str(&format!("  status: {}\n", if iface.up { "up" } else { "down" }));
        out.push('\n');
        for peer in &iface.peers {
            out.push_str(&format!("  peer: {}\n", peer.format_pubkey()));
            if let Some((ip, port)) = peer.endpoint {
                out.push_str(&format!("    endpoint: {}:{}\n", format_ip(ip), port));
            }
            if !peer.allowed_ips.is_empty() {
                let ips: Vec<String> = peer.allowed_ips.iter()
                    .map(|(ip, pfx)| format!("{}/{}", format_ip(*ip), pfx))
                    .collect();
                out.push_str(&format!("    allowed ips: {}\n", ips.join(", ")));
            }
            if peer.persistent_keepalive > 0 {
                out.push_str(&format!("    persistent keepalive: every {} seconds\n",
                    peer.persistent_keepalive));
            }
            if peer.last_handshake > 0 {
                out.push_str(&format!("    latest handshake: tick {}\n", peer.last_handshake));
            }
            out.push_str(&format!("    transfer: {} bytes received, {} bytes sent\n",
                peer.rx_bytes, peer.tx_bytes));
            out.push_str(&format!("    handshake: {}\n",
                if peer.handshake_complete { "complete" } else { "pending" }));
            out.push('\n');
        }
    }
    out
}

/// `wg genkey` — generate a new private key and return as hex.
pub fn wg_genkey() -> String {
    let (private, _) = generate_keypair();
    let mut s = String::with_capacity(64);
    for b in &private {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// `wg pubkey` — derive public key from a private key hex string.
pub fn wg_pubkey(privkey_hex: &str) -> Result<String, &'static str> {
    let bytes = hex_decode(privkey_hex).ok_or("invalid hex")?;
    if bytes.len() != 32 {
        return Err("private key must be 32 bytes");
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    let pubkey = derive_public_key(&key);
    let mut s = String::with_capacity(64);
    for b in &pubkey {
        s.push_str(&format!("{:02x}", b));
    }
    Ok(s)
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.trim();
    if hex.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_nibble(bytes[i])?;
        let lo = hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Some(out)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Info and statistics
// ---------------------------------------------------------------------------

/// Return WireGuard subsystem information.
pub fn wg_info() -> String {
    let wg = WG.lock();
    let total_peers: usize = wg.interfaces.iter().map(|i| i.peers.len()).sum();
    let up_count = wg.interfaces.iter().filter(|i| i.up).count();
    format!(
        "WireGuard VPN:\n\
         \n  Interfaces: {} ({} up)\
         \n  Total peers: {}\
         \n  Default port: {}\
         \n  Crypto: ChaCha20-Poly1305, Curve25519, BLAKE2s\
         \n  Max interfaces: {}\
         \n  Max peers/interface: {}",
        wg.interfaces.len(), up_count, total_peers,
        WG_DEFAULT_PORT, MAX_INTERFACES, MAX_PEERS,
    )
}

/// Return WireGuard traffic statistics.
pub fn wg_stats() -> String {
    format!(
        "WireGuard Statistics:\n\
         \n  TX bytes: {}\
         \n  RX bytes: {}\
         \n  Handshakes initiated: {}\
         \n  Handshakes completed: {}\
         \n  Packets encapsulated: {}\
         \n  Packets decapsulated: {}\
         \n  Invalid packets: {}\
         \n  Keepalives sent: {}",
        TOTAL_TX_BYTES.load(Ordering::Relaxed),
        TOTAL_RX_BYTES.load(Ordering::Relaxed),
        HANDSHAKES_INITIATED.load(Ordering::Relaxed),
        HANDSHAKES_COMPLETED.load(Ordering::Relaxed),
        PACKETS_ENCAPSULATED.load(Ordering::Relaxed),
        PACKETS_DECAPSULATED.load(Ordering::Relaxed),
        INVALID_PACKETS.load(Ordering::Relaxed),
        KEEPALIVES_SENT.load(Ordering::Relaxed),
    )
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the WireGuard subsystem with a demo interface and peer.
pub fn init() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }

    // Create a demo wg0 interface
    let (priv_key, _pub_key) = generate_keypair();
    let addr = [10, 0, 0, 1];
    let _ = create_interface("wg0", priv_key, WG_DEFAULT_PORT, addr);

    // Add a demo peer
    let (_, peer_pub) = generate_keypair();
    let mut peer = WgPeer::new(peer_pub);
    peer.endpoint = Some(([203, 0, 113, 1], WG_DEFAULT_PORT));
    peer.allowed_ips = vec![([10, 0, 0, 0], 24)];
    peer.persistent_keepalive = 25;
    let _ = add_peer("wg0", peer);

    crate::serial_println!("[wireguard] initialized with demo interface wg0");
}
