/// OCI-compatible container runtime for MerlionOS.
/// Runs containerized applications with namespace isolation,
/// filesystem overlays, and resource limits via cgroups.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_CONTAINERS: usize = 64;
const MAX_IMAGES: usize = 32;
const MAX_LOG_LINES: usize = 256;
const MAX_LAYERS: usize = 8;
const MAX_COMPOSE_SERVICES: usize = 16;

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static NEXT_ID: AtomicU32 = AtomicU32::new(1);
static CONTAINERS_CREATED: AtomicU64 = AtomicU64::new(0);
static CONTAINERS_STARTED: AtomicU64 = AtomicU64::new(0);
static CONTAINERS_STOPPED: AtomicU64 = AtomicU64::new(0);
static CONTAINERS_REMOVED: AtomicU64 = AtomicU64::new(0);

static RUNTIME: Mutex<OciRuntime> = Mutex::new(OciRuntime::new());

// ---------------------------------------------------------------------------
// Network mode
// ---------------------------------------------------------------------------

/// Container network modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    Bridge,
    Host,
    None,
}

impl NetworkMode {
    fn label(self) -> &'static str {
        match self {
            NetworkMode::Bridge => "bridge",
            NetworkMode::Host => "host",
            NetworkMode::None => "none",
        }
    }
}

// ---------------------------------------------------------------------------
// Mount
// ---------------------------------------------------------------------------

/// A bind mount for a container.
#[derive(Debug, Clone)]
pub struct Mount {
    pub source: String,
    pub target: String,
    pub readonly: bool,
}

impl Mount {
    pub fn new(source: &str, target: &str, readonly: bool) -> Self {
        Self {
            source: String::from(source),
            target: String::from(target),
            readonly,
        }
    }
}

// ---------------------------------------------------------------------------
// Container configuration (OCI spec simplified)
// ---------------------------------------------------------------------------

/// OCI container configuration.
pub struct ContainerConfig {
    pub name: String,
    pub image: String,
    pub command: Vec<String>,
    pub env: Vec<(String, String)>,
    pub mounts: Vec<Mount>,
    pub memory_limit: usize,
    pub cpu_shares: u32,
    pub pids_limit: u32,
    pub network_mode: NetworkMode,
    pub ports: Vec<(u16, u16)>,
}

impl ContainerConfig {
    pub fn new(name: &str, image: &str) -> Self {
        Self {
            name: String::from(name),
            image: String::from(image),
            command: Vec::new(),
            env: Vec::new(),
            mounts: Vec::new(),
            memory_limit: 0,
            cpu_shares: 1024,
            pids_limit: 128,
            network_mode: NetworkMode::Bridge,
            ports: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Container state
// ---------------------------------------------------------------------------

/// Container lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerState {
    Created,
    Running,
    Stopped,
}

impl ContainerState {
    fn label(self) -> &'static str {
        match self {
            ContainerState::Created => "created",
            ContainerState::Running => "running",
            ContainerState::Stopped => "stopped",
        }
    }
}

// ---------------------------------------------------------------------------
// Namespace types
// ---------------------------------------------------------------------------

/// Simulated namespace kinds.
#[derive(Debug, Clone, Copy)]
enum NamespaceKind {
    Pid,
    Net,
    Mnt,
    Uts,
    Ipc,
}

impl NamespaceKind {
    fn label(self) -> &'static str {
        match self {
            NamespaceKind::Pid => "pid",
            NamespaceKind::Net => "net",
            NamespaceKind::Mnt => "mnt",
            NamespaceKind::Uts => "uts",
            NamespaceKind::Ipc => "ipc",
        }
    }
}

/// A simulated namespace with an ID.
#[derive(Debug, Clone, Copy)]
struct Namespace {
    kind: NamespaceKind,
    id: u32,
}

// ---------------------------------------------------------------------------
// Filesystem overlay
// ---------------------------------------------------------------------------

/// Overlay filesystem: read-only base layer + writable upper layer.
struct OverlayFs {
    base_path: String,
    upper_path: String,
    merged_path: String,
}

