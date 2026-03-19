/// Userspace dynamic linker for MerlionOS (U6).
///
/// Provides shared library loading, symbol resolution, and GOT/PLT
/// relocation for user programs. Libraries are loaded at dynamic base
/// addresses and symbols resolved at load time.
///
/// Uses the existing elf_runtime.rs infrastructure for the linker core,
/// and provides kernel-side syscall handlers (SYS_DLOPEN, SYS_DLSYM,
/// SYS_DLCLOSE) plus built-in shared libraries as machine code.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use crate::serial_println;

// ═══════════════════════════════════════════════════════════════════
//  CONSTANTS
// ═══════════════════════════════════════════════════════════════════

/// Base address for dynamically loaded libraries.
const DYNLIB_BASE: u64 = 0x0000_0070_0000;

/// Size of each library slot (one page).
const DYNLIB_SLOT_SIZE: u64 = 4096;

/// Maximum number of loaded libraries.
const MAX_LIBRARIES: usize = 8;

// ═══════════════════════════════════════════════════════════════════
//  TYPES
// ═══════════════════════════════════════════════════════════════════

/// A loaded shared library.
struct SharedLib {
    handle: u32,
    name: String,
    base_addr: u64,
    code: Vec<u8>,
    symbols: Vec<LibSymbol>,
    ref_count: u32,
}

/// A symbol exported by a shared library.
#[derive(Clone)]
struct LibSymbol {
    name: String,
    offset: u64, // offset from library base
}

/// Global dynamic linker state.
struct DynLinker {
    libraries: Vec<SharedLib>,
    next_handle: u32,
    next_base: u64,
}

impl DynLinker {
    const fn new() -> Self {
        Self {
            libraries: Vec::new(),
            next_handle: 1,
            next_base: DYNLIB_BASE,
        }
    }
}

static DYNLINKER: Mutex<DynLinker> = Mutex::new(DynLinker::new());

// ═══════════════════════════════════════════════════════════════════
//  BUILT-IN SHARED LIBRARIES
// ═══════════════════════════════════════════════════════════════════

/// Generate libhello.so — a demo shared library with three functions.
fn gen_libhello() -> (Vec<u8>, Vec<LibSymbol>) {
    let mut code = vec![0xCC_u8; 4096]; // fill with int3

    // ── greet() at offset 0x000 ──
    // Prints "Hello from libhello.so!\n" via SYS_WRITE
    {
        let b = 0x000;
        // mov rax, 0 (SYS_WRITE)
        code[b] = 0x48; code[b+1] = 0xC7; code[b+2] = 0xC0;
        code[b+3] = 0x00; code[b+4] = 0x00; code[b+5] = 0x00; code[b+6] = 0x00;
        // lea rdi, [rip + offset_to_msg]
        // msg is at offset 0x100 in this library
        // RIP at this point = base + b + 14 (after lea)
        // msg at base + 0x100
        // disp = 0x100 - (b + 14) = 0x100 - 14 = 242 = 0xF2
        code[b+7] = 0x48; code[b+8] = 0x8D; code[b+9] = 0x3D;
        code[b+10] = 0xF2; code[b+11] = 0x00; code[b+12] = 0x00; code[b+13] = 0x00;
        // mov rsi, 24 (msg len)
        code[b+14] = 0x48; code[b+15] = 0xC7; code[b+16] = 0xC6;
        code[b+17] = 0x18; code[b+18] = 0x00; code[b+19] = 0x00; code[b+20] = 0x00;
        // int 0x80
        code[b+21] = 0xCD; code[b+22] = 0x80;
        // ret
        code[b+23] = 0xC3;
    }

    // ── add(rdi, rsi) -> rax at offset 0x020 ──
    {
        let b = 0x020;
        // mov rax, rdi
        code[b] = 0x48; code[b+1] = 0x89; code[b+2] = 0xF8;
        // add rax, rsi
        code[b+3] = 0x48; code[b+4] = 0x01; code[b+5] = 0xF0;
        // ret
        code[b+6] = 0xC3;
    }

    // ── multiply(rdi, rsi) -> rax at offset 0x030 ──
    {
        let b = 0x030;
        // mov rax, rdi
        code[b] = 0x48; code[b+1] = 0x89; code[b+2] = 0xF8;
        // imul rax, rsi
        code[b+3] = 0x48; code[b+4] = 0x0F; code[b+5] = 0xAF; code[b+6] = 0xC6;
        // ret
        code[b+7] = 0xC3;
    }

    // ── version() at offset 0x040 ──
    // Prints "libhello v1.0\n"
    {
        let b = 0x040;
        code[b] = 0x48; code[b+1] = 0xC7; code[b+2] = 0xC0;
        code[b+3] = 0x00; code[b+4] = 0x00; code[b+5] = 0x00; code[b+6] = 0x00;
        // lea rdi, [rip + offset_to_ver_msg]
        // ver_msg at 0x120, RIP = b+14 = 0x04E
        // disp = 0x120 - 0x04E = 0xD2
        code[b+7] = 0x48; code[b+8] = 0x8D; code[b+9] = 0x3D;
        code[b+10] = 0xD2; code[b+11] = 0x00; code[b+12] = 0x00; code[b+13] = 0x00;
        code[b+14] = 0x48; code[b+15] = 0xC7; code[b+16] = 0xC6;
        code[b+17] = 0x0E; code[b+18] = 0x00; code[b+19] = 0x00; code[b+20] = 0x00;
        code[b+21] = 0xCD; code[b+22] = 0x80;
        code[b+23] = 0xC3;
    }

    // ── String data ──
    // 0x100: "Hello from libhello.so!\n" (24 bytes)
    let msg = b"Hello from libhello.so!\n";
    code[0x100..0x100+msg.len()].copy_from_slice(msg);

    // 0x120: "libhello v1.0\n" (14 bytes)
    let ver = b"libhello v1.0\n";
    code[0x120..0x120+ver.len()].copy_from_slice(ver);

    let symbols = vec![
        LibSymbol { name: String::from("greet"), offset: 0x000 },
        LibSymbol { name: String::from("add"), offset: 0x020 },
        LibSymbol { name: String::from("multiply"), offset: 0x030 },
        LibSymbol { name: String::from("version"), offset: 0x040 },
    ];

    (code, symbols)
}

