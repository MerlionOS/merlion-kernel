/// ELF dynamic runtime linker for MerlionOS.
/// Handles dynamic library loading, symbol resolution, PLT/GOT relocation,
/// and shared library management.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

const MAX_LIBRARIES: usize = 64;
const MAX_SYMBOLS: usize = 4096;
const MAX_PRELOADS: usize = 8;

// x86_64 ELF relocation types.
const R_X86_64_64: u32 = 1;        // S + A
const R_X86_64_PC32: u32 = 2;      // S + A - P
const R_X86_64_GLOB_DAT: u32 = 6;  // S
const R_X86_64_JUMP_SLOT: u32 = 7; // S
const R_X86_64_RELATIVE: u32 = 8;  // B + A

/// How strongly a symbol is bound in the global table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolBinding {
    Strong,
    Weak,
}

/// A resolved symbol: name, virtual address, and binding.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub address: u64,
    pub binding: SymbolBinding,
    pub owner: u32,
}

/// A single relocation to apply when loading a library.
#[derive(Debug, Clone)]
pub struct RelocationEntry {
    pub offset: u64,
    pub rel_type: u32,
    pub symbol_name: String,
    pub addend: i64,
}

/// A single Global Offset Table entry.
#[derive(Debug, Clone)]
pub struct GotEntry {
    pub index: usize,
    pub symbol_name: String,
    pub resolved_addr: u64,
    pub bound: bool,
}

/// PLT stub metadata for lazy binding.
#[derive(Debug, Clone)]
pub struct PltStub {
    pub stub_addr: u64,
    pub got_index: usize,
    pub symbol_name: String,
}

/// A loaded shared library image.
#[derive(Debug, Clone)]
pub struct SharedLibrary {
    pub handle: u32,
    pub name: String,
    pub base_address: u64,
    pub size: u64,
    pub symbols: Vec<Symbol>,
    pub relocations: Vec<RelocationEntry>,
    pub got: Vec<GotEntry>,
    pub plt_stubs: Vec<PltStub>,
    pub dependencies: Vec<String>,
    pub ref_count: u32,
}

// ── Linker state ────────────────────────────────────────────────────

/// Runtime linker state: library cache, global symbol table, preloads.
pub struct LinkerState {
    libraries: Vec<SharedLibrary>,
    global_symbols: Vec<Symbol>,
    preload_list: Vec<String>,
    next_handle: u32,
    next_base: u64,
}

impl LinkerState {
    pub fn new() -> Self {
        Self {
            libraries: Vec::new(),
            global_symbols: Vec::new(),
            preload_list: Vec::new(),
            next_handle: 1,
            next_base: 0x0000_7000_0000_0000,
        }
    }

    /// Look up a symbol by name.  Strong symbols take precedence over weak.
    pub fn resolve_symbol(&self, name: &str) -> Option<u64> {
        let mut weak_addr: Option<u64> = None;
        for sym in &self.global_symbols {
            if sym.name == name {
                if sym.binding == SymbolBinding::Strong { return Some(sym.address); }
                if weak_addr.is_none() { weak_addr = Some(sym.address); }
            }
        }
        weak_addr
    }

    /// Register a symbol.  Strong replaces weak of the same name.
    fn register_symbol(&mut self, sym: Symbol) {
        if self.global_symbols.len() >= MAX_SYMBOLS { return; }
        if sym.binding == SymbolBinding::Strong {
            if let Some(existing) = self.global_symbols.iter_mut().find(|s| s.name == sym.name) {
                if existing.binding == SymbolBinding::Weak {
                    *existing = sym;
                    return;
                }
            }
        }
        self.global_symbols.push(sym);
    }

    /// Apply a single relocation for a library loaded at `base`.
    fn apply_relocation(&self, base: u64, rel: &RelocationEntry) -> bool {
        let p = base.wrapping_add(rel.offset);
        match rel.rel_type {
            R_X86_64_64 => {
                if let Some(s) = self.resolve_symbol(&rel.symbol_name) {
                    unsafe { *(p as *mut u64) = s.wrapping_add(rel.addend as u64); }
                    true
                } else { false }
            }
            R_X86_64_PC32 => {
                if let Some(s) = self.resolve_symbol(&rel.symbol_name) {
                    let v = (s as i64).wrapping_add(rel.addend).wrapping_sub(p as i64);
                    unsafe { *(p as *mut u32) = v as u32; }
                    true
                } else { false }
            }
            R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {
                if let Some(s) = self.resolve_symbol(&rel.symbol_name) {
                    unsafe { *(p as *mut u64) = s; }
                    true
                } else { false }
            }
            R_X86_64_RELATIVE => {
                unsafe { *(p as *mut u64) = base.wrapping_add(rel.addend as u64); }
                true
            }
            _ => false,
        }
    }

