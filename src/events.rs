/// Kernel event bus for MerlionOS internal pub/sub communication.
///
/// Provides a lightweight publish/subscribe mechanism so that kernel modules
/// can react to system-wide events (task lifecycle, file operations, network
/// activity, etc.) without tight coupling.  Subscribers register a callback
/// index and an optional event-kind filter; the bus delivers matching events
/// synchronously from the publisher's context.
///
/// A 64-entry ring buffer keeps a recent event log that can be formatted and
/// displayed via the shell for diagnostics.
///
/// # Example (conceptual)
///
/// ```ignore
/// events::subscribe("watchdog", Some(EventKind::PanicOccurred), 0, on_panic);
/// events::publish(Event::new(EventKind::PanicOccurred, "kernel", None));
/// ```

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use spin::Mutex;

use crate::timer;

// ---------------------------------------------------------------------------
// EventKind
// ---------------------------------------------------------------------------

/// Discriminant for the type of kernel event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    /// A new kernel task was spawned.
    TaskSpawned,
    /// A kernel task exited (normally or killed).
    TaskExited,
    /// A file was created in the VFS.
    FileCreated,
    /// A file was removed from the VFS.
    FileDeleted,
    /// A network packet was received or transmitted.
    NetworkPacket,
    /// The PIT timer fired a tick.
    TimerTick,
    /// A user logged in via the login subsystem.
    UserLogin,
    /// A user logged out.
    UserLogout,
    /// A block was written to a disk device.
    DiskWrite,
    /// Heap or physical memory pressure detected.
    MemoryWarning,
    /// A panic was caught or is about to halt the system.
    PanicOccurred,
    /// Application-defined event with a free-form tag.
    Custom(String),
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EventKind::TaskSpawned   => write!(f, "TaskSpawned"),
            EventKind::TaskExited    => write!(f, "TaskExited"),
            EventKind::FileCreated   => write!(f, "FileCreated"),
            EventKind::FileDeleted   => write!(f, "FileDeleted"),
            EventKind::NetworkPacket => write!(f, "NetworkPacket"),
            EventKind::TimerTick     => write!(f, "TimerTick"),
            EventKind::UserLogin     => write!(f, "UserLogin"),
            EventKind::UserLogout    => write!(f, "UserLogout"),
            EventKind::DiskWrite     => write!(f, "DiskWrite"),
            EventKind::MemoryWarning => write!(f, "MemoryWarning"),
            EventKind::PanicOccurred => write!(f, "PanicOccurred"),
            EventKind::Custom(tag)   => write!(f, "Custom({})", tag),
        }
    }
}

// ---------------------------------------------------------------------------
// Event
// ---------------------------------------------------------------------------

/// A single kernel event.
#[derive(Debug, Clone)]
pub struct Event {
    /// What happened.
    pub kind: EventKind,
    /// PIT tick at the moment the event was published.
    pub timestamp_ticks: u64,
    /// Kernel module that published the event (e.g. `"task"`, `"vfs"`).
    pub source_module: &'static str,
    /// Optional payload or description.
    pub data: Option<String>,
}

