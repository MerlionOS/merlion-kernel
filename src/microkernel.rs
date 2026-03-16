/// Microkernel mode for MerlionOS.
/// Provides an optional execution mode where drivers and services run as
/// isolated server processes, communicating via IPC message passing.
/// Supports fault isolation (crashed servers don't bring down the kernel)
/// and hot-restart of failed services.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Microkernel mode toggle
// ---------------------------------------------------------------------------

static MICROKERNEL_MODE: AtomicBool = AtomicBool::new(false);

/// Enable microkernel execution mode.
pub fn enable_microkernel_mode() {
    MICROKERNEL_MODE.store(true, Ordering::SeqCst);
}

/// Disable microkernel execution mode (return to monolithic).
pub fn disable_microkernel_mode() {
    MICROKERNEL_MODE.store(false, Ordering::SeqCst);
}

/// Check whether microkernel mode is active.
pub fn is_microkernel_mode() -> bool {
    MICROKERNEL_MODE.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Service / Server model
// ---------------------------------------------------------------------------

/// The kind of service running as a microkernel server.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ServiceType {
    Driver,
    Filesystem,
    Network,
    Security,
    Logger,
    Custom,
}

/// Lifecycle state of a service.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ServiceState {
    Stopped,
    Starting,
    Running,
    Crashed,
    Restarting,
}

/// A microkernel service (server process).
#[derive(Debug, Clone)]
pub struct Service {
    pub id: u32,
    pub name: String,
    pub service_type: ServiceType,
    pub state: ServiceState,
    pub pid: usize,
    pub restart_count: u32,
    pub max_restarts: u32,
    pub started_tick: u64,
    pub crashed_tick: u64,
    pub messages_sent: u64,
    pub messages_received: u64,
    pub faults: u32,
    pub dependencies: Vec<u32>,
}

// ---------------------------------------------------------------------------
// IPC message passing
// ---------------------------------------------------------------------------

/// IPC message type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MsgType {
    Request,
    Response,
    Notification,
    Error,
}

/// An IPC message exchanged between services.
#[derive(Debug, Clone)]
pub struct IpcMessage {
    pub id: u64,
    pub from_service: u32,
    pub to_service: u32,
    pub msg_type: MsgType,
    pub payload: Vec<u8>,
    pub timestamp: u64,
    pub reply_to: Option<u64>,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

const MAX_SERVICES: usize = 32;
const MAX_QUEUE_LEN: usize = 64;

static NEXT_SERVICE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_MSG_ID: AtomicU64 = AtomicU64::new(1);
static TOTAL_MESSAGES: AtomicU64 = AtomicU64::new(0);
static TOTAL_FAULTS: AtomicU64 = AtomicU64::new(0);
static TOTAL_RESTARTS: AtomicU64 = AtomicU64::new(0);

static SERVICES: Mutex<Vec<Service>> = Mutex::new(Vec::new());

/// Per-service message queue. Index corresponds to service id slot.
/// We keep a simple flat structure: Vec of (service_id, queue) pairs.
static MESSAGE_QUEUES: Mutex<Vec<(u32, Vec<IpcMessage>)>> = Mutex::new(Vec::new());

// ---------------------------------------------------------------------------
// Service registry
// ---------------------------------------------------------------------------

/// Register a new service. Returns the assigned service id.
pub fn register_service(name: &str, stype: ServiceType, max_restarts: u32) -> u32 {
    let id = NEXT_SERVICE_ID.fetch_add(1, Ordering::SeqCst) as u32;
    let svc = Service {
        id,
        name: String::from(name),
        service_type: stype,
        state: ServiceState::Stopped,
        pid: 0,
        restart_count: 0,
        max_restarts,
        started_tick: 0,
        crashed_tick: 0,
        messages_sent: 0,
        messages_received: 0,
        faults: 0,
        dependencies: Vec::new(),
    };
    {
        let mut services = SERVICES.lock();
        if services.len() < MAX_SERVICES {
            services.push(svc);
        }
    }
    {
        let mut queues = MESSAGE_QUEUES.lock();
        queues.push((id, Vec::new()));
    }
    id
}

/// Unregister a service by id.
pub fn unregister_service(id: u32) {
    {
        let mut services = SERVICES.lock();
        services.retain(|s| s.id != id);
    }
    {
        let mut queues = MESSAGE_QUEUES.lock();
        queues.retain(|(sid, _)| *sid != id);
    }
}

/// Start a service (transition to Running).
pub fn start_service(id: u32) -> Result<(), &'static str> {
    let mut services = SERVICES.lock();
    if let Some(svc) = services.iter_mut().find(|s| s.id == id) {
        match svc.state {
            ServiceState::Running => return Err("service already running"),
            ServiceState::Starting => return Err("service is starting"),
            _ => {}
        }
        svc.state = ServiceState::Starting;
        // In a real microkernel we would spawn a task here.
        // For now we simulate immediate start.
        svc.state = ServiceState::Running;
        svc.started_tick = crate::timer::ticks();
        svc.pid = id as usize; // placeholder pid
        Ok(())
    } else {
        Err("service not found")
    }
}

