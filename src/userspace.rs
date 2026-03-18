/// Userspace process execution for MerlionOS.
/// Loads ELF binaries into user address space (Ring 3),
/// sets up user stack, and switches to user mode via iretq.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;
use crate::{serial_println, klog_println};

// ═══════════════════════════════════════════════════════════════════
//  CONSTANTS
// ═══════════════════════════════════════════════════════════════════

/// Where program text (.text, .rodata) is loaded.
const TEXT_BASE: u64 = 0x0000_0040_0000;
/// Where program data (.data, .bss) is loaded.
const DATA_BASE: u64 = 0x0000_0060_0000;
/// Heap start (grows up via brk).
const HEAP_BASE: u64 = 0x0000_0080_0000;
/// User stack top (grows down, 8 MiB max).
const USER_STACK_TOP: u64 = 0x0000_7FFF_F000;
/// Number of stack pages to pre-map (16 KiB).
const STACK_PAGES: u64 = 4;

/// Maximum number of user processes.
const MAX_PROCESSES: usize = 16;

/// GDT selectors (from gdt.rs):
///   user data = index 5, RPL 3 => (5 << 3) | 3 = 0x2B
///   user code = index 6, RPL 3 => (6 << 3) | 3 = 0x33
const USER_DS: u64 = 0x2B;
const USER_CS: u64 = 0x33;

// ═══════════════════════════════════════════════════════════════════
//  TYPES
// ═══════════════════════════════════════════════════════════════════

/// Per-process file descriptor entry.
#[derive(Clone)]
pub struct FdEntry {
    pub path: String,
    pub offset: usize,
    pub flags: u32,
}

/// Process state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserProcessState {
    Ready,
    Running,
    Waiting,
    Zombie,
    Exited,
}

/// User process descriptor.
pub struct UserProcess {
    pub pid: u32,
    pub name: String,
    pub state: UserProcessState,
    pub page_table_phys: u64,
    pub entry_point: u64,
    pub user_stack_top: u64,
    pub brk: u64,
    pub exit_code: Option<i32>,
    pub fd_table: [Option<FdEntry>; 16],
}

