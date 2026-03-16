/// Service manager for MerlionOS (systemd-like).
/// Manages system services with dependency ordering, parallel startup,
/// restart policies, and service status monitoring.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use alloc::format;
use alloc::borrow::ToOwned;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::{timer, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum restart attempts before marking a service as failed.
const MAX_RESTART_RETRIES: u32 = 5;

/// Default restart delay in timer ticks (100 Hz → 500 ticks = 5 seconds).
const DEFAULT_RESTART_DELAY: u64 = 500;

// ---------------------------------------------------------------------------
// Service type
// ---------------------------------------------------------------------------

/// How the service process behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceType {
    /// Main process stays running; service is "up" while it runs.
    Simple,
    /// Process forks; parent exits, child stays.
    Forking,
    /// Process runs once and exits; considered successful on exit 0.
    Oneshot,
    /// Like Simple but the service notifies readiness explicitly.
    Notify,
}

impl ServiceType {
    fn as_str(self) -> &'static str {
        match self {
            ServiceType::Simple  => "simple",
            ServiceType::Forking => "forking",
            ServiceType::Oneshot => "oneshot",
            ServiceType::Notify  => "notify",
        }
    }
}

// ---------------------------------------------------------------------------
// Service state
// ---------------------------------------------------------------------------

/// Current state of a service unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Inactive,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

impl ServiceState {
    fn as_str(self) -> &'static str {
        match self {
            ServiceState::Inactive  => "inactive",
            ServiceState::Starting  => "starting",
            ServiceState::Running   => "running",
            ServiceState::Stopping  => "stopping",
            ServiceState::Stopped   => "stopped",
            ServiceState::Failed    => "failed",
        }
    }
}

// ---------------------------------------------------------------------------
// Restart policy
// ---------------------------------------------------------------------------

/// When to restart a failed service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    /// Always restart regardless of exit status.
    Always,
    /// Restart only on non-zero exit or signal.
    OnFailure,
    /// Restart on signal/timeout but not clean exit.
    OnAbnormal,
    /// Never restart.
    Never,
}

impl RestartPolicy {
    fn as_str(self) -> &'static str {
        match self {
            RestartPolicy::Always    => "always",
            RestartPolicy::OnFailure => "on-failure",
            RestartPolicy::OnAbnormal=> "on-abnormal",
            RestartPolicy::Never     => "never",
        }
    }
}

// ---------------------------------------------------------------------------
// Dependency relationship
// ---------------------------------------------------------------------------

/// Dependency types between service units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    /// Must start after this unit.
    After,
    /// Hard dependency — failure propagates.
    Requires,
    /// Soft dependency — best-effort.
    Wants,
}

/// A dependency edge.
#[derive(Debug, Clone)]
pub struct Dependency {
    pub target: String,
    pub kind: DepKind,
}

// ---------------------------------------------------------------------------
// Target unit
// ---------------------------------------------------------------------------

/// A target groups related services (like a runlevel).
#[derive(Debug, Clone)]
pub struct TargetUnit {
    pub name: String,
    pub description: String,
    /// Services that belong to this target.
    pub members: Vec<String>,
    pub active: bool,
}

// ---------------------------------------------------------------------------
// Timer unit
// ---------------------------------------------------------------------------

