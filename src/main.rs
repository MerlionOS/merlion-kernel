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
mod shell;
mod vga;

use core::panic::PanicInfo;
use bootloader::{entry_point, BootInfo};

entry_point!(kernel_main);

/// Kernel entry point, called by the bootloader with boot info.
fn kernel_main(boot_info: &'static BootInfo) -> ! {
    serial::SERIAL1.lock().init();
    serial_println!("MerlionOS v0.1.0 booting...");

    vga::print_banner();
    serial_println!("[ok] VGA banner displayed");

    gdt::init();
    println!("[ok] GDT loaded");
    serial_println!("[ok] GDT loaded");

    interrupts::init();
    println!("[ok] IDT loaded, interrupts enabled");
    serial_println!("[ok] IDT loaded, interrupts enabled");

    let phys_mem_offset = x86_64::VirtAddr::new(boot_info.physical_memory_offset);
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe {
        memory::BootInfoFrameAllocator::init(&boot_info.memory_map)
    };
    println!("[ok] Memory initialized");
    serial_println!("[ok] Page table and frame allocator initialized");

    allocator::init(&mut mapper, &mut frame_allocator)
        .expect("heap initialization failed");
    println!("[ok] Heap ready ({}K)", allocator::HEAP_SIZE / 1024);
    serial_println!("[ok] Heap allocator initialized ({}K)", allocator::HEAP_SIZE / 1024);

    println!();
    println!("Type 'help' for available commands.");
    serial_println!("Kernel initialization complete. Shell active.");
    shell::prompt();

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
    println!("\nKERNEL PANIC: {}", info);

    halt_loop();
}
