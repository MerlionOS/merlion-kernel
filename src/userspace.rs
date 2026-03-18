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
    // lea rdi, [rip + 0x15] (offset to msg = 21 bytes ahead)
    0x48, 0x8D, 0x3D, 0x15, 0x00, 0x00, 0x00,
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
        "counter" => COUNTER_CODE,
        "getpid" => GETPID_CODE,
        _ => return None,
    };
    Some(build_elf64(code))
}

/// List available built-in program names.
pub fn list_builtin_programs() -> &'static [&'static str] {
    &["hello", "counter", "getpid"]
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

    // For now, map user pages in the KERNEL page table (no CR3 switch).
    // This is less isolated but lets us debug the basic Ring 3 mechanism.
    // TODO: switch to per-process page tables once Ring 3 iretq works.
    let (_pml4_frame, _user_mapper) =
        crate::memory::create_user_page_table().ok_or("failed to create user page table")?;

    let user_flags =
        PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    let user_rw_flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

    // Get kernel page table mapper for mapping user pages
    let phys_offset = crate::memory::phys_mem_offset();
    let l4_table = unsafe { crate::memory::active_level_4_table(phys_offset) };
    let mut mapper = unsafe { x86_64::structures::paging::OffsetPageTable::new(l4_table, phys_offset) };

    // Map PT_LOAD segments
    let mut max_addr: u64 = HEAP_BASE;
    for seg in &elf.segments {
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
            let frame = crate::memory::map_page(&mut mapper, page, flags)
                .ok_or("failed to map ELF segment page")?;

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
        let _frame = crate::memory::map_page(&mut mapper, stack_page, user_rw_flags)
            .ok_or("failed to map stack page")?;
    }

    // Create process descriptor
    let mut proc = UserProcess::new(pid, name);
    proc.page_table_phys = _pml4_frame.start_address().as_u64();
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

/// Switch to Ring 3 and start executing user code for the given process.
#[cfg(target_arch = "x86_64")]
pub fn enter_userspace(pid: u32) -> Result<(), &'static str> {
    let (entry, stack, _cr3) = {
        let mut table = PROCESS_TABLE.lock();
        let slot = table.find_by_pid(pid).ok_or("process not found")?;
        let proc = table.slots[slot].as_mut().unwrap();
        proc.state = UserProcessState::Running;
        (proc.entry_point, proc.user_stack_top, proc.page_table_phys)
    };

    CURRENT_PID.store(pid, Ordering::SeqCst);

    serial_println!("[userspace] entering ring 3: pid={} entry={:#x} stack={:#x}", pid, entry, stack);

    // Send EOI + enable interrupts (we're called from interrupt context via shell)
    unsafe {
        crate::interrupts::end_of_interrupt(1);
    }
    x86_64::instructions::interrupts::enable();

    unsafe {
        do_iretq(entry, stack, 0);
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn do_iretq(entry: u64, stack: u64, cr3_val: u64) -> ! {
    // Skip CR3 switch for now — use kernel page table
    let _ = cr3_val;

    // iretq to Ring 3
    core::arch::asm!(
        "push 0x2B",        // SS = user data (index 5, RPL 3)
        "push {stack}",     // RSP = user stack
        "push 0x200",       // RFLAGS = IF=1
        "push 0x33",        // CS = user code (index 6, RPL 3)
        "push {entry}",     // RIP = entry point
        "iretq",
        stack = in(reg) stack,
        entry = in(reg) entry,
        options(noreturn),
    );
}

#[cfg(not(target_arch = "x86_64"))]
pub fn enter_userspace(pid: u32) -> Result<(), &'static str> {
    serial_println!("[userspace] enter_userspace pid={}: not supported on this architecture", pid);
    Err("userspace execution only supported on x86_64")
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
    let elf_data = get_builtin_program(name).ok_or("unknown built-in program")?;
    let pid = create_process(name, &elf_data)?;
    enter_userspace(pid)
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