/// A timer that activates a service on schedule.
#[derive(Debug, Clone)]
pub struct TimerUnit {
    pub name: String,
    /// Service to activate.
    pub service: String,
    /// Interval in seconds (repeating).
    pub interval_secs: u64,
    /// Last activation tick.
    pub last_trigger_tick: u64,
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// Socket activation
// ---------------------------------------------------------------------------

/// Socket that triggers service start on first connection.
#[derive(Debug, Clone)]
pub struct SocketUnit {
    pub name: String,
    /// Service to start when a connection arrives.
    pub service: String,
    /// Port to listen on.
    pub port: u16,
    pub active: bool,
}

// ---------------------------------------------------------------------------
// Journal entry
// ---------------------------------------------------------------------------

/// A captured log line from a service.
#[derive(Debug, Clone)]
pub struct JournalEntry {
    pub service: String,
    pub tick: u64,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Service unit
// ---------------------------------------------------------------------------

/// A service unit definition and runtime state.
#[derive(Debug, Clone)]
pub struct ServiceUnit {
    /// Unit name (e.g. "sshd.service").
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Service type.
    pub svc_type: ServiceType,
    /// Command to execute.
    pub exec_start: String,
    /// Command to run on stop (optional).
    pub exec_stop: String,
    /// Dependencies.
    pub dependencies: Vec<Dependency>,
    /// Current state.
    pub state: ServiceState,
    /// Restart policy.
    pub restart_policy: RestartPolicy,
    /// Delay between restarts in ticks.
    pub restart_delay: u64,
    /// Number of restarts attempted.
    pub restart_count: u32,
    /// Maximum restart retries.
    pub max_retries: u32,
    /// Whether the service is enabled (starts on boot).
    pub enabled: bool,
    /// Tick when the service entered the current state.
    pub state_change_tick: u64,
    /// Tick when the service was started (for uptime).
    pub start_tick: u64,
    /// PID if the service is running (simulated).
    pub pid: Option<u64>,
}

impl ServiceUnit {
    fn new(name: &str, desc: &str, svc_type: ServiceType, exec: &str) -> Self {
        Self {
            name: name.to_owned(),
            description: desc.to_owned(),
            svc_type,
            exec_start: exec.to_owned(),
            exec_stop: String::new(),
            dependencies: Vec::new(),
            state: ServiceState::Inactive,
            restart_policy: RestartPolicy::OnFailure,
            restart_delay: DEFAULT_RESTART_DELAY,
            restart_count: 0,
            max_retries: MAX_RESTART_RETRIES,
            enabled: false,
            state_change_tick: 0,
            start_tick: 0,
            pid: None,
        }
    }

    fn set_state(&mut self, new_state: ServiceState) {
        let old = self.state;
        self.state = new_state;
        self.state_change_tick = timer::ticks();
        serial_println!("[service_mgr] {} : {} -> {}", self.name, old.as_str(), new_state.as_str());
    }

    fn add_dep(&mut self, target: &str, kind: DepKind) {
        self.dependencies.push(Dependency {
            target: target.to_owned(),
            kind,
        });
    }

    /// Format a status line.
    fn status_line(&self) -> String {
        let uptime = if self.state == ServiceState::Running {
            let secs = (timer::ticks() - self.start_tick) / 100;
            format!("{} secs", secs)
        } else {
            "-".to_owned()
        };
        let pid_str = match self.pid {
            Some(p) => format!("{}", p),
            None => "-".to_owned(),
        };
        format!(
            "{:<24} {:<10} {:<6} {:<12} {:<10} {}",
            self.name, self.state.as_str(), pid_str,
            self.restart_policy.as_str(), uptime, self.description
        )
    }
}

// ---------------------------------------------------------------------------
// Service manager
// ---------------------------------------------------------------------------

/// The global service manager state.
pub struct ServiceManager {
    services: Vec<ServiceUnit>,
    targets: Vec<TargetUnit>,
    timers: Vec<TimerUnit>,
    sockets: Vec<SocketUnit>,
    journal: Vec<JournalEntry>,
    boot_start_tick: u64,
    boot_end_tick: u64,
    next_pid: u64,
}

impl ServiceManager {
    pub const fn new() -> Self {
        Self {
            services: Vec::new(),
            targets: Vec::new(),
            timers: Vec::new(),
            sockets: Vec::new(),
            journal: Vec::new(),
            boot_start_tick: 0,
            boot_end_tick: 0,
            next_pid: 100,
        }
    }

    fn alloc_pid(&mut self) -> u64 {
        let p = self.next_pid;
        self.next_pid += 1;
        p
    }

    fn find_service(&self, name: &str) -> Option<usize> {
        self.services.iter().position(|s| s.name == name)
    }

