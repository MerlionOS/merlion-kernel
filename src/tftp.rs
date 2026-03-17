/// TFTP server and client for MerlionOS (RFC 1350).
/// Simple file transfer over UDP for PXE network boot.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::borrow::ToOwned;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// TFTP opcodes (RFC 1350)
// ---------------------------------------------------------------------------

/// TFTP opcode constants.
pub const OPCODE_RRQ: u16 = 1;
pub const OPCODE_WRQ: u16 = 2;
pub const OPCODE_DATA: u16 = 3;
pub const OPCODE_ACK: u16 = 4;
pub const OPCODE_ERROR: u16 = 5;

/// TFTP opcode enum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Opcode {
    /// Read request.
    Rrq,
    /// Write request.
    Wrq,
    /// Data packet.
    Data,
    /// Acknowledgement.
    Ack,
    /// Error packet.
    Error,
}

impl Opcode {
    /// Convert from raw u16.
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            1 => Some(Opcode::Rrq),
            2 => Some(Opcode::Wrq),
            3 => Some(Opcode::Data),
            4 => Some(Opcode::Ack),
            5 => Some(Opcode::Error),
            _ => None,
        }
    }

    /// Convert to raw u16.
    pub fn to_u16(self) -> u16 {
        match self {
            Opcode::Rrq => 1,
            Opcode::Wrq => 2,
            Opcode::Data => 3,
            Opcode::Ack => 4,
            Opcode::Error => 5,
        }
    }
}

// ---------------------------------------------------------------------------
// TFTP error codes
// ---------------------------------------------------------------------------

/// TFTP error code constants.
pub const ERR_NOT_DEFINED: u16 = 0;
pub const ERR_FILE_NOT_FOUND: u16 = 1;
pub const ERR_ACCESS_VIOLATION: u16 = 2;
pub const ERR_DISK_FULL: u16 = 3;
pub const ERR_ILLEGAL_OP: u16 = 4;
pub const ERR_UNKNOWN_TID: u16 = 5;
pub const ERR_FILE_EXISTS: u16 = 6;
pub const ERR_NO_SUCH_USER: u16 = 7;

/// TFTP error code enum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TftpError {
    NotDefined,
    FileNotFound,
    AccessViolation,
    DiskFull,
    IllegalOperation,
    UnknownTid,
    FileExists,
    NoSuchUser,
}

impl TftpError {
    /// Convert from raw u16.
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0 => Some(TftpError::NotDefined),
            1 => Some(TftpError::FileNotFound),
            2 => Some(TftpError::AccessViolation),
            3 => Some(TftpError::DiskFull),
            4 => Some(TftpError::IllegalOperation),
            5 => Some(TftpError::UnknownTid),
            6 => Some(TftpError::FileExists),
            7 => Some(TftpError::NoSuchUser),
            _ => None,
        }
    }

    /// Convert to raw u16.
    pub fn to_u16(self) -> u16 {
        match self {
            TftpError::NotDefined => 0,
            TftpError::FileNotFound => 1,
            TftpError::AccessViolation => 2,
            TftpError::DiskFull => 3,
            TftpError::IllegalOperation => 4,
            TftpError::UnknownTid => 5,
            TftpError::FileExists => 6,
            TftpError::NoSuchUser => 7,
        }
    }

    /// Human-readable error message.
    pub fn message(&self) -> &'static str {
        match self {
            TftpError::NotDefined => "Not defined",
            TftpError::FileNotFound => "File not found",
            TftpError::AccessViolation => "Access violation",
            TftpError::DiskFull => "Disk full or allocation exceeded",
            TftpError::IllegalOperation => "Illegal TFTP operation",
            TftpError::UnknownTid => "Unknown transfer ID",
            TftpError::FileExists => "File already exists",
            TftpError::NoSuchUser => "No such user",
        }
    }
}

// ---------------------------------------------------------------------------
// Transfer modes
// ---------------------------------------------------------------------------

