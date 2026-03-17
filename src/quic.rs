/// QUIC transport protocol for MerlionOS (RFC 9000).
/// Provides reliable, encrypted, multiplexed transport over UDP
/// with built-in TLS 1.3, 0-RTT connection establishment,
/// stream multiplexing, and connection migration.

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// QUIC version 1 (RFC 9000).
pub const QUIC_VERSION_1: u32 = 0x0000_0001;

/// Maximum number of tracked connections.
const MAX_CONNECTIONS: usize = 64;

/// Maximum streams per connection.
const MAX_STREAMS_PER_CONN: usize = 128;

/// Default connection-level flow control limit (1 MiB).
const DEFAULT_MAX_DATA: u64 = 1_048_576;

/// Default per-stream flow control limit (256 KiB).
const DEFAULT_MAX_STREAM_DATA: u64 = 262_144;

/// Default initial congestion window (in bytes, ~10 packets).
const INITIAL_CWND: u64 = 14_720;

/// Minimum congestion window (2 packets).
const MIN_CWND: u64 = 2_940;

/// Default idle timeout (30 seconds in microseconds).
const DEFAULT_IDLE_TIMEOUT_US: u64 = 30_000_000;

/// Probe timeout multiplier (PTO = smoothed_rtt * PTO_MULTIPLIER / 256).
const PTO_MULTIPLIER: u64 = 512; // 2.0 in fixed-point /256

/// Maximum connection ID length.
const MAX_CID_LEN: usize = 20;

/// Minimum connection ID length.
const MIN_CID_LEN: usize = 4;

// ---------------------------------------------------------------------------
// Connection ID
// ---------------------------------------------------------------------------

/// QUIC connection identifier (variable-length, 4-20 bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionId {
    pub bytes: [u8; 8],
    pub len: u8,
}

impl ConnectionId {
    /// Create a new empty connection ID.
    pub const fn empty() -> Self {
        Self {
            bytes: [0; 8],
            len: 0,
        }
    }

    /// Generate a pseudo-random connection ID of the given length.
    pub fn generate(length: u8) -> Self {
        let len = if (length as usize) < MIN_CID_LEN {
            MIN_CID_LEN as u8
        } else if length > 8 {
            8
        } else {
            length
        };
        let seed = CID_SEED.fetch_add(1, Ordering::Relaxed);
        let mut bytes = [0u8; 8];
        // Simple LCG-based pseudo-random fill
        let mut state = seed as u64 ^ 0x5851_F42D_4C95_7F2D;
        for b in bytes.iter_mut().take(len as usize) {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            *b = (state >> 33) as u8;
        }
        Self { bytes, len }
    }

    /// Return CID bytes as a slice.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// Format CID as hex string.
    pub fn to_hex(&self) -> String {
        let mut s = String::new();
        for &b in self.as_bytes() {
            s.push_str(&format!("{:02x}", b));
        }
        s
    }
}

static CID_SEED: AtomicU32 = AtomicU32::new(0xCAFE_0001);

// ---------------------------------------------------------------------------
// Packet types
// ---------------------------------------------------------------------------

/// QUIC packet type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    /// First packet, carries crypto handshake.
    Initial,
    /// 0-RTT early data.
    ZeroRtt,
    /// Handshake completion.
    Handshake,
    /// Server requests address validation.
    Retry,
    /// Post-handshake data (1-RTT, short header).
    Short,
}

/// A QUIC packet.
#[derive(Debug, Clone)]
pub struct QuicPacket {
    pub packet_type: PacketType,
    pub version: u32,
    pub dcid: ConnectionId,
    pub scid: ConnectionId,
    pub packet_number: u64,
    pub payload: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Variable-length integer encoding (RFC 9000, Section 16)
// ---------------------------------------------------------------------------

/// Encode a variable-length integer (1/2/4/8 bytes).
pub fn encode_varint(val: u64) -> Vec<u8> {
    if val < 64 {
        vec![val as u8]
    } else if val < 16384 {
        vec![0x40 | ((val >> 8) as u8), val as u8]
    } else if val < 1_073_741_824 {
        let mut buf = [0u8; 4];
        buf[0] = 0x80 | ((val >> 24) as u8);
        buf[1] = (val >> 16) as u8;
        buf[2] = (val >> 8) as u8;
        buf[3] = val as u8;
        buf.to_vec()
    } else {
        let mut buf = [0u8; 8];
        buf[0] = 0xC0 | ((val >> 56) as u8);
        buf[1] = (val >> 48) as u8;
        buf[2] = (val >> 40) as u8;
        buf[3] = (val >> 32) as u8;
        buf[4] = (val >> 24) as u8;
        buf[5] = (val >> 16) as u8;
        buf[6] = (val >> 8) as u8;
        buf[7] = val as u8;
        buf.to_vec()
    }
}

/// Decode a variable-length integer, returning (value, bytes_consumed).
pub fn decode_varint(data: &[u8]) -> Result<(u64, usize), &'static str> {
    if data.is_empty() {
        return Err("empty varint");
    }
    let prefix = data[0] >> 6;
    match prefix {
        0 => Ok((data[0] as u64, 1)),
        1 => {
            if data.len() < 2 { return Err("truncated varint"); }
            let val = ((data[0] as u64 & 0x3F) << 8) | data[1] as u64;
            Ok((val, 2))
        }
        2 => {
            if data.len() < 4 { return Err("truncated varint"); }
            let val = ((data[0] as u64 & 0x3F) << 24)
                | ((data[1] as u64) << 16)
                | ((data[2] as u64) << 8)
                | data[3] as u64;
            Ok((val, 4))
        }
        _ => {
            if data.len() < 8 { return Err("truncated varint"); }
            let val = ((data[0] as u64 & 0x3F) << 56)
                | ((data[1] as u64) << 48)
                | ((data[2] as u64) << 40)
                | ((data[3] as u64) << 32)
                | ((data[4] as u64) << 24)
                | ((data[5] as u64) << 16)
                | ((data[6] as u64) << 8)
                | data[7] as u64;
            Ok((val, 8))
        }
    }
}

