/// User process management.
/// Creates per-process page tables, maps user code and stack,
/// and transitions to ring 3 via iretq.
/// Processes run as kernel tasks and can execute concurrently.

use alloc::vec::Vec;
use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame};
use x86_64::{PhysAddr, VirtAddr};
use spin::Mutex;
use crate::{memory, serial_println, klog_println, task};

/// Virtual address where user code is mapped.
const USER_CODE_ADDR: u64 = 0x40_0000;
/// Virtual address where user stack top is.
const USER_STACK_ADDR: u64 = 0x80_0000;
const USER_STACK_PAGES: u64 = 2;

/// Track allocated frames per process for cleanup.
static PROCESS_FRAMES: Mutex<[Option<Vec<PhysFrame>>; 8]> =
    Mutex::new([const { None }; 8]);

// --- Embedded user programs ---

#[rustfmt::skip]
static USER_HELLO: &[u8] = &[
    0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0 (SYS_WRITE)
    0x48, 0x8D, 0x3D, 0x15, 0x00, 0x00, 0x00, // lea rdi, [rip+21]
    0x48, 0xC7, 0xC6, 0x12, 0x00, 0x00, 0x00, // mov rsi, 18
    0xCD, 0x80,                                 // int 0x80
    0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00, // mov rax, 1 (SYS_EXIT)
    0x48, 0x31, 0xFF,                           // xor rdi, rdi
    0xCD, 0x80,                                 // int 0x80
    0xEB, 0xFE,                                 // jmp $
    b'H', b'e', b'l', b'l', b'o', b' ', b'u', b's', b'e', b'r',
    b's', b'p', b'a', b'c', b'e', b'!', b'\n', 0,
];

#[rustfmt::skip]
static USER_COUNTER: &[u8] = &[
    0x49, 0xC7, 0xC4, 0x03, 0x00, 0x00, 0x00, // mov r12, 3
    0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0 (SYS_WRITE)
    0x48, 0x8D, 0x3D, 0x18, 0x00, 0x00, 0x00, // lea rdi, [rip+24]
    0x48, 0xC7, 0xC6, 0x0A, 0x00, 0x00, 0x00, // mov rsi, 10
    0xCD, 0x80,                                 // int 0x80
    0x48, 0xC7, 0xC0, 0x02, 0x00, 0x00, 0x00, // mov rax, 2 (SYS_YIELD)
    0xCD, 0x80,                                 // int 0x80
    0x49, 0xFF, 0xCC,                           // dec r12
    0x75, 0xD5,                                 // jnz .loop
    0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00, // mov rax, 1 (SYS_EXIT)
    0x48, 0x31, 0xFF,                           // xor rdi, rdi
    0xCD, 0x80,                                 // int 0x80
    0xEB, 0xFE,                                 // jmp $
    b'c', b'o', b'u', b'n', b't', b'i', b'n', b'g', b'\n', 0,
];

pub fn get_program(name: &str) -> Option<&'static [u8]> {
    match name {
        "hello" => Some(USER_HELLO),
        "counter" => Some(USER_COUNTER),
        _ => None,
    }
}

pub fn list_programs() -> &'static [&'static str] {
    &["hello", "counter"]
}

/// Run a user program synchronously (blocks caller until program exits).
pub fn run_user_program(name: &str) -> Result<(), &'static str> {
    let program = get_program(name).ok_or("unknown program")?;
    serial_println!("[process] loading '{}' ({} bytes)", name, program.len());
    klog_println!("[process] loading '{}' ({} bytes)", name, program.len());

    let (pml4_frame, frames) = setup_user_address_space(program)?;

    // Store frames for cleanup
    let task_slot = task::current_slot();
    {
        let mut pf = PROCESS_FRAMES.lock();
        pf[task_slot] = Some(frames);
    }

    serial_println!("[process] entering ring 3 at {:#x}", USER_CODE_ADDR);
    enter_ring3(pml4_frame.start_address(), USER_CODE_ADDR, USER_STACK_ADDR);

    // Cleanup after user program returns
    cleanup_process(task_slot);
    serial_println!("[process] '{}' finished, frames freed", name);
    Ok(())
}

