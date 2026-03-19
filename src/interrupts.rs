/// Interrupt Descriptor Table setup and handlers.
/// Handles CPU exceptions, hardware interrupts (PIT, keyboard),
/// and syscalls (int 0x80) with register-based ABI.

use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use x86_64::VirtAddr;
use spin::Lazy;
use crate::gdt;
use crate::{serial_println, klog_println};

const PIC_OFFSET_PRIMARY: u8 = 32;
const PIC_OFFSET_SECONDARY: u8 = PIC_OFFSET_PRIMARY + 8;
const SYSCALL_VECTOR: usize = 0x80;

#[derive(Clone, Copy)]
#[repr(u8)]
enum HardwareInterrupt {
    Timer = PIC_OFFSET_PRIMARY,
    Keyboard = PIC_OFFSET_PRIMARY + 1,
}

static PICS: spin::Mutex<pic8259::ChainedPics> = spin::Mutex::new(
    unsafe { pic8259::ChainedPics::new(PIC_OFFSET_PRIMARY, PIC_OFFSET_SECONDARY) }
);

static IDT: Lazy<InterruptDescriptorTable> = Lazy::new(|| {
    let mut idt = InterruptDescriptorTable::new();

    // CPU exceptions
    idt.breakpoint.set_handler_fn(breakpoint_handler);
    idt.page_fault.set_handler_fn(page_fault_handler);
    unsafe {
        idt.double_fault
            .set_handler_fn(double_fault_handler)
            .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
    }

    // Hardware interrupts
    idt[HardwareInterrupt::Timer as u8 as usize].set_handler_fn(timer_handler);
    idt[HardwareInterrupt::Keyboard as u8 as usize].set_handler_fn(keyboard_handler);

    // Syscall (int 0x80) — raw handler to access user registers
    unsafe {
        let handler_addr = VirtAddr::new(syscall_trampoline as *const () as u64);
        idt[SYSCALL_VECTOR]
            .set_handler_addr(handler_addr)
            .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
    }

    idt
});

pub fn init() {
    IDT.load();
    unsafe { PICS.lock().initialize() };
    x86_64::instructions::interrupts::enable();
}

// --- Exception handlers ---

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    serial_println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
    klog_println!("EXCEPTION: BREAKPOINT at {:#x}", stack_frame.instruction_pointer.as_u64());
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;
    let fault_addr = Cr2::read();

    let addr = fault_addr.as_u64();

    // Try demand paging first
    if crate::paging::handle_page_fault(addr) {
        return;
    }

    // Try Copy-on-Write handling (write fault on shared page)
    if error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE) {
        if crate::cow::handle_cow_fault(addr).is_some() {
            return;
        }
    }

    serial_println!("EXCEPTION: PAGE FAULT");
    serial_println!("  Accessed address: {:?}", fault_addr);
    serial_println!("  Error code: {:?}", error_code);
    serial_println!("{:#?}", stack_frame);
    klog_println!("PAGE FAULT at {:?}, error: {:?}", fault_addr, error_code);

    // If user process caused the fault, kill it instead of panicking
    if error_code.contains(PageFaultErrorCode::USER_MODE) {
        serial_println!("[userspace] segfault in user process — terminating");
        if let Some(pid) = crate::userspace::current_process() {
            crate::userspace::exit_process(pid, -11); // SIGSEGV
            crate::userspace::return_to_kernel();
            return;
        }
    }

    panic!("page fault at {:?}", fault_addr);
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    serial_println!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
    panic!("double fault");
}

// --- Hardware interrupt handlers ---

extern "x86-interrupt" fn timer_handler(_stack_frame: InterruptStackFrame) {
    crate::timer::tick();

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(HardwareInterrupt::Timer as u8);
    }

    crate::task::timer_tick();
}

