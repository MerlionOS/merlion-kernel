/// Extended kernel configuration system for MerlionOS.
/// Provides typed configuration parameters, runtime tuning,
/// sysctl-like interface, and configuration profiles.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::borrow::ToOwned;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of configuration parameters.
const MAX_PARAMS: usize = 128;

/// Maximum depth of the hierarchical namespace.
const MAX_DEPTH: usize = 8;

/// Maximum number of saved profiles.
const MAX_PROFILES: usize = 16;

// ---------------------------------------------------------------------------
// Statistics (lock-free)
// ---------------------------------------------------------------------------

static READS: AtomicU64 = AtomicU64::new(0);
static WRITES: AtomicU64 = AtomicU64::new(0);
static ERRORS: AtomicU64 = AtomicU64::new(0);
static PROFILE_APPLIES: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Config parameter types
// ---------------------------------------------------------------------------

/// The type of a configuration parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamType {
    /// Boolean (true / false).
    Bool,
    /// 64-bit signed integer.
    Integer,
    /// Arbitrary string.
    Str,
    /// One of a fixed set of string values.
    Enum(Vec<String>),
    /// Integer within an inclusive range.
    Range(i64, i64),
}

/// A concrete configuration value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigValue {
    Bool(bool),
    Integer(i64),
    Str(String),
}

impl ConfigValue {
    /// Return a human-readable representation.
    pub fn display(&self) -> String {
        match self {
            ConfigValue::Bool(b) => if *b { "true".to_owned() } else { "false".to_owned() },
            ConfigValue::Integer(n) => format!("{}", n),
            ConfigValue::Str(s) => s.clone(),
        }
    }

    /// Parse a string into a value according to the expected type.
    fn parse(s: &str, ptype: &ParamType) -> Result<ConfigValue, &'static str> {
        match ptype {
            ParamType::Bool => match s {
                "true" | "1" | "yes" | "on" => Ok(ConfigValue::Bool(true)),
                "false" | "0" | "no" | "off" => Ok(ConfigValue::Bool(false)),
                _ => Err("invalid boolean"),
            },
            ParamType::Integer => {
                let n = parse_i64(s).ok_or("invalid integer")?;
                Ok(ConfigValue::Integer(n))
            }
            ParamType::Str => Ok(ConfigValue::Str(s.to_owned())),
            ParamType::Enum(variants) => {
                if variants.iter().any(|v| v == s) {
                    Ok(ConfigValue::Str(s.to_owned()))
                } else {
                    Err("value not in allowed enum variants")
                }
            }
            ParamType::Range(lo, hi) => {
                let n = parse_i64(s).ok_or("invalid integer for range")?;
                if n < *lo || n > *hi {
                    Err("value out of range")
                } else {
                    Ok(ConfigValue::Integer(n))
                }
            }
        }
    }
}

/// Simple i64 parser (no_std, no float).
fn parse_i64(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (neg, digits) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else {
        (false, s)
    };
    let mut result: i64 = 0;
    for b in digits.bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        result = result.checked_mul(10)?;
        result = result.checked_add((b - b'0') as i64)?;
    }
    if neg { Some(-result) } else { Some(result) }
}

// ---------------------------------------------------------------------------
// Config categories
// ---------------------------------------------------------------------------

/// Top-level category for a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Kernel,
    Memory,
    Network,
    Security,
    Scheduler,
    Filesystem,
    Audio,
    Display,
    AI,
    Power,
}

