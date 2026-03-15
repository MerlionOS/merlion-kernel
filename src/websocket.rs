/// WebSocket protocol implementation for MerlionOS.
///
/// Implements the WebSocket framing protocol (RFC 6455) with frame parsing,
/// frame building, HTTP upgrade handshake, and a connection abstraction.
/// Uses `no_std` compatible patterns with the `alloc` crate.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// WebSocket GUID used during the opening handshake (RFC 6455 Section 4.2.2).
const WS_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-5AB9DC11045A";
/// Maximum payload length we accept in a single frame (64 KiB).
const MAX_PAYLOAD_LEN: u64 = 64 * 1024;

/// WebSocket frame opcodes (RFC 6455 Section 5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Opcode {
    /// Continuation frame.
    Continuation = 0,
    /// UTF-8 text frame.
    Text = 1,
    /// Binary data frame.
    Binary = 2,
    /// Connection close control frame.
    Close = 8,
    /// Ping control frame.
    Ping = 9,
    /// Pong control frame.
    Pong = 10,
}

impl Opcode {
    /// Try to convert a raw `u8` value into a known [`Opcode`].
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::Continuation),
            1 => Some(Self::Text),
            2 => Some(Self::Binary),
            8 => Some(Self::Close),
            9 => Some(Self::Ping),
            10 => Some(Self::Pong),
            _ => None,
        }
    }

    /// Returns `true` for control opcodes (Close, Ping, Pong).
    pub fn is_control(self) -> bool {
        (self as u8) >= 8
    }
}

/// A parsed WebSocket frame.
#[derive(Debug, Clone)]
pub struct WsFrame {
    /// FIN bit — `true` if this is the final fragment.
    pub fin: bool,
    /// Frame opcode.
    pub opcode: Opcode,
    /// Whether the payload is masked.
    pub mask: bool,
    /// Decoded payload length.
    pub payload_len: u64,
    /// Four-byte masking key (all zeros when `mask` is `false`).
    pub masking_key: [u8; 4],
    /// Unmasked payload data.
    pub payload: Vec<u8>,
}

/// Parse a single WebSocket frame from `data`.
///
/// Returns `Some((frame, bytes_consumed))` on success, or `None` if the
/// buffer does not yet contain a complete frame.
pub fn parse_frame(data: &[u8]) -> Option<(WsFrame, usize)> {
    if data.len() < 2 {
        return None;
    }
    let (b0, b1) = (data[0], data[1]);
    let fin = (b0 & 0x80) != 0;
    let opcode = Opcode::from_u8(b0 & 0x0F)?;
    let mask = (b1 & 0x80) != 0;
    let len7 = (b1 & 0x7F) as u64;
    let mut off: usize = 2;

    let payload_len = if len7 <= 125 {
        len7
    } else if len7 == 126 {
        if data.len() < off + 2 { return None; }
        let l = u16::from_be_bytes([data[off], data[off + 1]]) as u64;
        off += 2;
        l
    } else {
        if data.len() < off + 8 { return None; }
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&data[off..off + 8]);
        off += 8;
        u64::from_be_bytes(buf)
    };
    if payload_len > MAX_PAYLOAD_LEN { return None; }

    let mut masking_key = [0u8; 4];
    if mask {
        if data.len() < off + 4 { return None; }
        masking_key.copy_from_slice(&data[off..off + 4]);
        off += 4;
    }
    let plen = payload_len as usize;
    if data.len() < off + plen { return None; }

    let mut payload = vec![0u8; plen];
    payload.copy_from_slice(&data[off..off + plen]);
    if mask { apply_mask(&mut payload, &masking_key); }

    Some((WsFrame { fin, opcode, mask, payload_len, masking_key, payload }, off + plen))
}

/// XOR-mask (or unmask) `data` in-place using the four-byte `key`.
fn apply_mask(data: &mut [u8], key: &[u8; 4]) {
    for (i, byte) in data.iter_mut().enumerate() {
        *byte ^= key[i & 3];
    }
}

/// Build a complete WebSocket frame from the given components.
///
/// If `mask` is `true` the payload is masked with a deterministic key
/// (`MERL`); a real client implementation would use a random key.
pub fn build_frame(opcode: Opcode, payload: &[u8], mask: bool) -> Vec<u8> {
    let plen = payload.len();
    let ext = if plen <= 125 { 0 } else if plen <= 0xFFFF { 2 } else { 8 };
    let mlen = if mask { 4 } else { 0 };
    let mut buf = Vec::with_capacity(2 + ext + mlen + plen);

    buf.push(0x80 | (opcode as u8)); // FIN + opcode
    let mb: u8 = if mask { 0x80 } else { 0x00 };
    if plen <= 125 {
        buf.push(mb | (plen as u8));
    } else if plen <= 0xFFFF {
        buf.push(mb | 126);
        buf.extend_from_slice(&(plen as u16).to_be_bytes());
    } else {
        buf.push(mb | 127);
        buf.extend_from_slice(&(plen as u64).to_be_bytes());
    }

    if mask {
        let key: [u8; 4] = [0x4D, 0x45, 0x52, 0x4C]; // "MERL"
        buf.extend_from_slice(&key);
        let mut masked = Vec::from(payload);
        apply_mask(&mut masked, &key);
        buf.extend_from_slice(&masked);
    } else {
        buf.extend_from_slice(payload);
    }
    buf
}

