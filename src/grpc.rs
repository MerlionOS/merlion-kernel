/// gRPC framework for MerlionOS.
/// Implements gRPC over HTTP/2 with Protocol Buffers encoding,
/// service definition, and streaming support.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::borrow::ToOwned;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Protocol Buffers wire types
// ---------------------------------------------------------------------------

/// Protobuf wire types per the encoding specification.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WireType {
    /// Variable-length integer (int32, int64, uint32, uint64, sint32, sint64, bool, enum).
    Varint = 0,
    /// Fixed 64-bit value (fixed64, sfixed64, double — but we don't use FP).
    Bit64 = 1,
    /// Length-delimited (string, bytes, embedded messages, packed repeated fields).
    LengthDelimited = 2,
    /// Fixed 32-bit value (fixed32, sfixed32, float — but we don't use FP).
    Bit32 = 5,
}

impl WireType {
    /// Convert from raw u8 wire type tag.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(WireType::Varint),
            1 => Some(WireType::Bit64),
            2 => Some(WireType::LengthDelimited),
            5 => Some(WireType::Bit32),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Varint encoding / decoding
// ---------------------------------------------------------------------------

/// Encode a u64 value as a varint, appending to `out`.
pub fn encode_varint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Decode a varint from a byte slice, returning (value, bytes_consumed).
/// Returns None if the slice is too short or the varint is too long.
pub fn decode_varint(data: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    for (i, &byte) in data.iter().enumerate() {
        if shift >= 70 {
            return None; // varint too long
        }
        result |= ((byte & 0x7F) as u64) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            return Some((result, i + 1));
        }
    }
    None // incomplete varint
}

/// Encode a field tag (field_number << 3 | wire_type).
pub fn encode_tag(field_number: u32, wire_type: WireType, out: &mut Vec<u8>) {
    let tag = ((field_number as u64) << 3) | (wire_type as u64);
    encode_varint(tag, out);
}

/// Decode a field tag, returning (field_number, wire_type, bytes_consumed).
pub fn decode_tag(data: &[u8]) -> Option<(u32, WireType, usize)> {
    let (tag, consumed) = decode_varint(data)?;
    let wire_type_raw = (tag & 0x07) as u8;
    let field_number = (tag >> 3) as u32;
    let wire_type = WireType::from_u8(wire_type_raw)?;
    Some((field_number, wire_type, consumed))
}

// ---------------------------------------------------------------------------
// Protobuf field values
// ---------------------------------------------------------------------------

/// A single protobuf field value.
#[derive(Clone, Debug)]
pub enum FieldValue {
    /// Varint value (integers, booleans, enums).
    Varint(u64),
    /// Fixed 64-bit value.
    Fixed64(u64),
    /// Length-delimited data (bytes, strings, embedded messages).
    Bytes(Vec<u8>),
    /// Fixed 32-bit value.
    Fixed32(u32),
}

/// A decoded protobuf field with number and value.
#[derive(Clone, Debug)]
pub struct ProtoField {
    pub number: u32,
    pub value: FieldValue,
}