impl Category {
    pub fn name(self) -> &'static str {
        match self {
            Category::Kernel     => "kernel",
            Category::Memory     => "memory",
            Category::Network    => "net",
            Category::Security   => "security",
            Category::Scheduler  => "sched",
            Category::Filesystem => "fs",
            Category::Audio      => "audio",
            Category::Display    => "display",
            Category::AI         => "ai",
            Category::Power      => "power",
        }
    }

    fn from_str(s: &str) -> Option<Category> {
        match s {
            "kernel"   => Some(Category::Kernel),
            "memory"   => Some(Category::Memory),
            "net"      => Some(Category::Network),
            "security" => Some(Category::Security),
            "sched"    => Some(Category::Scheduler),
            "fs"       => Some(Category::Filesystem),
            "audio"    => Some(Category::Audio),
            "display"  => Some(Category::Display),
            "ai"       => Some(Category::AI),
            "power"    => Some(Category::Power),
            _          => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Config entry
// ---------------------------------------------------------------------------

/// A single configuration entry.
#[derive(Debug, Clone)]
pub struct ConfigEntry {
    /// Dotted path, e.g. "kernel.hz".
    pub name: String,
    /// Top-level category.
    pub category: Category,
    /// Parameter type.
    pub ptype: ParamType,
    /// Default value.
    pub default: ConfigValue,
    /// Current value.
    pub current: ConfigValue,
    /// Human-readable description.
    pub description: String,
    /// If true, the parameter cannot be modified at runtime.
    pub readonly: bool,
}

impl ConfigEntry {
    fn new(
        name: &str,
        category: Category,
        ptype: ParamType,
        default: ConfigValue,
        description: &str,
        readonly: bool,
    ) -> Self {
        Self {
            name: name.to_owned(),
            category,
            ptype,
            default: default.clone(),
            current: default,
            description: description.to_owned(),
            readonly,
        }
    }

    /// True if the current value differs from the default.
    pub fn is_modified(&self) -> bool {
        self.current != self.default
    }
}

// ---------------------------------------------------------------------------
// Config profiles
// ---------------------------------------------------------------------------

/// A named profile: a snapshot of parameter overrides.
#[derive(Debug, Clone)]
pub struct ConfigProfile {
    pub name: String,
    pub description: String,
    /// (param_name, value_string) pairs.
    pub overrides: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// Config store (global state behind Mutex)
// ---------------------------------------------------------------------------

struct ConfigStore {
    params: Vec<ConfigEntry>,
    profiles: Vec<ConfigProfile>,
    initialized: bool,
}

impl ConfigStore {
    const fn new() -> Self {
        Self {
            params: Vec::new(),
            profiles: Vec::new(),
            initialized: false,
        }
    }
}

static STORE: Mutex<ConfigStore> = Mutex::new(ConfigStore::new());

// ---------------------------------------------------------------------------
// Built-in parameter registration
// ---------------------------------------------------------------------------

/// Register all built-in parameters and profiles.
fn register_defaults(store: &mut ConfigStore) {
    let p = &mut store.params;

    // -- kernel --
    p.push(ConfigEntry::new("kernel.hz",                Category::Kernel, ParamType::Range(10, 1000),   ConfigValue::Integer(100),   "Timer interrupt frequency",            false));
    p.push(ConfigEntry::new("kernel.preempt",           Category::Kernel, ParamType::Bool,              ConfigValue::Bool(true),     "Enable preemptive scheduling",         false));
    p.push(ConfigEntry::new("kernel.panic_timeout",     Category::Kernel, ParamType::Range(0, 600),     ConfigValue::Integer(0),     "Seconds to wait before reboot on panic (0=halt)", false));
    p.push(ConfigEntry::new("kernel.max_tasks",         Category::Kernel, ParamType::Range(4, 256),     ConfigValue::Integer(64),    "Maximum number of concurrent tasks",   false));
    p.push(ConfigEntry::new("kernel.hostname",          Category::Kernel, ParamType::Str,               ConfigValue::Str("merlion".to_owned()), "System hostname", false));
    p.push(ConfigEntry::new("kernel.version",           Category::Kernel, ParamType::Str,               ConfigValue::Str("0.1.0".to_owned()),   "Kernel version (read-only)", true));
    p.push(ConfigEntry::new("kernel.log_level",         Category::Kernel, ParamType::Enum(vec!["error".to_owned(),"warn".to_owned(),"info".to_owned(),"debug".to_owned(),"trace".to_owned()]), ConfigValue::Str("info".to_owned()), "Minimum log level", false));

    // -- memory --
    p.push(ConfigEntry::new("memory.heap_size",         Category::Memory, ParamType::Range(4096, 1048576), ConfigValue::Integer(65536), "Heap size in bytes",                false));
    p.push(ConfigEntry::new("memory.slab_min",          Category::Memory, ParamType::Range(8, 4096),      ConfigValue::Integer(32),    "Minimum slab allocation size",      false));
    p.push(ConfigEntry::new("memory.slab_max",          Category::Memory, ParamType::Range(64, 65536),    ConfigValue::Integer(4096),  "Maximum slab allocation size",      false));
    p.push(ConfigEntry::new("memory.overcommit",        Category::Memory, ParamType::Bool,                ConfigValue::Bool(false),    "Allow memory overcommit",           false));
    p.push(ConfigEntry::new("memory.heap_warn_pct",     Category::Memory, ParamType::Range(0, 100),      ConfigValue::Integer(70),    "Heap usage warning threshold (%)",  false));
    p.push(ConfigEntry::new("memory.heap_crit_pct",     Category::Memory, ParamType::Range(0, 100),      ConfigValue::Integer(90),    "Heap usage critical threshold (%)", false));

    // -- network --
    p.push(ConfigEntry::new("net.tcp.window",           Category::Network, ParamType::Range(1024, 1048576), ConfigValue::Integer(65535), "TCP window size", false));
    p.push(ConfigEntry::new("net.tcp.congestion",       Category::Network, ParamType::Enum(vec!["cubic".to_owned(),"reno".to_owned(),"bbr".to_owned()]), ConfigValue::Str("cubic".to_owned()), "TCP congestion algorithm", false));
    p.push(ConfigEntry::new("net.tcp.keepalive",        Category::Network, ParamType::Range(0, 7200),      ConfigValue::Integer(60),    "TCP keepalive interval (seconds)", false));
    p.push(ConfigEntry::new("net.tcp.syn_retries",      Category::Network, ParamType::Range(1, 10),        ConfigValue::Integer(3),     "TCP SYN retry count",              false));
    p.push(ConfigEntry::new("net.ipv4.forwarding",      Category::Network, ParamType::Bool,                ConfigValue::Bool(false),    "Enable IPv4 forwarding",           false));
    p.push(ConfigEntry::new("net.ipv6.enabled",         Category::Network, ParamType::Bool,                ConfigValue::Bool(true),     "Enable IPv6 support",              false));
    p.push(ConfigEntry::new("net.mtu",                  Category::Network, ParamType::Range(576, 9000),   ConfigValue::Integer(1500),  "Default MTU",                      false));
    p.push(ConfigEntry::new("net.dns.timeout",          Category::Network, ParamType::Range(1, 30),       ConfigValue::Integer(5),     "DNS query timeout (seconds)",       false));

    // -- security --
    p.push(ConfigEntry::new("security.capabilities",    Category::Security, ParamType::Bool, ConfigValue::Bool(true),  "Enable capability-based security", false));
    p.push(ConfigEntry::new("security.seccomp",         Category::Security, ParamType::Bool, ConfigValue::Bool(true),  "Enable seccomp filtering",         false));
    p.push(ConfigEntry::new("security.acl",             Category::Security, ParamType::Bool, ConfigValue::Bool(true),  "Enable access control lists",      false));
    p.push(ConfigEntry::new("security.aslr",            Category::Security, ParamType::Bool, ConfigValue::Bool(true),  "Enable ASLR",                      false));
    p.push(ConfigEntry::new("security.nx_bit",          Category::Security, ParamType::Bool, ConfigValue::Bool(true),  "Enforce NX bit on data pages",     true));
    p.push(ConfigEntry::new("security.audit",           Category::Security, ParamType::Bool, ConfigValue::Bool(false), "Enable security audit logging",    false));

    // -- scheduler --
    p.push(ConfigEntry::new("sched.policy",             Category::Scheduler, ParamType::Enum(vec!["roundrobin".to_owned(),"fifo".to_owned(),"cfs".to_owned(),"edf".to_owned()]), ConfigValue::Str("roundrobin".to_owned()), "Scheduler policy", false));
    p.push(ConfigEntry::new("sched.timeslice",          Category::Scheduler, ParamType::Range(1, 1000),  ConfigValue::Integer(10),    "Time slice in ms",             false));
    p.push(ConfigEntry::new("sched.rt_enabled",         Category::Scheduler, ParamType::Bool,            ConfigValue::Bool(true),     "Enable real-time scheduling",  false));
    p.push(ConfigEntry::new("sched.rt_priority_max",    Category::Scheduler, ParamType::Range(1, 99),    ConfigValue::Integer(99),    "Maximum RT priority",          false));
    p.push(ConfigEntry::new("sched.load_balance",       Category::Scheduler, ParamType::Bool,            ConfigValue::Bool(true),     "Enable SMP load balancing",    false));

    // -- filesystem --
    p.push(ConfigEntry::new("fs.max_inodes",            Category::Filesystem, ParamType::Range(16, 65536),   ConfigValue::Integer(64),    "Maximum number of inodes",         false));
    p.push(ConfigEntry::new("fs.max_file_size",         Category::Filesystem, ParamType::Range(512, 1048576), ConfigValue::Integer(4096), "Maximum file size in bytes",       false));
    p.push(ConfigEntry::new("fs.atime_update",          Category::Filesystem, ParamType::Bool,               ConfigValue::Bool(true),    "Update access time on read",       false));
    p.push(ConfigEntry::new("fs.sync_interval",         Category::Filesystem, ParamType::Range(1, 300),      ConfigValue::Integer(30),   "Sync interval in seconds",         false));
    p.push(ConfigEntry::new("fs.max_open_files",        Category::Filesystem, ParamType::Range(8, 4096),     ConfigValue::Integer(256),  "Max open file descriptors",        false));

    // -- audio --
    p.push(ConfigEntry::new("audio.enabled",            Category::Audio, ParamType::Bool,               ConfigValue::Bool(true),  "Enable audio subsystem",       false));
    p.push(ConfigEntry::new("audio.sample_rate",        Category::Audio, ParamType::Range(8000, 96000), ConfigValue::Integer(44100), "Audio sample rate (Hz)",    false));
    p.push(ConfigEntry::new("audio.buffer_size",        Category::Audio, ParamType::Range(64, 8192),    ConfigValue::Integer(1024), "Audio buffer size (frames)", false));
    p.push(ConfigEntry::new("audio.master_volume",      Category::Audio, ParamType::Range(0, 100),      ConfigValue::Integer(80),   "Master volume (%)",         false));

    // -- display --
    p.push(ConfigEntry::new("display.resolution_x",     Category::Display, ParamType::Range(320, 3840),  ConfigValue::Integer(1024), "Horizontal resolution",    true));
    p.push(ConfigEntry::new("display.resolution_y",     Category::Display, ParamType::Range(200, 2160),  ConfigValue::Integer(768),  "Vertical resolution",      true));
    p.push(ConfigEntry::new("display.depth",            Category::Display, ParamType::Range(8, 32),      ConfigValue::Integer(32),   "Color depth (bits)",       true));
    p.push(ConfigEntry::new("display.vsync",            Category::Display, ParamType::Bool,              ConfigValue::Bool(true),    "Enable vertical sync",     false));

    // -- ai --
    p.push(ConfigEntry::new("ai.enabled",               Category::AI, ParamType::Bool,                ConfigValue::Bool(true),                "Enable AI subsystem",      false));
    p.push(ConfigEntry::new("ai.proxy",                 Category::AI, ParamType::Enum(vec!["com1".to_owned(),"com2".to_owned(),"network".to_owned(),"none".to_owned()]), ConfigValue::Str("com2".to_owned()), "AI proxy transport", false));
    p.push(ConfigEntry::new("ai.shell_mode",            Category::AI, ParamType::Enum(vec!["assist".to_owned(),"auto".to_owned(),"disabled".to_owned()]),                ConfigValue::Str("assist".to_owned()), "AI shell mode", false));
    p.push(ConfigEntry::new("ai.max_tokens",            Category::AI, ParamType::Range(64, 32768),    ConfigValue::Integer(4096),             "Max tokens per AI query",  false));

    // -- power --
    p.push(ConfigEntry::new("power.governor",           Category::Power, ParamType::Enum(vec!["performance".to_owned(),"ondemand".to_owned(),"powersave".to_owned()]), ConfigValue::Str("ondemand".to_owned()), "CPU frequency governor", false));
    p.push(ConfigEntry::new("power.suspend_timeout",    Category::Power, ParamType::Range(0, 3600),   ConfigValue::Integer(300), "Auto-suspend timeout (seconds, 0=disabled)", false));
    p.push(ConfigEntry::new("power.acpi_enabled",       Category::Power, ParamType::Bool,             ConfigValue::Bool(true),   "Enable ACPI power management", true));

    // -- built-in profiles --
    register_profiles(store);
}

fn register_profiles(store: &mut ConfigStore) {
    store.profiles.push(ConfigProfile {
        name: "performance".to_owned(),
        description: "Maximize throughput and responsiveness".to_owned(),
        overrides: vec![
            ("kernel.hz".to_owned(),          "1000".to_owned()),
            ("kernel.preempt".to_owned(),     "true".to_owned()),
            ("sched.timeslice".to_owned(),    "5".to_owned()),
            ("sched.policy".to_owned(),       "cfs".to_owned()),
            ("memory.overcommit".to_owned(),  "true".to_owned()),
            ("net.tcp.window".to_owned(),     "1048576".to_owned()),
            ("power.governor".to_owned(),     "performance".to_owned()),
            ("power.suspend_timeout".to_owned(), "0".to_owned()),
            ("fs.atime_update".to_owned(),    "false".to_owned()),
        ],
    });

    store.profiles.push(ConfigProfile {
        name: "balanced".to_owned(),
        description: "Balance between performance and power usage".to_owned(),
        overrides: vec![
            ("kernel.hz".to_owned(),          "250".to_owned()),
            ("kernel.preempt".to_owned(),     "true".to_owned()),
            ("sched.timeslice".to_owned(),    "10".to_owned()),
            ("sched.policy".to_owned(),       "roundrobin".to_owned()),
            ("power.governor".to_owned(),     "ondemand".to_owned()),
            ("power.suspend_timeout".to_owned(), "300".to_owned()),
            ("net.tcp.window".to_owned(),     "65535".to_owned()),
        ],
    });

    store.profiles.push(ConfigProfile {
        name: "powersave".to_owned(),
        description: "Minimize power consumption".to_owned(),
        overrides: vec![
            ("kernel.hz".to_owned(),          "100".to_owned()),
            ("kernel.preempt".to_owned(),     "false".to_owned()),
            ("sched.timeslice".to_owned(),    "20".to_owned()),
            ("power.governor".to_owned(),     "powersave".to_owned()),
            ("power.suspend_timeout".to_owned(), "60".to_owned()),
            ("audio.enabled".to_owned(),      "false".to_owned()),
            ("display.vsync".to_owned(),      "false".to_owned()),
            ("net.tcp.window".to_owned(),     "16384".to_owned()),
        ],
    });

    store.profiles.push(ConfigProfile {
        name: "debug".to_owned(),
        description: "Verbose logging and safety checks".to_owned(),
        overrides: vec![
            ("kernel.log_level".to_owned(),   "trace".to_owned()),
            ("kernel.preempt".to_owned(),     "true".to_owned()),
            ("security.audit".to_owned(),     "true".to_owned()),
            ("memory.overcommit".to_owned(),  "false".to_owned()),
            ("memory.heap_warn_pct".to_owned(), "50".to_owned()),
            ("memory.heap_crit_pct".to_owned(), "75".to_owned()),
            ("fs.atime_update".to_owned(),    "true".to_owned()),
        ],
    });
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the extended configuration system with all built-in parameters.
pub fn init() {
    let mut store = STORE.lock();
    if store.initialized {
        return;
    }
    register_defaults(&mut store);
    store.initialized = true;
    let count = store.params.len();
    let profiles = store.profiles.len();
    drop(store);
    crate::klog_println!("[kconfig_ext] initialized {} parameters, {} profiles", count, profiles);
}

// ---------------------------------------------------------------------------
// Sysctl interface
// ---------------------------------------------------------------------------

/// Read a parameter by its dotted path. Returns the value string or error.
pub fn sysctl_read(path: &str) -> Result<String, &'static str> {
    READS.fetch_add(1, Ordering::Relaxed);
    let store = STORE.lock();
    for entry in store.params.iter() {
        if entry.name == path {
            return Ok(entry.current.display());
        }
    }
    ERRORS.fetch_add(1, Ordering::Relaxed);
    Err("parameter not found")
}

/// Write a parameter by its dotted path. Validates type and range.
pub fn sysctl_write(path: &str, value: &str) -> Result<(), &'static str> {
    WRITES.fetch_add(1, Ordering::Relaxed);
    let mut store = STORE.lock();
    for entry in store.params.iter_mut() {
        if entry.name == path {
            if entry.readonly {
                ERRORS.fetch_add(1, Ordering::Relaxed);
                return Err("parameter is read-only");
            }
            let parsed = ConfigValue::parse(value, &entry.ptype).map_err(|e| {
                ERRORS.fetch_add(1, Ordering::Relaxed);
                e
            })?;
            entry.current = parsed;
            return Ok(());
        }
    }
    ERRORS.fetch_add(1, Ordering::Relaxed);
    Err("parameter not found")
}

/// List all parameters whose path starts with the given prefix.
/// Pass an empty string to list everything.
pub fn sysctl_list(prefix: &str) -> Vec<(String, String)> {
    let store = STORE.lock();
    let mut result = Vec::new();
    for entry in store.params.iter() {
        if prefix.is_empty() || entry.name.starts_with(prefix) {
            result.push((entry.name.clone(), entry.current.display()));
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Runtime tuning helpers
// ---------------------------------------------------------------------------

/// Get a boolean parameter.
pub fn get_bool(path: &str) -> Option<bool> {
    let store = STORE.lock();
    for entry in store.params.iter() {
        if entry.name == path {
            if let ConfigValue::Bool(b) = &entry.current {
                return Some(*b);
            }
        }
    }
    None
}

/// Get an integer parameter.
pub fn get_int(path: &str) -> Option<i64> {
    let store = STORE.lock();
    for entry in store.params.iter() {
        if entry.name == path {
            if let ConfigValue::Integer(n) = &entry.current {
                return Some(*n);
            }
        }
    }
    None
}

/// Get a string parameter.
pub fn get_str(path: &str) -> Option<String> {
    let store = STORE.lock();
    for entry in store.params.iter() {
        if entry.name == path {
            match &entry.current {
                ConfigValue::Str(s) => return Some(s.clone()),
                other => return Some(other.display()),
            }
        }
    }
    None
}

/// Reset a single parameter to its default value.
pub fn reset_param(path: &str) -> Result<(), &'static str> {
    let mut store = STORE.lock();
    for entry in store.params.iter_mut() {
        if entry.name == path {
            if entry.readonly {
                return Err("parameter is read-only");
            }
            entry.current = entry.default.clone();
            return Ok(());
        }
    }
    Err("parameter not found")
}

/// Reset all non-readonly parameters to their defaults.
pub fn reset_all() {
    let mut store = STORE.lock();
    for entry in store.params.iter_mut() {
        if !entry.readonly {
            entry.current = entry.default.clone();
        }
    }
    crate::klog_println!("[kconfig_ext] all parameters reset to defaults");
}

// ---------------------------------------------------------------------------
// Config tree: hierarchical namespace helpers
// ---------------------------------------------------------------------------

/// List child nodes at a given tree level.
/// E.g., tree_children("kernel") returns ["hz","preempt","panic_timeout",...]
/// and tree_children("") returns ["kernel","memory","net","security",...].
pub fn tree_children(prefix: &str) -> Vec<String> {
    let store = STORE.lock();
    let mut children = Vec::new();
    let depth = if prefix.is_empty() { 0 } else { prefix.matches('.').count() + 1 };
    if depth >= MAX_DEPTH {
        return children;
    }

    for entry in store.params.iter() {
        let name = &entry.name;
        if !prefix.is_empty() && !name.starts_with(prefix) {
            continue;
        }
        if !prefix.is_empty() && !name[prefix.len()..].starts_with('.') {
            continue;
        }
        let suffix = if prefix.is_empty() { name.as_str() } else { &name[prefix.len() + 1..] };
        let child = if let Some(dot) = suffix.find('.') {
            &suffix[..dot]
        } else {
            suffix
        };
        let child_str = child.to_owned();
        if !children.contains(&child_str) {
            children.push(child_str);
        }
    }
    children
}

// ---------------------------------------------------------------------------
// Config profiles
// ---------------------------------------------------------------------------

/// Apply a named profile. Skips read-only parameters.
pub fn apply_profile(name: &str) -> Result<u32, &'static str> {
    PROFILE_APPLIES.fetch_add(1, Ordering::Relaxed);
    let mut store = STORE.lock();
    // Find the profile first and clone its overrides.
    let overrides = {
        let profile = store.profiles.iter().find(|p| p.name == name);
        match profile {
            Some(p) => p.overrides.clone(),
            None => return Err("profile not found"),
        }
    };
    let mut applied: u32 = 0;
    for (param_name, val_str) in overrides.iter() {
        for entry in store.params.iter_mut() {
            if entry.name == *param_name && !entry.readonly {
                if let Ok(parsed) = ConfigValue::parse(val_str, &entry.ptype) {
                    entry.current = parsed;
                    applied += 1;
                }
            }
        }
    }
    crate::klog_println!("[kconfig_ext] applied profile '{}': {} parameters changed", name, applied);
    Ok(applied)
}

/// List available profile names.
pub fn list_profiles() -> Vec<(String, String)> {
    let store = STORE.lock();
    store.profiles.iter().map(|p| (p.name.clone(), p.description.clone())).collect()
}

/// Save the current configuration as a new named profile.
pub fn save_profile(name: &str, description: &str) -> Result<(), &'static str> {
    let mut store = STORE.lock();
    if store.profiles.len() >= MAX_PROFILES {
        return Err("too many profiles");
    }
    // Collect all modified (non-default) parameters.
    let mut overrides = Vec::new();
    for entry in store.params.iter() {
        if entry.is_modified() && !entry.readonly {
            overrides.push((entry.name.clone(), entry.current.display()));
        }
    }
    // Replace if profile already exists.
    for p in store.profiles.iter_mut() {
        if p.name == name {
            p.description = description.to_owned();
            p.overrides = overrides;
            return Ok(());
        }
    }
    store.profiles.push(ConfigProfile {
        name: name.to_owned(),
        description: description.to_owned(),
        overrides,
    });
    Ok(())
}

/// Delete a named profile.
pub fn delete_profile(name: &str) -> Result<(), &'static str> {
    let mut store = STORE.lock();
    let len_before = store.profiles.len();
    store.profiles.retain(|p| p.name != name);
    if store.profiles.len() == len_before {
        Err("profile not found")
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Config diff
// ---------------------------------------------------------------------------

/// Return a list of parameters that differ from their defaults.
pub fn config_diff() -> Vec<(String, String, String)> {
    let store = STORE.lock();
    let mut diffs = Vec::new();
    for entry in store.params.iter() {
        if entry.is_modified() {
            diffs.push((
                entry.name.clone(),
                entry.default.display(),
                entry.current.display(),
            ));
        }
    }
    diffs
}

// ---------------------------------------------------------------------------
// Export / import (TOML-like format)
// ---------------------------------------------------------------------------

/// Serialize current configuration to a TOML-like string.
pub fn export_config() -> String {
    let store = STORE.lock();
    let mut out = String::from("# MerlionOS Extended Configuration\n");
    out.push_str("# Generated by kconfig_ext\n\n");

    let mut current_category: Option<Category> = None;

    for entry in store.params.iter() {
        if current_category != Some(entry.category) {
            if current_category.is_some() {
                out.push('\n');
            }
            out.push('[');
            out.push_str(entry.category.name());
            out.push_str("]\n");
            current_category = Some(entry.category);
        }
        // Strip category prefix from the key for TOML-style grouping.
        let short_key = strip_category_prefix(&entry.name, entry.category);
        out.push_str(&format!("{} = {}\n", short_key, entry.current.display()));
    }
    out
}

/// Import configuration from a TOML-like string. Skips unknown keys and
/// read-only parameters. Returns the count of parameters updated.
pub fn import_config(content: &str) -> u32 {
    let mut updated: u32 = 0;
    let mut current_section = String::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Section header: [category]
        if line.starts_with('[') && line.ends_with(']') {
            current_section = line[1..line.len() - 1].trim().to_owned();
            continue;
        }
        // key = value
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            let full_path = if current_section.is_empty() {
                key.to_owned()
            } else {
                format!("{}.{}", current_section, key)
            };
            if sysctl_write(&full_path, value).is_ok() {
                updated += 1;
            }
        }
    }
    crate::klog_println!("[kconfig_ext] imported {} parameters", updated);
    updated
}

/// Load configuration from a VFS file path.
pub fn load_from_file(path: &str) -> Result<u32, &'static str> {
    let content = crate::vfs::cat(path).map_err(|_| "failed to read config file")?;
    Ok(import_config(&content))
}