impl UserProcess {
    fn new(pid: u32, name: &str) -> Self {
        // Pre-open stdin, stdout, stderr
        const NONE_FD: Option<FdEntry> = None;
        let mut fds = [NONE_FD; 16];
        fds[0] = Some(FdEntry { path: String::from("/dev/stdin"), offset: 0, flags: 0 });
        fds[1] = Some(FdEntry { path: String::from("/dev/stdout"), offset: 0, flags: 1 });
        fds[2] = Some(FdEntry { path: String::from("/dev/stderr"), offset: 0, flags: 1 });

        Self {
            pid,
            name: String::from(name),
            state: UserProcessState::Ready,
            page_table_phys: 0,
            entry_point: TEXT_BASE,
            user_stack_top: USER_STACK_TOP,
            brk: HEAP_BASE,
            exit_code: None,
            fd_table: fds,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
//  GLOBAL STATE
// ═══════════════════════════════════════════════════════════════════

static NEXT_PID: AtomicU32 = AtomicU32::new(1);
static CURRENT_PID: AtomicU32 = AtomicU32::new(0);

struct ProcessTable {
    slots: [Option<UserProcess>; MAX_PROCESSES],
}

impl ProcessTable {
    const fn new() -> Self {
        const NONE: Option<UserProcess> = None;
        Self { slots: [NONE; MAX_PROCESSES] }
    }

    fn find_slot(&self) -> Option<usize> {
        self.slots.iter().position(|s| s.is_none())
    }

    fn find_by_pid(&self, pid: u32) -> Option<usize> {
        self.slots.iter().position(|s| {
            matches!(s, Some(p) if p.pid == pid)
        })
    }
}

static PROCESS_TABLE: Mutex<ProcessTable> = Mutex::new(ProcessTable::new());

// ═══════════════════════════════════════════════════════════════════
//  ELF PARSING (minimal)
// ═══════════════════════════════════════════════════════════════════

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 0x3E;
const PT_LOAD: u32 = 1;

/// Minimal ELF64 header fields we care about.
struct Elf64Info {
    entry: u64,
    segments: Vec<Elf64Phdr>,
}

#[derive(Clone)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_filesz: u64,
    p_memsz: u64,
}

fn parse_elf64(data: &[u8]) -> Result<Elf64Info, &'static str> {
    if data.len() < 64 {
        return Err("ELF too small");
    }
    if data[0..4] != ELF_MAGIC {
        return Err("not an ELF file");
    }
    if data[4] != 2 {
        return Err("not 64-bit ELF");
    }
    if data[5] != 1 {
        return Err("not little-endian");
    }

    let e_type = u16::from_le_bytes([data[16], data[17]]);
    let e_machine = u16::from_le_bytes([data[18], data[19]]);
    if e_type != ET_EXEC {
        return Err("not an executable ELF");
    }
    if e_machine != EM_X86_64 {
        return Err("not x86_64 ELF");
    }

    let e_entry = u64::from_le_bytes(data[24..32].try_into().unwrap());
    let e_phoff = u64::from_le_bytes(data[32..40].try_into().unwrap()) as usize;
    let e_phentsize = u16::from_le_bytes([data[54], data[55]]) as usize;
    let e_phnum = u16::from_le_bytes([data[56], data[57]]) as usize;

    let mut segments = Vec::new();
    for i in 0..e_phnum {
        let off = e_phoff + i * e_phentsize;
        if off + 56 > data.len() {
            break;
        }
        let ph = &data[off..];
        let p_type = u32::from_le_bytes(ph[0..4].try_into().unwrap());
        let p_flags = u32::from_le_bytes(ph[4..8].try_into().unwrap());
        let p_offset = u64::from_le_bytes(ph[8..16].try_into().unwrap());
        let p_vaddr = u64::from_le_bytes(ph[16..24].try_into().unwrap());
        let p_filesz = u64::from_le_bytes(ph[32..40].try_into().unwrap());
        let p_memsz = u64::from_le_bytes(ph[40..48].try_into().unwrap());

        segments.push(Elf64Phdr {
            p_type,
            p_flags,
            p_offset,
            p_vaddr,
            p_filesz,
            p_memsz,
        });
    }

    Ok(Elf64Info { entry: e_entry, segments })
}

// ═══════════════════════════════════════════════════════════════════
//  BUILT-IN USER PROGRAMS
// ═══════════════════════════════════════════════════════════════════

/// Hello program: writes a message then exits.
/// Machine code for x86_64:
///   mov rax, 0          ; SYS_WRITE
///   lea rdi, [rip+msg]  ; buffer
///   mov rsi, 31         ; length
///   int 0x80
///   mov rax, 1          ; SYS_EXIT
///   xor rdi, rdi        ; exit code 0
///   int 0x80
///   jmp $               ; safety halt
///   msg: "Hello from MerlionOS userspace!\n"
#[rustfmt::skip]
const HELLO_CODE: &[u8] = &[
    // mov rax, 0 (SYS_WRITE)
    0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00,
    // lea rdi, [rip + 0x17] (offset to msg: RIP=14, msg=37, 37-14=23=0x17)
    0x48, 0x8D, 0x3D, 0x17, 0x00, 0x00, 0x00,
    // mov rsi, 31 (msg_len)
    0x48, 0xC7, 0xC6, 0x1F, 0x00, 0x00, 0x00,
    // int 0x80
    0xCD, 0x80,
    // mov rax, 1 (SYS_EXIT)
    0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00,
    // xor rdi, rdi
    0x48, 0x31, 0xFF,
    // int 0x80
    0xCD, 0x80,
    // jmp $ (infinite loop safety)
    0xEB, 0xFE,
    // "Hello from MerlionOS userspace!\n"
    b'H', b'e', b'l', b'l', b'o', b' ', b'f', b'r', b'o', b'm', b' ',
    b'M', b'e', b'r', b'l', b'i', b'o', b'n', b'O', b'S', b' ',
    b'u', b's', b'e', b'r', b's', b'p', b'a', b'c', b'e', b'!', b'\n',
];

/// cat-test program: writes "File syscalls ready!\n" then exits.
/// Same structure as hello — SYS_WRITE + SYS_EXIT.
#[rustfmt::skip]
const CAT_TEST_CODE: &[u8] = &[
    // mov rax, 0 (SYS_WRITE)
    0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00,
    // lea rdi, [rip + 0x17] (offset to msg: RIP=14, msg=37, 37-14=23=0x17)
    0x48, 0x8D, 0x3D, 0x17, 0x00, 0x00, 0x00,
    // mov rsi, 21 (msg_len = "File syscalls ready!\n")
    0x48, 0xC7, 0xC6, 0x15, 0x00, 0x00, 0x00,
    // int 0x80
    0xCD, 0x80,
    // mov rax, 1 (SYS_EXIT)
    0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00,
    // xor rdi, rdi
    0x48, 0x31, 0xFF,
    // int 0x80
    0xCD, 0x80,
    // jmp $ (infinite loop safety)
    0xEB, 0xFE,
    // "File syscalls ready!\n"
    b'F', b'i', b'l', b'e', b' ', b's', b'y', b's', b'c', b'a', b'l', b'l',
    b's', b' ', b'r', b'e', b'a', b'd', b'y', b'!', b'\n',
];

/// qfc-test program: writes "QFC miner running in MerlionOS userspace!\n" then exits.
#[rustfmt::skip]
const QFC_TEST_CODE: &[u8] = &[
    // mov rax, 0 (SYS_WRITE)
    0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00,
    // lea rdi, [rip + 0x17] (offset to msg: RIP=14, msg=37, 37-14=23=0x17)
    0x48, 0x8D, 0x3D, 0x17, 0x00, 0x00, 0x00,
    // mov rsi, 42 (msg_len = "QFC miner running in MerlionOS userspace!\n")
    0x48, 0xC7, 0xC6, 0x2A, 0x00, 0x00, 0x00,
    // int 0x80
    0xCD, 0x80,
    // mov rax, 1 (SYS_EXIT)
    0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00,
    // xor rdi, rdi
    0x48, 0x31, 0xFF,
    // int 0x80
    0xCD, 0x80,
    // jmp $ (infinite loop safety)
    0xEB, 0xFE,
    // "QFC miner running in MerlionOS userspace!\n"
    b'Q', b'F', b'C', b' ', b'm', b'i', b'n', b'e', b'r', b' ',
    b'r', b'u', b'n', b'n', b'i', b'n', b'g', b' ',
    b'i', b'n', b' ',
    b'M', b'e', b'r', b'l', b'i', b'o', b'n', b'O', b'S', b' ',
    b'u', b's', b'e', b'r', b's', b'p', b'a', b'c', b'e', b'!', b'\n',
];

/// Counter program: writes "tick N\n" three times, yielding between each.
#[rustfmt::skip]
const COUNTER_CODE: &[u8] = &[
    // mov r12, 3 (loop count)
    0x49, 0xC7, 0xC4, 0x03, 0x00, 0x00, 0x00,
    // .loop:
    // mov rax, 0 (SYS_WRITE)
    0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00,
    // lea rdi, [rip + 0x18] (offset to msg)
    0x48, 0x8D, 0x3D, 0x18, 0x00, 0x00, 0x00,
    // mov rsi, 5 (msg len: "tick\n")
    0x48, 0xC7, 0xC6, 0x05, 0x00, 0x00, 0x00,
    // int 0x80
    0xCD, 0x80,
    // mov rax, 2 (SYS_YIELD)
    0x48, 0xC7, 0xC0, 0x02, 0x00, 0x00, 0x00,
    // int 0x80
    0xCD, 0x80,
    // dec r12
    0x49, 0xFF, 0xCC,
    // jnz .loop (back 0x2B = 43 bytes)
    0x75, 0xD5,
    // mov rax, 1 (SYS_EXIT)
    0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00,
    // xor rdi, rdi
    0x48, 0x31, 0xFF,
    // int 0x80
    0xCD, 0x80,
    // jmp $
    0xEB, 0xFE,
    // "tick\n"
    b't', b'i', b'c', b'k', b'\n',
];

/// Getpid program: calls SYS_GETPID then prints result, then exits.
#[rustfmt::skip]
const GETPID_CODE: &[u8] = &[
    // mov rax, 3 (SYS_GETPID)
    0x48, 0xC7, 0xC0, 0x03, 0x00, 0x00, 0x00,
    // int 0x80
    0xCD, 0x80,
    // (pid now in rax, but we just print a static message)
    // mov rax, 0 (SYS_WRITE)
    0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00,
    // lea rdi, [rip + 0x12]
    0x48, 0x8D, 0x3D, 0x12, 0x00, 0x00, 0x00,
    // mov rsi, 12
    0x48, 0xC7, 0xC6, 0x0C, 0x00, 0x00, 0x00,
    // int 0x80
    0xCD, 0x80,
    // mov rax, 1 (SYS_EXIT)
    0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00,
    // xor rdi, rdi
    0x48, 0x31, 0xFF,
    // int 0x80
    0xCD, 0x80,
    // jmp $
    0xEB, 0xFE,
    // "getpid ok!\n\0"
    b'g', b'e', b't', b'p', b'i', b'd', b' ', b'o', b'k', b'!', b'\n', 0,
];

/// Syscall-test program: calls SYS_GETPID, converts return value to ASCII digit,
/// writes "pid=N\n" via SYS_WRITE, then exits. Proves syscall return values work.
#[rustfmt::skip]
const SYSCALL_TEST_CODE: &[u8] = &[
    // mov rax, 3 (SYS_GETPID)
    0x48, 0xC7, 0xC0, 0x03, 0x00, 0x00, 0x00,
    // int 0x80 — rax now has PID
    0xCD, 0x80,
    // add al, 0x30 — convert PID (0-9) to ASCII digit
    0x04, 0x30,
    // lea rcx, [rip+0x42] — points to buf at offset 84
    0x48, 0x8D, 0x0D, 0x42, 0x00, 0x00, 0x00,
    // mov [rcx], al — store digit in buf
    0x88, 0x01,
    // mov rax, 0 (SYS_WRITE)
    0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00,
    // lea rdi, [rip+0x2E] — points to msg "pid=" at offset 80
    0x48, 0x8D, 0x3D, 0x2E, 0x00, 0x00, 0x00,
    // mov rsi, 4 (len of "pid=")
    0x48, 0xC7, 0xC6, 0x04, 0x00, 0x00, 0x00,
    // int 0x80
    0xCD, 0x80,
    // mov rax, 0 (SYS_WRITE)
    0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00,
    // lea rdi, [rip+0x1B] — points to buf at offset 84
    0x48, 0x8D, 0x3D, 0x1B, 0x00, 0x00, 0x00,
    // mov rsi, 2 (digit + newline)
    0x48, 0xC7, 0xC6, 0x02, 0x00, 0x00, 0x00,
    // int 0x80
    0xCD, 0x80,
    // mov rax, 1 (SYS_EXIT)
    0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00,
    // xor rdi, rdi — exit code 0
    0x48, 0x31, 0xFF,
    // int 0x80
    0xCD, 0x80,
    // jmp $ (safety halt)
    0xEB, 0xFE,
    // msg: "pid=" (offset 80)
    b'p', b'i', b'd', b'=',
    // buf: "0\n" (offset 84) — digit placeholder + newline
    b'0', b'\n',
];

/// Open-test program: calls SYS_OPEN("/tmp"), prints "fd=open:N\n", then exits.
/// Tests that SYS_OPEN returns a valid fd in rax.
#[rustfmt::skip]
const OPEN_TEST_CODE: &[u8] = &[
    // mov rax, 100 (SYS_OPEN)
    0x48, 0xC7, 0xC0, 0x64, 0x00, 0x00, 0x00,
    // lea rdi, [rip+0x5D] — points to path "/tmp" at offset 107
    0x48, 0x8D, 0x3D, 0x5D, 0x00, 0x00, 0x00,
    // mov rsi, 4 (path_len)
    0x48, 0xC7, 0xC6, 0x04, 0x00, 0x00, 0x00,
    // xor rdx, rdx — flags=0
    0x48, 0x31, 0xD2,
    // int 0x80 — rax now has fd
    0xCD, 0x80,
    // push rax — save fd
    0x50,
    // mov rax, 0 (SYS_WRITE)
    0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00,
    // lea rdi, [rip+0x3A] — points to msg "fd=open:" at offset 99
    0x48, 0x8D, 0x3D, 0x3A, 0x00, 0x00, 0x00,
    // mov rsi, 8 (len of "fd=open:")
    0x48, 0xC7, 0xC6, 0x08, 0x00, 0x00, 0x00,
    // int 0x80
    0xCD, 0x80,
    // pop rax — restore fd
    0x58,
    // add al, 0x30 — convert fd to ASCII digit
    0x04, 0x30,
    // lea rcx, [rip+0x33] — points to digit at offset 111
    0x48, 0x8D, 0x0D, 0x33, 0x00, 0x00, 0x00,
    // mov [rcx], al — store digit
    0x88, 0x01,
    // mov rax, 0 (SYS_WRITE)
    0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00,
    // lea rdi, [rip+0x23] — points to digit at offset 111
    0x48, 0x8D, 0x3D, 0x23, 0x00, 0x00, 0x00,
    // mov rsi, 2 (digit + newline)
    0x48, 0xC7, 0xC6, 0x02, 0x00, 0x00, 0x00,
    // int 0x80
    0xCD, 0x80,
    // mov rax, 1 (SYS_EXIT)
    0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00,
    // xor rdi, rdi — exit code 0
    0x48, 0x31, 0xFF,
    // int 0x80
    0xCD, 0x80,
    // jmp $ (safety halt)
    0xEB, 0xFE,
    // msg: "fd=open:" (offset 99)
    b'f', b'd', b'=', b'o', b'p', b'e', b'n', b':',
    // path: "/tmp" (offset 107)
    b'/', b't', b'm', b'p',
    // digit: "0\n" (offset 111)
    b'0', b'\n',
];

/// Build a minimal valid ELF64 executable wrapping raw machine code.
fn build_elf64(code: &[u8]) -> Vec<u8> {
    let ehdr_size: u64 = 64;
    let phdr_size: u64 = 56;
    let total_header = ehdr_size + phdr_size;
    let entry = TEXT_BASE;
    let file_size = total_header as usize + code.len();
    let mem_size = code.len() as u64;

    let mut elf = Vec::with_capacity(file_size);

    // ELF header (64 bytes)
    elf.extend_from_slice(&ELF_MAGIC);          // e_ident[0..4]: magic
    elf.push(2);                                 // EI_CLASS: 64-bit
    elf.push(1);                                 // EI_DATA: little-endian
    elf.push(1);                                 // EI_VERSION: current
    elf.push(0);                                 // EI_OSABI: ELFOSABI_NONE
    elf.extend_from_slice(&[0u8; 8]);            // padding
    elf.extend_from_slice(&ET_EXEC.to_le_bytes()); // e_type
    elf.extend_from_slice(&EM_X86_64.to_le_bytes()); // e_machine
    elf.extend_from_slice(&1u32.to_le_bytes());  // e_version
    elf.extend_from_slice(&entry.to_le_bytes()); // e_entry
    elf.extend_from_slice(&ehdr_size.to_le_bytes()); // e_phoff (phdrs right after ehdr)
    elf.extend_from_slice(&0u64.to_le_bytes());  // e_shoff (no section headers)
    elf.extend_from_slice(&0u32.to_le_bytes());  // e_flags
    elf.extend_from_slice(&(ehdr_size as u16).to_le_bytes()); // e_ehsize
    elf.extend_from_slice(&(phdr_size as u16).to_le_bytes()); // e_phentsize
    elf.extend_from_slice(&1u16.to_le_bytes());  // e_phnum
    elf.extend_from_slice(&0u16.to_le_bytes());  // e_shentsize
    elf.extend_from_slice(&0u16.to_le_bytes());  // e_shnum
    elf.extend_from_slice(&0u16.to_le_bytes());  // e_shstrndx

    // Program header (56 bytes) — single PT_LOAD
    elf.extend_from_slice(&PT_LOAD.to_le_bytes()); // p_type
    let pf_r: u32 = 4;
    let pf_x: u32 = 1;
    elf.extend_from_slice(&(pf_r | pf_x).to_le_bytes()); // p_flags: PF_R | PF_X
    elf.extend_from_slice(&total_header.to_le_bytes()); // p_offset: code starts after headers
    elf.extend_from_slice(&entry.to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&entry.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&mem_size.to_le_bytes()); // p_filesz
    elf.extend_from_slice(&mem_size.to_le_bytes()); // p_memsz
    elf.extend_from_slice(&0x1000u64.to_le_bytes()); // p_align

    // Code section
    elf.extend_from_slice(code);

    elf
}

/// Look up a built-in user program by name, returning an ELF binary.
pub fn get_builtin_program(name: &str) -> Option<Vec<u8>> {
    let code: &[u8] = match name {
        "hello" => HELLO_CODE,
        "cat-test" => CAT_TEST_CODE,
        "qfc-test" => QFC_TEST_CODE,
        "counter" => COUNTER_CODE,
        "getpid" => GETPID_CODE,
        "syscall-test" => SYSCALL_TEST_CODE,
        "open-test" => OPEN_TEST_CODE,
        _ => return None,
    };
    Some(build_elf64(code))
}

/// List available built-in program names.
pub fn list_builtin_programs() -> &'static [&'static str] {
    &["hello", "cat-test", "qfc-test", "counter", "getpid", "syscall-test", "open-test"]
}

