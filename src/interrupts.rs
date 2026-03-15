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

    // Try demand paging first
    if let Some(addr) = fault_addr.as_u64().into() {
        if crate::paging::handle_page_fault(addr) {
            return; // successfully mapped, resume execution
        }
    }

    serial_println!("EXCEPTION: PAGE FAULT");
    serial_println!("  Accessed address: {:?}", fault_addr);
    serial_println!("  Error code: {:?}", error_code);
    serial_println!("{:#?}", stack_frame);
    klog_println!("PAGE FAULT at {:?}, error: {:?}", fault_addr, error_code);

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

extern "x86-interrupt" fn keyboard_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;
    use crate::keyboard;

    let scancode: u8 = unsafe { Port::new(0x60).read() };

    if let Some(event) = keyboard::process_scancode(scancode) {
        if crate::login::is_logging_in() {
            crate::login::handle_input(event);
        } else if crate::snake::is_running() {
            crate::snake::handle_input(event);
        } else if crate::editor::is_editing() {
            crate::editor::handle_input(event);
        } else if crate::top::is_running() {
            crate::top::handle_input(event);
        } else if crate::watch::is_running() {
            crate::watch::handle_input(event);
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
/// dispatch function with syscall number and arguments, then returns via iretq.
#[unsafe(naked)]
extern "C" fn syscall_trampoline() {
    core::arch::naked_asm!(
        // Save all caller-saved registers we'll clobber
        "push rax",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",

        // Set up arguments for syscall_dispatch_inner(rax, rdi, rsi, rdx)
        // Currently: rax is at [rsp+64], rdi at [rsp+0], rsi at [rsp+8], rdx at [rsp+16]
        // But we pushed them, so we need to read from the stack.
        // Order pushed: rax, rcx, rdx, rsi, rdi, r8, r9, r10, r11
        // rsp+0 = r11, rsp+8 = r10, rsp+16 = r9, rsp+24 = r8
        // rsp+32 = rdi, rsp+40 = rsi, rsp+48 = rdx, rsp+56 = rcx, rsp+64 = rax

        // C ABI: arg1=rdi, arg2=rsi, arg3=rdx, arg4=rcx
        // We want: dispatch(rax, rdi, rsi, rdx)
        "mov rcx, [rsp+48]",  // arg4 = original rdx
        "mov rdx, [rsp+40]",  // arg3 = original rsi
        "mov rsi, [rsp+32]",  // arg2 = original rdi
        "mov rdi, [rsp+64]",  // arg1 = original rax

        // Call the Rust dispatch function
        "call {dispatch}",

        // Restore registers
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

/// Rust-callable syscall dispatch. Called by the trampoline with the
/// original user register values.
extern "C" fn syscall_dispatch_inner(syscall_num: u64, arg1: u64, arg2: u64, arg3: u64) {
    crate::syscall::dispatch(syscall_num, arg1, arg2, arg3);
}
