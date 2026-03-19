/// POSIX threads (pthreads) implementation for MerlionOS.
///
/// Provides mutex, condition variable, rwlock, barrier, and thread-local
/// storage for user programs. Kernel-side implementation exposed via syscalls.

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::string::String;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, AtomicBool, Ordering};
use crate::serial_println;

// ═══════════════════════════════════════════════════════════════════
//  MUTEX
// ═══════════════════════════════════════════════════════════════════

const MAX_MUTEXES: usize = 64;

struct PthreadMutex {
    id: u32,
    locked: AtomicBool,
    owner_tid: AtomicU32,  // thread that holds the lock
    waiters: AtomicU32,    // number of threads waiting
}

impl PthreadMutex {
    fn new(id: u32) -> Self {
        Self {
            id,
            locked: AtomicBool::new(false),
            owner_tid: AtomicU32::new(0),
            waiters: AtomicU32::new(0),
        }
    }
}

static MUTEXES: Mutex<Vec<Option<PthreadMutex>>> = Mutex::new(Vec::new());
static NEXT_MUTEX_ID: AtomicU32 = AtomicU32::new(1);

/// Create a new mutex. Returns mutex ID.
pub fn mutex_create() -> u32 {
    let id = NEXT_MUTEX_ID.fetch_add(1, Ordering::SeqCst);
    let mut mutexes = MUTEXES.lock();
    // Find free slot or push
    let mut found = false;
    for slot in mutexes.iter_mut() {
        if slot.is_none() {
            *slot = Some(PthreadMutex::new(id));
            found = true;
            break;
        }
    }
    if !found && mutexes.len() < MAX_MUTEXES {
        mutexes.push(Some(PthreadMutex::new(id)));
    }
    serial_println!("[pthread] mutex_create() = {}", id);
    id
}

/// Lock a mutex. Spins with yield until acquired.
pub fn mutex_lock(mutex_id: u32) -> i32 {
    let tid = crate::task::current_pid() as u32;
    let mutexes = MUTEXES.lock();
    let mtx = match mutexes.iter().flat_map(|s| s.as_ref()).find(|m| m.id == mutex_id) {
        Some(m) => m,
        None => return -1,
    };
    mtx.waiters.fetch_add(1, Ordering::SeqCst);
    drop(mutexes);

    // Spin-yield until we acquire the lock
    loop {
        let mutexes = MUTEXES.lock();
        let mtx = match mutexes.iter().flat_map(|s| s.as_ref()).find(|m| m.id == mutex_id) {
            Some(m) => m,
            None => return -1,
        };
        if !mtx.locked.load(Ordering::SeqCst) {
            mtx.locked.store(true, Ordering::SeqCst);
            mtx.owner_tid.store(tid, Ordering::SeqCst);
            mtx.waiters.fetch_sub(1, Ordering::SeqCst);
            return 0;
        }
        drop(mutexes);
        crate::task::yield_now();
    }
}

/// Unlock a mutex.
pub fn mutex_unlock(mutex_id: u32) -> i32 {
    let mutexes = MUTEXES.lock();
    let mtx = match mutexes.iter().flat_map(|s| s.as_ref()).find(|m| m.id == mutex_id) {
        Some(m) => m,
        None => return -1,
    };
    mtx.locked.store(false, Ordering::SeqCst);
    mtx.owner_tid.store(0, Ordering::SeqCst);
    0
}

/// Destroy a mutex.
pub fn mutex_destroy(mutex_id: u32) -> i32 {
    let mut mutexes = MUTEXES.lock();
    for slot in mutexes.iter_mut() {
        if let Some(m) = slot {
            if m.id == mutex_id {
                *slot = None;
                return 0;
            }
        }
    }
    -1
}

// ═══════════════════════════════════════════════════════════════════
//  CONDITION VARIABLE
// ═══════════════════════════════════════════════════════════════════

const MAX_CONDVARS: usize = 32;

struct PthreadCondvar {
    id: u32,
    signal_count: AtomicU32,
    waiters: AtomicU32,
}

