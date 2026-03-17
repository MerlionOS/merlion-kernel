#![no_std]
#![no_main]

extern crate alloc;

use core::panic::PanicInfo;

// ---------------------------------------------------------------------------
// Raspberry Pi kernel entry point
// ---------------------------------------------------------------------------

/// Raspberry Pi kernel entry point.
/// Called after the Pi firmware loads kernel8.img to 0x80000.
/// At this point we're in EL2 (or EL1), with MMU off, caches off.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // This would be the real entry on aarch64
    // On x86_64 build, this is just a stub
    #[cfg(target_arch = "aarch64")]
    unsafe {
        // Park secondary cores — only core 0 proceeds
        // core::arch::asm!(
        //     "mrs x0, mpidr_el1",
        //     "and x0, x0, #3",
        //     "cbnz x0, .Lpark",
        // );

        // Set stack pointer for boot core
        // core::arch::asm!("mov sp, #0x80000");

        // Clear BSS section
        // core::arch::asm!(
        //     "ldr x0, =__bss_start",
        //     "ldr x1, =__bss_end",
        //     ".Lbss_loop:",
        //     "cmp x0, x1",
        //     "b.ge .Lbss_done",
        //     "str xzr, [x0], #8",
        //     "b .Lbss_loop",
        //     ".Lbss_done:",
        // );

        // Jump to kernel main
        kernel_main_pi();

        // Secondary cores park here
        // .Lpark:
        // core::arch::asm!("wfe", "b .Lpark");
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        // Stub for x86_64 build — the Pi entry point is not used on x86
        loop {}
    }
}

// ---------------------------------------------------------------------------
// Pi kernel main (architecture-independent logic with cfg guards)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn kernel_main_pi() -> ! {
    // ---------------------------------------------------------------
    // Phase 1: Early UART output
    // ---------------------------------------------------------------
    // On a real Pi, we'd init the PL011 UART here:
    //   merlion_kernel::uart_pl011::init();
    //   merlion_kernel::uart_pl011::puts("[pi] MerlionOS on Raspberry Pi!\r\n");
    //
    // For now, use serial on x86 or uart_pl011 on aarch64:
    #[cfg(target_arch = "aarch64")]
    {
        // TODO: init PL011 UART at 0x3F201000 (Pi 3) or 0xFE201000 (Pi 4)
        // merlion_kernel::uart_pl011::init();
        // merlion_kernel::uart_pl011::puts("[pi] MerlionOS on Raspberry Pi!\r\n");
        // merlion_kernel::uart_pl011::puts("[pi] Booting...\r\n");
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        // On x86_64, use serial for diagnostic output
        merlion_kernel::serial::SERIAL1.lock().init();
        merlion_kernel::serial_println!("[pi-stub] MerlionOS Pi entry (x86_64 stub)");
    }

    // ---------------------------------------------------------------
    // Phase 2: Architecture init
    // ---------------------------------------------------------------
    #[cfg(target_arch = "aarch64")]
    {
        // merlion_kernel::arch_aarch64::init();
        // Sets up:
        //   - Exception vectors (EL1)
        //   - System timer
        //   - MMU with identity mapping
        // merlion_kernel::uart_pl011::puts("[ok] CPU + exceptions + timer\r\n");
    }

    // ---------------------------------------------------------------
    // Phase 3: Memory initialization
    // ---------------------------------------------------------------
    #[cfg(target_arch = "aarch64")]
    {
        // Query VideoCore for ARM memory region via mailbox
        // let (mem_base, mem_size) = merlion_kernel::pi_mailbox::get_arm_memory();
        //
        // Initialize frame allocator with discovered memory
        // let frame_start = mem_base + kernel_end_aligned;
        // let frame_end = mem_base + mem_size;
        // merlion_kernel::memory::init_frame_allocator(frame_start, frame_end);
        //
        // Initialize kernel heap
        // merlion_kernel::allocator::init_pi_heap();
        //
        // merlion_kernel::uart_pl011::puts("[ok] Memory initialized\r\n");
    }

    // ---------------------------------------------------------------
    // Phase 4: Kernel subsystems (reuse all platform-independent modules)
    // ---------------------------------------------------------------
    // These modules are architecture-independent and work on both
    // x86_64 and aarch64 once memory allocation is available.

    #[cfg(target_arch = "aarch64")]
    {
        // Task scheduler
        merlion_kernel::task::init();
        // merlion_kernel::uart_pl011::puts("[ok] Task system\r\n");

        // Virtual filesystem
        merlion_kernel::vfs::init();
        // merlion_kernel::uart_pl011::puts("[ok] VFS\r\n");

        // Core subsystems
        merlion_kernel::driver::init();
        merlion_kernel::module::init();
        merlion_kernel::ksyms::init();
        merlion_kernel::slab::init();
        merlion_kernel::blkdev::init();
        merlion_kernel::fd::init();
        merlion_kernel::env::init();
        // merlion_kernel::uart_pl011::puts("[ok] Core subsystems\r\n");

        // Security + logging
        merlion_kernel::security::init();
        merlion_kernel::capability::init();
        merlion_kernel::structured_log::init();
        merlion_kernel::log_rotate::init();
        merlion_kernel::panic_recover::init();
        // merlion_kernel::uart_pl011::puts("[ok] Security + logging\r\n");

        // Network (platform-independent parts)
        merlion_kernel::netstack::init();
        // merlion_kernel::uart_pl011::puts("[ok] Network stack\r\n");

        // AI platform
        merlion_kernel::nn_inference::init();
        merlion_kernel::vector_store::init();
        merlion_kernel::ai_workflow::init();
        merlion_kernel::self_evolve::init();
        // merlion_kernel::uart_pl011::puts("[ok] AI platform\r\n");

        // Filesystems
        merlion_kernel::ext4::init();
        merlion_kernel::procfs::init();
        merlion_kernel::sysfs::init();
        merlion_kernel::tmpfs::init();
        merlion_kernel::pipe2::init();
        // merlion_kernel::uart_pl011::puts("[ok] Filesystems\r\n");

        // User-space support
        merlion_kernel::userland::init();
        merlion_kernel::libc::init();
        // merlion_kernel::uart_pl011::puts("[ok] Userland\r\n");

        // merlion_kernel::uart_pl011::puts("Kernel initialization complete.\r\n");
        // merlion_kernel::uart_pl011::puts("Type 'help' for available commands.\r\n");
    }

    // ---------------------------------------------------------------
    // Phase 5: Main loop — read from UART, feed to shell
    // ---------------------------------------------------------------
    loop {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            // Wait for event (interrupt) — saves power on ARM
            core::arch::asm!("wfe");
        }

        #[cfg(not(target_arch = "aarch64"))]
        {
            // x86_64 stub — just spin
        }
    }
}

// ---------------------------------------------------------------------------
// Panic handler
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // On aarch64: write to UART
    #[cfg(target_arch = "aarch64")]
    {
        // merlion_kernel::uart_pl011::puts("\n== KERNEL PANIC ==\n");
        // Can't easily format on bare metal without alloc, but we try:
        // merlion_kernel::uart_pl011::puts("panic occurred\r\n");
    }

    // On x86_64: write to serial
    #[cfg(not(target_arch = "aarch64"))]
    {
        merlion_kernel::serial_println!("\n== KERNEL PANIC (Pi stub) ==");
        merlion_kernel::serial_println!("{}", info);
    }

    loop {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("wfe");
        }

        #[cfg(not(target_arch = "aarch64"))]
        {
            // spin
        }
    }
}