impl Event {
    /// Create a new event stamped with the current timer tick.
    pub fn new(kind: EventKind, source_module: &'static str, data: Option<String>) -> Self {
        Self {
            kind,
            timestamp_ticks: timer::ticks(),
            source_module,
            data,
        }
    }
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.data {
            Some(d) => write!(
                f,
                "[{:>8}] {} {}: {}",
                self.timestamp_ticks, self.kind, self.source_module, d,
            ),
            None => write!(
                f,
                "[{:>8}] {} {}",
                self.timestamp_ticks, self.kind, self.source_module,
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Subscriber
// ---------------------------------------------------------------------------

/// Type alias for subscriber callback functions.
///
/// The callback receives a reference to the published event.
pub type HandlerFn = fn(&Event);

/// A registered subscriber on the event bus.
struct Subscriber {
    /// Human-readable name used for identification and unsubscription.
    name: &'static str,
    /// If `Some`, only events matching this kind are delivered.
    filter: Option<EventKind>,
    /// Opaque index the subscriber can use to distinguish handlers.
    callback_index: usize,
    /// Function invoked when a matching event is published.
    handler: HandlerFn,
}

// ---------------------------------------------------------------------------
// EventLog — ring buffer of the last 64 events
// ---------------------------------------------------------------------------

/// Capacity of the recent-event ring buffer.
const EVENT_LOG_CAPACITY: usize = 64;

/// Ring buffer that retains the most recent [`EVENT_LOG_CAPACITY`] events.
struct EventLog {
    entries: Vec<Event>,
    write_pos: usize,
    total_written: usize,
}

impl EventLog {
    /// Create an empty event log.
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            write_pos: 0,
            total_written: 0,
        }
    }

    /// Record an event, overwriting the oldest entry when full.
    fn push(&mut self, event: Event) {
        if self.entries.len() < EVENT_LOG_CAPACITY {
            self.entries.push(event);
        } else {
            self.entries[self.write_pos] = event;
        }
        self.write_pos = (self.write_pos + 1) % EVENT_LOG_CAPACITY;
        self.total_written += 1;
    }

    /// Return all stored events in chronological order.
    fn all(&self) -> Vec<Event> {
        let len = self.entries.len();
        if len == 0 {
            return Vec::new();
        }
        let mut result = Vec::with_capacity(len);
        let start = if len < EVENT_LOG_CAPACITY {
            0
        } else {
            self.write_pos
        };
        for i in 0..len {
            let idx = (start + i) % len;
            result.push(self.entries[idx].clone());
        }
        result
    }
}

// ---------------------------------------------------------------------------
// EventBus (global state)
// ---------------------------------------------------------------------------

/// Global subscriber list, protected by a spin mutex.
static SUBSCRIBERS: Mutex<Vec<Subscriber>> = Mutex::new(Vec::new());

/// Global event log (ring buffer), protected by a spin mutex.
static EVENT_LOG: Mutex<EventLog> = Mutex::new(EventLog::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a subscriber on the event bus.
///
/// * `name` — unique identifier; used later to [`unsubscribe`].
/// * `filter` — if `Some(kind)`, only events of that kind trigger the callback.
///   Pass `None` to receive every event.
/// * `callback_index` — opaque value stored alongside the subscriber.
/// * `handler` — function pointer called synchronously when a matching event
///   is published.
pub fn subscribe(
    name: &'static str,
    filter: Option<EventKind>,
    callback_index: usize,
    handler: HandlerFn,
) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        SUBSCRIBERS.lock().push(Subscriber {
            name,
            filter,
            callback_index,
            handler,
        });
    });
}

/// Publish an event to all matching subscribers and record it in the log.
///
/// Delivery is synchronous: each subscriber callback runs in the caller's
/// context before `publish` returns.  Subscribers whose filter does not
/// match the event kind are silently skipped.
pub fn publish(event: Event) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        // Deliver to subscribers.
        let subs = SUBSCRIBERS.lock();
        for sub in subs.iter() {
            let dominated = match &sub.filter {
                None => true,
                Some(ref k) => *k == event.kind,
            };
            if dominated {
                (sub.handler)(&event);
            }
        }
    });

    // Record in the ring buffer (separate lock scope to avoid nesting).
    x86_64::instructions::interrupts::without_interrupts(|| {
        EVENT_LOG.lock().push(event);
    });
}

/// Remove all subscribers registered under `name`.
///
/// Returns the number of subscribers that were removed.
pub fn unsubscribe(name: &'static str) -> usize {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut subs = SUBSCRIBERS.lock();
        let before = subs.len();
        subs.retain(|s| s.name != name);
        before - subs.len()
    })
}

/// Format the recent event log as a human-readable string.
///
/// Each line has the form:
/// ```text
/// [     420] TaskSpawned task: pid=3 "shell"
/// [     421] FileCreated vfs: /tmp/out.txt
/// ```
pub fn format_event_log() -> String {
    use core::fmt::Write;

    let events = x86_64::instructions::interrupts::without_interrupts(|| {
        EVENT_LOG.lock().all()
    });

    let mut out = String::new();
    if events.is_empty() {
        let _ = out.write_str("(no events recorded)\n");
        return out;
    }
    for ev in &events {
        let _ = writeln!(out, "{}", ev);
    }
    out
}

/// Return the total number of events published since boot.
pub fn total_published() -> usize {
    x86_64::instructions::interrupts::without_interrupts(|| {
        EVENT_LOG.lock().total_written
    })
}

/// Return the current number of registered subscribers.
pub fn subscriber_count() -> usize {
    x86_64::instructions::interrupts::without_interrupts(|| {
        SUBSCRIBERS.lock().len()
    })
}
