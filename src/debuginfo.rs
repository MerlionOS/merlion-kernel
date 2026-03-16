/// Debug information parser for MerlionOS.
/// Parses DWARF-like debug info to provide source file/line information
/// in stack traces, and supports breakpoint management.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

const MAX_LINE_ENTRIES: usize = 4096;
const MAX_FUNCTIONS: usize = 1024;
const MAX_BREAKPOINTS: usize = 64;
const MAX_WATCHPOINTS: usize = 32;
const MAX_UNWIND_DEPTH: usize = 64;

/// INT3 opcode used for software breakpoints.
const INT3_OPCODE: u8 = 0xCC;

// ── Source location ─────────────────────────────────────────────────

/// Mapping from an instruction address to source file / line / column.
#[derive(Debug, Clone)]
pub struct SourceLocation {
    pub address: u64,
    pub file: String,
    pub line: u32,
    pub column: u32,
}

// ── Line number table ───────────────────────────────────────────────

/// A row in the line number program mapping an address range to source.
#[derive(Debug, Clone)]
pub struct LineEntry {
    pub start_addr: u64,
    pub end_addr: u64,
    pub file: String,
    pub line: u32,
    pub column: u32,
}

/// Simplified DWARF-like line number table, sorted by start address.
pub struct LineNumberTable {
    entries: Vec<LineEntry>,
}

impl LineNumberTable {
    pub fn new() -> Self { Self { entries: Vec::new() } }

    /// Insert a mapping in sorted order.
    pub fn add(&mut self, entry: LineEntry) {
        if self.entries.len() >= MAX_LINE_ENTRIES { return; }
        let pos = self.entries.iter()
            .position(|e| e.start_addr > entry.start_addr)
            .unwrap_or(self.entries.len());
        self.entries.insert(pos, entry);
    }

    /// Binary-search for the source location covering `addr`.
    pub fn lookup(&self, addr: u64) -> Option<SourceLocation> {
        let mut lo = 0usize;
        let mut hi = self.entries.len();
        let mut result: Option<&LineEntry> = None;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.entries[mid].start_addr <= addr {
                result = Some(&self.entries[mid]);
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let e = result?;
        if e.end_addr != 0 && addr >= e.end_addr { return None; }
        Some(SourceLocation { address: addr, file: e.file.clone(), line: e.line, column: e.column })
    }

    pub fn len(&self) -> usize { self.entries.len() }
}

// ── Function info ───────────────────────────────────────────────────

/// Metadata about a function from debug information.
#[derive(Debug, Clone)]
pub struct FunctionInfo {
    pub name: String,
    pub start_addr: u64,
    pub end_addr: u64,
    pub source_file: String,
    pub source_line: u32,
    pub params: Vec<String>,
}

/// Registry of known functions, sorted by start address.
pub struct FunctionTable {
    entries: Vec<FunctionInfo>,
}

impl FunctionTable {
    pub fn new() -> Self { Self { entries: Vec::new() } }

    pub fn add(&mut self, info: FunctionInfo) {
        if self.entries.len() >= MAX_FUNCTIONS { return; }
        let pos = self.entries.iter()
            .position(|e| e.start_addr > info.start_addr)
            .unwrap_or(self.entries.len());
        self.entries.insert(pos, info);
    }

    /// Find the function containing `addr`.
    pub fn lookup(&self, addr: u64) -> Option<&FunctionInfo> {
        let mut lo = 0usize;
        let mut hi = self.entries.len();
        let mut result: Option<&FunctionInfo> = None;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.entries[mid].start_addr <= addr {
                result = Some(&self.entries[mid]);
                lo = mid + 1;
            } else { hi = mid; }
        }
        let f = result?;
        if addr < f.end_addr { Some(f) } else { None }
    }

    pub fn len(&self) -> usize { self.entries.len() }
}

// ── Breakpoint manager ──────────────────────────────────────────────

/// A software breakpoint (INT3 replacement).
#[derive(Debug, Clone)]
pub struct Breakpoint {
    pub id: u32,
    pub address: u64,
    pub original_byte: u8,
    pub enabled: bool,
}

pub struct BreakpointManager {
    breakpoints: Vec<Breakpoint>,
    next_id: u32,
}

impl BreakpointManager {
    pub fn new() -> Self { Self { breakpoints: Vec::new(), next_id: 1 } }

    /// Set a breakpoint at `addr`.  Returns the breakpoint id.
    pub fn set(&mut self, addr: u64) -> Option<u32> {
        if self.breakpoints.iter().any(|b| b.address == addr) { return None; }
        if self.breakpoints.len() >= MAX_BREAKPOINTS { return None; }
        let id = self.next_id;
        self.next_id += 1;
        let original = unsafe { *(addr as *const u8) };
        unsafe { *(addr as *mut u8) = INT3_OPCODE; }
        self.breakpoints.push(Breakpoint { id, address: addr, original_byte: original, enabled: true });
        STATS.breakpoints_set.fetch_add(1, Ordering::Relaxed);
        Some(id)
    }