/// Generate libmath.so — arithmetic operations.
fn gen_libmath() -> (Vec<u8>, Vec<LibSymbol>) {
    let mut code = vec![0xCC_u8; 4096];

    // ── square(rdi) -> rax at offset 0x000 ──
    {
        let b = 0x000;
        code[b] = 0x48; code[b+1] = 0x89; code[b+2] = 0xF8; // mov rax, rdi
        code[b+3] = 0x48; code[b+4] = 0x0F; code[b+5] = 0xAF; code[b+6] = 0xC7; // imul rax, rdi
        code[b+7] = 0xC3; // ret
    }

    // ── cube(rdi) -> rax at offset 0x010 ──
    {
        let b = 0x010;
        code[b] = 0x48; code[b+1] = 0x89; code[b+2] = 0xF8; // mov rax, rdi
        code[b+3] = 0x48; code[b+4] = 0x0F; code[b+5] = 0xAF; code[b+6] = 0xC7; // imul rax, rdi
        code[b+7] = 0x48; code[b+8] = 0x0F; code[b+9] = 0xAF; code[b+10] = 0xC7; // imul rax, rdi
        code[b+11] = 0xC3; // ret
    }

    // ── abs(rdi) -> rax at offset 0x020 ──
    {
        let b = 0x020;
        code[b] = 0x48; code[b+1] = 0x89; code[b+2] = 0xF8; // mov rax, rdi
        // test rax, rax
        code[b+3] = 0x48; code[b+4] = 0x85; code[b+5] = 0xC0;
        // jns .done (+3)
        code[b+6] = 0x79; code[b+7] = 0x03;
        // neg rax
        code[b+8] = 0x48; code[b+9] = 0xF7; code[b+10] = 0xD8;
        // .done: ret
        code[b+11] = 0xC3;
    }

    // ── max(rdi, rsi) -> rax at offset 0x030 ──
    {
        let b = 0x030;
        code[b] = 0x48; code[b+1] = 0x89; code[b+2] = 0xF8; // mov rax, rdi
        code[b+3] = 0x48; code[b+4] = 0x39; code[b+5] = 0xF7; // cmp rdi, rsi
        // jge .done (+3)
        code[b+6] = 0x7D; code[b+7] = 0x03;
        // mov rax, rsi
        code[b+8] = 0x48; code[b+9] = 0x89; code[b+10] = 0xF0;
        // .done: ret
        code[b+11] = 0xC3;
    }

    let symbols = vec![
        LibSymbol { name: String::from("square"), offset: 0x000 },
        LibSymbol { name: String::from("cube"), offset: 0x010 },
        LibSymbol { name: String::from("abs"), offset: 0x020 },
        LibSymbol { name: String::from("max"), offset: 0x030 },
    ];

    (code, symbols)
}