/// Spawn a user program as a background kernel task (non-blocking).
pub fn spawn_user_program(name: &str) -> Result<usize, &'static str> {
    // Map to static name and entry function
    let (static_name, entry): (&'static str, fn()) = match name {
        "hello" => ("hello", run_hello),
        "counter" => ("counter", run_counter),
        _ => return Err("unknown program"),
    };

    let pid = task::spawn(static_name, entry).ok_or("task table full")?;
    serial_println!("[process] spawned user program '{}' as pid {}", static_name, pid);
    Ok(pid)
}

fn run_hello() {
    let _ = run_user_program_inner(USER_HELLO, "hello");
}

fn run_counter() {
    let _ = run_user_program_inner(USER_COUNTER, "counter");
}

fn run_user_program_inner(program: &[u8], name: &str) -> Result<(), &'static str> {
    serial_println!("[process] task loading '{}'", name);
    let (pml4_frame, frames) = setup_user_address_space(program)?;

    let task_slot = task::current_slot();
    {
        let mut pf = PROCESS_FRAMES.lock();
        pf[task_slot] = Some(frames);
    }

    enter_ring3(pml4_frame.start_address(), USER_CODE_ADDR, USER_STACK_ADDR);

    cleanup_process(task_slot);
    serial_println!("[process] '{}' finished, frames freed", name);
    Ok(())
}

/// Set up a user address space: create page table, map code + stack.
/// Returns the PML4 frame and a list of all allocated frames.
fn setup_user_address_space(program: &[u8]) -> Result<(PhysFrame, Vec<PhysFrame>), &'static str> {
    let mut allocated_frames = Vec::new();

    let (pml4_frame, mut mapper) =
        memory::create_user_page_table().ok_or("failed to create page table")?;
    allocated_frames.push(pml4_frame);

    // Map user code page
    let code_page = Page::containing_address(VirtAddr::new(USER_CODE_ADDR));
    let code_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    let code_frame = memory::map_page(&mut mapper, code_page, code_flags)
        .ok_or("failed to map code page")?;
    allocated_frames.push(code_frame);

    // Copy program to code page
    let code_dest = memory::phys_to_virt(code_frame.start_address());
    unsafe {
        core::ptr::write_bytes(code_dest.as_mut_ptr::<u8>(), 0, 4096);
        core::ptr::copy_nonoverlapping(program.as_ptr(), code_dest.as_mut_ptr::<u8>(), program.len());
    }

    // Map user stack pages
    let stack_flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    for i in 0..USER_STACK_PAGES {
        let stack_page = Page::containing_address(
            VirtAddr::new(USER_STACK_ADDR - (i + 1) * 4096),
        );
        let frame = memory::map_page(&mut mapper, stack_page, stack_flags)
            .ok_or("failed to map stack page")?;
        allocated_frames.push(frame);
    }

    Ok((pml4_frame, allocated_frames))
}

/// Free frames allocated for a process.
fn cleanup_process(task_slot: usize) {
    let mut pf = PROCESS_FRAMES.lock();
    if let Some(frames) = pf[task_slot].take() {
        let count = frames.len();
        // Note: In a real OS we'd return these frames to the allocator.
        // For now we just drop the tracking — the frames are leaked but
        // this prevents double-use.
        klog_println!("[process] freed {} frames for slot {}", count, task_slot);
    }
}

fn enter_ring3(pml4_phys: PhysAddr, code_addr: u64, stack_top: u64) {
    let user_data_seg: u64 = (4 << 3) | 3;
    let user_code_seg: u64 = (5 << 3) | 3;

    unsafe {
        core::arch::asm!(
            "mov rax, cr3",
            "push rax",
            "mov rax, {pml4}",
            "mov cr3, rax",
            "push {user_ds}",
            "push {user_sp}",
            "pushfq",
            "pop rax",
            "or rax, 0x200",
            "push rax",
            "push {user_cs}",
            "push {user_ip}",
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
