/// Minimal libc implementation for MerlionOS userspace.
/// Provides string functions, memory operations, stdio, and stdlib
/// equivalents for user programs running in the kernel.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// errno simulation
// ---------------------------------------------------------------------------

static ERRNO: AtomicI32 = AtomicI32::new(0);

pub const ENOENT: i32 = 2;
pub const ENOMEM: i32 = 12;
pub const EACCES: i32 = 13;
pub const EINVAL: i32 = 22;

/// Set the global errno value.
pub fn set_errno(e: i32) {
    ERRNO.store(e, Ordering::Relaxed);
}

/// Get the current errno value.
pub fn get_errno() -> i32 {
    ERRNO.load(Ordering::Relaxed)
}

/// Return a human-readable description of an errno code.
pub fn strerror(e: i32) -> &'static str {
    match e {
        0      => "success",
        ENOENT => "no such file or directory",
        ENOMEM => "out of memory",
        EACCES => "permission denied",
        EINVAL => "invalid argument",
        _      => "unknown error",
    }
}

// ---------------------------------------------------------------------------
// String functions
// ---------------------------------------------------------------------------

/// Return the length of a NUL-terminated byte slice (not counting the NUL).
/// If there is no NUL, returns the slice length.
pub fn strlen(s: &[u8]) -> usize {
    s.iter().position(|&b| b == 0).unwrap_or(s.len())
}

/// Compare two byte slices lexicographically (C-style).
/// Returns negative, zero, or positive.
pub fn strcmp(a: &[u8], b: &[u8]) -> i32 {
    let len = if a.len() < b.len() { a.len() } else { b.len() };
    for i in 0..len {
        if a[i] != b[i] {
            return (a[i] as i32) - (b[i] as i32);
        }
        // Stop at NUL for C-string semantics.
        if a[i] == 0 {
            return 0;
        }
    }
    (a.len() as i32) - (b.len() as i32)
}

/// Compare at most `n` bytes of two slices lexicographically.
pub fn strncmp(a: &[u8], b: &[u8], n: usize) -> i32 {
    let la = if a.len() < n { a.len() } else { n };
    let lb = if b.len() < n { b.len() } else { n };
    let len = if la < lb { la } else { lb };
    for i in 0..len {
        if a[i] != b[i] {
            return (a[i] as i32) - (b[i] as i32);
        }
        if a[i] == 0 {
            return 0;
        }
    }
    (la as i32) - (lb as i32)
}

/// Return the index of the first occurrence of byte `c` in `s`, or `None`.
pub fn strchr(s: &[u8], c: u8) -> Option<usize> {
    s.iter().position(|&b| b == c)
}

/// Return the index where `needle` first occurs in `haystack`, or `None`.
pub fn strstr(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    for i in 0..=(haystack.len() - needle.len()) {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
    }
    None
}

/// Duplicate a string (equivalent to C strdup).
pub fn strdup(s: &str) -> String {
    String::from(s)
}

/// Parse a string as a signed 64-bit integer with the given radix.
pub fn strtol(s: &str, base: u32) -> Result<i64, &'static str> {
    if s.is_empty() {
        return Err("empty string");
    }
    let (negative, digits) = if s.starts_with('-') {
        (true, &s[1..])
    } else if s.starts_with('+') {
        (false, &s[1..])
    } else {
        (false, s)
    };
    if digits.is_empty() {
        return Err("no digits");
    }
    let mut result: i64 = 0;
    for c in digits.bytes() {
        let digit = match c {
            b'0'..=b'9' => (c - b'0') as u32,
            b'a'..=b'f' => (c - b'a' + 10) as u32,
            b'A'..=b'F' => (c - b'A' + 10) as u32,
            _ => return Err("invalid character"),
        };
        if digit >= base {
            return Err("digit out of range for base");
        }
        result = result.wrapping_mul(base as i64).wrapping_add(digit as i64);
    }
    if negative {
        result = -result;
    }
    Ok(result)
}

/// Parse a decimal string to i32 (like C atoi).
pub fn atoi(s: &str) -> i32 {
    strtol(s, 10).unwrap_or(0) as i32
}

// ---------------------------------------------------------------------------
// Memory functions
// ---------------------------------------------------------------------------

/// Fill a buffer with a repeated byte value.
pub fn memset(buf: &mut [u8], val: u8) {
    for b in buf.iter_mut() {
        *b = val;
    }
}

/// Copy bytes from `src` to `dst`.  The slices must not overlap; use
/// `memmove` if overlap is possible.  Copies `min(dst.len(), src.len())`
/// bytes.
pub fn memcpy(dst: &mut [u8], src: &[u8]) {
    let len = if dst.len() < src.len() { dst.len() } else { src.len() };
    for i in 0..len {
        dst[i] = src[i];
    }
}