/// Get a built-in shared library by name.
fn get_builtin_library(name: &str) -> Option<(Vec<u8>, Vec<LibSymbol>)> {
    match name {
        "libhello" | "libhello.so" => Some(gen_libhello()),
        "libmath" | "libmath.so" => Some(gen_libmath()),
        _ => None,
    }
}

// ═══════════════════════════════════════════════════════════════════
//  PUBLIC API
// ═══════════════════════════════════════════════════════════════════

/// Open a shared library. Returns a handle, or 0 on failure.
#[cfg(target_arch = "x86_64")]
pub fn dlopen(name: &str) -> u64 {
    use x86_64::structures::paging::{Page, PageTableFlags};
    use x86_64::VirtAddr;

    let mut linker = DYNLINKER.lock();

    // Check if already loaded
    for lib in &linker.libraries {
        if lib.name == name {
            return lib.handle as u64;
        }
    }

    // Load built-in library
    let (code, symbols) = match get_builtin_library(name) {
        Some(lib) => lib,
        None => {
            serial_println!("[dynlink] library '{}' not found", name);
            return 0;
        }
    };

    if linker.libraries.len() >= MAX_LIBRARIES {
        serial_println!("[dynlink] too many libraries loaded");
        return 0;
    }

    let base_addr = linker.next_base;
    let handle = linker.next_handle;

    // Map library code into user address space
    let user_rx = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    let page = Page::containing_address(VirtAddr::new(base_addr));
    match crate::memory::map_page_global(page, user_rx) {
        Some(frame) => {
            let dest = crate::memory::phys_to_virt(frame.start_address());
            unsafe {
                core::ptr::write_bytes(dest.as_mut_ptr::<u8>(), 0, 4096);
                core::ptr::copy_nonoverlapping(
                    code.as_ptr(),
                    dest.as_mut_ptr::<u8>(),
                    code.len().min(4096),
                );
            }
        }
        None => {
            serial_println!("[dynlink] failed to map library page at {:#x}", base_addr);
            return 0;
        }
    }

    serial_println!("[dynlink] loaded '{}' at {:#x} ({} symbols, handle={})",
        name, base_addr, symbols.len(), handle);

    linker.libraries.push(SharedLib {
        handle,
        name: String::from(name),
        base_addr,
        code,
        symbols,
        ref_count: 1,
    });
    linker.next_handle += 1;
    linker.next_base += DYNLIB_SLOT_SIZE;

    handle as u64
}

#[cfg(not(target_arch = "x86_64"))]
pub fn dlopen(_name: &str) -> u64 { 0 }

/// Look up a symbol in a loaded library. Returns the absolute address, or 0.
pub fn dlsym(handle: u64, symbol_name: &str) -> u64 {
    let linker = DYNLINKER.lock();
    for lib in &linker.libraries {
        if lib.handle == handle as u32 {
            for sym in &lib.symbols {
                if sym.name == symbol_name {
                    let addr = lib.base_addr + sym.offset;
                    serial_println!("[dynlink] dlsym({}, '{}') = {:#x}", handle, symbol_name, addr);
                    return addr;
                }
            }
            serial_println!("[dynlink] symbol '{}' not found in handle {}", symbol_name, handle);
            return 0;
        }
    }
    serial_println!("[dynlink] invalid handle {}", handle);
    0
}

/// Close a shared library by handle.
pub fn dlclose(handle: u64) -> i64 {
    let mut linker = DYNLINKER.lock();
    if let Some(pos) = linker.libraries.iter().position(|l| l.handle == handle as u32) {
        let lib = &mut linker.libraries[pos];
        lib.ref_count = lib.ref_count.saturating_sub(1);
        if lib.ref_count == 0 {
            let name = lib.name.clone();
            linker.libraries.remove(pos);
            serial_println!("[dynlink] unloaded '{}'", name);
        } else {
            serial_println!("[dynlink] dlclose handle {} (ref_count={})", handle, lib.ref_count);
        }
        0
    } else {
        serial_println!("[dynlink] dlclose: invalid handle {}", handle);
        -1
    }
}