impl OverlayFs {
    fn new(container_name: &str, image: &str) -> Self {
        Self {
            base_path: format!("/var/lib/oci/images/{}/rootfs", image),
            upper_path: format!("/var/lib/oci/containers/{}/upper", container_name),
            merged_path: format!("/var/lib/oci/containers/{}/merged", container_name),
        }
    }
}

// ---------------------------------------------------------------------------
// Container
// ---------------------------------------------------------------------------

/// A running or stopped container instance.
struct Container {
    id: u32,
    name: String,
    image: String,
    command: Vec<String>,
    env: Vec<(String, String)>,
    mounts: Vec<Mount>,
    state: ContainerState,
    namespaces: Vec<Namespace>,
    overlay: OverlayFs,
    hostname: String,
    pid: u32,
    memory_limit: usize,
    cpu_shares: u32,
    pids_limit: u32,
    network_mode: NetworkMode,
    ports: Vec<(u16, u16)>,
    logs: Vec<String>,
    created_ticks: u64,
    started_ticks: u64,
    cgroup_path: String,
}

static NEXT_NS_ID: AtomicU32 = AtomicU32::new(1000);

impl Container {
    fn new(config: ContainerConfig, id: u32) -> Self {
        let hostname = config.name.clone();
        let overlay = OverlayFs::new(&config.name, &config.image);
        let cgroup_path = format!("/sys/fs/cgroup/oci/{}", config.name);

        // Create namespaces
        let ns_kinds = [NamespaceKind::Pid, NamespaceKind::Net,
                        NamespaceKind::Mnt, NamespaceKind::Uts,
                        NamespaceKind::Ipc];
        let mut namespaces = Vec::new();
        for kind in &ns_kinds {
            let ns_id = NEXT_NS_ID.fetch_add(1, Ordering::Relaxed);
            namespaces.push(Namespace { kind: *kind, id: ns_id });
        }

        let ticks = crate::timer::ticks();

        Self {
            id,
            name: config.name,
            image: config.image,
            command: config.command,
            env: config.env,
            mounts: config.mounts,
            state: ContainerState::Created,
            namespaces,
            overlay,
            hostname,
            pid: 0,
            memory_limit: config.memory_limit,
            cpu_shares: config.cpu_shares,
            pids_limit: config.pids_limit,
            network_mode: config.network_mode,
            ports: config.ports,
            logs: Vec::new(),
            created_ticks: ticks,
            started_ticks: 0,
            cgroup_path,
        }
    }

    fn log(&mut self, msg: &str) {
        if self.logs.len() < MAX_LOG_LINES {
            self.logs.push(String::from(msg));
        }
    }

    fn display(&self) -> String {
        let cmd_str = if self.command.is_empty() {
            String::from("(none)")
        } else {
            self.command.join(" ")
        };
        format!("{:<6} {:<16} {:<12} {:<10} {}",
            self.id, self.name, self.image, self.state.label(), cmd_str)
    }

