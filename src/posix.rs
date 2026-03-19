/// Extended POSIX compatibility layer for MerlionOS.
///
/// Implements musl libc core functions as kernel-side syscall handlers.
/// User programs call these via int 0x80 with the corresponding syscall number.
/// This enables C programs compiled against musl to run on MerlionOS.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use alloc::collections::BTreeMap;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::serial_println;

// ═══════════════════════════════════════════════════════════════════
//  STDIO: fopen/fclose/fread/fwrite/fprintf/fseek/ftell
// ═══════════════════════════════════════════════════════════════════

const MAX_FILE_STREAMS: usize = 32;

struct FileStream {
    id: u32,
    path: String,
    offset: usize,
    mode: u8, // 0=read, 1=write, 2=append
}

static STREAMS: Mutex<Vec<Option<FileStream>>> = Mutex::new(Vec::new());
static NEXT_STREAM_ID: AtomicU64 = AtomicU64::new(100); // start above fd range

/// fopen(path, mode) → stream_id (0 on failure).
pub fn fopen(path: &str, mode: &str) -> u64 {
    let m = match mode {
        "r" | "rb" => 0,
        "w" | "wb" => 1,
        "a" | "ab" => 2,
        "r+" | "rb+" => 0,
        "w+" | "wb+" => 1,
        _ => 0,
    };
    // Create file if writing
    if m == 1 {
        let _ = crate::vfs::write(path, "");
    }
    let id = NEXT_STREAM_ID.fetch_add(1, Ordering::SeqCst);
    let mut streams = STREAMS.lock();
    if streams.len() < MAX_FILE_STREAMS {
        streams.push(Some(FileStream {
            id: id as u32, path: String::from(path), offset: 0, mode: m,
        }));
    } else {
        for slot in streams.iter_mut() {
            if slot.is_none() {
                *slot = Some(FileStream {
                    id: id as u32, path: String::from(path), offset: 0, mode: m,
                });
                break;
            }
        }
    }
    serial_println!("[posix] fopen({}, {}) = {}", path, mode, id);
    id
}

/// fclose(stream_id) → 0 or -1.
pub fn fclose(stream_id: u64) -> i32 {
    let mut streams = STREAMS.lock();
    for slot in streams.iter_mut() {
        if let Some(s) = slot {
            if s.id == stream_id as u32 {
                *slot = None;
                return 0;
            }
        }
    }
    -1
}

/// fread(stream_id, buf, max_len) → bytes_read.
pub fn fread(stream_id: u64, max_len: usize) -> Vec<u8> {
    let streams = STREAMS.lock();
    let stream = match streams.iter().flat_map(|s| s.as_ref()).find(|s| s.id == stream_id as u32) {
        Some(s) => s,
        None => return Vec::new(),
    };
    match crate::vfs::cat(&stream.path) {
        Ok(content) => {
            let bytes = content.as_bytes();
            let start = stream.offset.min(bytes.len());
            let end = (start + max_len).min(bytes.len());
            bytes[start..end].to_vec()
        }
        Err(_) => Vec::new(),
    }
}

/// fwrite(stream_id, data) → bytes_written.
pub fn fwrite(stream_id: u64, data: &[u8]) -> usize {
    let streams = STREAMS.lock();
    let stream = match streams.iter().flat_map(|s| s.as_ref()).find(|s| s.id == stream_id as u32) {
        Some(s) => s,
        None => return 0,
    };
    if let Ok(s) = core::str::from_utf8(data) {
        let _ = crate::vfs::write(&stream.path, s);
        data.len()
    } else {
        0
    }
}

/// fseek(stream_id, offset, whence) → 0 or -1.
pub fn fseek(stream_id: u64, offset: i64, whence: u32) -> i32 {
    let mut streams = STREAMS.lock();
    for slot in streams.iter_mut() {
        if let Some(s) = slot {
            if s.id == stream_id as u32 {
                match whence {
                    0 => s.offset = offset.max(0) as usize, // SEEK_SET
                    1 => s.offset = (s.offset as i64 + offset).max(0) as usize, // SEEK_CUR
                    2 => { // SEEK_END
                        if let Ok(content) = crate::vfs::cat(&s.path) {
                            s.offset = (content.len() as i64 + offset).max(0) as usize;
                        }
                    }
                    _ => return -1,
                }
                return 0;
            }
        }
    }
    -1
}