// ─── Base64 encoder (minimal, no_std) ────────────────────────────────────────

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode `input` as a Base64 [`String`] (minimal no_std implementation).
fn base64_encode(input: &[u8]) -> String {
    let mut out = Vec::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[((triple >> 18) & 0x3F) as usize]);
        out.push(B64[((triple >> 12) & 0x3F) as usize]);
        out.push(if chunk.len() > 1 { B64[((triple >> 6) & 0x3F) as usize] } else { b'=' });
        out.push(if chunk.len() > 2 { B64[(triple & 0x3F) as usize] } else { b'=' });
    }
    // SAFETY: Base64 output is always valid ASCII.
    unsafe { String::from_utf8_unchecked(out) }
}

// ─── SHA-1 (RFC 3174, not for crypto — only for Sec-WebSocket-Accept) ────────

/// Minimal SHA-1 digest for the WebSocket handshake.
fn sha1(data: &[u8]) -> [u8; 20] {
    let (mut h0, mut h1, mut h2, mut h3, mut h4) =
        (0x67452301u32, 0xEFCDAB89u32, 0x98BADCFEu32, 0x10325476u32, 0xC3D2E1F0u32);
    let bit_len = (data.len() as u64) * 8;
    let mut msg = Vec::from(data);
    msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0); }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            let o = i * 4;
            w[i] = u32::from_be_bytes([block[o], block[o+1], block[o+2], block[o+3]]);
        }
        for i in 16..80 {
            w[i] = (w[i-3] ^ w[i-8] ^ w[i-14] ^ w[i-16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for i in 0..80 {
            let (f, k) = match i {
                0..=19  => ((b & c) | ((!b) & d),         0x5A827999u32),
                20..=39 => (b ^ c ^ d,                    0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d),  0x8F1BBCDCu32),
                _       => (b ^ c ^ d,                    0xCA62C1D6u32),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e)
                     .wrapping_add(k).wrapping_add(w[i]);
            e = d; d = c; c = b.rotate_left(30); b = a; a = t;
        }
        h0 = h0.wrapping_add(a); h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c); h3 = h3.wrapping_add(d); h4 = h4.wrapping_add(e);
    }
    let mut d = [0u8; 20];
    d[0..4].copy_from_slice(&h0.to_be_bytes());   d[4..8].copy_from_slice(&h1.to_be_bytes());
    d[8..12].copy_from_slice(&h2.to_be_bytes());   d[12..16].copy_from_slice(&h3.to_be_bytes());
    d[16..20].copy_from_slice(&h4.to_be_bytes());
    d
}

// ─── Upgrade handshake ───────────────────────────────────────────────────────

/// Build an HTTP 101 Switching Protocols response for the WebSocket upgrade.
///
/// Locates the `Sec-WebSocket-Key` header in `request_headers`, concatenates
/// the WebSocket GUID, computes SHA-1, and returns the full HTTP response.
/// Returns an empty `Vec` if the key header is not found.
pub fn upgrade_handshake(request_headers: &[u8]) -> Vec<u8> {
    let hdr = match core::str::from_utf8(request_headers) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let key = hdr.lines().find_map(|line| {
        if line.to_ascii_lowercase().starts_with("sec-websocket-key:") {
            Some(line.split_once(':')?.1.trim())
        } else {
            None
        }
    });
    let key = match key {
        Some(k) => k,
        None => return Vec::new(),
    };
    let mut concat = Vec::with_capacity(key.len() + WS_GUID.len());
    concat.extend_from_slice(key.as_bytes());
    concat.extend_from_slice(WS_GUID);
    let accept = base64_encode(&sha1(&concat));
    alloc::format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\r\n"
    ).into_bytes()
}

// ─── WsConnection ────────────────────────────────────────────────────────────

/// Represents one side of a WebSocket connection.
///
/// Collects outgoing frames in an internal buffer which the caller can drain
/// via [`WsConnection::take_outgoing`] and feed to the underlying transport.
pub struct WsConnection {
    outgoing: Vec<u8>,
    is_client: bool,
    closed: bool,
}

impl WsConnection {
    /// Create a new connection. Set `is_client` to mask outgoing frames.
    pub fn new(is_client: bool) -> Self {
        Self { outgoing: Vec::new(), is_client, closed: false }
    }

    /// Enqueue a UTF-8 text frame.
    pub fn send_text(&mut self, text: &str) {
        if self.closed { return; }
        self.outgoing.extend_from_slice(&build_frame(Opcode::Text, text.as_bytes(), self.is_client));
    }

    /// Enqueue a binary data frame.
    pub fn send_binary(&mut self, data: &[u8]) {
        if self.closed { return; }
        self.outgoing.extend_from_slice(&build_frame(Opcode::Binary, data, self.is_client));
    }

    /// Enqueue a Ping frame with optional payload.
    pub fn send_ping(&mut self, payload: &[u8]) {
        if self.closed { return; }
        self.outgoing.extend_from_slice(&build_frame(Opcode::Ping, payload, self.is_client));
    }

    /// Enqueue a Close frame with an optional status code.
    pub fn send_close(&mut self, status_code: Option<u16>) {
        if self.closed { return; }
        let body = match status_code {
            Some(code) => Vec::from(code.to_be_bytes().as_slice()),
            None => Vec::new(),
        };
        self.outgoing.extend_from_slice(&build_frame(Opcode::Close, &body, self.is_client));
        self.closed = true;
    }

    /// Drain all buffered outgoing bytes.
    pub fn take_outgoing(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.outgoing)
    }

    /// Returns `true` if a Close frame has been sent.
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}