    fn info(&self) -> String {
        let mut out = format!("Container: {} (ID: {})\n", self.name, self.id);
        out.push_str(&format!("  Image:    {}\n", self.image));
        out.push_str(&format!("  State:    {}\n", self.state.label()));
        out.push_str(&format!("  Hostname: {}\n", self.hostname));
        out.push_str(&format!("  PID:      {}\n", self.pid));
        let cmd_str = if self.command.is_empty() {
            String::from("(none)")
        } else {
            self.command.join(" ")
        };
        out.push_str(&format!("  Command:  {}\n", cmd_str));
        out.push_str(&format!("  Network:  {}\n", self.network_mode.label()));
        if !self.ports.is_empty() {
            out.push_str("  Ports:\n");
            for (host, cont) in &self.ports {
                out.push_str(&format!("    {}:{}\n", host, cont));
            }
        }
        out.push_str(&format!("  Memory limit: {} bytes\n", self.memory_limit));
        out.push_str(&format!("  CPU shares:   {}\n", self.cpu_shares));
        out.push_str(&format!("  PIDs limit:   {}\n", self.pids_limit));
        out.push_str(&format!("  Overlay base:  {}\n", self.overlay.base_path));
        out.push_str(&format!("  Overlay upper: {}\n", self.overlay.upper_path));
        out.push_str("  Namespaces:\n");
        for ns in &self.namespaces {
            out.push_str(&format!("    {} (ns-id: {})\n", ns.kind.label(), ns.id));
        }
        if !self.mounts.is_empty() {
            out.push_str("  Mounts:\n");
            for m in &self.mounts {
                let ro = if m.readonly { "ro" } else { "rw" };
                out.push_str(&format!("    {} -> {} ({})\n", m.source, m.target, ro));
            }
        }
        out.push_str(&format!("  Cgroup:   {}\n", self.cgroup_path));
        out.push_str(&format!("  Created:  tick {}\n", self.created_ticks));
        if self.started_ticks > 0 {
            out.push_str(&format!("  Started:  tick {}\n", self.started_ticks));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Image
// ---------------------------------------------------------------------------

/// A container image (simplified).
struct Image {
    name: String,
    tag: String,
    size_kb: u32,
    layers: u32,
}

impl Image {
    fn new(name: &str, tag: &str, size_kb: u32, layers: u32) -> Self {
        Self {
            name: String::from(name),
            tag: String::from(tag),
            size_kb,
            layers,
        }
    }

    fn display(&self) -> String {
        format!("{:<24} {:<10} {} KB  {} layers",
            self.name, self.tag, self.size_kb, self.layers)
    }
}

// ---------------------------------------------------------------------------
// OCI Runtime
// ---------------------------------------------------------------------------

struct OciRuntime {
    containers: Vec<Container>,
    images: Vec<Image>,
}

impl OciRuntime {
    const fn new() -> Self {
        Self {
            containers: Vec::new(),
            images: Vec::new(),
        }
    }

    fn find_container(&self, name: &str) -> Option<usize> {
        self.containers.iter().position(|c| c.name == name)
    }

    fn find_container_by_id(&self, id: u32) -> Option<usize> {
        self.containers.iter().position(|c| c.id == id)
    }

    fn find_image(&self, name: &str) -> Option<usize> {
        self.images.iter().position(|i| i.name == name)
    }

    fn running_count(&self) -> usize {
        self.containers.iter().filter(|c| c.state == ContainerState::Running).count()
    }
}

// ---------------------------------------------------------------------------
// Port mapping
// ---------------------------------------------------------------------------

/// Set up port mapping (simulated via iptables DNAT).
fn setup_port_mapping(container_name: &str, ports: &[(u16, u16)]) {
    for (host_port, container_port) in ports {
        let rule = format!(
            "-t nat -A PREROUTING -p tcp --dport {} -j DNAT --to-destination container_{}:{}",
            host_port, container_name, container_port
        );
        let _ = rule; // Would call iptables in a real implementation
    }
}

/// Remove port mapping rules.
fn remove_port_mapping(container_name: &str, ports: &[(u16, u16)]) {
    for (host_port, container_port) in ports {
        let rule = format!(
            "-t nat -D PREROUTING -p tcp --dport {} -j DNAT --to-destination container_{}:{}",
            host_port, container_name, container_port
        );
        let _ = rule;
    }
}

// ---------------------------------------------------------------------------
// Compose (simple multi-container config)
// ---------------------------------------------------------------------------

/// A service definition in a compose file (simplified).
struct ComposeService {
    name: String,
    image: String,
    command: String,
    ports: Vec<(u16, u16)>,
    depends_on: Vec<String>,
}

/// Parse a simple compose-like config string.
/// Format: "name:image:cmd:hostport:containerport"
fn parse_compose_line(line: &str) -> Option<ComposeService> {
    let parts: Vec<&str> = line.split(':').collect();
    if parts.len() < 3 {
        return None;
    }
    let mut ports = Vec::new();
    if parts.len() >= 5 {
        if let (Ok(hp), Ok(cp)) = (parts[3].parse::<u16>(), parts[4].parse::<u16>()) {
            ports.push((hp, cp));
        }
    }
    Some(ComposeService {
        name: String::from(parts[0]),
        image: String::from(parts[1]),
        command: String::from(parts[2]),
        ports,
        depends_on: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the OCI container runtime.
pub fn init() {
    let mut rt = RUNTIME.lock();

    // Register some built-in images
    rt.images.push(Image::new("alpine", "latest", 5500, 1));
    rt.images.push(Image::new("busybox", "latest", 1400, 1));
    rt.images.push(Image::new("nginx", "1.25", 42000, 3));
    rt.images.push(Image::new("redis", "7", 32000, 3));
    rt.images.push(Image::new("postgres", "16", 85000, 5));

    // Create image directories in VFS
    let _ = crate::vfs::mkdir("/var");
    let _ = crate::vfs::mkdir("/var/lib");
    let _ = crate::vfs::mkdir("/var/lib/oci");
    let _ = crate::vfs::mkdir("/var/lib/oci/images");
    let _ = crate::vfs::mkdir("/var/lib/oci/containers");

    INITIALIZED.store(true, Ordering::SeqCst);
}

/// Create and start a container from an image with a command.
pub fn run(image: &str, cmd: &str) -> Result<String, &'static str> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let name = format!("{}-{}", image.split('/').last().unwrap_or(image), id);

    let mut config = ContainerConfig::new(&name, image);
    if !cmd.is_empty() {
        for part in cmd.split_whitespace() {
            config.command.push(String::from(part));
        }
    }

    let mut rt = RUNTIME.lock();

    // Verify image exists
    if rt.find_image(image).is_none() {
        return Err("image not found (use 'container images' to list)");
    }

    if rt.containers.len() >= MAX_CONTAINERS {
        return Err("maximum containers reached");
    }

    let mut container = Container::new(config, id);
    container.state = ContainerState::Running;
    container.started_ticks = crate::timer::ticks();
    container.pid = id + 1000; // simulated PID
    container.log(&format!("Container {} started", name));
    container.log(&format!("Running: {}", cmd));

    // Setup port mapping
    setup_port_mapping(&container.name, &container.ports);

    // Create container VFS directories
    let _ = crate::vfs::mkdir(&format!("/var/lib/oci/containers/{}", container.name));
    let _ = crate::vfs::mkdir(&container.overlay.upper_path);
    let _ = crate::vfs::mkdir(&container.overlay.merged_path);

    let result = format!("Container {} ({}) started from image '{}'", name, id, image);
    rt.containers.push(container);

    CONTAINERS_CREATED.fetch_add(1, Ordering::Relaxed);
    CONTAINERS_STARTED.fetch_add(1, Ordering::Relaxed);

    Ok(result)
}

/// Stop a running container.
pub fn stop(name: &str) -> Result<String, &'static str> {
    let mut rt = RUNTIME.lock();
    let idx = rt.find_container(name).ok_or("container not found")?;

    if rt.containers[idx].state != ContainerState::Running {
        return Err("container is not running");
    }

    rt.containers[idx].state = ContainerState::Stopped;
    rt.containers[idx].log("Container stopped");

    // Remove port mapping
    let ports = rt.containers[idx].ports.clone();
    remove_port_mapping(name, &ports);

    CONTAINERS_STOPPED.fetch_add(1, Ordering::Relaxed);
    Ok(format!("Container '{}' stopped", name))
}

/// Force-kill a running container.
pub fn kill(name: &str) -> Result<String, &'static str> {
    let mut rt = RUNTIME.lock();
    let idx = rt.find_container(name).ok_or("container not found")?;

    if rt.containers[idx].state != ContainerState::Running {
        return Err("container is not running");
    }

    rt.containers[idx].state = ContainerState::Stopped;
    rt.containers[idx].log("Container killed (SIGKILL)");
    rt.containers[idx].pid = 0;

    let ports = rt.containers[idx].ports.clone();
    remove_port_mapping(name, &ports);

    CONTAINERS_STOPPED.fetch_add(1, Ordering::Relaxed);
    Ok(format!("Container '{}' killed", name))
}

/// Remove a stopped container.
pub fn rm(name: &str) -> Result<String, &'static str> {
    let mut rt = RUNTIME.lock();
    let idx = rt.find_container(name).ok_or("container not found")?;

    if rt.containers[idx].state == ContainerState::Running {
        return Err("cannot remove running container (stop it first)");
    }

    rt.containers.remove(idx);
    CONTAINERS_REMOVED.fetch_add(1, Ordering::Relaxed);
    Ok(format!("Container '{}' removed", name))
}

/// Execute a command inside a running container.
pub fn exec(name: &str, cmd: &str) -> Result<String, &'static str> {
    let mut rt = RUNTIME.lock();
    let idx = rt.find_container(name).ok_or("container not found")?;

    if rt.containers[idx].state != ContainerState::Running {
        return Err("container is not running");
    }

    rt.containers[idx].log(&format!("exec: {}", cmd));
    Ok(format!("[{}] $ {}\n(exec completed)", name, cmd))
}

/// Get logs from a container.
pub fn logs(name: &str) -> Result<String, &'static str> {
    let rt = RUNTIME.lock();
    let idx = rt.find_container(name).ok_or("container not found")?;

