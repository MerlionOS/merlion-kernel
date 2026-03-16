/// ELF executable loader and runner for MerlionOS.
/// Loads real ELF binaries from disk/VFS, sets up user address space,
/// passes arguments, and manages execution lifecycle.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_LOADED: usize = 64;
const MAX_ARGV: usize = 64;
const MAX_ENVP: usize = 64;
const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_0000;
const USER_STACK_PAGES: u64 = 16; // 64 KiB stack
const PAGE_SIZE: u64 = 4096;
const DEFAULT_LOAD_BASE: u64 = 0x0000_0000_0040_0000;
const INTERP_LOAD_BASE: u64 = 0x0000_0000_0080_0000;

// ELF constants
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 0x3E;
const PT_NULL: u32 = 0;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_NOTE: u32 = 4;
const PT_PHDR: u32 = 6;
const PT_GNU_STACK: u32 = 0x6474_E551;
const PT_GNU_RELRO: u32 = 0x6474_E552;

// Segment permission flags
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

// Section header types
const SHT_NULL: u32 = 0;
const SHT_PROGBITS: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_RELA: u32 = 4;
const SHT_NOBITS: u32 = 8;
const SHT_DYNSYM: u32 = 11;

// Auxiliary vector types
const AT_NULL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_PAGESZ: u64 = 6;
const AT_BASE: u64 = 7;
const AT_ENTRY: u64 = 9;
const AT_UID: u64 = 11;
const AT_EUID: u64 = 12;
const AT_GID: u64 = 13;
const AT_EGID: u64 = 14;
const AT_RANDOM: u64 = 25;

// ---------------------------------------------------------------------------
// ELF structures (extended)
// ---------------------------------------------------------------------------

/// ELF-64 section header.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64SectionHeader {
    pub sh_name: u32,
    pub sh_type: u32,
    pub sh_flags: u64,
    pub sh_addr: u64,
    pub sh_offset: u64,
    pub sh_size: u64,
    pub sh_link: u32,
    pub sh_info: u32,
    pub sh_addralign: u64,
    pub sh_entsize: u64,
}

// ---------------------------------------------------------------------------
// Loaded ELF tracking
// ---------------------------------------------------------------------------

/// Information about a loaded ELF binary attached to a process.
#[derive(Clone)]
pub struct LoadedElf {
    pub pid: u32,
    pub path: String,
    pub entry_point: u64,
    pub base_address: u64,
    pub ph_addr: u64,
    pub ph_count: u16,
    pub ph_entry_size: u16,
    pub interp: Option<String>,
    pub load_segments: Vec<LoadSegment>,
    pub is_pie: bool,
}

#[derive(Clone)]
pub struct LoadSegment {
    pub vaddr: u64,
    pub memsz: u64,
    pub filesz: u64,
    pub flags: u32,
}

impl LoadSegment {
    pub fn perm_string(&self) -> String {
        let mut s = String::new();
        if self.flags & PF_R != 0 { s.push('R'); }
        if self.flags & PF_W != 0 { s.push('W'); }
        if self.flags & PF_X != 0 { s.push('X'); }
        s
    }
}