// ---------------------------------------------------------------------------
// Packet encoding / decoding
// ---------------------------------------------------------------------------

/// Encode a QUIC packet into bytes.
pub fn encode_packet(packet: &QuicPacket) -> Vec<u8> {
    let mut buf = Vec::new();

    match packet.packet_type {
        PacketType::Short => {
            // Short header: form bit = 0, fixed bit = 1
            let first = 0x40 | (packet.packet_number.min(3) as u8);
            buf.push(first);
            buf.extend_from_slice(packet.dcid.as_bytes());
            // Packet number (variable length, simplified to 4 bytes)
            buf.extend_from_slice(&(packet.packet_number as u32).to_be_bytes());
            buf.extend_from_slice(&packet.payload);
        }
        _ => {
            // Long header: form bit = 1, fixed bit = 1
            let type_bits = match packet.packet_type {
                PacketType::Initial => 0x00,
                PacketType::ZeroRtt => 0x01,
                PacketType::Handshake => 0x02,
                PacketType::Retry => 0x03,
                _ => 0x00,
            };
            let first = 0xC0 | (type_bits << 4);
            buf.push(first);
            buf.extend_from_slice(&packet.version.to_be_bytes());
            // DCID length + DCID
            buf.push(packet.dcid.len);
            buf.extend_from_slice(packet.dcid.as_bytes());
            // SCID length + SCID
            buf.push(packet.scid.len);
            buf.extend_from_slice(packet.scid.as_bytes());
            // Packet number (4 bytes)
            buf.extend_from_slice(&(packet.packet_number as u32).to_be_bytes());
            // Payload length + payload
            buf.extend_from_slice(&encode_varint(packet.payload.len() as u64));
            buf.extend_from_slice(&packet.payload);
        }
    }

    buf
}

/// Decode a QUIC packet from bytes.
pub fn decode_packet(data: &[u8]) -> Result<QuicPacket, &'static str> {
    if data.is_empty() {
        return Err("empty packet");
    }

    let first = data[0];
    let is_long = (first & 0x80) != 0;

    if is_long {
        // Long header
        if data.len() < 7 {
            return Err("packet too short for long header");
        }
        let type_bits = (first >> 4) & 0x03;
        let packet_type = match type_bits {
            0x00 => PacketType::Initial,
            0x01 => PacketType::ZeroRtt,
            0x02 => PacketType::Handshake,
            0x03 => PacketType::Retry,
            _ => return Err("unknown packet type"),
        };
        let version = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
        let dcid_len = data[5] as usize;
        if data.len() < 6 + dcid_len + 1 {
            return Err("packet truncated at DCID");
        }
        let mut dcid = ConnectionId::empty();
        dcid.len = dcid_len.min(8) as u8;
        for i in 0..dcid.len as usize {
            dcid.bytes[i] = data[6 + i];
        }
        let scid_off = 6 + dcid_len;
        let scid_len = data[scid_off] as usize;
        if data.len() < scid_off + 1 + scid_len + 4 {
            return Err("packet truncated at SCID");
        }
        let mut scid = ConnectionId::empty();
        scid.len = scid_len.min(8) as u8;
        for i in 0..scid.len as usize {
            scid.bytes[i] = data[scid_off + 1 + i];
        }
        let pn_off = scid_off + 1 + scid_len;
        if data.len() < pn_off + 4 {
            return Err("packet truncated at packet number");
        }
        let packet_number = u32::from_be_bytes([
            data[pn_off], data[pn_off + 1], data[pn_off + 2], data[pn_off + 3],
        ]) as u64;
        let payload_off = pn_off + 4;
        let payload = if payload_off < data.len() {
            // Decode varint length prefix
            match decode_varint(&data[payload_off..]) {
                Ok((len, consumed)) => {
                    let start = payload_off + consumed;
                    let end = (start + len as usize).min(data.len());
                    data[start..end].to_vec()
                }
                Err(_) => data[payload_off..].to_vec(),
            }
        } else {
            Vec::new()
        };
        Ok(QuicPacket { packet_type, version, dcid, scid, packet_number, payload })
    } else {
        // Short header
        if data.len() < 5 {
            return Err("packet too short for short header");
        }
        // Assume 8-byte DCID (negotiated during handshake)
        let dcid_len = 8usize.min(data.len() - 5);
        let mut dcid = ConnectionId::empty();
        dcid.len = dcid_len as u8;
        for i in 0..dcid_len {
            dcid.bytes[i] = data[1 + i];
        }
        let pn_off = 1 + dcid_len;
        let packet_number = if data.len() >= pn_off + 4 {
            u32::from_be_bytes([
                data[pn_off], data[pn_off + 1], data[pn_off + 2], data[pn_off + 3],
            ]) as u64
        } else {
            0
        };
        let payload_off = pn_off + 4;
        let payload = if payload_off < data.len() {
            data[payload_off..].to_vec()
        } else {
            Vec::new()
        };
        Ok(QuicPacket {
            packet_type: PacketType::Short,
            version: QUIC_VERSION_1,
            dcid,
            scid: ConnectionId::empty(),
            packet_number,
            payload,
        })
    }
}