    let mut out = format!("Logs for container '{}':\n", name);
    for line in &rt.containers[idx].logs {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    if rt.containers[idx].logs.is_empty() {
        out.push_str("  (no logs)\n");
    }
    Ok(out)
}

/// List all containers.
pub fn list_containers() -> String {
    let rt = RUNTIME.lock();
    let mut out = format!("{:<6} {:<16} {:<12} {:<10} {}\n",
        "ID", "NAME", "IMAGE", "STATE", "COMMAND");
    for c in &rt.containers {
        out.push_str(&c.display());
        out.push('\n');
    }
    if rt.containers.is_empty() {
        out.push_str("(no containers)\n");
    }
    out
}

/// Get detailed info about a container.
pub fn container_info(name: &str) -> Result<String, &'static str> {
    let rt = RUNTIME.lock();
    let idx = rt.find_container(name).ok_or("container not found")?;
    Ok(rt.containers[idx].info())
}

/// List available images.
pub fn list_images() -> String {
    let rt = RUNTIME.lock();
    let mut out = format!("{:<24} {:<10} {:<12} {}\n",
        "REPOSITORY", "TAG", "SIZE", "LAYERS");
    for img in &rt.images {
        out.push_str(&img.display());
        out.push('\n');
    }
    out
}

/// Pull an image (simulated: register from VFS).
pub fn pull_image(name: &str) -> Result<String, &'static str> {
    let mut rt = RUNTIME.lock();
    if rt.find_image(name).is_some() {
        return Ok(format!("Image '{}' already exists", name));
    }
    if rt.images.len() >= MAX_IMAGES {
        return Err("maximum images reached");
    }
    rt.images.push(Image::new(name, "latest", 1024, 1));
    let _ = crate::vfs::mkdir(&format!("/var/lib/oci/images/{}", name));
    Ok(format!("Pulled image '{}'", name))
}