impl PthreadCondvar {
    fn new(id: u32) -> Self {
        Self { id, signal_count: AtomicU32::new(0), waiters: AtomicU32::new(0) }
    }
}

static CONDVARS: Mutex<Vec<Option<PthreadCondvar>>> = Mutex::new(Vec::new());
static NEXT_CONDVAR_ID: AtomicU32 = AtomicU32::new(1);

/// Create a condition variable.
pub fn condvar_create() -> u32 {
    let id = NEXT_CONDVAR_ID.fetch_add(1, Ordering::SeqCst);
    let mut cvs = CONDVARS.lock();
    if cvs.len() < MAX_CONDVARS {
        cvs.push(Some(PthreadCondvar::new(id)));
    }
    serial_println!("[pthread] condvar_create() = {}", id);
    id
}

/// Wait on a condition variable (releases mutex, waits, re-acquires).
pub fn condvar_wait(condvar_id: u32, mutex_id: u32) -> i32 {
    let cvs = CONDVARS.lock();
    let cv = match cvs.iter().flat_map(|s| s.as_ref()).find(|c| c.id == condvar_id) {
        Some(c) => c,
        None => return -1,
    };
    let initial_signal = cv.signal_count.load(Ordering::SeqCst);
    cv.waiters.fetch_add(1, Ordering::SeqCst);
    drop(cvs);

    // Release mutex
    mutex_unlock(mutex_id);

    // Wait for signal
    for _ in 0..1000 {
        let cvs = CONDVARS.lock();
        if let Some(cv) = cvs.iter().flat_map(|s| s.as_ref()).find(|c| c.id == condvar_id) {
            if cv.signal_count.load(Ordering::SeqCst) != initial_signal {
                cv.waiters.fetch_sub(1, Ordering::SeqCst);
                drop(cvs);
                mutex_lock(mutex_id);
                return 0;
            }
        }
        drop(cvs);
        crate::task::yield_now();
    }

    // Timeout
    mutex_lock(mutex_id);
    -1
}

/// Signal one waiter on a condition variable.
pub fn condvar_signal(condvar_id: u32) -> i32 {
    let cvs = CONDVARS.lock();
    if let Some(cv) = cvs.iter().flat_map(|s| s.as_ref()).find(|c| c.id == condvar_id) {
        cv.signal_count.fetch_add(1, Ordering::SeqCst);
        return 0;
    }
    -1
}

/// Broadcast: wake all waiters.
pub fn condvar_broadcast(condvar_id: u32) -> i32 {
    let cvs = CONDVARS.lock();
    if let Some(cv) = cvs.iter().flat_map(|s| s.as_ref()).find(|c| c.id == condvar_id) {
        let waiters = cv.waiters.load(Ordering::SeqCst);
        cv.signal_count.fetch_add(waiters.max(1), Ordering::SeqCst);
        return 0;
    }
    -1
}

/// Destroy a condition variable.
pub fn condvar_destroy(condvar_id: u32) -> i32 {
    let mut cvs = CONDVARS.lock();
    for slot in cvs.iter_mut() {
        if let Some(c) = slot {
            if c.id == condvar_id {
                *slot = None;
                return 0;
            }
        }
    }
    -1
}

// ═══════════════════════════════════════════════════════════════════
//  READ-WRITE LOCK
// ═══════════════════════════════════════════════════════════════════

const MAX_RWLOCKS: usize = 32;

struct PthreadRwlock {
    id: u32,
    readers: AtomicU32,
    writer: AtomicBool,
}

impl PthreadRwlock {
    fn new(id: u32) -> Self {
        Self { id, readers: AtomicU32::new(0), writer: AtomicBool::new(false) }
    }
}

static RWLOCKS: Mutex<Vec<Option<PthreadRwlock>>> = Mutex::new(Vec::new());
static NEXT_RWLOCK_ID: AtomicU32 = AtomicU32::new(1);

