/// WASI runtime for MerlionOS.
/// Implements WASI preview1 syscall interface for WebAssembly modules,
/// providing filesystem, clock, random, and environment access.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::borrow::ToOwned;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// WASM binary magic number: `\0asm`.
const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];

/// WASM binary format version 1.
const WASM_VERSION: [u8; 4] = [0x01, 0x00, 0x00, 0x00];

/// Maximum number of open file descriptors per WASI module.
const MAX_FDS: usize = 64;

/// Maximum linear memory size (64 KiB pages, start with 16 pages = 1 MiB).
const MAX_MEMORY_PAGES: usize = 256;

/// Size of a single WASM memory page in bytes.
const WASM_PAGE_SIZE: usize = 65536;

/// Default initial memory pages.
const INITIAL_MEMORY_PAGES: usize = 16;

/// Maximum number of loaded WASI modules.
const MAX_MODULES: usize = 16;

/// Maximum environment variables per module.
const MAX_ENV_VARS: usize = 32;

/// Maximum arguments per module.
const MAX_ARGS: usize = 32;

// ---------------------------------------------------------------------------
// WASI Error codes (subset of wasi-libc errno values)
// ---------------------------------------------------------------------------

/// WASI error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasiError {
    Success,
    Badf,
    Inval,
    Noent,
    Nosys,
    Overflow,
    Acces,
    Io,
    Fault,
    Nomem,
}

impl WasiError {
    /// Convert to WASI errno number.
    pub fn code(self) -> u32 {
        match self {
            WasiError::Success  => 0,
            WasiError::Badf     => 8,
            WasiError::Inval    => 28,
            WasiError::Noent    => 44,
            WasiError::Nosys    => 52,
            WasiError::Overflow => 61,
            WasiError::Acces    => 2,
            WasiError::Io       => 29,
            WasiError::Fault    => 21,
            WasiError::Nomem    => 48,
        }
    }
}

// ---------------------------------------------------------------------------
// WASI Syscalls
// ---------------------------------------------------------------------------

/// Supported WASI preview1 syscall identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasiSyscall {
    ArgsGet,
    ArgsSizesGet,
    EnvironGet,
    EnvironSizesGet,
    ClockTimeGet,
    FdRead,
    FdWrite,
    FdClose,
    FdSeek,
    FdPrestatGet,
    PathOpen,
    PathCreateDirectory,
    PathRemoveDirectory,
    PathUnlinkFile,
    ProcExit,
    RandomGet,
}

// ---------------------------------------------------------------------------
// File descriptor table
// ---------------------------------------------------------------------------

/// Type of a WASI file descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdType {
    /// Stdin.
    Stdin,
    /// Stdout.
    Stdout,
    /// Stderr.
    Stderr,
    /// A preopened directory.
    PreopenDir,
    /// A regular file opened via path_open.
    RegularFile,
}

/// An entry in the WASI file descriptor table.
#[derive(Debug, Clone)]
pub struct WasiFd {
    /// WASI fd number.
    pub fd: u32,
    /// Type of this descriptor.
    pub fd_type: FdType,
    /// VFS path this fd maps to (empty for stdin/stdout/stderr).
    pub path: String,
    /// Current seek offset for regular files.
    pub offset: u32,
    /// Whether this fd is open.
    pub open: bool,
}

/// File descriptor table for a WASI module instance.
pub struct FdTable {
    entries: Vec<WasiFd>,
    next_fd: u32,
}

impl FdTable {
    /// Create a new fd table with the standard three descriptors.
    fn new() -> Self {
        let mut entries = Vec::new();
        entries.push(WasiFd { fd: 0, fd_type: FdType::Stdin, path: String::new(), offset: 0, open: true });
        entries.push(WasiFd { fd: 1, fd_type: FdType::Stdout, path: String::new(), offset: 0, open: true });
        entries.push(WasiFd { fd: 2, fd_type: FdType::Stderr, path: String::new(), offset: 0, open: true });
        Self { entries, next_fd: 3 }
    }