    fn journal_log(&mut self, service: &str, msg: &str) {
        self.journal.push(JournalEntry {
            service: service.to_owned(),
            tick: timer::ticks(),
            message: msg.to_owned(),
        });
        // Keep journal bounded.
        if self.journal.len() > 512 {
            self.journal.remove(0);
        }
    }
}

/// Global service manager instance.
pub static SVC_MGR: Mutex<ServiceManager> = Mutex::new(ServiceManager::new());

/// Counter of services started since boot.
static STARTS_TOTAL: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Dependency resolution — topological sort
// ---------------------------------------------------------------------------

/// Return service names in dependency-resolved start order.
fn resolve_boot_order(mgr: &ServiceManager) -> Vec<String> {
    let names: Vec<String> = mgr.services.iter()
        .filter(|s| s.enabled)
        .map(|s| s.name.clone())
        .collect();

    // Build adjacency: if A depends After B, then B must come before A.
    let mut order: Vec<String> = Vec::new();
    let mut visited: Vec<String> = Vec::new();

    fn visit(
        name: &str,
        services: &[ServiceUnit],
        visited: &mut Vec<String>,
        order: &mut Vec<String>,
    ) {
        if visited.iter().any(|v| v == name) {
            return;
        }
        visited.push(name.to_owned());
        if let Some(svc) = services.iter().find(|s| s.name == name) {
            for dep in &svc.dependencies {
                if dep.kind == DepKind::After || dep.kind == DepKind::Requires {
                    visit(&dep.target, services, visited, order);
                }
            }
        }
        order.push(name.to_owned());
    }

    for n in &names {
        visit(n, &mgr.services, &mut visited, &mut order);
    }
    order
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the service manager and register built-in services.
pub fn init() {
    let mut mgr = SVC_MGR.lock();

    // --- Targets ---
    mgr.targets.push(TargetUnit {
        name: "network.target".to_owned(),
        description: "Network is online".to_owned(),
        members: vec!["network.service".to_owned()],
        active: false,
    });
    mgr.targets.push(TargetUnit {
        name: "multi-user.target".to_owned(),
        description: "Multi-user system".to_owned(),
        members: vec![
            "sshd.service".to_owned(),
            "crond.service".to_owned(),
            "syslog.service".to_owned(),
        ],
        active: false,
    });
    mgr.targets.push(TargetUnit {
        name: "graphical.target".to_owned(),
        description: "Graphical interface".to_owned(),
        members: vec!["httpd.service".to_owned()],
        active: false,
    });

    // --- Built-in services ---
    let mut net = ServiceUnit::new(
        "network.service", "Network stack initialisation", ServiceType::Oneshot, "/sbin/net-init");
    net.enabled = true;
    mgr.services.push(net);

    let mut sshd = ServiceUnit::new(
        "sshd.service", "Secure Shell daemon", ServiceType::Simple, "/usr/sbin/sshd");
    sshd.restart_policy = RestartPolicy::Always;
    sshd.enabled = true;
    sshd.add_dep("network.service", DepKind::After);
    sshd.add_dep("network.service", DepKind::Requires);
    mgr.services.push(sshd);

    let mut httpd = ServiceUnit::new(
        "httpd.service", "HTTP server", ServiceType::Simple, "/usr/sbin/httpd");
    httpd.restart_policy = RestartPolicy::OnFailure;
    httpd.enabled = true;
    httpd.add_dep("network.service", DepKind::After);
    mgr.services.push(httpd);

    let mut crond = ServiceUnit::new(
        "crond.service", "Cron daemon", ServiceType::Simple, "/usr/sbin/crond");
    crond.restart_policy = RestartPolicy::Always;
    crond.enabled = true;
    mgr.services.push(crond);

    let mut syslog = ServiceUnit::new(
        "syslog.service", "System logger", ServiceType::Simple, "/usr/sbin/syslogd");
    syslog.restart_policy = RestartPolicy::Always;
    syslog.enabled = true;
    mgr.services.push(syslog);

    // --- Timer ---
    mgr.timers.push(TimerUnit {
        name: "logrotate.timer".to_owned(),
        service: "logrotate.service".to_owned(),
        interval_secs: 3600,
        last_trigger_tick: 0,
        enabled: true,
    });

    // --- Socket activation ---
    mgr.sockets.push(SocketUnit {
        name: "sshd.socket".to_owned(),
        service: "sshd.service".to_owned(),
        port: 22,
        active: false,
    });
    mgr.sockets.push(SocketUnit {
        name: "httpd.socket".to_owned(),
        service: "httpd.service".to_owned(),
        port: 80,
        active: false,
    });

    serial_println!("[service_mgr] initialised with {} services, {} targets",
        mgr.services.len(), mgr.targets.len());
}

/// Start a service by name.
pub fn start(name: &str) -> Result<(), String> {
    let mut mgr = SVC_MGR.lock();

    let idx = mgr.find_service(name)
        .ok_or_else(|| format!("service '{}' not found", name))?;

    if mgr.services[idx].state == ServiceState::Running {
        return Err(format!("'{}' is already running", name));
    }

    // Check required dependencies are running.
    let deps: Vec<Dependency> = mgr.services[idx].dependencies.clone();
    for dep in &deps {
        if dep.kind == DepKind::Requires {
            let dep_running = mgr.services.iter()
                .any(|s| s.name == dep.target && s.state == ServiceState::Running);
            if !dep_running {
                return Err(format!("required dependency '{}' is not running", dep.target));
            }
        }
    }

    let pid = mgr.alloc_pid();
    let svc_name = mgr.services[idx].name.clone();
    mgr.services[idx].set_state(ServiceState::Starting);
    mgr.services[idx].pid = Some(pid);
    mgr.services[idx].start_tick = timer::ticks();
    mgr.services[idx].restart_count = 0;

    // For oneshot, immediately transition to stopped after "running".
    if mgr.services[idx].svc_type == ServiceType::Oneshot {
        mgr.services[idx].set_state(ServiceState::Running);
        mgr.journal_log(&svc_name, "started (oneshot)");
        mgr.services[idx].set_state(ServiceState::Stopped);
        mgr.services[idx].pid = None;
        mgr.journal_log(&svc_name, "completed (oneshot)");
    } else {
        mgr.services[idx].set_state(ServiceState::Running);
        mgr.journal_log(&svc_name, &format!("started, pid={}", pid));
    }

    STARTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Stop a service by name.
pub fn stop(name: &str) -> Result<(), String> {
    let mut mgr = SVC_MGR.lock();

    let idx = mgr.find_service(name)
        .ok_or_else(|| format!("service '{}' not found", name))?;

    if mgr.services[idx].state != ServiceState::Running {
        return Err(format!("'{}' is not running", name));
    }

    let svc_name = mgr.services[idx].name.clone();
    mgr.services[idx].set_state(ServiceState::Stopping);
    mgr.services[idx].pid = None;
    mgr.services[idx].set_state(ServiceState::Stopped);
    mgr.journal_log(&svc_name, "stopped");
    Ok(())
}

/// Restart a service by name.
pub fn restart(name: &str) -> Result<(), String> {
    // Stop if running (ignore error if not running).
    let _ = stop(name);
    start(name)
}

/// Enable a service for boot.
pub fn enable(name: &str) -> Result<(), String> {
    let mut mgr = SVC_MGR.lock();
    let idx = mgr.find_service(name)
        .ok_or_else(|| format!("service '{}' not found", name))?;
    mgr.services[idx].enabled = true;
    Ok(())
}

/// Disable a service from boot.
pub fn disable(name: &str) -> Result<(), String> {
    let mut mgr = SVC_MGR.lock();
    let idx = mgr.find_service(name)
        .ok_or_else(|| format!("service '{}' not found", name))?;
    mgr.services[idx].enabled = false;
    Ok(())
}

/// Get the status string of a service.
pub fn status(name: &str) -> String {
    let mgr = SVC_MGR.lock();
    match mgr.services.iter().find(|s| s.name == name) {
        Some(svc) => {
            let mut out = String::new();
            out.push_str(&format!("● {} - {}\n", svc.name, svc.description));
            out.push_str(&format!("   Loaded: {} ({})\n",
                if svc.enabled { "enabled" } else { "disabled" },
                svc.restart_policy.as_str()));
            out.push_str(&format!("   Active: {}\n", svc.state.as_str()));
            out.push_str(&format!("     Type: {}\n", svc.svc_type.as_str()));
            out.push_str(&format!("     Exec: {}\n", svc.exec_start));
            if let Some(pid) = svc.pid {
                out.push_str(&format!("      PID: {}\n", pid));
            }
            if svc.state == ServiceState::Running {
                let uptime = (timer::ticks() - svc.start_tick) / 100;
                out.push_str(&format!("   Uptime: {} secs\n", uptime));
            }
            out.push_str(&format!(" Restarts: {}/{}\n", svc.restart_count, svc.max_retries));

            // Show recent journal lines for this service.
            let entries: Vec<_> = mgr.journal.iter()
                .filter(|e| e.service == svc.name)
                .rev()
                .take(5)
                .collect();
            if !entries.is_empty() {
                out.push_str("\n   Journal:\n");
                for e in entries.iter().rev() {
                    out.push_str(&format!("     [{}] {}\n", e.tick / 100, e.message));
                }
            }
            out
        }
        None => format!("service '{}' not found\n", name),
    }
}

/// List all services and their states.
pub fn list_services() -> String {
    let mgr = SVC_MGR.lock();
    let mut out = String::new();
    out.push_str(&format!("{:<24} {:<10} {:<6} {:<12} {:<10} {}\n",
        "UNIT", "STATE", "PID", "RESTART", "UPTIME", "DESCRIPTION"));
    let mut sorted: Vec<&ServiceUnit> = mgr.services.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    for svc in sorted {
        out.push_str(&svc.status_line());
        out.push('\n');
    }
    out
}

/// Get status of a service by name (convenience wrapper).
pub fn service_status(name: &str) -> String {
    status(name)
}

/// Run the boot sequence: start all enabled services in dependency order.
pub fn boot_sequence() {
    let order = {
        let mut mgr = SVC_MGR.lock();
        mgr.boot_start_tick = 0; // will set below
        resolve_boot_order(&mgr)
    };

    {
        let mut mgr = SVC_MGR.lock();
        mgr.boot_start_tick = timer::ticks();
    }

    serial_println!("[service_mgr] boot sequence: {} services", order.len());

    for name in &order {
        match start(name) {
            Ok(()) => {}
            Err(e) => serial_println!("[service_mgr] boot: failed to start {}: {}", name, e),
        }
    }

    // Activate targets whose members are all running/stopped(oneshot).
    {
        let mut mgr = SVC_MGR.lock();
        mgr.boot_end_tick = timer::ticks();
        // Collect member lists to avoid borrow conflict.
        let target_members: Vec<(usize, Vec<String>)> = mgr.targets.iter()
            .enumerate()
            .map(|(i, t)| (i, t.members.clone()))
            .collect();
        for (i, members) in &target_members {
            let all_ok = members.iter().all(|m| {
                mgr.services.iter().any(|s| {
                    s.name == *m && (s.state == ServiceState::Running || s.state == ServiceState::Stopped)
                })
            });
            mgr.targets[*i].active = all_ok;
            if all_ok {
                serial_println!("[service_mgr] target '{}' reached", mgr.targets[*i].name);
            }
        }
    }
}

/// Generate a boot timing report.
pub fn boot_report() -> String {
    let mgr = SVC_MGR.lock();
    let mut out = String::new();
    let total_ms = if mgr.boot_end_tick > mgr.boot_start_tick {
        (mgr.boot_end_tick - mgr.boot_start_tick) * 10 // ticks to ms at 100Hz
    } else {
        0
    };
    out.push_str(&format!("Boot completed in {} ms\n", total_ms));
    out.push_str(&format!("Services started: {}\n", STARTS_TOTAL.load(Ordering::Relaxed)));

    out.push_str("\nTargets:\n");
    for t in &mgr.targets {
        let mark = if t.active { "●" } else { "○" };
        out.push_str(&format!("  {} {} — {}\n", mark, t.name, t.description));
    }

    out.push_str("\nTimers:\n");
    for t in &mgr.timers {
        out.push_str(&format!("  {} → {} (every {} s, {})\n",
            t.name, t.service, t.interval_secs,
            if t.enabled { "enabled" } else { "disabled" }));
    }

    out.push_str("\nSockets:\n");
    for s in &mgr.sockets {
        out.push_str(&format!("  {} → {} (port {}, {})\n",
            s.name, s.service, s.port,
            if s.active { "listening" } else { "inactive" }));
    }

    out
}

/// Check timers and trigger services if due. Call periodically.
pub fn check_timers() {
    let mut triggers: Vec<String> = Vec::new();
    {
        let mut mgr = SVC_MGR.lock();
        let now = timer::ticks();
        for t in &mut mgr.timers {
            if !t.enabled {
                continue;
            }
            let interval_ticks = t.interval_secs * 100;
            if now - t.last_trigger_tick >= interval_ticks {
                t.last_trigger_tick = now;
                triggers.push(t.service.clone());
            }
        }
    }
    for svc in triggers {
        let _ = restart(&svc);
    }
}

/// Check for failed services that need restarting per their policy.
pub fn check_restart() {
    let mut to_restart: Vec<String> = Vec::new();
    {
        let mgr = SVC_MGR.lock();
        for svc in &mgr.services {
            if svc.state != ServiceState::Failed && svc.state != ServiceState::Stopped {
                continue;
            }
            if svc.restart_count >= svc.max_retries {
                continue;
            }
            let should_restart = match svc.restart_policy {
                RestartPolicy::Always => true,
                RestartPolicy::OnFailure => svc.state == ServiceState::Failed,
                RestartPolicy::OnAbnormal => svc.state == ServiceState::Failed,
                RestartPolicy::Never => false,
            };
            if should_restart {
                let elapsed = timer::ticks() - svc.state_change_tick;
                if elapsed >= svc.restart_delay {
                    to_restart.push(svc.name.clone());
                }
            }
        }
    }
    for name in to_restart {
        {
            let mut mgr = SVC_MGR.lock();
            if let Some(idx) = mgr.find_service(&name) {
                mgr.services[idx].restart_count += 1;
                let count = mgr.services[idx].restart_count;
                let svc_name = mgr.services[idx].name.clone();
                mgr.journal_log(&svc_name, &format!("auto-restart attempt {}", count));
            }
        }
        match start(&name) {
            Ok(()) => serial_println!("[service_mgr] auto-restarted {}", name),
            Err(e) => {
                serial_println!("[service_mgr] auto-restart failed for {}: {}", name, e);
                let mut mgr = SVC_MGR.lock();
                if let Some(idx) = mgr.find_service(&name) {
                    if mgr.services[idx].restart_count >= mgr.services[idx].max_retries {
                        mgr.services[idx].set_state(ServiceState::Failed);
                        let svc_name = mgr.services[idx].name.clone();
                        mgr.journal_log(&svc_name, "max restart retries reached — marked failed");
                    }
                }
            }
        }
    }
}

/// Simulate a service socket activation: start service for a port.
pub fn socket_activate(port: u16) -> Option<String> {
    let service_name = {
        let mgr = SVC_MGR.lock();
        mgr.sockets.iter()
            .find(|s| s.port == port && !s.active)
            .map(|s| s.service.clone())
    };
    if let Some(ref name) = service_name {
        serial_println!("[service_mgr] socket activation on port {} -> {}", port, name);
        let _ = start(name);
        let mut mgr = SVC_MGR.lock();
        if let Some(sock) = mgr.sockets.iter_mut().find(|s| s.port == port) {
            sock.active = true;
        }
    }
    service_name
}