pub fn rwlock_create() -> u32 {
    let id = NEXT_RWLOCK_ID.fetch_add(1, Ordering::SeqCst);
    let mut locks = RWLOCKS.lock();
    if locks.len() < MAX_RWLOCKS {
        locks.push(Some(PthreadRwlock::new(id)));
    }
    id
}

pub fn rwlock_rdlock(rwlock_id: u32) -> i32 {
    loop {
        let locks = RWLOCKS.lock();
        if let Some(rw) = locks.iter().flat_map(|s| s.as_ref()).find(|r| r.id == rwlock_id) {
            if !rw.writer.load(Ordering::SeqCst) {
                rw.readers.fetch_add(1, Ordering::SeqCst);
                return 0;
            }
        } else { return -1; }
        drop(locks);
        crate::task::yield_now();
    }
}

pub fn rwlock_wrlock(rwlock_id: u32) -> i32 {
    loop {
        let locks = RWLOCKS.lock();
        if let Some(rw) = locks.iter().flat_map(|s| s.as_ref()).find(|r| r.id == rwlock_id) {
            if !rw.writer.load(Ordering::SeqCst) && rw.readers.load(Ordering::SeqCst) == 0 {
                rw.writer.store(true, Ordering::SeqCst);
                return 0;
            }
        } else { return -1; }
        drop(locks);
        crate::task::yield_now();
    }
}

pub fn rwlock_unlock(rwlock_id: u32) -> i32 {
    let locks = RWLOCKS.lock();
    if let Some(rw) = locks.iter().flat_map(|s| s.as_ref()).find(|r| r.id == rwlock_id) {
        if rw.writer.load(Ordering::SeqCst) {
            rw.writer.store(false, Ordering::SeqCst);
        } else {
            rw.readers.fetch_sub(1, Ordering::SeqCst);
        }
        return 0;
    }
    -1
}

// ═══════════════════════════════════════════════════════════════════
//  FUTEX (fast userspace mutex)
// ═══════════════════════════════════════════════════════════════════

const MAX_FUTEX_WAITERS: usize = 64;

struct FutexWaiter {
    addr: u64,      // userspace address being waited on
    tid: u32,       // thread ID
    woken: AtomicBool,
}

static FUTEX_WAITERS: Mutex<Vec<FutexWaiter>> = Mutex::new(Vec::new());

/// Futex wait: if *addr == expected, sleep until woken.
pub fn futex_wait(addr: u64, expected: u32) -> i32 {
    let tid = crate::task::current_pid() as u32;
    // Check current value
    let current = unsafe { *(addr as *const u32) };
    if current != expected {
        return -1; // value changed, don't sleep
    }

    let mut waiters = FUTEX_WAITERS.lock();
    if waiters.len() >= MAX_FUTEX_WAITERS {
        return -1;
    }
    waiters.push(FutexWaiter {
        addr, tid, woken: AtomicBool::new(false),
    });
    drop(waiters);

    // Yield until woken (with timeout)
    for _ in 0..1000 {
        let waiters = FUTEX_WAITERS.lock();
        if let Some(w) = waiters.iter().find(|w| w.addr == addr && w.tid == tid) {
            if w.woken.load(Ordering::SeqCst) {
                drop(waiters);
                // Remove ourselves
                let mut waiters = FUTEX_WAITERS.lock();
                waiters.retain(|w| !(w.addr == addr && w.tid == tid));
                return 0;
            }
        } else {
            return 0; // already removed
        }
        drop(waiters);
        crate::task::yield_now();
    }
    // Timeout — remove ourselves
    let mut waiters = FUTEX_WAITERS.lock();
    waiters.retain(|w| !(w.addr == addr && w.tid == tid));
    -1
}

/// Futex wake: wake up to `count` waiters on `addr`.
pub fn futex_wake(addr: u64, count: u32) -> i32 {
    let waiters = FUTEX_WAITERS.lock();
    let mut woken = 0u32;
    for w in waiters.iter() {
        if w.addr == addr && !w.woken.load(Ordering::SeqCst) {
            w.woken.store(true, Ordering::SeqCst);
            woken += 1;
            if woken >= count { break; }
        }
    }
    woken as i32
}

