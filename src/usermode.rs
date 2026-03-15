/// User-mode (ring 3) groundwork.
/// Demonstrates transitioning from kernel mode (ring 0) to user mode
/// (ring 3) using iretq, with a syscall (int 0x80) to return.
///
/// This is a minimal proof-of-concept — the "user program" runs in
/// kernel address space with a separate stack. Real user-mode isolation
/// requires per-process page tables (future work).

use x86_64::VirtAddr;

/// Size of the user-mode stack.
const USER_STACK_SIZE: usize = 4096 * 2;

/// Static user-mode stack.
static mut USER_STACK: [u8; USER_STACK_SIZE] = [0; USER_STACK_SIZE];

/// Jump to ring 3, executing `user_program`.
/// Returns when the user program issues int 0x80.
pub fn enter_usermode() {
    let user_code = user_program as *const () as usize;
    let user_stack =
        VirtAddr::from_ptr(&raw const USER_STACK).as_u64() + USER_STACK_SIZE as u64;

    // GDT layout (from gdt.rs):
    //   0: null
    //   1: kernel code (0x08)
    //   2-3: TSS (0x10, 0x18) — TSS takes two entries
    //   4: user data (0x23 = index 4, RPL 3)
    //   5: user code (0x2B = index 5, RPL 3)
    let user_data_seg: u16 = (4 << 3) | 3; // 0x23
    let user_code_seg: u16 = (5 << 3) | 3; // 0x2B

    unsafe {
        core::arch::asm!(
            // Push stack segment (user data)
            "push {user_ds:r}",
            // Push user stack pointer
            "push {user_sp:r}",
            // Push RFLAGS with IF set (interrupts enabled)
            "pushfq",
            "pop rax",
            "or rax, 0x200",  // set IF
            "push rax",
            // Push code segment (user code)
            "push {user_cs:r}",
            // Push user instruction pointer
            "push {user_ip:r}",
            // iretq pops: RIP, CS, RFLAGS, RSP, SS
            "iretq",
            user_ds = in(reg) user_data_seg as u64,
            user_sp = in(reg) user_stack,
            user_cs = in(reg) user_code_seg as u64,
            user_ip = in(reg) user_code as u64,
            out("rax") _,
        );
    }
}

/// A trivial "user-mode program" that issues int 0x80 to return to the kernel.
#[no_mangle]
extern "C" fn user_program() {
    // Write marker to serial (via int 0x80 syscall)
    unsafe {
        core::arch::asm!(
            "int 0x80",
            in("rax") 0u64,  // syscall number 0 = "hello from usermode"
        );
    }
    // If we get here, loop forever
    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}