/// ftell(stream_id) → offset or -1.
pub fn ftell(stream_id: u64) -> i64 {
    let streams = STREAMS.lock();
    for slot in streams.iter() {
        if let Some(s) = slot {
            if s.id == stream_id as u32 {
                return s.offset as i64;
            }
        }
    }
    -1
}

// ═══════════════════════════════════════════════════════════════════
//  MEMORY: calloc, realloc
// ═══════════════════════════════════════════════════════════════════

/// calloc(count, size) → ptr (zero-initialized).
/// Uses brk-based allocation (delegates to userspace malloc + memset).
pub fn calloc_size(count: usize, size: usize) -> usize {
    count * size // return total size; kernel allocates via brk
}

// ═══════════════════════════════════════════════════════════════════
//  TIME: gettimeofday, clock_gettime(MONOTONIC)
// ═══════════════════════════════════════════════════════════════════

/// gettimeofday() → (seconds, microseconds).
pub fn gettimeofday() -> (u64, u64) {
    let secs = crate::timer::uptime_secs();
    let ticks = crate::timer::ticks();
    let us = (ticks % crate::timer::PIT_FREQUENCY_HZ) * 1_000_000 / crate::timer::PIT_FREQUENCY_HZ;
    (secs, us)
}

/// clock_gettime(CLOCK_MONOTONIC) → (seconds, nanoseconds).
pub fn clock_gettime_monotonic() -> (u64, u64) {
    let secs = crate::timer::uptime_secs();
    let ticks = crate::timer::ticks();
    let ns = (ticks % crate::timer::PIT_FREQUENCY_HZ) * 1_000_000_000 / crate::timer::PIT_FREQUENCY_HZ;
    (secs, ns)
}

// ═══════════════════════════════════════════════════════════════════
//  NETWORK: getaddrinfo, inet_ntop, inet_pton, htons/ntohs
// ═══════════════════════════════════════════════════════════════════

/// Simplified getaddrinfo: resolve hostname to IP.
pub fn getaddrinfo(hostname: &str) -> Option<[u8; 4]> {
    // Try DNS resolution
    match crate::dns_client::resolve(hostname) {
        Ok(ip) => Some(ip),
        Err(_) => {
            // Try parsing as dotted quad
            let parts: Vec<&str> = hostname.split('.').collect();
            if parts.len() == 4 {
                let a = parts[0].parse::<u8>().ok()?;
                let b = parts[1].parse::<u8>().ok()?;
                let c = parts[2].parse::<u8>().ok()?;
                let d = parts[3].parse::<u8>().ok()?;
                Some([a, b, c, d])
            } else {
                None
            }
        }
    }
}

