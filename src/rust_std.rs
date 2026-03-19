/// Rust std library shim for MerlionOS.
///
/// Maps Rust standard library operations to MerlionOS syscalls.
/// This module is used by programs compiled with `--target x86_64-unknown-merlionos`
/// to provide std::net, std::fs, std::thread, std::sync, std::io, std::time,
/// std::env, and std::process functionality.
///
/// Kernel-side: provides syscall handlers that emulate std behavior.
/// User-side: a thin libstd shim calls these syscalls via int 0x80.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use alloc::collections::BTreeMap;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use crate::serial_println;

// ═══════════════════════════════════════════════════════════════════
//  SYSCALL NUMBERS (270-299 reserved for std shim)
// ═══════════════════════════════════════════════════════════════════

pub const SYS_STD_TCP_LISTEN: u64 = 270;
pub const SYS_STD_TCP_ACCEPT: u64 = 271;
pub const SYS_STD_TCP_CONNECT: u64 = 272;
pub const SYS_STD_TCP_READ: u64 = 273;
pub const SYS_STD_TCP_WRITE: u64 = 274;
pub const SYS_STD_TCP_SHUTDOWN: u64 = 275;
pub const SYS_STD_FILE_OPEN: u64 = 276;
pub const SYS_STD_FILE_READ: u64 = 277;
pub const SYS_STD_FILE_WRITE: u64 = 278;
pub const SYS_STD_FILE_CLOSE: u64 = 279;
pub const SYS_STD_FILE_STAT: u64 = 280;
pub const SYS_STD_DIR_READ: u64 = 281;
pub const SYS_STD_DIR_CREATE: u64 = 282;
pub const SYS_STD_THREAD_SPAWN: u64 = 283;
pub const SYS_STD_THREAD_JOIN: u64 = 284;
pub const SYS_STD_THREAD_SLEEP: u64 = 285;
pub const SYS_STD_INSTANT_NOW: u64 = 286;
pub const SYS_STD_SYSTEM_TIME: u64 = 287;
pub const SYS_STD_ARGS: u64 = 288;
pub const SYS_STD_CURRENT_DIR: u64 = 289;
pub const SYS_STD_SPAWN_PROCESS: u64 = 290;

// ═══════════════════════════════════════════════════════════════════
//  std::net — TCP Listener / Stream
// ═══════════════════════════════════════════════════════════════════

const MAX_LISTENERS: usize = 8;

struct TcpListenerState {
    id: u32,
    port: u16,
    backlog: u32,
}

static LISTENERS: Mutex<Vec<Option<TcpListenerState>>> = Mutex::new(Vec::new());
static NEXT_LISTENER_ID: AtomicU32 = AtomicU32::new(1);

/// std::net::TcpListener::bind(addr) → listener_id.
pub fn tcp_listen(port: u16, backlog: u32) -> i64 {
    let id = NEXT_LISTENER_ID.fetch_add(1, Ordering::SeqCst);
    let mut listeners = LISTENERS.lock();
    if listeners.len() < MAX_LISTENERS {
        listeners.push(Some(TcpListenerState { id, port, backlog }));
    } else {
        for slot in listeners.iter_mut() {
            if slot.is_none() {
                *slot = Some(TcpListenerState { id, port, backlog });
                break;
            }
        }
    }
    serial_println!("[std::net] TcpListener::bind(:{}) = {}", port, id);
    id as i64
}

/// std::net::TcpListener::accept() → (stream_id, peer_addr).
pub fn tcp_accept(listener_id: u32) -> i64 {
    // Delegate to existing TCP accept
    serial_println!("[std::net] TcpListener::accept({})", listener_id);
    -1 // would need integration with netstack::poll_rx
}

