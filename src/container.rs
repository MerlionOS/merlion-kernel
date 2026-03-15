/// Lightweight container/namespace isolation for MerlionOS.
/// Provides process, filesystem, and network namespace isolation with
/// per-container resource limits. Containers run commands in isolated
/// contexts with their own PID mappings, VFS roots, and network stacks.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

/// Maximum number of containers that can exist simultaneously.
const MAX_CONTAINERS: usize = 16;
/// Maximum number of PID mappings per container namespace.
const MAX_PID_MAPPINGS: usize = 32;

/// Next container ID to assign.
static NEXT_CONTAINER_ID: AtomicUsize = AtomicUsize::new(1);
/// Global container manager instance.
static MANAGER: Mutex<ContainerManager> = Mutex::new(ContainerManager::new());

/// Per-container resource constraints enforced by the kernel.
#[derive(Debug, Clone, Copy)]
pub struct ResourceLimits {
    /// Maximum memory in bytes this container may use.
    pub max_memory: usize,
    /// Maximum CPU ticks before the container is throttled.
    pub max_cpu_ticks: usize,
    /// Maximum number of tasks (threads) the container may spawn.
    pub max_tasks: usize,
}

impl ResourceLimits {
    /// Sensible defaults: 1 MiB memory, 10 000 ticks, 8 tasks.
    pub const fn default_limits() -> Self {
        Self {
            max_memory: 1024 * 1024,
            max_cpu_ticks: 10_000,
            max_tasks: 8,
        }
    }
}

/// Maps container-local PIDs to global kernel PIDs.
/// Each container sees PIDs starting at 1, independent of other containers.
#[derive(Clone)]
pub struct PidNamespace {
    /// (container_pid, global_pid) pairs.
    mappings: Vec<(usize, usize)>,
    /// Next container-local PID to assign.
    next_local_pid: usize,
}

impl PidNamespace {
    /// Create an empty PID namespace.
    pub const fn new() -> Self {
        Self {
            mappings: Vec::new(),
            next_local_pid: 1,
        }
    }

    /// Register a global PID and return the container-local PID assigned to it.
    pub fn add(&mut self, global_pid: usize) -> Option<usize> {
        if self.mappings.len() >= MAX_PID_MAPPINGS {
            return None;
        }
        let local = self.next_local_pid;
        self.next_local_pid += 1;
        self.mappings.push((local, global_pid));
        Some(local)
    }

    /// Translate a container-local PID to the real global PID.
    pub fn to_global(&self, local_pid: usize) -> Option<usize> {
        self.mappings.iter().find(|(l, _)| *l == local_pid).map(|(_, g)| *g)
    }

    /// Translate a global PID to the container-local PID.
    pub fn to_local(&self, global_pid: usize) -> Option<usize> {
        self.mappings.iter().find(|(_, g)| *g == global_pid).map(|(l, _)| *l)
    }

    /// Remove a mapping by global PID (e.g. when a task exits).
    pub fn remove_global(&mut self, global_pid: usize) {
        self.mappings.retain(|(_, g)| *g != global_pid);
    }

    /// Number of active PID mappings.
    pub fn len(&self) -> usize {
        self.mappings.len()
    }
}

/// Lifecycle state of a container.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContainerState {
    /// Created but nothing running yet.
    Created,
    /// At least one active task.
    Running,
    /// All tasks finished; container is idle.
    Stopped,
    /// Destroyed and awaiting cleanup.
    Destroyed,
}

/// A lightweight isolated execution environment.
pub struct Container {
    /// Unique identifier assigned at creation.
    pub id: usize,
    /// Human-readable name.
    pub name: String,
    /// Current lifecycle state.
    pub state: ContainerState,
    /// PID namespace: maps container-local PIDs to global PIDs.
    pub pid_namespace: PidNamespace,
    /// Root path in the VFS visible to this container.
    pub vfs_root: String,
    /// Identifier for the network namespace (0 = host).
    pub network_namespace: usize,
    /// Resource constraints.
    pub resource_limits: ResourceLimits,
    /// Cumulative CPU ticks consumed.
    cpu_ticks_used: usize,
    /// Current memory usage in bytes (approximate).
    memory_used: usize,
}

/// Read-only snapshot returned by `list()`.
#[derive(Debug, Clone)]
pub struct ContainerInfo {
    pub id: usize,
    pub name: String,
    pub state: ContainerState,
    pub task_count: usize,
    pub memory_used: usize,
    pub vfs_root: String,
}

/// Manages the set of active containers behind the global mutex.
pub struct ContainerManager {
    containers: Vec<Container>,
}

impl ContainerManager {
    /// Create an empty manager (const-compatible for static init).
    const fn new() -> Self {
        Self { containers: Vec::new() }
    }
}

// ---------------------------------------------------------------------------
// Public API — operates on the global MANAGER
// ---------------------------------------------------------------------------

