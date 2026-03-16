/// Package registry for MerlionOS.
/// Manages installable packages with version tracking, dependency resolution,
/// and a local package database.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Maximum packages the registry can hold.
const MAX_PACKAGES: usize = 128;

/// Global tick counter used to timestamp installations.
static TICK: AtomicU64 = AtomicU64::new(1);

/// Advance the tick and return the new value.
fn next_tick() -> u64 {
    TICK.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Version
// ---------------------------------------------------------------------------

/// Semantic version (major.minor.patch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self { major, minor, patch }
    }

    /// Parse a version string such as `"1.2.3"`.
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.split('.');
        let major = parts.next()?.parse::<u32>().ok()?;
        let minor = parts.next()?.parse::<u32>().ok()?;
        let patch = parts.next()?.parse::<u32>().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self { major, minor, patch })
    }

    /// Human-readable display string.
    pub fn display(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }

    /// Two versions are compatible if they share the same major version.
    pub fn is_compatible(&self, other: &Self) -> bool {
        self.major == other.major
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
    }
}

// ---------------------------------------------------------------------------
// Dependency
// ---------------------------------------------------------------------------

/// A dependency on another package, with optional version bounds.
#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    pub min_version: Option<Version>,
    pub max_version: Option<Version>,
}

impl Dependency {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            min_version: None,
            max_version: None,
        }
    }

    pub fn with_min(mut self, v: Version) -> Self {
        self.min_version = Some(v);
        self
    }

    pub fn with_max(mut self, v: Version) -> Self {
        self.max_version = Some(v);
        self
    }

    /// Check whether `ver` satisfies this dependency's bounds.
    pub fn satisfied_by(&self, ver: &Version) -> bool {
        if let Some(ref min) = self.min_version {
            if ver < min {
                return false;
            }
        }
        if let Some(ref max) = self.max_version {
            if ver > max {
                return false;
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// PackageCategory
// ---------------------------------------------------------------------------

/// Broad category a package belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageCategory {
    System,
    Network,
    Security,
    AI,
    Development,
    Utility,
    Game,
    Library,
}

impl PackageCategory {
    pub fn label(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Network => "network",
            Self::Security => "security",
            Self::AI => "ai",
            Self::Development => "development",
            Self::Utility => "utility",
            Self::Game => "game",
            Self::Library => "library",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "system" => Some(Self::System),
            "network" => Some(Self::Network),
            "security" => Some(Self::Security),
            "ai" => Some(Self::AI),
            "development" => Some(Self::Development),
            "utility" => Some(Self::Utility),
            "game" => Some(Self::Game),
            "library" => Some(Self::Library),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Package
// ---------------------------------------------------------------------------

/// Metadata and state for a single package.
#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: Version,
    pub description: String,
    pub author: String,
    pub license: String,
    pub size: usize,
    pub dependencies: Vec<Dependency>,
    pub installed: bool,
    pub install_tick: u64,
    pub category: PackageCategory,
    pub files: Vec<String>,
}

impl Package {
    fn new(
        name: &str,
        ver: Version,
        desc: &str,
        cat: PackageCategory,
        deps: Vec<Dependency>,
        files: Vec<&str>,
        size: usize,
    ) -> Self {
        Self {
            name: String::from(name),
            version: ver,
            description: String::from(desc),
            author: String::from("MerlionOS Team"),
            license: String::from("MIT"),
            size,
            dependencies: deps,
            installed: false,
            install_tick: 0,
            category: cat,
            files: files.iter().map(|f| String::from(*f)).collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Global registry
// ---------------------------------------------------------------------------

static REGISTRY: Mutex<Vec<Package>> = Mutex::new(Vec::new());

/// Initialise the registry with built-in packages.
pub fn init() {
    let mut reg = REGISTRY.lock();
    reg.clear();

    let v = |ma, mi, pa| Version::new(ma, mi, pa);
    let d = |n: &str| Dependency::new(n);

    // ---- system ----
    reg.push(Package::new("kernel", v(1,0,0), "MerlionOS core kernel", PackageCategory::System, Vec::new(), Vec::new(), 262144));
    reg.push(Package::new("bootloader", v(0,9,23), "Bootloader crate for x86_64", PackageCategory::System, Vec::new(), Vec::new(), 131072));
    reg.push(Package::new("vfs", v(1,0,0), "Virtual filesystem layer", PackageCategory::System, vec![d("kernel")], vec!["/sys/mod/vfs.ko"], 40960));
    reg.push(Package::new("memory", v(1,0,0), "Physical and virtual memory manager", PackageCategory::System, vec![d("kernel")], vec!["/sys/mod/memory.ko"], 53248));
    reg.push(Package::new("scheduler", v(1,0,0), "Preemptive round-robin scheduler", PackageCategory::System, vec![d("kernel"), d("memory")], vec!["/sys/mod/scheduler.ko"], 36864));

    // ---- network ----
    reg.push(Package::new("tcp-ip", v(1,0,0), "TCP/IP network stack", PackageCategory::Network, vec![d("kernel"), d("memory")], vec!["/sys/mod/tcp_ip.ko"], 81920));
    reg.push(Package::new("http", v(1,1,0), "HTTP/1.1 client and server", PackageCategory::Network, vec![d("tcp-ip")], vec!["/sys/mod/http.ko"], 40960));
    reg.push(Package::new("dns", v(1,0,0), "DNS resolver and server", PackageCategory::Network, vec![d("tcp-ip")], vec!["/sys/mod/dns.ko"], 28672));
    reg.push(Package::new("ssh", v(2,0,0), "Secure shell daemon", PackageCategory::Network, vec![d("tcp-ip"), d("tls")], vec!["/sys/mod/ssh.ko"], 57344));
    reg.push(Package::new("mqtt", v(1,0,0), "MQTT message broker", PackageCategory::Network, vec![d("tcp-ip")], vec!["/sys/mod/mqtt.ko"], 32768));
    reg.push(Package::new("websocket", v(1,0,0), "WebSocket client and server", PackageCategory::Network, vec![d("http")], vec!["/sys/mod/websocket.ko"], 24576));
    reg.push(Package::new("tls", v(1,0,0), "Transport layer security", PackageCategory::Network, vec![d("tcp-ip")], vec!["/sys/mod/tls.ko"], 65536));

    // ---- security ----
    reg.push(Package::new("permissions", v(1,0,0), "Unix-style permission system", PackageCategory::Security, vec![d("vfs")], vec!["/sys/mod/permissions.ko"], 20480));
    reg.push(Package::new("capabilities", v(1,0,0), "Capability-based security", PackageCategory::Security, vec![d("kernel")], vec!["/sys/mod/capabilities.ko"], 24576));
    reg.push(Package::new("firewall", v(1,0,0), "Network packet firewall", PackageCategory::Security, vec![d("tcp-ip")], vec!["/sys/mod/firewall.ko"], 28672));
    reg.push(Package::new("audit", v(1,0,0), "Security audit logging", PackageCategory::Security, vec![d("vfs"), d("permissions")], vec!["/sys/mod/audit.ko"], 16384));

    // ---- ai ----
    reg.push(Package::new("inference", v(1,0,0), "Neural network inference engine", PackageCategory::AI, vec![d("kernel"), d("memory")], vec!["/sys/mod/inference.ko"], 131072));
    reg.push(Package::new("training", v(0,5,0), "On-device model training", PackageCategory::AI, vec![d("inference")], vec!["/sys/mod/training.ko"], 163840));
    reg.push(Package::new("knowledge", v(1,0,0), "Knowledge graph and vector store", PackageCategory::AI, vec![d("vfs")], vec!["/sys/mod/knowledge.ko"], 81920));
    reg.push(Package::new("workflow", v(1,0,0), "AI agent workflow engine", PackageCategory::AI, vec![d("inference"), d("knowledge")], vec!["/sys/mod/workflow.ko"], 49152));
    reg.push(Package::new("evolve", v(0,3,0), "Self-evolving kernel subsystem", PackageCategory::AI, vec![d("inference"), d("kernel")], vec!["/sys/mod/evolve.ko"], 40960));

    // ---- development ----
    reg.push(Package::new("debugger", v(1,0,0), "Kernel debugger (kdb)", PackageCategory::Development, vec![d("kernel")], vec!["/usr/bin/kdb"], 36864));
    reg.push(Package::new("profiler", v(1,0,0), "Performance profiler", PackageCategory::Development, vec![d("kernel"), d("scheduler")], vec!["/usr/bin/profiler"], 28672));
    reg.push(Package::new("unittest", v(1,0,0), "Unit testing framework", PackageCategory::Development, vec![d("kernel")], vec!["/usr/bin/unittest"], 20480));
    reg.push(Package::new("fuzzer", v(1,0,0), "Fuzz testing tool", PackageCategory::Development, vec![d("kernel")], vec!["/usr/bin/fuzzer"], 24576));

    // ---- utility ----
    reg.push(Package::new("shell", v(1,0,0), "Interactive command shell", PackageCategory::Utility, vec![d("vfs"), d("scheduler")], vec!["/usr/bin/msh"], 32768));
    reg.push(Package::new("editor", v(1,0,0), "Terminal text editor", PackageCategory::Utility, vec![d("vfs")], vec!["/usr/bin/edit"], 28672));
    reg.push(Package::new("calculator", v(1,0,0), "Expression calculator", PackageCategory::Utility, Vec::new(), vec!["/usr/bin/calc"], 8192));

    // ---- game ----
    reg.push(Package::new("snake", v(1,0,0), "Classic snake game", PackageCategory::Game, vec![d("vfs")], vec!["/usr/games/snake"], 12288));
    reg.push(Package::new("tetris", v(1,0,0), "Tetris clone", PackageCategory::Game, vec![d("vfs")], vec!["/usr/games/tetris"], 16384));
}

// ---------------------------------------------------------------------------
// Registration helpers
// ---------------------------------------------------------------------------

/// Add a package to the registry.
pub fn register_package(pkg: Package) {
    let mut reg = REGISTRY.lock();
    if reg.len() >= MAX_PACKAGES {
        return;
    }
    if reg.iter().any(|p| p.name == pkg.name) {
        return;
    }
    reg.push(pkg);
}

/// Remove a package from the registry by name.
pub fn unregister_package(name: &str) {
    let mut reg = REGISTRY.lock();
    reg.retain(|p| p.name != name);
}

/// Find a package by exact name.
pub fn find_package(name: &str) -> Option<Package> {
    let reg = REGISTRY.lock();
    reg.iter().find(|p| p.name == name).cloned()
}

/// Search packages whose name or description contains `query` (case-insensitive).
pub fn search_packages(query: &str) -> Vec<Package> {
    let reg = REGISTRY.lock();
    let q = query.to_ascii_lowercase();
    reg.iter()
        .filter(|p| {
            p.name.to_ascii_lowercase().contains(&q)
                || p.description.to_ascii_lowercase().contains(&q)
        })
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

/// List all packages.
pub fn list_packages() -> String {
    let reg = REGISTRY.lock();
    let mut out = format!("All packages ({}):\n", reg.len());
    for p in reg.iter() {
        let status = if p.installed { "[installed]" } else { "[available]" };
        out.push_str(&format!("  {} {} {} - {}\n", p.name, p.version.display(), status, p.description));
    }
    out
}

/// List only installed packages.
pub fn list_installed() -> String {
    let reg = REGISTRY.lock();
    let pkgs: Vec<_> = reg.iter().filter(|p| p.installed).collect();
    let mut out = format!("Installed packages ({}):\n", pkgs.len());
    for p in &pkgs {
        out.push_str(&format!("  {} {}\n", p.name, p.version.display()));
    }
    out
}

/// List packages that are not yet installed.
pub fn list_available() -> String {
    let reg = REGISTRY.lock();
    let pkgs: Vec<_> = reg.iter().filter(|p| !p.installed).collect();
    let mut out = format!("Available packages ({}):\n", pkgs.len());
    for p in &pkgs {
        out.push_str(&format!("  {} {} - {}\n", p.name, p.version.display(), p.description));
    }
    out
}

/// List packages in a given category.
pub fn list_by_category(cat: PackageCategory) -> String {
    let reg = REGISTRY.lock();
    let pkgs: Vec<_> = reg.iter().filter(|p| p.category == cat).collect();
    let mut out = format!("Packages in [{}] ({}):\n", cat.label(), pkgs.len());
    for p in &pkgs {
        let status = if p.installed { "[installed]" } else { "[available]" };
        out.push_str(&format!("  {} {} {}\n", p.name, p.version.display(), status));
    }
    out
}

// ---------------------------------------------------------------------------
// Dependency resolution (DFS topological sort with cycle detection)
// ---------------------------------------------------------------------------

/// Resolve dependencies for installing a package.
/// Returns the ordered list of packages to install (topological sort).
pub fn resolve_dependencies(name: &str) -> Result<Vec<String>, String> {
    let reg = REGISTRY.lock();

    // Adjacency: package name -> list of dependency names
    let adj = |n: &str| -> Option<Vec<String>> {
        reg.iter()
            .find(|p| p.name == n)
            .map(|p| p.dependencies.iter().map(|d| d.name.clone()).collect())
    };

    if adj(name).is_none() {
        return Err(format!("package '{}' not found", name));
    }

    // DFS states: 0 = unvisited, 1 = in-progress, 2 = done
    let mut state: Vec<(String, u8)> = reg.iter().map(|p| (p.name.clone(), 0u8)).collect();
    let mut order: Vec<String> = Vec::new();

    fn dfs(
        node: &str,
        state: &mut Vec<(String, u8)>,
        order: &mut Vec<String>,
        adj: &dyn Fn(&str) -> Option<Vec<String>>,
    ) -> Result<(), String> {
        let idx = state.iter().position(|(n, _)| n == node);
        let idx = match idx {
            Some(i) => i,
            None => return Err(format!("unknown dependency '{}'", node)),
        };
        match state[idx].1 {
            2 => return Ok(()),     // already processed
            1 => return Err(format!("circular dependency detected involving '{}'", node)),
            _ => {}
        }
        state[idx].1 = 1; // mark in-progress
        if let Some(deps) = adj(node) {
            for dep in deps {
                dfs(&dep, state, order, adj)?;
            }
        }
        let idx = state.iter().position(|(n, _)| n == node).unwrap();
        state[idx].1 = 2; // done
        order.push(String::from(node));
        Ok(())
    }

    dfs(name, &mut state, &mut order, &adj)?;
    Ok(order)
}

/// Check if all dependencies for a package are satisfied (installed).
pub fn check_dependencies(name: &str) -> Result<(), String> {
    let reg = REGISTRY.lock();
    let pkg = reg.iter().find(|p| p.name == name);
    let pkg = match pkg {
        Some(p) => p,
        None => return Err(format!("package '{}' not found", name)),
    };
    for dep in &pkg.dependencies {
        match reg.iter().find(|p| p.name == dep.name) {
            None => return Err(format!("dependency '{}' not in registry", dep.name)),
            Some(p) => {
                if !p.installed {
                    return Err(format!("dependency '{}' is not installed", dep.name));
                }
                if !dep.satisfied_by(&p.version) {
                    return Err(format!(
                        "dependency '{}' version {} does not satisfy constraints",
                        dep.name,
                        p.version.display()
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Find reverse dependencies (packages that depend on `name`).
pub fn reverse_deps(name: &str) -> Vec<String> {
    let reg = REGISTRY.lock();
    reg.iter()
        .filter(|p| p.dependencies.iter().any(|d| d.name == name))
        .map(|p| p.name.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Install / Uninstall / Upgrade
// ---------------------------------------------------------------------------

/// Install a package and all of its dependencies.
pub fn install(name: &str) -> Result<String, String> {
    // Resolve deps outside the main lock to avoid re-entrancy issues.
    let order = resolve_dependencies(name)?;

    let mut reg = REGISTRY.lock();
    let mut installed_now: Vec<String> = Vec::new();

    for pkg_name in &order {
        if let Some(p) = reg.iter_mut().find(|p| p.name == *pkg_name) {
            if !p.installed {
                p.installed = true;
                p.install_tick = next_tick();
                installed_now.push(p.name.clone());
            }
        }
    }

    if installed_now.is_empty() {
        Ok(format!("'{}' is already installed", name))
    } else {
        Ok(format!("installed: {}", installed_now.join(", ")))
    }
}

/// Uninstall a package. Fails if other installed packages depend on it.
pub fn uninstall(name: &str) -> Result<String, String> {
    // Check reverse deps first.
    let rdeps = reverse_deps(name);
    {
        let reg = REGISTRY.lock();
        let blocking: Vec<_> = rdeps
            .iter()
            .filter(|r| reg.iter().any(|p| p.name == **r && p.installed))
            .cloned()
            .collect();
        if !blocking.is_empty() {
            return Err(format!(
                "cannot uninstall '{}': required by {}",
                name,
                blocking.join(", ")
            ));
        }
    }

    let mut reg = REGISTRY.lock();
    match reg.iter_mut().find(|p| p.name == name) {
        None => Err(format!("package '{}' not found", name)),
        Some(p) => {
            if !p.installed {
                return Err(format!("'{}' is not installed", name));
            }
            p.installed = false;
            p.install_tick = 0;
            Ok(format!("uninstalled '{}'", name))
        }
    }
}

/// Upgrade a package by bumping its patch version and re-marking as installed.
pub fn upgrade(name: &str) -> Result<String, String> {
    let mut reg = REGISTRY.lock();
    match reg.iter_mut().find(|p| p.name == name) {
        None => Err(format!("package '{}' not found", name)),
        Some(p) => {
            if !p.installed {
                return Err(format!("'{}' is not installed — install it first", name));
            }
            let old = p.version.display();
            p.version.patch += 1;
            p.install_tick = next_tick();
            Ok(format!("upgraded '{}' from {} to {}", name, old, p.version.display()))
        }
    }
}

/// Upgrade all installed packages.
pub fn upgrade_all() -> String {
    let mut reg = REGISTRY.lock();
    let mut count = 0u32;
    for p in reg.iter_mut() {
        if p.installed {
            p.version.patch += 1;
            p.install_tick = next_tick();
            count += 1;
        }
    }
    format!("upgraded {} package(s)", count)
}

// ---------------------------------------------------------------------------
// Info helpers
// ---------------------------------------------------------------------------

/// Detailed information about a single package.
pub fn package_info(name: &str) -> String {
    let reg = REGISTRY.lock();
    match reg.iter().find(|p| p.name == name) {
        None => format!("package '{}' not found", name),
        Some(p) => {
            let deps_str = if p.dependencies.is_empty() {
                String::from("(none)")
            } else {
                p.dependencies.iter().map(|d| d.name.clone()).collect::<Vec<_>>().join(", ")
            };
            let status = if p.installed {
                format!("installed (tick {})", p.install_tick)
            } else {
                String::from("not installed")
            };
            format!(
                "Package: {}\nVersion: {}\nCategory: {}\nDescription: {}\nAuthor: {}\nLicense: {}\nSize: {} bytes\nDependencies: {}\nStatus: {}\nFiles: {}",
                p.name,
                p.version.display(),
                p.category.label(),
                p.description,
                p.author,
                p.license,
                p.size,
                deps_str,
                status,
                if p.files.is_empty() { String::from("(none)") } else { p.files.join(", ") },
            )
        }
    }
}

/// List installed files for a package.
pub fn package_files(name: &str) -> String {
    let reg = REGISTRY.lock();
    match reg.iter().find(|p| p.name == name) {
        None => format!("package '{}' not found", name),
        Some(p) => {
            if p.files.is_empty() {
                return format!("'{}' has no tracked files", name);
            }
            let mut out = format!("Files for '{}':\n", name);
            for f in &p.files {
                out.push_str(&format!("  {}\n", f));
            }
            out
        }
    }
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

/// Summary statistics for the registry.
pub fn registry_stats() -> String {
    let reg = REGISTRY.lock();
    let total = reg.len();
    let installed = reg.iter().filter(|p| p.installed).count();
    let total_size: usize = reg.iter().filter(|p| p.installed).map(|p| p.size).sum();

    let mut cats: Vec<(&str, usize)> = Vec::new();
    for p in reg.iter() {
        let label = p.category.label();
        if let Some(entry) = cats.iter_mut().find(|(l, _)| *l == label) {
            entry.1 += 1;
        } else {
            cats.push((label, 1));
        }
    }

    let mut out = format!(
        "Registry: {} total, {} installed, {} available\nInstalled size: {} bytes\nCategories:\n",
        total,
        installed,
        total - installed,
        total_size,
    );
    for (label, count) in &cats {
        out.push_str(&format!("  {}: {}\n", label, count));
    }
    out
}
