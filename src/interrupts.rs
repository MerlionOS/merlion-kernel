/// Interrupt Descriptor Table setup and handlers.
/// Handles CPU exceptions (breakpoint, double fault) and hardware
/// interrupts (PIT timer via the 8259 PIC).

use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};
use spin::Lazy;
use crate::gdt;
use crate::serial_println;

/// PIC interrupt offset — hardware IRQs start at interrupt 32.
const PIC_OFFSET_PRIMARY: u8 = 32;
const PIC_OFFSET_SECONDARY: u8 = PIC_OFFSET_PRIMARY + 8;

/// Hardware interrupt numbers (offset from PIC_OFFSET_PRIMARY).
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
    unsafe {
        idt.double_fault
            .set_handler_fn(double_fault_handler)
            .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
    }

    // Hardware interrupts
    idt[HardwareInterrupt::Timer as u8 as usize].set_handler_fn(timer_handler);
    idt[HardwareInterrupt::Keyboard as u8 as usize].set_handler_fn(keyboard_handler);

    idt
});

/// Load the IDT and initialize the PICs.
pub fn init() {
    IDT.load();
    unsafe { PICS.lock().initialize() };
    // Enable hardware interrupts
    x86_64::instructions::interrupts::enable();
}

// --- Exception handlers ---

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    serial_println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
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
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(HardwareInterrupt::Timer as u8);
    }
}

extern "x86-interrupt" fn keyboard_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;
    use crate::keyboard;

    // Read the scancode from the PS/2 data port
    let scancode: u8 = unsafe { Port::new(0x60).read() };

    if let Some(ch) = keyboard::scancode_to_ascii(scancode) {
        serial_println!("key: '{}'", ch);
    }

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(HardwareInterrupt::Keyboard as u8);
    }
}