/// Compare two byte slices.  Returns negative, zero, or positive.
pub fn memcmp(a: &[u8], b: &[u8]) -> i32 {
    let len = if a.len() < b.len() { a.len() } else { b.len() };
    for i in 0..len {
        if a[i] != b[i] {
            return (a[i] as i32) - (b[i] as i32);
        }
    }
    (a.len() as i32) - (b.len() as i32)
}

/// Copy bytes from `src` to `dst`, correctly handling overlapping regions.
/// Uses an intermediate buffer.
pub fn memmove(dst: &mut [u8], src: &[u8]) {
    let len = if dst.len() < src.len() { dst.len() } else { src.len() };
    let mut tmp: Vec<u8> = Vec::with_capacity(len);
    for i in 0..len {
        tmp.push(src[i]);
    }
    for i in 0..len {
        dst[i] = tmp[i];
    }
}

// ---------------------------------------------------------------------------
// stdio equivalents
// ---------------------------------------------------------------------------

/// Statistics for stdio calls.
static PRINTF_CALLS: AtomicU64 = AtomicU64::new(0);

/// Simple printf-style formatter supporting %s, %d, %x, and %%.
/// Arguments are passed as string slices; %d and %x interpret them via atoi
/// or strtol.
pub fn printf(fmt: &str, args: &[&str]) -> String {
    PRINTF_CALLS.fetch_add(1, Ordering::Relaxed);
    let mut out = String::new();
    let bytes = fmt.as_bytes();
    let mut i = 0;
    let mut arg_idx = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 1 < bytes.len() {
            i += 1;
            match bytes[i] {
                b's' => {
                    if arg_idx < args.len() {
                        out.push_str(args[arg_idx]);
                        arg_idx += 1;
                    } else {
                        out.push_str("(null)");
                    }
                }
                b'd' => {
                    if arg_idx < args.len() {
                        let n = atoi(args[arg_idx]);
                        arg_idx += 1;
                        out.push_str(&format!("{}", n));
                    } else {
                        out.push('0');
                    }
                }
                b'x' => {
                    if arg_idx < args.len() {
                        let n = strtol(args[arg_idx], 10).unwrap_or(0);
                        arg_idx += 1;
                        out.push_str(&format!("{:x}", n));
                    } else {
                        out.push('0');
                    }
                }
                b'%' => {
                    out.push('%');
                }
                other => {
                    out.push('%');
                    out.push(other as char);
                }
            }
        } else {
            out.push(bytes[i] as char);
        }
        i += 1;
    }
    out
}

/// Alias for `printf` (same semantics in our implementation).
pub fn sprintf(fmt: &str, args: &[&str]) -> String {
    printf(fmt, args)
}

/// Print a string to serial and VGA (like C puts, appends newline).
pub fn puts(s: &str) {
    crate::serial_println!("{}", s);
}

/// Read a character from the keyboard buffer, if available.
pub fn getchar() -> Option<u8> {
    // Delegate to the keyboard subsystem.
    // In practice this would block; here we just poll.
    None
}

/// Write a single character to serial output.
pub fn putchar(c: u8) {
    crate::serial_println!("{}", c as char);
}

// ---------------------------------------------------------------------------
// stdlib equivalents
// ---------------------------------------------------------------------------

/// Simulated heap allocation tracking.
struct AllocEntry {
    ptr: u64,
    size: usize,
}

static SIM_HEAP: Mutex<Vec<AllocEntry>> = Mutex::new(Vec::new());
static MALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static FREE_COUNT: AtomicU64 = AtomicU64::new(0);
static NEXT_PTR: AtomicU64 = AtomicU64::new(0x1000_0000);

/// Simulated malloc — tracks an allocation of `size` bytes and returns a
/// pseudo-pointer.  Does not allocate real memory.
pub fn malloc_sim(size: usize) -> u64 {
    if size == 0 {
        set_errno(EINVAL);
        return 0;
    }
    let ptr = NEXT_PTR.fetch_add(size as u64, Ordering::Relaxed);
    let mut heap = SIM_HEAP.lock();
    heap.push(AllocEntry { ptr, size });
    MALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
    ptr
}