/// TFTP transfer mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransferMode {
    /// NetASCII mode (CR/LF translation).
    NetAscii,
    /// Octet mode (raw binary).
    Octet,
}

impl TransferMode {
    /// Parse from mode string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "netascii" => Some(TransferMode::NetAscii),
            "octet" => Some(TransferMode::Octet),
            _ => None,
        }
    }

    /// Get the mode string.
    pub fn as_str(&self) -> &'static str {
        match self {
            TransferMode::NetAscii => "netascii",
            TransferMode::Octet => "octet",
        }
    }
}

// We need this helper since we don't have std
fn to_ascii_lowercase(s: &str) -> String {
    s.chars().map(|c| {
        if c.is_ascii_uppercase() {
            (c as u8 + 32) as char
        } else {
            c
        }
    }).collect()
}

// ---------------------------------------------------------------------------
// TFTP packet encoding / decoding
// ---------------------------------------------------------------------------

/// Block size for TFTP transfers.
pub const BLOCK_SIZE: usize = 512;

/// Default TFTP server port.
pub const DEFAULT_PORT: u16 = 69;

/// Encode a RRQ or WRQ packet.
pub fn encode_request(opcode: Opcode, filename: &str, mode: TransferMode) -> Vec<u8> {
    let mut pkt = Vec::new();
    let op = opcode.to_u16();
    pkt.push((op >> 8) as u8);
    pkt.push((op & 0xFF) as u8);
    pkt.extend_from_slice(filename.as_bytes());
    pkt.push(0); // null terminator
    pkt.extend_from_slice(mode.as_str().as_bytes());
    pkt.push(0); // null terminator
    pkt
}

/// Encode a DATA packet.
pub fn encode_data(block: u16, data: &[u8]) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(4 + data.len());
    pkt.push((OPCODE_DATA >> 8) as u8);
    pkt.push((OPCODE_DATA & 0xFF) as u8);
    pkt.push((block >> 8) as u8);
    pkt.push((block & 0xFF) as u8);
    pkt.extend_from_slice(data);
    pkt
}

/// Encode an ACK packet.
pub fn encode_ack(block: u16) -> Vec<u8> {
    let mut pkt = vec![0u8; 4];
    pkt[0] = (OPCODE_ACK >> 8) as u8;
    pkt[1] = (OPCODE_ACK & 0xFF) as u8;
    pkt[2] = (block >> 8) as u8;
    pkt[3] = (block & 0xFF) as u8;
    pkt
}

/// Encode an ERROR packet.
pub fn encode_error(error: TftpError, message: &str) -> Vec<u8> {
    let code = error.to_u16();
    let mut pkt = Vec::new();
    pkt.push((OPCODE_ERROR >> 8) as u8);
    pkt.push((OPCODE_ERROR & 0xFF) as u8);
    pkt.push((code >> 8) as u8);
    pkt.push((code & 0xFF) as u8);
    pkt.extend_from_slice(message.as_bytes());
    pkt.push(0); // null terminator
    pkt
}

/// Decode an opcode from a packet.
pub fn decode_opcode(data: &[u8]) -> Option<Opcode> {
    if data.len() < 2 {
        return None;
    }
    let op = ((data[0] as u16) << 8) | (data[1] as u16);
    Opcode::from_u16(op)
}

/// Decode a DATA packet, returning (block_number, payload).
pub fn decode_data(data: &[u8]) -> Option<(u16, &[u8])> {
    if data.len() < 4 {
        return None;
    }
    let block = ((data[2] as u16) << 8) | (data[3] as u16);
    Some((block, &data[4..]))
}

/// Decode an ACK packet, returning the block number.
pub fn decode_ack(data: &[u8]) -> Option<u16> {
    if data.len() < 4 {
        return None;
    }
    Some(((data[2] as u16) << 8) | (data[3] as u16))
}