/// Stop a running service.
pub fn stop_service(id: u32) -> Result<(), &'static str> {
    let mut services = SERVICES.lock();
    if let Some(svc) = services.iter_mut().find(|s| s.id == id) {
        if svc.state == ServiceState::Stopped {
            return Err("service already stopped");
        }
        svc.state = ServiceState::Stopped;
        svc.pid = 0;
        Ok(())
    } else {
        Err("service not found")
    }
}

/// Add a dependency: `service_id` depends on `dependency_id`.
pub fn add_dependency(service_id: u32, dependency_id: u32) -> Result<(), &'static str> {
    let mut services = SERVICES.lock();
    if let Some(svc) = services.iter_mut().find(|s| s.id == service_id) {
        if !svc.dependencies.contains(&dependency_id) {
            svc.dependencies.push(dependency_id);
        }
        Ok(())
    } else {
        Err("service not found")
    }
}

/// List all registered services as a formatted string.
pub fn list_services() -> String {
    let services = SERVICES.lock();
    if services.is_empty() {
        return String::from("No services registered.\n");
    }
    let mut out = String::from("ID  NAME             TYPE        STATE       PID  RESTARTS  FAULTS\n");
    out.push_str(       "--- ---------------- ----------- ----------- ---- --------- ------\n");
    for svc in services.iter() {
        let tname = type_str(svc.service_type);
        let sname = state_str(svc.state);
        out.push_str(&format!(
            "{:<3} {:<16} {:<11} {:<11} {:<4} {:<9} {}\n",
            svc.id, svc.name, tname, sname, svc.pid, svc.restart_count, svc.faults,
        ));
    }
    out
}

/// Get detailed info for a single service.
pub fn service_info(id: u32) -> String {
    let services = SERVICES.lock();
    if let Some(svc) = services.iter().find(|s| s.id == id) {
        let deps: Vec<String> = svc.dependencies.iter().map(|d| format!("{}", d)).collect();
        let dep_str = if deps.is_empty() {
            String::from("none")
        } else {
            deps.join(", ")
        };
        format!(
            "Service #{}\n  name:         {}\n  type:         {}\n  state:        {}\n  pid:          {}\n  restarts:     {}/{}\n  started:      tick {}\n  crashed:      tick {}\n  msgs sent:    {}\n  msgs recv:    {}\n  faults:       {}\n  dependencies: {}\n",
            svc.id, svc.name, type_str(svc.service_type), state_str(svc.state),
            svc.pid, svc.restart_count,
            if svc.max_restarts == 0 { String::from("unlimited") } else { format!("{}", svc.max_restarts) },
            svc.started_tick, svc.crashed_tick,
            svc.messages_sent, svc.messages_received,
            svc.faults, dep_str,
        )
    } else {
        format!("Service #{} not found.\n", id)
    }
}

fn type_str(t: ServiceType) -> &'static str {
    match t {
        ServiceType::Driver => "driver",
        ServiceType::Filesystem => "filesystem",
        ServiceType::Network => "network",
        ServiceType::Security => "security",
        ServiceType::Logger => "logger",
        ServiceType::Custom => "custom",
    }
}

fn state_str(s: ServiceState) -> &'static str {
    match s {
        ServiceState::Stopped => "stopped",
        ServiceState::Starting => "starting",
        ServiceState::Running => "running",
        ServiceState::Crashed => "crashed",
        ServiceState::Restarting => "restarting",
    }
}