/// Simulated free — removes the tracked allocation for `ptr`.
pub fn free_sim(ptr: u64) {
    let mut heap = SIM_HEAP.lock();
    if let Some(pos) = heap.iter().position(|e| e.ptr == ptr) {
        heap.remove(pos);
        FREE_COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

/// PRNG state (xorshift32).
static RNG_STATE: AtomicU32 = AtomicU32::new(0xDEAD_BEEF);

/// Return a pseudo-random 32-bit integer (xorshift32).
pub fn rand() -> u32 {
    let mut s = RNG_STATE.load(Ordering::Relaxed);
    s ^= s << 13;
    s ^= s >> 17;
    s ^= s << 5;
    RNG_STATE.store(s, Ordering::Relaxed);
    s
}

/// Seed the PRNG.
pub fn srand(seed: u32) {
    let s = if seed == 0 { 1 } else { seed };
    RNG_STATE.store(s, Ordering::Relaxed);
}

/// Absolute value.
pub fn abs(n: i32) -> i32 {
    if n < 0 { -n } else { n }
}

/// Minimum of two integers.
pub fn min(a: i32, b: i32) -> i32 {
    if a < b { a } else { b }
}

/// Maximum of two integers.
pub fn max(a: i32, b: i32) -> i32 {
    if a > b { a } else { b }
}

/// Exit the current process.  Delegates to the userland subsystem.
pub fn exit(code: i32) {
    let pid = getpid();
    if pid > 0 {
        crate::userland::exit_process(pid, code);
    }
}

/// Return the PID of the current process.
/// In this simulated environment, returns 0 if no process context is active.
pub fn getpid() -> u32 {
    CURRENT_PID.load(Ordering::Relaxed)
}

/// Per-CPU current PID (set by the scheduler before entering userspace).
static CURRENT_PID: AtomicU32 = AtomicU32::new(0);

/// Set the current PID (called by scheduler/dispatcher).
pub fn set_current_pid(pid: u32) {
    CURRENT_PID.store(pid, Ordering::Relaxed);
}

/// Sleep for `ms` milliseconds (simulated — just records the intent).
pub fn sleep_ms(ms: u32) {
    // In a real kernel this would mark the task as sleeping and set a timer.
    crate::serial_println!("[libc] sleep_ms({})", ms);
    let _ = ms;
}

// ---------------------------------------------------------------------------
// ctype functions
// ---------------------------------------------------------------------------

/// Check if byte is an alphabetic ASCII character.
pub fn isalpha(c: u8) -> bool {
    (c >= b'A' && c <= b'Z') || (c >= b'a' && c <= b'z')
}

/// Check if byte is a decimal digit.
pub fn isdigit(c: u8) -> bool {
    c >= b'0' && c <= b'9'
}

/// Check if byte is alphanumeric.
pub fn isalnum(c: u8) -> bool {
    isalpha(c) || isdigit(c)
}

/// Check if byte is an uppercase letter.
pub fn isupper(c: u8) -> bool {
    c >= b'A' && c <= b'Z'
}

/// Check if byte is a lowercase letter.
pub fn islower(c: u8) -> bool {
    c >= b'a' && c <= b'z'
}

/// Check if byte is whitespace (space, tab, newline, carriage return, form feed, vertical tab).
pub fn isspace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C)
}

/// Convert lowercase to uppercase.  Non-lowercase bytes are returned unchanged.
pub fn toupper(c: u8) -> u8 {
    if islower(c) { c - 32 } else { c }
}

/// Convert uppercase to lowercase.  Non-uppercase bytes are returned unchanged.
pub fn tolower(c: u8) -> u8 {
    if isupper(c) { c + 32 } else { c }
}

// ---------------------------------------------------------------------------
// Statistics and initialisation
// ---------------------------------------------------------------------------

/// Return a formatted summary of libc usage statistics.
pub fn libc_stats() -> String {
    let malloc = MALLOC_COUNT.load(Ordering::Relaxed);
    let free = FREE_COUNT.load(Ordering::Relaxed);
    let printfs = PRINTF_CALLS.load(Ordering::Relaxed);
    let heap = SIM_HEAP.lock();
    let live_bytes: usize = heap.iter().map(|e| e.size).sum();
    format!(
        "libc statistics:\n  malloc calls: {}\n  free calls: {}\n  live allocations: {} ({} bytes)\n  printf calls: {}\n  errno: {} ({})\n",
        malloc, free, heap.len(), live_bytes, printfs,
        get_errno(), strerror(get_errno()),
    )
}

/// Initialise the libc subsystem: seed PRNG, reset errno and counters.
pub fn init() {
    srand(0xC0DE_CAFE);
    set_errno(0);
    MALLOC_COUNT.store(0, Ordering::Relaxed);
    FREE_COUNT.store(0, Ordering::Relaxed);
    PRINTF_CALLS.store(0, Ordering::Relaxed);
    NEXT_PTR.store(0x1000_0000, Ordering::Relaxed);
    set_current_pid(0);
    crate::serial_println!("[libc] initialised");
    crate::klog_println!("[libc] subsystem ready");
}
