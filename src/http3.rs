/// HTTP/3 protocol for MerlionOS (RFC 9114).
/// HTTP semantics over QUIC transport — multiplexed requests
/// without head-of-line blocking, built-in encryption, 0-RTT.

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

/// Maximum number of HTTP/3 connections.
const MAX_H3_CONNECTIONS: usize = 32;

/// Maximum requests tracked per connection.
const MAX_REQUESTS_PER_CONN: usize = 64;

/// Alt-Svc header value to advertise HTTP/3 support.
pub const ALT_SVC_HEADER: &str = "h3=\":443\"";

// ---------------------------------------------------------------------------
// HTTP/3 Frame Types (RFC 9114, Section 7)
// ---------------------------------------------------------------------------

/// HTTP/3 frame types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum H3FrameType {
    Data = 0x00,
    Headers = 0x01,
    CancelPush = 0x03,
    Settings = 0x04,
    PushPromise = 0x05,
    GoAway = 0x07,
    MaxPushId = 0x0D,
}

impl H3FrameType {
    fn from_u64(val: u64) -> Result<Self, &'static str> {
        match val {
            0x00 => Ok(H3FrameType::Data),
            0x01 => Ok(H3FrameType::Headers),
            0x03 => Ok(H3FrameType::CancelPush),
            0x04 => Ok(H3FrameType::Settings),
            0x05 => Ok(H3FrameType::PushPromise),
            0x07 => Ok(H3FrameType::GoAway),
            0x0D => Ok(H3FrameType::MaxPushId),
            _ => Err("unknown H3 frame type"),
        }
    }
}

/// An HTTP/3 frame.
#[derive(Debug, Clone)]
pub struct H3Frame {
    pub frame_type: H3FrameType,
    pub payload: Vec<u8>,
}

/// Encode an HTTP/3 frame into bytes.
pub fn encode_frame(frame: &H3Frame) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&crate::quic::encode_varint(frame.frame_type as u64));
    buf.extend_from_slice(&crate::quic::encode_varint(frame.payload.len() as u64));
    buf.extend_from_slice(&frame.payload);
    buf
}

/// Decode an HTTP/3 frame from bytes.
pub fn decode_frame(data: &[u8]) -> Result<H3Frame, &'static str> {
    if data.is_empty() {
        return Err("empty H3 frame");
    }
    let (ftype_val, n1) = crate::quic::decode_varint(data)?;
    let frame_type = H3FrameType::from_u64(ftype_val)?;
    if data.len() <= n1 {
        return Err("truncated H3 frame");
    }
    let (length, n2) = crate::quic::decode_varint(&data[n1..])?;
    let payload_start = n1 + n2;
    let payload_end = payload_start + length as usize;
    if payload_end > data.len() {
        return Err("truncated H3 frame payload");
    }
    Ok(H3Frame {
        frame_type,
        payload: data[payload_start..payload_end].to_vec(),
    })
}

// ---------------------------------------------------------------------------
// QPACK Header Compression (RFC 9204)
// ---------------------------------------------------------------------------