/// Decode an ERROR packet, returning (error_code, message).
pub fn decode_error(data: &[u8]) -> Option<(TftpError, String)> {
    if data.len() < 5 {
        return None;
    }
    let code = ((data[2] as u16) << 8) | (data[3] as u16);
    let error = TftpError::from_u16(code)?;
    // Find null terminator for message
    let msg_bytes = &data[4..];
    let msg_len = msg_bytes.iter().position(|&b| b == 0).unwrap_or(msg_bytes.len());
    let message = core::str::from_utf8(&msg_bytes[..msg_len])
        .unwrap_or("")
        .to_owned();
    Some((error, message))
}

// ---------------------------------------------------------------------------
// TFTP transfer state
// ---------------------------------------------------------------------------

/// State of a TFTP transfer.
#[derive(Clone, Debug)]
pub struct Transfer {
    /// Transfer ID.
    pub id: u64,
    /// Remote IP address.
    pub remote_ip: [u8; 4],
    /// Remote port (TID).
    pub remote_port: u16,
    /// Filename being transferred.
    pub filename: String,
    /// Transfer mode.
    pub mode: TransferMode,
    /// Whether this is a read (true) or write (false) transfer.
    pub is_read: bool,
    /// Current block number.
    pub block: u16,
    /// Accumulated data (for writes).
    pub data: Vec<u8>,
    /// Whether the transfer is complete.
    pub complete: bool,
    /// Total bytes transferred.
    pub bytes_transferred: u64,
}

// ---------------------------------------------------------------------------
// TFTP server state
// ---------------------------------------------------------------------------

const MAX_TRANSFERS: usize = 16;

struct TftpServer {
    /// Whether the server is listening.
    running: bool,
    /// Active transfers.
    transfers: Vec<Transfer>,
    /// Next transfer ID.
    next_id: u64,
    /// Server root directory in VFS.
    root_dir: String,
    /// Whether the module is initialized.
    initialized: bool,
}

impl TftpServer {
    const fn new() -> Self {
        Self {
            running: false,
            transfers: Vec::new(),
            next_id: 1,
            root_dir: String::new(),
            initialized: false,
        }
    }
}

static SERVER: Mutex<TftpServer> = Mutex::new(TftpServer::new());

// Statistics
static FILES_SERVED: AtomicU64 = AtomicU64::new(0);
static FILES_RECEIVED: AtomicU64 = AtomicU64::new(0);
static BYTES_SENT: AtomicU64 = AtomicU64::new(0);
static BYTES_RECEIVED: AtomicU64 = AtomicU64::new(0);
static ERRORS_SENT: AtomicU64 = AtomicU64::new(0);
static TRANSFERS_ACTIVE: AtomicU64 = AtomicU64::new(0);
static TFTP_INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Server operations
// ---------------------------------------------------------------------------

/// Handle a RRQ (read request) from a client.
fn handle_rrq(filename: &str, mode: TransferMode, remote_ip: [u8; 4], remote_port: u16) -> Result<u64, TftpError> {
    let mut server = SERVER.lock();
    if server.transfers.len() >= MAX_TRANSFERS {
        return Err(TftpError::NotDefined);
    }

    // Try to read the file from VFS
    let vfs_path = if filename.starts_with('/') {
        filename.to_owned()
    } else {
        format!("{}/{}", server.root_dir, filename)
    };

    let file_data = crate::vfs::cat(&vfs_path).map_err(|_| TftpError::FileNotFound)?;

    let id = server.next_id;
    server.next_id += 1;

    let transfer = Transfer {
        id,
        remote_ip,
        remote_port,
        filename: filename.to_owned(),
        mode,
        is_read: true,
        block: 1,
        data: file_data.into_bytes(),
        complete: false,
        bytes_transferred: 0,
    };

    server.transfers.push(transfer);
    TRANSFERS_ACTIVE.fetch_add(1, Ordering::Relaxed);
    FILES_SERVED.fetch_add(1, Ordering::Relaxed);

    Ok(id)
}

