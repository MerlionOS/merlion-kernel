/// HTTP/2 protocol support for MerlionOS.
/// Implements HTTP/2 framing, HPACK header compression,
/// multiplexed streams, flow control, and server push.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// HTTP/2 connection preface magic bytes (RFC 7540 Section 3.5).
pub const CONNECTION_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// Default header table size for HPACK (bytes).
const DEFAULT_HEADER_TABLE_SIZE: u32 = 4096;

/// Default enable push setting.
const DEFAULT_ENABLE_PUSH: u32 = 1;

/// Default max concurrent streams.
const DEFAULT_MAX_CONCURRENT_STREAMS: u32 = 100;

/// Default initial window size (bytes).
const DEFAULT_INITIAL_WINDOW_SIZE: i32 = 65535;

/// Default max frame size (bytes).
const DEFAULT_MAX_FRAME_SIZE: u32 = 16384;

/// Maximum number of tracked streams.
const MAX_STREAMS: usize = 128;

/// Frame header size (9 bytes: 3 length + 1 type + 1 flags + 4 stream id).
const FRAME_HEADER_SIZE: usize = 9;

// ---------------------------------------------------------------------------
// Frame types (RFC 7540 Section 6)
// ---------------------------------------------------------------------------

/// HTTP/2 frame types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    /// DATA frame (carries request/response body).
    Data = 0,
    /// HEADERS frame (carries header block fragment).
    Headers = 1,
    /// PRIORITY frame (stream dependency/weight).
    Priority = 2,
    /// RST_STREAM frame (abnormal stream termination).
    RstStream = 3,
    /// SETTINGS frame (connection configuration).
    Settings = 4,
    /// PUSH_PROMISE frame (server push initiation).
    PushPromise = 5,
    /// PING frame (connectivity check / RTT measurement).
    Ping = 6,
    /// GOAWAY frame (graceful connection shutdown).
    GoAway = 7,
    /// WINDOW_UPDATE frame (flow control).
    WindowUpdate = 8,
    /// CONTINUATION frame (header block continuation).
    Continuation = 9,
}

impl FrameType {
    /// Convert a raw byte to a FrameType, if valid.
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::Data),
            1 => Some(Self::Headers),
            2 => Some(Self::Priority),
            3 => Some(Self::RstStream),
            4 => Some(Self::Settings),
            5 => Some(Self::PushPromise),
            6 => Some(Self::Ping),
            7 => Some(Self::GoAway),
            8 => Some(Self::WindowUpdate),
            9 => Some(Self::Continuation),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Frame flags
// ---------------------------------------------------------------------------

/// END_STREAM flag (DATA, HEADERS).
pub const FLAG_END_STREAM: u8 = 0x01;
/// END_HEADERS flag (HEADERS, PUSH_PROMISE, CONTINUATION).
pub const FLAG_END_HEADERS: u8 = 0x04;
/// PADDED flag (DATA, HEADERS).
pub const FLAG_PADDED: u8 = 0x08;
/// PRIORITY flag (HEADERS).
pub const FLAG_PRIORITY: u8 = 0x20;
/// ACK flag (SETTINGS, PING).
pub const FLAG_ACK: u8 = 0x01;

// ---------------------------------------------------------------------------
// Frame structure
// ---------------------------------------------------------------------------

/// An HTTP/2 frame.
pub struct Frame {
    /// Payload length (24-bit, max 16384 by default).
    pub length: u32,
    /// Frame type.
    pub frame_type: FrameType,
    /// Frame flags.
    pub flags: u8,
    /// Stream identifier (31-bit, 0 for connection-level frames).
    pub stream_id: u32,
    /// Frame payload bytes.
    pub payload: Vec<u8>,
}

impl Frame {
    /// Create a new frame.
    pub fn new(frame_type: FrameType, flags: u8, stream_id: u32, payload: Vec<u8>) -> Self {
        Self {
            length: payload.len() as u32,
            frame_type,
            flags,
            stream_id,
            payload,
        }
    }