// ═══════════════════════════════════════════════════════════════════
//  FCNTL FLAGS
// ═══════════════════════════════════════════════════════════════════

pub const F_GETFL: u32 = 3;
pub const F_SETFL: u32 = 4;
pub const F_GETFD: u32 = 1;
pub const F_SETFD: u32 = 2;
pub const O_NONBLOCK: u32 = 0x800;
pub const O_CLOEXEC: u32 = 0x80000;
pub const FD_CLOEXEC: u32 = 1;

/// Per-fd flags storage.
static FD_FLAGS: Mutex<BTreeMap<u32, u32>> = Mutex::new(BTreeMap::new());

/// fcntl — file descriptor control.
pub fn fcntl(fd: u32, cmd: u32, arg: u32) -> i32 {
    match cmd {
        F_GETFL => {
            let flags = FD_FLAGS.lock();
            *flags.get(&fd).unwrap_or(&0) as i32
        }
        F_SETFL => {
            FD_FLAGS.lock().insert(fd, arg);
            0
        }
        F_GETFD => {
            let flags = FD_FLAGS.lock();
            (flags.get(&(fd | 0x8000_0000)).unwrap_or(&0) & FD_CLOEXEC) as i32
        }
        F_SETFD => {
            FD_FLAGS.lock().insert(fd | 0x8000_0000, arg);
            0
        }
        _ => -1,
    }
}

/// Check if a fd is set to non-blocking.
pub fn is_nonblocking(fd: u32) -> bool {
    let flags = FD_FLAGS.lock();
    flags.get(&fd).map_or(false, |f| f & O_NONBLOCK != 0)
}

// ═══════════════════════════════════════════════════════════════════
//  SOCKET OPTIONS
// ═══════════════════════════════════════════════════════════════════

pub const SOL_SOCKET: u32 = 1;
pub const SO_REUSEADDR: u32 = 2;
pub const SO_KEEPALIVE: u32 = 9;
pub const SO_RCVBUF: u32 = 8;
pub const SO_SNDBUF: u32 = 7;
pub const IPPROTO_TCP: u32 = 6;
pub const TCP_NODELAY: u32 = 1;

static SOCK_OPTS: Mutex<BTreeMap<(u32, u32, u32), u32>> = Mutex::new(BTreeMap::new());

pub fn setsockopt(fd: u32, level: u32, optname: u32, optval: u32) -> i32 {
    SOCK_OPTS.lock().insert((fd, level, optname), optval);
    serial_println!("[socket] setsockopt(fd={}, level={}, opt={}, val={})", fd, level, optname, optval);
    0
}

pub fn getsockopt(fd: u32, level: u32, optname: u32) -> i32 {
    let opts = SOCK_OPTS.lock();
    *opts.get(&(fd, level, optname)).unwrap_or(&0) as i32
}

// ═══════════════════════════════════════════════════════════════════
//  INITIALIZATION
// ═══════════════════════════════════════════════════════════════════

pub fn init() {
    serial_println!("[pthread] POSIX threads initialized (mutex/condvar/rwlock/futex/fcntl)");
}

pub fn info() -> String {
    let mutexes = MUTEXES.lock().iter().filter(|s| s.is_some()).count();
    let condvars = CONDVARS.lock().iter().filter(|s| s.is_some()).count();
    let rwlocks = RWLOCKS.lock().iter().filter(|s| s.is_some()).count();
    let futex_waiters = FUTEX_WAITERS.lock().len();
    alloc::format!(
        "POSIX Threads:\n\
         Mutexes:    {} / {}\n\
         Condvars:   {} / {}\n\
         RW Locks:   {} / {}\n\
         Futex waiters: {} / {}\n",
        mutexes, MAX_MUTEXES, condvars, MAX_CONDVARS,
        rwlocks, MAX_RWLOCKS, futex_waiters, MAX_FUTEX_WAITERS,
    )
}