    /// Apply all relocations for a library.
    fn apply_all_relocations(&self, lib: &SharedLibrary) -> usize {
        let mut n = 0usize;
        for rel in &lib.relocations {
            if self.apply_relocation(lib.base_address, rel) { n += 1; }
        }
        n
    }

    /// Resolve a single GOT entry by index (lazy binding).
    pub fn bind_got_entry(&mut self, handle: u32, got_index: usize) -> Option<u64> {
        let lib = self.libraries.iter_mut().find(|l| l.handle == handle)?;
        let entry = lib.got.get_mut(got_index)?;
        if entry.bound { return Some(entry.resolved_addr); }
        let name = entry.symbol_name.clone();
        drop(lib);
        let addr = self.resolve_symbol(&name)?;
        let lib = self.libraries.iter_mut().find(|l| l.handle == handle)?;
        let entry = lib.got.get_mut(got_index)?;
        entry.resolved_addr = addr;
        entry.bound = true;
        STATS.lazy_binds.fetch_add(1, Ordering::Relaxed);
        Some(addr)
    }

    /// Eagerly bind all GOT entries (LD_BIND_NOW equivalent).
    fn bind_all_got(&mut self, handle: u32) -> usize {
        let lib = match self.libraries.iter().find(|l| l.handle == handle) {
            Some(l) => l, None => return 0,
        };
        let pending: Vec<(usize, String)> = lib.got.iter().enumerate()
            .filter(|(_, e)| !e.bound)
            .map(|(i, e)| (i, e.symbol_name.clone()))
            .collect();
        let mut n = 0usize;
        for (idx, name) in &pending {
            if let Some(addr) = self.resolve_symbol(name) {
                if let Some(lib) = self.libraries.iter_mut().find(|l| l.handle == handle) {
                    if let Some(entry) = lib.got.get_mut(*idx) {
                        entry.resolved_addr = addr;
                        entry.bound = true;
                        n += 1;
                    }
                }
            }
        }
        n
    }

    fn find_library(&self, name: &str) -> Option<u32> {
        self.libraries.iter().find(|l| l.name == name).map(|l| l.handle)
    }

    /// Load a shared library by path.  Returns cached handle if already loaded.
    pub fn dlopen(&mut self, path: &str) -> Result<u32, &'static str> {
        if let Some(handle) = self.find_library(path) {
            if let Some(lib) = self.libraries.iter_mut().find(|l| l.handle == handle) {
                lib.ref_count += 1;
            }
            return Ok(handle);
        }
        if self.libraries.len() >= MAX_LIBRARIES {
            return Err("too many loaded libraries");
        }
        let handle = self.next_handle;
        self.next_handle += 1;
        let base = self.next_base;
        let size = 0x10000u64;
        self.next_base = self.next_base.wrapping_add(size + 0x1000);