    /// Clear the breakpoint at `addr`, restoring the original byte.
    pub fn clear(&mut self, addr: u64) -> bool {
        let idx = match self.breakpoints.iter().position(|b| b.address == addr) {
            Some(i) => i, None => return false,
        };
        let bp = self.breakpoints.remove(idx);
        if bp.enabled { unsafe { *(bp.address as *mut u8) = bp.original_byte; } }
        true
    }

    pub fn enable(&mut self, id: u32) -> bool {
        if let Some(bp) = self.breakpoints.iter_mut().find(|b| b.id == id) {
            if !bp.enabled {
                bp.original_byte = unsafe { *(bp.address as *const u8) };
                unsafe { *(bp.address as *mut u8) = INT3_OPCODE; }
                bp.enabled = true;
            }
            true
        } else { false }
    }

    pub fn disable(&mut self, id: u32) -> bool {
        if let Some(bp) = self.breakpoints.iter_mut().find(|b| b.id == id) {
            if bp.enabled {
                unsafe { *(bp.address as *mut u8) = bp.original_byte; }
                bp.enabled = false;
            }
            true
        } else { false }
    }

    pub fn list(&self) -> String {
        if self.breakpoints.is_empty() { return String::from("(no breakpoints)\n"); }
        let mut out = String::new();
        out.push_str("ID   ADDRESS           ENABLED  ORIG\n");
        out.push_str("---- ----------------  -------  ----\n");
        for bp in &self.breakpoints {
            out.push_str(&format!(
                "{:<4} {:#016x}  {:<7}  {:#04x}\n",
                bp.id, bp.address, if bp.enabled { "yes" } else { "no" }, bp.original_byte,
            ));
        }
        out
    }

    pub fn count(&self) -> usize { self.breakpoints.len() }
}

// ── Watchpoints ─────────────────────────────────────────────────────

/// Type of memory access a watchpoint monitors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchKind { Write, Read, ReadWrite }

/// A simulated memory watchpoint.
#[derive(Debug, Clone)]
pub struct Watchpoint {
    pub id: u32,
    pub address: u64,
    pub size: usize,
    pub kind: WatchKind,
    pub enabled: bool,
    pub hit_count: u64,
}

pub struct WatchpointManager {
    watchpoints: Vec<Watchpoint>,
    next_id: u32,
}

impl WatchpointManager {
    pub fn new() -> Self { Self { watchpoints: Vec::new(), next_id: 1 } }

    pub fn add(&mut self, address: u64, size: usize, kind: WatchKind) -> Option<u32> {
        if self.watchpoints.len() >= MAX_WATCHPOINTS { return None; }
        let id = self.next_id;
        self.next_id += 1;
        self.watchpoints.push(Watchpoint { id, address, size, kind, enabled: true, hit_count: 0 });
        Some(id)
    }

    pub fn remove(&mut self, id: u32) -> bool {
        if let Some(pos) = self.watchpoints.iter().position(|w| w.id == id) {
            self.watchpoints.remove(pos);
            true
        } else { false }
    }

    /// Check whether an access triggers any watchpoint.  Returns matching ids.
    pub fn check(&mut self, addr: u64, size: usize, kind: WatchKind) -> Vec<u32> {
        let mut hits = Vec::new();
        let end = addr.wrapping_add(size as u64);
        for wp in &mut self.watchpoints {
            if !wp.enabled { continue; }
            let wp_end = wp.address.wrapping_add(wp.size as u64);
            if addr < wp_end && end > wp.address {
                let m = wp.kind == WatchKind::ReadWrite || wp.kind == kind;
                if m { wp.hit_count += 1; hits.push(wp.id); }
            }
        }
        hits
    }

    pub fn list(&self) -> String {
        if self.watchpoints.is_empty() { return String::from("(no watchpoints)\n"); }
        let mut out = String::new();
        out.push_str("ID   ADDRESS           SIZE  KIND   ENABLED  HITS\n");
        out.push_str("---- ----------------  ----  -----  -------  ----\n");
        for wp in &self.watchpoints {
            let k = match wp.kind { WatchKind::Write => "write", WatchKind::Read => "read ", WatchKind::ReadWrite => "rw   " };
            out.push_str(&format!(
                "{:<4} {:#016x}  {:<4}  {}  {:<7}  {}\n",
                wp.id, wp.address, wp.size, k, if wp.enabled { "yes" } else { "no" }, wp.hit_count,
            ));
        }
        out
    }
}

// ── Stack unwinder ──────────────────────────────────────────────────

/// A single frame in an unwound call stack.
#[derive(Debug, Clone)]
pub struct StackFrame {
    pub index: usize,
    pub ip: u64,
    pub bp: u64,
    pub function: Option<String>,
    pub location: Option<SourceLocation>,
}

// ── Variable inspector ──────────────────────────────────────────────

/// A local variable read from a stack frame.
#[derive(Debug, Clone)]
pub struct LocalVariable {
    pub name: String,
    pub rbp_offset: i64,
    pub value: u64,
}