    /// Serialize the frame to bytes (header + payload).
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(FRAME_HEADER_SIZE + self.payload.len());
        // 24-bit length
        buf.push((self.length >> 16) as u8);
        buf.push((self.length >> 8) as u8);
        buf.push(self.length as u8);
        // Type
        buf.push(self.frame_type as u8);
        // Flags
        buf.push(self.flags);
        // 31-bit stream ID (high bit reserved, always 0)
        let sid = self.stream_id & 0x7FFF_FFFF;
        buf.push((sid >> 24) as u8);
        buf.push((sid >> 16) as u8);
        buf.push((sid >> 8) as u8);
        buf.push(sid as u8);
        // Payload
        buf.extend_from_slice(&self.payload);
        buf
    }

    /// Decode a frame from raw bytes. Returns `(frame, bytes_consumed)`.
    pub fn decode(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < FRAME_HEADER_SIZE {
            return None;
        }
        let length = ((data[0] as u32) << 16) | ((data[1] as u32) << 8) | (data[2] as u32);
        let frame_type = FrameType::from_u8(data[3])?;
        let flags = data[4];
        let stream_id = ((data[5] as u32 & 0x7F) << 24)
            | ((data[6] as u32) << 16)
            | ((data[7] as u32) << 8)
            | (data[8] as u32);

        let total = FRAME_HEADER_SIZE + length as usize;
        if data.len() < total {
            return None;
        }
        let payload = data[FRAME_HEADER_SIZE..total].to_vec();
        Some((
            Self { length, frame_type, flags, stream_id, payload },
            total,
        ))
    }
}

// ---------------------------------------------------------------------------
// Stream states (RFC 7540 Section 5.1)
// ---------------------------------------------------------------------------

/// HTTP/2 stream states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    /// Stream is idle (not yet opened).
    Idle,
    /// Stream is open (both sides can send).
    Open,
    /// Local side has sent END_STREAM.
    HalfClosedLocal,
    /// Remote side has sent END_STREAM.
    HalfClosedRemote,
    /// Stream is closed.
    Closed,
}

// ---------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------

/// A single HTTP/2 stream within a connection.
pub struct H2Stream {
    /// Stream identifier.
    pub id: u32,
    /// Current stream state.
    pub state: StreamState,
    /// Receive flow control window.
    pub recv_window: i32,
    /// Send flow control window.
    pub send_window: i32,
    /// Decoded headers for this stream.
    pub headers: Vec<(String, String)>,
    /// Accumulated data payload.
    pub data: Vec<u8>,
}