        let lib = SharedLibrary {
            handle, name: String::from(path), base_address: base, size,
            symbols: Vec::new(), relocations: Vec::new(), got: Vec::new(),
            plt_stubs: Vec::new(), dependencies: Vec::new(), ref_count: 1,
        };
        self.libraries.push(lib);
        if let Some(lib) = self.libraries.iter().find(|l| l.handle == handle) {
            let syms: Vec<Symbol> = lib.symbols.clone();
            for sym in syms { self.register_symbol(sym); }
        }
        STATS.libs_loaded.fetch_add(1, Ordering::Relaxed);
        Ok(handle)
    }

    /// Look up a symbol.  Use `handle = 0` for global scope.
    pub fn dlsym(&self, handle: u32, symbol: &str) -> Option<u64> {
        if handle == 0 { return self.resolve_symbol(symbol); }
        let lib = self.libraries.iter().find(|l| l.handle == handle)?;
        lib.symbols.iter().find(|s| s.name == symbol).map(|s| s.address)
    }

    /// Close a library handle.  Unloads when ref count reaches zero.
    pub fn dlclose(&mut self, handle: u32) -> bool {
        let idx = match self.libraries.iter().position(|l| l.handle == handle) {
            Some(i) => i, None => return false,
        };
        self.libraries[idx].ref_count = self.libraries[idx].ref_count.saturating_sub(1);
        if self.libraries[idx].ref_count == 0 {
            let lib = self.libraries.remove(idx);
            self.global_symbols.retain(|s| s.owner != lib.handle);
            STATS.libs_loaded.fetch_sub(1, Ordering::Relaxed);
        }
        true
    }

    /// Recursively load all transitive dependencies of a library.
    pub fn load_dependencies(&mut self, handle: u32) -> Result<usize, &'static str> {
        let deps = match self.libraries.iter().find(|l| l.handle == handle) {
            Some(lib) => lib.dependencies.clone(),
            None => return Err("library not found"),
        };
        let mut total = 0usize;
        for dep in &deps {
            let dh = self.dlopen(dep)?;
            total += 1;
            total += self.load_dependencies(dh)?;
        }
        Ok(total)
    }

    /// Add a library to the preload list (LD_PRELOAD equivalent).
    pub fn add_preload(&mut self, path: &str) {
        if self.preload_list.len() < MAX_PRELOADS {
            self.preload_list.push(String::from(path));
        }
    }

    /// Load all preloaded libraries so their symbols take precedence.
    pub fn load_preloads(&mut self) -> usize {
        let paths: Vec<String> = self.preload_list.clone();
        let mut n = 0usize;
        for p in &paths { if self.dlopen(p).is_ok() { n += 1; } }
        n
    }

    pub fn preload_list(&self) -> &[String] { &self.preload_list }

    /// Produce a human-readable summary of all loaded libraries.
    pub fn list_libraries(&self) -> String {
        if self.libraries.is_empty() {
            return String::from("(no shared libraries loaded)\n");
        }
        let mut out = String::new();
        out.push_str("HANDLE  BASE              SIZE      REFS  NAME\n");
        out.push_str("------  ----------------  --------  ----  ----\n");
        for lib in &self.libraries {
            out.push_str(&format!(
                "{:<6}  {:#016x}  {:<8}  {:<4}  {}\n",
                lib.handle, lib.base_address, lib.size, lib.ref_count, lib.name
            ));
        }
        out
    }
}

// ── Global state ────────────────────────────────────────────────────

/// Atomic counters for linker activity.
pub struct LinkerStats {
    pub libs_loaded: AtomicU64,
    pub symbols_resolved: AtomicU64,
    pub relocations_applied: AtomicU64,
    pub lazy_binds: AtomicU64,
}

impl LinkerStats {
    const fn new() -> Self {
        Self {
            libs_loaded: AtomicU64::new(0),
            symbols_resolved: AtomicU64::new(0),
            relocations_applied: AtomicU64::new(0),
            lazy_binds: AtomicU64::new(0),
        }
    }
}

pub static STATS: LinkerStats = LinkerStats::new();
pub static LINKER: Mutex<Option<LinkerState>> = Mutex::new(None);

// ── Public free-standing API ────────────────────────────────────────

pub fn dlopen(path: &str) -> Result<u32, &'static str> {
    LINKER.lock().as_mut().ok_or("linker not initialised")?.dlopen(path)
}

pub fn dlsym(handle: u32, symbol: &str) -> Option<u64> {
    LINKER.lock().as_ref().and_then(|l| l.dlsym(handle, symbol))
}

pub fn dlclose(handle: u32) -> bool {
    LINKER.lock().as_mut().map_or(false, |l| l.dlclose(handle))
}

/// Return a formatted summary of the runtime linker state.
pub fn linker_info() -> String {
    let guard = LINKER.lock();
    let state = match guard.as_ref() {
        Some(s) => s,
        None => return String::from("(runtime linker not initialised)\n"),
    };
    let mut out = state.list_libraries();
    out.push_str(&format!(
        "\nSymbols in global table: {}\n\
         Stats: libs={}, symbols_resolved={}, relocations={}, lazy_binds={}\n",
        state.global_symbols.len(),
        STATS.libs_loaded.load(Ordering::Relaxed),
        STATS.symbols_resolved.load(Ordering::Relaxed),
        STATS.relocations_applied.load(Ordering::Relaxed),
        STATS.lazy_binds.load(Ordering::Relaxed),
    ));
    if !state.preload_list.is_empty() {
        out.push_str("Preload: ");
        for (i, p) in state.preload_list.iter().enumerate() {
            if i > 0 { out.push_str(", "); }
            out.push_str(p);
        }
        out.push('\n');
    }
    out
}

/// Initialise the ELF dynamic runtime linker subsystem.
pub fn init() {
    *LINKER.lock() = Some(LinkerState::new());
}
