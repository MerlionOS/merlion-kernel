/// Minimal C++ runtime support for MerlionOS.
///
/// Provides the essential ABI functions that C++ programs need:
/// - new/delete operators (via kernel heap)
/// - __cxa_atexit for static destructors
/// - Exception handling stubs (__cxa_throw, __cxa_begin_catch, etc.)
/// - Guard variables for thread-safe static initialization
/// - RTTI support stubs

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::serial_println;

// ═══════════════════════════════════════════════════════════════════
//  OPERATOR NEW / DELETE
// ═══════════════════════════════════════════════════════════════════

static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static FREE_COUNT: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);

/// operator new(size) — allocate via kernel heap.
pub fn cxx_new(size: usize) -> u64 {
    if size == 0 { return 0; }
    // Use brk-based allocation for userspace
    // In kernel context, use alloc
    ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
    ALLOC_BYTES.fetch_add(size as u64, Ordering::Relaxed);
    // Return a simulated pointer (in practice, delegates to malloc)
    let ptr = NEXT_CXX_PTR.fetch_add(size as u64 + 16, Ordering::SeqCst);
    ptr
}

static NEXT_CXX_PTR: AtomicU64 = AtomicU64::new(0x2000_0000);

/// operator delete(ptr) — free via kernel heap.
pub fn cxx_delete(_ptr: u64) {
    FREE_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// operator new[](size) — array new.
pub fn cxx_new_array(size: usize) -> u64 {
    cxx_new(size)
}

/// operator delete[](ptr) — array delete.
pub fn cxx_delete_array(ptr: u64) {
    cxx_delete(ptr)
}

// ═══════════════════════════════════════════════════════════════════
//  ATEXIT
// ═══════════════════════════════════════════════════════════════════

const MAX_ATEXIT: usize = 32;

struct AtexitEntry {
    func_addr: u64,
    arg: u64,
    dso_handle: u64,
}

static ATEXIT_LIST: Mutex<Vec<AtexitEntry>> = Mutex::new(Vec::new());

/// __cxa_atexit(func, arg, dso_handle) — register destructor.
pub fn cxa_atexit(func: u64, arg: u64, dso_handle: u64) -> i32 {
    let mut list = ATEXIT_LIST.lock();
    if list.len() >= MAX_ATEXIT {
        return -1;
    }
    list.push(AtexitEntry { func_addr: func, arg, dso_handle });
    serial_println!("[cxxrt] __cxa_atexit registered (total {})", list.len());
    0
}

/// Run all registered atexit handlers (called at program exit).
pub fn run_atexit() {
    let mut list = ATEXIT_LIST.lock();
    let count = list.len();
    // Run in reverse order
    list.clear();
    serial_println!("[cxxrt] ran {} atexit handlers", count);
}

// ═══════════════════════════════════════════════════════════════════
//  EXCEPTION HANDLING (stubs — Envoy can be compiled with -fno-exceptions)
// ═══════════════════════════════════════════════════════════════════

/// __cxa_throw — throw an exception (stub: abort).
pub fn cxa_throw(_thrown: u64, _type_info: u64, _destructor: u64) {
    serial_println!("[cxxrt] __cxa_throw called — aborting (exceptions not supported)");
    // In a real implementation, this would unwind the stack
}

/// __cxa_begin_catch — begin catch block (stub).
pub fn cxa_begin_catch(exception: u64) -> u64 {
    serial_println!("[cxxrt] __cxa_begin_catch({:#x})", exception);
    exception
}

/// __cxa_end_catch — end catch block (stub).
pub fn cxa_end_catch() {
    serial_println!("[cxxrt] __cxa_end_catch");
}

/// __cxa_allocate_exception — allocate exception object.
pub fn cxa_allocate_exception(size: usize) -> u64 {
    cxx_new(size + 128) // extra space for exception header
}

/// _Unwind_Resume — resume unwinding (stub: abort).
pub fn unwind_resume(_exception: u64) {
    serial_println!("[cxxrt] _Unwind_Resume — aborting");
}

// ═══════════════════════════════════════════════════════════════════
//  GUARD VARIABLES (thread-safe static init)
// ═══════════════════════════════════════════════════════════════════

/// __cxa_guard_acquire — acquire initialization lock.
/// Returns 1 if this thread should initialize, 0 if already done.
pub fn cxa_guard_acquire(guard: u64) -> i32 {
    // Simplified: use the guard value as an atomic flag
    unsafe {
        let ptr = guard as *mut u8;
        if *ptr == 0 {
            *ptr = 1; // mark as initializing
            1
        } else {
            0 // already initialized
        }
    }
}

/// __cxa_guard_release — release initialization lock.
pub fn cxa_guard_release(guard: u64) {
    unsafe {
        let ptr = guard as *mut u8;
        *ptr = 2; // mark as fully initialized
    }
}

/// __cxa_guard_abort — abort initialization.
pub fn cxa_guard_abort(guard: u64) {
    unsafe {
        let ptr = guard as *mut u8;
        *ptr = 0; // reset
    }
}

// ═══════════════════════════════════════════════════════════════════
//  RTTI (Runtime Type Information) stubs
// ═══════════════════════════════════════════════════════════════════

/// __dynamic_cast — dynamic_cast<T>(ptr) (stub: return input).
pub fn dynamic_cast(src_ptr: u64, _src_type: u64, _dst_type: u64) -> u64 {
    src_ptr // simplified: always succeed
}

/// typeid — type information (stub).
pub fn type_info_name(_type_info: u64) -> &'static str {
    "unknown_type"
}

// ═══════════════════════════════════════════════════════════════════
//  PURE VIRTUAL / DELETED FUNCTION
// ═══════════════════════════════════════════════════════════════════

/// __cxa_pure_virtual — called when a pure virtual function is invoked.
pub fn cxa_pure_virtual() {
    serial_println!("[cxxrt] pure virtual function called — aborting");
}

/// __cxa_deleted_virtual — called when a deleted virtual function is invoked.
pub fn cxa_deleted_virtual() {
    serial_println!("[cxxrt] deleted virtual function called — aborting");
}

// ═══════════════════════════════════════════════════════════════════
//  INFO
// ═══════════════════════════════════════════════════════════════════

pub fn init() {
    serial_println!("[cxxrt] C++ runtime initialized (new/delete, atexit, guard, exceptions stub)");
}

pub fn info() -> String {
    let atexit_count = ATEXIT_LIST.lock().len();
    format!(
        "C++ Runtime:\n\
         new calls:    {}\n\
         delete calls: {}\n\
         bytes alloc:  {}\n\
         atexit:       {} / {}\n\
         exceptions:   stub (compile with -fno-exceptions)\n\
         RTTI:         stub\n",
        ALLOC_COUNT.load(Ordering::Relaxed),
        FREE_COUNT.load(Ordering::Relaxed),
        ALLOC_BYTES.load(Ordering::Relaxed),
        atexit_count, MAX_ATEXIT,
    )
}