// ═══════════════════════════════════════════════════════════════════
//  PROCESS CREATION
// ═══════════════════════════════════════════════════════════════════

/// Load an ELF binary and prepare a user process. Returns the PID.
#[cfg(target_arch = "x86_64")]
pub fn create_process(name: &str, elf_data: &[u8]) -> Result<u32, &'static str> {
    use x86_64::structures::paging::{Page, PageTableFlags};
    use x86_64::VirtAddr;

    // Parse ELF
    let elf = parse_elf64(elf_data)?;

    // Allocate PID
    let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);

    serial_println!("[userspace] create_process: mapping user pages in kernel page table");

    // Map user pages directly in kernel page table (no separate address space).
    // This has no isolation but lets us debug Ring 3 execution first.
    let user_flags =
        PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    let user_rw_flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

    // Map PT_LOAD segments
    serial_println!("[userspace] ELF entry={:#x} segments={}", elf.entry, elf.segments.len());
    let mut max_addr: u64 = HEAP_BASE;
    for (si, seg) in elf.segments.iter().enumerate() {
        serial_println!("[userspace] seg[{}] type={} vaddr={:#x} memsz={} filesz={} flags={}",
            si, seg.p_type, seg.p_vaddr, seg.p_memsz, seg.p_filesz, seg.p_flags);
        if seg.p_type != PT_LOAD {
            continue;
        }
        let vaddr = seg.p_vaddr;
        let memsz = seg.p_memsz;
        if memsz == 0 {
            continue;
        }

        // Determine flags: writable if PF_W (bit 1) is set
        let flags = if seg.p_flags & 2 != 0 { user_rw_flags } else { user_flags };

        // Map pages covering [vaddr, vaddr + memsz)
        let start_page = vaddr & !0xFFF;
        let end = vaddr + memsz;
        let mut page_addr = start_page;
        while page_addr < end {
            let page = Page::containing_address(VirtAddr::new(page_addr));
            serial_println!("[userspace] mapping page at {:#x}", page_addr);
            let frame = crate::memory::map_page_global(page, flags)
                .ok_or("failed to map ELF segment page")?;
            serial_println!("[userspace] mapped -> frame {:#x}", frame.start_address().as_u64());

            // Copy file data into the page
            let page_start = page_addr;
            let page_end = page_addr + 4096;
            let dest = crate::memory::phys_to_virt(frame.start_address());
            unsafe {
                // Zero the page first
                core::ptr::write_bytes(dest.as_mut_ptr::<u8>(), 0, 4096);
            }
            // Copy the overlap between [seg file range] and [page range]
            let file_start = seg.p_offset;
            let file_end = seg.p_offset + seg.p_filesz;
            let seg_vstart = seg.p_vaddr;
            // For each byte in this page that corresponds to file data, copy it
            if file_end > file_start {
                let copy_start = if seg_vstart > page_start { seg_vstart } else { page_start };
                let copy_end_file = seg_vstart + seg.p_filesz;
                let copy_end = if copy_end_file < page_end { copy_end_file } else { page_end };
                if copy_start < copy_end {
                    let src_off = (copy_start - seg_vstart + seg.p_offset) as usize;
                    let dst_off = (copy_start - page_start) as usize;
                    let len = (copy_end - copy_start) as usize;
                    if src_off + len <= elf_data.len() {
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                elf_data.as_ptr().add(src_off),
                                dest.as_mut_ptr::<u8>().add(dst_off),
                                len,
                            );
                        }
                    }
                }
            }

            if page_addr + 4096 > max_addr {
                max_addr = page_addr + 4096;
            }
            page_addr += 4096;
        }
    }

    // Map user stack pages (at top of lower half)
    for i in 0..STACK_PAGES {
        let stack_page = Page::containing_address(
            VirtAddr::new(USER_STACK_TOP - (i + 1) * 4096),
        );
        let _frame = crate::memory::map_page_global(stack_page, user_rw_flags)
            .ok_or("failed to map stack page")?;
    }

    // Create process descriptor
    let mut proc = UserProcess::new(pid, name);
    proc.page_table_phys = 0; // using kernel page table for now
    proc.entry_point = elf.entry;
    proc.user_stack_top = USER_STACK_TOP;
    proc.brk = max_addr;

    // Insert into process table
    let mut table = PROCESS_TABLE.lock();
    let slot = table.find_slot().ok_or("process table full")?;
    table.slots[slot] = Some(proc);

    serial_println!("[userspace] created process '{}' pid={} entry={:#x}", name, pid, elf.entry);
    klog_println!("[userspace] created process '{}' pid={} entry={:#x}", name, pid, elf.entry);

    Ok(pid)
}

