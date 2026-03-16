/// Build system for MerlionOS.
/// Manages module compilation, linking, configuration, and the module
/// dependency graph. Provides make-like build targets.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Maximum build targets the system can track.
const MAX_TARGETS: usize = 128;

/// Monotonic tick for build timestamps.
static BUILD_TICK: AtomicU64 = AtomicU64::new(1);

fn next_tick() -> u64 {
    BUILD_TICK.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// TargetType
// ---------------------------------------------------------------------------

/// The kind of artefact a build target produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetType {
    Kernel,
    Module,
    Program,
    Library,
    Test,
}

impl TargetType {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Kernel  => "kernel",
            Self::Module  => "module",
            Self::Program => "program",
            Self::Library => "library",
            Self::Test    => "test",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "kernel"  => Some(Self::Kernel),
            "module"  => Some(Self::Module),
            "program" => Some(Self::Program),
            "library" => Some(Self::Library),
            "test"    => Some(Self::Test),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// OptLevel / BuildConfig
// ---------------------------------------------------------------------------

/// Optimisation level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptLevel {
    Debug,
    Release,
    Size,
}

impl OptLevel {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Debug   => "debug",
            Self::Release => "release",
            Self::Size    => "size",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "debug"   => Some(Self::Debug),
            "release" => Some(Self::Release),
            "size"    => Some(Self::Size),
            _ => None,
        }
    }
}

/// Global build configuration.
struct BuildConfig {
    optimization: OptLevel,
    debug_symbols: bool,
    target_arch: String,
    features: Vec<String>,
}

