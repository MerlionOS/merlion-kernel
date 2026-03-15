/// Simple RPC (Remote Procedure Call) framework for MerlionOS distributed computing.
///
/// Provides a lightweight binary RPC protocol over UDP. Messages use a simple
/// wire format with u32 length-prefixed fields, making them easy to serialize
/// and deserialize without external dependencies.
///
/// # Wire format
///
/// Each RPC message on the wire looks like:
///
/// ```text
/// [id: u64][method_len: u32][method: bytes][args_len: u32][args: bytes]
/// ```
///
/// Responses carry the same `id` with the method set to `"__response"` and the
/// payload in the args field.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;

use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default RPC port for MerlionOS services.
pub const RPC_DEFAULT_PORT: u16 = 9100;

/// Magic method name used in response messages.
const RESPONSE_METHOD: &str = "__response";

/// Maximum RPC payload size (64 KiB, fits a single UDP datagram).
const MAX_PAYLOAD: usize = 65000;

/// Monotonically increasing request ID generator.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------
// RpcMessage
// ---------------------------------------------------------------------------

/// A single RPC message, representing either a request or a response.
///
/// For requests, `method` names the remote procedure and `args` carries the
/// serialized arguments.  For responses, `response` holds the return value.
#[derive(Debug, Clone)]
pub struct RpcMessage {
    /// Unique identifier linking a request to its response.
    pub id: u64,
    /// Method name (e.g. `"ping"`, `"exec"`, `"stat"`).
    pub method: String,
    /// Serialized argument bytes (caller-defined encoding).
    pub args: Vec<u8>,
    /// Populated only in response messages; `None` for requests.
    pub response: Option<Vec<u8>>,
}

impl RpcMessage {
    /// Create a new RPC request with an auto-assigned ID.
    pub fn new_request(method: &str, args: Vec<u8>) -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            method: String::from(method),
            args,
            response: None,
        }
    }

    /// Create a response message for a given request ID.
    pub fn new_response(id: u64, data: Vec<u8>) -> Self {
        Self {
            id,
            method: String::from(RESPONSE_METHOD),
            args: data,
            response: None,
        }
    }

    /// Returns `true` if this message is a response (not a request).
    pub fn is_response(&self) -> bool {
        self.method == RESPONSE_METHOD
    }
}

// ---------------------------------------------------------------------------
// Serialization helpers — simple binary format
// ---------------------------------------------------------------------------