/// Handle a WRQ (write request) from a client.
fn handle_wrq(filename: &str, mode: TransferMode, remote_ip: [u8; 4], remote_port: u16) -> Result<u64, TftpError> {
    let mut server = SERVER.lock();
    if server.transfers.len() >= MAX_TRANSFERS {
        return Err(TftpError::NotDefined);
    }

    let id = server.next_id;
    server.next_id += 1;

    let transfer = Transfer {
        id,
        remote_ip,
        remote_port,
        filename: filename.to_owned(),
        mode,
        is_read: false,
        block: 0,
        data: Vec::new(),
        complete: false,
        bytes_transferred: 0,
    };

    server.transfers.push(transfer);
    TRANSFERS_ACTIVE.fetch_add(1, Ordering::Relaxed);

    Ok(id)
}

/// Get the next DATA block for a read transfer.
pub fn get_data_block(transfer_id: u64) -> Option<Vec<u8>> {
    let mut server = SERVER.lock();
    let transfer = server.transfers.iter_mut().find(|t| t.id == transfer_id)?;
    if !transfer.is_read || transfer.complete {
        return None;
    }

    let start = ((transfer.block as usize) - 1) * BLOCK_SIZE;
    if start >= transfer.data.len() {
        transfer.complete = true;
        TRANSFERS_ACTIVE.fetch_sub(1, Ordering::Relaxed);
        return Some(encode_data(transfer.block, &[]));
    }

    let end = core::cmp::min(start + BLOCK_SIZE, transfer.data.len());
    let block_data = &transfer.data[start..end];
    let pkt = encode_data(transfer.block, block_data);

    let sent = block_data.len() as u64;
    transfer.bytes_transferred += sent;
    BYTES_SENT.fetch_add(sent, Ordering::Relaxed);

    if block_data.len() < BLOCK_SIZE {
        transfer.complete = true;
        TRANSFERS_ACTIVE.fetch_sub(1, Ordering::Relaxed);
    } else {
        transfer.block += 1;
    }

    Some(pkt)
}

/// Receive a DATA block for a write transfer.
pub fn receive_data_block(transfer_id: u64, block: u16, data: &[u8]) -> Option<Vec<u8>> {
    let mut server = SERVER.lock();
    let root_dir = server.root_dir.clone();

    let transfer = server.transfers.iter_mut().find(|t| t.id == transfer_id)?;
    if transfer.is_read || transfer.complete {
        return None;
    }

    // Verify block number
    if block != transfer.block + 1 {
        return None;
    }

    transfer.block = block;
    transfer.data.extend_from_slice(data);
    transfer.bytes_transferred += data.len() as u64;
    BYTES_RECEIVED.fetch_add(data.len() as u64, Ordering::Relaxed);

    // If block is less than 512 bytes, transfer is complete
    if data.len() < BLOCK_SIZE {
        transfer.complete = true;
        TRANSFERS_ACTIVE.fetch_sub(1, Ordering::Relaxed);
        FILES_RECEIVED.fetch_add(1, Ordering::Relaxed);

        // Write the received file to VFS
        let vfs_path = if transfer.filename.starts_with('/') {
            transfer.filename.clone()
        } else {
            format!("{}/{}", root_dir, transfer.filename)
        };
        let content = core::str::from_utf8(&transfer.data).unwrap_or("");
        let _ = crate::vfs::write(&vfs_path, content);
    }

    Some(encode_ack(block))
}

// ---------------------------------------------------------------------------
// Client operations
// ---------------------------------------------------------------------------

/// Request a file from a TFTP server (client mode, simulated).
pub fn tftp_get(server_ip: &str, filename: &str) -> Result<String, &'static str> {
    // In a real implementation, this would send UDP packets.
    // For now, simulate the protocol exchange.
    let _rrq = encode_request(Opcode::Rrq, filename, TransferMode::Octet);
    Ok(format!("TFTP GET {} from {} (simulated)", filename, server_ip))
}