/// inet_ntop: IP bytes to dotted string.
pub fn inet_ntop(ip: &[u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

/// inet_pton: dotted string to IP bytes.
pub fn inet_pton(s: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 { return None; }
    Some([
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
        parts[3].parse().ok()?,
    ])
}

// ═══════════════════════════════════════════════════════════════════
//  THREAD: pthread_create (via SYS_CLONE)
// ═══════════════════════════════════════════════════════════════════

/// Thread-local storage keys.
static TLS_DATA: Mutex<BTreeMap<(u32, u32), u64>> = Mutex::new(BTreeMap::new()); // (tid, key) → value
static NEXT_TLS_KEY: AtomicU64 = AtomicU64::new(1);

/// pthread_key_create → key.
pub fn tls_key_create() -> u32 {
    NEXT_TLS_KEY.fetch_add(1, Ordering::SeqCst) as u32
}

/// pthread_setspecific(key, value).
pub fn tls_set(key: u32, value: u64) {
    let tid = crate::task::current_pid() as u32;
    TLS_DATA.lock().insert((tid, key), value);
}

/// pthread_getspecific(key) → value.
pub fn tls_get(key: u32) -> u64 {
    let tid = crate::task::current_pid() as u32;
    *TLS_DATA.lock().get(&(tid, key)).unwrap_or(&0)
}

// ═══════════════════════════════════════════════════════════════════
//  ERRNO (thread-local)
// ═══════════════════════════════════════════════════════════════════

static ERRNO_MAP: Mutex<BTreeMap<u32, i32>> = Mutex::new(BTreeMap::new());

pub fn set_errno(val: i32) {
    let tid = crate::task::current_pid() as u32;
    ERRNO_MAP.lock().insert(tid, val);
}

pub fn get_errno() -> i32 {
    let tid = crate::task::current_pid() as u32;
    *ERRNO_MAP.lock().get(&tid).unwrap_or(&0)
}

// ═══════════════════════════════════════════════════════════════════
//  ENVIRONMENT
// ═══════════════════════════════════════════════════════════════════

/// getenv(name) → value.
pub fn getenv(name: &str) -> Option<String> {
    crate::env::get(name)
}

/// setenv(name, value).
pub fn setenv(name: &str, value: &str) {
    crate::env::set(name, value);
}

// ═══════════════════════════════════════════════════════════════════
//  EXTENDED SOCKET API (E2)
// ═══════════════════════════════════════════════════════════════════

/// accept4: accept with flags (SOCK_NONBLOCK, SOCK_CLOEXEC).
pub fn accept4(listen_fd: usize, flags: u32) -> i32 {
    // Delegate to TCP accept
    // For now, just accept without flags
    serial_println!("[posix] accept4(fd={}, flags={:#x})", listen_fd, flags);
    -1 // would need real TCP listener state
}

/// shutdown(fd, how): 0=SHUT_RD, 1=SHUT_WR, 2=SHUT_RDWR.
pub fn shutdown(fd: usize, how: u32) -> i32 {
    serial_println!("[posix] shutdown(fd={}, how={})", fd, how);
    match how {
        2 => {
            let _ = crate::tcp_real::close(fd);
            0
        }
        _ => 0,
    }
}

/// sendmsg: scatter-gather send (simplified — concatenate iovecs).
pub fn sendmsg(fd: usize, data: &[u8]) -> i64 {
    match crate::tcp_real::send(fd, data) {
        Ok(_) => data.len() as i64,
        Err(_) => -1,
    }
}

/// recvmsg: scatter-gather receive (simplified).
pub fn recvmsg(fd: usize, max_len: usize) -> Vec<u8> {
    match crate::tcp_real::recv(fd) {
        Ok(data) => {
            if data.len() > max_len { data[..max_len].to_vec() }
            else { data }
        }
        Err(_) => Vec::new(),
    }
}

/// poll: wait for events on multiple fds.
pub fn poll(fds: &[(u32, u16)], timeout_ms: i32) -> Vec<(u32, u16)> {
    // Simplified: check each fd for readiness
    let mut ready = Vec::new();
    for &(fd, events) in fds {
        let mut revents: u16 = 0;
        if events & 1 != 0 { revents |= 1; } // POLLIN → always ready (simplified)
        if events & 4 != 0 { revents |= 4; } // POLLOUT → always ready
        if revents != 0 {
            ready.push((fd, revents));
        }
    }
    if ready.is_empty() && timeout_ms > 0 {
        crate::task::yield_now();
    }
    ready
}

// ═══════════════════════════════════════════════════════════════════
//  EVENTFD + TIMERFD (E3)
// ═══════════════════════════════════════════════════════════════════

const MAX_EVENTFDS: usize = 16;
const MAX_TIMERFDS: usize = 16;

struct EventFd {
    id: u32,
    counter: AtomicU64,
    semaphore: bool, // EFD_SEMAPHORE
}

struct TimerFd {
    id: u32,
    interval_ms: u64,    // repeat interval (0 = one-shot)
    next_fire: AtomicU64, // tick when next fires
    armed: bool,
}

static EVENTFDS: Mutex<Vec<Option<EventFd>>> = Mutex::new(Vec::new());
static TIMERFDS: Mutex<Vec<Option<TimerFd>>> = Mutex::new(Vec::new());
static NEXT_SPECIAL_FD: AtomicU64 = AtomicU64::new(500);

/// eventfd(initval, flags) → fd.
pub fn eventfd_create(initval: u64, semaphore: bool) -> u64 {
    let id = NEXT_SPECIAL_FD.fetch_add(1, Ordering::SeqCst);
    let mut fds = EVENTFDS.lock();
    if fds.len() < MAX_EVENTFDS {
        fds.push(Some(EventFd {
            id: id as u32,
            counter: AtomicU64::new(initval),
            semaphore,
        }));
    }
    serial_println!("[posix] eventfd({}, sem={}) = {}", initval, semaphore, id);
    id
}

/// eventfd_read(fd) → counter value (blocks if zero in blocking mode).
pub fn eventfd_read(fd: u64) -> u64 {
    let fds = EVENTFDS.lock();
    if let Some(efd) = fds.iter().flat_map(|s| s.as_ref()).find(|e| e.id == fd as u32) {
        if efd.semaphore {
            // Decrement by 1
            let val = efd.counter.load(Ordering::SeqCst);
            if val > 0 {
                efd.counter.fetch_sub(1, Ordering::SeqCst);
                return 1;
            }
            return 0;
        } else {
            // Read and reset
            let val = efd.counter.swap(0, Ordering::SeqCst);
            return val;
        }
    }
    0
}

/// eventfd_write(fd, value) → adds to counter.
pub fn eventfd_write(fd: u64, value: u64) -> i32 {
    let fds = EVENTFDS.lock();
    if let Some(efd) = fds.iter().flat_map(|s| s.as_ref()).find(|e| e.id == fd as u32) {
        efd.counter.fetch_add(value, Ordering::SeqCst);
        return 0;
    }
    -1
}

/// timerfd_create() → fd.
pub fn timerfd_create() -> u64 {
    let id = NEXT_SPECIAL_FD.fetch_add(1, Ordering::SeqCst);
    let mut fds = TIMERFDS.lock();
    if fds.len() < MAX_TIMERFDS {
        fds.push(Some(TimerFd {
            id: id as u32,
            interval_ms: 0,
            next_fire: AtomicU64::new(0),
            armed: false,
        }));
    }
    serial_println!("[posix] timerfd_create() = {}", id);
    id
}

/// timerfd_settime(fd, initial_ms, interval_ms) → 0.
pub fn timerfd_settime(fd: u64, initial_ms: u64, interval_ms: u64) -> i32 {
    let mut fds = TIMERFDS.lock();
    for slot in fds.iter_mut() {
        if let Some(tfd) = slot {
            if tfd.id == fd as u32 {
                tfd.interval_ms = interval_ms;
                let fire_tick = crate::timer::ticks() +
                    (initial_ms * crate::timer::PIT_FREQUENCY_HZ) / 1000;
                tfd.next_fire.store(fire_tick, Ordering::SeqCst);
                tfd.armed = true;
                return 0;
            }
        }
    }
    -1
}

/// timerfd_read(fd) → number of expirations (0 if not expired).
pub fn timerfd_read(fd: u64) -> u64 {
    let fds = TIMERFDS.lock();
    if let Some(tfd) = fds.iter().flat_map(|s| s.as_ref()).find(|t| t.id == fd as u32) {
        if !tfd.armed { return 0; }
        let now = crate::timer::ticks();
        let fire = tfd.next_fire.load(Ordering::SeqCst);
        if now >= fire {
            if tfd.interval_ms > 0 {
                // Repeating timer — compute expirations
                let elapsed = now - fire;
                let interval_ticks = (tfd.interval_ms * crate::timer::PIT_FREQUENCY_HZ) / 1000;
                let expirations = if interval_ticks > 0 { elapsed / interval_ticks + 1 } else { 1 };
                tfd.next_fire.store(now + interval_ticks, Ordering::SeqCst);
                return expirations;
            } else {
                return 1; // one-shot
            }
        }
    }
    0
}

// ═══════════════════════════════════════════════════════════════════
//  RANDOM
// ═══════════════════════════════════════════════════════════════════

/// getrandom(buf, len) → bytes filled.
pub fn getrandom(buf: &mut [u8]) -> usize {
    crate::crypto::random_bytes(buf);
    buf.len()
}

// ═══════════════════════════════════════════════════════════════════
//  STRING HELPERS
// ═══════════════════════════════════════════════════════════════════

/// snprintf-style formatting (simplified: %s, %d, %x, %u, %p).
pub fn snprintf(fmt: &str, args: &[u64]) -> String {
    let mut out = String::new();
    let bytes = fmt.as_bytes();
    let mut i = 0;
    let mut arg_idx = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 1 < bytes.len() {
            i += 1;
            match bytes[i] {
                b'd' | b'i' => {
                    let v = if arg_idx < args.len() { args[arg_idx] as i64 } else { 0 };
                    arg_idx += 1;
                    out.push_str(&format!("{}", v));
                }
                b'u' => {
                    let v = if arg_idx < args.len() { args[arg_idx] } else { 0 };
                    arg_idx += 1;
                    out.push_str(&format!("{}", v));
                }
                b'x' => {
                    let v = if arg_idx < args.len() { args[arg_idx] } else { 0 };
                    arg_idx += 1;
                    out.push_str(&format!("{:x}", v));
                }
                b'p' => {
                    let v = if arg_idx < args.len() { args[arg_idx] } else { 0 };
                    arg_idx += 1;
                    out.push_str(&format!("0x{:x}", v));
                }
                b's' => {
                    if arg_idx < args.len() {
                        let ptr = args[arg_idx] as *const u8;
                        arg_idx += 1;
                        if !ptr.is_null() {
                            let mut len = 0;
                            unsafe { while len < 256 && *ptr.add(len) != 0 { len += 1; } }
                            if let Ok(s) = core::str::from_utf8(unsafe { core::slice::from_raw_parts(ptr, len) }) {
                                out.push_str(s);
                            }
                        }
                    }
                }
                b'%' => out.push('%'),
                b'l' => {
                    // Skip 'l' prefix (ld, lu, lx)
                    if i + 1 < bytes.len() {
                        i += 1; // consume the next char
                        let v = if arg_idx < args.len() { args[arg_idx] } else { 0 };
                        arg_idx += 1;
                        match bytes[i] {
                            b'd' => out.push_str(&format!("{}", v as i64)),
                            b'u' => out.push_str(&format!("{}", v)),
                            b'x' => out.push_str(&format!("{:x}", v)),
                            _ => {}
                        }
                    }
                }
                _ => { out.push('%'); out.push(bytes[i] as char); }
            }
        } else {
            out.push(bytes[i] as char);
        }
        i += 1;
    }
    out
}

// ═══════════════════════════════════════════════════════════════════
//  INITIALIZATION & INFO
// ═══════════════════════════════════════════════════════════════════

pub fn init() {
    serial_println!("[posix] extended POSIX layer initialized");
    serial_println!("[posix] stdio: fopen/fclose/fread/fwrite/fseek/ftell");
    serial_println!("[posix] net: getaddrinfo/inet_ntop/inet_pton/sendmsg/recvmsg/poll/accept4/shutdown");
    serial_println!("[posix] event: eventfd/timerfd (create/read/write/settime)");
    serial_println!("[posix] misc: getrandom/getenv/setenv/snprintf/tls/errno");
}

pub fn info() -> String {
    let streams = STREAMS.lock().iter().filter(|s| s.is_some()).count();
    let eventfds = EVENTFDS.lock().iter().filter(|s| s.is_some()).count();
    let timerfds = TIMERFDS.lock().iter().filter(|s| s.is_some()).count();
    format!(
        "POSIX Compatibility Layer:\n\
         File streams: {} / {}\n\
         Event FDs:    {} / {}\n\
         Timer FDs:    {} / {}\n\
         TLS keys:     {}\n",
        streams, MAX_FILE_STREAMS,
        eventfds, MAX_EVENTFDS,
        timerfds, MAX_TIMERFDS,
        NEXT_TLS_KEY.load(Ordering::Relaxed) - 1,
    )
}