// ---------------------------------------------------------------------------
// IPC operations
// ---------------------------------------------------------------------------

/// Send a message from one service to another. Returns the message id.
pub fn send_message(from: u32, to: u32, msg_type: MsgType, payload: Vec<u8>) -> u64 {
    let id = NEXT_MSG_ID.fetch_add(1, Ordering::SeqCst);
    let tick = crate::timer::ticks();
    let msg = IpcMessage {
        id,
        from_service: from,
        to_service: to,
        msg_type,
        payload,
        timestamp: tick,
        reply_to: None,
    };

    // Enqueue into the target service's mailbox.
    {
        let mut queues = MESSAGE_QUEUES.lock();
        if let Some((_, q)) = queues.iter_mut().find(|(sid, _)| *sid == to) {
            if q.len() < MAX_QUEUE_LEN {
                q.push(msg);
            }
            // silently drop if queue full
        }
    }

    // Update per-service counters.
    {
        let mut services = SERVICES.lock();
        if let Some(svc) = services.iter_mut().find(|s| s.id == from) {
            svc.messages_sent += 1;
        }
        if let Some(svc) = services.iter_mut().find(|s| s.id == to) {
            svc.messages_received += 1;
        }
    }

    TOTAL_MESSAGES.fetch_add(1, Ordering::SeqCst);
    id
}

/// Receive the next message for a service (FIFO).
pub fn recv_message(service_id: u32) -> Option<IpcMessage> {
    let mut queues = MESSAGE_QUEUES.lock();
    if let Some((_, q)) = queues.iter_mut().find(|(sid, _)| *sid == service_id) {
        if !q.is_empty() {
            return Some(q.remove(0));
        }
    }
    None
}

/// Reply to a message by its id. The reply is routed back to the original sender.
pub fn reply(original_msg_id: u64, from: u32, to: u32, payload: Vec<u8>) {
    let id = NEXT_MSG_ID.fetch_add(1, Ordering::SeqCst);
    let tick = crate::timer::ticks();
    let msg = IpcMessage {
        id,
        from_service: from,
        to_service: to,
        msg_type: MsgType::Response,
        payload,
        timestamp: tick,
        reply_to: Some(original_msg_id),
    };

    {
        let mut queues = MESSAGE_QUEUES.lock();
        if let Some((_, q)) = queues.iter_mut().find(|(sid, _)| *sid == to) {
            if q.len() < MAX_QUEUE_LEN {
                q.push(msg);
            }
        }
    }

    {
        let mut services = SERVICES.lock();
        if let Some(svc) = services.iter_mut().find(|s| s.id == from) {
            svc.messages_sent += 1;
        }
        if let Some(svc) = services.iter_mut().find(|s| s.id == to) {
            svc.messages_received += 1;
        }
    }

    TOTAL_MESSAGES.fetch_add(1, Ordering::SeqCst);
}

/// Return IPC statistics as a formatted string.
pub fn message_stats() -> String {
    let total = TOTAL_MESSAGES.load(Ordering::SeqCst);
    let queues = MESSAGE_QUEUES.lock();
    let pending: usize = queues.iter().map(|(_, q)| q.len()).sum();
    format!(
        "IPC Statistics\n  total messages: {}\n  pending in queues: {}\n  queue slots: {}\n",
        total, pending, queues.len(),
    )
}

// ---------------------------------------------------------------------------
// Fault isolation & hot restart
// ---------------------------------------------------------------------------