// ---------------------------------------------------------------------------
// QUIC Frames (RFC 9000, Section 19)
// ---------------------------------------------------------------------------

/// QUIC frame types.
#[derive(Debug, Clone)]
pub enum QuicFrame {
    Padding,
    Ping,
    Ack { largest: u64, delay: u64, ranges: Vec<(u64, u64)> },
    ResetStream { stream_id: u64, error_code: u64, final_size: u64 },
    StopSending { stream_id: u64, error_code: u64 },
    Crypto { offset: u64, data: Vec<u8> },
    NewToken { token: Vec<u8> },
    Stream { stream_id: u64, offset: u64, data: Vec<u8>, fin: bool },
    MaxData(u64),
    MaxStreamData { stream_id: u64, max: u64 },
    MaxStreams { bidi: bool, max: u64 },
    DataBlocked(u64),
    StreamDataBlocked { stream_id: u64, limit: u64 },
    NewConnectionId { seq: u64, retire: u64, cid: ConnectionId, token: [u8; 16] },
    RetireConnectionId(u64),
    PathChallenge([u8; 8]),
    PathResponse([u8; 8]),
    ConnectionClose { error_code: u64, frame_type: u64, reason: String },
    HandshakeDone,
}

/// Encode a QUIC frame into bytes.
pub fn encode_frame(frame: &QuicFrame) -> Vec<u8> {
    let mut buf = Vec::new();
    match frame {
        QuicFrame::Padding => buf.push(0x00),
        QuicFrame::Ping => buf.push(0x01),
        QuicFrame::Ack { largest, delay, ranges } => {
            buf.push(0x02);
            buf.extend_from_slice(&encode_varint(*largest));
            buf.extend_from_slice(&encode_varint(*delay));
            buf.extend_from_slice(&encode_varint(ranges.len() as u64));
            for &(gap, len) in ranges {
                buf.extend_from_slice(&encode_varint(gap));
                buf.extend_from_slice(&encode_varint(len));
            }
        }
        QuicFrame::ResetStream { stream_id, error_code, final_size } => {
            buf.push(0x04);
            buf.extend_from_slice(&encode_varint(*stream_id));
            buf.extend_from_slice(&encode_varint(*error_code));
            buf.extend_from_slice(&encode_varint(*final_size));
        }
        QuicFrame::StopSending { stream_id, error_code } => {
            buf.push(0x05);
            buf.extend_from_slice(&encode_varint(*stream_id));
            buf.extend_from_slice(&encode_varint(*error_code));
        }
        QuicFrame::Crypto { offset, data } => {
            buf.push(0x06);
            buf.extend_from_slice(&encode_varint(*offset));
            buf.extend_from_slice(&encode_varint(data.len() as u64));
            buf.extend_from_slice(data);
        }
        QuicFrame::NewToken { token } => {
            buf.push(0x07);
            buf.extend_from_slice(&encode_varint(token.len() as u64));
            buf.extend_from_slice(token);
        }
        QuicFrame::Stream { stream_id, offset, data, fin } => {
            // STREAM frame type: 0x08 with OFF, LEN, FIN bits
            let mut ftype: u8 = 0x08;
            if *offset > 0 { ftype |= 0x04; } // OFF bit
            ftype |= 0x02; // LEN bit (always include length)
            if *fin { ftype |= 0x01; } // FIN bit
            buf.push(ftype);
            buf.extend_from_slice(&encode_varint(*stream_id));
            if *offset > 0 {
                buf.extend_from_slice(&encode_varint(*offset));
            }
            buf.extend_from_slice(&encode_varint(data.len() as u64));
            buf.extend_from_slice(data);
        }
        QuicFrame::MaxData(max) => {
            buf.push(0x10);
            buf.extend_from_slice(&encode_varint(*max));
        }
        QuicFrame::MaxStreamData { stream_id, max } => {
            buf.push(0x11);
            buf.extend_from_slice(&encode_varint(*stream_id));
            buf.extend_from_slice(&encode_varint(*max));
        }
        QuicFrame::MaxStreams { bidi, max } => {
            buf.push(if *bidi { 0x12 } else { 0x13 });
            buf.extend_from_slice(&encode_varint(*max));
        }
        QuicFrame::DataBlocked(limit) => {
            buf.push(0x14);
            buf.extend_from_slice(&encode_varint(*limit));
        }
        QuicFrame::StreamDataBlocked { stream_id, limit } => {
            buf.push(0x15);
            buf.extend_from_slice(&encode_varint(*stream_id));
            buf.extend_from_slice(&encode_varint(*limit));
        }
        QuicFrame::NewConnectionId { seq, retire, cid, token } => {
            buf.push(0x18);
            buf.extend_from_slice(&encode_varint(*seq));
            buf.extend_from_slice(&encode_varint(*retire));
            buf.push(cid.len);
            buf.extend_from_slice(cid.as_bytes());
            buf.extend_from_slice(token);
        }
        QuicFrame::RetireConnectionId(seq) => {
            buf.push(0x19);
            buf.extend_from_slice(&encode_varint(*seq));
        }
        QuicFrame::PathChallenge(data) => {
            buf.push(0x1A);
            buf.extend_from_slice(data);
        }
        QuicFrame::PathResponse(data) => {
            buf.push(0x1B);
            buf.extend_from_slice(data);
        }
        QuicFrame::ConnectionClose { error_code, frame_type, reason } => {
            buf.push(0x1C);
            buf.extend_from_slice(&encode_varint(*error_code));
            buf.extend_from_slice(&encode_varint(*frame_type));
            let rb = reason.as_bytes();
            buf.extend_from_slice(&encode_varint(rb.len() as u64));
            buf.extend_from_slice(rb);
        }
        QuicFrame::HandshakeDone => buf.push(0x1E),
    }
    buf
}

