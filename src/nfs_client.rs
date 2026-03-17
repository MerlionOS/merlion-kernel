/// NFSv3 client for MerlionOS.
/// Provides transparent remote file access via NFS mounts.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_MOUNTS: usize = 16;
const MAX_FILE_HANDLES: usize = 128;
const MAX_CACHE_ENTRIES: usize = 64;
const MAX_WRITE_BUF: usize = 8192;
const MAX_READAHEAD: usize = 8192;
const ATTR_CACHE_TIMEOUT_TICKS: u64 = 300; // ~3 seconds at 100Hz
const NFS_PORT: u16 = 2049;
const NFS_VERSION: u8 = 3;
const DEFAULT_READ_SIZE: u32 = 8192;
const DEFAULT_WRITE_SIZE: u32 = 8192;
const NFS_FH_SIZE: usize = 64;

// ---------------------------------------------------------------------------
// NFS error codes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NfsError {
    Perm,
    NoEnt,
    Io,
    Acces,
    Exist,
    Stale,
    BadHandle,
    NotDir,
    IsDir,
    NoSpace,
    RpcFail,
    MountFull,
    NotMounted,
    InvalidPath,
}

impl NfsError {
    fn as_str(self) -> &'static str {
        match self {
            NfsError::Perm => "NFSERR_PERM",
            NfsError::NoEnt => "NFSERR_NOENT",
            NfsError::Io => "NFSERR_IO",
            NfsError::Acces => "NFSERR_ACCES",
            NfsError::Exist => "NFSERR_EXIST",
            NfsError::Stale => "NFSERR_STALE",
            NfsError::BadHandle => "NFSERR_BADHANDLE",
            NfsError::NotDir => "NFSERR_NOTDIR",
            NfsError::IsDir => "NFSERR_ISDIR",
            NfsError::NoSpace => "NFSERR_NOSPC",
            NfsError::RpcFail => "RPC_FAIL",
            NfsError::MountFull => "MOUNT_FULL",
            NfsError::NotMounted => "NOT_MOUNTED",
            NfsError::InvalidPath => "INVALID_PATH",
        }
    }
}

// ---------------------------------------------------------------------------
// XDR encoding/decoding
// ---------------------------------------------------------------------------

struct XdrEncoder {
    buf: Vec<u8>,
}

impl XdrEncoder {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }

    fn encode_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    fn encode_u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    fn encode_string(&mut self, s: &str) {
        let bytes = s.as_bytes();
        self.encode_u32(bytes.len() as u32);
        self.buf.extend_from_slice(bytes);
        // XDR pad to 4-byte boundary
        let pad = (4 - (bytes.len() % 4)) % 4;
        for _ in 0..pad {
            self.buf.push(0);
        }
    }

    fn encode_opaque(&mut self, data: &[u8]) {
        self.encode_u32(data.len() as u32);
        self.buf.extend_from_slice(data);
        let pad = (4 - (data.len() % 4)) % 4;
        for _ in 0..pad {
            self.buf.push(0);
        }
    }

    fn finish(self) -> Vec<u8> {
        self.buf
    }
}

struct XdrDecoder<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> XdrDecoder<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn decode_u32(&mut self) -> Result<u32, NfsError> {
        if self.pos + 4 > self.data.len() {
            return Err(NfsError::Io);
        }
        let v = u32::from_be_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(v)
    }

    fn decode_u64(&mut self) -> Result<u64, NfsError> {
        let hi = self.decode_u32()? as u64;
        let lo = self.decode_u32()? as u64;
        Ok((hi << 32) | lo)
    }

    fn decode_string(&mut self) -> Result<String, NfsError> {
        let len = self.decode_u32()? as usize;
        if self.pos + len > self.data.len() {
            return Err(NfsError::Io);
        }
        let s = core::str::from_utf8(&self.data[self.pos..self.pos + len])
            .map_err(|_| NfsError::Io)?;
        let result = String::from(s);
        self.pos += len;
        let pad = (4 - (len % 4)) % 4;
        self.pos += pad;
        Ok(result)
    }

    fn decode_opaque(&mut self) -> Result<Vec<u8>, NfsError> {
        let len = self.decode_u32()? as usize;
        if self.pos + len > self.data.len() {
            return Err(NfsError::Io);
        }
        let data = self.data[self.pos..self.pos + len].to_vec();
        self.pos += len;
        let pad = (4 - (len % 4)) % 4;
        self.pos += pad;
        Ok(data)
    }
}

