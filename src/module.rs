/// Loadable kernel module framework.
///
/// Modules are compiled into the kernel but can be dynamically loaded
/// and unloaded at runtime. Each module implements the `KernelModule`
/// trait with `init()` and `cleanup()` lifecycle hooks.
///
/// In a future phase, this could be extended to load modules from
/// disk (ELF .ko files), but for now all modules are built-in.

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::borrow::ToOwned;
use spin::Mutex;

const MAX_MODULES: usize = 32;

static MODULES: Mutex<Vec<ModuleEntry>> = Mutex::new(Vec::new());

/// Trait that all kernel modules must implement.
pub trait KernelModule: Send + Sync {
    /// Module name (e.g. "hello", "watchdog").
    fn name(&self) -> &str;

    /// One-line description.
    fn description(&self) -> &str;

    /// Module version.
    fn version(&self) -> &str { "0.1.0" }

    /// Called when the module is loaded. Return Ok(()) on success.
    fn init(&self) -> Result<(), &'static str>;

    /// Called when the module is unloaded.
    fn cleanup(&self);
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModuleState {
    Loaded,
    Unloaded,
}

struct ModuleEntry {
    module: Box<dyn KernelModule>,
    state: ModuleState,
}

/// Module info for display.
pub struct ModuleInfo {
    pub name: String,
    pub description: String,
    pub version: String,
    pub state: ModuleState,
}

/// Register a module (does not load it).
pub fn register(module: Box<dyn KernelModule>) -> Result<(), &'static str> {
    let mut modules = MODULES.lock();
    if modules.len() >= MAX_MODULES {
        return Err("module table full");
    }
    // Check for duplicate name
    let name = module.name();
    if modules.iter().any(|m| m.module.name() == name) {
        return Err("module already registered");
    }
    modules.push(ModuleEntry {
        module,
        state: ModuleState::Unloaded,
    });
    Ok(())
}

/// Load (initialize) a registered module by name.
pub fn load(name: &str) -> Result<(), &'static str> {
    let mut modules = MODULES.lock();
    for entry in modules.iter_mut() {
        if entry.module.name() == name {
            if entry.state == ModuleState::Loaded {
                return Err("module already loaded");
            }
            entry.module.init()?;
            entry.state = ModuleState::Loaded;
            crate::serial_println!("[module] loaded '{}'", name);
            crate::klog_println!("[module] loaded '{}'", name);
            return Ok(());
        }
    }
    Err("module not found")
}

/// Unload a module by name.
pub fn unload(name: &str) -> Result<(), &'static str> {
    let mut modules = MODULES.lock();
    for entry in modules.iter_mut() {
        if entry.module.name() == name {
            if entry.state == ModuleState::Unloaded {
                return Err("module not loaded");
            }
            entry.module.cleanup();
            entry.state = ModuleState::Unloaded;
            crate::serial_println!("[module] unloaded '{}'", name);
            crate::klog_println!("[module] unloaded '{}'", name);
            return Ok(());
        }
    }
    Err("module not found")
}

/// List all registered modules.
pub fn list() -> Vec<ModuleInfo> {
    let modules = MODULES.lock();
    modules.iter().map(|m| ModuleInfo {
        name: m.module.name().to_owned(),
        description: m.module.description().to_owned(),
        version: m.module.version().to_owned(),
        state: m.state,
    }).collect()
}

/// Get info about a specific module.
pub fn info(name: &str) -> Option<ModuleInfo> {
    let modules = MODULES.lock();
    modules.iter().find(|m| m.module.name() == name).map(|m| ModuleInfo {
        name: m.module.name().to_owned(),
        description: m.module.description().to_owned(),
        version: m.module.version().to_owned(),
        state: m.state,
    })
}

// ─── Built-in modules ────────────────────────────────────────

/// "hello" demo module — prints a message on load/unload.
struct HelloModule;

impl KernelModule for HelloModule {
    fn name(&self) -> &str { "hello" }
    fn description(&self) -> &str { "Demo module that prints hello/goodbye" }
    fn init(&self) -> Result<(), &'static str> {
        crate::println!("[hello] Hello from kernel module!");
        Ok(())
    }
    fn cleanup(&self) {
        crate::println!("[hello] Goodbye from kernel module!");
    }
}

/// "watchdog" module — monitors system uptime and logs warnings.
struct WatchdogModule;

impl KernelModule for WatchdogModule {
    fn name(&self) -> &str { "watchdog" }
    fn description(&self) -> &str { "System watchdog timer monitor" }
    fn init(&self) -> Result<(), &'static str> {
        let ticks = crate::timer::ticks();
        crate::println!("[watchdog] Started at tick {}", ticks);
        crate::klog_println!("[watchdog] initialized at tick {}", ticks);
        Ok(())
    }
    fn cleanup(&self) {
        let ticks = crate::timer::ticks();
        crate::println!("[watchdog] Stopped at tick {}", ticks);
    }
}

/// "memstat" module — adds memory statistics to kernel log on load.
struct MemstatModule;

impl KernelModule for MemstatModule {
    fn name(&self) -> &str { "memstat" }
    fn description(&self) -> &str { "Memory statistics reporter" }
    fn init(&self) -> Result<(), &'static str> {
        let mem = crate::memory::stats();
        let heap = crate::allocator::stats();
        crate::println!("[memstat] Physical: {} KiB usable, {} frames allocated",
            mem.total_usable_bytes / 1024, mem.allocated_frames);
        crate::println!("[memstat] Heap: {}/{} bytes used", heap.used, heap.total);
        Ok(())
    }
    fn cleanup(&self) {
        crate::println!("[memstat] Reporter stopped.");
    }
}

/// Register all built-in modules (called at boot).
pub fn init() {
    let _ = register(Box::new(HelloModule));
    let _ = register(Box::new(WatchdogModule));
    let _ = register(Box::new(MemstatModule));
}