/// Remove an image.
pub fn remove_image(name: &str) -> Result<String, &'static str> {
    let mut rt = RUNTIME.lock();

    // Check no running container uses this image
    let in_use = rt.containers.iter().any(|c| c.image == name && c.state == ContainerState::Running);
    if in_use {
        return Err("image in use by running container");
    }

    let idx = rt.find_image(name).ok_or("image not found")?;
    rt.images.remove(idx);
    Ok(format!("Removed image '{}'", name))
}

/// Run a compose-like multi-container config.
pub fn compose_up(config: &str) -> Result<String, &'static str> {
    let mut results = Vec::new();
    for line in config.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(svc) = parse_compose_line(line) {
            match run(&svc.image, &svc.command) {
                Ok(msg) => results.push(msg),
                Err(e) => results.push(format!("Error starting {}: {}", svc.name, e)),
            }
        }
    }
    Ok(results.join("\n"))
}

/// OCI runtime info.
pub fn oci_info() -> String {
    let rt = RUNTIME.lock();
    let mut out = String::from("OCI Container Runtime:\n");
    out.push_str(&format!("  Initialized: {}\n", INITIALIZED.load(Ordering::Relaxed)));
    out.push_str(&format!("  Containers:  {} ({} running)\n",
        rt.containers.len(), rt.running_count()));
    out.push_str(&format!("  Images:      {}\n", rt.images.len()));
    out.push_str(&format!("  Max containers: {}\n", MAX_CONTAINERS));
    out.push_str(&format!("  Max images:     {}\n", MAX_IMAGES));
    out.push_str("  Namespaces: PID, NET, MNT, UTS, IPC\n");
    out.push_str("  Overlay FS: base (ro) + upper (rw)\n");
    out
}

