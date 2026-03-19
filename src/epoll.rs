/// epoll — I/O event notification facility (Linux-compatible).
///
/// Provides efficient I/O multiplexing for network servers.
/// User creates an epoll instance, registers file descriptors with
/// interest masks, then waits for events.

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::serial_println;

// ═══════════════════════════════════════════════════════════════════
//  CONSTANTS (Linux-compatible)
// ═══════════════════════════════════════════════════════════════════

pub const EPOLLIN: u32 = 0x001;
pub const EPOLLOUT: u32 = 0x004;
pub const EPOLLERR: u32 = 0x008;
pub const EPOLLHUP: u32 = 0x010;
pub const EPOLLET: u32 = 1 << 31;  // Edge-triggered
pub const EPOLLONESHOT: u32 = 1 << 30;

pub const EPOLL_CTL_ADD: u32 = 1;
pub const EPOLL_CTL_DEL: u32 = 2;
pub const EPOLL_CTL_MOD: u32 = 3;

const MAX_EPOLL_INSTANCES: usize = 16;
const MAX_EVENTS_PER_INSTANCE: usize = 64;

// ═══════════════════════════════════════════════════════════════════
//  TYPES
// ═══════════════════════════════════════════════════════════════════

/// An event returned by epoll_wait.
#[derive(Clone, Copy)]
pub struct EpollEvent {
    pub events: u32,  // EPOLLIN | EPOLLOUT | ...
    pub fd: u32,      // user data (file descriptor)
}

/// A registered interest on a file descriptor.
#[derive(Clone)]
struct Interest {
    fd: u32,
    events: u32,      // mask: EPOLLIN | EPOLLOUT | ...
    edge_triggered: bool,
    oneshot: bool,
    triggered: bool,   // for oneshot: already fired
}

/// An epoll instance.
struct EpollInstance {
    id: u32,
    interests: BTreeMap<u32, Interest>,  // fd → Interest
}

impl EpollInstance {
    fn new(id: u32) -> Self {
        Self { id, interests: BTreeMap::new() }
    }
}

// ═══════════════════════════════════════════════════════════════════
//  GLOBAL STATE
// ═══════════════════════════════════════════════════════════════════

struct EpollState {
    instances: Vec<Option<EpollInstance>>,
    next_id: u32,
}

impl EpollState {
    const fn new() -> Self {
        Self { instances: Vec::new(), next_id: 1 }
    }
}

static STATE: Mutex<EpollState> = Mutex::new(EpollState::new());
static EPOLL_WAITS: AtomicU32 = AtomicU32::new(0);

// ═══════════════════════════════════════════════════════════════════
//  PUBLIC API
// ═══════════════════════════════════════════════════════════════════

/// Create a new epoll instance. Returns epoll fd (instance ID).
pub fn epoll_create() -> i32 {
    let mut state = STATE.lock();
    let id = state.next_id;
    state.next_id += 1;

    if state.instances.len() >= MAX_EPOLL_INSTANCES {
        // Find a free slot
        for slot in state.instances.iter_mut() {
            if slot.is_none() {
                *slot = Some(EpollInstance::new(id));
                serial_println!("[epoll] created instance {}", id);
                return id as i32;
            }
        }
        return -1;
    }
    state.instances.push(Some(EpollInstance::new(id)));
    serial_println!("[epoll] created instance {}", id);
    id as i32
}