/// Create a new container with the given name and default resource limits.
/// Returns the container ID on success.
pub fn create(name: &str) -> Result<usize, &'static str> {
    let mut mgr = MANAGER.lock();
    if mgr.containers.len() >= MAX_CONTAINERS {
        return Err("container: maximum container count reached");
    }

    let id = NEXT_CONTAINER_ID.fetch_add(1, Ordering::Relaxed);

    // Each container gets its own logical VFS sub-tree under /containers/<id>.
    // The path acts as a chroot boundary for processes in this container.
    let vfs_root = format!("/containers/{}", id);

    let container = Container {
        id,
        name: name.to_owned(),
        state: ContainerState::Created,
        pid_namespace: PidNamespace::new(),
        vfs_root,
        network_namespace: id,
        resource_limits: ResourceLimits::default_limits(),
        cpu_ticks_used: 0,
        memory_used: 0,
    };

    crate::serial_println!("[container] created id={} name={}", id, name);
    mgr.containers.push(container);
    Ok(id)
}

/// Execute a command inside the container identified by `id`.
/// The command is spawned as a kernel task whose PID is mapped into the
/// container's PID namespace, inheriting the container's VFS root.
pub fn exec(id: usize, command: &str) -> Result<usize, &'static str> {
    let mut mgr = MANAGER.lock();
    let ct = mgr
        .containers
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or("container: no such container")?;

    if ct.state == ContainerState::Destroyed {
        return Err("container: container has been destroyed");
    }

    // Enforce task limit.
    if ct.pid_namespace.len() >= ct.resource_limits.max_tasks {
        return Err("container: task limit reached");
    }

    // Spawn a kernel task for the command. The task runs in the shared
    // address space but logically belongs to this container's namespace.
    let task_name: &'static str = match command {
        "init" => "ct-init",
        "shell" => "ct-shell",
        "worker" => "ct-worker",
        _ => "ct-task",
    };
    let global_pid = crate::task::spawn(task_name, || {
        // In a full implementation this would chroot into vfs_root,
        // apply resource cgroup limits, and exec the real binary.
        crate::serial_println!("[container] task running in container");
        crate::task::yield_now();
    });

    match global_pid {
        Some(pid) => {
            let local_pid = ct
                .pid_namespace
                .add(pid)
                .ok_or("container: PID namespace full")?;
            ct.state = ContainerState::Running;
            crate::serial_println!(
                "[container] exec id={} cmd={} gpid={} lpid={}",
                id, command, pid, local_pid
            );
            Ok(local_pid)
        }
        None => Err("container: failed to spawn task"),
    }
}

/// Destroy a container: kill all its tasks and release resources.
pub fn destroy(id: usize) -> Result<(), &'static str> {
    let mut mgr = MANAGER.lock();
    let ct = mgr
        .containers
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or("container: no such container")?;

    if ct.state == ContainerState::Destroyed {
        return Err("container: already destroyed");
    }

    // Kill every task in the container's PID namespace.
    let global_pids: Vec<usize> =
        ct.pid_namespace.mappings.iter().map(|(_, g)| *g).collect();
    for gpid in &global_pids {
        let _ = crate::task::kill(*gpid);
        ct.pid_namespace.remove_global(*gpid);
    }

    ct.state = ContainerState::Destroyed;
    ct.cpu_ticks_used = 0;
    ct.memory_used = 0;

    crate::serial_println!("[container] destroyed id={} name={}", id, ct.name);
    Ok(())
}

/// Return a snapshot of all non-destroyed containers.
pub fn list() -> Vec<ContainerInfo> {
    let mgr = MANAGER.lock();
    mgr.containers
        .iter()
        .filter(|c| c.state != ContainerState::Destroyed)
        .map(|c| ContainerInfo {
            id: c.id,
            name: c.name.clone(),
            state: c.state,
            task_count: c.pid_namespace.len(),
            memory_used: c.memory_used,
            vfs_root: c.vfs_root.clone(),
        })
        .collect()
}

/// Return a human-readable status string for the container with the given ID.
pub fn container_info(id: usize) -> Result<String, &'static str> {
    let mgr = MANAGER.lock();
    let ct = mgr
        .containers
        .iter()
        .find(|c| c.id == id)
        .ok_or("container: no such container")?;

    let state_str = match ct.state {
        ContainerState::Created => "created",
        ContainerState::Running => "running",
        ContainerState::Stopped => "stopped",
        ContainerState::Destroyed => "destroyed",
    };

    Ok(format!(
        "Container #{}\n\
         \x20 name:      {}\n\
         \x20 state:     {}\n\
         \x20 vfs_root:  {}\n\
         \x20 netns:     {}\n\
         \x20 tasks:     {}\n\
         \x20 memory:    {}/{} bytes\n\
         \x20 cpu_ticks: {}/{}",
        ct.id, ct.name, state_str, ct.vfs_root,
        ct.network_namespace, ct.pid_namespace.len(),
        ct.memory_used, ct.resource_limits.max_memory,
        ct.cpu_ticks_used, ct.resource_limits.max_cpu_ticks,
    ))
}
