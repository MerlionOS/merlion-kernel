/// User process management.
/// Creates per-process page tables, maps user code and stack,
/// and transitions to ring 3 via iretq.

use x86_64::structures::paging::{Page, PageTableFlags};
use x86_64::{PhysAddr, VirtAddr};
use crate::{memory, serial_println, klog_println};

/// Virtual address where user code is mapped.
const USER_CODE_ADDR: u64 = 0x40_0000; // 4 MiB
/// Virtual address where user stack top is.
const USER_STACK_ADDR: u64 = 0x80_0000; // 8 MiB
const USER_STACK_PAGES: u64 = 2; // 8 KiB stack

/// Embedded user program: "hello" — writes a message via syscall, then exits.
/// Assembled from:
///   mov rax, 0             ; SYS_WRITE
///   lea rdi, [rip+msg]     ; buf pointer (RIP-relative)
///   mov rsi, 18            ; length
///   int 0x80
///   mov rax, 1             ; SYS_EXIT
///   xor rdi, rdi           ; code = 0
///   int 0x80
///   jmp $                  ; should not reach
///   msg: "Hello userspace!\n"
#[rustfmt::skip]
static USER_HELLO: &[u8] = &[
    // mov rax, 0
    0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00,
    // lea rdi, [rip+21]  (offset to msg: 7+7+3+2+2 = 21 bytes ahead)
    0x48, 0x8D, 0x3D, 0x15, 0x00, 0x00, 0x00,
    // mov rsi, 18
    0x48, 0xC7, 0xC6, 0x12, 0x00, 0x00, 0x00,
    // int 0x80
    0xCD, 0x80,
    // mov rax, 1
    0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00,
    // xor rdi, rdi
    0x48, 0x31, 0xFF,
    // int 0x80
    0xCD, 0x80,
    // jmp $ (infinite loop fallback)
    0xEB, 0xFE,
    // msg: "Hello userspace!\n\0"
    b'H', b'e', b'l', b'l', b'o', b' ',
    b'u', b's', b'e', b'r', b's', b'p',
    b'a', b'c', b'e', b'!', b'\n', 0,
];

/// Embedded user program: "counter" — writes 3 messages with yields between.
/// Assembled from:
///   mov r12, 3             ; counter
///   .loop:
///   mov rax, 0             ; SYS_WRITE
///   lea rdi, [rip+msg]     ; buf
///   mov rsi, 10            ; len
///   int 0x80
///   mov rax, 2             ; SYS_YIELD
///   int 0x80
///   dec r12
///   jnz .loop
///   mov rax, 1             ; SYS_EXIT
///   xor rdi, rdi
///   int 0x80
///   jmp $
///   msg: "counting\n\0"
#[rustfmt::skip]
static USER_COUNTER: &[u8] = &[
    // mov r12, 3
    0x49, 0xC7, 0xC4, 0x03, 0x00, 0x00, 0x00,
    // .loop (offset 7):
    // mov rax, 0
    0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00,
    // lea rdi, [rip+24] (offset to msg from next instruction)
    0x48, 0x8D, 0x3D, 0x18, 0x00, 0x00, 0x00,
    // mov rsi, 10
    0x48, 0xC7, 0xC6, 0x0A, 0x00, 0x00, 0x00,
    // int 0x80
    0xCD, 0x80,
    // mov rax, 2 (SYS_YIELD)
    0x48, 0xC7, 0xC0, 0x02, 0x00, 0x00, 0x00,
    // int 0x80
    0xCD, 0x80,
    // dec r12
    0x49, 0xFF, 0xCC,
    // jnz .loop (offset 7, relative = 7 - current_pos)
    // current_pos after jnz = 50, target = 7, diff = 7 - 50 = -43 = 0xD5
    0x75, 0xD5,
    // mov rax, 1
    0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00,
    // xor rdi, rdi
    0x48, 0x31, 0xFF,
    // int 0x80
    0xCD, 0x80,
    // jmp $
    0xEB, 0xFE,
    // msg: "counting\n\0"
    b'c', b'o', b'u', b'n', b't', b'i',
    b'n', b'g', b'\n', 0,
];