/// Simplified core dump information.
#[derive(Clone)]
pub struct CoreDump {
    pub pid: u32,
    pub reason: String,
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
    pub segments: Vec<LoadSegment>,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static LOADED_ELFS: Mutex<Vec<LoadedElf>> = Mutex::new(Vec::new());
static CORE_DUMPS: Mutex<Vec<CoreDump>> = Mutex::new(Vec::new());
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static EXEC_COUNT: AtomicU64 = AtomicU64::new(0);
static EXEC_FAILURES: AtomicU64 = AtomicU64::new(0);
static TOTAL_PAGES_MAPPED: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Built-in ELF programs (small valid ELF64 binaries in memory)
// ---------------------------------------------------------------------------

/// Tiny x86_64 machine code: write(1, msg, len); exit(0)
/// Message is embedded right after the code.
fn build_hello_elf() -> Vec<u8> {
    // The user code:
    //   mov rax, 0        ; SYS_WRITE
    //   lea rdi, [rip+21] ; pointer to message
    //   mov rsi, 14       ; length
    //   int 0x80
    //   mov rax, 1        ; SYS_EXIT
    //   xor rdi, rdi
    //   int 0x80
    //   jmp $
    //   "Hello, world!\n"
    #[rustfmt::skip]
    let code: &[u8] = &[
        0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0
        0x48, 0x8D, 0x3D, 0x15, 0x00, 0x00, 0x00, // lea rdi, [rip+21]
        0x48, 0xC7, 0xC6, 0x0E, 0x00, 0x00, 0x00, // mov rsi, 14
        0xCD, 0x80,                                 // int 0x80
        0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00, // mov rax, 1
        0x48, 0x31, 0xFF,                           // xor rdi, rdi
        0xCD, 0x80,                                 // int 0x80
        0xEB, 0xFE,                                 // jmp $
        b'H', b'e', b'l', b'l', b'o', b',', b' ',
        b'w', b'o', b'r', b'l', b'd', b'!', b'\n',
    ];
    crate::elf_loader::build_elf(code)
}

/// Tiny x86_64 echo: write argv[1] then exit.
/// Since we cannot really pass argv via int 0x80 in this simple model,
/// this just prints a fixed message.
fn build_echo_elf() -> Vec<u8> {
    #[rustfmt::skip]
    let code: &[u8] = &[
        0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0
        0x48, 0x8D, 0x3D, 0x15, 0x00, 0x00, 0x00, // lea rdi, [rip+21]
        0x48, 0xC7, 0xC6, 0x05, 0x00, 0x00, 0x00, // mov rsi, 5
        0xCD, 0x80,                                 // int 0x80
        0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00, // mov rax, 1
        0x48, 0x31, 0xFF,                           // xor rdi, rdi
        0xCD, 0x80,                                 // int 0x80
        0xEB, 0xFE,                                 // jmp $
        b'e', b'c', b'h', b'o', b'\n',
    ];
    crate::elf_loader::build_elf(code)
}

/// Tiny x86_64 cat: just prints "cat: no file" and exits.
fn build_cat_elf() -> Vec<u8> {
    #[rustfmt::skip]
    let code: &[u8] = &[
        0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0
        0x48, 0x8D, 0x3D, 0x15, 0x00, 0x00, 0x00, // lea rdi, [rip+21]
        0x48, 0xC7, 0xC6, 0x0D, 0x00, 0x00, 0x00, // mov rsi, 13
        0xCD, 0x80,                                 // int 0x80
        0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00, // mov rax, 1
        0x48, 0x31, 0xFF,                           // xor rdi, rdi
        0xCD, 0x80,                                 // int 0x80
        0xEB, 0xFE,                                 // jmp $
        b'c', b'a', b't', b':', b' ', b'n', b'o',
        b' ', b'f', b'i', b'l', b'e', b'\n',
    ];
    crate::elf_loader::build_elf(code)
}

/// Get a built-in ELF program by name.
pub fn builtin_elf(name: &str) -> Option<Vec<u8>> {
    match name {
        "hello" | "/bin/hello" => Some(build_hello_elf()),
        "echo" | "/bin/echo" => Some(build_echo_elf()),
        "cat" | "/bin/cat" => Some(build_cat_elf()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// ELF parser (extended)
// ---------------------------------------------------------------------------

/// Extended ELF parse result.
pub struct ParsedElf {
    pub entry_point: u64,
    pub elf_type: u16,
    pub machine: u16,
    pub ph_offset: u64,
    pub ph_count: u16,
    pub ph_entry_size: u16,
    pub sh_offset: u64,
    pub sh_count: u16,
    pub sh_entry_size: u16,
    pub sh_strndx: u16,
    pub interp: Option<String>,
    pub load_segments: Vec<LoadSegment>,
    pub has_dynamic: bool,
    pub is_pie: bool,
}

/// Parse an ELF-64 binary with extended information.
pub fn parse_elf(data: &[u8]) -> Result<ParsedElf, &'static str> {
    if data.len() < 64 {
        return Err("too small for ELF header");
    }
    if data[0..4] != ELF_MAGIC {
        return Err("invalid ELF magic");
    }
    if data[4] != 2 {
        return Err("not ELF64");
    }
    if data[5] != 1 {
        return Err("not little-endian");
    }

    let hdr = unsafe { &*(data.as_ptr() as *const crate::elf::Elf64Header) };

    if hdr.machine != EM_X86_64 {
        return Err("not x86_64");
    }
    if hdr.elf_type != ET_EXEC && hdr.elf_type != ET_DYN {
        return Err("not executable or shared object");
    }

    let is_pie = hdr.elf_type == ET_DYN;
    let ph_off = hdr.ph_offset as usize;
    let ph_size = hdr.ph_entry_size as usize;
    let ph_count = hdr.ph_count as usize;

    let mut load_segments = Vec::new();
    let mut interp: Option<String> = None;
    let mut has_dynamic = false;

    for i in 0..ph_count {
        let off = ph_off + i * ph_size;
        if off + ph_size > data.len() {
            break;
        }

        let ph = unsafe { &*(data[off..].as_ptr() as *const crate::elf::Elf64ProgramHeader) };

        match ph.seg_type {
            PT_LOAD => {
                load_segments.push(LoadSegment {
                    vaddr: ph.vaddr,
                    memsz: ph.mem_size,
                    filesz: ph.file_size,
                    flags: ph.flags,
                });
            }
            PT_INTERP => {
                let start = ph.offset as usize;
                let end = start + ph.file_size as usize;
                if end <= data.len() {
                    let raw = &data[start..end];
                    // Strip trailing NUL
                    let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                    let mut s = String::new();
                    for &b in &raw[..len] {
                        s.push(b as char);
                    }
                    interp = Some(s);
                }
            }
            PT_DYNAMIC => {
                has_dynamic = true;
            }
            _ => {}
        }
    }

    Ok(ParsedElf {
        entry_point: hdr.entry,
        elf_type: hdr.elf_type,
        machine: hdr.machine,
        ph_offset: hdr.ph_offset,
        ph_count: hdr.ph_count,
        ph_entry_size: hdr.ph_entry_size,
        sh_offset: hdr.sh_offset,
        sh_count: hdr.sh_count,
        sh_entry_size: hdr.sh_entry_size,
        sh_strndx: hdr.sh_strndx,
        interp,
        load_segments,
        has_dynamic,
        is_pie,
    })
}

/// Parse section headers from an ELF binary.
pub fn parse_sections(data: &[u8]) -> Vec<(u32, u32, u64, u64)> {
    if data.len() < 64 {
        return Vec::new();
    }
    let hdr = unsafe { &*(data.as_ptr() as *const crate::elf::Elf64Header) };
    let sh_off = hdr.sh_offset as usize;
    let sh_size = hdr.sh_entry_size as usize;
    let sh_count = hdr.sh_count as usize;

    let mut sections = Vec::new();
    for i in 0..sh_count {
        let off = sh_off + i * sh_size;
        if off + sh_size > data.len() || sh_size < 64 {
            break;
        }
        let sh = unsafe { &*(data[off..].as_ptr() as *const Elf64SectionHeader) };
        sections.push((sh.sh_name, sh.sh_type, sh.sh_addr, sh.sh_size));
    }
    sections
}

fn section_type_name(t: u32) -> &'static str {
    match t {
        SHT_NULL => "NULL",
        SHT_PROGBITS => "PROGBITS",
        SHT_SYMTAB => "SYMTAB",
        SHT_STRTAB => "STRTAB",
        SHT_RELA => "RELA",
        SHT_NOBITS => "NOBITS",
        SHT_DYNSYM => "DYNSYM",
        _ => "OTHER",
    }
}

// ---------------------------------------------------------------------------
// Stack setup (System V ABI)
// ---------------------------------------------------------------------------

/// Build the initial user stack contents in System V ABI format.
/// Returns a Vec of u64 values to be pushed onto the stack (top to bottom).
///
/// Stack layout (high to low):
///   - null terminator for strings
///   - environment strings
///   - argument strings
///   - padding for alignment
///   - auxv entries (AT_NULL terminated)
///   - envp pointers (NULL terminated)
///   - argv pointers (NULL terminated)
///   - argc
pub fn build_stack_data(
    argv: &[&str],
    envp: &[&str],
    entry: u64,
    phdr_addr: u64,
    phnum: u16,
    phent: u16,
    base: u64,
) -> Vec<u8> {
    let mut stack = Vec::new();

    // Serialize strings into a data area and record offsets
    let mut string_area = Vec::new();
    let mut argv_offsets = Vec::new();
    let mut envp_offsets = Vec::new();

    for arg in argv.iter().take(MAX_ARGV) {
        argv_offsets.push(string_area.len());
        string_area.extend_from_slice(arg.as_bytes());
        string_area.push(0);
    }
    for env in envp.iter().take(MAX_ENVP) {
        envp_offsets.push(string_area.len());
        string_area.extend_from_slice(env.as_bytes());
        string_area.push(0);
    }

    // We'll lay out the stack with strings at the top (high addresses)
    // For the simulation, we build a flat buffer representing the stack contents.
    // In a real loader, these would be placed relative to the stack pointer.

    let string_base = USER_STACK_TOP - string_area.len() as u64;
    // Align down to 16 bytes
    let string_base = string_base & !0xF;

    // Build auxiliary vector
    let mut auxv: Vec<(u64, u64)> = Vec::new();
    auxv.push((AT_PHDR, phdr_addr));
    auxv.push((AT_PHENT, phent as u64));
    auxv.push((AT_PHNUM, phnum as u64));
    auxv.push((AT_PAGESZ, PAGE_SIZE));
    auxv.push((AT_ENTRY, entry));
    auxv.push((AT_BASE, base));
    auxv.push((AT_UID, 0));
    auxv.push((AT_EUID, 0));
    auxv.push((AT_GID, 0));
    auxv.push((AT_EGID, 0));
    auxv.push((AT_RANDOM, string_base)); // point at some "random" data
    auxv.push((AT_NULL, 0));

    // Pack: argc, argv ptrs, NULL, envp ptrs, NULL, auxv pairs, string data
    // argc
    let argc = argv.len() as u64;
    stack.extend_from_slice(&argc.to_le_bytes());

    // argv pointers
    for off in &argv_offsets {
        let ptr = string_base + *off as u64;
        stack.extend_from_slice(&ptr.to_le_bytes());
    }
    stack.extend_from_slice(&0u64.to_le_bytes()); // NULL terminator

    // envp pointers
    for off in &envp_offsets {
        let ptr = string_base + *off as u64;
        stack.extend_from_slice(&ptr.to_le_bytes());
    }
    stack.extend_from_slice(&0u64.to_le_bytes()); // NULL terminator

    // auxiliary vector
    for (key, val) in &auxv {
        stack.extend_from_slice(&key.to_le_bytes());
        stack.extend_from_slice(&val.to_le_bytes());
    }

    // string data
    stack.extend_from_slice(&string_area);

    stack
}

// ---------------------------------------------------------------------------
// Program loading
// ---------------------------------------------------------------------------

/// Execute an ELF binary by path (from VFS or built-in).
pub fn exec_elf(path: &str, argv: &[&str], envp: &[&str]) -> Result<u32, &'static str> {
    EXEC_COUNT.fetch_add(1, Ordering::Relaxed);

    // Try built-in first
    let data = if let Some(builtin) = builtin_elf(path) {
        builtin
    } else if let Ok(contents) = crate::vfs::cat(path) {
        contents.into_bytes()
    } else {
        EXEC_FAILURES.fetch_add(1, Ordering::Relaxed);
        return Err("file not found");
    };

    // Parse the ELF
    let parsed = parse_elf(&data)?;

    if parsed.load_segments.is_empty() {
        EXEC_FAILURES.fetch_add(1, Ordering::Relaxed);
        return Err("no LOAD segments");
    }

    // Determine base address for PIE
    let base = if parsed.is_pie { DEFAULT_LOAD_BASE } else { 0 };
    let actual_entry = parsed.entry_point + base;

    // Log interpreter if present
    if let Some(ref interp) = parsed.interp {
        crate::serial_println!("[elf-exec] interpreter: {}", interp);
    }

    // Count pages needed
    let mut total_pages = 0u64;
    for seg in &parsed.load_segments {
        let start = (seg.vaddr + base) & !(PAGE_SIZE - 1);
        let end = seg.vaddr + base + seg.memsz;
        let pages = (end - start + PAGE_SIZE - 1) / PAGE_SIZE;
        total_pages += pages;
    }
    total_pages += USER_STACK_PAGES;
    TOTAL_PAGES_MAPPED.fetch_add(total_pages, Ordering::Relaxed);

    // Compute phdr address in memory
    let ph_addr = if !parsed.load_segments.is_empty() {
        parsed.load_segments[0].vaddr + base + parsed.ph_offset
    } else {
        0
    };

    // Build stack data
    let _stack_data = build_stack_data(
        argv, envp, actual_entry, ph_addr,
        parsed.ph_count, parsed.ph_entry_size, base,
    );

    // Assign a PID
    let pid = EXEC_COUNT.load(Ordering::Relaxed) as u32;

    // Record loaded ELF info
    let loaded = LoadedElf {
        pid,
        path: String::from(path),
        entry_point: actual_entry,
        base_address: base,
        ph_addr,
        ph_count: parsed.ph_count,
        ph_entry_size: parsed.ph_entry_size,
        interp: parsed.interp.clone(),
        load_segments: parsed.load_segments,
        is_pie: parsed.is_pie,
    };

    {
        let mut elfs = LOADED_ELFS.lock();
        if elfs.len() >= MAX_LOADED {
            // Evict oldest
            elfs.remove(0);
        }
        elfs.push(loaded);
    }

    crate::serial_println!(
        "[elf-exec] loaded '{}' pid={} entry={:#x} base={:#x} pages={}",
        path, pid, actual_entry, base, total_pages
    );

    // In a full implementation, we would:
    // 1. Create a user page table
    // 2. Map all LOAD segments
    // 3. Copy data from ELF file into mapped pages
    // 4. Set up the stack with argv/envp/auxv
    // 5. Enter ring 3 at the entry point
    //
    // For now, delegate to the existing elf_loader for actual execution:
    let name = if let Some(last) = path.rsplit('/').next() { last } else { path };
    if let Err(e) = crate::elf_loader::load_and_exec(name, &data) {
        EXEC_FAILURES.fetch_add(1, Ordering::Relaxed);
        return Err(e);
    }

    Ok(pid)
}

// ---------------------------------------------------------------------------
// Interpreter support
// ---------------------------------------------------------------------------

/// Load a dynamic interpreter (e.g., ld-linux-x86-64.so.2) at INTERP_LOAD_BASE.
/// Returns the interpreter's entry point.
pub fn load_interpreter(data: &[u8]) -> Result<u64, &'static str> {
    let parsed = parse_elf(data)?;
    let base = INTERP_LOAD_BASE;
    let entry = parsed.entry_point + base;