impl H2Stream {
    /// Create a new stream with default window sizes.
    fn new(id: u32) -> Self {
        Self {
            id,
            state: StreamState::Open,
            recv_window: DEFAULT_INITIAL_WINDOW_SIZE,
            send_window: DEFAULT_INITIAL_WINDOW_SIZE,
            headers: Vec::new(),
            data: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// HPACK static table (RFC 7541, Appendix A) — subset
// ---------------------------------------------------------------------------

/// HPACK static table entries (index, name, value).
/// We include the 61 standard entries with commonly used ones having values.
static HPACK_STATIC_TABLE: &[(&str, &str)] = &[
    // Index 0 is unused (1-based)
    ("", ""),
    (":authority", ""),                    // 1
    (":method", "GET"),                    // 2
    (":method", "POST"),                   // 3
    (":path", "/"),                        // 4
    (":path", "/index.html"),              // 5
    (":scheme", "http"),                   // 6
    (":scheme", "https"),                  // 7
    (":status", "200"),                    // 8
    (":status", "204"),                    // 9
    (":status", "206"),                    // 10
    (":status", "304"),                    // 11
    (":status", "400"),                    // 12
    (":status", "404"),                    // 13
    (":status", "500"),                    // 14
    ("accept-charset", ""),                // 15
    ("accept-encoding", "gzip, deflate"), // 16
    ("accept-language", ""),               // 17
    ("accept-ranges", ""),                 // 18
    ("accept", ""),                        // 19
    ("access-control-allow-origin", ""),   // 20
    ("age", ""),                           // 21
    ("allow", ""),                         // 22
    ("authorization", ""),                 // 23
    ("cache-control", ""),                 // 24
    ("content-disposition", ""),           // 25
    ("content-encoding", ""),              // 26
    ("content-language", ""),              // 27
    ("content-length", ""),                // 28
    ("content-location", ""),              // 29
    ("content-range", ""),                 // 30
    ("content-type", ""),                  // 31
    ("cookie", ""),                        // 32
    ("date", ""),                          // 33
    ("etag", ""),                          // 34
    ("expect", ""),                        // 35
    ("expires", ""),                       // 36
    ("from", ""),                          // 37
    ("host", ""),                          // 38
    ("if-match", ""),                      // 39
    ("if-modified-since", ""),             // 40
    ("if-none-match", ""),                 // 41
    ("if-range", ""),                      // 42
    ("if-unmodified-since", ""),           // 43
    ("last-modified", ""),                 // 44
    ("link", ""),                          // 45
    ("location", ""),                      // 46
    ("max-forwards", ""),                  // 47
    ("proxy-authenticate", ""),            // 48
    ("proxy-authorization", ""),           // 49
    ("range", ""),                         // 50
    ("referer", ""),                       // 51
    ("refresh", ""),                       // 52
    ("retry-after", ""),                   // 53
    ("server", ""),                        // 54
    ("set-cookie", ""),                    // 55
    ("strict-transport-security", ""),     // 56
    ("transfer-encoding", ""),             // 57
    ("user-agent", ""),                    // 58
    ("vary", ""),                          // 59
    ("via", ""),                           // 60
    ("www-authenticate", ""),              // 61
];

// ---------------------------------------------------------------------------
// HPACK dynamic table
// ---------------------------------------------------------------------------

/// HPACK dynamic table for header compression.
struct HpackDynamicTable {
    entries: Vec<(String, String)>,
    size: usize,
    max_size: usize,
}

impl HpackDynamicTable {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            size: 0,
            max_size: DEFAULT_HEADER_TABLE_SIZE as usize,
        }
    }

    /// Add an entry, evicting old entries if the table exceeds max_size.
    fn add(&mut self, name: String, value: String) {
        let entry_size = name.len() + value.len() + 32; // RFC 7541 overhead
        // Evict entries from the end (oldest) until there is room
        while self.size + entry_size > self.max_size && !self.entries.is_empty() {
            if let Some(old) = self.entries.pop() {
                let old_size = old.0.len() + old.1.len() + 32;
                self.size = self.size.saturating_sub(old_size);
            }
        }
        if entry_size <= self.max_size {
            self.entries.insert(0, (name, value));
            self.size += entry_size;
        }
    }

    /// Look up an entry by dynamic table index (0-based within dynamic table).
    fn get(&self, index: usize) -> Option<(&str, &str)> {
        self.entries.get(index).map(|(n, v)| (n.as_str(), v.as_str()))
    }

    /// Look up by combined index (static table is 1..=61, dynamic starts at 62).
    fn lookup(&self, index: usize) -> Option<(String, String)> {
        if index == 0 {
            return None;
        }
        if index < HPACK_STATIC_TABLE.len() {
            let (n, v) = HPACK_STATIC_TABLE[index];
            return Some((String::from(n), String::from(v)));
        }
        let dyn_idx = index - HPACK_STATIC_TABLE.len();
        self.get(dyn_idx).map(|(n, v)| (String::from(n), String::from(v)))
    }

    /// Find index for a header in the static table (name match only).
    fn find_static_name(name: &str) -> Option<usize> {
        for i in 1..HPACK_STATIC_TABLE.len() {
            if HPACK_STATIC_TABLE[i].0 == name {
                return Some(i);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// HPACK encoding/decoding (simplified, no Huffman)
// ---------------------------------------------------------------------------

/// Encode an HPACK integer with the given prefix bits.
fn encode_hpack_int(value: u32, prefix_bits: u8) -> Vec<u8> {
    let max_prefix = (1u32 << prefix_bits) - 1;
    if value < max_prefix {
        return vec![value as u8];
    }
    let mut buf = vec![max_prefix as u8];
    let mut remaining = value - max_prefix;
    while remaining >= 128 {
        buf.push((remaining & 0x7F) as u8 | 0x80);
        remaining >>= 7;
    }
    buf.push(remaining as u8);
    buf
}

/// Decode an HPACK integer. Returns (value, bytes_consumed).
fn decode_hpack_int(data: &[u8], prefix_bits: u8) -> Option<(u32, usize)> {
    if data.is_empty() {
        return None;
    }
    let max_prefix = (1u32 << prefix_bits) - 1;
    let first = (data[0] as u32) & max_prefix;
    if first < max_prefix {
        return Some((first, 1));
    }
    let mut value = max_prefix;
    let mut shift = 0u32;
    let mut i = 1;
    loop {
        if i >= data.len() {
            return None;
        }
        let b = data[i] as u32;
        value += (b & 0x7F) << shift;
        i += 1;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 28 {
            return None;
        }
    }
    Some((value, i))
}

/// Encode a set of headers into an HPACK header block.
pub fn encode_headers(headers: &[(String, String)]) -> Vec<u8> {
    let mut buf = Vec::new();
    for (name, value) in headers {
        // Check static table for a full match or name match
        let mut found_full = false;
        for i in 1..HPACK_STATIC_TABLE.len() {
            let (sn, sv) = HPACK_STATIC_TABLE[i];
            if sn == name.as_str() && sv == value.as_str() && !sv.is_empty() {
                // Indexed header field (Section 6.1): top bit set
                let mut encoded = encode_hpack_int(i as u32, 7);
                encoded[0] |= 0x80;
                buf.extend_from_slice(&encoded);
                found_full = true;
                break;
            }
        }
        if found_full {
            continue;
        }
        // Literal header with incremental indexing (Section 6.2.1)
        if let Some(idx) = HpackDynamicTable::find_static_name(name) {
            let mut encoded = encode_hpack_int(idx as u32, 6);
            encoded[0] |= 0x40;
            buf.extend_from_slice(&encoded);
        } else {
            buf.push(0x40); // new name
            let name_bytes = name.as_bytes();
            buf.extend_from_slice(&encode_hpack_int(name_bytes.len() as u32, 7));
            buf.extend_from_slice(name_bytes);
        }
        let value_bytes = value.as_bytes();
        buf.extend_from_slice(&encode_hpack_int(value_bytes.len() as u32, 7));
        buf.extend_from_slice(value_bytes);
    }
    buf
}

/// Decode an HPACK header block into a list of (name, value) pairs.
pub fn decode_headers(data: &[u8]) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    let mut table = HpackDynamicTable::new();
    let mut pos = 0;
    while pos < data.len() {
        let byte = data[pos];
        if byte & 0x80 != 0 {
            // Indexed header field
            if let Some((idx, consumed)) = decode_hpack_int(&data[pos..], 7) {
                pos += consumed;
                if let Some((name, value)) = table.lookup(idx as usize) {
                    headers.push((name, value));
                }
            } else {
                break;
            }
        } else if byte & 0x40 != 0 {
            // Literal with incremental indexing
            if let Some((idx, consumed)) = decode_hpack_int(&data[pos..], 6) {
                pos += consumed;
                let name = if idx > 0 {
                    table.lookup(idx as usize).map(|(n, _)| n).unwrap_or_default()
                } else {
                    // Read literal name
                    if let Some((len, c)) = decode_hpack_int(&data[pos..], 7) {
                        pos += c;
                        let end = pos + len as usize;
                        if end <= data.len() {
                            let n = core::str::from_utf8(&data[pos..end])
                                .unwrap_or("").into();
                            pos = end;
                            n
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                };
                // Read value
                if let Some((len, c)) = decode_hpack_int(&data[pos..], 7) {
                    pos += c;
                    let end = pos + len as usize;
                    if end <= data.len() {
                        let value: String = core::str::from_utf8(&data[pos..end])
                            .unwrap_or("").into();
                        pos = end;
                        table.add(name.clone(), value.clone());
                        headers.push((name, value));
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            } else {
                break;
            }
        } else {
            // Literal without indexing or never indexed — skip
            let prefix = if byte & 0xF0 == 0x10 { 4u8 } else { 4u8 };
            if let Some((idx, consumed)) = decode_hpack_int(&data[pos..], prefix) {
                pos += consumed;
                let _name = if idx > 0 {
                    table.lookup(idx as usize).map(|(n, _)| n).unwrap_or_default()
                } else {
                    if let Some((len, c)) = decode_hpack_int(&data[pos..], 7) {
                        pos += c;
                        let end = pos + len as usize;
                        if end <= data.len() {
                            let n: String = core::str::from_utf8(&data[pos..end])
                                .unwrap_or("").into();
                            pos = end;
                            n
                        } else { break; }
                    } else { break; }
                };
                if let Some((len, c)) = decode_hpack_int(&data[pos..], 7) {
                    pos += c;
                    let end = pos + len as usize;
                    if end <= data.len() {
                        let value: String = core::str::from_utf8(&data[pos..end])
                            .unwrap_or("").into();
                        pos = end;
                        headers.push((_name, value));
                    } else { break; }
                } else { break; }
            } else {
                break;
            }
        }
    }
    headers
}

// ---------------------------------------------------------------------------
// Connection settings
// ---------------------------------------------------------------------------

/// HTTP/2 connection settings.
pub struct H2Settings {
    pub header_table_size: u32,
    pub enable_push: u32,
    pub max_concurrent_streams: u32,
    pub initial_window_size: i32,
    pub max_frame_size: u32,
    pub max_header_list_size: u32,
}

impl H2Settings {
    /// Default settings per RFC 7540.
    pub fn default_settings() -> Self {
        Self {
            header_table_size: DEFAULT_HEADER_TABLE_SIZE,
            enable_push: DEFAULT_ENABLE_PUSH,
            max_concurrent_streams: DEFAULT_MAX_CONCURRENT_STREAMS,
            initial_window_size: DEFAULT_INITIAL_WINDOW_SIZE,
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
            max_header_list_size: 0, // unlimited
        }
    }

    /// Encode settings as a SETTINGS frame payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(36);
        // Each setting is 6 bytes: u16 identifier + u32 value
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(&self.header_table_size.to_be_bytes());
        buf.extend_from_slice(&2u16.to_be_bytes());
        buf.extend_from_slice(&self.enable_push.to_be_bytes());
        buf.extend_from_slice(&3u16.to_be_bytes());
        buf.extend_from_slice(&self.max_concurrent_streams.to_be_bytes());
        buf.extend_from_slice(&4u16.to_be_bytes());
        buf.extend_from_slice(&(self.initial_window_size as u32).to_be_bytes());
        buf.extend_from_slice(&5u16.to_be_bytes());
        buf.extend_from_slice(&self.max_frame_size.to_be_bytes());
        buf
    }

    /// Parse settings from a SETTINGS frame payload.
    pub fn parse(data: &[u8]) -> Self {
        let mut s = Self::default_settings();
        let mut pos = 0;
        while pos + 6 <= data.len() {
            let id = u16::from_be_bytes([data[pos], data[pos + 1]]);
            let val = u32::from_be_bytes([data[pos + 2], data[pos + 3],
                                          data[pos + 4], data[pos + 5]]);
            match id {
                1 => s.header_table_size = val,
                2 => s.enable_push = val,
                3 => s.max_concurrent_streams = val,
                4 => s.initial_window_size = val as i32,
                5 => s.max_frame_size = val,
                6 => s.max_header_list_size = val,
                _ => {}
            }
            pos += 6;
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Connection state
// ---------------------------------------------------------------------------

/// An HTTP/2 connection managing multiple streams.
pub struct H2Connection {
    /// Local settings.
    pub local_settings: H2Settings,
    /// Remote peer settings.
    pub remote_settings: H2Settings,
    /// Active streams.
    pub streams: Vec<H2Stream>,
    /// Connection-level receive window.
    pub conn_recv_window: i32,
    /// Connection-level send window.
    pub conn_send_window: i32,
    /// Next stream ID for server-initiated (even) streams.
    pub next_push_stream_id: u32,
    /// Whether connection preface has been received.
    pub preface_received: bool,
}

impl H2Connection {
    /// Create a new server-side HTTP/2 connection.
    pub fn new() -> Self {
        Self {
            local_settings: H2Settings::default_settings(),
            remote_settings: H2Settings::default_settings(),
            streams: Vec::new(),
            conn_recv_window: DEFAULT_INITIAL_WINDOW_SIZE,
            conn_send_window: DEFAULT_INITIAL_WINDOW_SIZE,
            next_push_stream_id: 2, // server push streams are even
            preface_received: false,
        }
    }

    /// Build the server connection preface: SETTINGS frame.
    pub fn server_preface(&self) -> Vec<u8> {
        let settings_payload = self.local_settings.encode();
        let frame = Frame::new(FrameType::Settings, 0, 0, settings_payload);
        frame.encode()
    }

    /// Get or create a stream by ID.
    pub fn get_or_create_stream(&mut self, stream_id: u32) -> &mut H2Stream {
        if !self.streams.iter().any(|s| s.id == stream_id) {
            if self.streams.len() >= MAX_STREAMS {
                // Evict oldest closed stream
                self.streams.retain(|s| s.state != StreamState::Closed);
            }
            self.streams.push(H2Stream::new(stream_id));
        }
        self.streams.iter_mut().find(|s| s.id == stream_id).unwrap()
    }

    /// Process an incoming frame.
    pub fn process_frame(&mut self, frame: &Frame) -> Vec<Frame> {
        let mut responses = Vec::new();
        match frame.frame_type {
            FrameType::Settings => {
                if frame.flags & FLAG_ACK == 0 {
                    // Apply remote settings
                    self.remote_settings = H2Settings::parse(&frame.payload);
                    // Send SETTINGS ACK
                    responses.push(Frame::new(FrameType::Settings, FLAG_ACK, 0, Vec::new()));
                    FRAMES_RECEIVED.fetch_add(1, Ordering::Relaxed);
                }
            }
            FrameType::Headers => {
                let stream = self.get_or_create_stream(frame.stream_id);
                let headers = decode_headers(&frame.payload);
                stream.headers = headers;
                if frame.flags & FLAG_END_STREAM != 0 {
                    stream.state = StreamState::HalfClosedRemote;
                }
                FRAMES_RECEIVED.fetch_add(1, Ordering::Relaxed);
            }
            FrameType::Data => {
                if let Some(stream) = self.streams.iter_mut().find(|s| s.id == frame.stream_id) {
                    stream.data.extend_from_slice(&frame.payload);
                    stream.recv_window -= frame.payload.len() as i32;
                    self.conn_recv_window -= frame.payload.len() as i32;
                    if frame.flags & FLAG_END_STREAM != 0 {
                        stream.state = StreamState::HalfClosedRemote;
                    }
                    BYTES_RECEIVED.fetch_add(frame.payload.len() as u64, Ordering::Relaxed);
                }
                FRAMES_RECEIVED.fetch_add(1, Ordering::Relaxed);
            }
            FrameType::WindowUpdate => {
                if frame.payload.len() >= 4 {
                    let increment = u32::from_be_bytes([
                        frame.payload[0] & 0x7F,
                        frame.payload[1],
                        frame.payload[2],
                        frame.payload[3],
                    ]) as i32;
                    if frame.stream_id == 0 {
                        self.conn_send_window += increment;
                    } else if let Some(stream) = self.streams.iter_mut()
                        .find(|s| s.id == frame.stream_id)
                    {
                        stream.send_window += increment;
                    }
                }
                FRAMES_RECEIVED.fetch_add(1, Ordering::Relaxed);
            }
            FrameType::Ping => {
                if frame.flags & FLAG_ACK == 0 {
                    // Reply with PING ACK
                    responses.push(Frame::new(
                        FrameType::Ping,
                        FLAG_ACK,
                        0,
                        frame.payload.clone(),
                    ));
                }
                FRAMES_RECEIVED.fetch_add(1, Ordering::Relaxed);
            }
            FrameType::GoAway => {
                // Close all streams
                for stream in &mut self.streams {
                    stream.state = StreamState::Closed;
                }
                FRAMES_RECEIVED.fetch_add(1, Ordering::Relaxed);
            }
            FrameType::RstStream => {
                if let Some(stream) = self.streams.iter_mut()
                    .find(|s| s.id == frame.stream_id)
                {
                    stream.state = StreamState::Closed;
                }
                FRAMES_RECEIVED.fetch_add(1, Ordering::Relaxed);
            }
            FrameType::Priority | FrameType::Continuation => {
                FRAMES_RECEIVED.fetch_add(1, Ordering::Relaxed);
            }
            FrameType::PushPromise => {
                FRAMES_RECEIVED.fetch_add(1, Ordering::Relaxed);
            }
        }
        responses
    }

    /// Build a PUSH_PROMISE frame for server push.
    pub fn push_promise(
        &mut self,
        stream_id: u32,
        path: &str,
        headers: &[(String, String)],
    ) -> Vec<Frame> {
        let push_stream_id = self.next_push_stream_id;
        self.next_push_stream_id += 2;

        // Create the promised stream
        let stream = self.get_or_create_stream(push_stream_id);
        stream.headers = headers.to_vec();

        // Build PUSH_PROMISE payload: 4-byte promised stream ID + header block
        let mut payload = Vec::new();
        let psid = push_stream_id & 0x7FFF_FFFF;
        payload.push((psid >> 24) as u8);
        payload.push((psid >> 16) as u8);
        payload.push((psid >> 8) as u8);
        payload.push(psid as u8);

        let mut push_headers = vec![
            (String::from(":method"), String::from("GET")),
            (String::from(":path"), String::from(path)),
        ];
        push_headers.extend_from_slice(headers);
        let hpack_block = encode_headers(&push_headers);
        payload.extend_from_slice(&hpack_block);

        PUSHES_SENT.fetch_add(1, Ordering::Relaxed);

        vec![Frame::new(
            FrameType::PushPromise,
            FLAG_END_HEADERS,
            stream_id,
            payload,
        )]
    }

    /// Build a WINDOW_UPDATE frame.
    pub fn window_update(stream_id: u32, increment: u32) -> Frame {
        let mut payload = Vec::with_capacity(4);
        let inc = increment & 0x7FFF_FFFF;
        payload.push((inc >> 24) as u8);
        payload.push((inc >> 16) as u8);
        payload.push((inc >> 8) as u8);
        payload.push(inc as u8);
        Frame::new(FrameType::WindowUpdate, 0, stream_id, payload)
    }

    /// Build a GOAWAY frame.
    pub fn goaway(&self, last_stream_id: u32, error_code: u32) -> Frame {
        let mut payload = Vec::with_capacity(8);
        let lsid = last_stream_id & 0x7FFF_FFFF;
        payload.push((lsid >> 24) as u8);
        payload.push((lsid >> 16) as u8);
        payload.push((lsid >> 8) as u8);
        payload.push(lsid as u8);
        payload.push((error_code >> 24) as u8);
        payload.push((error_code >> 16) as u8);
        payload.push((error_code >> 8) as u8);
        payload.push(error_code as u8);
        Frame::new(FrameType::GoAway, 0, 0, payload)
    }
}

// ---------------------------------------------------------------------------
// Global statistics
// ---------------------------------------------------------------------------

/// Total connections handled.
static TOTAL_CONNECTIONS: AtomicU64 = AtomicU64::new(0);

/// Total frames received.
static FRAMES_RECEIVED: AtomicU64 = AtomicU64::new(0);

/// Total frames sent.
static FRAMES_SENT: AtomicU64 = AtomicU64::new(0);

/// Total bytes received (DATA frames).
static BYTES_RECEIVED: AtomicU64 = AtomicU64::new(0);

/// Total bytes sent (DATA frames).
static BYTES_SENT: AtomicU64 = AtomicU64::new(0);

/// Total server pushes sent.
static PUSHES_SENT: AtomicU64 = AtomicU64::new(0);

/// Active stream count (approximate).
static ACTIVE_STREAMS: AtomicU32 = AtomicU32::new(0);

/// Active HTTP/2 connections tracked globally.
static CONNECTIONS: Mutex<Vec<H2ConnInfo>> = Mutex::new(Vec::new());

/// Summary info for a tracked connection.
struct H2ConnInfo {
    id: u32,
    peer_ip: [u8; 4],
    stream_count: u32,
    settings_acked: bool,
}

static NEXT_CONN_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the HTTP/2 subsystem.
pub fn init() {
    crate::serial_println!("[http2] HTTP/2 protocol support initialized");
    crate::klog_println!("[http2] initialized");
}

/// Return HTTP/2 subsystem information.
pub fn http2_info() -> String {
    let conns = CONNECTIONS.lock();
    let active = conns.len();
    format!(
        "MerlionOS HTTP/2 Protocol (RFC 7540)\n\
         Status:                running\n\
         Active connections:    {}\n\
         Max concurrent streams: {}\n\
         Initial window size:   {} bytes\n\
         Max frame size:        {} bytes\n\
         Header table size:     {} bytes\n\
         Server push:           enabled\n\
         HPACK static entries:  {}\n\
         Frame types:           DATA, HEADERS, PRIORITY, RST_STREAM,\n\
                                SETTINGS, PUSH_PROMISE, PING, GOAWAY,\n\
                                WINDOW_UPDATE, CONTINUATION\n",
        active,
        DEFAULT_MAX_CONCURRENT_STREAMS,
        DEFAULT_INITIAL_WINDOW_SIZE,
        DEFAULT_MAX_FRAME_SIZE,
        DEFAULT_HEADER_TABLE_SIZE,
        HPACK_STATIC_TABLE.len() - 1,
    )
}

/// Return HTTP/2 statistics.
pub fn http2_stats() -> String {
    let total_conns = TOTAL_CONNECTIONS.load(Ordering::Relaxed);
    let active = CONNECTIONS.lock().len();
    let frames_rx = FRAMES_RECEIVED.load(Ordering::Relaxed);
    let frames_tx = FRAMES_SENT.load(Ordering::Relaxed);
    let bytes_rx = BYTES_RECEIVED.load(Ordering::Relaxed);
    let bytes_tx = BYTES_SENT.load(Ordering::Relaxed);
    let pushes = PUSHES_SENT.load(Ordering::Relaxed);
    format!(
        "HTTP/2 Statistics\n\
         Total connections:  {}\n\
         Active connections: {}\n\
         Frames received:    {}\n\
         Frames sent:        {}\n\
         Bytes received:     {}\n\
         Bytes sent:         {}\n\
         Server pushes:      {}\n",
        total_conns, active, frames_rx, frames_tx, bytes_rx, bytes_tx, pushes,
    )
}

/// List active HTTP/2 streams across all connections.
pub fn list_streams() -> String {
    let conns = CONNECTIONS.lock();
    if conns.is_empty() {
        return String::from("No active HTTP/2 connections.\n");
    }
    let mut out = format!("{:<6} {:<16} {:<10} {:<10}\n",
        "ConnID", "Peer IP", "Streams", "Settings");
    out.push_str(&format!("{}\n", "-".repeat(45)));
    for c in conns.iter() {
        let ip = format!("{}.{}.{}.{}",
            c.peer_ip[0], c.peer_ip[1], c.peer_ip[2], c.peer_ip[3]);
        let acked = if c.settings_acked { "acked" } else { "pending" };
        out.push_str(&format!("{:<6} {:<16} {:<10} {:<10}\n",
            c.id, ip, c.stream_count, acked));
    }
    out
}

/// Register a new HTTP/2 connection for tracking.
pub fn register_connection(peer_ip: [u8; 4]) -> u32 {
    let id = NEXT_CONN_ID.fetch_add(1, Ordering::SeqCst);
    TOTAL_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
    let mut conns = CONNECTIONS.lock();
    conns.push(H2ConnInfo {
        id,
        peer_ip,
        stream_count: 0,
        settings_acked: false,
    });
    id
}

/// Remove a tracked connection.
pub fn unregister_connection(id: u32) {
    let mut conns = CONNECTIONS.lock();
    conns.retain(|c| c.id != id);
}