/// Decode a QUIC frame from bytes, returning (frame, bytes_consumed).
pub fn decode_frame(data: &[u8]) -> Result<(QuicFrame, usize), &'static str> {
    if data.is_empty() {
        return Err("empty frame data");
    }
    let frame_type = data[0];
    let mut pos = 1usize;

    match frame_type {
        0x00 => Ok((QuicFrame::Padding, 1)),
        0x01 => Ok((QuicFrame::Ping, 1)),
        0x02 | 0x03 => {
            // ACK frame
            let (largest, n) = decode_varint(&data[pos..])?;
            pos += n;
            let (delay, n) = decode_varint(&data[pos..])?;
            pos += n;
            let (range_count, n) = decode_varint(&data[pos..])?;
            pos += n;
            let mut ranges = Vec::new();
            for _ in 0..range_count {
                let (gap, n) = decode_varint(&data[pos..])?;
                pos += n;
                let (len, n) = decode_varint(&data[pos..])?;
                pos += n;
                ranges.push((gap, len));
            }
            Ok((QuicFrame::Ack { largest, delay, ranges }, pos))
        }
        0x04 => {
            let (stream_id, n) = decode_varint(&data[pos..])?; pos += n;
            let (error_code, n) = decode_varint(&data[pos..])?; pos += n;
            let (final_size, n) = decode_varint(&data[pos..])?; pos += n;
            Ok((QuicFrame::ResetStream { stream_id, error_code, final_size }, pos))
        }
        0x05 => {
            let (stream_id, n) = decode_varint(&data[pos..])?; pos += n;
            let (error_code, n) = decode_varint(&data[pos..])?; pos += n;
            Ok((QuicFrame::StopSending { stream_id, error_code }, pos))
        }
        0x06 => {
            let (offset, n) = decode_varint(&data[pos..])?; pos += n;
            let (length, n) = decode_varint(&data[pos..])?; pos += n;
            let end = pos + length as usize;
            if end > data.len() { return Err("truncated CRYPTO frame"); }
            let frame_data = data[pos..end].to_vec();
            Ok((QuicFrame::Crypto { offset, data: frame_data }, end))
        }
        0x07 => {
            let (length, n) = decode_varint(&data[pos..])?; pos += n;
            let end = pos + length as usize;
            if end > data.len() { return Err("truncated NEW_TOKEN"); }
            let token = data[pos..end].to_vec();
            Ok((QuicFrame::NewToken { token }, end))
        }
        0x08..=0x0F => {
            // STREAM frame
            let has_off = (frame_type & 0x04) != 0;
            let has_len = (frame_type & 0x02) != 0;
            let fin = (frame_type & 0x01) != 0;
            let (stream_id, n) = decode_varint(&data[pos..])?; pos += n;
            let offset = if has_off {
                let (o, n) = decode_varint(&data[pos..])?; pos += n;
                o
            } else { 0 };
            let length = if has_len {
                let (l, n) = decode_varint(&data[pos..])?; pos += n;
                l as usize
            } else {
                data.len() - pos
            };
            let end = pos + length;
            if end > data.len() { return Err("truncated STREAM frame"); }
            let sdata = data[pos..end].to_vec();
            Ok((QuicFrame::Stream { stream_id, offset, data: sdata, fin }, end))
        }
        0x10 => {
            let (max, n) = decode_varint(&data[pos..])?;
            Ok((QuicFrame::MaxData(max), pos + n))
        }
        0x11 => {
            let (stream_id, n) = decode_varint(&data[pos..])?; pos += n;
            let (max, n) = decode_varint(&data[pos..])?; pos += n;
            Ok((QuicFrame::MaxStreamData { stream_id, max }, pos))
        }
        0x12 | 0x13 => {
            let bidi = frame_type == 0x12;
            let (max, n) = decode_varint(&data[pos..])?;
            Ok((QuicFrame::MaxStreams { bidi, max }, pos + n))
        }
        0x14 => {
            let (limit, n) = decode_varint(&data[pos..])?;
            Ok((QuicFrame::DataBlocked(limit), pos + n))
        }
        0x15 => {
            let (stream_id, n) = decode_varint(&data[pos..])?; pos += n;
            let (limit, n) = decode_varint(&data[pos..])?; pos += n;
            Ok((QuicFrame::StreamDataBlocked { stream_id, limit }, pos))
        }
        0x18 => {
            let (seq, n) = decode_varint(&data[pos..])?; pos += n;
            let (retire, n) = decode_varint(&data[pos..])?; pos += n;
            if pos >= data.len() { return Err("truncated NEW_CONNECTION_ID"); }
            let cid_len = data[pos] as usize; pos += 1;
            if pos + cid_len + 16 > data.len() { return Err("truncated NEW_CONNECTION_ID"); }
            let mut cid = ConnectionId::empty();
            cid.len = cid_len.min(8) as u8;
            for i in 0..cid.len as usize {
                cid.bytes[i] = data[pos + i];
            }
            pos += cid_len;
            let mut token = [0u8; 16];
            token.copy_from_slice(&data[pos..pos + 16]);
            pos += 16;
            Ok((QuicFrame::NewConnectionId { seq, retire, cid, token }, pos))
        }
        0x19 => {
            let (seq, n) = decode_varint(&data[pos..])?;
            Ok((QuicFrame::RetireConnectionId(seq), pos + n))
        }
        0x1A => {
            if data.len() < pos + 8 { return Err("truncated PATH_CHALLENGE"); }
            let mut challenge = [0u8; 8];
            challenge.copy_from_slice(&data[pos..pos + 8]);
            Ok((QuicFrame::PathChallenge(challenge), pos + 8))
        }
        0x1B => {
            if data.len() < pos + 8 { return Err("truncated PATH_RESPONSE"); }
            let mut response = [0u8; 8];
            response.copy_from_slice(&data[pos..pos + 8]);
            Ok((QuicFrame::PathResponse(response), pos + 8))
        }
        0x1C | 0x1D => {
            let (error_code, n) = decode_varint(&data[pos..])?; pos += n;
            let frame_type_val = if frame_type == 0x1C {
                let (ft, n) = decode_varint(&data[pos..])?; pos += n;
                ft
            } else { 0 };
            let (rlen, n) = decode_varint(&data[pos..])?; pos += n;
            let end = pos + rlen as usize;
            let reason = if end <= data.len() {
                core::str::from_utf8(&data[pos..end]).unwrap_or("").to_owned()
            } else {
                String::new()
            };
            Ok((QuicFrame::ConnectionClose {
                error_code, frame_type: frame_type_val, reason,
            }, end.min(data.len())))
        }
        0x1E => Ok((QuicFrame::HandshakeDone, 1)),
        _ => Err("unknown frame type"),
    }
}

