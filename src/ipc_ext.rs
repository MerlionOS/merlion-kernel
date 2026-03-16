/// Extended IPC mechanisms for MerlionOS.
/// Provides Unix domain sockets, message queues, semaphores,
/// shared memory segments, and event file descriptors.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum Unix domain sockets.
const MAX_UNIX_SOCKETS: usize = 64;

/// Maximum message queues.
const MAX_MESSAGE_QUEUES: usize = 32;

/// Maximum messages per queue.
const MAX_MESSAGES_PER_QUEUE: usize = 128;

/// Maximum message size in bytes.
const MAX_MESSAGE_SIZE: usize = 4096;

/// Maximum named semaphores.
const MAX_SEMAPHORES: usize = 64;

/// Maximum shared memory segments.
const MAX_SHM_SEGMENTS: usize = 32;

/// Maximum eventfd instances.
const MAX_EVENTFDS: usize = 64;

/// Maximum signalfd instances.
const MAX_SIGNALFDS: usize = 32;

/// Maximum timerfd instances.
const MAX_TIMERFDS: usize = 32;

/// Maximum epoll instances.
const MAX_EPOLLS: usize = 32;

/// Maximum file descriptors per epoll.
const MAX_EPOLL_FDS: usize = 64;

/// Maximum IPC namespaces.
const MAX_IPC_NAMESPACES: usize = 16;

// ---------------------------------------------------------------------------
// Unix domain sockets
// ---------------------------------------------------------------------------

/// State of a Unix domain socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    /// Newly created.
    Created,
    /// Bound to an address.
    Bound,
    /// Listening for connections.
    Listening,
    /// Connected to a peer.
    Connected,
    /// Closed.
    Closed,
}

/// A Unix domain socket (simulated as in-kernel message passing).
struct UnixSocket {
    /// Unique socket ID.
    id: usize,
    /// Bound address (path).
    address: String,
    /// Current state.
    state: SocketState,
    /// Owning process ID.
    pid: usize,
    /// Connected peer socket ID (if connected).
    peer_id: Option<usize>,
    /// Incoming message buffer.
    recv_buf: Vec<Vec<u8>>,
    /// Pending connection requests (socket IDs).
    backlog: Vec<usize>,
    /// Maximum backlog size.
    max_backlog: usize,
    /// IPC namespace this socket belongs to.
    namespace_id: usize,
}

static UNIX_SOCKETS: Mutex<UnixSocketTable> = Mutex::new(UnixSocketTable::new());

struct UnixSocketTable {
    sockets: Vec<UnixSocket>,
    next_id: usize,
}

impl UnixSocketTable {
    const fn new() -> Self {
        Self { sockets: Vec::new(), next_id: 1 }
    }

    fn create(&mut self, pid: usize, namespace_id: usize) -> Option<usize> {
        if self.sockets.len() >= MAX_UNIX_SOCKETS { return None; }
        let id = self.next_id;
        self.next_id += 1;
        self.sockets.push(UnixSocket {
            id, address: String::new(), state: SocketState::Created,
            pid, peer_id: None, recv_buf: Vec::new(),
            backlog: Vec::new(), max_backlog: 16, namespace_id,
        });
        Some(id)
    }

    fn find_mut(&mut self, id: usize) -> Option<&mut UnixSocket> {
        self.sockets.iter_mut().find(|s| s.id == id)
    }

    fn find_by_addr(&self, addr: &str, ns: usize) -> Option<usize> {
        self.sockets.iter()
            .find(|s| s.address == addr && s.namespace_id == ns
                  && (s.state == SocketState::Bound || s.state == SocketState::Listening))
            .map(|s| s.id)
    }