/// Save current configuration to a VFS file path.
pub fn save_to_file(path: &str) -> Result<(), &'static str> {
    let content = export_config();
    crate::vfs::write(path, &content)
}

// ---------------------------------------------------------------------------
// Info / stats / dump
// ---------------------------------------------------------------------------

/// Return a short summary string about the configuration system.
pub fn config_info() -> String {
    let store = STORE.lock();
    let total = store.params.len();
    let modified = store.params.iter().filter(|e| e.is_modified()).count();
    let readonly = store.params.iter().filter(|e| e.readonly).count();
    let profiles = store.profiles.len();
    format!(
        "kconfig_ext: {} params ({} modified, {} read-only), {} profiles",
        total, modified, readonly, profiles
    )
}

/// Return statistics about sysctl usage.
pub fn config_stats() -> String {
    format!(
        "kconfig_ext stats: reads={}, writes={}, errors={}, profile_applies={}",
        READS.load(Ordering::Relaxed),
        WRITES.load(Ordering::Relaxed),
        ERRORS.load(Ordering::Relaxed),
        PROFILE_APPLIES.load(Ordering::Relaxed),
    )
}

/// Dump all parameters with their details (for debugging).
pub fn dump_config() -> String {
    let store = STORE.lock();
    let mut out = String::new();
    out.push_str(&format!("{:<30} {:<10} {:<12} {:<12} {}\n",
        "PARAMETER", "CATEGORY", "CURRENT", "DEFAULT", "DESCRIPTION"));
    out.push_str(&format!("{}\n", "-".repeat(90)));
    for entry in store.params.iter() {
        let marker = if entry.readonly { " [ro]" } else if entry.is_modified() { " [*]" } else { "" };
        out.push_str(&format!("{:<30} {:<10} {:<12} {:<12} {}{}\n",
            entry.name,
            entry.category.name(),
            entry.current.display(),
            entry.default.display(),
            entry.description,
            marker,
        ));
    }
    out
}

