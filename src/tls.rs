/// TLS 1.2/1.3 stub/foundation for MerlionOS HTTPS support.
///
/// Provides record framing, ClientHello construction with SNI, ServerHello
/// parsing, and a high-level `https_request` stub. When a real crypto backend
/// is added this scaffolding can be extended into a working implementation.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_println, tcp_real, timer};

// --- TLS content types (RFC 8446 §5.1) ------------------------------------

/// TLS record content type: ChangeCipherSpec.
pub const CONTENT_CHANGE_CIPHER_SPEC: u8 = 20;
/// TLS record content type: Alert.
pub const CONTENT_ALERT: u8 = 21;
/// TLS record content type: Handshake.
pub const CONTENT_HANDSHAKE: u8 = 22;
/// TLS record content type: Application Data.
pub const CONTENT_APPLICATION_DATA: u8 = 23;

// --- TLS version constants ------------------------------------------------

/// TLS 1.2 wire version (0x0303).
pub const TLS_12: u16 = 0x0303;
/// TLS 1.3 wire version (0x0304).
pub const TLS_13: u16 = 0x0304;
/// Legacy version used in TLS 1.3 record headers (same as TLS 1.2).
pub const TLS_LEGACY: u16 = TLS_12;

// --- Cipher suite identifiers (RFC 8446 §B.4) -----------------------------

/// TLS_AES_128_GCM_SHA256 (0x1301).
pub const TLS_AES_128_GCM_SHA256: u16 = 0x1301;
/// TLS_CHACHA20_POLY1305_SHA256 (0x1303).
pub const TLS_CHACHA20_POLY1305_SHA256: u16 = 0x1303;

// --- Handshake & extension types ------------------------------------------

const HANDSHAKE_CLIENT_HELLO: u8 = 1;
const HANDSHAKE_SERVER_HELLO: u8 = 2;
const EXT_SNI: u16 = 0x0000;
const EXT_SUPPORTED_VERSIONS: u16 = 0x002B;

// --- TLS record -----------------------------------------------------------

/// A single TLS record (the outermost framing layer).
#[derive(Debug, Clone)]
pub struct TlsRecord {
    /// Content type (handshake, alert, application data, ...).
    pub content_type: u8,
    /// Protocol version on the wire.
    pub version: u16,
    /// Payload length in bytes.
    pub length: u16,
    /// Record payload.
    pub data: Vec<u8>,
}

impl TlsRecord {
    /// Serialise the record into on-wire bytes (5-byte header + payload).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(5 + self.data.len());
        out.push(self.content_type);
        out.push((self.version >> 8) as u8);
        out.push((self.version & 0xFF) as u8);
        out.push((self.length >> 8) as u8);
        out.push((self.length & 0xFF) as u8);
        out.extend_from_slice(&self.data);
        out
    }
}

// --- TLS ServerHello (parsed) ---------------------------------------------

/// Parsed fields from a TLS ServerHello message.
#[derive(Debug, Clone)]
pub struct TlsServerHello {
    /// Server-selected protocol version.
    pub version: u16,
    /// Server-selected cipher suite.
    pub cipher_suite: u16,
    /// Session ID echoed by the server (may be empty in TLS 1.3).
    pub session_id: Vec<u8>,
}

// --- Pseudo-random bytes (timer-based, NOT cryptographic) -----------------

/// Generate `n` pseudo-random bytes from the PIT tick counter.
/// **Not** cryptographically secure — placeholder until RDRAND / virtio-rng.
fn pseudo_random_bytes(n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n);
    let mut s: u64 = timer::ticks().wrapping_mul(6364136223846793005).wrapping_add(1);
    for _ in 0..n {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        out.push((s >> 33) as u8);
    }
    out
}

// --- ClientHello builder --------------------------------------------------

