/// Systemd-like init system for MerlionOS.
///
/// Manages kernel services with dependency ordering, restart policies, and
/// lifecycle control.  Services are loaded from `/etc/services.conf` at boot
/// using an INI-style format: `[name]` sections with `command`, `type`,
/// `restart`, `depends`, and `enabled` keys.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;
use spin::Mutex;
use crate::{serial_println, println, vfs, task, shell};

const MAX_SERVICES: usize = 32;

/// Global service manager instance.
static MANAGER: Mutex<Option<ServiceManager>> = Mutex::new(None);

/// Execution model for a service.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ServiceType {
    /// Runs once to completion, then transitions to `Stopped`.
    OneShot,
    /// Long-running background process.
    Daemon,
    /// Periodic timer-driven execution.
    Timer,
}

/// Current lifecycle state of a service.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ServiceState {
    Inactive,
    Starting,
    Running,
    Failed,
    Stopped,
}

/// Restart behaviour after a service exits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RestartPolicy {
    Always,
    OnFailure,
    Never,
}

/// Describes a single managed service.
#[derive(Clone)]
pub struct Service {
    pub name: String,
    pub command: String,
    pub svc_type: ServiceType,
    pub state: ServiceState,
    pub pid: usize,
    pub restart_policy: RestartPolicy,
    pub depends_on: Vec<String>,
    pub enabled: bool,
}

impl Service {
    /// Create a service with sensible defaults.
    fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(), command: String::new(),
            svc_type: ServiceType::OneShot, state: ServiceState::Inactive,
            pid: 0, restart_policy: RestartPolicy::Never,
            depends_on: Vec::new(), enabled: true,
        }
    }
}

/// Owns the list of services and provides lifecycle operations.
pub struct ServiceManager {
    services: Vec<Service>,
}

impl ServiceManager {
    fn new() -> Self { Self { services: Vec::new() } }

    /// Register a service.  Duplicates by name are replaced.
    fn register(&mut self, svc: Service) {
        if let Some(existing) = self.services.iter_mut().find(|s| s.name == svc.name) {
            *existing = svc;
        } else if self.services.len() < MAX_SERVICES {
            self.services.push(svc);
        }
    }

    /// Look up a service index by name.
    fn find(&self, name: &str) -> Option<usize> {
        self.services.iter().position(|s| s.name == name)
    }

    /// Start a service and its dependencies (recursively).
    fn start_service(&mut self, name: &str) -> Result<(), &'static str> {
        let idx = self.find(name).ok_or("service not found")?;
        match self.services[idx].state {
            ServiceState::Running | ServiceState::Starting => return Ok(()),
            _ => {}
        }
        // Recursively start unmet dependencies first.
        let deps = self.services[idx].depends_on.clone();
        for dep in &deps {
            if self.find(dep).is_none() {
                serial_println!("init: missing dependency '{}' for '{}'", dep, name);
                return Err("missing dependency");
            }
            let di = self.find(dep).unwrap();
            let ds = self.services[di].state;
            if ds != ServiceState::Running && ds != ServiceState::Stopped {
                let d = dep.clone();
                self.start_service(&d)?;
            }
        }
        // Mark starting and dispatch.
        let idx = self.find(name).unwrap();
        self.services[idx].state = ServiceState::Starting;
        serial_println!("init: starting '{}'", name);
        let cmd = self.services[idx].command.clone();
        let svc_name: &str = &self.services[idx].name;
        match self.services[idx].svc_type {
            ServiceType::Daemon => {
                let _cmd_owned = cmd.clone();
                let task_name: &'static str = leak_str(svc_name);
                if let Some(pid) = task::spawn(task_name, || {
                    crate::serial_println!("[init] daemon started");
                }) {
                    let i = self.find(name).unwrap();
                    self.services[i].pid = pid;
                    self.services[i].state = ServiceState::Running;
                } else {
                    let i = self.find(name).unwrap();
                    self.services[i].state = ServiceState::Failed;
                    return Err("failed to spawn task");
                }
            }
            ServiceType::OneShot => {
                shell::dispatch(&cmd);
                let i = self.find(name).unwrap();
                self.services[i].state = ServiceState::Stopped;
            }
            ServiceType::Timer => {
                let i = self.find(name).unwrap();
                self.services[i].state = ServiceState::Running;
            }
        }
        Ok(())
    }

    /// Stop a running service by killing its task.
    fn stop_service(&mut self, name: &str) -> Result<(), &'static str> {
        let idx = self.find(name).ok_or("service not found")?;
        let st = self.services[idx].state;
        if st != ServiceState::Running && st != ServiceState::Starting {
            return Err("service is not running");
        }
        if self.services[idx].pid != 0 { let _ = task::kill(self.services[idx].pid); }
        self.services[idx].state = ServiceState::Stopped;
        self.services[idx].pid = 0;
        serial_println!("init: stopped '{}'", name);
        Ok(())
    }

    /// Restart a service (stop then start).
    fn restart_service(&mut self, name: &str) -> Result<(), &'static str> {
        let _ = self.stop_service(name);
        self.start_service(name)
    }

    /// Enable or disable a service for auto-start at boot.
    fn set_enabled(&mut self, name: &str, val: bool) -> Result<(), &'static str> {
        let idx = self.find(name).ok_or("service not found")?;
        self.services[idx].enabled = val;
        Ok(())
    }

    /// Print a status table of all registered services.
    fn status(&self) {
        println!("{:<16} {:<10} {:<10} {:<6} {}", "SERVICE", "TYPE", "STATE", "PID", "ENABLED");
        println!("{}", "-".repeat(56));
        for svc in &self.services {
            let ty = match svc.svc_type {
                ServiceType::OneShot => "oneshot", ServiceType::Daemon => "daemon",
                ServiceType::Timer => "timer",
            };
            let st = match svc.state {
                ServiceState::Inactive => "inactive", ServiceState::Starting => "starting",
                ServiceState::Running => "running", ServiceState::Failed => "failed",
                ServiceState::Stopped => "stopped",
            };
            let pid = if svc.pid != 0 { format!("{}", svc.pid) } else { String::from("-") };
            let en = if svc.enabled { "yes" } else { "no" };
            println!("{:<16} {:<10} {:<10} {:<6} {}", svc.name, ty, st, pid, en);
        }
    }
}