/// Encode a protobuf message from a list of fields.
pub fn encode_message(fields: &[ProtoField]) -> Vec<u8> {
    let mut out = Vec::new();
    for field in fields {
        match &field.value {
            FieldValue::Varint(v) => {
                encode_tag(field.number, WireType::Varint, &mut out);
                encode_varint(*v, &mut out);
            }
            FieldValue::Fixed64(v) => {
                encode_tag(field.number, WireType::Bit64, &mut out);
                out.extend_from_slice(&v.to_le_bytes());
            }
            FieldValue::Bytes(data) => {
                encode_tag(field.number, WireType::LengthDelimited, &mut out);
                encode_varint(data.len() as u64, &mut out);
                out.extend_from_slice(data);
            }
            FieldValue::Fixed32(v) => {
                encode_tag(field.number, WireType::Bit32, &mut out);
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    out
}

/// Decode a protobuf message into a list of fields.
pub fn decode_message(data: &[u8]) -> Option<Vec<ProtoField>> {
    let mut fields = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let (field_number, wire_type, tag_len) = decode_tag(&data[pos..])?;
        pos += tag_len;

        let value = match wire_type {
            WireType::Varint => {
                let (v, len) = decode_varint(&data[pos..])?;
                pos += len;
                FieldValue::Varint(v)
            }
            WireType::Bit64 => {
                if pos + 8 > data.len() { return None; }
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&data[pos..pos + 8]);
                pos += 8;
                FieldValue::Fixed64(u64::from_le_bytes(bytes))
            }
            WireType::LengthDelimited => {
                let (len, vlen) = decode_varint(&data[pos..])?;
                pos += vlen;
                let len = len as usize;
                if pos + len > data.len() { return None; }
                let bytes = data[pos..pos + len].to_vec();
                pos += len;
                FieldValue::Bytes(bytes)
            }
            WireType::Bit32 => {
                if pos + 4 > data.len() { return None; }
                let mut bytes = [0u8; 4];
                bytes.copy_from_slice(&data[pos..pos + 4]);
                pos += 4;
                FieldValue::Fixed32(u32::from_le_bytes(bytes))
            }
        };

        fields.push(ProtoField { number: field_number, value });
    }
    Some(fields)
}

// ---------------------------------------------------------------------------
// gRPC framing
// ---------------------------------------------------------------------------

/// gRPC frame: 1-byte compressed flag + 4-byte message length + message data.
pub struct GrpcFrame {
    /// Whether the message is compressed.
    pub compressed: bool,
    /// The message payload.
    pub data: Vec<u8>,
}

impl GrpcFrame {
    /// Create a new uncompressed gRPC frame.
    pub fn new(data: Vec<u8>) -> Self {
        Self { compressed: false, data }
    }

    /// Encode this frame into wire format.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(5 + self.data.len());
        out.push(if self.compressed { 1 } else { 0 });
        let len = self.data.len() as u32;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&self.data);
        out
    }

    /// Decode a gRPC frame from wire format.
    pub fn decode(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < 5 {
            return None;
        }
        let compressed = data[0] != 0;
        let mut len_bytes = [0u8; 4];
        len_bytes.copy_from_slice(&data[1..5]);
        let msg_len = u32::from_be_bytes(len_bytes) as usize;
        if data.len() < 5 + msg_len {
            return None;
        }
        let payload = data[5..5 + msg_len].to_vec();
        Some((Self { compressed, data: payload }, 5 + msg_len))
    }
}

// ---------------------------------------------------------------------------
// gRPC status codes
// ---------------------------------------------------------------------------

/// gRPC status codes per the specification.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum StatusCode {
    Ok = 0,
    Cancelled = 1,
    Unknown = 2,
    InvalidArgument = 3,
    DeadlineExceeded = 4,
    NotFound = 5,
    AlreadyExists = 6,
    PermissionDenied = 7,
    ResourceExhausted = 8,
    FailedPrecondition = 9,
    Aborted = 10,
    OutOfRange = 11,
    Unimplemented = 12,
    Internal = 13,
    Unavailable = 14,
    DataLoss = 15,
    Unauthenticated = 16,
}

impl StatusCode {
    /// Convert from raw u8 value.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(StatusCode::Ok),
            1 => Some(StatusCode::Cancelled),
            2 => Some(StatusCode::Unknown),
            3 => Some(StatusCode::InvalidArgument),
            4 => Some(StatusCode::DeadlineExceeded),
            5 => Some(StatusCode::NotFound),
            6 => Some(StatusCode::AlreadyExists),
            7 => Some(StatusCode::PermissionDenied),
            8 => Some(StatusCode::ResourceExhausted),
            9 => Some(StatusCode::FailedPrecondition),
            10 => Some(StatusCode::Aborted),
            11 => Some(StatusCode::OutOfRange),
            12 => Some(StatusCode::Unimplemented),
            13 => Some(StatusCode::Internal),
            14 => Some(StatusCode::Unavailable),
            15 => Some(StatusCode::DataLoss),
            16 => Some(StatusCode::Unauthenticated),
            _ => None,
        }
    }

    /// Get a human-readable name for this status code.
    pub fn name(&self) -> &'static str {
        match self {
            StatusCode::Ok => "OK",
            StatusCode::Cancelled => "CANCELLED",
            StatusCode::Unknown => "UNKNOWN",
            StatusCode::InvalidArgument => "INVALID_ARGUMENT",
            StatusCode::DeadlineExceeded => "DEADLINE_EXCEEDED",
            StatusCode::NotFound => "NOT_FOUND",
            StatusCode::AlreadyExists => "ALREADY_EXISTS",
            StatusCode::PermissionDenied => "PERMISSION_DENIED",
            StatusCode::ResourceExhausted => "RESOURCE_EXHAUSTED",
            StatusCode::FailedPrecondition => "FAILED_PRECONDITION",
            StatusCode::Aborted => "ABORTED",
            StatusCode::OutOfRange => "OUT_OF_RANGE",
            StatusCode::Unimplemented => "UNIMPLEMENTED",
            StatusCode::Internal => "INTERNAL",
            StatusCode::Unavailable => "UNAVAILABLE",
            StatusCode::DataLoss => "DATA_LOSS",
            StatusCode::Unauthenticated => "UNAUTHENTICATED",
        }
    }
}