    /// Add a preopened directory and return its fd number.
    fn preopen_dir(&mut self, path: &str) -> Option<u32> {
        if self.entries.len() >= MAX_FDS {
            return None;
        }
        let fd = self.next_fd;
        self.next_fd += 1;
        self.entries.push(WasiFd {
            fd,
            fd_type: FdType::PreopenDir,
            path: path.to_owned(),
            offset: 0,
            open: true,
        });
        Some(fd)
    }

    /// Open a regular file and return its fd number.
    fn open_file(&mut self, path: &str) -> Option<u32> {
        if self.entries.len() >= MAX_FDS {
            return None;
        }
        let fd = self.next_fd;
        self.next_fd += 1;
        self.entries.push(WasiFd {
            fd,
            fd_type: FdType::RegularFile,
            path: path.to_owned(),
            offset: 0,
            open: true,
        });
        Some(fd)
    }

    /// Look up an open fd.
    fn get(&self, fd: u32) -> Option<&WasiFd> {
        self.entries.iter().find(|e| e.fd == fd && e.open)
    }

    /// Look up an open fd mutably.
    fn get_mut(&mut self, fd: u32) -> Option<&mut WasiFd> {
        self.entries.iter_mut().find(|e| e.fd == fd && e.open)
    }

    /// Close a file descriptor.
    fn close(&mut self, fd: u32) -> Result<(), WasiError> {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.fd == fd && e.open) {
            entry.open = false;
            Ok(())
        } else {
            Err(WasiError::Badf)
        }
    }
}

// ---------------------------------------------------------------------------
// WASI linear memory
// ---------------------------------------------------------------------------

/// Simulated WASM linear memory.
pub struct WasiMemory {
    /// Backing storage.
    data: Vec<u8>,
    /// Number of allocated pages.
    pages: usize,
}

impl WasiMemory {
    /// Create linear memory with `initial_pages` pages.
    fn new(initial_pages: usize) -> Self {
        let size = initial_pages * WASM_PAGE_SIZE;
        Self {
            data: vec![0u8; size],
            pages: initial_pages,
        }
    }

    /// Read bytes from linear memory.
    fn read(&self, offset: u32, len: u32) -> Result<&[u8], WasiError> {
        let start = offset as usize;
        let end = start + len as usize;
        if end > self.data.len() {
            return Err(WasiError::Fault);
        }
        Ok(&self.data[start..end])
    }

    /// Write bytes into linear memory.
    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), WasiError> {
        let start = offset as usize;
        let end = start + bytes.len();
        if end > self.data.len() {
            return Err(WasiError::Fault);
        }
        self.data[start..end].copy_from_slice(bytes);
        Ok(())
    }

    /// Write a u32 (little-endian) into linear memory.
    fn write_u32(&mut self, offset: u32, val: u32) -> Result<(), WasiError> {
        self.write(offset, &val.to_le_bytes())
    }

    /// Grow memory by `additional` pages. Returns old page count or error.
    fn grow(&mut self, additional: usize) -> Result<usize, WasiError> {
        let new_pages = self.pages + additional;
        if new_pages > MAX_MEMORY_PAGES {
            return Err(WasiError::Nomem);
        }
        let old = self.pages;
        self.data.resize(new_pages * WASM_PAGE_SIZE, 0);
        self.pages = new_pages;
        Ok(old)
    }

    /// Current size in bytes.
    fn size(&self) -> usize {
        self.data.len()
    }
}

// ---------------------------------------------------------------------------
// WASI execution context (per-module instance)
// ---------------------------------------------------------------------------

/// Execution context for a running WASI module.
pub struct WasiContext {
    /// Module name.
    pub name: String,
    /// Command-line arguments.
    pub args: Vec<String>,
    /// Environment variables (KEY=VALUE format).
    pub env_vars: Vec<String>,
    /// File descriptor table.
    pub fds: FdTable,
    /// Linear memory.
    pub memory: WasiMemory,
    /// Exit code (set by proc_exit).
    pub exit_code: Option<u32>,
    /// Preopened directories.
    pub preopens: Vec<String>,
    /// Stdout capture buffer.
    pub stdout_buf: Vec<u8>,
}