    fn remove(&mut self, id: usize) -> bool {
        if let Some(pos) = self.sockets.iter().position(|s| s.id == id) {
            self.sockets.remove(pos);
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// POSIX message queues
// ---------------------------------------------------------------------------

/// A single message with priority.
#[derive(Clone)]
struct MqMessage {
    data: Vec<u8>,
    priority: u32,
}

/// A POSIX message queue.
struct MessageQueue {
    id: usize,
    name: String,
    messages: Vec<MqMessage>,
    max_messages: usize,
    max_msg_size: usize,
    namespace_id: usize,
    total_sent: u64,
    total_received: u64,
}

static MESSAGE_QUEUES: Mutex<MqTable> = Mutex::new(MqTable::new());

struct MqTable {
    queues: Vec<MessageQueue>,
    next_id: usize,
}

impl MqTable {
    const fn new() -> Self {
        Self { queues: Vec::new(), next_id: 1 }
    }

    fn open(&mut self, name: &str, namespace_id: usize) -> Option<usize> {
        // Return existing queue if name matches.
        for q in &self.queues {
            if q.name == name && q.namespace_id == namespace_id {
                return Some(q.id);
            }
        }
        if self.queues.len() >= MAX_MESSAGE_QUEUES { return None; }
        let id = self.next_id;
        self.next_id += 1;
        self.queues.push(MessageQueue {
            id, name: String::from(name), messages: Vec::new(),
            max_messages: MAX_MESSAGES_PER_QUEUE, max_msg_size: MAX_MESSAGE_SIZE,
            namespace_id, total_sent: 0, total_received: 0,
        });
        Some(id)
    }

    fn send(&mut self, id: usize, data: &[u8], priority: u32) -> bool {
        for q in self.queues.iter_mut() {
            if q.id == id {
                if q.messages.len() >= q.max_messages { return false; }
                if data.len() > q.max_msg_size { return false; }
                let msg = MqMessage { data: data.to_vec(), priority };
                // Insert sorted by priority (higher priority first).
                let pos = q.messages.iter()
                    .position(|m| m.priority < priority)
                    .unwrap_or(q.messages.len());
                q.messages.insert(pos, msg);
                q.total_sent += 1;
                return true;
            }
        }
        false
    }

    fn receive(&mut self, id: usize) -> Option<(Vec<u8>, u32)> {
        for q in self.queues.iter_mut() {
            if q.id == id {
                if q.messages.is_empty() { return None; }
                let msg = q.messages.remove(0);
                q.total_received += 1;
                return Some((msg.data, msg.priority));
            }
        }
        None
    }

    fn close(&mut self, id: usize) -> bool {
        if let Some(pos) = self.queues.iter().position(|q| q.id == id) {
            self.queues.remove(pos);
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Semaphores
// ---------------------------------------------------------------------------

/// A named semaphore.
struct Semaphore {
    id: usize,
    name: String,
    value: i32,
    max_value: i32,
    waiters: usize,
    namespace_id: usize,
    total_waits: u64,
    total_posts: u64,
}

static SEMAPHORES: Mutex<SemTable> = Mutex::new(SemTable::new());

struct SemTable {
    sems: Vec<Semaphore>,
    next_id: usize,
}

impl SemTable {
    const fn new() -> Self {
        Self { sems: Vec::new(), next_id: 1 }
    }

    fn init(&mut self, name: &str, value: i32, namespace_id: usize) -> Option<usize> {
        // Return existing if name matches.
        for s in &self.sems {
            if s.name == name && s.namespace_id == namespace_id {
                return Some(s.id);
            }
        }
        if self.sems.len() >= MAX_SEMAPHORES { return None; }
        let id = self.next_id;
        self.next_id += 1;
        self.sems.push(Semaphore {
            id, name: String::from(name), value, max_value: i32::MAX,
            waiters: 0, namespace_id, total_waits: 0, total_posts: 0,
        });
        Some(id)
    }

    fn wait(&mut self, id: usize) -> bool {
        for s in self.sems.iter_mut() {
            if s.id == id {
                if s.value > 0 {
                    s.value -= 1;
                    s.total_waits += 1;
                    return true;
                }
                s.waiters += 1;
                return false; // would block
            }
        }
        false
    }

    fn trywait(&mut self, id: usize) -> bool {
        for s in self.sems.iter_mut() {
            if s.id == id {
                if s.value > 0 {
                    s.value -= 1;
                    s.total_waits += 1;
                    return true;
                }
                return false;
            }
        }
        false
    }

    fn post(&mut self, id: usize) -> bool {
        for s in self.sems.iter_mut() {
            if s.id == id {
                s.value += 1;
                s.total_posts += 1;
                if s.waiters > 0 {
                    s.waiters -= 1;
                }
                return true;
            }
        }
        false
    }

    fn destroy(&mut self, id: usize) -> bool {
        if let Some(pos) = self.sems.iter().position(|s| s.id == id) {
            self.sems.remove(pos);
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// POSIX shared memory segments
// ---------------------------------------------------------------------------

/// A POSIX shared memory segment.
struct ShmSegment {
    id: usize,
    name: String,
    size: usize,
    data: Vec<u8>,
    attached_pids: Vec<usize>,
    namespace_id: usize,
}

static SHM_SEGMENTS: Mutex<ShmTable> = Mutex::new(ShmTable::new());

struct ShmTable {
    segments: Vec<ShmSegment>,
    next_id: usize,
}

impl ShmTable {
    const fn new() -> Self {
        Self { segments: Vec::new(), next_id: 1 }
    }

    fn open(&mut self, name: &str, size: usize, namespace_id: usize) -> Option<usize> {
        for seg in &self.segments {
            if seg.name == name && seg.namespace_id == namespace_id {
                return Some(seg.id);
            }
        }
        if self.segments.len() >= MAX_SHM_SEGMENTS { return None; }
        if size == 0 { return None; }
        let id = self.next_id;
        self.next_id += 1;
        self.segments.push(ShmSegment {
            id, name: String::from(name), size,
            data: vec![0u8; size], attached_pids: Vec::new(), namespace_id,
        });
        Some(id)
    }

    fn unlink(&mut self, name: &str, namespace_id: usize) -> bool {
        if let Some(pos) = self.segments.iter().position(|s| s.name == name && s.namespace_id == namespace_id) {
            self.segments.remove(pos);
            true
        } else {
            false
        }
    }

    fn attach(&mut self, id: usize, pid: usize) -> bool {
        for seg in self.segments.iter_mut() {
            if seg.id == id {
                if !seg.attached_pids.contains(&pid) {
                    seg.attached_pids.push(pid);
                }
                return true;
            }
        }
        false
    }

    fn detach(&mut self, id: usize, pid: usize) -> bool {
        for seg in self.segments.iter_mut() {
            if seg.id == id {
                if let Some(pos) = seg.attached_pids.iter().position(|&p| p == pid) {
                    seg.attached_pids.remove(pos);
                    return true;
                }
                return false;
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Eventfd
// ---------------------------------------------------------------------------

/// An event file descriptor for event notification.
struct EventFd {
    id: usize,
    counter: u64,
    pid: usize,
    semaphore_mode: bool,
}

static EVENTFDS: Mutex<EventFdTable> = Mutex::new(EventFdTable::new());

struct EventFdTable {
    fds: Vec<EventFd>,
    next_id: usize,
}

impl EventFdTable {
    const fn new() -> Self {
        Self { fds: Vec::new(), next_id: 1 }
    }

    fn create(&mut self, pid: usize, init_val: u64, semaphore_mode: bool) -> Option<usize> {
        if self.fds.len() >= MAX_EVENTFDS { return None; }
        let id = self.next_id;
        self.next_id += 1;
        self.fds.push(EventFd { id, counter: init_val, pid, semaphore_mode });
        Some(id)
    }

    fn write(&mut self, id: usize, value: u64) -> bool {
        for fd in self.fds.iter_mut() {
            if fd.id == id {
                fd.counter = fd.counter.saturating_add(value);
                return true;
            }
        }
        false
    }

    fn read(&mut self, id: usize) -> Option<u64> {
        for fd in self.fds.iter_mut() {
            if fd.id == id {
                if fd.counter == 0 { return None; } // would block
                if fd.semaphore_mode {
                    fd.counter -= 1;
                    return Some(1);
                }
                let val = fd.counter;
                fd.counter = 0;
                return Some(val);
            }
        }
        None
    }

    fn close(&mut self, id: usize) -> bool {
        if let Some(pos) = self.fds.iter().position(|f| f.id == id) {
            self.fds.remove(pos);
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Signalfd
// ---------------------------------------------------------------------------

/// A signal file descriptor for receiving signals as readable events.
struct SignalFd {
    id: usize,
    pid: usize,
    /// Mask of signal numbers being watched.
    signal_mask: u64,
    /// Pending signals delivered to this fd.
    pending: Vec<u32>,
}

static SIGNALFDS: Mutex<SignalFdTable> = Mutex::new(SignalFdTable::new());

struct SignalFdTable {
    fds: Vec<SignalFd>,
    next_id: usize,
}

impl SignalFdTable {
    const fn new() -> Self {
        Self { fds: Vec::new(), next_id: 1 }
    }

    fn create(&mut self, pid: usize, signal_mask: u64) -> Option<usize> {
        if self.fds.len() >= MAX_SIGNALFDS { return None; }
        let id = self.next_id;
        self.next_id += 1;
        self.fds.push(SignalFd { id, pid, signal_mask, pending: Vec::new() });
        Some(id)
    }

    fn deliver(&mut self, pid: usize, signal: u32) -> bool {
        let mut delivered = false;
        for fd in self.fds.iter_mut() {
            if fd.pid == pid && (fd.signal_mask & (1u64 << signal)) != 0 {
                fd.pending.push(signal);
                delivered = true;
            }
        }
        delivered
    }

    fn read(&mut self, id: usize) -> Option<u32> {
        for fd in self.fds.iter_mut() {
            if fd.id == id && !fd.pending.is_empty() {
                return Some(fd.pending.remove(0));
            }
        }
        None
    }

    fn close(&mut self, id: usize) -> bool {
        if let Some(pos) = self.fds.iter().position(|f| f.id == id) {
            self.fds.remove(pos);
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Timerfd
// ---------------------------------------------------------------------------

/// A timer file descriptor that delivers timer expirations.
struct TimerFd {
    id: usize,
    pid: usize,
    /// Interval in ticks (0 = one-shot).
    interval_ticks: u64,
    /// Next expiration tick.
    next_expiry: u64,
    /// Number of unread expirations.
    expirations: u64,
    /// Whether the timer is armed.
    armed: bool,
}

static TIMERFDS: Mutex<TimerFdTable> = Mutex::new(TimerFdTable::new());

struct TimerFdTable {
    fds: Vec<TimerFd>,
    next_id: usize,
}

impl TimerFdTable {
    const fn new() -> Self {
        Self { fds: Vec::new(), next_id: 1 }
    }

    fn create(&mut self, pid: usize) -> Option<usize> {
        if self.fds.len() >= MAX_TIMERFDS { return None; }
        let id = self.next_id;
        self.next_id += 1;
        self.fds.push(TimerFd {
            id, pid, interval_ticks: 0, next_expiry: 0,
            expirations: 0, armed: false,
        });
        Some(id)
    }

    fn arm(&mut self, id: usize, initial_ticks: u64, interval_ticks: u64, now: u64) -> bool {
        for fd in self.fds.iter_mut() {
            if fd.id == id {
                fd.next_expiry = now + initial_ticks;
                fd.interval_ticks = interval_ticks;
                fd.expirations = 0;
                fd.armed = true;
                return true;
            }
        }
        false
    }

    fn tick(&mut self, now: u64) {
        for fd in self.fds.iter_mut() {
            if fd.armed && now >= fd.next_expiry {
                fd.expirations += 1;
                if fd.interval_ticks > 0 {
                    fd.next_expiry = now + fd.interval_ticks;
                } else {
                    fd.armed = false;
                }
            }
        }
    }

    fn read(&mut self, id: usize) -> Option<u64> {
        for fd in self.fds.iter_mut() {
            if fd.id == id && fd.expirations > 0 {
                let val = fd.expirations;
                fd.expirations = 0;
                return Some(val);
            }
        }
        None
    }

    fn close(&mut self, id: usize) -> bool {
        if let Some(pos) = self.fds.iter().position(|f| f.id == id) {
            self.fds.remove(pos);
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Epoll
// ---------------------------------------------------------------------------

/// Events that can be monitored by epoll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpollEvent {
    /// Data available for reading.
    In,
    /// Ready for writing.
    Out,
    /// Error condition.
    Err,
    /// Hang up.
    Hup,
}

/// A file descriptor registered with an epoll instance.
#[derive(Debug, Clone)]
struct EpollEntry {
    fd: usize,
    events: Vec<EpollEvent>,
}

/// An epoll instance for I/O event multiplexing.
struct EpollInstance {
    id: usize,
    pid: usize,
    entries: Vec<EpollEntry>,
}

static EPOLLS: Mutex<EpollTable> = Mutex::new(EpollTable::new());

struct EpollTable {
    instances: Vec<EpollInstance>,
    next_id: usize,
}

impl EpollTable {
    const fn new() -> Self {
        Self { instances: Vec::new(), next_id: 1 }
    }

    fn create(&mut self, pid: usize) -> Option<usize> {
        if self.instances.len() >= MAX_EPOLLS { return None; }
        let id = self.next_id;
        self.next_id += 1;
        self.instances.push(EpollInstance { id, pid, entries: Vec::new() });
        Some(id)
    }

    fn add_fd(&mut self, epoll_id: usize, fd: usize, events: Vec<EpollEvent>) -> bool {
        for inst in self.instances.iter_mut() {
            if inst.id == epoll_id {
                if inst.entries.len() >= MAX_EPOLL_FDS { return false; }
                if inst.entries.iter().any(|e| e.fd == fd) { return false; }
                inst.entries.push(EpollEntry { fd, events });
                return true;
            }
        }
        false
    }

    fn remove_fd(&mut self, epoll_id: usize, fd: usize) -> bool {
        for inst in self.instances.iter_mut() {
            if inst.id == epoll_id {
                if let Some(pos) = inst.entries.iter().position(|e| e.fd == fd) {
                    inst.entries.remove(pos);
                    return true;
                }
                return false;
            }
        }
        false
    }

    fn wait(&self, epoll_id: usize) -> Vec<(usize, EpollEvent)> {
        // In a real kernel this would block; here we return all fds as
        // ready with EpollEvent::In for simulation purposes.
        let mut ready = Vec::new();
        for inst in &self.instances {
            if inst.id == epoll_id {
                for entry in &inst.entries {
                    if let Some(evt) = entry.events.first() {
                        ready.push((entry.fd, *evt));
                    }
                }
                break;
            }
        }
        ready
    }

    fn close(&mut self, id: usize) -> bool {
        if let Some(pos) = self.instances.iter().position(|i| i.id == id) {
            self.instances.remove(pos);
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// IPC namespace
// ---------------------------------------------------------------------------

/// An IPC namespace for container-level isolation.
struct IpcNamespace {
    id: usize,
    name: String,
    /// PIDs belonging to this namespace.
    pids: Vec<usize>,
}

static IPC_NAMESPACES: Mutex<NamespaceTable> = Mutex::new(NamespaceTable::new());

struct NamespaceTable {
    namespaces: Vec<IpcNamespace>,
    next_id: usize,
}

impl NamespaceTable {
    const fn new() -> Self {
        // Namespace 0 is the default/global namespace (created lazily).
        Self { namespaces: Vec::new(), next_id: 1 }
    }

    fn create(&mut self, name: &str) -> Option<usize> {
        if self.namespaces.len() >= MAX_IPC_NAMESPACES { return None; }
        let id = self.next_id;
        self.next_id += 1;
        self.namespaces.push(IpcNamespace {
            id, name: String::from(name), pids: Vec::new(),
        });
        Some(id)
    }

    fn add_pid(&mut self, ns_id: usize, pid: usize) -> bool {
        for ns in self.namespaces.iter_mut() {
            if ns.id == ns_id {
                if !ns.pids.contains(&pid) {
                    ns.pids.push(pid);
                }
                return true;
            }
        }
        false
    }

    fn remove_pid(&mut self, ns_id: usize, pid: usize) -> bool {
        for ns in self.namespaces.iter_mut() {
            if ns.id == ns_id {
                if let Some(pos) = ns.pids.iter().position(|&p| p == pid) {
                    ns.pids.remove(pos);
                    return true;
                }
                return false;
            }
        }
        false
    }

    fn destroy(&mut self, ns_id: usize) -> bool {
        if let Some(pos) = self.namespaces.iter().position(|n| n.id == ns_id) {
            self.namespaces.remove(pos);
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Global statistics
// ---------------------------------------------------------------------------

static TOTAL_MESSAGES_SENT: AtomicU64 = AtomicU64::new(0);
static TOTAL_MESSAGES_RECV: AtomicU64 = AtomicU64::new(0);
static TOTAL_SEM_OPS: AtomicU64 = AtomicU64::new(0);
static TOTAL_SOCKET_OPS: AtomicU64 = AtomicU64::new(0);
static INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Public API — Unix domain sockets
// ---------------------------------------------------------------------------

/// Create a new Unix domain socket.  Returns the socket ID.
pub fn socket_create(pid: usize) -> Option<usize> {
    let result = UNIX_SOCKETS.lock().create(pid, 0);
    if result.is_some() {
        TOTAL_SOCKET_OPS.fetch_add(1, Ordering::Relaxed);
        crate::serial_println!("[ipc_ext] socket created for pid {}", pid);
    }
    result
}

/// Bind a socket to an address (path).
pub fn socket_bind(id: usize, address: &str) -> bool {
    let mut table = UNIX_SOCKETS.lock();
    if let Some(sock) = table.find_mut(id) {
        if sock.state != SocketState::Created { return false; }
        sock.address = String::from(address);
        sock.state = SocketState::Bound;
        crate::serial_println!("[ipc_ext] socket {} bound to '{}'", id, address);
        true
    } else {
        false
    }
}

/// Start listening on a bound socket.
pub fn socket_listen(id: usize, backlog: usize) -> bool {
    let mut table = UNIX_SOCKETS.lock();
    if let Some(sock) = table.find_mut(id) {
        if sock.state != SocketState::Bound { return false; }
        sock.state = SocketState::Listening;
        sock.max_backlog = backlog;
        crate::serial_println!("[ipc_ext] socket {} listening (backlog={})", id, backlog);
        true
    } else {
        false
    }
}

/// Accept a pending connection on a listening socket.
/// Returns the new connected socket ID, or None if no connections pending.
pub fn socket_accept(id: usize) -> Option<usize> {
    let mut table = UNIX_SOCKETS.lock();
    // Extract the pending client socket ID first.
    let client_id = {
        let sock = table.find_mut(id)?;
        if sock.state != SocketState::Listening { return None; }
        if sock.backlog.is_empty() { return None; }
        sock.backlog.remove(0)
    };
    // Create a new server-side socket and connect both ends.
    let pid = table.find_mut(id)?.pid;
    let ns = table.find_mut(id)?.namespace_id;
    let server_id = table.create(pid, ns)?;
    if let Some(server_sock) = table.find_mut(server_id) {
        server_sock.state = SocketState::Connected;
        server_sock.peer_id = Some(client_id);
    }
    if let Some(client_sock) = table.find_mut(client_id) {
        client_sock.state = SocketState::Connected;
        client_sock.peer_id = Some(server_id);
    }
    TOTAL_SOCKET_OPS.fetch_add(1, Ordering::Relaxed);
    Some(server_id)
}

/// Connect a socket to a listening socket at the given address.
pub fn socket_connect(id: usize, address: &str) -> bool {
    let mut table = UNIX_SOCKETS.lock();
    let ns = match table.find_mut(id) {
        Some(s) => s.namespace_id,
        None => return false,
    };
    let listener_id = match table.find_by_addr(address, ns) {
        Some(lid) => lid,
        None => return false,
    };
    // Add to listener's backlog.
    if let Some(listener) = table.find_mut(listener_id) {
        if listener.state != SocketState::Listening { return false; }
        if listener.backlog.len() >= listener.max_backlog { return false; }
        listener.backlog.push(id);
    }
    TOTAL_SOCKET_OPS.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[ipc_ext] socket {} connecting to '{}'", id, address);
    true
}

/// Send data through a connected socket.
pub fn socket_send(id: usize, data: &[u8]) -> bool {
    let mut table = UNIX_SOCKETS.lock();
    let peer_id = match table.find_mut(id) {
        Some(s) if s.state == SocketState::Connected => s.peer_id,
        _ => return false,
    };
    if let Some(pid) = peer_id {
        if let Some(peer) = table.find_mut(pid) {
            peer.recv_buf.push(data.to_vec());
            TOTAL_MESSAGES_SENT.fetch_add(1, Ordering::Relaxed);
            return true;
        }
    }
    false
}

/// Receive data from a connected socket.
pub fn socket_recv(id: usize) -> Option<Vec<u8>> {
    let mut table = UNIX_SOCKETS.lock();
    if let Some(sock) = table.find_mut(id) {
        if sock.state != SocketState::Connected { return None; }
        if sock.recv_buf.is_empty() { return None; }
        TOTAL_MESSAGES_RECV.fetch_add(1, Ordering::Relaxed);
        Some(sock.recv_buf.remove(0))
    } else {
        None
    }
}

/// Close a socket.
pub fn socket_close(id: usize) -> bool {
    UNIX_SOCKETS.lock().remove(id)
}

// ---------------------------------------------------------------------------
// Public API — message queues
// ---------------------------------------------------------------------------

/// Open (or create) a named message queue.
pub fn mq_open(name: &str) -> Option<usize> {
    let result = MESSAGE_QUEUES.lock().open(name, 0);
    if let Some(id) = result {
        crate::serial_println!("[ipc_ext] mq '{}' opened (id={})", name, id);
    }
    result
}

/// Send a message to a queue with the given priority.
pub fn mq_send(id: usize, data: &[u8], priority: u32) -> bool {
    let ok = MESSAGE_QUEUES.lock().send(id, data, priority);
    if ok {
        TOTAL_MESSAGES_SENT.fetch_add(1, Ordering::Relaxed);
    }
    ok
}

/// Receive the highest-priority message from a queue.
pub fn mq_receive(id: usize) -> Option<(Vec<u8>, u32)> {
    let result = MESSAGE_QUEUES.lock().receive(id);
    if result.is_some() {
        TOTAL_MESSAGES_RECV.fetch_add(1, Ordering::Relaxed);
    }
    result
}

/// Close and destroy a message queue.
pub fn mq_close(id: usize) -> bool {
    MESSAGE_QUEUES.lock().close(id)
}

// ---------------------------------------------------------------------------
// Public API — semaphores
// ---------------------------------------------------------------------------

/// Create or open a named semaphore with the given initial value.
pub fn sem_init(name: &str, value: i32) -> Option<usize> {
    let result = SEMAPHORES.lock().init(name, value, 0);
    if let Some(id) = result {
        crate::serial_println!("[ipc_ext] semaphore '{}' initialized (id={}, val={})", name, id, value);
    }
    result
}

/// Decrement (wait on) a semaphore.  Returns true if acquired, false if
/// it would block.
pub fn sem_wait(id: usize) -> bool {
    let ok = SEMAPHORES.lock().wait(id);
    if ok {
        TOTAL_SEM_OPS.fetch_add(1, Ordering::Relaxed);
    }
    ok
}

/// Try to decrement a semaphore without blocking.
pub fn sem_trywait(id: usize) -> bool {
    let ok = SEMAPHORES.lock().trywait(id);
    if ok {
        TOTAL_SEM_OPS.fetch_add(1, Ordering::Relaxed);
    }
    ok
}

/// Increment (post) a semaphore.
pub fn sem_post(id: usize) -> bool {
    let ok = SEMAPHORES.lock().post(id);
    if ok {
        TOTAL_SEM_OPS.fetch_add(1, Ordering::Relaxed);
    }
    ok
}

/// Destroy a semaphore.
pub fn sem_destroy(id: usize) -> bool {
    SEMAPHORES.lock().destroy(id)
}

// ---------------------------------------------------------------------------
// Public API — POSIX shared memory
// ---------------------------------------------------------------------------

/// Open (or create) a named shared memory segment.
pub fn shm_open(name: &str, size: usize) -> Option<usize> {
    SHM_SEGMENTS.lock().open(name, size, 0)
}

/// Unlink (destroy) a named shared memory segment.
pub fn shm_unlink(name: &str) -> bool {
    SHM_SEGMENTS.lock().unlink(name, 0)
}

/// Attach a process to a shared memory segment.
pub fn shm_attach(id: usize, pid: usize) -> bool {
    SHM_SEGMENTS.lock().attach(id, pid)
}

/// Detach a process from a shared memory segment.
pub fn shm_detach(id: usize, pid: usize) -> bool {
    SHM_SEGMENTS.lock().detach(id, pid)
}

// ---------------------------------------------------------------------------
// Public API — eventfd
// ---------------------------------------------------------------------------

/// Create an event file descriptor.
pub fn eventfd_create(pid: usize, init_val: u64, semaphore_mode: bool) -> Option<usize> {
    EVENTFDS.lock().create(pid, init_val, semaphore_mode)
}

/// Write (signal) to an eventfd.
pub fn eventfd_write(id: usize, value: u64) -> bool {
    EVENTFDS.lock().write(id, value)
}

/// Read from an eventfd.  Returns None if counter is zero (would block).
pub fn eventfd_read(id: usize) -> Option<u64> {
    EVENTFDS.lock().read(id)
}

/// Close an eventfd.
pub fn eventfd_close(id: usize) -> bool {
    EVENTFDS.lock().close(id)
}

// ---------------------------------------------------------------------------
// Public API — signalfd
// ---------------------------------------------------------------------------

/// Create a signal file descriptor watching the given signal mask.
pub fn signalfd_create(pid: usize, signal_mask: u64) -> Option<usize> {
    SIGNALFDS.lock().create(pid, signal_mask)
}

/// Deliver a signal to all matching signalfds for a process.
pub fn signalfd_deliver(pid: usize, signal: u32) -> bool {
    SIGNALFDS.lock().deliver(pid, signal)
}

/// Read the next pending signal from a signalfd.
pub fn signalfd_read(id: usize) -> Option<u32> {
    SIGNALFDS.lock().read(id)
}

/// Close a signalfd.
pub fn signalfd_close(id: usize) -> bool {
    SIGNALFDS.lock().close(id)
}

// ---------------------------------------------------------------------------
// Public API — timerfd
// ---------------------------------------------------------------------------

/// Create a timer file descriptor.
pub fn timerfd_create(pid: usize) -> Option<usize> {
    TIMERFDS.lock().create(pid)
}

/// Arm a timer with initial and interval ticks.
pub fn timerfd_arm(id: usize, initial_ticks: u64, interval_ticks: u64, now: u64) -> bool {
    TIMERFDS.lock().arm(id, initial_ticks, interval_ticks, now)
}

/// Advance all timers (called from timer interrupt).
pub fn timerfd_tick(now: u64) {
    TIMERFDS.lock().tick(now);
}

/// Read expiration count from a timerfd.
pub fn timerfd_read(id: usize) -> Option<u64> {
    TIMERFDS.lock().read(id)
}

/// Close a timerfd.
pub fn timerfd_close(id: usize) -> bool {
    TIMERFDS.lock().close(id)
}

// ---------------------------------------------------------------------------
// Public API — epoll
// ---------------------------------------------------------------------------

/// Create an epoll instance.
pub fn epoll_create(pid: usize) -> Option<usize> {
    EPOLLS.lock().create(pid)
}

/// Add a file descriptor to an epoll instance.
pub fn epoll_add(epoll_id: usize, fd: usize, events: Vec<EpollEvent>) -> bool {
    EPOLLS.lock().add_fd(epoll_id, fd, events)
}

/// Remove a file descriptor from an epoll instance.
pub fn epoll_remove(epoll_id: usize, fd: usize) -> bool {
    EPOLLS.lock().remove_fd(epoll_id, fd)
}

/// Wait for events on an epoll instance.  Returns ready file descriptors.
pub fn epoll_wait(epoll_id: usize) -> Vec<(usize, EpollEvent)> {
    EPOLLS.lock().wait(epoll_id)
}

/// Close an epoll instance.
pub fn epoll_close(id: usize) -> bool {
    EPOLLS.lock().close(id)
}

// ---------------------------------------------------------------------------
// Public API — IPC namespaces
// ---------------------------------------------------------------------------

/// Create a new IPC namespace for container isolation.
pub fn namespace_create(name: &str) -> Option<usize> {
    let result = IPC_NAMESPACES.lock().create(name);
    if let Some(id) = result {
        crate::serial_println!("[ipc_ext] namespace '{}' created (id={})", name, id);
    }
    result
}

/// Add a process to an IPC namespace.
pub fn namespace_add_pid(ns_id: usize, pid: usize) -> bool {
    IPC_NAMESPACES.lock().add_pid(ns_id, pid)
}

/// Remove a process from an IPC namespace.
pub fn namespace_remove_pid(ns_id: usize, pid: usize) -> bool {
    IPC_NAMESPACES.lock().remove_pid(ns_id, pid)
}

/// Destroy an IPC namespace.
pub fn namespace_destroy(ns_id: usize) -> bool {
    IPC_NAMESPACES.lock().destroy(ns_id)
}

// ---------------------------------------------------------------------------
// Public API — statistics and info
// ---------------------------------------------------------------------------

/// Return IPC statistics as a formatted string.
pub fn ipc_ext_info() -> String {
    let sockets = UNIX_SOCKETS.lock();
    let mqs = MESSAGE_QUEUES.lock();
    let sems = SEMAPHORES.lock();
    let shm = SHM_SEGMENTS.lock();
    let efds = EVENTFDS.lock();
    let sfds = SIGNALFDS.lock();
    let tfds = TIMERFDS.lock();
    let epolls = EPOLLS.lock();
    let ns = IPC_NAMESPACES.lock();

    let mut out = String::from("=== Extended IPC Status ===\n");
    out.push_str(&format!("Unix sockets: {}\n", sockets.sockets.len()));
    out.push_str(&format!("Message queues: {}\n", mqs.queues.len()));
    for q in &mqs.queues {
        out.push_str(&format!("  mq '{}': {} pending, sent={}, recv={}\n",
            q.name, q.messages.len(), q.total_sent, q.total_received));
    }
    out.push_str(&format!("Semaphores: {}\n", sems.sems.len()));
    for s in &sems.sems {
        out.push_str(&format!("  sem '{}': val={}, waiters={}, waits={}, posts={}\n",
            s.name, s.value, s.waiters, s.total_waits, s.total_posts));
    }
    out.push_str(&format!("Shared memory segments: {}\n", shm.segments.len()));
    out.push_str(&format!("Event FDs: {}\n", efds.fds.len()));
    out.push_str(&format!("Signal FDs: {}\n", sfds.fds.len()));
    out.push_str(&format!("Timer FDs: {}\n", tfds.fds.len()));
    out.push_str(&format!("Epoll instances: {}\n", epolls.instances.len()));
    out.push_str(&format!("IPC namespaces: {}\n", ns.namespaces.len()));
    out.push_str(&format!("Total messages sent: {}\n", TOTAL_MESSAGES_SENT.load(Ordering::Relaxed)));
    out.push_str(&format!("Total messages received: {}\n", TOTAL_MESSAGES_RECV.load(Ordering::Relaxed)));
    out.push_str(&format!("Total semaphore ops: {}\n", TOTAL_SEM_OPS.load(Ordering::Relaxed)));
    out.push_str(&format!("Total socket ops: {}\n", TOTAL_SOCKET_OPS.load(Ordering::Relaxed)));
    out
}

/// List all message queues.
pub fn mq_list_info() -> String {
    let mqs = MESSAGE_QUEUES.lock();
    if mqs.queues.is_empty() {
        return String::from("(no message queues)");
    }
    let mut out = String::new();
    for q in &mqs.queues {
        out.push_str(&format!("mq '{}': {} pending, sent={}, recv={}\n",
            q.name, q.messages.len(), q.total_sent, q.total_received));
    }
    out
}

/// List all semaphores.
pub fn sem_list_info() -> String {
    let sems = SEMAPHORES.lock();
    if sems.sems.is_empty() {
        return String::from("(no semaphores)");
    }
    let mut out = String::new();
    for s in &sems.sems {
        out.push_str(&format!("sem '{}': val={}, waiters={}, waits={}, posts={}\n",
            s.name, s.value, s.waiters, s.total_waits, s.total_posts));
    }
    out
}

/// Initialise the extended IPC subsystem.
pub fn init() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::serial_println!("[ipc_ext] extended IPC subsystem initialized");
    crate::klog_println!("[ipc_ext] unix sockets, mqueues, semaphores, epoll ready");
}
