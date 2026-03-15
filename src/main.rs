#![no_std]
#![no_main]

mod vga;

use core::panic::PanicInfo;

/// Kernel entry point, called by the bootloader.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    vga::print_banner();
    halt_loop();
}

/// Halt the CPU in a loop. Used after kernel init and on panic.
fn halt_loop() -> ! {
    loop {
        x86_64_hlt();
    }
}

/// Execute the HLT instruction to save power while waiting.
#[inline(always)]
fn x86_64_hlt() {
    unsafe {
        core::arch::asm!("hlt");
    }
}

/// Panic handler — writes a message to the VGA buffer and halts.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Red on black attribute
    const PANIC_ATTR: u8 = 0x0C;
    let vga = 0xB8000 as *mut u8;

    // Write "PANIC: " at row 24 (last line)
    let msg = b"PANIC!";
    let row_offset = 24 * 80 * 2;
    for (i, &byte) in msg.iter().enumerate() {
        unsafe {
            vga.add(row_offset + i * 2).write_volatile(byte);
            vga.add(row_offset + i * 2 + 1).write_volatile(PANIC_ATTR);
        }
    }

    // If we have a message, write a truncated version after "PANIC! "
    if let Some(message) = info.message().as_str() {
        let start = row_offset + msg.len() * 2 + 2; // +2 for a space
        // Write space
        unsafe {
            vga.add(start - 2).write_volatile(b' ');
            vga.add(start - 1).write_volatile(PANIC_ATTR);
        }
        for (i, byte) in message.bytes().enumerate() {
            if i >= 72 {
                break;
            }
            unsafe {
                vga.add(start + i * 2).write_volatile(byte);
                vga.add(start + i * 2 + 1).write_volatile(PANIC_ATTR);
            }
        }
    }

    halt_loop();
}
