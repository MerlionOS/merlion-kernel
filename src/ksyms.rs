/// Kernel symbol table for stack traces and panic diagnostics.
/// Stores function name + address mappings registered at boot.
/// Used by the panic handler to show human-readable backtraces.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::borrow::ToOwned;
use spin::Mutex;

static SYMBOLS: Mutex<Vec<Symbol>> = Mutex::new(Vec::new());

struct Symbol {
    name: String,
    addr: u64,
}

/// Register a kernel symbol (called during init).
pub fn register(name: &str, addr: u64) {
    SYMBOLS.lock().push(Symbol {
        name: name.to_owned(),
        addr,
    });
}

/// Look up a symbol by address. Returns the closest symbol at or before `addr`.
pub fn lookup(addr: u64) -> Option<(String, u64)> {
    let syms = SYMBOLS.lock();
    let mut best: Option<&Symbol> = None;
    for sym in syms.iter() {
        if sym.addr <= addr {
            if best.is_none() || sym.addr > best.unwrap().addr {
                best = Some(sym);
            }
        }
    }
    best.map(|s| (s.name.clone(), addr - s.addr))
}

/// Walk the stack frames using frame pointers (rbp chain).
/// Returns a list of return addresses.
pub fn backtrace() -> Vec<u64> {
    let mut frames = Vec::new();
    let mut rbp: u64;

    unsafe {
        core::arch::asm!("mov {}, rbp", out(reg) rbp);
    }

    // Walk the frame pointer chain (max 16 frames)
    for _ in 0..16 {
        if rbp == 0 || rbp % 8 != 0 {
            break;
        }
        // Return address is at rbp + 8
        let ret_addr = unsafe { *((rbp + 8) as *const u64) };
        if ret_addr == 0 {
            break;
        }
        frames.push(ret_addr);
        // Next frame pointer is at *rbp
        rbp = unsafe { *(rbp as *const u64) };
    }

    frames
}

/// Format a backtrace with symbol resolution.
pub fn format_backtrace() -> String {
    let frames = backtrace();
    let mut out = String::from("Stack trace:\n");
    for (i, addr) in frames.iter().enumerate() {
        if let Some((name, offset)) = lookup(*addr) {
            out.push_str(&alloc::format!("  #{}: {:#x} <{}+{:#x}>\n", i, addr, name, offset));
        } else {
            out.push_str(&alloc::format!("  #{}: {:#x} <unknown>\n", i, addr));
        }
    }
    out
}

/// Register core kernel symbols. Called at boot.
pub fn init() {
    // Register key function addresses as symbols.
    // In a real OS these would be extracted from the ELF symbol table.
    register("kernel_main", kernel_main_addr());
    register("halt_loop", halt_loop_addr());
}

// Helper functions to get addresses of known functions
fn kernel_main_addr() -> u64 {
    // We can't easily get the address of main from lib, so use a sentinel
    register as *const () as u64 // approximate
}

fn halt_loop_addr() -> u64 {
    x86_64::instructions::hlt as *const () as u64
}
