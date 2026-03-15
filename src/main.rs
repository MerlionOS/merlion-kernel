#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod allocator;
mod gdt;
mod interrupts;
mod keyboard;
mod log;
mod memory;
mod process;
mod serial;
mod shell;
mod syscall;
mod task;
mod timer;
mod usermode;
mod vga;

use core::panic::PanicInfo;
use bootloader::{entry_point, BootInfo};

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    serial::SERIAL1.lock().init();
    serial_println!("MerlionOS v0.1.0 booting...");
    klog_println!("MerlionOS v0.1.0 booting...");

    vga::print_banner();

    gdt::init();
    println!("[ok] GDT loaded");
    serial_println!("[ok] GDT loaded");

    timer::init();
    serial_println!("[ok] PIT configured at {} Hz", timer::PIT_FREQUENCY_HZ);

    interrupts::init();
    println!("[ok] IDT + interrupts enabled");
    serial_println!("[ok] IDT loaded, interrupts enabled");

    // Initialize memory system (page tables + global frame allocator)
    let phys_mem_offset = x86_64::VirtAddr::new(boot_info.physical_memory_offset);
    let mut mapper = unsafe { memory::init(phys_mem_offset, &boot_info.memory_map) };
    println!("[ok] Memory initialized");
    serial_println!("[ok] Page table and frame allocator initialized");

    // Initialize heap using the global frame allocator
    memory::with_frame_allocator(|fa| {
        allocator::init(&mut mapper, fa)
            .expect("heap initialization failed");
    });
    println!("[ok] Heap ready ({}K)", allocator::HEAP_SIZE / 1024);
    serial_println!("[ok] Heap allocator initialized ({}K)", allocator::HEAP_SIZE / 1024);

    task::init();
    println!("[ok] Task system ready");
    serial_println!("[ok] Task system initialized");

    klog_println!("Kernel initialization complete.");
    println!();
    println!("Type 'help' for available commands.");
    serial_println!("Kernel initialization complete. Shell active.");
    shell::prompt();

    halt_loop();
}

pub fn halt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("KERNEL PANIC: {}", info);
    klog_println!("KERNEL PANIC: {}", info);
    println!("\nKERNEL PANIC: {}", info);

    halt_loop();
}
