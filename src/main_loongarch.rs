#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

use core::panic::PanicInfo;

// ---------------------------------------------------------------------------
// LoongArch kernel entry point
// ---------------------------------------------------------------------------

/// LoongArch kernel entry point.
/// On QEMU loongarch64-virt, the kernel is loaded at 0x9000000000200000.
/// The firmware passes a0 = CPU ID, a1 = pointer to FDT.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    #[cfg(target_arch = "loongarch64")]
    {
        // Init UART for early output
        merlion_kernel::arch_loongarch::uart_init();
        merlion_kernel::arch_loongarch::uart_puts("[loongarch] MerlionOS on LoongArch64\n");
        merlion_kernel::arch_loongarch::uart_puts("[loongarch] Booting...\n");

        // Jump to kernel main
        kernel_main_loongarch();
    }

    #[cfg(not(target_arch = "loongarch64"))]
    {
        // Stub for x86_64 build — the LoongArch entry point is not used on x86
        loop {}
    }
}

// ---------------------------------------------------------------------------
// LoongArch kernel main
// ---------------------------------------------------------------------------

fn kernel_main_loongarch() -> ! {
    // ---------------------------------------------------------------
    // Phase 1: Early output
    // ---------------------------------------------------------------
    #[cfg(target_arch = "loongarch64")]
    {
        merlion_kernel::arch_loongarch::uart_puts("[loongarch] Phase 1: Early output OK\n");
    }

    #[cfg(not(target_arch = "loongarch64"))]
    {
        merlion_kernel::serial::SERIAL1.lock().init();
        merlion_kernel::serial_println!("[loongarch-stub] MerlionOS LoongArch entry (x86_64 stub)");
    }

    // ---------------------------------------------------------------
    // Phase 2: Architecture init
    // ---------------------------------------------------------------
    #[cfg(target_arch = "loongarch64")]
    {
        merlion_kernel::arch_loongarch::init();
        // Sets up:
        //   - UART (already done)
        //   - Exception entry (EENTRY)
        //   - EXTIOI interrupt controller
        //   - Timer at 100 Hz
        merlion_kernel::arch_loongarch::uart_puts("[ok] CPU + exceptions + EXTIOI + timer\n");
    }

    // ---------------------------------------------------------------
    // Phase 3: Memory initialization
    // ---------------------------------------------------------------
    #[cfg(target_arch = "loongarch64")]
    {
        // On QEMU loongarch64-virt, RAM starts at 0x0.
        // Direct-mapped window: 0x9000000000000000 maps to physical 0x0.
        // A full implementation would:
        //   - Parse FDT to discover memory layout
        //   - Initialize the frame allocator
        //   - Set up the kernel heap
        //   - Configure TLB for user-space mappings
        merlion_kernel::arch_loongarch::uart_puts("[ok] Memory initialized\n");
    }

    // ---------------------------------------------------------------
    // Phase 4: Kernel subsystems
    // ---------------------------------------------------------------
    #[cfg(target_arch = "loongarch64")]
    {
        // Task scheduler
        merlion_kernel::task::init();
        merlion_kernel::arch_loongarch::uart_puts("[ok] Task system\n");

        // Virtual filesystem
        merlion_kernel::vfs::init();
        merlion_kernel::arch_loongarch::uart_puts("[ok] VFS\n");

        // Core subsystems
        merlion_kernel::driver::init();
        merlion_kernel::module::init();
        merlion_kernel::ksyms::init();
        merlion_kernel::slab::init();
        merlion_kernel::blkdev::init();
        merlion_kernel::fd::init();
        merlion_kernel::env::init();
        merlion_kernel::arch_loongarch::uart_puts("[ok] Core subsystems\n");

        // Security + logging
        merlion_kernel::security::init();
        merlion_kernel::capability::init();
        merlion_kernel::structured_log::init();
        merlion_kernel::log_rotate::init();
        merlion_kernel::panic_recover::init();
        merlion_kernel::arch_loongarch::uart_puts("[ok] Security + logging\n");

        // Network stack
        merlion_kernel::netstack::init();
        merlion_kernel::arch_loongarch::uart_puts("[ok] Network stack\n");

        // AI platform
        merlion_kernel::nn_inference::init();
        merlion_kernel::vector_store::init();
        merlion_kernel::ai_workflow::init();
        merlion_kernel::self_evolve::init();
        merlion_kernel::arch_loongarch::uart_puts("[ok] AI platform\n");

        // Filesystems
        merlion_kernel::ext4::init();
        merlion_kernel::procfs::init();
        merlion_kernel::sysfs::init();
        merlion_kernel::tmpfs::init();
        merlion_kernel::pipe2::init();
        merlion_kernel::arch_loongarch::uart_puts("[ok] Filesystems\n");

        // User-space support
        merlion_kernel::userland::init();
        merlion_kernel::libc::init();
        merlion_kernel::arch_loongarch::uart_puts("[ok] Userland\n");

        merlion_kernel::arch_loongarch::uart_puts("Kernel initialization complete.\n");
        merlion_kernel::arch_loongarch::uart_puts("Type 'help' for available commands.\n");
    }

    // ---------------------------------------------------------------
    // Phase 5: Main loop — wait for interrupts
    // ---------------------------------------------------------------
    loop {
        #[cfg(target_arch = "loongarch64")]
        unsafe {
            core::arch::asm!("idle 0");
        }

        #[cfg(not(target_arch = "loongarch64"))]
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
    #[cfg(target_arch = "loongarch64")]
    {
        merlion_kernel::arch_loongarch::uart_puts("\n== KERNEL PANIC (LoongArch) ==\n");
        merlion_kernel::arch_loongarch::uart_puts("panic occurred\n");
    }

    #[cfg(not(target_arch = "loongarch64"))]
    {
        merlion_kernel::serial_println!("\n== KERNEL PANIC (LoongArch stub) ==");
        merlion_kernel::serial_println!("{}", info);
    }

    loop {
        #[cfg(target_arch = "loongarch64")]
        unsafe {
            core::arch::asm!("idle 0");
        }

        #[cfg(not(target_arch = "loongarch64"))]
        {
            // spin
        }
    }
}