#[cfg(not(target_arch = "x86_64"))]
pub fn create_process(name: &str, _elf_data: &[u8]) -> Result<u32, &'static str> {
    serial_println!("[userspace] create_process '{}': not supported on this architecture", name);
    Err("userspace processes only supported on x86_64")
}

// ═══════════════════════════════════════════════════════════════════
//  ENTER USERSPACE
// ═══════════════════════════════════════════════════════════════════

/// Flag: set to true when user process has exited.
static USER_EXITED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Switch to Ring 3 and start executing user code.
/// Does NOT return — the user process runs until SYS_EXIT.
/// After SYS_EXIT, the iret returns to user's jmp$ loop,
/// and timer/keyboard interrupts resume normal kernel operation.
#[cfg(target_arch = "x86_64")]
pub fn enter_userspace(pid: u32) -> ! {
    let (entry, stack, _cr3) = {
        let mut table = PROCESS_TABLE.lock();
        let slot = table.find_by_pid(pid).expect("process not found");
        let proc = table.slots[slot].as_mut().unwrap();
        proc.state = UserProcessState::Running;
        (proc.entry_point, proc.user_stack_top, proc.page_table_phys)
    };

    CURRENT_PID.store(pid, Ordering::SeqCst);

    serial_println!("[userspace] entering ring 3: pid={} entry={:#x} stack={:#x}", pid, entry, stack);

    USER_EXITED.store(false, core::sync::atomic::Ordering::SeqCst);

    unsafe {
        // iretq to Ring 3 — this does NOT return.
        // When user calls SYS_EXIT, the syscall handler sets USER_EXITED=true
        // and the user code hits its jmp$ loop. Timer interrupt preempts it
        // and we check USER_EXITED in the loop below.
        core::arch::asm!(
            "push 0x2B",        // SS
            "push {stack}",     // RSP
            "push 0x200",       // RFLAGS
            "push 0x33",        // CS
            "push {entry}",     // RIP
            "iretq",
            stack = in(reg) stack,
            entry = in(reg) entry,
            options(noreturn),
        );
    }
}