/// Serialize an [`RpcMessage`] into a length-prefixed binary blob.
///
/// Layout: `[id:8][method_len:4][method][args_len:4][args]`
pub fn serialize(msg: &RpcMessage) -> Vec<u8> {
    let method_bytes = msg.method.as_bytes();
    let total = 8 + 4 + method_bytes.len() + 4 + msg.args.len();
    let mut buf = Vec::with_capacity(4 + total);
    // Overall length prefix (excludes the prefix itself).
    buf.extend_from_slice(&(total as u32).to_be_bytes());
    // ID
    buf.extend_from_slice(&msg.id.to_be_bytes());
    // Method
    buf.extend_from_slice(&(method_bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(method_bytes);
    // Args
    buf.extend_from_slice(&(msg.args.len() as u32).to_be_bytes());
    buf.extend_from_slice(&msg.args);
    buf
}

/// Deserialize a binary blob produced by [`serialize`] back into an
/// [`RpcMessage`].  Returns `None` if the data is truncated or malformed.
pub fn deserialize(data: &[u8]) -> Option<RpcMessage> {
    if data.len() < 4 {
        return None;
    }
    let total_len = read_u32(&data[0..4]) as usize;
    let body = &data[4..];
    if body.len() < total_len || total_len < 8 + 4 {
        return None;
    }

    let id = read_u64(&body[0..8]);
    let method_len = read_u32(&body[8..12]) as usize;
    if body.len() < 12 + method_len + 4 {
        return None;
    }
    let method = core::str::from_utf8(&body[12..12 + method_len]).ok()?;
    let args_off = 12 + method_len;
    let args_len = read_u32(&body[args_off..args_off + 4]) as usize;
    if body.len() < args_off + 4 + args_len {
        return None;
    }
    let args = body[args_off + 4..args_off + 4 + args_len].to_vec();

    let is_resp = method == RESPONSE_METHOD;
    Some(RpcMessage {
        id,
        method: String::from(method),
        args: if is_resp { Vec::new() } else { args.clone() },
        response: if is_resp { Some(args) } else { None },
    })
}

// ---------------------------------------------------------------------------
// Primitive encoding / decoding helpers
// ---------------------------------------------------------------------------

/// Encode a UTF-8 string as `[len:4][bytes]`.
pub fn encode_string(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(4 + b.len());
    out.extend_from_slice(&(b.len() as u32).to_be_bytes());
    out.extend_from_slice(b);
    out
}

/// Decode a length-prefixed string from `data`.  Returns the string and the
/// number of bytes consumed, or `None` on failure.
pub fn decode_string(data: &[u8]) -> Option<(String, usize)> {
    if data.len() < 4 {
        return None;
    }
    let len = read_u32(&data[0..4]) as usize;
    if data.len() < 4 + len {
        return None;
    }
    let s = core::str::from_utf8(&data[4..4 + len]).ok()?;
    Some((String::from(s), 4 + len))
}

/// Encode a `u64` in big-endian.
pub fn encode_u64(val: u64) -> [u8; 8] {
    val.to_be_bytes()
}

/// Decode a big-endian `u64` from `data`.  Returns `None` if too short.
pub fn decode_u64(data: &[u8]) -> Option<u64> {
    if data.len() < 8 {
        return None;
    }
    Some(read_u64(&data[0..8]))
}

/// Read a big-endian `u32` from a 4-byte slice.
fn read_u32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Read a big-endian `u64` from an 8-byte slice.
fn read_u64(b: &[u8]) -> u64 {
    u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

// ---------------------------------------------------------------------------
// RpcServer
// ---------------------------------------------------------------------------

/// Type alias for an RPC method handler function.
///
/// Receives the raw argument bytes and returns a result payload or an error
/// description.
pub type HandlerFn = fn(&[u8]) -> Result<Vec<u8>, &'static str>;

/// A lightweight RPC server that dispatches incoming requests to registered
/// handler functions.
pub struct RpcServer {
    /// Registered (method_name, handler) pairs.
    handlers: Vec<(String, HandlerFn)>,
    /// UDP port this server listens on.
    pub port: u16,
}

impl RpcServer {
    /// Create a new RPC server bound to `port`.
    pub fn new(port: u16) -> Self {
        Self {
            handlers: Vec::new(),
            port,
        }
    }

    /// Register a handler for `method`.  If the method already exists, the
    /// previous handler is replaced.
    pub fn register(&mut self, method: &str, handler: HandlerFn) {
        // Replace existing handler if any.
        for entry in self.handlers.iter_mut() {
            if entry.0 == method {
                entry.1 = handler;
                return;
            }
        }
        self.handlers.push((String::from(method), handler));
    }

    /// Look up the handler for `method`.
    pub fn find_handler(&self, method: &str) -> Option<HandlerFn> {
        for (name, h) in &self.handlers {
            if name == method {
                return Some(*h);
            }
        }
        None
    }

    /// Dispatch an incoming [`RpcMessage`] to the appropriate handler and
    /// return a serialized response ready to send back.
    pub fn dispatch(&self, msg: &RpcMessage) -> Vec<u8> {
        let result = match self.find_handler(&msg.method) {
            Some(handler) => match handler(&msg.args) {
                Ok(data) => data,
                Err(e) => {
                    let mut err = vec![0x01]; // error marker
                    err.extend_from_slice(e.as_bytes());
                    err
                }
            },
            None => {
                let mut err = vec![0x01]; // error marker
                err.extend_from_slice(b"unknown method");
                err
            }
        };
        let resp = RpcMessage::new_response(msg.id, result);
        serialize(&resp)
    }
}

// ---------------------------------------------------------------------------
// RpcClient
// ---------------------------------------------------------------------------

/// A minimal RPC client that builds request messages and sends them over UDP.
pub struct RpcClient {
    /// Local UDP port used as the source.
    pub src_port: u16,
}

impl RpcClient {
    /// Create a new client with the given local source port.
    pub fn new(src_port: u16) -> Self {
        Self { src_port }
    }

    /// Build a serialized RPC request for `method` with `args`.
    pub fn build_request(&self, method: &str, args: Vec<u8>) -> (u64, Vec<u8>) {
        let msg = RpcMessage::new_request(method, args);
        let id = msg.id;
        (id, serialize(&msg))
    }
}

// ---------------------------------------------------------------------------
// send_rpc — high-level send helper
// ---------------------------------------------------------------------------

/// Send an RPC request to `dst_ip:port` for `method` with `args`.
///
/// Serializes the message, transmits it via [`crate::netstack::send_udp`],
/// and returns the request ID on success.  The caller is responsible for
/// listening for the matching response (by ID) on its own receive path.
///
/// Returns `Err` if the payload exceeds [`MAX_PAYLOAD`] or if the UDP send
/// fails at the network layer.
pub fn send_rpc(
    dst_ip: [u8; 4],
    port: u16,
    method: &str,
    args: &[u8],
) -> Result<u64, &'static str> {
    if args.len() > MAX_PAYLOAD {
        return Err("rpc: payload exceeds maximum size");
    }
    let msg = RpcMessage::new_request(method, args.to_vec());
    let id = msg.id;
    let wire = serialize(&msg);

    let ok = crate::netstack::send_udp(dst_ip, RPC_DEFAULT_PORT, port, &wire);
    if ok {
        Ok(id)
    } else {
        Err("rpc: udp send failed")
    }
}

// ---------------------------------------------------------------------------
// Remote exec stub
// ---------------------------------------------------------------------------

/// Stub for remote command execution over RPC.
///
/// Encodes `command` as the argument payload and sends an `"exec"` RPC to the
/// target node.  A real implementation would await the response, deserialize
/// stdout/stderr/exit-code, and return them to the caller.
///
/// Currently returns the request ID so callers can correlate the future
/// response.
pub fn remote_exec(
    dst_ip: [u8; 4],
    port: u16,
    command: &str,
) -> Result<u64, &'static str> {
    let args = encode_string(command);
    send_rpc(dst_ip, port, "exec", &args)
}

/// Handler stub for the server side of `remote_exec`.
///
/// Decodes the command string from the argument payload and returns a
/// placeholder acknowledgement.  A full implementation would spawn the
/// command in a subprocess, capture its output, and return the results.
pub fn handle_exec(args: &[u8]) -> Result<Vec<u8>, &'static str> {
    let (cmd, _consumed) = decode_string(args).ok_or("exec: bad command encoding")?;
    // TODO: actually execute `cmd` via the kernel shell / task system.
    let _ = cmd;
    let mut resp = Vec::new();
    resp.extend_from_slice(b"exec: accepted (stub)");
    Ok(resp)
}
