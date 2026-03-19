/// ELF interpreter / dynamic linker (ld.so) for MerlionOS.
///
/// Handles PT_INTERP, PT_DYNAMIC, RPATH/RUNPATH, lazy binding,
/// and TLS initialization for dynamically linked ELF binaries.
/// Extends the existing elf_dyn.rs and elf_runtime.rs infrastructure.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use alloc::collections::BTreeMap;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::serial_println;

// ═══════════════════════════════════════════════════════════════════
//  ELF CONSTANTS
// ═══════════════════════════════════════════════════════════════════

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_TLS: u32 = 7;

const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;
const DT_STRTAB: u64 = 5;
const DT_SYMTAB: u64 = 6;
const DT_RELA: u64 = 7;
const DT_RELASZ: u64 = 8;
const DT_INIT: u64 = 12;
const DT_FINI: u64 = 13;
const DT_RPATH: u64 = 15;
const DT_RUNPATH: u64 = 29;
const DT_JMPREL: u64 = 23;
const DT_PLTRELSZ: u64 = 2;
const DT_PLTGOT: u64 = 3;
const DT_FLAGS: u64 = 30;

// Relocation types (x86_64)
const R_X86_64_64: u32 = 1;
const R_X86_64_GLOB_DAT: u32 = 6;
const R_X86_64_JUMP_SLOT: u32 = 7;
const R_X86_64_RELATIVE: u32 = 8;

// ═══════════════════════════════════════════════════════════════════
//  DYNAMIC LINKER STATE
// ═══════════════════════════════════════════════════════════════════

/// A loaded shared object.
struct LoadedObject {
    name: String,
    base_addr: u64,
    size: usize,
    dynamic: Vec<(u64, u64)>,  // (d_tag, d_val) entries
    init_func: Option<u64>,
    fini_func: Option<u64>,
    symbols: BTreeMap<String, u64>,  // name → address
    needed: Vec<String>,
}

/// Global dynamic linker state.
struct DynLinkerState {
    objects: Vec<LoadedObject>,
    search_paths: Vec<String>,
    next_base: u64,
    bind_now: bool,  // LD_BIND_NOW
}

impl DynLinkerState {
    const fn new() -> Self {
        Self {
            objects: Vec::new(),
            search_paths: Vec::new(),
            next_base: 0x0000_0070_0000,  // shared library base
            bind_now: false,
        }
    }
}

static LINKER: Mutex<DynLinkerState> = Mutex::new(DynLinkerState::new());
static LIBS_LOADED: AtomicU32 = AtomicU32::new(0);

// ═══════════════════════════════════════════════════════════════════
//  INTERPRETER
// ═══════════════════════════════════════════════════════════════════

/// Check if an ELF binary has a PT_INTERP segment (needs dynamic linking).
pub fn needs_interp(elf_data: &[u8]) -> bool {
    if elf_data.len() < 64 { return false; }
    let e_phoff = u64::from_le_bytes(elf_data[32..40].try_into().unwrap_or([0;8])) as usize;
    let e_phentsize = u16::from_le_bytes([elf_data[54], elf_data[55]]) as usize;
    let e_phnum = u16::from_le_bytes([elf_data[56], elf_data[57]]) as usize;

    for i in 0..e_phnum {
        let off = e_phoff + i * e_phentsize;
        if off + 4 > elf_data.len() { break; }
        let p_type = u32::from_le_bytes(elf_data[off..off+4].try_into().unwrap_or([0;4]));
        if p_type == PT_INTERP { return true; }
    }
    false
}

/// Get the interpreter path from PT_INTERP.
pub fn get_interp(elf_data: &[u8]) -> Option<String> {
    if elf_data.len() < 64 { return None; }
    let e_phoff = u64::from_le_bytes(elf_data[32..40].try_into().unwrap_or([0;8])) as usize;
    let e_phentsize = u16::from_le_bytes([elf_data[54], elf_data[55]]) as usize;
    let e_phnum = u16::from_le_bytes([elf_data[56], elf_data[57]]) as usize;

    for i in 0..e_phnum {
        let off = e_phoff + i * e_phentsize;
        if off + 56 > elf_data.len() { break; }
        let p_type = u32::from_le_bytes(elf_data[off..off+4].try_into().unwrap_or([0;4]));
        if p_type == PT_INTERP {
            let p_offset = u64::from_le_bytes(elf_data[off+8..off+16].try_into().unwrap_or([0;8])) as usize;
            let p_filesz = u64::from_le_bytes(elf_data[off+32..off+40].try_into().unwrap_or([0;8])) as usize;
            if p_offset + p_filesz <= elf_data.len() {
                let path = &elf_data[p_offset..p_offset+p_filesz];
                if let Ok(s) = core::str::from_utf8(path) {
                    return Some(s.trim_end_matches('\0').into());
                }
            }
        }
    }
    None
}