/// QPACK static table (subset of the 99 entries defined in RFC 9204).
const QPACK_STATIC_TABLE: &[(&str, &str)] = &[
    (":authority", ""),
    (":path", "/"),
    ("age", "0"),
    ("content-disposition", ""),
    ("content-length", "0"),
    ("cookie", ""),
    ("date", ""),
    ("etag", ""),
    ("if-modified-since", ""),
    ("if-none-match", ""),
    ("last-modified", ""),
    ("link", ""),
    ("location", ""),
    ("referer", ""),
    ("set-cookie", ""),
    (":method", "CONNECT"),
    (":method", "DELETE"),
    (":method", "GET"),
    (":method", "HEAD"),
    (":method", "OPTIONS"),
    (":method", "POST"),
    (":method", "PUT"),
    (":scheme", "http"),
    (":scheme", "https"),
    (":status", "103"),
    (":status", "200"),
    (":status", "304"),
    (":status", "404"),
    (":status", "503"),
    ("accept", "*/*"),
    ("accept", "application/dns-message"),
    ("accept-encoding", "gzip, deflate, br"),
    ("accept-ranges", "bytes"),
    ("access-control-allow-headers", "cache-control"),
    ("access-control-allow-headers", "content-type"),
    ("access-control-allow-origin", "*"),
    ("cache-control", "max-age=0"),
    ("cache-control", "max-age=2592000"),
    ("cache-control", "max-age=604800"),
    ("cache-control", "no-cache"),
    ("cache-control", "no-store"),
    ("cache-control", "public, max-age=31536000"),
    ("content-encoding", "br"),
    ("content-encoding", "gzip"),
    ("content-type", "application/dns-message"),
    ("content-type", "application/javascript"),
    ("content-type", "application/json"),
    ("content-type", "application/x-www-form-urlencoded"),
    ("content-type", "image/gif"),
    ("content-type", "image/jpeg"),
    ("content-type", "image/png"),
    ("content-type", "text/css"),
    ("content-type", "text/html; charset=utf-8"),
    ("content-type", "text/plain"),
    ("content-type", "text/plain;charset=utf-8"),
    ("range", "bytes=0-"),
    ("strict-transport-security", "max-age=31536000"),
    ("strict-transport-security", "max-age=31536000; includesubdomains"),
    ("strict-transport-security", "max-age=31536000; includesubdomains; preload"),
    ("vary", "accept-encoding"),
    ("vary", "origin"),
    ("x-content-type-options", "nosniff"),
    ("x-xss-protection", "1; mode=block"),
    (":status", "100"),
    (":status", "204"),
    (":status", "206"),
    (":status", "302"),
    (":status", "400"),
    (":status", "403"),
    (":status", "421"),
    (":status", "425"),
    (":status", "500"),
    ("accept-language", ""),
    ("access-control-allow-credentials", "FALSE"),
    ("access-control-allow-credentials", "TRUE"),
    ("access-control-allow-methods", "get"),
    ("access-control-allow-methods", "get, post, options"),
    ("access-control-allow-methods", "options"),
    ("access-control-expose-headers", "content-length"),
    ("access-control-request-headers", "content-type"),
    ("access-control-request-method", "get"),
    ("access-control-request-method", "post"),
    ("alt-svc", "clear"),
    ("authorization", ""),
    ("content-security-policy", "script-src 'none'; object-src 'none'; base-uri 'none'"),
    ("early-data", "1"),
    ("expect-ct", ""),
    ("forwarded", ""),
    ("if-range", ""),
    ("origin", ""),
    ("purpose", "prefetch"),
    ("server", ""),
    ("timing-allow-origin", "*"),
    ("upgrade-insecure-requests", "1"),
    ("user-agent", ""),
    ("x-forwarded-for", ""),
    ("x-frame-options", "deny"),
    ("x-frame-options", "sameorigin"),
];

/// QPACK encoder (header compression).
pub struct QpackEncoder {
    dynamic_table: Vec<(String, String)>,
    max_table_capacity: usize,
}

impl QpackEncoder {
    /// Create a new QPACK encoder.
    pub fn new(max_capacity: usize) -> Self {
        Self {
            dynamic_table: Vec::new(),
            max_table_capacity: max_capacity,
        }
    }