/// Mark that userspace process has exited.
/// Called from SYS_EXIT handler.
pub fn return_to_kernel() {
    USER_EXITED.store(true, core::sync::atomic::Ordering::SeqCst);
    CURRENT_PID.store(0, Ordering::SeqCst);
    // Don't try to restore kernel stack — just return from syscall.
    // The user code's jmp$ loop + timer interrupt will handle the rest.
}

/// Check if userspace process has finished.
pub fn has_user_exited() -> bool {
    USER_EXITED.load(core::sync::atomic::Ordering::SeqCst)
}

#[cfg(not(target_arch = "x86_64"))]
pub fn enter_userspace(pid: u32) -> ! {
    serial_println!("[userspace] enter_userspace pid={}: not supported on this architecture", pid);
    loop { core::hint::spin_loop(); }
}

// ═══════════════════════════════════════════════════════════════════
//  PROCESS LIFECYCLE
// ═══════════════════════════════════════════════════════════════════

/// List all user processes as a formatted string.
pub fn list_processes() -> String {
    let table = PROCESS_TABLE.lock();
    let mut out = String::from("  PID  STATE     NAME\n");
    for slot in &table.slots {
        if let Some(proc) = slot {
            let state_str = match proc.state {
                UserProcessState::Ready   => "ready   ",
                UserProcessState::Running => "running ",
                UserProcessState::Waiting => "waiting ",
                UserProcessState::Zombie  => "zombie  ",
                UserProcessState::Exited  => "exited  ",
            };
            out.push_str(&format!("  {:3}  {}  {}\n", proc.pid, state_str, proc.name));
        }
    }
    out
}