/// Parse `/etc/services.conf` content into service definitions.
fn parse_config(contents: &str) -> Vec<Service> {
    let mut services: Vec<Service> = Vec::new();
    let mut current: Option<Service> = None;
    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        // Section header: [name]
        if line.starts_with('[') && line.ends_with(']') {
            if let Some(svc) = current.take() { services.push(svc); }
            current = Some(Service::new(&line[1..line.len() - 1]));
            continue;
        }
        if let Some(ref mut svc) = current {
            if let Some(pos) = line.find('=') {
                let (key, val) = (line[..pos].trim(), line[pos + 1..].trim());
                match key {
                    "command" => svc.command = val.to_owned(),
                    "type" => svc.svc_type = match val {
                        "daemon" => ServiceType::Daemon, "timer" => ServiceType::Timer,
                        _ => ServiceType::OneShot,
                    },
                    "restart" => svc.restart_policy = match val {
                        "always" => RestartPolicy::Always, "on-failure" => RestartPolicy::OnFailure,
                        _ => RestartPolicy::Never,
                    },
                    "depends" => svc.depends_on = val.split(',')
                        .map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect(),
                    "enabled" => svc.enabled = val == "true",
                    _ => serial_println!("init: unknown key '{}'", key),
                }
            }
        }
    }
    if let Some(svc) = current { services.push(svc); }
    services
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the init system: load `/etc/services.conf` and start enabled services.
pub fn init() {
    let mut mgr = ServiceManager::new();
    if let Ok(contents) = vfs::cat("/etc/services.conf") {
        for svc in parse_config(&contents) { mgr.register(svc); }
        serial_println!("init: loaded {} service(s)", mgr.services.len());
    } else {
        serial_println!("init: /etc/services.conf not found");
    }
    let names: Vec<String> = mgr.services.iter()
        .filter(|s| s.enabled).map(|s| s.name.clone()).collect();
    for name in &names {
        if let Err(e) = mgr.start_service(name) {
            serial_println!("init: failed to start '{}': {}", name, e);
        }
    }
    *MANAGER.lock() = Some(mgr);
}

/// Start a service by name.
pub fn start_service(name: &str) -> Result<(), &'static str> {
    MANAGER.lock().as_mut().ok_or("init not ready")?.start_service(name)
}

/// Stop a service by name.
pub fn stop_service(name: &str) -> Result<(), &'static str> {
    MANAGER.lock().as_mut().ok_or("init not ready")?.stop_service(name)
}

/// Restart (stop + start) a service by name.
pub fn restart_service(name: &str) -> Result<(), &'static str> {
    MANAGER.lock().as_mut().ok_or("init not ready")?.restart_service(name)
}

/// Enable a service for auto-start at boot.
pub fn enable(name: &str) -> Result<(), &'static str> {
    MANAGER.lock().as_mut().ok_or("init not ready")?.set_enabled(name, true)
}

/// Disable a service from auto-start at boot.
pub fn disable(name: &str) -> Result<(), &'static str> {
    MANAGER.lock().as_mut().ok_or("init not ready")?.set_enabled(name, false)
}

/// Print the status table of all registered services.
pub fn status() {
    if let Some(ref mgr) = *MANAGER.lock() { mgr.status(); }
    else { println!("init system not initialised"); }
}

/// Leak a `&str` to `&'static str` for task names (acceptable for fixed service set).
fn leak_str(s: &str) -> &'static str {
    Box::leak(String::from(s).into_boxed_str())
}
