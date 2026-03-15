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
mod serial;
mod shell;
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
    klog_println!("[ok] GDT loaded (kernel + user segments)");
    println!("[ok] GDT loaded");
    serial_println!("[ok] GDT loaded");

    timer::init();
    klog_println!("[ok] PIT configured at {} Hz", timer::PIT_FREQUENCY_HZ);
    serial_println!("[ok] PIT configured at {} Hz", timer::PIT_FREQUENCY_HZ);

    interrupts::init();
    println!("[ok] IDT + interrupts enabled");
    klog_println!("[ok] IDT loaded, interrupts enabled");
    serial_println!("[ok] IDT loaded, interrupts enabled");

    let phys_mem_offset = x86_64::VirtAddr::new(boot_info.physical_memory_offset);
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe {
        memory::BootInfoFrameAllocator::init(&boot_info.memory_map)
    };
    println!("[ok] Memory initialized");
    klog_println!("[ok] Page table and frame allocator initialized");
    serial_println!("[ok] Page table and frame allocator initialized");

    allocator::init(&mut mapper, &mut frame_allocator)
        .expect("heap initialization failed");
    println!("[ok] Heap ready ({}K)", allocator::HEAP_SIZE / 1024);
    klog_println!("[ok] Heap allocator initialized ({}K)", allocator::HEAP_SIZE / 1024);
    serial_println!("[ok] Heap allocator initialized ({}K)", allocator::HEAP_SIZE / 1024);

    println!();
    println!("Type 'help' for available commands.");
    klog_println!("Kernel initialization complete.");
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