impl WasiContext {
    /// Create a new WASI context for the given module name.
    pub fn new(name: &str) -> Self {
        let mut ctx = Self {
            name: name.to_owned(),
            args: Vec::new(),
            env_vars: Vec::new(),
            fds: FdTable::new(),
            memory: WasiMemory::new(INITIAL_MEMORY_PAGES),
            exit_code: None,
            preopens: Vec::new(),
            stdout_buf: Vec::new(),
        };
        // Preopen "/" by default.
        ctx.preopen("/");
        ctx
    }

    /// Add a command-line argument.
    pub fn add_arg(&mut self, arg: &str) {
        if self.args.len() < MAX_ARGS {
            self.args.push(arg.to_owned());
        }
    }

    /// Add an environment variable in KEY=VALUE form.
    pub fn add_env(&mut self, var: &str) {
        if self.env_vars.len() < MAX_ENV_VARS {
            self.env_vars.push(var.to_owned());
        }
    }

    /// Preopen a directory.
    pub fn preopen(&mut self, path: &str) {
        if let Some(_fd) = self.fds.preopen_dir(path) {
            self.preopens.push(path.to_owned());
        }
    }
}

// ---------------------------------------------------------------------------
// WASI Module
// ---------------------------------------------------------------------------

/// A loaded WASI module (header validated, ready to run).
pub struct WasiModule {
    /// Module name.
    pub name: String,
    /// Raw WASM binary.
    pub binary: Vec<u8>,
    /// Execution context.
    pub context: WasiContext,
}

// ---------------------------------------------------------------------------
// Syscall dispatch
// ---------------------------------------------------------------------------

/// Simple PRNG state for random_get (xorshift32).
static PRNG_STATE: Mutex<u32> = Mutex::new(0xDEAD_BEEF);