/// List loaded shared libraries.
pub fn list_libraries() -> String {
    let linker = DYNLINKER.lock();
    if linker.libraries.is_empty() {
        return String::from("No shared libraries loaded.\n");
    }
    let mut out = String::from("Loaded shared libraries:\n");
    for lib in &linker.libraries {
        out.push_str(&format!("  [{}] {} at {:#x} ({} symbols, refs={})\n",
            lib.handle, lib.name, lib.base_addr, lib.symbols.len(), lib.ref_count));
        for sym in &lib.symbols {
            out.push_str(&format!("       {} @ {:#x}\n", sym.name, lib.base_addr + sym.offset));
        }
    }
    out
}

/// Available built-in shared libraries.
pub fn list_available() -> &'static [&'static str] {
    &["libhello.so", "libmath.so"]
}

// ═══════════════════════════════════════════════════════════════════
//  DEMO PROGRAMS
// ═══════════════════════════════════════════════════════════════════

/// Generate "dynlink-test" program: loads libhello.so, calls functions, unloads.
pub fn gen_dynlink_test() -> Vec<u8> {
    use crate::ulibc::*;

    let text_base: u64 = 0x0000_0040_0000;
    let mut c: Vec<u8> = Vec::new();

    // --- puts("dynlink-test: loading libhello.so...") ---
    let msg1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // --- dlopen("libhello") via SYS_DLOPEN (170) ---
    let libname_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0); // name ptr
    let libname_len_fixup = c.len() + 2;
    emit_mov_rsi_imm64(&mut c, 0); // name len
    // mov rax, 170 (SYS_DLOPEN)
    c.extend_from_slice(&[0x48, 0xC7, 0xC0, 0xAA, 0x00, 0x00, 0x00]);
    c.extend_from_slice(&[0xCD, 0x80]); // int 0x80
    // rax = handle, save in r12
    c.extend_from_slice(&[0x49, 0x89, 0xC4]); // mov r12, rax

    // --- puts("dynlink-test: calling greet()...") ---
    let msg2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // --- dlsym(handle, "greet") via SYS_DLSYM (171) ---
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12 (handle)
    let sym_greet_fixup = c.len() + 2;
    emit_mov_rsi_imm64(&mut c, 0); // sym name ptr
    let sym_greet_len_fixup = c.len() + 2;
    emit_mov_rdx_imm64(&mut c, 0); // sym name len
    c.extend_from_slice(&[0x48, 0xC7, 0xC0, 0xAB, 0x00, 0x00, 0x00]); // mov rax, 171
    c.extend_from_slice(&[0xCD, 0x80]);
    // rax = function address, call it
    c.extend_from_slice(&[0xFF, 0xD0]); // call rax

    // --- dlsym(handle, "add") and call add(10, 32) ---
    let msg3_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    let sym_add_fixup = c.len() + 2;
    emit_mov_rsi_imm64(&mut c, 0);
    let sym_add_len_fixup = c.len() + 2;
    emit_mov_rdx_imm64(&mut c, 0);
    c.extend_from_slice(&[0x48, 0xC7, 0xC0, 0xAB, 0x00, 0x00, 0x00]); // mov rax, 171
    c.extend_from_slice(&[0xCD, 0x80]);
    // Save func ptr in r13
    c.extend_from_slice(&[0x49, 0x89, 0xC5]); // mov r13, rax
    // Call add(10, 32)
    emit_mov_rdi_imm64(&mut c, 10);
    emit_mov_rsi_imm64(&mut c, 32);
    c.extend_from_slice(&[0x41, 0xFF, 0xD5]); // call r13
    // Print "add(10,32) = " then result
    emit_push_rax(&mut c);
    let msg4_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    c.push(0x58); // pop rax
    emit_mov_rdi_rax(&mut c);
    emit_call_libc(&mut c, FN_PRINT_INT);
    let nl_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // --- dlclose(handle) via SYS_DLCLOSE (172) ---
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    c.extend_from_slice(&[0x48, 0xC7, 0xC0, 0xAC, 0x00, 0x00, 0x00]); // mov rax, 172
    c.extend_from_slice(&[0xCD, 0x80]);

    // --- puts("dynlink-test: done!") ---
    let msg5_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // exit(0)
    c.extend_from_slice(&[0x48, 0x31, 0xFF]);
    emit_call_libc(&mut c, FN_EXIT);
    c.extend_from_slice(&[0xEB, 0xFE]);

    // --- String data ---
    let msg1_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"dynlink-test: loading libhello.so...\n\0");

    let libname_addr = text_base + c.len() as u64;
    let libname = b"libhello";
    let libname_len = libname.len() as u64;
    c.extend_from_slice(libname);
    c.push(0);

    let msg2_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"dynlink-test: calling greet()...\n\0");

    let sym_greet_addr = text_base + c.len() as u64;
    let sym_greet = b"greet";
    let sym_greet_len = sym_greet.len() as u64;
    c.extend_from_slice(sym_greet);
    c.push(0);

    let msg3_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"dynlink-test: calling add(10, 32)...\n\0");

    let sym_add_addr = text_base + c.len() as u64;
    let sym_add = b"add";
    let sym_add_len = sym_add.len() as u64;
    c.extend_from_slice(sym_add);
    c.push(0);

    let msg4_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"  add(10, 32) = \0");

    let nl_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"\n\0");

    let msg5_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"dynlink-test: library unloaded, done!\n\0");

    // Patch all addresses
    c[msg1_fixup..msg1_fixup+8].copy_from_slice(&msg1_addr.to_le_bytes());
    c[libname_fixup..libname_fixup+8].copy_from_slice(&libname_addr.to_le_bytes());
    c[libname_len_fixup..libname_len_fixup+8].copy_from_slice(&libname_len.to_le_bytes());
    c[msg2_fixup..msg2_fixup+8].copy_from_slice(&msg2_addr.to_le_bytes());
    c[sym_greet_fixup..sym_greet_fixup+8].copy_from_slice(&sym_greet_addr.to_le_bytes());
    c[sym_greet_len_fixup..sym_greet_len_fixup+8].copy_from_slice(&sym_greet_len.to_le_bytes());
    c[msg3_fixup..msg3_fixup+8].copy_from_slice(&msg3_addr.to_le_bytes());
    c[sym_add_fixup..sym_add_fixup+8].copy_from_slice(&sym_add_addr.to_le_bytes());
    c[sym_add_len_fixup..sym_add_len_fixup+8].copy_from_slice(&sym_add_len.to_le_bytes());
    c[msg4_fixup..msg4_fixup+8].copy_from_slice(&msg4_addr.to_le_bytes());
    c[nl_fixup..nl_fixup+8].copy_from_slice(&nl_addr.to_le_bytes());
    c[msg5_fixup..msg5_fixup+8].copy_from_slice(&msg5_addr.to_le_bytes());

    c
}