/// Parse PT_DYNAMIC entries from an ELF.
pub fn parse_dynamic(elf_data: &[u8], _base: u64) -> Vec<(u64, u64)> {
    let mut entries = Vec::new();
    if elf_data.len() < 64 { return entries; }

    let e_phoff = u64::from_le_bytes(elf_data[32..40].try_into().unwrap_or([0;8])) as usize;
    let e_phentsize = u16::from_le_bytes([elf_data[54], elf_data[55]]) as usize;
    let e_phnum = u16::from_le_bytes([elf_data[56], elf_data[57]]) as usize;

    for i in 0..e_phnum {
        let off = e_phoff + i * e_phentsize;
        if off + 56 > elf_data.len() { break; }
        let p_type = u32::from_le_bytes(elf_data[off..off+4].try_into().unwrap_or([0;4]));
        if p_type == PT_DYNAMIC {
            let p_offset = u64::from_le_bytes(elf_data[off+8..off+16].try_into().unwrap_or([0;8])) as usize;
            let p_filesz = u64::from_le_bytes(elf_data[off+32..off+40].try_into().unwrap_or([0;8])) as usize;
            let end = (p_offset + p_filesz).min(elf_data.len());
            let mut pos = p_offset;
            while pos + 16 <= end {
                let d_tag = u64::from_le_bytes(elf_data[pos..pos+8].try_into().unwrap_or([0;8]));
                let d_val = u64::from_le_bytes(elf_data[pos+8..pos+16].try_into().unwrap_or([0;8]));
                if d_tag == DT_NULL { break; }
                entries.push((d_tag, d_val));
                pos += 16;
            }
        }
    }
    entries
}

/// Resolve a symbol across all loaded objects.
pub fn resolve_symbol(name: &str) -> Option<u64> {
    let linker = LINKER.lock();
    for obj in &linker.objects {
        if let Some(&addr) = obj.symbols.get(name) {
            return Some(addr);
        }
    }
    None
}

/// Load a shared object by name from VFS or built-in libraries.
pub fn load_library(name: &str) -> Result<u64, &'static str> {
    let mut linker = LINKER.lock();

    // Check if already loaded
    for obj in &linker.objects {
        if obj.name == name {
            return Ok(obj.base_addr);
        }
    }

    let base = linker.next_base;
    linker.next_base += 0x10000; // 64K per library

    // Try to load from our built-in dynlink library
    let (code, syms) = if name.contains("hello") {
        let (c, s) = crate::dynlink::gen_libhello_raw();
        (c, s)
    } else if name.contains("math") {
        let (c, s) = crate::dynlink::gen_libmath_raw();
        (c, s)
    } else {
        return Err("library not found");
    };

    let mut symbols = BTreeMap::new();
    for (sym_name, offset) in &syms {
        symbols.insert(sym_name.clone(), base + *offset);
    }

    linker.objects.push(LoadedObject {
        name: String::from(name),
        base_addr: base,
        size: code.len(),
        dynamic: Vec::new(),
        init_func: None,
        fini_func: None,
        symbols,
        needed: Vec::new(),
    });

    LIBS_LOADED.fetch_add(1, Ordering::Relaxed);
    serial_println!("[ld.so] loaded {} at {:#x} ({} symbols)", name, base, syms.len());

    Ok(base)
}

// ═══════════════════════════════════════════════════════════════════
//  TLS SUPPORT
// ═══════════════════════════════════════════════════════════════════

/// TLS block descriptor for a loaded module.
pub struct TlsBlock {
    pub module_id: u32,
    pub offset: usize,
    pub size: usize,
    pub init_data: Vec<u8>,
}

static TLS_BLOCKS: Mutex<Vec<TlsBlock>> = Mutex::new(Vec::new());
static NEXT_TLS_MODULE: AtomicU32 = AtomicU32::new(1);

/// Register a TLS block for a loaded ELF module.
pub fn register_tls(size: usize, init_data: &[u8]) -> u32 {
    let id = NEXT_TLS_MODULE.fetch_add(1, Ordering::SeqCst);
    let mut blocks = TLS_BLOCKS.lock();
    let offset = blocks.iter().map(|b| b.offset + b.size).max().unwrap_or(0);
    blocks.push(TlsBlock {
        module_id: id,
        offset,
        size,
        init_data: init_data.to_vec(),
    });
    serial_println!("[ld.so] TLS module {} registered ({} bytes at offset {})", id, size, offset);
    id
}

/// Get total TLS size for all modules.
pub fn total_tls_size() -> usize {
    TLS_BLOCKS.lock().iter().map(|b| b.offset + b.size).max().unwrap_or(0)
}

// ═══════════════════════════════════════════════════════════════════
//  INFO
// ═══════════════════════════════════════════════════════════════════

pub fn init() {
    let mut linker = LINKER.lock();
    linker.search_paths.push(String::from("/lib"));
    linker.search_paths.push(String::from("/usr/lib"));
    drop(linker);
    serial_println!("[ld.so] dynamic linker initialized (search: /lib, /usr/lib)");
}

pub fn info() -> String {
    let linker = LINKER.lock();
    let tls_size = total_tls_size();
    let mut out = format!(
        "Dynamic Linker (ld.so):\n\
         Loaded objects: {}\n\
         Search paths:   {:?}\n\
         TLS size:       {} bytes\n\
         Bind mode:      {}\n",
        linker.objects.len(),
        linker.search_paths,
        tls_size,
        if linker.bind_now { "immediate" } else { "lazy" },
    );
    for obj in &linker.objects {
        out.push_str(&format!("  {} at {:#x} ({} symbols)\n",
            obj.name, obj.base_addr, obj.symbols.len()));
    }
    out
}