/// Kill a user process by PID.
pub fn kill_process(pid: u32) -> Result<(), &'static str> {
    let mut table = PROCESS_TABLE.lock();
    let slot = table.find_by_pid(pid).ok_or("process not found")?;
    let proc = table.slots[slot].as_mut().unwrap();
    proc.state = UserProcessState::Exited;
    proc.exit_code = Some(-9);
    serial_println!("[userspace] killed pid={}", pid);
    klog_println!("[userspace] killed pid={}", pid);
    Ok(())
}

/// Wait for a process to exit. Returns exit code if it has exited.
pub fn wait_process(pid: u32) -> Option<i32> {
    let mut table = PROCESS_TABLE.lock();
    let slot = table.find_by_pid(pid)?;
    let proc = table.slots[slot].as_ref()?;
    if proc.state == UserProcessState::Exited || proc.state == UserProcessState::Zombie {
        let code = proc.exit_code.unwrap_or(0);
        // Clean up the slot
        table.slots[slot] = None;
        Some(code)
    } else {
        None
    }
}

/// Get the currently running user process PID, or None if none.
pub fn current_process() -> Option<u32> {
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    if pid == 0 { None } else { Some(pid) }
}

/// Mark a process as exited (called from syscall handler on SYS_EXIT).
pub fn exit_process(pid: u32, code: i32) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(slot) = table.find_by_pid(pid) {
        if let Some(proc) = table.slots[slot].as_mut() {
            proc.state = UserProcessState::Exited;
            proc.exit_code = Some(code);
        }
    }
    if CURRENT_PID.load(Ordering::SeqCst) == pid {
        CURRENT_PID.store(0, Ordering::SeqCst);
    }
}