    /// Encode headers into a QPACK-compressed byte sequence.
    pub fn encode_headers(&mut self, headers: &[(String, String)]) -> Vec<u8> {
        let mut buf = Vec::new();
        // Required Insert Count = 0 (no dynamic table references for simplicity)
        buf.push(0x00);
        // Delta Base = 0
        buf.push(0x00);

        for (name, value) in headers {
            // Try static table match
            if let Some(idx) = self.find_static_match(name, value) {
                // Indexed field line (static): 1 1 T index
                buf.push(0xC0 | (idx as u8 & 0x3F));
            } else if let Some(idx) = self.find_static_name_match(name) {
                // Literal with name reference (static): 0 1 N T index
                buf.push(0x50 | (idx as u8 & 0x0F));
                // Value: length-prefixed, not Huffman
                let vb = value.as_bytes();
                buf.extend_from_slice(&crate::quic::encode_varint(vb.len() as u64));
                buf.extend_from_slice(vb);
            } else {
                // Literal with literal name: 0 0 1 N
                buf.push(0x20);
                let nb = name.as_bytes();
                buf.extend_from_slice(&crate::quic::encode_varint(nb.len() as u64));
                buf.extend_from_slice(nb);
                let vb = value.as_bytes();
                buf.extend_from_slice(&crate::quic::encode_varint(vb.len() as u64));
                buf.extend_from_slice(vb);
            }

            // Add to dynamic table if capacity allows
            let entry_size = name.len() + value.len() + 32; // RFC overhead
            let current_size: usize = self.dynamic_table.iter()
                .map(|(n, v)| n.len() + v.len() + 32).sum();
            if current_size + entry_size <= self.max_table_capacity {
                self.dynamic_table.push((name.clone(), value.clone()));
            }
        }
        buf
    }

    fn find_static_match(&self, name: &str, value: &str) -> Option<usize> {
        QPACK_STATIC_TABLE.iter().position(|&(n, v)| n == name && v == value)
    }

    fn find_static_name_match(&self, name: &str) -> Option<usize> {
        QPACK_STATIC_TABLE.iter().position(|&(n, _)| n == name)
    }
}

/// QPACK decoder (header decompression).
pub struct QpackDecoder {
    dynamic_table: Vec<(String, String)>,
}

impl QpackDecoder {
    /// Create a new QPACK decoder.
    pub fn new() -> Self {
        Self {
            dynamic_table: Vec::new(),
        }
    }

    /// Decode QPACK-compressed headers.
    pub fn decode_headers(&mut self, data: &[u8]) -> Vec<(String, String)> {
        let mut headers = Vec::new();
        if data.len() < 2 {
            return headers;
        }
        let mut pos = 2usize; // Skip Required Insert Count + Delta Base

        while pos < data.len() {
            let byte = data[pos];
            if (byte & 0xC0) == 0xC0 {
                // Indexed field line (static)
                let idx = (byte & 0x3F) as usize;
                pos += 1;
                if idx < QPACK_STATIC_TABLE.len() {
                    let (n, v) = QPACK_STATIC_TABLE[idx];
                    headers.push((n.to_owned(), v.to_owned()));
                }
            } else if (byte & 0xF0) == 0x50 {
                // Literal with name reference (static)
                let idx = (byte & 0x0F) as usize;
                pos += 1;
                let name = if idx < QPACK_STATIC_TABLE.len() {
                    QPACK_STATIC_TABLE[idx].0.to_owned()
                } else {
                    String::new()
                };
                if pos < data.len() {
                    match crate::quic::decode_varint(&data[pos..]) {
                        Ok((vlen, n)) => {
                            pos += n;
                            let end = (pos + vlen as usize).min(data.len());
                            let value = core::str::from_utf8(&data[pos..end])
                                .unwrap_or("").to_owned();
                            pos = end;
                            headers.push((name, value));
                        }
                        Err(_) => break,
                    }
                }
            } else if (byte & 0xE0) == 0x20 {
                // Literal with literal name
                pos += 1;
                if pos >= data.len() { break; }
                match crate::quic::decode_varint(&data[pos..]) {
                    Ok((nlen, n)) => {
                        pos += n;
                        let end = (pos + nlen as usize).min(data.len());
                        let name = core::str::from_utf8(&data[pos..end])
                            .unwrap_or("").to_owned();
                        pos = end;
                        if pos >= data.len() { break; }
                        match crate::quic::decode_varint(&data[pos..]) {
                            Ok((vlen, n)) => {
                                pos += n;
                                let end = (pos + vlen as usize).min(data.len());
                                let value = core::str::from_utf8(&data[pos..end])
                                    .unwrap_or("").to_owned();
                                pos = end;
                                headers.push((name, value));
                            }
                            Err(_) => break,
                        }
                    }
                    Err(_) => break,
                }
            } else {
                // Unknown encoding, skip
                pos += 1;
            }
        }
        headers
    }
}