/// Build a TLS ClientHello message wrapped in a TLS record.
///
/// Includes 32 pseudo-random bytes, an empty session ID, two cipher suites
/// (AES-128-GCM, ChaCha20-Poly1305), SNI with `hostname`, and a Supported
/// Versions extension advertising TLS 1.3.
///
/// **Note:** key-share and signature-algorithm extensions are omitted, so
/// this alone cannot complete a real handshake.
pub fn build_client_hello(hostname: &str) -> Vec<u8> {
    let random = pseudo_random_bytes(32);

    // SNI extension payload
    let name_bytes = hostname.as_bytes();
    let mut sni_ext = Vec::new();
    let sni_list_len = 1 + 2 + name_bytes.len();
    push_u16(&mut sni_ext, sni_list_len as u16);
    sni_ext.push(0x00); // host_name type
    push_u16(&mut sni_ext, name_bytes.len() as u16);
    sni_ext.extend_from_slice(name_bytes);

    // Supported Versions extension payload
    let mut sv_ext = Vec::new();
    sv_ext.push(2); // 1 version x 2 bytes
    push_u16(&mut sv_ext, TLS_13);

    // Combine extensions
    let mut extensions = Vec::new();
    push_u16(&mut extensions, EXT_SNI);
    push_u16(&mut extensions, sni_ext.len() as u16);
    extensions.extend_from_slice(&sni_ext);
    push_u16(&mut extensions, EXT_SUPPORTED_VERSIONS);
    push_u16(&mut extensions, sv_ext.len() as u16);
    extensions.extend_from_slice(&sv_ext);

    // Cipher suites
    let suites: [u16; 2] = [TLS_AES_128_GCM_SHA256, TLS_CHACHA20_POLY1305_SHA256];

    // Assemble ClientHello handshake body
    let mut body = Vec::new();
    push_u16(&mut body, TLS_LEGACY);        // client_version (legacy 0x0303)
    body.extend_from_slice(&random);         // random[32]
    body.push(0);                            // session_id length = 0
    push_u16(&mut body, (suites.len() * 2) as u16);
    for cs in &suites {
        push_u16(&mut body, *cs);
    }
    body.push(1);                            // compression_methods length
    body.push(0);                            // null compression
    push_u16(&mut body, extensions.len() as u16);
    body.extend_from_slice(&extensions);

    // Wrap in handshake header (type + u24 length)
    let mut handshake = Vec::new();
    handshake.push(HANDSHAKE_CLIENT_HELLO);
    push_u24(&mut handshake, body.len() as u32);
    handshake.extend_from_slice(&body);

    // Wrap in TLS record
    let record = TlsRecord {
        content_type: CONTENT_HANDSHAKE,
        version: TLS_LEGACY,
        length: handshake.len() as u16,
        data: handshake,
    };
    record.to_bytes()
}

// --- ServerHello parser ---------------------------------------------------

/// Parse a TLS ServerHello from raw bytes (record header included).
///
/// Returns `None` if the data is too short or the message type is wrong.
/// Only version, cipher suite, and session ID are extracted.
pub fn parse_server_hello(data: &[u8]) -> Option<TlsServerHello> {
    // Min: 5 (record) + 4 (handshake hdr) + 2 (ver) + 32 (random)
    //      + 1 (sid len) + 2 (cipher) + 1 (compression) = 47
    if data.len() < 47 || data[0] != CONTENT_HANDSHAKE {
        return None;
    }
    let hs = &data[5..];
    if hs[0] != HANDSHAKE_SERVER_HELLO {
        return None;
    }
    let body = &hs[4..];
    if body.len() < 35 {
        return None;
    }

    let version = u16::from_be_bytes([body[0], body[1]]);
    let session_id_len = body[34] as usize;
    let after_sid = 35 + session_id_len;
    if body.len() < after_sid + 3 {
        return None;
    }
    let session_id = body[35..after_sid].to_vec();
    let cipher_suite = u16::from_be_bytes([body[after_sid], body[after_sid + 1]]);

    Some(TlsServerHello { version, cipher_suite, session_id })
}

// --- High-level HTTPS stub ------------------------------------------------

/// Attempt an HTTPS request to `host` (port 443) for `path`.
///
/// Performs a real TCP connection and sends a valid ClientHello, but does
/// **not** complete the TLS handshake (no key exchange, certificate
/// validation, or AEAD encryption). Returns an error explaining the gap.
pub fn https_request(host: &str, path: &str) -> Result<Vec<u8>, &'static str> {
    serial_println!("[tls] https_request: connecting to {}:443 for {}", host, path);

    let ip = crate::dns_client::resolve(host).map_err(|_| "tls: DNS resolution failed")?;
    let sock = tcp_real::connect(crate::net::Ipv4Addr(ip), 443)?;

    let client_hello = build_client_hello(host);
    serial_println!("[tls] sending ClientHello ({} bytes) to {}", client_hello.len(), host);
    tcp_real::send(sock, &client_hello)?;

    // Full handshake would continue here:
    //   1. Receive ServerHello + EncryptedExtensions + Certificate + Finished
    //   2. Derive traffic keys via HKDF
    //   3. Send client Finished
    //   4. Encrypt HTTP request with AEAD, decrypt response
    serial_println!("[tls] WARNING: full TLS handshake not yet implemented");
    serial_println!("[tls] use plain HTTP or the AI/LLM proxy for now");

    let _ = tcp_real::close(sock);
    Err("TLS not yet implemented \u{2014} use HTTP or LLM proxy")
}

// --- Subsystem info -------------------------------------------------------

/// Return a human-readable summary of TLS subsystem status.
pub fn info() -> String {
    format!(
        "tls: stub (TLS 1.2/1.3 framing only, no crypto)\n\
         tls: cipher suites: AES-128-GCM-SHA256, ChaCha20-Poly1305\n\
         tls: full handshake: not yet implemented"
    )
}

// --- Helpers --------------------------------------------------------------

/// Append a big-endian u16 to a byte vector.
fn push_u16(buf: &mut Vec<u8>, val: u16) {
    buf.push((val >> 8) as u8);
    buf.push((val & 0xFF) as u8);
}

/// Append a big-endian u24 (3 bytes) to a byte vector.
fn push_u24(buf: &mut Vec<u8>, val: u32) {
    buf.push(((val >> 16) & 0xFF) as u8);
    buf.push(((val >> 8) & 0xFF) as u8);
    buf.push((val & 0xFF) as u8);
}