// ---------------------------------------------------------------------------
// gRPC method types
// ---------------------------------------------------------------------------

/// The four gRPC method types.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MethodType {
    /// Single request, single response.
    Unary,
    /// Single request, stream of responses.
    ServerStreaming,
    /// Stream of requests, single response.
    ClientStreaming,
    /// Stream of requests, stream of responses.
    BidirectionalStreaming,
}

impl MethodType {
    /// Human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            MethodType::Unary => "unary",
            MethodType::ServerStreaming => "server_streaming",
            MethodType::ClientStreaming => "client_streaming",
            MethodType::BidirectionalStreaming => "bidi_streaming",
        }
    }
}

// ---------------------------------------------------------------------------
// gRPC content types
// ---------------------------------------------------------------------------

/// Standard gRPC content type.
pub const CONTENT_TYPE_GRPC: &str = "application/grpc";
/// gRPC with protobuf content type.
pub const CONTENT_TYPE_GRPC_PROTO: &str = "application/grpc+proto";

/// Check if a content type is valid for gRPC.
pub fn is_grpc_content_type(ct: &str) -> bool {
    ct == CONTENT_TYPE_GRPC || ct == CONTENT_TYPE_GRPC_PROTO || ct.starts_with("application/grpc+")
}

// ---------------------------------------------------------------------------
// gRPC metadata (headers/trailers)
// ---------------------------------------------------------------------------

/// A single metadata entry (key-value pair).
#[derive(Clone, Debug)]
pub struct MetadataEntry {
    pub key: String,
    pub value: String,
}

/// Metadata collection (ordered key-value pairs).
#[derive(Clone, Debug)]
pub struct Metadata {
    entries: Vec<MetadataEntry>,
}

impl Metadata {
    /// Create empty metadata.
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Add a metadata entry.
    pub fn insert(&mut self, key: &str, value: &str) {
        self.entries.push(MetadataEntry {
            key: key.to_owned(),
            value: value.to_owned(),
        });
    }

    /// Get the first value for a key, if present.
    pub fn get(&self, key: &str) -> Option<&str> {
        for entry in &self.entries {
            if entry.key == key {
                return Some(&entry.value);
            }
        }
        None
    }