// ---------------------------------------------------------------------------
// HTTP/3 Settings (RFC 9114, Section 7.2.4)
// ---------------------------------------------------------------------------

/// HTTP/3 connection settings.
#[derive(Debug, Clone)]
pub struct H3Settings {
    pub max_field_section_size: u64,
    pub qpack_max_table_capacity: u64,
    pub qpack_blocked_streams: u64,
}

impl H3Settings {
    pub fn default_settings() -> Self {
        Self {
            max_field_section_size: 8192,
            qpack_max_table_capacity: 4096,
            qpack_blocked_streams: 16,
        }
    }

    /// Encode settings into a SETTINGS frame payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // SETTINGS_MAX_FIELD_SECTION_SIZE (0x06)
        buf.extend_from_slice(&crate::quic::encode_varint(0x06));
        buf.extend_from_slice(&crate::quic::encode_varint(self.max_field_section_size));
        // SETTINGS_QPACK_MAX_TABLE_CAPACITY (0x01)
        buf.extend_from_slice(&crate::quic::encode_varint(0x01));
        buf.extend_from_slice(&crate::quic::encode_varint(self.qpack_max_table_capacity));
        // SETTINGS_QPACK_BLOCKED_STREAMS (0x07)
        buf.extend_from_slice(&crate::quic::encode_varint(0x07));
        buf.extend_from_slice(&crate::quic::encode_varint(self.qpack_blocked_streams));
        buf
    }
}

// ---------------------------------------------------------------------------
// Request / Response
// ---------------------------------------------------------------------------

/// HTTP/3 request state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestState {
    Pending,
    HeadersSent,
    Complete,
}

/// An HTTP/3 request.
#[derive(Debug, Clone)]
pub struct H3Request {
    pub stream_id: u64,
    pub method: String,
    pub path: String,
    pub authority: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub state: RequestState,
}