/// Control an epoll instance: add, modify, or delete an fd.
pub fn epoll_ctl(epfd: u32, op: u32, fd: u32, events: u32) -> i32 {
    let mut state = STATE.lock();
    let instance = match state.instances.iter_mut()
        .flat_map(|s| s.as_mut())
        .find(|inst| inst.id == epfd)
    {
        Some(inst) => inst,
        None => return -1,
    };

    match op {
        EPOLL_CTL_ADD => {
            if instance.interests.len() >= MAX_EVENTS_PER_INSTANCE {
                return -1;
            }
            let interest = Interest {
                fd,
                events: events & !(EPOLLET | EPOLLONESHOT),
                edge_triggered: events & EPOLLET != 0,
                oneshot: events & EPOLLONESHOT != 0,
                triggered: false,
            };
            instance.interests.insert(fd, interest);
            serial_println!("[epoll] fd {} added to epfd {} (events={:#x})", fd, epfd, events);
            0
        }
        EPOLL_CTL_MOD => {
            if let Some(interest) = instance.interests.get_mut(&fd) {
                interest.events = events & !(EPOLLET | EPOLLONESHOT);
                interest.edge_triggered = events & EPOLLET != 0;
                interest.oneshot = events & EPOLLONESHOT != 0;
                interest.triggered = false;
                0
            } else {
                -1
            }
        }
        EPOLL_CTL_DEL => {
            if instance.interests.remove(&fd).is_some() { 0 } else { -1 }
        }
        _ => -1,
    }
}

/// Wait for events on an epoll instance.
/// Returns the number of ready file descriptors (up to max_events).
/// timeout_ms: -1 = block forever, 0 = return immediately, >0 = wait ms.
pub fn epoll_wait(epfd: u32, max_events: usize, timeout_ms: i32) -> Vec<EpollEvent> {
    EPOLL_WAITS.fetch_add(1, Ordering::Relaxed);

    let deadline = if timeout_ms > 0 {
        Some(crate::timer::ticks() + (timeout_ms as u64 * crate::timer::PIT_FREQUENCY_HZ) / 1000)
    } else if timeout_ms == 0 {
        Some(crate::timer::ticks()) // immediate
    } else {
        None // block forever (we'll yield a few times)
    };

    let max_polls = if timeout_ms < 0 { 100 } else { 1 };

    for _ in 0..max_polls {
        let events = poll_ready(epfd, max_events);
        if !events.is_empty() {
            return events;
        }
        if let Some(dl) = deadline {
            if crate::timer::ticks() >= dl {
                return Vec::new();
            }
        }
        crate::task::yield_now();
    }

    Vec::new()
}

/// Poll which registered fds are ready.
fn poll_ready(epfd: u32, max_events: usize) -> Vec<EpollEvent> {
    let mut state = STATE.lock();
    let instance = match state.instances.iter_mut()
        .flat_map(|s| s.as_mut())
        .find(|inst| inst.id == epfd)
    {
        Some(inst) => inst,
        None => return Vec::new(),
    };

    let mut ready = Vec::new();

    for (_, interest) in instance.interests.iter_mut() {
        if interest.oneshot && interest.triggered {
            continue;
        }
        // Check if fd is ready — for now, all fds are considered ready for write,
        // and ready for read if they have data (simplified).
        let mut revents: u32 = 0;

        if interest.events & EPOLLIN != 0 {
            // Check if fd has data available
            // For socket fds, check if TCP has data; for file fds, always ready
            revents |= EPOLLIN;
        }
        if interest.events & EPOLLOUT != 0 {
            // Write is usually ready unless buffer full
            revents |= EPOLLOUT;
        }

        if revents != 0 {
            ready.push(EpollEvent { events: revents, fd: interest.fd });
            if interest.oneshot {
                interest.triggered = true;
            }
            if ready.len() >= max_events {
                break;
            }
        }
    }

    ready
}

/// Close an epoll instance.
pub fn epoll_close(epfd: u32) -> i32 {
    let mut state = STATE.lock();
    for slot in state.instances.iter_mut() {
        if let Some(inst) = slot {
            if inst.id == epfd {
                serial_println!("[epoll] closed instance {}", epfd);
                *slot = None;
                return 0;
            }
        }
    }
    -1
}

pub fn init() {
    serial_println!("[epoll] I/O multiplexing initialized (max {} instances)", MAX_EPOLL_INSTANCES);
}

pub fn info() -> alloc::string::String {
    let state = STATE.lock();
    let active = state.instances.iter().filter(|s| s.is_some()).count();
    alloc::format!(
        "epoll: {} active instances, {} total waits\n",
        active, EPOLL_WAITS.load(Ordering::Relaxed),
    )
}