fn xorshift32() -> u32 {
    let mut state = PRNG_STATE.lock();
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

/// Monotonic clock tick counter (microseconds, approximate).
static CLOCK_US: AtomicU64 = AtomicU64::new(0);

/// Advance the simulated clock by `delta_us` microseconds.
pub fn tick_clock(delta_us: u64) {
    CLOCK_US.fetch_add(delta_us, Ordering::Relaxed);
}

/// Dispatch a WASI syscall.
///
/// `ctx` is the module's execution context.
/// `syscall` identifies the WASI function.
/// `args` are syscall-specific u32 arguments (offsets, lengths, fd numbers, etc.).
///
/// Returns a result buffer on success or a `WasiError` on failure.
pub fn wasi_call(
    ctx: &mut WasiContext,
    syscall: WasiSyscall,
    args: &[u32],
) -> Result<Vec<u8>, WasiError> {
    WASI_STATS.calls.fetch_add(1, Ordering::Relaxed);

    match syscall {
        WasiSyscall::ArgsSizesGet => {
            // Returns (argc: u32, argv_buf_size: u32).
            let argc = ctx.args.len() as u32;
            let buf_size: u32 = ctx.args.iter()
                .map(|a| a.len() as u32 + 1) // +1 for NUL
                .sum();
            let mut out = Vec::with_capacity(8);
            out.extend_from_slice(&argc.to_le_bytes());
            out.extend_from_slice(&buf_size.to_le_bytes());
            Ok(out)
        }
        WasiSyscall::ArgsGet => {
            // Write args into linear memory at args[0] (argv) and args[1] (buf).
            if args.len() < 2 { return Err(WasiError::Inval); }
            let argv_ptr = args[0];
            let mut buf_ptr = args[1];
            for (i, arg) in ctx.args.iter().enumerate() {
                // Write pointer to argv table.
                ctx.memory.write_u32(argv_ptr + (i as u32) * 4, buf_ptr)?;
                // Write string + NUL to buffer.
                ctx.memory.write(buf_ptr, arg.as_bytes())?;
                ctx.memory.write(buf_ptr + arg.len() as u32, &[0])?;
                buf_ptr += arg.len() as u32 + 1;
            }
            Ok(Vec::new())
        }
        WasiSyscall::EnvironSizesGet => {
            let count = ctx.env_vars.len() as u32;
            let buf_size: u32 = ctx.env_vars.iter()
                .map(|e| e.len() as u32 + 1)
                .sum();
            let mut out = Vec::with_capacity(8);
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&buf_size.to_le_bytes());
            Ok(out)
        }
        WasiSyscall::EnvironGet => {
            if args.len() < 2 { return Err(WasiError::Inval); }
            let env_ptr = args[0];
            let mut buf_ptr = args[1];
            for (i, var) in ctx.env_vars.iter().enumerate() {
                ctx.memory.write_u32(env_ptr + (i as u32) * 4, buf_ptr)?;
                ctx.memory.write(buf_ptr, var.as_bytes())?;
                ctx.memory.write(buf_ptr + var.len() as u32, &[0])?;
                buf_ptr += var.len() as u32 + 1;
            }
            Ok(Vec::new())
        }
        WasiSyscall::ClockTimeGet => {
            // Returns monotonic time in nanoseconds as u64.
            let us = CLOCK_US.load(Ordering::Relaxed);
            let ns = us.saturating_mul(1000);
            Ok(ns.to_le_bytes().to_vec())
        }
        WasiSyscall::FdWrite => {
            // args: [fd, iovs_ptr, iovs_len, nwritten_ptr]
            if args.len() < 4 { return Err(WasiError::Inval); }
            let fd = args[0];
            let iovs_ptr = args[1];
            let iovs_len = args[2];
            let _nwritten_ptr = args[3];

            let entry = ctx.fds.get(fd).ok_or(WasiError::Badf)?;
            if entry.fd_type != FdType::Stdout && entry.fd_type != FdType::Stderr
                && entry.fd_type != FdType::RegularFile
            {
                return Err(WasiError::Badf);
            }

            let mut total: u32 = 0;
            for i in 0..iovs_len {
                let iov_offset = iovs_ptr + i * 8;
                let buf_bytes = ctx.memory.read(iov_offset, 4)?;
                let buf_addr = u32::from_le_bytes([buf_bytes[0], buf_bytes[1], buf_bytes[2], buf_bytes[3]]);
                let len_bytes = ctx.memory.read(iov_offset + 4, 4)?;
                let buf_len = u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]);
                let data = ctx.memory.read(buf_addr, buf_len)?;
                // Capture stdout.
                if fd == 1 || fd == 2 {
                    ctx.stdout_buf.extend_from_slice(data);
                }
                total += buf_len;
            }
            Ok(total.to_le_bytes().to_vec())
        }
        WasiSyscall::FdRead => {
            // args: [fd, iovs_ptr, iovs_len, nread_ptr]
            if args.len() < 4 { return Err(WasiError::Inval); }
            let fd = args[0];
            let _entry = ctx.fds.get(fd).ok_or(WasiError::Badf)?;
            // For stdin / files: return 0 bytes read (EOF) for now.
            Ok(0u32.to_le_bytes().to_vec())
        }
        WasiSyscall::FdClose => {
            if args.is_empty() { return Err(WasiError::Inval); }
            ctx.fds.close(args[0])?;
            Ok(Vec::new())
        }
        WasiSyscall::FdSeek => {
            // args: [fd, offset_lo, offset_hi, whence]
            if args.len() < 4 { return Err(WasiError::Inval); }
            let fd = args[0];
            let offset = args[1]; // simplified: ignore hi 32 bits
            let _whence = args[3];
            let entry = ctx.fds.get_mut(fd).ok_or(WasiError::Badf)?;
            entry.offset = offset;
            Ok(offset.to_le_bytes().to_vec())
        }
        WasiSyscall::FdPrestatGet => {
            if args.is_empty() { return Err(WasiError::Inval); }
            let fd = args[0];
            let entry = ctx.fds.get(fd).ok_or(WasiError::Badf)?;
            if entry.fd_type != FdType::PreopenDir {
                return Err(WasiError::Badf);
            }
            // Return (type=0 for dir, name_len).
            let mut out = Vec::with_capacity(8);
            out.extend_from_slice(&0u32.to_le_bytes()); // type: directory
            out.extend_from_slice(&(entry.path.len() as u32).to_le_bytes());
            Ok(out)
        }
        WasiSyscall::PathOpen => {
            // Simplified: args[0]=dirfd, args[1]=path_ptr, args[2]=path_len
            if args.len() < 3 { return Err(WasiError::Inval); }
            let path_ptr = args[1];
            let path_len = args[2];
            let path_bytes = ctx.memory.read(path_ptr, path_len)?;
            let path = core::str::from_utf8(path_bytes).map_err(|_| WasiError::Inval)?;
            let fd = ctx.fds.open_file(path).ok_or(WasiError::Nomem)?;
            Ok(fd.to_le_bytes().to_vec())
        }
        WasiSyscall::PathCreateDirectory => {
            // Simplified: acknowledge but no-op on VFS for now.
            Ok(Vec::new())
        }
        WasiSyscall::PathRemoveDirectory => {
            Ok(Vec::new())
        }
        WasiSyscall::PathUnlinkFile => {
            Ok(Vec::new())
        }
        WasiSyscall::ProcExit => {
            let code = if args.is_empty() { 0 } else { args[0] };
            ctx.exit_code = Some(code);
            Ok(code.to_le_bytes().to_vec())
        }
        WasiSyscall::RandomGet => {
            // args: [buf_ptr, buf_len]
            if args.len() < 2 { return Err(WasiError::Inval); }
            let buf_ptr = args[0];
            let buf_len = args[1];
            let mut buf = vec![0u8; buf_len as usize];
            let mut i = 0;
            while i < buf.len() {
                let r = xorshift32();
                let bytes = r.to_le_bytes();
                for b in &bytes {
                    if i < buf.len() {
                        buf[i] = *b;
                        i += 1;
                    }
                }
            }
            ctx.memory.write(buf_ptr, &buf)?;
            Ok(Vec::new())
        }
    }
}