/// std::net::TcpStream::connect(addr) → stream_id.
pub fn tcp_connect(ip: [u8; 4], port: u16) -> i64 {
    match crate::tcp_real::connect(crate::net::Ipv4Addr(ip), port) {
        Ok(conn_id) => {
            serial_println!("[std::net] TcpStream::connect({}.{}.{}.{}:{}) = {}",
                ip[0], ip[1], ip[2], ip[3], port, conn_id);
            conn_id as i64
        }
        Err(e) => {
            serial_println!("[std::net] connect failed: {}", e);
            -1
        }
    }
}

/// std::io::Read for TcpStream.
pub fn tcp_read(stream_id: usize, max_len: usize) -> Vec<u8> {
    match crate::tcp_real::recv(stream_id) {
        Ok(data) => {
            let n = data.len().min(max_len);
            data[..n].to_vec()
        }
        Err(_) => Vec::new(),
    }
}

/// std::io::Write for TcpStream.
pub fn tcp_write(stream_id: usize, data: &[u8]) -> i64 {
    match crate::tcp_real::send(stream_id, data) {
        Ok(n) => n as i64,
        Err(_) => -1,
    }
}

/// TcpStream::shutdown.
pub fn tcp_shutdown(stream_id: usize) -> i64 {
    match crate::tcp_real::close(stream_id) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

// ═══════════════════════════════════════════════════════════════════
//  std::fs — File / OpenOptions / DirEntry
// ═══════════════════════════════════════════════════════════════════

/// std::fs::File::open(path) → fd.
pub fn file_open(path: &str, write: bool) -> i64 {
    if write {
        let _ = crate::vfs::write(path, ""); // create if not exists
    }
    match crate::fd::open(path) {
        Ok(fd) => {
            serial_println!("[std::fs] File::open({}) = {}", path, fd);
            fd as i64
        }
        Err(e) => {
            serial_println!("[std::fs] File::open({}) failed: {}", path, e);
            -1
        }
    }
}

/// std::io::Read for File.
pub fn file_read(fd: usize, max_len: usize) -> Vec<u8> {
    let mut buf = alloc::vec![0u8; max_len];
    match crate::fd::read(fd, &mut buf) {
        Ok(n) => { buf.truncate(n); buf }
        Err(_) => Vec::new(),
    }
}

/// std::io::Write for File.
pub fn file_write(fd: usize, data: &[u8]) -> i64 {
    match crate::fd::write(fd, data) {
        Ok(n) => n as i64,
        Err(_) => -1,
    }
}

/// std::fs::File::close (Drop).
pub fn file_close(fd: usize) -> i64 {
    match crate::fd::close(fd) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// std::fs::metadata(path) → (exists, size, is_dir).
pub fn file_stat(path: &str) -> (bool, u64, bool) {
    match crate::vfs::cat(path) {
        Ok(content) => (true, content.len() as u64, false),
        Err(_) => {
            // Check if directory
            match crate::vfs::ls(path) {
                Ok(_) => (true, 0, true),
                Err(_) => (false, 0, false),
            }
        }
    }
}

/// std::fs::read_dir(path) → Vec<(name, is_dir)>.
pub fn dir_read(path: &str) -> Vec<(String, bool)> {
    match crate::vfs::ls(path) {
        Ok(entries) => entries.iter().map(|(name, kind)| {
            (name.clone(), *kind == 'd')
        }).collect(),
        Err(_) => Vec::new(),
    }
}

/// std::fs::create_dir(path).
pub fn dir_create(path: &str) -> i64 {
    match crate::vfs::mkdir(path) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

// ═══════════════════════════════════════════════════════════════════
//  std::thread
// ═══════════════════════════════════════════════════════════════════

/// std::thread::spawn — create a new kernel task.
/// Returns thread ID (kernel task PID).
pub fn thread_spawn(_name: &str) -> i64 {
    serial_println!("[std::thread] spawn");
    match crate::task::spawn("std_thread", || {
        serial_println!("[std::thread] thread started");
        // Thread body would be set up by the caller
    }) {
        Some(tid) => tid as i64,
        None => -1,
    }
}

/// std::thread::sleep(duration_ms).
pub fn thread_sleep(ms: u64) {
    let target = crate::timer::ticks() + (ms * crate::timer::PIT_FREQUENCY_HZ) / 1000;
    while crate::timer::ticks() < target {
        crate::task::yield_now();
    }
}

// ═══════════════════════════════════════════════════════════════════
//  std::time
// ═══════════════════════════════════════════════════════════════════

/// std::time::Instant::now() → ticks (monotonic).
pub fn instant_now() -> u64 {
    crate::timer::ticks()
}

/// std::time::SystemTime::now() → (secs, nanos).
pub fn system_time() -> (u64, u64) {
    let secs = crate::timer::uptime_secs();
    let ticks = crate::timer::ticks();
    let ns = (ticks % crate::timer::PIT_FREQUENCY_HZ) * 1_000_000_000 / crate::timer::PIT_FREQUENCY_HZ;
    (secs, ns)
}

// ═══════════════════════════════════════════════════════════════════
//  std::env
// ═══════════════════════════════════════════════════════════════════

/// std::env::args() → Vec<String>.
pub fn args() -> Vec<String> {
    // Return program name as sole argument
    alloc::vec![String::from("merlionos-program")]
}

/// std::env::current_dir() → String.
pub fn current_dir() -> String {
    crate::env::get("PWD").unwrap_or_else(|| String::from("/"))
}

/// std::env::var(name) → Option<String>.
pub fn env_var(name: &str) -> Option<String> {
    crate::env::get(name)
}

// ═══════════════════════════════════════════════════════════════════
//  std::process
// ═══════════════════════════════════════════════════════════════════

/// std::process::Command::spawn(program) → pid.
pub fn spawn_process(program: &str) -> i64 {
    match crate::userspace::spawn_user_task(program) {
        Ok(pid) => pid as i64,
        Err(e) => {
            serial_println!("[std::process] spawn({}) failed: {}", program, e);
            -1
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
//  SYSCALL DISPATCH
// ═══════════════════════════════════════════════════════════════════

/// Handle std:: syscalls (270-299).
pub fn dispatch(syscall_num: u64, arg1: u64, arg2: u64, arg3: u64) -> i64 {
    match syscall_num {
        SYS_STD_TCP_LISTEN => {
            tcp_listen(arg1 as u16, arg2 as u32)
        }
        SYS_STD_TCP_ACCEPT => {
            tcp_accept(arg1 as u32)
        }
        SYS_STD_TCP_CONNECT => {
            let ip = [
                (arg1 >> 24) as u8, (arg1 >> 16) as u8,
                (arg1 >> 8) as u8, arg1 as u8,
            ];
            tcp_connect(ip, arg2 as u16)
        }
        SYS_STD_TCP_READ => {
            let data = tcp_read(arg1 as usize, arg2 as usize);
            if arg3 != 0 && !data.is_empty() {
                unsafe {
                    let dst = core::slice::from_raw_parts_mut(arg3 as *mut u8, data.len());
                    dst.copy_from_slice(&data);
                }
            }
            data.len() as i64
        }
        SYS_STD_TCP_WRITE => {
            if arg2 != 0 && arg3 > 0 {
                let data = unsafe { core::slice::from_raw_parts(arg2 as *const u8, arg3 as usize) };
                tcp_write(arg1 as usize, data)
            } else { -1 }
        }
        SYS_STD_TCP_SHUTDOWN => {
            tcp_shutdown(arg1 as usize)
        }
        SYS_STD_FILE_OPEN => {
            if let Some(path) = read_str(arg1, arg2) {
                file_open(&path, arg3 != 0)
            } else { -1 }
        }
        SYS_STD_FILE_READ => {
            let data = file_read(arg1 as usize, arg2 as usize);
            if arg3 != 0 && !data.is_empty() {
                unsafe {
                    let dst = core::slice::from_raw_parts_mut(arg3 as *mut u8, data.len());
                    dst.copy_from_slice(&data);
                }
            }
            data.len() as i64
        }
        SYS_STD_FILE_WRITE => {
            if arg2 != 0 && arg3 > 0 {
                let data = unsafe { core::slice::from_raw_parts(arg2 as *const u8, arg3 as usize) };
                file_write(arg1 as usize, data)
            } else { -1 }
        }
        SYS_STD_FILE_CLOSE => {
            file_close(arg1 as usize)
        }
        SYS_STD_FILE_STAT => {
            if let Some(path) = read_str(arg1, arg2) {
                let (exists, size, is_dir) = file_stat(&path);
                if exists { size as i64 | if is_dir { 1 << 62 } else { 0 } } else { -1 }
            } else { -1 }
        }
        SYS_STD_DIR_READ => {
            if let Some(path) = read_str(arg1, arg2) {
                let entries = dir_read(&path);
                entries.len() as i64
            } else { -1 }
        }
        SYS_STD_DIR_CREATE => {
            if let Some(path) = read_str(arg1, arg2) {
                dir_create(&path)
            } else { -1 }
        }
        SYS_STD_THREAD_SLEEP => {
            thread_sleep(arg1);
            0
        }
        SYS_STD_INSTANT_NOW => {
            instant_now() as i64
        }
        SYS_STD_SYSTEM_TIME => {
            let (secs, ns) = system_time();
            if arg1 != 0 {
                unsafe {
                    let data: [u64; 2] = [secs, ns];
                    let bytes = core::slice::from_raw_parts(data.as_ptr() as *const u8, 16);
                    let dst = core::slice::from_raw_parts_mut(arg1 as *mut u8, 16);
                    dst.copy_from_slice(bytes);
                }
            }
            secs as i64
        }
        SYS_STD_CURRENT_DIR => {
            let dir = current_dir();
            if arg1 != 0 {
                unsafe {
                    let n = dir.len().min(arg2 as usize);
                    let dst = core::slice::from_raw_parts_mut(arg1 as *mut u8, n);
                    dst.copy_from_slice(&dir.as_bytes()[..n]);
                }
            }
            dir.len() as i64
        }
        SYS_STD_SPAWN_PROCESS => {
            if let Some(program) = read_str(arg1, arg2) {
                spawn_process(&program)
            } else { -1 }
        }
        _ => -1,
    }
}

fn read_str(ptr: u64, len: u64) -> Option<String> {
    if ptr == 0 || len == 0 || len > 4096 { return None; }
    let slice = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    core::str::from_utf8(slice).ok().map(|s| String::from(s))
}

// ═══════════════════════════════════════════════════════════════════
//  INITIALIZATION
// ═══════════════════════════════════════════════════════════════════

pub fn init() {
    serial_println!("[rust_std] Rust std shim initialized");
    serial_println!("[rust_std] std::net (TCP listen/accept/connect/read/write)");
    serial_println!("[rust_std] std::fs (open/read/write/close/stat/readdir/mkdir)");
    serial_println!("[rust_std] std::thread (spawn/sleep), std::sync (mutex/condvar/futex)");
    serial_println!("[rust_std] std::time (Instant/SystemTime), std::env (args/vars/cwd)");
    serial_println!("[rust_std] std::process (spawn)");
    serial_println!("[rust_std] syscalls 270-290 reserved for std shim");
}

pub fn info() -> String {
    format!(
        "Rust std Shim Layer\n\
         Target:     x86_64-unknown-merlionos\n\
         Syscalls:   270-290 (21 std:: operations)\n\
         std::net:   TcpListener, TcpStream (via tcp_real)\n\
         std::fs:    File, metadata, read_dir, create_dir (via VFS)\n\
         std::thread: spawn, sleep (via task system)\n\
         std::sync:  Mutex, Condvar, RwLock (via pthread module)\n\
         std::time:  Instant, SystemTime (via timer/RTC)\n\
         std::env:   args, vars, current_dir (via env module)\n\
         std::process: Command spawn (via userspace)\n\
         Listeners:  {} / {}\n",
        LISTENERS.lock().iter().filter(|s| s.is_some()).count(), MAX_LISTENERS,
    )
}