    crate::serial_println!(
        "[elf-exec] interpreter loaded at base={:#x} entry={:#x}",
        base, entry
    );
    Ok(entry)
}

// ---------------------------------------------------------------------------
// Core dump
// ---------------------------------------------------------------------------

/// Record a simplified core dump for a crashed process.
pub fn save_core_dump(pid: u32, reason: &str, rip: u64, rsp: u64, rflags: u64) {
    let segments = {
        let elfs = LOADED_ELFS.lock();
        elfs.iter()
            .find(|e| e.pid == pid)
            .map(|e| e.load_segments.clone())
            .unwrap_or_default()
    };

    let dump = CoreDump {
        pid,
        reason: String::from(reason),
        rip,
        rsp,
        rflags,
        segments,
    };

    crate::serial_println!(
        "[elf-exec] core dump: pid={} reason='{}' rip={:#x} rsp={:#x}",
        pid, reason, rip, rsp
    );

    let mut dumps = CORE_DUMPS.lock();
    if dumps.len() >= MAX_LOADED {
        dumps.remove(0);
    }
    dumps.push(dump);
}

/// Get core dumps for a process.
pub fn get_core_dumps(pid: u32) -> Vec<CoreDump> {
    CORE_DUMPS.lock().iter().filter(|d| d.pid == pid).cloned().collect()
}