/// Get a user program by name.
pub fn get_program(name: &str) -> Option<&'static [u8]> {
    match name {
        "hello" => Some(USER_HELLO),
        "counter" => Some(USER_COUNTER),
        _ => None,
    }
}

/// List available user programs.
pub fn list_programs() -> &'static [&'static str] {
    &["hello", "counter"]
}

/// Spawn a user process: create page table, map code+stack, run in ring 3.
pub fn run_user_program(name: &str) -> Result<(), &'static str> {
    let program = get_program(name).ok_or("unknown program")?;

    serial_println!("[process] loading '{}' ({} bytes)", name, program.len());
    klog_println!("[process] loading '{}' ({} bytes)", name, program.len());

    // 1. Create a new page table cloning kernel mappings
    let (_pml4_frame, mut mapper) =
        memory::create_user_page_table().ok_or("failed to create page table")?;

    // 2. Map user code page (readable + user-accessible)
    let code_page = Page::containing_address(VirtAddr::new(USER_CODE_ADDR));
    let code_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    let code_frame = memory::map_page(&mut mapper, code_page, code_flags)
        .ok_or("failed to map code page")?;

    // 3. Copy program code to the mapped frame
    let code_dest = memory::phys_to_virt(code_frame.start_address());
    unsafe {
        // Zero the page first
        core::ptr::write_bytes(code_dest.as_mut_ptr::<u8>(), 0, 4096);
        // Copy program bytes
        core::ptr::copy_nonoverlapping(
            program.as_ptr(),
            code_dest.as_mut_ptr::<u8>(),
            program.len(),
        );
    }

    // 4. Map user stack pages (writable + user-accessible)
    let stack_flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    for i in 0..USER_STACK_PAGES {
        let stack_page = Page::containing_address(
            VirtAddr::new(USER_STACK_ADDR - (i + 1) * 4096),
        );
        memory::map_page(&mut mapper, stack_page, stack_flags)
            .ok_or("failed to map stack page")?;
    }

    // 5. Switch to the new page table and jump to user mode
    let pml4_phys = _pml4_frame.start_address();
    serial_println!("[process] entering ring 3 at {:#x}", USER_CODE_ADDR);

    enter_ring3(pml4_phys, USER_CODE_ADDR, USER_STACK_ADDR);

    // 6. Restore kernel page table when user program returns
    serial_println!("[process] '{}' returned to kernel", name);
    Ok(())
}

/// Switch CR3 to the user page table and iretq to ring 3.
fn enter_ring3(pml4_phys: PhysAddr, code_addr: u64, stack_top: u64) {
    // GDT selectors for ring 3 (from gdt.rs)
    let user_data_seg: u64 = (4 << 3) | 3; // 0x23
    let user_code_seg: u64 = (5 << 3) | 3; // 0x2B

    unsafe {
        core::arch::asm!(
            // Save kernel CR3 on the stack so we can restore it
            "mov rax, cr3",
            "push rax",
            // Switch to user page table
            "mov rax, {pml4}",
            "mov cr3, rax",
            // Build iretq frame: SS, RSP, RFLAGS, CS, RIP
            "push {user_ds}",   // SS
            "push {user_sp}",   // RSP
            "pushfq",
            "pop rax",
            "or rax, 0x200",    // set IF (interrupts enabled)
            "push rax",         // RFLAGS
            "push {user_cs}",   // CS
            "push {user_ip}",   // RIP
            "iretq",
            pml4 = in(reg) pml4_phys.as_u64(),
            user_ds = in(reg) user_data_seg,
            user_sp = in(reg) stack_top,
            user_cs = in(reg) user_code_seg,
            user_ip = in(reg) code_addr,
            out("rax") _,
        );
    }
}