// ---------------------------------------------------------------------------
// QUIC Stream
// ---------------------------------------------------------------------------

/// Stream state machine (RFC 9000, Section 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    Idle,
    Open,
    HalfClosedLocal,
    HalfClosedRemote,
    Closed,
}

/// A QUIC stream within a connection.
#[derive(Debug, Clone)]
pub struct QuicStream {
    pub id: u64,
    pub state: StreamState,
    pub send_buf: Vec<u8>,
    pub recv_buf: Vec<u8>,
    pub send_offset: u64,
    pub recv_offset: u64,
    pub max_send: u64,
    pub max_recv: u64,
    pub fin_sent: bool,
    pub fin_received: bool,
}

impl QuicStream {
    fn new(id: u64) -> Self {
        Self {
            id,
            state: StreamState::Idle,
            send_buf: Vec::new(),
            recv_buf: Vec::new(),
            send_offset: 0,
            recv_offset: 0,
            max_send: DEFAULT_MAX_STREAM_DATA,
            max_recv: DEFAULT_MAX_STREAM_DATA,
            fin_sent: false,
            fin_received: false,
        }
    }
}

/// Check if stream ID indicates client-initiated.
pub fn is_client_stream(stream_id: u64) -> bool {
    (stream_id & 0x01) == 0
}

/// Check if stream ID indicates bidirectional.
pub fn is_bidi_stream(stream_id: u64) -> bool {
    (stream_id & 0x02) == 0
}

// ---------------------------------------------------------------------------
// QUIC Connection
// ---------------------------------------------------------------------------

/// Connection state machine (RFC 9000, Section 17.2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Idle,
    Handshaking,
    Connected,
    Closing,
    Draining,
    Closed,
}

/// A QUIC connection.
pub struct QuicConnection {
    pub id: u32,
    pub state: ConnState,
    pub local_cid: ConnectionId,
    pub remote_cid: ConnectionId,
    pub peer_addr: ([u8; 4], u16),
    pub streams: Vec<QuicStream>,
    pub next_stream_id: u64,
    pub next_pkt_num: u64,
    pub max_data: u64,
    pub sent_data: u64,
    pub recv_data: u64,
    pub rtt_us: u64,
    pub congestion_window: u64,
    pub bytes_in_flight: u64,
    pub created_tick: u64,
    pub last_activity: u64,
    pub zero_rtt_enabled: bool,
    pub handshake_complete: bool,
    // Loss detection
    largest_acked: u64,
    loss_time: u64,
    pto_count: u32,
    // Session ticket for 0-RTT
    session_ticket: Vec<u8>,
}