impl BuildConfig {
    fn new() -> Self {
        Self {
            optimization: OptLevel::Release,
            debug_symbols: false,
            target_arch: String::from("x86_64"),
            features: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// BuildTarget
// ---------------------------------------------------------------------------

/// A single build target (module, program, etc.).
#[derive(Debug, Clone)]
pub struct BuildTarget {
    pub name: String,
    pub target_type: TargetType,
    pub sources: Vec<String>,
    pub dependencies: Vec<String>,
    pub config: Vec<(String, String)>,
    pub built: bool,
    pub build_time_ticks: u64,
    pub size_bytes: usize,
}

impl BuildTarget {
    fn new(name: &str, tt: TargetType, sources: Vec<&str>, deps: Vec<&str>, size: usize) -> Self {
        Self {
            name: String::from(name),
            target_type: tt,
            sources: sources.iter().map(|s| String::from(*s)).collect(),
            dependencies: deps.iter().map(|d| String::from(*d)).collect(),
            config: Vec::new(),
            built: false,
            build_time_ticks: 0,
            size_bytes: size,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static TARGETS: Mutex<Vec<BuildTarget>> = Mutex::new(Vec::new());
static CONFIG: Mutex<Option<BuildConfig>> = Mutex::new(None);

fn ensure_config() {
    let mut cfg = CONFIG.lock();
    if cfg.is_none() {
        *cfg = Some(BuildConfig::new());
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialise the build system with kernel build targets.
pub fn init() {
    ensure_config();
    let mut tgts = TARGETS.lock();
    tgts.clear();

    tgts.push(BuildTarget::new("kernel",    TargetType::Kernel,  vec!["src/main.rs"], vec![], 262144));
    tgts.push(BuildTarget::new("gdt",       TargetType::Module,  vec!["src/gdt.rs"], vec!["kernel"], 8192));
    tgts.push(BuildTarget::new("interrupts",TargetType::Module,  vec!["src/interrupts.rs"], vec!["kernel", "gdt"], 16384));
    tgts.push(BuildTarget::new("memory",    TargetType::Module,  vec!["src/memory.rs"], vec!["kernel"], 53248));
    tgts.push(BuildTarget::new("allocator", TargetType::Module,  vec!["src/allocator.rs"], vec!["kernel", "memory"], 12288));
    tgts.push(BuildTarget::new("timer",     TargetType::Module,  vec!["src/timer.rs"], vec!["kernel", "interrupts"], 4096));
    tgts.push(BuildTarget::new("scheduler", TargetType::Module,  vec!["src/scheduler.rs"], vec!["kernel", "timer", "memory"], 36864));
    tgts.push(BuildTarget::new("vfs",       TargetType::Module,  vec!["src/vfs.rs"], vec!["kernel", "allocator"], 40960));
    tgts.push(BuildTarget::new("serial",    TargetType::Module,  vec!["src/serial.rs"], vec!["kernel"], 4096));
    tgts.push(BuildTarget::new("vga",       TargetType::Module,  vec!["src/vga.rs"], vec!["kernel"], 8192));
    tgts.push(BuildTarget::new("shell",     TargetType::Program, vec!["src/shell.rs"], vec!["vfs", "scheduler"], 32768));
    tgts.push(BuildTarget::new("net",       TargetType::Module,  vec!["src/net.rs", "src/tcp.rs"], vec!["kernel", "memory"], 81920));
    tgts.push(BuildTarget::new("tests",     TargetType::Test,    vec!["src/unittest.rs"], vec!["kernel"], 20480));
    tgts.push(BuildTarget::new("libcore",   TargetType::Library, vec!["src/ulib.rs"], vec!["kernel"], 16384));
}

// ---------------------------------------------------------------------------
// Target management
// ---------------------------------------------------------------------------

/// Register a new build target.
pub fn register_target(target: BuildTarget) {
    let mut tgts = TARGETS.lock();
    if tgts.len() >= MAX_TARGETS {
        return;
    }
    if tgts.iter().any(|t| t.name == target.name) {
        return;
    }
    tgts.push(target);
}

/// Remove a build target by name.
pub fn remove_target(name: &str) {
    let mut tgts = TARGETS.lock();
    tgts.retain(|t| t.name != name);
}

// ---------------------------------------------------------------------------
// Dependency graph — topological sort with cycle detection
// ---------------------------------------------------------------------------

/// Return the build order (topological sort of all targets).
pub fn build_order() -> Vec<String> {
    let tgts = TARGETS.lock();
    let mut state: Vec<(String, u8)> = tgts.iter().map(|t| (t.name.clone(), 0u8)).collect();
    let mut order: Vec<String> = Vec::new();

    fn dfs(
        node: &str,
        state: &mut Vec<(String, u8)>,
        order: &mut Vec<String>,
        tgts: &[BuildTarget],
    ) {
        let idx = match state.iter().position(|(n, _)| n == node) {
            Some(i) => i,
            None => return,
        };
        if state[idx].1 != 0 {
            return; // visited or in-progress (skip cycles silently)
        }
        state[idx].1 = 1;
        if let Some(t) = tgts.iter().find(|t| t.name == node) {
            for dep in &t.dependencies {
                dfs(dep, state, order, tgts);
            }
        }
        let idx = state.iter().position(|(n, _)| n == node).unwrap();
        state[idx].1 = 2;
        order.push(String::from(node));
    }

    for i in 0..tgts.len() {
        let name = tgts[i].name.clone();
        dfs(&name, &mut state, &mut order, &tgts);
    }
    order
}

/// Detect circular dependencies. Returns `Some(cycle)` if found.
pub fn check_circular() -> Option<Vec<String>> {
    let tgts = TARGETS.lock();
    // 0=unvisited, 1=in-progress, 2=done
    let mut state: Vec<(String, u8)> = tgts.iter().map(|t| (t.name.clone(), 0u8)).collect();
    let mut path: Vec<String> = Vec::new();

    fn dfs(
        node: &str,
        state: &mut Vec<(String, u8)>,
        path: &mut Vec<String>,
        tgts: &[BuildTarget],
    ) -> bool {
        let idx = match state.iter().position(|(n, _)| n == node) {
            Some(i) => i,
            None => return false,
        };
        match state[idx].1 {
            2 => return false,
            1 => {
                path.push(String::from(node));
                return true;
            }
            _ => {}
        }
        state[idx].1 = 1;
        path.push(String::from(node));
        if let Some(t) = tgts.iter().find(|t| t.name == node) {
            for dep in &t.dependencies {
                if dfs(dep, state, path, tgts) {
                    return true;
                }
            }
        }
        path.pop();
        let idx = state.iter().position(|(n, _)| n == node).unwrap();
        state[idx].1 = 2;
        false
    }

    for i in 0..tgts.len() {
        let name = tgts[i].name.clone();
        path.clear();
        if dfs(&name, &mut state, &mut path, &tgts) {
            return Some(path);
        }
    }
    None
}

/// Display the dependency graph as a tree string.
pub fn dep_graph() -> String {
    let tgts = TARGETS.lock();
    let mut out = String::from("Build dependency graph:\n");
    for t in tgts.iter() {
        if t.dependencies.is_empty() {
            out.push_str(&format!("  {}\n", t.name));
        } else {
            out.push_str(&format!("  {} -> {}\n", t.name, t.dependencies.join(", ")));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Build / Clean
// ---------------------------------------------------------------------------

/// Build a target and all of its dependencies.
pub fn build(name: &str) -> Result<String, String> {
    // Get the build order then filter to what we need.
    let order = build_order();
    let full_order = order; // entire topo order

    // We need the target and its transitive deps.
    let needed = {
        let tgts = TARGETS.lock();
        if !tgts.iter().any(|t| t.name == name) {
            return Err(format!("target '{}' not found", name));
        }
        // Walk deps transitively.
        let mut needed: Vec<String> = Vec::new();
        fn collect(n: &str, needed: &mut Vec<String>, tgts: &[BuildTarget]) {
            if needed.iter().any(|x| x == n) {
                return;
            }
            if let Some(t) = tgts.iter().find(|t| t.name == n) {
                for dep in &t.dependencies {
                    collect(dep, needed, tgts);
                }
            }
            needed.push(String::from(n));
        }
        collect(name, &mut needed, &tgts);
        needed
    };

    // Build in topological order (only those in `needed`).
    let mut built_now: Vec<String> = Vec::new();
    let mut tgts = TARGETS.lock();
    for target_name in &full_order {
        if !needed.iter().any(|n| n == target_name) {
            continue;
        }
        if let Some(t) = tgts.iter_mut().find(|t| t.name == *target_name) {
            if !t.built {
                t.built = true;
                t.build_time_ticks = next_tick();
                built_now.push(t.name.clone());
            }
        }
    }

    if built_now.is_empty() {
        Ok(format!("'{}' is up to date", name))
    } else {
        Ok(format!("built: {}", built_now.join(" -> ")))
    }
}

/// Build all targets in dependency order.
pub fn build_all() -> String {
    let order = build_order();
    let mut count = 0u32;
    let mut tgts = TARGETS.lock();
    for target_name in &order {
        if let Some(t) = tgts.iter_mut().find(|t| t.name == *target_name) {
            if !t.built {
                t.built = true;
                t.build_time_ticks = next_tick();
                count += 1;
            }
        }
    }
    format!("built {} target(s)", count)
}

/// Mark a target as unbuilt.
pub fn clean(name: &str) {
    let mut tgts = TARGETS.lock();
    if let Some(t) = tgts.iter_mut().find(|t| t.name == name) {
        t.built = false;
        t.build_time_ticks = 0;
    }
}

/// Mark all targets as unbuilt.
pub fn clean_all() {
    let mut tgts = TARGETS.lock();
    for t in tgts.iter_mut() {
        t.built = false;
        t.build_time_ticks = 0;
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Set a build configuration value.
pub fn set_config(key: &str, value: &str) {
    ensure_config();
    let mut cfg = CONFIG.lock();
    let c = cfg.as_mut().unwrap();
    match key {
        "optimization" | "opt" => {
            if let Some(o) = OptLevel::from_str(value) {
                c.optimization = o;
            }
        }
        "debug_symbols" | "debug" => {
            c.debug_symbols = value == "true" || value == "1";
        }
        "target_arch" | "arch" => {
            c.target_arch = String::from(value);
        }
        "feature" => {
            if !c.features.iter().any(|f| f == value) {
                c.features.push(String::from(value));
            }
        }
        _ => {}
    }
}

/// Get a build configuration value.
pub fn get_config(key: &str) -> Option<String> {
    ensure_config();
    let cfg = CONFIG.lock();
    let c = cfg.as_ref()?;
    match key {
        "optimization" | "opt" => Some(String::from(c.optimization.label())),
        "debug_symbols" | "debug" => Some(format!("{}", c.debug_symbols)),
        "target_arch" | "arch" => Some(c.target_arch.clone()),
        "features" => Some(c.features.join(", ")),
        _ => None,
    }
}

/// Display the full build configuration.
pub fn show_config() -> String {
    ensure_config();
    let cfg = CONFIG.lock();
    let c = cfg.as_ref().unwrap();
    format!(
        "Build configuration:\n  optimization: {}\n  debug_symbols: {}\n  target_arch: {}\n  features: [{}]",
        c.optimization.label(),
        c.debug_symbols,
        c.target_arch,
        c.features.join(", "),
    )
}

// ---------------------------------------------------------------------------
// Manifest parsing (simple TOML-like)
// ---------------------------------------------------------------------------

/// Parse a module manifest (simple TOML-like key=value format).
///
/// Expected keys: `name`, `type`, `sources` (comma-separated), `deps`
/// (comma-separated), `size`.
pub fn parse_manifest(content: &str) -> Result<BuildTarget, &str> {
    let mut name: Option<&str> = None;
    let mut tt = TargetType::Module;
    let mut sources: Vec<String> = Vec::new();
    let mut deps: Vec<String> = Vec::new();
    let mut size: usize = 0;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(pos) = line.find('=') {
            let key = line[..pos].trim();
            let val = line[pos + 1..].trim().trim_matches('"');
            match key {
                "name" => name = Some(val),
                "type" => {
                    if let Some(t) = TargetType::from_str(val) {
                        tt = t;
                    }
                }
                "sources" => {
                    sources = val.split(',').map(|s| String::from(s.trim())).collect();
                }
                "deps" | "dependencies" => {
                    deps = val
                        .split(',')
                        .map(|s| String::from(s.trim()))
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                "size" => {
                    size = val.parse::<usize>().unwrap_or(0);
                }
                _ => {}
            }
        }
    }

    let pkg_name = name.ok_or("missing 'name' field")?;
    Ok(BuildTarget {
        name: String::from(pkg_name),
        target_type: tt,
        sources,
        dependencies: deps,
        config: Vec::new(),
        built: false,
        build_time_ticks: 0,
        size_bytes: size,
    })
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

/// Summary statistics for the build system.
pub fn build_stats() -> String {
    let tgts = TARGETS.lock();
    let total = tgts.len();
    let built = tgts.iter().filter(|t| t.built).count();
    let total_size: usize = tgts.iter().filter(|t| t.built).map(|t| t.size_bytes).sum();

    let mut by_type: Vec<(&str, usize)> = Vec::new();
    for t in tgts.iter() {
        let label = t.target_type.label();
        if let Some(entry) = by_type.iter_mut().find(|(l, _)| *l == label) {
            entry.1 += 1;
        } else {
            by_type.push((label, 1));
        }
    }

    let mut out = format!(
        "Build system: {} targets, {} built, {} pending\nBuilt size: {} bytes\nTarget types:\n",
        total,
        built,
        total - built,
        total_size,
    );
    for (label, count) in &by_type {
        out.push_str(&format!("  {}: {}\n", label, count));
    }
    out
}