// ---------------------------------------------------------------------------
// /proc/[pid]/exe
// ---------------------------------------------------------------------------

/// Get the path of the loaded ELF for a given PID (for /proc/[pid]/exe).
pub fn proc_pid_exe(pid: u32) -> Option<String> {
    LOADED_ELFS.lock()
        .iter()
        .find(|e| e.pid == pid)
        .map(|e| e.path.clone())
}

/// Get loaded ELF info for a PID.
pub fn get_loaded_elf(pid: u32) -> Option<LoadedElf> {
    LOADED_ELFS.lock()
        .iter()
        .find(|e| e.pid == pid)
        .cloned()
}

// ---------------------------------------------------------------------------
// Query API
// ---------------------------------------------------------------------------

/// List all known built-in executables.
pub fn list_executables() -> Vec<&'static str> {
    alloc::vec!["/bin/hello", "/bin/echo", "/bin/cat"]
}

/// List all currently loaded ELF processes.
pub fn list_loaded() -> Vec<(u32, String, u64, bool)> {
    LOADED_ELFS.lock()
        .iter()
        .map(|e| (e.pid, e.path.clone(), e.entry_point, e.is_pie))
        .collect()
}

/// Format detailed info about a loaded ELF.
pub fn format_loaded_elf(pid: u32) -> String {
    let elfs = LOADED_ELFS.lock();
    let e = match elfs.iter().find(|e| e.pid == pid) {
        Some(e) => e,
        None => return format!("pid {} not found", pid),
    };

    let mut out = format!(
        "ELF: {} (pid {})\n  entry:  {:#x}\n  base:   {:#x}\n  PIE:    {}\n  phdr:   {:#x} ({} x {} bytes)\n",
        e.path, e.pid, e.entry_point, e.base_address,
        if e.is_pie { "yes" } else { "no" },
        e.ph_addr, e.ph_count, e.ph_entry_size,
    );

    if let Some(ref interp) = e.interp {
        out.push_str(&format!("  interp: {}\n", interp));
    }

    if !e.load_segments.is_empty() {
        out.push_str("  Segments:\n");
        for seg in &e.load_segments {
            out.push_str(&format!(
                "    vaddr={:#010x} memsz={:#08x} filesz={:#08x} [{}]\n",
                seg.vaddr, seg.memsz, seg.filesz, seg.perm_string(),
            ));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Info / stats
// ---------------------------------------------------------------------------

/// Summary of the ELF exec subsystem.
pub fn elf_exec_info() -> String {
    let loaded = LOADED_ELFS.lock().len();
    let dumps = CORE_DUMPS.lock().len();
    let builtins = list_executables().len();

    let mut out = format!(
        "[elf-exec] loaded: {} | builtins: {} | core dumps: {}\n",
        loaded, builtins, dumps
    );

    let elfs = LOADED_ELFS.lock();
    for e in elfs.iter() {
        out.push_str(&format!(
            "  pid={} {} entry={:#x} {}\n",
            e.pid, e.path, e.entry_point,
            if e.is_pie { "PIE" } else { "static" },
        ));
    }
    out
}

/// Execution statistics.
pub fn elf_exec_stats() -> String {
    let total = EXEC_COUNT.load(Ordering::Relaxed);
    let failures = EXEC_FAILURES.load(Ordering::Relaxed);
    let pages = TOTAL_PAGES_MAPPED.load(Ordering::Relaxed);
    let loaded = LOADED_ELFS.lock().len();
    let dumps = CORE_DUMPS.lock().len();

    format!(
        "[elf-exec] exec calls: {} | failures: {} | loaded: {} | \
         pages mapped: {} | core dumps: {}",
        total, failures, loaded, pages, dumps
    )
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the ELF exec subsystem.
pub fn init() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::serial_println!(
        "[elf-exec] initialized ({} built-in executables)",
        list_executables().len()
    );
}