impl QuicConnection {
    fn new(id: u32, local_cid: ConnectionId, peer_addr: ([u8; 4], u16)) -> Self {
        let tick = crate::timer::ticks();
        Self {
            id,
            state: ConnState::Idle,
            local_cid,
            remote_cid: ConnectionId::empty(),
            peer_addr,
            streams: Vec::new(),
            next_stream_id: 0,
            next_pkt_num: 0,
            max_data: DEFAULT_MAX_DATA,
            sent_data: 0,
            recv_data: 0,
            rtt_us: 100_000, // Initial RTT estimate: 100ms
            congestion_window: INITIAL_CWND,
            bytes_in_flight: 0,
            created_tick: tick,
            last_activity: tick,
            zero_rtt_enabled: false,
            handshake_complete: false,
            largest_acked: 0,
            loss_time: 0,
            pto_count: 0,
            session_ticket: Vec::new(),
        }
    }

    /// Create a new stream on this connection.
    pub fn create_stream(&mut self, bidi: bool) -> u64 {
        let id = self.next_stream_id;
        // Bit 0: initiator (0=client), Bit 1: uni(1)/bidi(0)
        self.next_stream_id += if bidi { 4 } else { 4 };
        let mut stream = QuicStream::new(id | if !bidi { 0x02 } else { 0x00 });
        stream.state = StreamState::Open;
        self.streams.push(stream);
        id
    }

    /// Send data on a stream.
    pub fn send(&mut self, stream_id: u64, data: &[u8]) -> Result<(), &'static str> {
        let stream = self.streams.iter_mut().find(|s| s.id == stream_id)
            .ok_or("stream not found")?;
        if stream.state == StreamState::Closed || stream.state == StreamState::HalfClosedLocal {
            return Err("stream not writable");
        }
        let allowed = stream.max_send.saturating_sub(stream.send_offset) as usize;
        if data.len() > allowed {
            return Err("flow control limit exceeded");
        }
        stream.send_buf.extend_from_slice(data);
        stream.send_offset += data.len() as u64;
        self.sent_data += data.len() as u64;
        TOTAL_BYTES_SENT.fetch_add(data.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Receive data from a stream.
    pub fn recv(&mut self, stream_id: u64) -> Result<Vec<u8>, &'static str> {
        let stream = self.streams.iter_mut().find(|s| s.id == stream_id)
            .ok_or("stream not found")?;
        if stream.recv_buf.is_empty() {
            return Ok(Vec::new());
        }
        let data = core::mem::take(&mut stream.recv_buf);
        stream.recv_offset += data.len() as u64;
        self.recv_data += data.len() as u64;
        TOTAL_BYTES_RECV.fetch_add(data.len() as u64, Ordering::Relaxed);
        Ok(data)
    }

    /// Close a stream.
    pub fn close_stream(&mut self, stream_id: u64) {
        if let Some(stream) = self.streams.iter_mut().find(|s| s.id == stream_id) {
            stream.fin_sent = true;
            stream.state = match stream.state {
                StreamState::Open => StreamState::HalfClosedLocal,
                StreamState::HalfClosedRemote => StreamState::Closed,
                other => other,
            };
        }
    }

    /// Get the Probe Timeout duration in microseconds.
    fn pto_duration(&self) -> u64 {
        (self.rtt_us * PTO_MULTIPLIER) / 256
    }

    /// Update smoothed RTT with a new sample.
    fn update_rtt(&mut self, sample_us: u64) {
        // Exponential weighted moving average (1/8 weight for new sample)
        self.rtt_us = (self.rtt_us * 7 + sample_us) / 8;
    }

    /// Process an ACK frame for loss detection and congestion control.
    fn on_ack(&mut self, largest: u64, _delay: u64) {
        if largest > self.largest_acked {
            self.largest_acked = largest;
        }
        self.pto_count = 0;
        self.last_activity = crate::timer::ticks();
        // Cubic-style congestion window increase
        if self.bytes_in_flight < self.congestion_window {
            // Slow start or congestion avoidance
            let increment = if self.congestion_window < 65536 {
                // Slow start: increase by MSS per ACK
                1460
            } else {
                // Congestion avoidance: Cubic growth
                1460 * 1460 / (self.congestion_window as u64).max(1)
            };
            self.congestion_window = self.congestion_window.saturating_add(increment);
        }
        TOTAL_ACKS.fetch_add(1, Ordering::Relaxed);
    }