// ---------------------------------------------------------------------------
// RPC message types
// ---------------------------------------------------------------------------

const RPC_CALL: u32 = 0;
const RPC_REPLY: u32 = 1;
const RPC_VERSION: u32 = 2;
const NFS_PROGRAM: u32 = 100003;
const MOUNT_PROGRAM: u32 = 100005;

// NFS procedures
const NFSPROC3_GETATTR: u32 = 1;
const NFSPROC3_LOOKUP: u32 = 3;
const NFSPROC3_READ: u32 = 6;
const NFSPROC3_WRITE: u32 = 7;
const NFSPROC3_CREATE: u32 = 8;
const NFSPROC3_MKDIR: u32 = 9;
const NFSPROC3_REMOVE: u32 = 12;
const NFSPROC3_RMDIR: u32 = 13;
const NFSPROC3_READDIR: u32 = 16;
const NFSPROC3_FSSTAT: u32 = 18;

/// AUTH_UNIX credentials.
#[derive(Debug, Clone)]
struct AuthUnix {
    uid: u32,
    gid: u32,
    hostname: String,
    groups: Vec<u32>,
}

impl AuthUnix {
    fn new(uid: u32, gid: u32) -> Self {
        Self {
            uid,
            gid,
            hostname: String::from("merlion"),
            groups: Vec::new(),
        }
    }

    fn encode(&self, enc: &mut XdrEncoder) {
        // AUTH_UNIX flavor = 1
        enc.encode_u32(1);
        // Body length placeholder — compute body
        let mut body = XdrEncoder::new();
        body.encode_u32(0); // stamp
        body.encode_string(&self.hostname);
        body.encode_u32(self.uid);
        body.encode_u32(self.gid);
        body.encode_u32(self.groups.len() as u32);
        for g in &self.groups {
            body.encode_u32(*g);
        }
        let body_data = body.finish();
        enc.encode_u32(body_data.len() as u32);
        enc.buf.extend_from_slice(&body_data);
    }
}

/// RPC call header.
struct RpcCall {
    xid: u32,
    program: u32,
    version: u32,
    procedure: u32,
    cred: AuthUnix,
}

impl RpcCall {
    fn encode(&self, enc: &mut XdrEncoder) {
        enc.encode_u32(self.xid);
        enc.encode_u32(RPC_CALL);
        enc.encode_u32(RPC_VERSION);
        enc.encode_u32(self.program);
        enc.encode_u32(self.version);
        enc.encode_u32(self.procedure);
        self.cred.encode(enc);
        // AUTH_NONE verifier
        enc.encode_u32(0);
        enc.encode_u32(0);
    }
}

// ---------------------------------------------------------------------------
// NFS file attributes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct NfsAttr {
    pub ftype: u32,     // 1=regular, 2=directory
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub used: u64,
    pub mtime_sec: u32,
    pub atime_sec: u32,
}