// ═══════════════════════════════════════════════════════════════════
//  INITIALIZATION
// ═══════════════════════════════════════════════════════════════════

pub fn init() {
    serial_println!("[dynlink] dynamic linker initialized");
    serial_println!("[dynlink] DYNLIB_BASE={:#x} max={} libraries", DYNLIB_BASE, MAX_LIBRARIES);
    serial_println!("[dynlink] built-in: libhello.so, libmath.so");
}

pub fn info() -> String {
    let linker = DYNLINKER.lock();
    format!(
        "Dynamic Linker (U6)\n\
         Base addr:   {:#010x}\n\
         Slot size:   {} bytes\n\
         Max libs:    {}\n\
         Loaded:      {}\n\
         Available:   libhello.so (greet, add, multiply, version)\n\
         \x20            libmath.so (square, cube, abs, max)\n\
         Syscalls:    170 (dlopen), 171 (dlsym), 172 (dlclose)\n",
        DYNLIB_BASE, DYNLIB_SLOT_SIZE, MAX_LIBRARIES, linker.libraries.len(),
    )
}

/// Get raw libhello code + symbol offsets for elf_interp.
pub fn gen_libhello_raw() -> (Vec<u8>, Vec<(String, u64)>) {
    let (code, syms) = gen_libhello();
    let offsets = syms.iter().map(|s| (s.name.clone(), s.offset)).collect();
    (code, offsets)
}

/// Get raw libmath code + symbol offsets for elf_interp.
pub fn gen_libmath_raw() -> (Vec<u8>, Vec<(String, u64)>) {
    let (code, syms) = gen_libmath();
    let offsets = syms.iter().map(|s| (s.name.clone(), s.offset)).collect();
    (code, offsets)
}