/// Describe a single parameter in detail.
pub fn describe_param(path: &str) -> Option<String> {
    let store = STORE.lock();
    for entry in store.params.iter() {
        if entry.name == path {
            let type_str = match &entry.ptype {
                ParamType::Bool => "bool".to_owned(),
                ParamType::Integer => "integer".to_owned(),
                ParamType::Str => "string".to_owned(),
                ParamType::Enum(v) => format!("enum({})", v.join("|")),
                ParamType::Range(lo, hi) => format!("range({}..{})", lo, hi),
            };
            return Some(format!(
                "Parameter: {}\nCategory:  {}\nType:      {}\nDefault:   {}\nCurrent:   {}\nReadonly:  {}\nDescription: {}",
                entry.name,
                entry.category.name(),
                type_str,
                entry.default.display(),
                entry.current.display(),
                entry.readonly,
                entry.description,
            ));
        }
    }
    None
}

/// Register a custom parameter at runtime.
pub fn register_param(
    name: &str,
    category: Category,
    ptype: ParamType,
    default: ConfigValue,
    description: &str,
    readonly: bool,
) -> Result<(), &'static str> {
    let mut store = STORE.lock();
    if store.params.len() >= MAX_PARAMS {
        return Err("maximum parameter count reached");
    }
    // Check for duplicates.
    if store.params.iter().any(|e| e.name == name) {
        return Err("parameter already exists");
    }
    store.params.push(ConfigEntry::new(name, category, ptype, default, description, readonly));
    Ok(())
}

/// Unregister a custom parameter (built-in parameters cannot be removed).
pub fn unregister_param(name: &str) -> Result<(), &'static str> {
    let mut store = STORE.lock();
    let len_before = store.params.len();
    store.params.retain(|e| e.name != name);
    if store.params.len() == len_before {
        Err("parameter not found")
    } else {
        Ok(())
    }
}

/// Return the total number of registered parameters.
pub fn param_count() -> usize {
    STORE.lock().params.len()
}

/// Return the number of parameters by category.
pub fn params_by_category(cat: Category) -> Vec<(String, String)> {
    let store = STORE.lock();
    store.params.iter()
        .filter(|e| e.category == cat)
        .map(|e| (e.name.clone(), e.current.display()))
        .collect()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Strip the leading category prefix from a parameter name.
/// E.g., "kernel.hz" with category Kernel ("kernel") becomes "hz".
/// For nested paths like "net.tcp.window", returns "tcp.window".
fn strip_category_prefix(name: &str, cat: Category) -> String {
    let prefix = cat.name();
    if name.starts_with(prefix) && name.len() > prefix.len() && name.as_bytes()[prefix.len()] == b'.' {
        name[prefix.len() + 1..].to_owned()
    } else {
        name.to_owned()
    }
}