/// Read a local variable given a frame pointer and RBP offset.
pub fn read_local(bp: u64, name: &str, rbp_offset: i64) -> LocalVariable {
    let addr = (bp as i64).wrapping_add(rbp_offset) as u64;
    let value = unsafe { *(addr as *const u64) };
    LocalVariable { name: String::from(name), rbp_offset, value }
}

// ── Debug state ─────────────────────────────────────────────────────

/// Combined debug information state.
pub struct DebugState {
    pub line_table: LineNumberTable,
    pub functions: FunctionTable,
    pub breakpoints: BreakpointManager,
    pub watchpoints: WatchpointManager,
}

impl DebugState {
    pub fn new() -> Self {
        Self {
            line_table: LineNumberTable::new(),
            functions: FunctionTable::new(),
            breakpoints: BreakpointManager::new(),
            watchpoints: WatchpointManager::new(),
        }
    }

    /// Walk the call stack using frame pointers, annotating each frame
    /// with source location and function name when available.
    pub fn unwind_stack(&self, initial_ip: u64, initial_bp: u64) -> Vec<StackFrame> {
        let mut frames = Vec::new();
        let mut ip = initial_ip;
        let mut bp = initial_bp;
        for i in 0..MAX_UNWIND_DEPTH {
            if bp == 0 { break; }
            let function = self.functions.lookup(ip).map(|f| f.name.clone());
            let location = self.line_table.lookup(ip);
            frames.push(StackFrame { index: i, ip, bp, function, location });
            let saved_bp = unsafe { *(bp as *const u64) };
            let ret_addr = unsafe { *((bp.wrapping_add(8)) as *const u64) };
            if saved_bp == 0 || ret_addr == 0 || saved_bp <= bp { break; }
            bp = saved_bp;
            ip = ret_addr;
        }
        STATS.backtraces.fetch_add(1, Ordering::Relaxed);
        frames
    }
}

// ── Stats & global state ────────────────────────────────────────────

pub struct DebugStats {
    pub breakpoints_set: AtomicU64,
    pub breakpoints_hit: AtomicU64,
    pub backtraces: AtomicU64,
    pub symbol_lookups: AtomicU64,
}

impl DebugStats {
    const fn new() -> Self {
        Self {
            breakpoints_set: AtomicU64::new(0),
            breakpoints_hit: AtomicU64::new(0),
            backtraces: AtomicU64::new(0),
            symbol_lookups: AtomicU64::new(0),
        }
    }
}

pub static STATS: DebugStats = DebugStats::new();
pub static DEBUG: Mutex<Option<DebugState>> = Mutex::new(None);

// ── Public free-standing API ────────────────────────────────────────

/// Produce an annotated backtrace from the given instruction pointer and
/// frame pointer (in practice these come from the trap frame).
pub fn backtrace_annotated() -> String {
    let guard = DEBUG.lock();
    let state = match guard.as_ref() {
        Some(s) => s,
        None => return String::from("(debuginfo not initialised)\n"),
    };
    // In a real kernel, ip/bp come from the interrupt/trap frame.
    let (ip, bp): (u64, u64) = (0, 0);
    let frames = state.unwind_stack(ip, bp);
    if frames.is_empty() { return String::from("(empty backtrace)\n"); }
    let mut out = String::from("BACKTRACE:\n");
    for f in &frames {
        let func = f.function.as_deref().unwrap_or("<unknown>");
        let loc = match &f.location {
            Some(l) => format!("{}:{}:{}", l.file, l.line, l.column),
            None => String::from("??:0:0"),
        };
        out.push_str(&format!("  #{:<3} {:#016x} in {} at {}\n", f.index, f.ip, func, loc));
    }
    out
}

pub fn list_breakpoints() -> String {
    DEBUG.lock().as_ref().map_or(String::from("(debuginfo not initialised)\n"), |s| s.breakpoints.list())
}

pub fn set_breakpoint(addr: u64) -> Option<u32> {
    DEBUG.lock().as_mut().and_then(|s| s.breakpoints.set(addr))
}

pub fn clear_breakpoint(addr: u64) -> bool {
    DEBUG.lock().as_mut().map_or(false, |s| s.breakpoints.clear(addr))
}

/// Return a formatted summary of the debuginfo subsystem.
pub fn debug_info() -> String {
    let guard = DEBUG.lock();
    let state = match guard.as_ref() {
        Some(s) => s,
        None => return String::from("(debuginfo not initialised)\n"),
    };
    format!(
        "Debug info: {} line entries, {} functions, {} breakpoints, {} watchpoints\n\
         Stats: bp_set={}, bp_hit={}, backtraces={}, lookups={}\n",
        state.line_table.len(), state.functions.len(),
        state.breakpoints.count(), state.watchpoints.watchpoints.len(),
        STATS.breakpoints_set.load(Ordering::Relaxed),
        STATS.breakpoints_hit.load(Ordering::Relaxed),
        STATS.backtraces.load(Ordering::Relaxed),
        STATS.symbol_lookups.load(Ordering::Relaxed),
    )
}

/// Initialise the debug information subsystem.
pub fn init() {
    *DEBUG.lock() = Some(DebugState::new());
}