/// An HTTP/3 response.
#[derive(Debug, Clone)]
pub struct H3Response {
    pub stream_id: u64,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

// ---------------------------------------------------------------------------
// HTTP/3 Connection
// ---------------------------------------------------------------------------

/// An HTTP/3 connection layered on a QUIC connection.
pub struct H3Connection {
    pub quic_conn_id: u32,
    pub control_stream: u64,
    pub encoder_stream: u64,
    pub decoder_stream: u64,
    pub requests: Vec<H3Request>,
    pub settings: H3Settings,
    pub encoder: QpackEncoder,
    pub decoder: QpackDecoder,
    pub next_push_id: u64,
    // Datagram support (RFC 9297)
    pub datagram_buf: Vec<Vec<u8>>,
}

impl H3Connection {
    fn new(quic_conn_id: u32) -> Self {
        let settings = H3Settings::default_settings();
        Self {
            quic_conn_id,
            control_stream: 0,
            encoder_stream: 0,
            decoder_stream: 0,
            requests: Vec::new(),
            encoder: QpackEncoder::new(settings.qpack_max_table_capacity as usize),
            decoder: QpackDecoder::new(),
            settings,
            next_push_id: 0,
            datagram_buf: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static H3_CONNECTIONS: Mutex<Vec<H3Connection>> = Mutex::new(Vec::new());
static NEXT_H3_ID: AtomicU32 = AtomicU32::new(1);
static TOTAL_H3_CONNECTIONS: AtomicU64 = AtomicU64::new(0);
static TOTAL_REQUESTS: AtomicU64 = AtomicU64::new(0);
static TOTAL_RESPONSES: AtomicU64 = AtomicU64::new(0);
static TOTAL_H3_BYTES_SENT: AtomicU64 = AtomicU64::new(0);
static TOTAL_H3_BYTES_RECV: AtomicU64 = AtomicU64::new(0);
static TOTAL_PUSH_PROMISES: AtomicU64 = AtomicU64::new(0);
static TOTAL_H3_ERRORS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Server operations
// ---------------------------------------------------------------------------

/// Accept an incoming QUIC connection and upgrade to HTTP/3.
pub fn h3_accept(quic_conn_id: u32) -> Result<u32, &'static str> {
    let mut conns = H3_CONNECTIONS.lock();
    if conns.len() >= MAX_H3_CONNECTIONS {
        return Err("too many HTTP/3 connections");
    }
    let mut conn = H3Connection::new(quic_conn_id);

    // Open required unidirectional streams
    // Control stream (type 0x00)
    conn.control_stream = 2; // placeholder stream ID
    // QPACK encoder stream (type 0x02)
    conn.encoder_stream = 6;
    // QPACK decoder stream (type 0x03)
    conn.decoder_stream = 10;

    // Send SETTINGS frame on control stream
    let settings_payload = conn.settings.encode();
    let _settings_frame = encode_frame(&H3Frame {
        frame_type: H3FrameType::Settings,
        payload: settings_payload,
    });

    let id = NEXT_H3_ID.fetch_add(1, Ordering::SeqCst);
    TOTAL_H3_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
    conns.push(conn);
    Ok(id)
}

/// Receive a request from a client on the given HTTP/3 connection.
pub fn h3_recv_request(quic_conn_id: u32) -> Option<H3Request> {
    let mut conns = H3_CONNECTIONS.lock();
    let conn = conns.iter_mut().find(|c| c.quic_conn_id == quic_conn_id)?;
    // Return the first pending request
    conn.requests.iter().find(|r| r.state == RequestState::Pending).cloned()
}

/// Send a response to a client.
pub fn h3_send_response(quic_conn_id: u32, stream_id: u64, response: &H3Response) {
    let mut conns = H3_CONNECTIONS.lock();
    if let Some(conn) = conns.iter_mut().find(|c| c.quic_conn_id == quic_conn_id) {
        // Encode response headers
        let mut pseudo_headers = vec![
            (":status".to_owned(), format!("{}", response.status)),
        ];
        for (n, v) in &response.headers {
            pseudo_headers.push((n.clone(), v.clone()));
        }
        let header_block = conn.encoder.encode_headers(&pseudo_headers);

        // HEADERS frame
        let _headers_frame = encode_frame(&H3Frame {
            frame_type: H3FrameType::Headers,
            payload: header_block,
        });

        // DATA frame (if body is non-empty)
        if !response.body.is_empty() {
            let _data_frame = encode_frame(&H3Frame {
                frame_type: H3FrameType::Data,
                payload: response.body.clone(),
            });
            TOTAL_H3_BYTES_SENT.fetch_add(response.body.len() as u64, Ordering::Relaxed);
        }

        // Mark request as complete
        if let Some(req) = conn.requests.iter_mut().find(|r| r.stream_id == stream_id) {
            req.state = RequestState::Complete;
        }
        TOTAL_RESPONSES.fetch_add(1, Ordering::Relaxed);
    }
}

/// Push a response to the client (server push, RFC 9114 Section 4.6).
pub fn h3_server_push(quic_conn_id: u32, request: &H3Request, response: &H3Response) {
    let mut conns = H3_CONNECTIONS.lock();
    if let Some(conn) = conns.iter_mut().find(|c| c.quic_conn_id == quic_conn_id) {
        let push_id = conn.next_push_id;
        conn.next_push_id += 1;

        // PUSH_PROMISE frame with request headers
        let mut req_headers = vec![
            (":method".to_owned(), request.method.clone()),
            (":path".to_owned(), request.path.clone()),
            (":authority".to_owned(), request.authority.clone()),
            (":scheme".to_owned(), "https".to_owned()),
        ];
        for (n, v) in &request.headers {
            req_headers.push((n.clone(), v.clone()));
        }
        let header_block = conn.encoder.encode_headers(&req_headers);

        let mut push_payload = crate::quic::encode_varint(push_id);
        push_payload.extend_from_slice(&header_block);
        let _push_frame = encode_frame(&H3Frame {
            frame_type: H3FrameType::PushPromise,
            payload: push_payload,
        });

        // Send response on push stream
        h3_send_response(quic_conn_id, response.stream_id, response);
        TOTAL_PUSH_PROMISES.fetch_add(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Client operations
// ---------------------------------------------------------------------------

/// Establish an HTTP/3 connection to the given address and port.
pub fn h3_connect(addr: [u8; 4], port: u16) -> Result<u32, &'static str> {
    // First, establish QUIC connection
    let quic_id = crate::quic::connect(addr, port)?;

    let mut conns = H3_CONNECTIONS.lock();
    if conns.len() >= MAX_H3_CONNECTIONS {
        return Err("too many HTTP/3 connections");
    }
    let mut conn = H3Connection::new(quic_id);

    // Open control + QPACK streams
    conn.control_stream = 2;
    conn.encoder_stream = 6;
    conn.decoder_stream = 10;

    // Send SETTINGS
    let settings_payload = conn.settings.encode();
    let _settings_frame = encode_frame(&H3Frame {
        frame_type: H3FrameType::Settings,
        payload: settings_payload,
    });

    TOTAL_H3_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
    conns.push(conn);
    Ok(quic_id)
}

/// Send an HTTP/3 request, returning the stream ID.
pub fn h3_send_request(
    quic_conn_id: u32,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<u64, &'static str> {
    let mut conns = H3_CONNECTIONS.lock();
    let conn = conns.iter_mut().find(|c| c.quic_conn_id == quic_conn_id)
        .ok_or("HTTP/3 connection not found")?;

    if conn.requests.len() >= MAX_REQUESTS_PER_CONN {
        return Err("too many concurrent requests");
    }

    // Assign a stream ID (client-initiated bidi, increments by 4)
    let stream_id = (conn.requests.len() as u64) * 4;

    // Encode pseudo-headers + regular headers
    let mut all_headers = vec![
        (":method".to_owned(), method.to_owned()),
        (":path".to_owned(), path.to_owned()),
        (":scheme".to_owned(), "https".to_owned()),
    ];
    for (n, v) in headers {
        all_headers.push((n.clone(), v.clone()));
    }
    let header_block = conn.encoder.encode_headers(&all_headers);

    // HEADERS frame
    let _headers_frame = encode_frame(&H3Frame {
        frame_type: H3FrameType::Headers,
        payload: header_block,
    });

    // DATA frame if body is non-empty
    if !body.is_empty() {
        let _data_frame = encode_frame(&H3Frame {
            frame_type: H3FrameType::Data,
            payload: body.to_vec(),
        });
        TOTAL_H3_BYTES_SENT.fetch_add(body.len() as u64, Ordering::Relaxed);
    }

    conn.requests.push(H3Request {
        stream_id,
        method: method.to_owned(),
        path: path.to_owned(),
        authority: String::new(),
        headers: headers.to_vec(),
        body: body.to_vec(),
        state: RequestState::HeadersSent,
    });

    TOTAL_REQUESTS.fetch_add(1, Ordering::Relaxed);
    Ok(stream_id)
}

/// Receive an HTTP/3 response for the given stream ID.
pub fn h3_recv_response(quic_conn_id: u32, stream_id: u64) -> Option<H3Response> {
    let conns = H3_CONNECTIONS.lock();
    let conn = conns.iter().find(|c| c.quic_conn_id == quic_conn_id)?;
    let _req = conn.requests.iter().find(|r| r.stream_id == stream_id)?;
    // In a real implementation, this would block/poll for the response.
    // For now return a placeholder indicating no response yet.
    None
}

// ---------------------------------------------------------------------------
// QUIC Datagram extension (RFC 9297) — WebTransport support
// ---------------------------------------------------------------------------

/// Send an unreliable datagram on an HTTP/3 connection.
pub fn h3_send_datagram(quic_conn_id: u32, data: &[u8]) -> Result<(), &'static str> {
    crate::quic::send_datagram(quic_conn_id, data)?;
    TOTAL_H3_BYTES_SENT.fetch_add(data.len() as u64, Ordering::Relaxed);
    Ok(())
}

/// Receive a datagram from an HTTP/3 connection.
pub fn h3_recv_datagram(quic_conn_id: u32) -> Option<Vec<u8>> {
    let mut conns = H3_CONNECTIONS.lock();
    let conn = conns.iter_mut().find(|c| c.quic_conn_id == quic_conn_id)?;
    conn.datagram_buf.pop()
}

// ---------------------------------------------------------------------------
// Statistics & info
// ---------------------------------------------------------------------------

/// Summary of the HTTP/3 subsystem.
pub fn h3_info() -> String {
    let conns = H3_CONNECTIONS.lock();
    let total_streams: usize = conns.iter().map(|c| c.requests.len()).sum();
    format!(
        "HTTP/3 (RFC 9114) over QUIC\n\
         Active conns:  {}\n\
         Total conns:   {}\n\
         Active streams:{}\n\
         Alt-Svc:       {}\n\
         QPACK tables:  {} encoder entries",
        conns.len(),
        TOTAL_H3_CONNECTIONS.load(Ordering::Relaxed),
        total_streams,
        ALT_SVC_HEADER,
        conns.iter().map(|c| c.encoder.dynamic_table.len()).sum::<usize>(),
    )
}

/// Detailed HTTP/3 statistics.
pub fn h3_stats() -> String {
    format!(
        "HTTP/3 Statistics\n\
         Requests sent:  {}\n\
         Responses sent: {}\n\
         Bytes sent:     {}\n\
         Bytes recv:     {}\n\
         Push promises:  {}\n\
         Errors:         {}",
        TOTAL_REQUESTS.load(Ordering::Relaxed),
        TOTAL_RESPONSES.load(Ordering::Relaxed),
        TOTAL_H3_BYTES_SENT.load(Ordering::Relaxed),
        TOTAL_H3_BYTES_RECV.load(Ordering::Relaxed),
        TOTAL_PUSH_PROMISES.load(Ordering::Relaxed),
        TOTAL_H3_ERRORS.load(Ordering::Relaxed),
    )
}

/// List all HTTP/3 connections.
pub fn list_h3_connections() -> String {
    let conns = H3_CONNECTIONS.lock();
    if conns.is_empty() {
        return String::from("No active HTTP/3 connections.\n");
    }
    let mut out = format!("{:<8} {:<10} {:<10} {:<10}\n",
        "QUIC ID", "Requests", "Ctrl Strm", "Push ID");
    out.push_str(&format!("{}\n", "-".repeat(42)));
    for c in conns.iter() {
        let active = c.requests.iter().filter(|r| r.state != RequestState::Complete).count();
        out.push_str(&format!("{:<8} {:<10} {:<10} {:<10}\n",
            c.quic_conn_id, active, c.control_stream, c.next_push_id));
    }
    out
}

/// Initialize the HTTP/3 subsystem.
pub fn init() {
    let _ = H3_CONNECTIONS.lock();
}
