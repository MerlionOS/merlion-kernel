/// Interrupt Descriptor Table setup and handlers.
/// Handles CPU exceptions (breakpoint, double fault, page fault) and
/// hardware interrupts (PIT timer, PS/2 keyboard).

use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use spin::Lazy;
use crate::gdt;
use crate::{serial_println, klog_println};

/// PIC interrupt offset — hardware IRQs start at interrupt 32.
const PIC_OFFSET_PRIMARY: u8 = 32;
const PIC_OFFSET_SECONDARY: u8 = PIC_OFFSET_PRIMARY + 8;

/// Syscall interrupt vector.
const SYSCALL_VECTOR: u8 = 0x80;

#[derive(Clone, Copy)]
#[repr(u8)]
enum HardwareInterrupt {
    Timer = PIC_OFFSET_PRIMARY,
    Keyboard = PIC_OFFSET_PRIMARY + 1,
}

/// 8259 PIC pair, remapped to interrupts 32–47.
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

    // Syscall (int 0x80) — callable from ring 3
    let syscall_entry = idt[SYSCALL_VECTOR as usize].set_handler_fn(syscall_handler);
    syscall_entry.set_privilege_level(x86_64::PrivilegeLevel::Ring3);

    idt
});

/// Load the IDT and initialize the PICs.
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

    // Preemptive scheduling: yield on every timer tick
    crate::task::timer_tick();
}

extern "x86-interrupt" fn keyboard_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;
    use crate::keyboard;

    let scancode: u8 = unsafe { Port::new(0x60).read() };

    if let Some(ch) = keyboard::scancode_to_ascii(scancode) {
        crate::shell::handle_key(ch);
    }

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(HardwareInterrupt::Keyboard as u8);
    }
}

// --- Syscall handler ---

extern "x86-interrupt" fn syscall_handler(_stack_frame: InterruptStackFrame) {
    // Minimal syscall: just log that user-mode called us
    serial_println!("[syscall] int 0x80 from user-mode");
    klog_println!("[syscall] int 0x80 received");
}