// ---------------------------------------------------------------------------
// Module loader
// ---------------------------------------------------------------------------

/// Load a WASI module from a raw WASM binary.
///
/// Validates the WASM magic number and version header. The binary is stored
/// for later interpretation. Returns a `WasiModule` with a fresh context.
pub fn load_wasi_module(name: &str, binary: &[u8]) -> Result<WasiModule, &'static str> {
    if binary.len() < 8 {
        return Err("wasi: binary too short for WASM header");
    }
    if binary[0..4] != WASM_MAGIC {
        return Err("wasi: invalid WASM magic number");
    }
    if binary[4..8] != WASM_VERSION {
        return Err("wasi: unsupported WASM version");
    }
    let mut module = WasiModule {
        name: name.to_owned(),
        binary: binary.to_vec(),
        context: WasiContext::new(name),
    };
    module.context.add_arg(name);
    WASI_STATS.modules_loaded.fetch_add(1, Ordering::Relaxed);
    Ok(module)
}

// ---------------------------------------------------------------------------
// Built-in test module
// ---------------------------------------------------------------------------

/// Create a minimal valid WASM binary (header only + empty sections) for testing.
fn test_wasm_binary() -> Vec<u8> {
    let mut bin = Vec::new();
    bin.extend_from_slice(&WASM_MAGIC);
    bin.extend_from_slice(&WASM_VERSION);
    // Empty custom section (id=0, size=4, name="test").
    bin.push(0); // section id
    bin.push(4); // section size (LEB128)
    bin.push(4); // name length
    bin.extend_from_slice(b"test");
    bin
}

