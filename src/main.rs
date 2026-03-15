#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod allocator;
mod gdt;
mod interrupts;
mod keyboard;
mod memory;
mod serial;
mod vga;

use core::panic::PanicInfo;
use bootloader::{entry_point, BootInfo};
use alloc::vec::Vec;

entry_point!(kernel_main);

/// Kernel entry point, called by the bootloader with boot info.
fn kernel_main(boot_info: &'static BootInfo) -> ! {
    serial::SERIAL1.lock().init();
    serial_println!("MerlionOS v0.1.0 booting...");

    vga::print_banner();
    serial_println!("[ok] VGA banner displayed");

    gdt::init();
    serial_println!("[ok] GDT loaded");

    interrupts::init();
    serial_println!("[ok] IDT loaded, interrupts enabled");

    // Set up virtual memory and frame allocator
    let phys_mem_offset = x86_64::VirtAddr::new(boot_info.physical_memory_offset);
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe {
        memory::BootInfoFrameAllocator::init(&boot_info.memory_map)
    };
    serial_println!("[ok] Page table and frame allocator initialized");

    // Set up the kernel heap
    allocator::init(&mut mapper, &mut frame_allocator)
        .expect("heap initialization failed");
    serial_println!("[ok] Heap allocator initialized ({}K)", allocator::HEAP_SIZE / 1024);

    // Quick test: allocate on the heap to prove it works
    let mut v = Vec::new();
    for i in 0..10 {
        v.push(i);
    }
    serial_println!("[ok] Heap test passed: {:?}", v);

    serial_println!("Kernel initialization complete.");
    serial_println!("Keyboard input active — type in the QEMU window.");
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
    serial_println!("KERNEL PANIC: {}", info);

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