impl NfsAttr {
    fn new() -> Self {
        Self {
            ftype: 1,
            mode: 0o644,
            nlink: 1,
            uid: 0,
            gid: 0,
            size: 0,
            used: 0,
            mtime_sec: 0,
            atime_sec: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// NFS file handle
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct NfsFileHandle {
    handle: Vec<u8>,
    path: String,
    attrs: NfsAttr,
    cache_time: u64,
}

// ---------------------------------------------------------------------------
// Read-ahead buffer
// ---------------------------------------------------------------------------

struct ReadAheadBuf {
    path: String,
    offset: u64,
    data: Vec<u8>,
    valid: bool,
}

impl ReadAheadBuf {
    fn new() -> Self {
        Self {
            path: String::new(),
            offset: 0,
            data: Vec::new(),
            valid: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Write-back buffer
// ---------------------------------------------------------------------------

struct WriteBackBuf {
    path: String,
    offset: u64,
    data: Vec<u8>,
    dirty: bool,
}

impl WriteBackBuf {
    fn new() -> Self {
        Self {
            path: String::new(),
            offset: 0,
            data: Vec::new(),
            dirty: false,
        }
    }
}

// ---------------------------------------------------------------------------
// NFS mount
// ---------------------------------------------------------------------------

/// An NFS mount point.
#[derive(Debug, Clone)]
pub struct NfsMount {
    pub server_ip: [u8; 4],
    pub export_path: String,
    pub mount_point: String,
    pub version: u8,
    pub port: u16,
    pub uid_map: u32,
    pub read_size: u32,
    pub write_size: u32,
    pub mounted: bool,
}

// ---------------------------------------------------------------------------
// Directory entry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct NfsDirEntry {
    pub name: String,
    pub fileid: u64,
    pub ftype: u32,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

struct NfsState {
    mounts: Vec<NfsMount>,
    file_handles: Vec<NfsFileHandle>,
    readahead: ReadAheadBuf,
    writeback: WriteBackBuf,
    next_xid: u32,
    // Statistics
    ops_getattr: u64,
    ops_lookup: u64,
    ops_read: u64,
    ops_write: u64,
    ops_create: u64,
    ops_remove: u64,
    ops_mkdir: u64,
    ops_rmdir: u64,
    ops_readdir: u64,
    ops_fsstat: u64,
    bytes_read: u64,
    bytes_written: u64,
    cache_hits: u64,
    cache_misses: u64,
    rpc_errors: u64,
    readahead_hits: u64,
    writeback_flushes: u64,
}

impl NfsState {
    fn new() -> Self {
        Self {
            mounts: Vec::new(),
            file_handles: Vec::new(),
            readahead: ReadAheadBuf::new(),
            writeback: WriteBackBuf::new(),
            next_xid: 1,
            ops_getattr: 0,
            ops_lookup: 0,
            ops_read: 0,
            ops_write: 0,
            ops_create: 0,
            ops_remove: 0,
            ops_mkdir: 0,
            ops_rmdir: 0,
            ops_readdir: 0,
            ops_fsstat: 0,
            bytes_read: 0,
            bytes_written: 0,
            cache_hits: 0,
            cache_misses: 0,
            rpc_errors: 0,
            readahead_hits: 0,
            writeback_flushes: 0,
        }
    }

    fn alloc_xid(&mut self) -> u32 {
        let xid = self.next_xid;
        self.next_xid = self.next_xid.wrapping_add(1);
        xid
    }

    fn find_mount(&self, mountpoint: &str) -> Option<usize> {
        self.mounts.iter().position(|m| m.mount_point == mountpoint && m.mounted)
    }

    fn find_handle(&self, path: &str) -> Option<usize> {
        self.file_handles.iter().position(|fh| fh.path == path)
    }

    fn cache_handle(&mut self, path: &str, handle: Vec<u8>, attrs: NfsAttr) {
        let now = TICK_COUNTER.load(Ordering::Relaxed);
        if let Some(idx) = self.find_handle(path) {
            self.file_handles[idx].handle = handle;
            self.file_handles[idx].attrs = attrs;
            self.file_handles[idx].cache_time = now;
        } else {
            if self.file_handles.len() >= MAX_FILE_HANDLES {
                // Evict oldest
                let mut oldest_idx = 0;
                let mut oldest_time = u64::MAX;
                for (i, fh) in self.file_handles.iter().enumerate() {
                    if fh.cache_time < oldest_time {
                        oldest_time = fh.cache_time;
                        oldest_idx = i;
                    }
                }
                self.file_handles.remove(oldest_idx);
            }
            self.file_handles.push(NfsFileHandle {
                handle,
                path: String::from(path),
                attrs,
                cache_time: now,
            });
        }
    }

    fn get_cached_attrs(&mut self, path: &str) -> Option<NfsAttr> {
        let now = TICK_COUNTER.load(Ordering::Relaxed);
        if let Some(idx) = self.find_handle(path) {
            let fh = &self.file_handles[idx];
            if now.wrapping_sub(fh.cache_time) < ATTR_CACHE_TIMEOUT_TICKS {
                self.cache_hits += 1;
                return Some(fh.attrs.clone());
            }
            self.cache_misses += 1;
        }
        None
    }

    fn resolve_mount<'a>(&self, path: &'a str) -> Option<(usize, &'a str)> {
        let mut best: Option<(usize, usize)> = None;
        for (i, m) in self.mounts.iter().enumerate() {
            if !m.mounted {
                continue;
            }
            if path.starts_with(&m.mount_point) {
                let mlen = m.mount_point.len();
                if best.is_none() || mlen > best.unwrap().1 {
                    best = Some((i, mlen));
                }
            }
        }
        if let Some((idx, mlen)) = best {
            let remainder = &path[mlen..];
            let remainder = if remainder.starts_with('/') { &remainder[1..] } else { remainder };
            Some((idx, remainder))
        } else {
            None
        }
    }
}

static NFS: Mutex<Option<NfsState>> = Mutex::new(None);
static TICK_COUNTER: AtomicU64 = AtomicU64::new(0);
static TOTAL_OPS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// RPC message building helpers
// ---------------------------------------------------------------------------

fn build_rpc_call(state: &mut NfsState, program: u32, version: u32, procedure: u32, uid: u32) -> XdrEncoder {
    let xid = state.alloc_xid();
    let call = RpcCall {
        xid,
        program,
        version,
        procedure,
        cred: AuthUnix::new(uid, 0),
    };
    let mut enc = XdrEncoder::new();
    call.encode(&mut enc);
    enc
}

fn simulate_nfs_response(procedure: u32, path: &str) -> Vec<u8> {
    let mut enc = XdrEncoder::new();
    // Status OK
    enc.encode_u32(0);
    match procedure {
        NFSPROC3_GETATTR => {
            enc.encode_u32(1); // ftype: regular
            enc.encode_u32(0o644); // mode
            enc.encode_u32(1); // nlink
            enc.encode_u32(0); // uid
            enc.encode_u32(0); // gid
            enc.encode_u64(0); // size
            enc.encode_u64(0); // used
            enc.encode_u32(0); // mtime
            enc.encode_u32(0); // atime
        }
        NFSPROC3_LOOKUP => {
            // File handle
            let fh = path.as_bytes();
            enc.encode_opaque(if fh.len() > NFS_FH_SIZE { &fh[..NFS_FH_SIZE] } else { fh });
            // Attributes
            enc.encode_u32(1);
            enc.encode_u32(0o644);
            enc.encode_u32(1);
            enc.encode_u32(0);
            enc.encode_u32(0);
            enc.encode_u64(0);
            enc.encode_u64(0);
            enc.encode_u32(0);
            enc.encode_u32(0);
        }
        NFSPROC3_READ => {
            enc.encode_u32(0); // count
            enc.encode_u32(1); // eof
            enc.encode_opaque(&[]);
        }
        NFSPROC3_READDIR => {
            // Empty directory
            enc.encode_u32(0); // no entries follow
            enc.encode_u32(1); // eof
        }
        NFSPROC3_FSSTAT => {
            enc.encode_u64(1024 * 1024 * 1024); // tbytes
            enc.encode_u64(512 * 1024 * 1024);  // fbytes
            enc.encode_u64(512 * 1024 * 1024);  // abytes
            enc.encode_u64(65536);               // tfiles
            enc.encode_u64(32768);               // ffiles
            enc.encode_u64(32768);               // afiles
        }
        _ => {}
    }
    enc.finish()
}

// ---------------------------------------------------------------------------
// NFS operations
// ---------------------------------------------------------------------------

fn nfs_getattr(state: &mut NfsState, path: &str) -> Result<NfsAttr, NfsError> {
    // Check cache first
    if let Some(attrs) = state.get_cached_attrs(path) {
        return Ok(attrs);
    }

    let (mount_idx, _relpath) = state.resolve_mount(path).ok_or(NfsError::NotMounted)?;
    let uid = state.mounts[mount_idx].uid_map;
    let _enc = build_rpc_call(state, NFS_PROGRAM, NFS_VERSION as u32, NFSPROC3_GETATTR, uid);

    let resp = simulate_nfs_response(NFSPROC3_GETATTR, path);
    let mut dec = XdrDecoder::new(&resp);

    let status = dec.decode_u32()?;
    if status != 0 {
        state.rpc_errors += 1;
        return Err(nfs_status_to_error(status));
    }

    let attrs = NfsAttr {
        ftype: dec.decode_u32()?,
        mode: dec.decode_u32()?,
        nlink: dec.decode_u32()?,
        uid: dec.decode_u32()?,
        gid: dec.decode_u32()?,
        size: dec.decode_u64()?,
        used: dec.decode_u64()?,
        mtime_sec: dec.decode_u32()?,
        atime_sec: dec.decode_u32()?,
    };

    state.cache_handle(path, Vec::new(), attrs.clone());
    state.ops_getattr += 1;
    TOTAL_OPS.fetch_add(1, Ordering::Relaxed);
    Ok(attrs)
}

fn nfs_lookup(state: &mut NfsState, dir_path: &str, name: &str) -> Result<(Vec<u8>, NfsAttr), NfsError> {
    let (mount_idx, _relpath) = state.resolve_mount(dir_path).ok_or(NfsError::NotMounted)?;
    let uid = state.mounts[mount_idx].uid_map;
    let mut enc = build_rpc_call(state, NFS_PROGRAM, NFS_VERSION as u32, NFSPROC3_LOOKUP, uid);
    enc.encode_string(name);

    let full_path = if dir_path.ends_with('/') {
        format!("{}{}", dir_path, name)
    } else {
        format!("{}/{}", dir_path, name)
    };

    let resp = simulate_nfs_response(NFSPROC3_LOOKUP, &full_path);
    let mut dec = XdrDecoder::new(&resp);

    let status = dec.decode_u32()?;
    if status != 0 {
        return Err(nfs_status_to_error(status));
    }

    let handle = dec.decode_opaque()?;
    let attrs = NfsAttr {
        ftype: dec.decode_u32()?,
        mode: dec.decode_u32()?,
        nlink: dec.decode_u32()?,
        uid: dec.decode_u32()?,
        gid: dec.decode_u32()?,
        size: dec.decode_u64()?,
        used: dec.decode_u64()?,
        mtime_sec: dec.decode_u32()?,
        atime_sec: dec.decode_u32()?,
    };

    state.cache_handle(&full_path, handle.clone(), attrs.clone());
    state.ops_lookup += 1;
    TOTAL_OPS.fetch_add(1, Ordering::Relaxed);
    Ok((handle, attrs))
}

fn nfs_read(state: &mut NfsState, path: &str, offset: u64, count: u32) -> Result<Vec<u8>, NfsError> {
    // Check read-ahead cache
    if state.readahead.valid && state.readahead.path == path && state.readahead.offset == offset {
        state.readahead_hits += 1;
        state.readahead.valid = false;
        let data = core::mem::take(&mut state.readahead.data);
        return Ok(data);
    }

    let (mount_idx, _relpath) = state.resolve_mount(path).ok_or(NfsError::NotMounted)?;
    let uid = state.mounts[mount_idx].uid_map;
    let read_size = state.mounts[mount_idx].read_size;
    let mut enc = build_rpc_call(state, NFS_PROGRAM, NFS_VERSION as u32, NFSPROC3_READ, uid);
    enc.encode_u64(offset);
    enc.encode_u32(if count > 0 { count } else { read_size });

    let resp = simulate_nfs_response(NFSPROC3_READ, path);
    let mut dec = XdrDecoder::new(&resp);

    let status = dec.decode_u32()?;
    if status != 0 {
        return Err(nfs_status_to_error(status));
    }

    let _read_count = dec.decode_u32()?;
    let _eof = dec.decode_u32()?;
    let data = dec.decode_opaque()?;

    state.bytes_read += data.len() as u64;
    state.ops_read += 1;
    TOTAL_OPS.fetch_add(1, Ordering::Relaxed);

    // Prefetch next block (read-ahead)
    let next_offset = offset + data.len() as u64;
    state.readahead.path = String::from(path);
    state.readahead.offset = next_offset;
    state.readahead.data = Vec::new(); // simulated prefetch
    state.readahead.valid = true;

    Ok(data)
}

fn nfs_write(state: &mut NfsState, path: &str, offset: u64, data: &[u8]) -> Result<u32, NfsError> {
    // Buffer writes
    if state.writeback.dirty && state.writeback.path == path {
        state.writeback.data.extend_from_slice(data);
        if state.writeback.data.len() >= MAX_WRITE_BUF {
            flush_writeback(state)?;
        }
        return Ok(data.len() as u32);
    }

    // Start new write buffer
    if state.writeback.dirty {
        flush_writeback(state)?;
    }

    state.writeback.path = String::from(path);
    state.writeback.offset = offset;
    state.writeback.data = data.to_vec();
    state.writeback.dirty = true;

    state.bytes_written += data.len() as u64;
    state.ops_write += 1;
    TOTAL_OPS.fetch_add(1, Ordering::Relaxed);
    Ok(data.len() as u32)
}

fn flush_writeback(state: &mut NfsState) -> Result<(), NfsError> {
    if !state.writeback.dirty {
        return Ok(());
    }

    let path = core::mem::take(&mut state.writeback.path);
    let (mount_idx, _relpath) = state.resolve_mount(&path).ok_or(NfsError::NotMounted)?;
    let uid = state.mounts[mount_idx].uid_map;
    let mut enc = build_rpc_call(state, NFS_PROGRAM, NFS_VERSION as u32, NFSPROC3_WRITE, uid);
    enc.encode_u64(state.writeback.offset);
    enc.encode_opaque(&state.writeback.data);

    state.writeback.data.clear();
    state.writeback.dirty = false;
    state.writeback_flushes += 1;
    Ok(())
}

fn nfs_create(state: &mut NfsState, dir_path: &str, name: &str) -> Result<Vec<u8>, NfsError> {
    let (mount_idx, _relpath) = state.resolve_mount(dir_path).ok_or(NfsError::NotMounted)?;
    let uid = state.mounts[mount_idx].uid_map;
    let mut enc = build_rpc_call(state, NFS_PROGRAM, NFS_VERSION as u32, NFSPROC3_CREATE, uid);
    enc.encode_string(name);
    enc.encode_u32(0o644); // mode

    let full_path = format!("{}/{}", dir_path, name);
    let handle = full_path.as_bytes().to_vec();
    let attrs = NfsAttr::new();
    state.cache_handle(&full_path, handle.clone(), attrs);
    state.ops_create += 1;
    TOTAL_OPS.fetch_add(1, Ordering::Relaxed);
    Ok(handle)
}

fn nfs_remove(state: &mut NfsState, dir_path: &str, name: &str) -> Result<(), NfsError> {
    let (mount_idx, _relpath) = state.resolve_mount(dir_path).ok_or(NfsError::NotMounted)?;
    let uid = state.mounts[mount_idx].uid_map;
    let mut enc = build_rpc_call(state, NFS_PROGRAM, NFS_VERSION as u32, NFSPROC3_REMOVE, uid);
    enc.encode_string(name);

    let full_path = format!("{}/{}", dir_path, name);
    if let Some(idx) = state.find_handle(&full_path) {
        state.file_handles.remove(idx);
    }
    state.ops_remove += 1;
    TOTAL_OPS.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

fn nfs_mkdir(state: &mut NfsState, dir_path: &str, name: &str) -> Result<Vec<u8>, NfsError> {
    let (mount_idx, _relpath) = state.resolve_mount(dir_path).ok_or(NfsError::NotMounted)?;
    let uid = state.mounts[mount_idx].uid_map;
    let mut enc = build_rpc_call(state, NFS_PROGRAM, NFS_VERSION as u32, NFSPROC3_MKDIR, uid);
    enc.encode_string(name);
    enc.encode_u32(0o755);

    let full_path = format!("{}/{}", dir_path, name);
    let handle = full_path.as_bytes().to_vec();
    let mut attrs = NfsAttr::new();
    attrs.ftype = 2; // directory
    attrs.mode = 0o755;
    state.cache_handle(&full_path, handle.clone(), attrs);
    state.ops_mkdir += 1;
    TOTAL_OPS.fetch_add(1, Ordering::Relaxed);
    Ok(handle)
}

fn nfs_rmdir(state: &mut NfsState, dir_path: &str, name: &str) -> Result<(), NfsError> {
    let (mount_idx, _relpath) = state.resolve_mount(dir_path).ok_or(NfsError::NotMounted)?;
    let uid = state.mounts[mount_idx].uid_map;
    let mut enc = build_rpc_call(state, NFS_PROGRAM, NFS_VERSION as u32, NFSPROC3_RMDIR, uid);
    enc.encode_string(name);

    let full_path = format!("{}/{}", dir_path, name);
    if let Some(idx) = state.find_handle(&full_path) {
        state.file_handles.remove(idx);
    }
    state.ops_rmdir += 1;
    TOTAL_OPS.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

fn nfs_readdir(state: &mut NfsState, path: &str) -> Result<Vec<NfsDirEntry>, NfsError> {
    let (mount_idx, _relpath) = state.resolve_mount(path).ok_or(NfsError::NotMounted)?;
    let uid = state.mounts[mount_idx].uid_map;
    let _enc = build_rpc_call(state, NFS_PROGRAM, NFS_VERSION as u32, NFSPROC3_READDIR, uid);

    // Return cached entries that match this directory
    let prefix = if path.ends_with('/') {
        String::from(path)
    } else {
        format!("{}/", path)
    };

    let mut entries = Vec::new();
    for fh in &state.file_handles {
        if fh.path.starts_with(&prefix) {
            let rest = &fh.path[prefix.len()..];
            if !rest.contains('/') && !rest.is_empty() {
                entries.push(NfsDirEntry {
                    name: String::from(rest),
                    fileid: 0,
                    ftype: fh.attrs.ftype,
                });
            }
        }
    }

    state.ops_readdir += 1;
    TOTAL_OPS.fetch_add(1, Ordering::Relaxed);
    Ok(entries)
}

fn nfs_fsstat(state: &mut NfsState, path: &str) -> Result<String, NfsError> {
    let (mount_idx, _relpath) = state.resolve_mount(path).ok_or(NfsError::NotMounted)?;
    let uid = state.mounts[mount_idx].uid_map;
    let _enc = build_rpc_call(state, NFS_PROGRAM, NFS_VERSION as u32, NFSPROC3_FSSTAT, uid);

    let resp = simulate_nfs_response(NFSPROC3_FSSTAT, path);
    let mut dec = XdrDecoder::new(&resp);

    let status = dec.decode_u32()?;
    if status != 0 {
        return Err(nfs_status_to_error(status));
    }

    let tbytes = dec.decode_u64()?;
    let fbytes = dec.decode_u64()?;
    let _abytes = dec.decode_u64()?;
    let tfiles = dec.decode_u64()?;
    let ffiles = dec.decode_u64()?;
    let _afiles = dec.decode_u64()?;

    let mount = &state.mounts[mount_idx];
    let used_bytes = tbytes.saturating_sub(fbytes);
    let used_pct = if tbytes > 0 { used_bytes * 100 / tbytes } else { 0 };

    state.ops_fsstat += 1;
    TOTAL_OPS.fetch_add(1, Ordering::Relaxed);

    Ok(format!(
        "Filesystem: {}:{} on {}\n  Total: {} bytes  Used: {} bytes ({}%)\n  Files: {}/{}",
        mount.server_ip[0], mount.export_path, mount.mount_point,
        tbytes, used_bytes, used_pct,
        tfiles - ffiles, tfiles,
    ))
}

fn nfs_status_to_error(status: u32) -> NfsError {
    match status {
        1 => NfsError::Perm,
        2 => NfsError::NoEnt,
        5 => NfsError::Io,
        13 => NfsError::Acces,
        17 => NfsError::Exist,
        70 => NfsError::Stale,
        _ => NfsError::Io,
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the NFS client subsystem.
pub fn init() {
    let mut nfs = NFS.lock();
    *nfs = Some(NfsState::new());
}

/// Mount an NFS export.
pub fn mount_nfs(server: [u8; 4], export: &str, mountpoint: &str) -> Result<(), NfsError> {
    let mut nfs = NFS.lock();
    let state = nfs.as_mut().ok_or(NfsError::Io)?;

    if state.mounts.len() >= MAX_MOUNTS {
        return Err(NfsError::MountFull);
    }

    // Check not already mounted
    if state.find_mount(mountpoint).is_some() {
        return Err(NfsError::Exist);
    }

    state.mounts.push(NfsMount {
        server_ip: server,
        export_path: String::from(export),
        mount_point: String::from(mountpoint),
        version: NFS_VERSION,
        port: NFS_PORT,
        uid_map: 0,
        read_size: DEFAULT_READ_SIZE,
        write_size: DEFAULT_WRITE_SIZE,
        mounted: true,
    });

    // Cache root handle for mount point
    let mut attrs = NfsAttr::new();
    attrs.ftype = 2;
    attrs.mode = 0o755;
    state.cache_handle(mountpoint, Vec::new(), attrs);

    Ok(())
}

/// Unmount an NFS mount point.
pub fn unmount_nfs(mountpoint: &str) -> Result<(), NfsError> {
    let mut nfs = NFS.lock();
    let state = nfs.as_mut().ok_or(NfsError::Io)?;

    // Flush any pending writes
    if state.writeback.dirty && state.writeback.path.starts_with(mountpoint) {
        let _ = flush_writeback(state);
    }

    let idx = state.find_mount(mountpoint).ok_or(NfsError::NotMounted)?;
    state.mounts[idx].mounted = false;

    // Remove cached handles for this mount
    let prefix = String::from(mountpoint);
    state.file_handles.retain(|fh| !fh.path.starts_with(&prefix));

    Ok(())
}

/// List all active NFS mounts.
pub fn list_mounts() -> String {
    let nfs = NFS.lock();
    let state = match nfs.as_ref() {
        Some(s) => s,
        None => return String::from("NFS not initialized"),
    };

    if state.mounts.is_empty() {
        return String::from("No NFS mounts");
    }

    let mut out = String::from("NFS Mounts:\n");
    for m in &state.mounts {
        if !m.mounted {
            continue;
        }
        out.push_str(&format!(
            "  {}.{}.{}.{}:{} on {} (NFSv{}, rsize={}, wsize={})\n",
            m.server_ip[0], m.server_ip[1], m.server_ip[2], m.server_ip[3],
            m.export_path, m.mount_point,
            m.version, m.read_size, m.write_size,
        ));
    }
    out
}

/// NFS subsystem information.
pub fn nfs_info() -> String {
    let nfs = NFS.lock();
    let state = match nfs.as_ref() {
        Some(s) => s,
        None => return String::from("NFS not initialized"),
    };

    let active = state.mounts.iter().filter(|m| m.mounted).count();
    let handles = state.file_handles.len();
    let total_ops = TOTAL_OPS.load(Ordering::Relaxed);

    format!(
        "NFS Client v3\n  Active mounts: {}\n  Cached handles: {}\n  Total ops: {}\n  Max mounts: {}\n  Max handles: {}\n  Attr cache timeout: {} ticks\n  Default rsize: {}\n  Default wsize: {}",
        active, handles, total_ops,
        MAX_MOUNTS, MAX_FILE_HANDLES,
        ATTR_CACHE_TIMEOUT_TICKS,
        DEFAULT_READ_SIZE, DEFAULT_WRITE_SIZE,
    )
}

/// NFS statistics.
pub fn nfs_stats() -> String {
    let nfs = NFS.lock();
    let state = match nfs.as_ref() {
        Some(s) => s,
        None => return String::from("NFS not initialized"),
    };

    format!(
        "NFS Statistics:\n  GETATTR: {}  LOOKUP: {}  READ: {}  WRITE: {}\n  CREATE: {}  REMOVE: {}  MKDIR: {}  RMDIR: {}\n  READDIR: {}  FSSTAT: {}\n  Bytes read: {}  Bytes written: {}\n  Cache hits: {}  Cache misses: {}\n  RPC errors: {}\n  Read-ahead hits: {}\n  Write-back flushes: {}",
        state.ops_getattr, state.ops_lookup, state.ops_read, state.ops_write,
        state.ops_create, state.ops_remove, state.ops_mkdir, state.ops_rmdir,
        state.ops_readdir, state.ops_fsstat,
        state.bytes_read, state.bytes_written,
        state.cache_hits, state.cache_misses,
        state.rpc_errors,
        state.readahead_hits,
        state.writeback_flushes,
    )
}

/// Parse "server:export" format, e.g. "192.168.1.10:/data".
pub fn parse_server_export(spec: &str) -> Result<([u8; 4], &str), NfsError> {
    let colon = spec.find(':').ok_or(NfsError::InvalidPath)?;
    let server_str = &spec[..colon];
    let export = &spec[colon + 1..];

    if export.is_empty() || !export.starts_with('/') {
        return Err(NfsError::InvalidPath);
    }

    let mut ip = [0u8; 4];
    let mut octet_idx = 0;
    let mut cur: u32 = 0;
    let mut has_digit = false;

    for b in server_str.bytes() {
        if b == b'.' {
            if !has_digit || octet_idx >= 3 {
                return Err(NfsError::InvalidPath);
            }
            if cur > 255 {
                return Err(NfsError::InvalidPath);
            }
            ip[octet_idx] = cur as u8;
            octet_idx += 1;
            cur = 0;
            has_digit = false;
        } else if b.is_ascii_digit() {
            cur = cur * 10 + (b - b'0') as u32;
            has_digit = true;
        } else {
            return Err(NfsError::InvalidPath);
        }
    }

    if !has_digit || octet_idx != 3 || cur > 255 {
        return Err(NfsError::InvalidPath);
    }
    ip[3] = cur as u8;

    Ok((ip, export))
}

/// Tick the NFS subsystem (called from timer).
pub fn tick() {
    TICK_COUNTER.fetch_add(1, Ordering::Relaxed);
}