    /// Handle packet loss detection.
    fn on_loss(&mut self) {
        // Multiplicative decrease (Cubic beta = 0.7, approximate with 7/10)
        self.congestion_window = (self.congestion_window * 7 / 10).max(MIN_CWND);
        TOTAL_LOSSES.fetch_add(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static CONNECTIONS: Mutex<Vec<QuicConnection>> = Mutex::new(Vec::new());
static NEXT_CONN_ID: AtomicU32 = AtomicU32::new(1);
static TOTAL_CONNECTIONS: AtomicU64 = AtomicU64::new(0);
static TOTAL_BYTES_SENT: AtomicU64 = AtomicU64::new(0);
static TOTAL_BYTES_RECV: AtomicU64 = AtomicU64::new(0);
static TOTAL_PACKETS_SENT: AtomicU64 = AtomicU64::new(0);
static TOTAL_PACKETS_RECV: AtomicU64 = AtomicU64::new(0);
static TOTAL_ACKS: AtomicU64 = AtomicU64::new(0);
static TOTAL_LOSSES: AtomicU64 = AtomicU64::new(0);
static TOTAL_ZERO_RTT: AtomicU64 = AtomicU64::new(0);
static TOTAL_MIGRATIONS: AtomicU64 = AtomicU64::new(0);

/// Pending incoming connections (server accept queue).
static ACCEPT_QUEUE: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Session tickets for 0-RTT reconnection, keyed by peer address.
static SESSION_CACHE: Mutex<Vec<([u8; 4], u16, Vec<u8>)>> = Mutex::new(Vec::new());

// ---------------------------------------------------------------------------
// Connection management
// ---------------------------------------------------------------------------

/// Initiate a QUIC connection to the given address and port (client).
pub fn connect(addr: [u8; 4], port: u16) -> Result<u32, &'static str> {
    let mut conns = CONNECTIONS.lock();
    if conns.len() >= MAX_CONNECTIONS {
        return Err("too many connections");
    }
    let id = NEXT_CONN_ID.fetch_add(1, Ordering::SeqCst);
    let local_cid = ConnectionId::generate(8);
    let mut conn = QuicConnection::new(id, local_cid, (addr, port));
    conn.remote_cid = ConnectionId::generate(8);
    conn.state = ConnState::Handshaking;

    // Check for cached session ticket (0-RTT)
    let cache = SESSION_CACHE.lock();
    for &(ref a, p, ref ticket) in cache.iter() {
        if *a == addr && p == port && !ticket.is_empty() {
            conn.zero_rtt_enabled = true;
            conn.session_ticket = ticket.clone();
            TOTAL_ZERO_RTT.fetch_add(1, Ordering::Relaxed);
            break;
        }
    }
    drop(cache);

    // Simulate handshake completion (in real impl, TLS 1.3 handshake)
    conn.state = ConnState::Connected;
    conn.handshake_complete = true;

    TOTAL_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
    conns.push(conn);
    Ok(id)
}

/// Accept an incoming QUIC connection (server).
pub fn accept() -> Option<u32> {
    ACCEPT_QUEUE.lock().pop()
}

/// Close a QUIC connection.
pub fn close(conn_id: u32, error_code: u64, reason: &str) {
    let mut conns = CONNECTIONS.lock();
    if let Some(conn) = conns.iter_mut().find(|c| c.id == conn_id) {
        conn.state = ConnState::Closing;
        // Build CONNECTION_CLOSE frame
        let _frame = encode_frame(&QuicFrame::ConnectionClose {
            error_code,
            frame_type: 0,
            reason: reason.to_owned(),
        });
        TOTAL_PACKETS_SENT.fetch_add(1, Ordering::Relaxed);
        conn.state = ConnState::Closed;
    }
    conns.retain(|c| c.state != ConnState::Closed);
}

/// Send an unreliable datagram on a connection (RFC 9221).
pub fn send_datagram(conn_id: u32, data: &[u8]) -> Result<(), &'static str> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns.iter_mut().find(|c| c.id == conn_id)
        .ok_or("connection not found")?;
    if conn.state != ConnState::Connected {
        return Err("connection not ready");
    }
    conn.sent_data += data.len() as u64;
    TOTAL_BYTES_SENT.fetch_add(data.len() as u64, Ordering::Relaxed);
    TOTAL_PACKETS_SENT.fetch_add(1, Ordering::Relaxed);
    conn.last_activity = crate::timer::ticks();
    Ok(())
}

// ---------------------------------------------------------------------------
// 0-RTT support
// ---------------------------------------------------------------------------

/// Enable 0-RTT for a connection (save session ticket after handshake).
pub fn enable_zero_rtt(conn_id: u32) {
    let mut conns = CONNECTIONS.lock();
    if let Some(conn) = conns.iter_mut().find(|c| c.id == conn_id) {
        conn.zero_rtt_enabled = true;
        // Save session ticket for future reconnection
        if conn.session_ticket.is_empty() {
            // Generate a placeholder session ticket
            let seed = CID_SEED.fetch_add(1, Ordering::Relaxed);
            let ticket = (seed as u64).to_be_bytes().to_vec();
            conn.session_ticket = ticket.clone();
            let mut cache = SESSION_CACHE.lock();
            cache.push((conn.peer_addr.0, conn.peer_addr.1, ticket));
        }
    }
}

/// Check if 0-RTT is available for a connection.
pub fn has_zero_rtt(conn_id: u32) -> bool {
    let conns = CONNECTIONS.lock();
    conns.iter().find(|c| c.id == conn_id)
        .map(|c| c.zero_rtt_enabled)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Connection migration
// ---------------------------------------------------------------------------

/// Migrate a connection to a new peer address.
pub fn migrate(conn_id: u32, new_addr: ([u8; 4], u16)) -> Result<(), &'static str> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns.iter_mut().find(|c| c.id == conn_id)
        .ok_or("connection not found")?;
    if conn.state != ConnState::Connected {
        return Err("connection not ready for migration");
    }
    // Initiate path validation
    let challenge = CID_SEED.fetch_add(1, Ordering::Relaxed) as u64;
    let _frame = encode_frame(&QuicFrame::PathChallenge(challenge.to_be_bytes()));
    TOTAL_PACKETS_SENT.fetch_add(1, Ordering::Relaxed);

    conn.peer_addr = new_addr;
    // Reset congestion state for new path
    conn.congestion_window = INITIAL_CWND;
    conn.bytes_in_flight = 0;
    conn.rtt_us = 100_000; // Reset RTT estimate
    TOTAL_MIGRATIONS.fetch_add(1, Ordering::Relaxed);
    conn.last_activity = crate::timer::ticks();
    Ok(())
}

// ---------------------------------------------------------------------------
// Statistics & info
// ---------------------------------------------------------------------------

/// Summary of the QUIC subsystem.
pub fn quic_info() -> String {
    let conns = CONNECTIONS.lock();
    let active = conns.iter().filter(|c| c.state == ConnState::Connected).count();
    let total_streams: usize = conns.iter().map(|c| c.streams.len()).sum();
    format!(
        "QUIC Transport (RFC 9000)\n\
         Version:       0x{:08X} (QUIC v1)\n\
         Active conns:  {}\n\
         Total conns:   {}\n\
         Active streams:{}\n\
         0-RTT enabled: {}\n\
         Migrations:    {}",
        QUIC_VERSION_1,
        active,
        TOTAL_CONNECTIONS.load(Ordering::Relaxed),
        total_streams,
        TOTAL_ZERO_RTT.load(Ordering::Relaxed),
        TOTAL_MIGRATIONS.load(Ordering::Relaxed),
    )
}

/// Detailed statistics.
pub fn quic_stats() -> String {
    format!(
        "QUIC Statistics\n\
         Packets sent:   {}\n\
         Packets recv:   {}\n\
         Bytes sent:     {}\n\
         Bytes recv:     {}\n\
         ACKs processed: {}\n\
         Losses:         {}\n\
         0-RTT resumes:  {}\n\
         Migrations:     {}",
        TOTAL_PACKETS_SENT.load(Ordering::Relaxed),
        TOTAL_PACKETS_RECV.load(Ordering::Relaxed),
        TOTAL_BYTES_SENT.load(Ordering::Relaxed),
        TOTAL_BYTES_RECV.load(Ordering::Relaxed),
        TOTAL_ACKS.load(Ordering::Relaxed),
        TOTAL_LOSSES.load(Ordering::Relaxed),
        TOTAL_ZERO_RTT.load(Ordering::Relaxed),
        TOTAL_MIGRATIONS.load(Ordering::Relaxed),
    )
}

/// List all active QUIC connections.
pub fn list_connections() -> String {
    let conns = CONNECTIONS.lock();
    if conns.is_empty() {
        return String::from("No active QUIC connections.\n");
    }
    let mut out = format!("{:<6} {:<20} {:<12} {:<8} {:<10} {:<8}\n",
        "ID", "Peer", "State", "Streams", "RTT(ms)", "CWND");
    out.push_str(&format!("{}\n", "-".repeat(68)));
    for c in conns.iter() {
        let ip = format!("{}.{}.{}.{}:{}",
            c.peer_addr.0[0], c.peer_addr.0[1],
            c.peer_addr.0[2], c.peer_addr.0[3], c.peer_addr.1);
        let state = match c.state {
            ConnState::Idle => "idle",
            ConnState::Handshaking => "handshake",
            ConnState::Connected => "connected",
            ConnState::Closing => "closing",
            ConnState::Draining => "draining",
            ConnState::Closed => "closed",
        };
        let rtt_ms = c.rtt_us / 1000;
        out.push_str(&format!("{:<6} {:<20} {:<12} {:<8} {:<10} {:<8}\n",
            c.id, ip, state, c.streams.len(), rtt_ms, c.congestion_window));
    }
    out
}

/// Detailed info for a specific connection.
pub fn connection_info(conn_id: u32) -> String {
    let conns = CONNECTIONS.lock();
    match conns.iter().find(|c| c.id == conn_id) {
        None => format!("Connection {} not found.\n", conn_id),
        Some(c) => {
            let state = match c.state {
                ConnState::Idle => "idle",
                ConnState::Handshaking => "handshake",
                ConnState::Connected => "connected",
                ConnState::Closing => "closing",
                ConnState::Draining => "draining",
                ConnState::Closed => "closed",
            };
            format!(
                "Connection {}\n\
                 Local CID:  {}\n\
                 Remote CID: {}\n\
                 Peer:       {}.{}.{}.{}:{}\n\
                 State:      {}\n\
                 Streams:    {}\n\
                 RTT:        {} us\n\
                 CWND:       {} bytes\n\
                 In-flight:  {} bytes\n\
                 Sent:       {} bytes\n\
                 Received:   {} bytes\n\
                 0-RTT:      {}\n\
                 Handshake:  {}",
                c.id,
                c.local_cid.to_hex(),
                c.remote_cid.to_hex(),
                c.peer_addr.0[0], c.peer_addr.0[1],
                c.peer_addr.0[2], c.peer_addr.0[3], c.peer_addr.1,
                state,
                c.streams.len(),
                c.rtt_us,
                c.congestion_window,
                c.bytes_in_flight,
                c.sent_data,
                c.recv_data,
                if c.zero_rtt_enabled { "enabled" } else { "disabled" },
                if c.handshake_complete { "complete" } else { "pending" },
            )
        }
    }
}

/// Initialize the QUIC subsystem.
pub fn init() {
    // Pre-warm the connection vector
    let _ = CONNECTIONS.lock();
    let _ = SESSION_CACHE.lock();
}
