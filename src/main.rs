#![no_std]
#![no_main]
extern crate alloc;

use core::panic::PanicInfo;
use bootloader::{entry_point, BootInfo};
use merlion_kernel::*;

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

    let phys_mem_offset = x86_64::VirtAddr::new(boot_info.physical_memory_offset);
    let mut mapper = unsafe { memory::init(phys_mem_offset, &boot_info.memory_map) };
    println!("[ok] Memory initialized");
    serial_println!("[ok] Page table and frame allocator initialized");

    memory::with_frame_allocator(|fa| {
        allocator::init(&mut mapper, fa)
            .expect("heap initialization failed");
    });
    println!("[ok] Heap ready ({}K)", allocator::HEAP_SIZE / 1024);
    serial_println!("[ok] Heap allocator initialized ({}K)", allocator::HEAP_SIZE / 1024);

    task::init();
    println!("[ok] Task system ready");
    serial_println!("[ok] Task system initialized");

    vfs::init();
    println!("[ok] VFS mounted");
    serial_println!("[ok] VFS initialized");

    driver::init();
    println!("[ok] Drivers registered");
    serial_println!("[ok] Drivers registered");

    env::init();

    smp::init();
    println!("[ok] SMP: {} CPU(s) online", smp::online_cpus());
    serial_println!("[ok] SMP initialized");

    // Show date/time from RTC
    let dt = rtc::read();
    println!("[ok] RTC: {}", dt);
    serial_println!("[ok] RTC: {}", dt);

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