/// Run the built-in WASI test module. Returns a summary string.
pub fn run_test_module() -> String {
    let bin = test_wasm_binary();
    let module = match load_wasi_module("__test__", &bin) {
        Ok(m) => m,
        Err(e) => return format!("wasi test: load failed: {}\n", e),
    };
    let mut ctx = module.context;
    ctx.add_arg("--test");
    ctx.add_env("MERLION=1");

    // Simulate a sequence of WASI calls.
    let mut out = String::new();

    // args_sizes_get
    match wasi_call(&mut ctx, WasiSyscall::ArgsSizesGet, &[]) {
        Ok(buf) if buf.len() >= 8 => {
            let argc = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
            out.push_str(&format!("  args_sizes_get: argc={}\n", argc));
        }
        Ok(_) => out.push_str("  args_sizes_get: ok (short)\n"),
        Err(e) => out.push_str(&format!("  args_sizes_get: err={:?}\n", e)),
    }

    // environ_sizes_get
    match wasi_call(&mut ctx, WasiSyscall::EnvironSizesGet, &[]) {
        Ok(buf) if buf.len() >= 8 => {
            let count = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
            out.push_str(&format!("  environ_sizes_get: count={}\n", count));
        }
        Ok(_) => out.push_str("  environ_sizes_get: ok\n"),
        Err(e) => out.push_str(&format!("  environ_sizes_get: err={:?}\n", e)),
    }

    // clock_time_get
    match wasi_call(&mut ctx, WasiSyscall::ClockTimeGet, &[]) {
        Ok(buf) if buf.len() >= 8 => {
            let ns = u64::from_le_bytes([
                buf[0], buf[1], buf[2], buf[3],
                buf[4], buf[5], buf[6], buf[7],
            ]);
            out.push_str(&format!("  clock_time_get: {}ns\n", ns));
        }
        Ok(_) => out.push_str("  clock_time_get: ok\n"),
        Err(e) => out.push_str(&format!("  clock_time_get: err={:?}\n", e)),
    }

    // random_get (4 bytes at memory offset 0)
    match wasi_call(&mut ctx, WasiSyscall::RandomGet, &[0, 4]) {
        Ok(_) => out.push_str("  random_get: ok\n"),
        Err(e) => out.push_str(&format!("  random_get: err={:?}\n", e)),
    }

    // proc_exit
    match wasi_call(&mut ctx, WasiSyscall::ProcExit, &[0]) {
        Ok(_) => {
            out.push_str(&format!("  proc_exit: code={}\n",
                ctx.exit_code.unwrap_or(255)));
        }
        Err(e) => out.push_str(&format!("  proc_exit: err={:?}\n", e)),
    }

    format!("WASI test module results:\n{}", out)
}

// ---------------------------------------------------------------------------
// Global stats
// ---------------------------------------------------------------------------

/// WASI subsystem statistics.
struct WasiStatsInner {
    calls: AtomicU64,
    modules_loaded: AtomicU64,
    errors: AtomicU64,
}

static WASI_STATS: WasiStatsInner = WasiStatsInner {
    calls: AtomicU64::new(0),
    modules_loaded: AtomicU64::new(0),
    errors: AtomicU64::new(0),
};

/// Return information about the WASI subsystem.
pub fn wasi_info() -> String {
    format!(
        "WASI Runtime (MerlionOS)\n\
         \x20 preview1 syscalls: {}\n\
         \x20 max fds/module: {}\n\
         \x20 max memory pages: {} ({} KiB)\n\
         \x20 max modules: {}\n",
        16, // number of supported syscalls
        MAX_FDS,
        MAX_MEMORY_PAGES, MAX_MEMORY_PAGES * WASM_PAGE_SIZE / 1024,
        MAX_MODULES,
    )
}

/// Return runtime statistics for the WASI subsystem.
pub fn wasi_stats() -> String {
    let calls = WASI_STATS.calls.load(Ordering::Relaxed);
    let loaded = WASI_STATS.modules_loaded.load(Ordering::Relaxed);
    let errs = WASI_STATS.errors.load(Ordering::Relaxed);
    format!(
        "WASI Stats\n\
         \x20 syscalls dispatched: {}\n\
         \x20 modules loaded: {}\n\
         \x20 errors: {}\n",
        calls, loaded, errs,
    )
}

/// Initialise the WASI subsystem.
pub fn init() {
    // Reset stats.
    WASI_STATS.calls.store(0, Ordering::Relaxed);
    WASI_STATS.modules_loaded.store(0, Ordering::Relaxed);
    WASI_STATS.errors.store(0, Ordering::Relaxed);
    // Seed PRNG with a non-zero value.
    *PRNG_STATE.lock() = 0xCAFE_BABE;
}