// ═══════════════════════════════════════════════════════════════════
//  HIGH-LEVEL RUN COMMAND
// ═══════════════════════════════════════════════════════════════════

/// Run a built-in user program by name: creates the process and enters userspace.
pub fn run_builtin(name: &str) -> Result<(), &'static str> {
    serial_println!("[userspace] run_builtin: looking up '{}'", name);
    let elf_data = get_builtin_program(name).ok_or("unknown built-in program")?;
    serial_println!("[userspace] run_builtin: got ELF data ({} bytes)", elf_data.len());
    let pid = create_process(name, &elf_data)?;
    serial_println!("[userspace] run_builtin: process created, entering userspace pid={}", pid);
    enter_userspace(pid);
    // never reaches here
}

// ═══════════════════════════════════════════════════════════════════
//  INFO / INIT
// ═══════════════════════════════════════════════════════════════════

/// Initialization (called lazily, no work needed at boot).
pub fn init() {
    serial_println!("[userspace] userspace subsystem initialized");
    klog_println!("[userspace] userspace subsystem initialized");
}

/// Return a summary of the userspace subsystem.
pub fn userspace_info() -> String {
    let table = PROCESS_TABLE.lock();
    let active = table.slots.iter().filter(|s| s.is_some()).count();
    let running = table.slots.iter().filter(|s| {
        matches!(s, Some(p) if p.state == UserProcessState::Running)
    }).count();

    let mut info = format!(
        "Userspace Process Manager\n\
         Address space layout:\n\
         Text base:   {:#014x}\n\
         Data base:   {:#014x}\n\
         Heap base:   {:#014x}\n\
         Stack top:   {:#014x}\n\
         Stack pages: {}\n\
         Max procs:   {}\n\
         Active:      {}\n\
         Running:     {}\n\
         User CS:     {:#04x}\n\
         User DS:     {:#04x}\n",
        TEXT_BASE, DATA_BASE, HEAP_BASE, USER_STACK_TOP,
        STACK_PAGES, MAX_PROCESSES, active, running,
        USER_CS, USER_DS,
    );

    info.push_str("\nBuilt-in programs: ");
    for (i, name) in list_builtin_programs().iter().enumerate() {
        if i > 0 {
            info.push_str(", ");
        }
        info.push_str(name);
    }
    info.push('\n');

    info
}
