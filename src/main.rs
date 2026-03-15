#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

mod gdt;
mod interrupts;
mod serial;
mod vga;

use core::panic::PanicInfo;

/// Kernel entry point, called by the bootloader.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    serial::SERIAL1.lock().init();
    serial_println!("MerlionOS v0.1.0 booting...");

    vga::print_banner();
    serial_println!("[ok] VGA banner displayed");

    gdt::init();
    serial_println!("[ok] GDT loaded");

    interrupts::init();
    serial_println!("[ok] IDT loaded, interrupts enabled");

    serial_println!("Kernel initialization complete. Halting.");
    halt_loop();
}

/// Halt the CPU in a loop. Used after kernel init and on panic.
pub fn halt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}

/// Panic handler — prints to both serial and VGA, then halts.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Print full panic info to serial (includes file, line, message)
    serial_println!("KERNEL PANIC: {}", info);

    // Also show a brief message on VGA (last row, red)
    const PANIC_ATTR: u8 = 0x0C;
    let vga = 0xB8000 as *mut u8;
    let row_offset = 24 * 80 * 2;

    let msg = b"KERNEL PANIC! See serial output for details.";
    for (i, &byte) in msg.iter().enumerate() {
        unsafe {
            vga.add(row_offset + i * 2).write_volatile(byte);
            vga.add(row_offset + i * 2 + 1).write_volatile(PANIC_ATTR);
        }
    }

    halt_loop();
}