/// OCI runtime statistics.
pub fn oci_stats() -> String {
    let mut out = String::from("OCI Runtime Statistics:\n");
    out.push_str(&format!("  Containers created: {}\n", CONTAINERS_CREATED.load(Ordering::Relaxed)));
    out.push_str(&format!("  Containers started: {}\n", CONTAINERS_STARTED.load(Ordering::Relaxed)));
    out.push_str(&format!("  Containers stopped: {}\n", CONTAINERS_STOPPED.load(Ordering::Relaxed)));
    out.push_str(&format!("  Containers removed: {}\n", CONTAINERS_REMOVED.load(Ordering::Relaxed)));
    out
}

/// Handle shell commands for containers.
pub fn handle_command(args: &str) -> String {
    let parts: Vec<&str> = args.splitn(3, ' ').collect();
    if parts.is_empty() {
        return String::from("Usage: container <run|stop|kill|rm|exec|logs|ls|images|pull|rmi|info|inspect> ...");
    }

    match parts[0] {
        "run" => {
            if parts.len() < 2 {
                return String::from("Usage: container run <image> [cmd]");
            }
            let cmd = if parts.len() >= 3 { parts[2] } else { "" };
            match run(parts[1], cmd) {
                Ok(msg) => msg,
                Err(e) => format!("Error: {}", e),
            }
        }
        "stop" => {
            if parts.len() < 2 {
                return String::from("Usage: container stop <name>");
            }
            match stop(parts[1]) {
                Ok(msg) => msg,
                Err(e) => format!("Error: {}", e),
            }
        }
        "kill" => {
            if parts.len() < 2 {
                return String::from("Usage: container kill <name>");
            }
            match kill(parts[1]) {
                Ok(msg) => msg,
                Err(e) => format!("Error: {}", e),
            }
        }
        "rm" => {
            if parts.len() < 2 {
                return String::from("Usage: container rm <name>");
            }
            match rm(parts[1]) {
                Ok(msg) => msg,
                Err(e) => format!("Error: {}", e),
            }
        }
        "exec" => {
            if parts.len() < 3 {
                return String::from("Usage: container exec <name> <cmd>");
            }
            match exec(parts[1], parts[2]) {
                Ok(msg) => msg,
                Err(e) => format!("Error: {}", e),
            }
        }
        "logs" => {
            if parts.len() < 2 {
                return String::from("Usage: container logs <name>");
            }
            match logs(parts[1]) {
                Ok(msg) => msg,
                Err(e) => format!("Error: {}", e),
            }
        }
        "ls" => list_containers(),
        "images" => list_images(),
        "pull" => {
            if parts.len() < 2 {
                return String::from("Usage: container pull <image>");
            }
            match pull_image(parts[1]) {
                Ok(msg) => msg,
                Err(e) => format!("Error: {}", e),
            }
        }
        "rmi" => {
            if parts.len() < 2 {
                return String::from("Usage: container rmi <image>");
            }
            match remove_image(parts[1]) {
                Ok(msg) => msg,
                Err(e) => format!("Error: {}", e),
            }
        }
        "inspect" => {
            if parts.len() < 2 {
                return String::from("Usage: container inspect <name>");
            }
            match container_info(parts[1]) {
                Ok(msg) => msg,
                Err(e) => format!("Error: {}", e),
            }
        }
        "info" => oci_info(),
        "stats" => oci_stats(),
        "ps" => list_containers(),

        // ── Docker Compose ──────────────────────────────────────
        "compose" => {
            if parts.len() < 2 {
                return String::from("Usage: container compose <up|down|ps|logs> [args]");
            }
            let sub = parts[1];
            match sub {
                "up" => {
                    if parts.len() >= 3 {
                        // Read compose file from VFS
                        match crate::vfs::cat(parts[2]) {
                            Ok(content) => match compose_up(&content) {
                                Ok(msg) => msg,
                                Err(e) => format!("compose up failed: {}", e),
                            },
                            Err(e) => format!("Cannot read {}: {}", parts[2], e),
                        }
                    } else {
                        // Try default docker-compose.yml
                        match crate::vfs::cat("/docker-compose.yml") {
                            Ok(content) => match compose_up(&content) {
                                Ok(msg) => msg,
                                Err(e) => format!("compose up failed: {}", e),
                            },
                            Err(_) => String::from("Usage: container compose up [file]\nNo /docker-compose.yml found"),
                        }
                    }
                }
                "down" => compose_down(),
                "ps" => compose_ps(),
                "logs" => {
                    if parts.len() >= 3 {
                        match logs(parts[2]) {
                            Ok(msg) => msg,
                            Err(e) => format!("Error: {}", e),
                        }
                    } else {
                        compose_logs()
                    }
                }
                "restart" => {
                    let down_msg = compose_down();
                    let up_msg = match crate::vfs::cat("/docker-compose.yml") {
                        Ok(content) => compose_up(&content).unwrap_or_else(|e| format!("Error: {}", e)),
                        Err(_) => String::from("No /docker-compose.yml found"),
                    };
                    format!("{}\n{}", down_msg, up_msg)
                }
                _ => format!("Unknown compose subcommand: {}", sub),
            }
        }

        _ => format!("Unknown container subcommand: {}", parts[0]),
    }
}