/// Send End-of-Interrupt to PIC for a given IRQ.
/// Safety: must only be called when an interrupt is pending.
pub unsafe fn end_of_interrupt(irq: u8) {
    PICS.lock().notify_end_of_interrupt(PIC_OFFSET_PRIMARY + irq);
}

extern "x86-interrupt" fn keyboard_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;
    use crate::keyboard;

    let scancode: u8 = unsafe { Port::new(0x60).read() };

    if let Some(event) = keyboard::process_scancode(scancode) {
        if crate::login::is_logging_in() {
            crate::login::handle_input(event);
        } else if crate::snake::is_running() {
            crate::snake::handle_input(event);
        } else if crate::vim::is_active() {
            crate::vim::handle_input(event);
        } else if crate::editor::is_editing() {
            crate::editor::handle_input(event);
        } else if crate::top::is_running() {
            crate::top::handle_input(event);
        } else if crate::watch::is_running() {
            crate::watch::handle_input(event);
        } else if crate::screensaver::is_running() {
            crate::screensaver::handle_input(event);
        } else if crate::forth::is_running() {
            crate::forth::handle_input(event);
        } else if crate::chat::is_chatting() {
            crate::chat::handle_input(event);
        } else {
            crate::shell::handle_key_event(event);
        }
    }

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(HardwareInterrupt::Keyboard as u8);
    }
}

// --- Syscall handler ---

/// Raw trampoline for int 0x80. Saves user registers, calls the Rust
/// dispatch function with syscall number and 6 arguments, then returns via iretq.
///
/// Syscall ABI (matches Linux convention):
///   rax = syscall number
///   rdi = arg1, rsi = arg2, rdx = arg3
///   r10 = arg4, r8  = arg5, r9  = arg6
///   Return value in rax.
#[unsafe(naked)]
extern "C" fn syscall_trampoline() {
    core::arch::naked_asm!(
        // Save all caller-saved registers
        "push rax",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",

        // Stack layout after pushes:
        // rsp+0  = r11, rsp+8  = r10, rsp+16 = r9, rsp+24 = r8
        // rsp+32 = rdi, rsp+40 = rsi, rsp+48 = rdx, rsp+56 = rcx, rsp+64 = rax

        // C ABI: arg1=rdi, arg2=rsi, arg3=rdx, arg4=rcx, arg5=r8, arg6=r9
        // We want: dispatch(rax, rdi, rsi, rdx, r10, r8)
        //
        // Map user registers to C calling convention:
        //   C arg1 (rdi) = user rax (syscall number)
        //   C arg2 (rsi) = user rdi (arg1)
        //   C arg3 (rdx) = user rsi (arg2)
        //   C arg4 (rcx) = user rdx (arg3)
        //   C arg5 (r8)  = user r10 (arg4)
        //   C arg6 (r9)  = user r8  (arg5)

        "mov r9,  [rsp+24]",  // C arg6 = original r8  (user arg5)
        "mov r8,  [rsp+8]",   // C arg5 = original r10 (user arg4)
        "mov rcx, [rsp+48]",  // C arg4 = original rdx (user arg3)
        "mov rdx, [rsp+40]",  // C arg3 = original rsi (user arg2)
        "mov rsi, [rsp+32]",  // C arg2 = original rdi (user arg1)
        "mov rdi, [rsp+64]",  // C arg1 = original rax (syscall number)

        // Call the Rust dispatch function (returns i64 in rax)
        "call {dispatch}",

        // Store return value where saved rax will be restored from
        "mov [rsp+64], rax",

        // Restore registers (rax gets the return value)
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rax",

        "iretq",
        dispatch = sym syscall_dispatch_inner,
    );
}

/// Rust-callable syscall dispatch. Called by the trampoline with 6 arguments.
extern "C" fn syscall_dispatch_inner(
    syscall_num: u64, arg1: u64, arg2: u64, arg3: u64,
    arg4: u64, arg5: u64,
) -> i64 {
    crate::syscall::dispatch(syscall_num, arg1, arg2, arg3, arg4, arg5)
}