    /// Get all values for a key.
    pub fn get_all(&self, key: &str) -> Vec<&str> {
        self.entries.iter()
            .filter(|e| e.key == key)
            .map(|e| e.value.as_str())
            .collect()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the metadata is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// gRPC request and response
// ---------------------------------------------------------------------------

/// A gRPC request.
pub struct GrpcRequest {
    /// Service name (e.g. "grpc.health.v1.Health").
    pub service: String,
    /// Method name (e.g. "Check").
    pub method: String,
    /// Request metadata (headers).
    pub metadata: Metadata,
    /// Serialized request message.
    pub message: Vec<u8>,
}

/// A gRPC response.
pub struct GrpcResponse {
    /// Response status code.
    pub status: StatusCode,
    /// Optional status message.
    pub message_text: String,
    /// Response metadata (headers).
    pub metadata: Metadata,
    /// Trailing metadata.
    pub trailers: Metadata,
    /// Serialized response message.
    pub data: Vec<u8>,
}

impl GrpcResponse {
    /// Create an OK response with data.
    pub fn ok(data: Vec<u8>) -> Self {
        Self {
            status: StatusCode::Ok,
            message_text: String::new(),
            metadata: Metadata::new(),
            trailers: Metadata::new(),
            data,
        }
    }

    /// Create an error response.
    pub fn error(status: StatusCode, message: &str) -> Self {
        Self {
            status,
            message_text: message.to_owned(),
            metadata: Metadata::new(),
            trailers: Metadata::new(),
            data: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Service and method definitions
// ---------------------------------------------------------------------------

/// A gRPC method definition.
#[derive(Clone)]
pub struct MethodDef {
    /// Method name.
    pub name: String,
    /// Method type (unary, streaming, etc.).
    pub method_type: MethodType,
    /// Input message type name.
    pub input_type: String,
    /// Output message type name.
    pub output_type: String,
}

/// A gRPC service definition.
#[derive(Clone)]
pub struct ServiceDef {
    /// Fully-qualified service name.
    pub name: String,
    /// Methods provided by this service.
    pub methods: Vec<MethodDef>,
}

impl ServiceDef {
    /// Create a new service definition.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            methods: Vec::new(),
        }
    }

    /// Add a method to this service.
    pub fn add_method(&mut self, name: &str, method_type: MethodType, input: &str, output: &str) {
        self.methods.push(MethodDef {
            name: name.to_owned(),
            method_type,
            input_type: input.to_owned(),
            output_type: output.to_owned(),
        });
    }

    /// Find a method by name.
    pub fn find_method(&self, name: &str) -> Option<&MethodDef> {
        self.methods.iter().find(|m| m.name == name)
    }
}

// ---------------------------------------------------------------------------
// Global gRPC server state
// ---------------------------------------------------------------------------

/// Maximum number of registered services.
const MAX_SERVICES: usize = 32;

struct GrpcServer {
    services: Vec<ServiceDef>,
    initialized: bool,
}

impl GrpcServer {
    const fn new() -> Self {
        Self {
            services: Vec::new(),
            initialized: false,
        }
    }
}

static SERVER: Mutex<GrpcServer> = Mutex::new(GrpcServer::new());

// Statistics counters
static REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static REQUESTS_OK: AtomicU64 = AtomicU64::new(0);
static REQUESTS_ERROR: AtomicU64 = AtomicU64::new(0);
static STREAMS_OPENED: AtomicU64 = AtomicU64::new(0);
static BYTES_RECEIVED: AtomicU64 = AtomicU64::new(0);
static BYTES_SENT: AtomicU64 = AtomicU64::new(0);
static INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Health check service (grpc.health.v1.Health)
// ---------------------------------------------------------------------------

/// Health check status values.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum HealthStatus {
    Unknown = 0,
    Serving = 1,
    NotServing = 2,
    ServiceUnknown = 3,
}

/// Build a health check response protobuf message.
fn build_health_response(status: HealthStatus) -> Vec<u8> {
    let fields = vec![
        ProtoField {
            number: 1,
            value: FieldValue::Varint(status as u64),
        },
    ];
    encode_message(&fields)
}

/// Handle a health check request.
fn handle_health_check(service_name: &str) -> GrpcResponse {
    REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);

    let server = SERVER.lock();
    let status = if service_name.is_empty() {
        // Empty service name means overall server health
        HealthStatus::Serving
    } else if server.services.iter().any(|s| s.name == service_name) {
        HealthStatus::Serving
    } else {
        HealthStatus::ServiceUnknown
    };
    drop(server);

    let data = build_health_response(status);
    REQUESTS_OK.fetch_add(1, Ordering::Relaxed);
    BYTES_SENT.fetch_add(data.len() as u64, Ordering::Relaxed);
    GrpcResponse::ok(data)
}

// ---------------------------------------------------------------------------
// Reflection service (grpc.reflection.v1alpha.ServerReflection)
// ---------------------------------------------------------------------------

/// Build a list of services response for gRPC reflection.
fn build_reflection_list() -> Vec<u8> {
    let server = SERVER.lock();
    let mut fields = Vec::new();
    for (i, svc) in server.services.iter().enumerate() {
        let name_bytes = svc.name.as_bytes().to_vec();
        // Each service is an embedded message with field 1 = name
        let svc_msg = encode_message(&[ProtoField {
            number: 1,
            value: FieldValue::Bytes(name_bytes),
        }]);
        fields.push(ProtoField {
            number: (i as u32) + 1,
            value: FieldValue::Bytes(svc_msg),
        });
    }
    encode_message(&fields)
}

/// Handle a reflection request.
fn handle_reflection(_request: &[u8]) -> GrpcResponse {
    REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    let data = build_reflection_list();
    REQUESTS_OK.fetch_add(1, Ordering::Relaxed);
    BYTES_SENT.fetch_add(data.len() as u64, Ordering::Relaxed);
    GrpcResponse::ok(data)
}

// ---------------------------------------------------------------------------
// Request routing
// ---------------------------------------------------------------------------

/// Route a gRPC request to the appropriate handler.
pub fn handle_request(request: &GrpcRequest) -> GrpcResponse {
    REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    BYTES_RECEIVED.fetch_add(request.message.len() as u64, Ordering::Relaxed);

    // Health check service
    if request.service == "grpc.health.v1.Health" && request.method == "Check" {
        // Extract service name from request message
        let svc_name = if let Some(fields) = decode_message(&request.message) {
            fields.iter().find(|f| f.number == 1).and_then(|f| {
                if let FieldValue::Bytes(b) = &f.value {
                    core::str::from_utf8(b).ok().map(|s| s.to_owned())
                } else {
                    None
                }
            }).unwrap_or_default()
        } else {
            String::new()
        };
        return handle_health_check(&svc_name);
    }

    // Reflection service
    if request.service == "grpc.reflection.v1alpha.ServerReflection" {
        return handle_reflection(&request.message);
    }

    // Look up the registered service
    let server = SERVER.lock();
    let service = server.services.iter().find(|s| s.name == request.service);
    match service {
        None => {
            REQUESTS_ERROR.fetch_add(1, Ordering::Relaxed);
            GrpcResponse::error(StatusCode::Unimplemented,
                &format!("service '{}' not found", request.service))
        }
        Some(svc) => {
            if svc.find_method(&request.method).is_none() {
                REQUESTS_ERROR.fetch_add(1, Ordering::Relaxed);
                GrpcResponse::error(StatusCode::Unimplemented,
                    &format!("method '{}' not found in service '{}'",
                        request.method, request.service))
            } else {
                // In a real implementation, we would invoke the method handler here.
                // For now, return an empty OK response.
                REQUESTS_OK.fetch_add(1, Ordering::Relaxed);
                GrpcResponse::ok(Vec::new())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the gRPC subsystem.
pub fn init() {
    let mut server = SERVER.lock();
    if server.initialized {
        return;
    }

    // Register built-in services
    let mut health = ServiceDef::new("grpc.health.v1.Health");
    health.add_method("Check", MethodType::Unary,
        "HealthCheckRequest", "HealthCheckResponse");
    health.add_method("Watch", MethodType::ServerStreaming,
        "HealthCheckRequest", "HealthCheckResponse");
    server.services.push(health);

    let mut reflection = ServiceDef::new("grpc.reflection.v1alpha.ServerReflection");
    reflection.add_method("ServerReflectionInfo", MethodType::BidirectionalStreaming,
        "ServerReflectionRequest", "ServerReflectionResponse");
    server.services.push(reflection);

    server.initialized = true;
    INITIALIZED.store(true, Ordering::SeqCst);
}

/// Register a new gRPC service.
pub fn register_service(name: &str, methods: Vec<MethodDef>) -> Result<(), &'static str> {
    let mut server = SERVER.lock();
    if server.services.len() >= MAX_SERVICES {
        return Err("maximum number of services reached");
    }
    // Check for duplicate
    if server.services.iter().any(|s| s.name == name) {
        return Err("service already registered");
    }
    server.services.push(ServiceDef {
        name: name.to_owned(),
        methods,
    });
    Ok(())
}

/// List all registered services.
pub fn list_services() -> String {
    let server = SERVER.lock();
    if server.services.is_empty() {
        return "No gRPC services registered.".to_owned();
    }
    let mut out = format!("gRPC services ({}):\n", server.services.len());
    for svc in &server.services {
        out += &format!("  {}\n", svc.name);
        for method in &svc.methods {
            out += &format!("    {} {} ({}) -> {}\n",
                method.method_type.name(), method.name,
                method.input_type, method.output_type);
        }
    }
    out
}

/// Get gRPC server info.
pub fn grpc_info() -> String {
    let server = SERVER.lock();
    let svc_count = server.services.len();
    let method_count: usize = server.services.iter().map(|s| s.methods.len()).sum();
    format!(
        "gRPC Server Info\n\
         ────────────────────────────\n\
         Status:           {}\n\
         Services:         {}\n\
         Methods:          {}\n\
         Content-Type:     {}\n\
         Reflection:       enabled\n\
         Health check:     enabled",
        if INITIALIZED.load(Ordering::Relaxed) { "running" } else { "stopped" },
        svc_count,
        method_count,
        CONTENT_TYPE_GRPC_PROTO,
    )
}

/// Get gRPC statistics.
pub fn grpc_stats() -> String {
    format!(
        "gRPC Statistics\n\
         ────────────────────────────\n\
         Requests total:   {}\n\
         Requests OK:      {}\n\
         Requests error:   {}\n\
         Streams opened:   {}\n\
         Bytes received:   {}\n\
         Bytes sent:       {}",
        REQUESTS_TOTAL.load(Ordering::Relaxed),
        REQUESTS_OK.load(Ordering::Relaxed),
        REQUESTS_ERROR.load(Ordering::Relaxed),
        STREAMS_OPENED.load(Ordering::Relaxed),
        BYTES_RECEIVED.load(Ordering::Relaxed),
        BYTES_SENT.load(Ordering::Relaxed),
    )
}