/// Stop all running containers (compose down).
pub fn compose_down() -> String {
    let rt = RUNTIME.lock();
    let names: Vec<String> = rt.containers.iter()
        .filter(|c| c.state == ContainerState::Running)
        .map(|c| c.name.clone())
        .collect();
    drop(rt);

    if names.is_empty() {
        return String::from("No running containers to stop.");
    }

    let mut results = Vec::new();
    for name in &names {
        match stop(name) {
            Ok(msg) => results.push(msg),
            Err(e) => results.push(format!("Error stopping {}: {}", name, e)),
        }
    }
    results.join("\n")
}

/// List containers launched by compose (same as ps, but formatted for compose).
pub fn compose_ps() -> String {
    let rt = RUNTIME.lock();
    if rt.containers.is_empty() {
        return String::from("No containers.");
    }
    let mut out = String::from("NAME                IMAGE           STATUS\n");
    for c in &rt.containers {
        let status = match c.state {
            ContainerState::Created => "created",
            ContainerState::Running => "running",
            ContainerState::Stopped => "stopped",
        };
        out.push_str(&alloc::format!("{:<20}{:<16}{}\n", c.name, c.image, status));
    }
    out
}

/// Aggregate logs from all running containers.
pub fn compose_logs() -> String {
    let rt = RUNTIME.lock();
    let names: Vec<String> = rt.containers.iter()
        .map(|c| c.name.clone())
        .collect();
    drop(rt);

    let mut out = String::new();
    for name in &names {
        if let Ok(log) = logs(name) {
            out.push_str(&alloc::format!("=== {} ===\n{}\n", name, log));
        }
    }
    if out.is_empty() {
        String::from("No logs available.")
    } else {
        out
    }
}