/// Send a file to a TFTP server (client mode, simulated).
pub fn tftp_put(server_ip: &str, filename: &str, data: &[u8]) -> Result<String, &'static str> {
    let _wrq = encode_request(Opcode::Wrq, filename, TransferMode::Octet);
    let blocks = (data.len() + BLOCK_SIZE - 1) / BLOCK_SIZE;
    Ok(format!("TFTP PUT {} to {} ({} bytes, {} blocks, simulated)",
        filename, server_ip, data.len(), blocks))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the TFTP subsystem.
pub fn init() {
    let mut server = SERVER.lock();
    if server.initialized {
        return;
    }
    server.root_dir = "/srv/tftp".to_owned();
    server.initialized = true;
    TFTP_INITIALIZED.store(true, Ordering::SeqCst);
}

/// Start the TFTP server.
pub fn start_server() -> Result<(), &'static str> {
    let mut server = SERVER.lock();
    if server.running {
        return Err("TFTP server already running");
    }
    server.running = true;
    // Create the TFTP root directory in VFS
    let _ = crate::vfs::mkdir(&server.root_dir);
    Ok(())
}

/// Stop the TFTP server.
pub fn stop_server() -> Result<(), &'static str> {
    let mut server = SERVER.lock();
    if !server.running {
        return Err("TFTP server not running");
    }
    server.running = false;
    // Cancel active transfers
    let active = server.transfers.iter().filter(|t| !t.complete).count() as u64;
    if active > 0 {
        TRANSFERS_ACTIVE.fetch_sub(active, Ordering::Relaxed);
    }
    server.transfers.clear();
    Ok(())
}

/// Get TFTP server/client info and status.
pub fn tftp_info() -> String {
    let server = SERVER.lock();
    let active = server.transfers.iter().filter(|t| !t.complete).count();
    let completed = server.transfers.iter().filter(|t| t.complete).count();
    let mut out = format!(
        "TFTP Server/Client Info\n\
         ────────────────────────────\n\
         Status:           {}\n\
         Server mode:      {}\n\
         Listen port:      UDP {}\n\
         Root directory:   {}\n\
         Block size:       {} bytes\n\
         Active transfers: {}\n\
         Completed:        {}\n\
         Max transfers:    {}\n",
        if TFTP_INITIALIZED.load(Ordering::Relaxed) { "initialized" } else { "not initialized" },
        if server.running { "running" } else { "stopped" },
        DEFAULT_PORT,
        server.root_dir,
        BLOCK_SIZE,
        active,
        completed,
        MAX_TRANSFERS,
    );

    if !server.transfers.is_empty() {
        out += "\nTransfers:\n";
        for t in &server.transfers {
            let dir = if t.is_read { "READ" } else { "WRITE" };
            let status = if t.complete { "done" } else { "active" };
            out += &format!("  [{}] {} {} block={} bytes={} ({})\n",
                t.id, dir, t.filename, t.block, t.bytes_transferred, status);
        }
    }

    out
}

/// Get TFTP statistics.
pub fn tftp_stats() -> String {
    format!(
        "TFTP Statistics\n\
         ────────────────────────────\n\
         Files served:     {}\n\
         Files received:   {}\n\
         Bytes sent:       {}\n\
         Bytes received:   {}\n\
         Errors sent:      {}\n\
         Active transfers: {}",
        FILES_SERVED.load(Ordering::Relaxed),
        FILES_RECEIVED.load(Ordering::Relaxed),
        BYTES_SENT.load(Ordering::Relaxed),
        BYTES_RECEIVED.load(Ordering::Relaxed),
        ERRORS_SENT.load(Ordering::Relaxed),
        TRANSFERS_ACTIVE.load(Ordering::Relaxed),
    )
}