/// Handle a service crash. Marks the service as crashed and optionally triggers
/// a restart if the restart limit has not been reached.
pub fn handle_fault(service_id: u32) -> String {
    TOTAL_FAULTS.fetch_add(1, Ordering::SeqCst);
    let mut services = SERVICES.lock();
    if let Some(svc) = services.iter_mut().find(|s| s.id == service_id) {
        svc.state = ServiceState::Crashed;
        svc.crashed_tick = crate::timer::ticks();
        svc.faults += 1;
        let name = svc.name.clone();
        let faults = svc.faults;

        // Check if auto-restart is allowed.
        let can_restart = svc.max_restarts == 0 || svc.restart_count < svc.max_restarts;
        if can_restart {
            svc.state = ServiceState::Restarting;
            svc.restart_count += 1;
            svc.state = ServiceState::Running;
            svc.started_tick = crate::timer::ticks();
            TOTAL_RESTARTS.fetch_add(1, Ordering::SeqCst);
            format!(
                "Service '{}' (#{}) crashed (fault #{}). Auto-restarted (restart #{}).\n",
                name, service_id, faults, svc.restart_count,
            )
        } else {
            format!(
                "Service '{}' (#{}) crashed (fault #{}). Restart limit reached ({}).\n",
                name, service_id, faults, svc.max_restarts,
            )
        }
    } else {
        format!("Service #{} not found.\n", service_id)
    }
}

/// Manually hot-restart a crashed or stopped service.
pub fn restart_service(service_id: u32) -> Result<(), &'static str> {
    let mut services = SERVICES.lock();
    if let Some(svc) = services.iter_mut().find(|s| s.id == service_id) {
        match svc.state {
            ServiceState::Running => return Err("service is already running"),
            ServiceState::Starting | ServiceState::Restarting => {
                return Err("service is already starting")
            }
            _ => {}
        }
        svc.state = ServiceState::Restarting;
        svc.restart_count += 1;
        svc.state = ServiceState::Running;
        svc.started_tick = crate::timer::ticks();
        svc.pid = svc.id as usize;
        TOTAL_RESTARTS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    } else {
        Err("service not found")
    }
}

/// Perform a health check on all running services.
/// In a real system this would verify heartbeats; here we report state.
pub fn health_check() -> String {
    let services = SERVICES.lock();
    if services.is_empty() {
        return String::from("No services to check.\n");
    }
    let mut out = String::from("Health Check\n");
    let mut healthy = 0u32;
    let mut unhealthy = 0u32;
    for svc in services.iter() {
        let status = match svc.state {
            ServiceState::Running => {
                healthy += 1;
                "OK"
            }
            ServiceState::Crashed => {
                unhealthy += 1;
                "CRASHED"
            }
            ServiceState::Stopped => {
                "STOPPED"
            }
            _ => {
                "TRANSITIONING"
            }
        };
        out.push_str(&format!("  [{}] {} (#{}) — {} faults\n", status, svc.name, svc.id, svc.faults));
    }
    out.push_str(&format!("Summary: {} healthy, {} unhealthy\n", healthy, unhealthy));
    out
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// Return overall microkernel subsystem statistics.
pub fn ukernel_stats() -> String {
    let mode = if is_microkernel_mode() { "enabled" } else { "disabled" };
    let services = SERVICES.lock();
    let running = services.iter().filter(|s| s.state == ServiceState::Running).count();
    let crashed = services.iter().filter(|s| s.state == ServiceState::Crashed).count();
    let total_svc = services.len();
    drop(services);

    let total_msgs = TOTAL_MESSAGES.load(Ordering::SeqCst);
    let total_faults = TOTAL_FAULTS.load(Ordering::SeqCst);
    let total_restarts = TOTAL_RESTARTS.load(Ordering::SeqCst);

    format!(
        "Microkernel Statistics\n  mode:           {}\n  services:       {} total, {} running, {} crashed\n  messages:       {}\n  faults:         {}\n  restarts:       {}\n",
        mode, total_svc, running, crashed, total_msgs, total_faults, total_restarts,
    )
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the microkernel subsystem with default built-in services.
pub fn init() {
    // Register built-in server processes.
    let vfs_id = register_service("vfs-server", ServiceType::Filesystem, 0);
    let net_id = register_service("net-server", ServiceType::Network, 5);
    let log_id = register_service("log-server", ServiceType::Logger, 0);
    let sec_id = register_service("sec-server", ServiceType::Security, 3);

    // net-server depends on vfs-server.
    let _ = add_dependency(net_id, vfs_id);
    // sec-server depends on log-server.
    let _ = add_dependency(sec_id, log_id);

    // Start all default services.
    let _ = start_service(vfs_id);
    let _ = start_service(log_id);
    let _ = start_service(net_id);
    let _ = start_service(sec_id);

    // Enable microkernel mode by default on init.
    enable_microkernel_mode();
}
