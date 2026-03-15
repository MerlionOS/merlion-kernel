#![no_std]
#![no_main]
extern crate alloc;

use core::panic::PanicInfo;
use bootloader::{entry_point, BootInfo};
use merlion_kernel::*;

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    serial::SERIAL1.lock().init();
    serial_println!("{}", version::banner());
    serial_println!("Booting...");
    klog_println!("{} booting...", version::full());

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

    module::init();
    println!("[ok] Modules registered");
    serial_println!("[ok] Modules registered");

    ksyms::init();
    slab::init();
    blkdev::init();
    fd::init();
    println!("[ok] Slab caches ready");
    serial_println!("[ok] Kernel symbols + slab allocator initialized");

    env::init();

    smp::init();
    apic_timer::init();
    virtio_blk::init();
    virtio_net::init();
    ahci::init();
    nvme::init();
    xhci::init();
    e1000e::init();
    netstack::init();
    usb_hid::init();
    semfs::init();
    ai_proxy::init();
    agent::init();
    kconfig::load();
    script::create_default_init();
    println!("[ok] AI subsystem ready");
    serial_println!("[ok] AI agents + proxy initialized");
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
    // Disable interrupts to prevent further damage
    x86_64::instructions::interrupts::disable();

    serial_println!("\n══════════════════════════════════════");
    serial_println!("KERNEL PANIC: {}", info);

    // AI-assisted diagnosis
    let msg = alloc::format!("{}", info);
    let (category, _) = ai_syscall::classify(&msg);
    serial_println!("[ai-diagnosis] Category: {}", category);

    if msg.contains("page fault") {
        serial_println!("[ai-diagnosis] Likely cause: invalid memory access");
        serial_println!("[ai-diagnosis] Check for null pointers or stack overflow");
    } else if msg.contains("double fault") {
        serial_println!("[ai-diagnosis] Likely cause: unhandled exception during exception handling");
        serial_println!("[ai-diagnosis] Stack overflow is the most common trigger");
    } else if msg.contains("alloc") || msg.contains("heap") {
        serial_println!("[ai-diagnosis] Likely cause: out of memory");
        serial_println!("[ai-diagnosis] Heap size: {}K", allocator::HEAP_SIZE / 1024);
    }

    serial_println!("══════════════════════════════════════");

    // VGA output (simpler, no alloc)
    println!("\n\x1b[31m══ KERNEL PANIC ══\x1b[0m");
    println!("{}", info);
    println!("\x1b[33m[ai] category: {}\x1b[0m", category);
    println!("\x1b[90mSystem halted. Reboot with 'reboot' is not possible.\x1b[0m");

    halt_loop();
}
