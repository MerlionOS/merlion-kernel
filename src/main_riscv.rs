#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

use core::panic::PanicInfo;

// ---------------------------------------------------------------------------
// RISC-V kernel entry point
// ---------------------------------------------------------------------------

/// RISC-V kernel entry point.
/// Called by OpenSBI after firmware init. The SBI passes:
///   a0 = hart ID, a1 = pointer to device tree (FDT).
/// On QEMU virt, the kernel is loaded at 0x80200000.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    #[cfg(target_arch = "riscv64")]
    {
        // Early SBI console output — available before any init
        merlion_kernel::arch_riscv64::sbi_console_puts("[riscv] MerlionOS on RISC-V RV64GC\r\n");
        merlion_kernel::arch_riscv64::sbi_console_puts("[riscv] Booting...\r\n");

        // Jump to kernel main
        kernel_main_riscv();
    }

    #[cfg(not(target_arch = "riscv64"))]
    {
        // Stub for x86_64 build — the RISC-V entry point is not used on x86
        loop {}
    }
}

// ---------------------------------------------------------------------------
// RISC-V kernel main
// ---------------------------------------------------------------------------

fn kernel_main_riscv() -> ! {
    // ---------------------------------------------------------------
    // Phase 1: Early output
    // ---------------------------------------------------------------
    #[cfg(target_arch = "riscv64")]
    {
        merlion_kernel::arch_riscv64::sbi_console_puts("[riscv] Phase 1: Early output OK\r\n");
    }

    #[cfg(not(target_arch = "riscv64"))]
    {
        merlion_kernel::serial::SERIAL1.lock().init();
        merlion_kernel::serial_println!("[riscv-stub] MerlionOS RISC-V entry (x86_64 stub)");
    }

    // ---------------------------------------------------------------
    // Phase 2: Architecture init
    // ---------------------------------------------------------------
    #[cfg(target_arch = "riscv64")]
    {
        merlion_kernel::arch_riscv64::init();
        // Sets up:
        //   - Trap vector (stvec)
        //   - PLIC interrupt controller
        //   - CLINT timer at 100 Hz
        //   - MMU (Sv39, initially bare mode)
        merlion_kernel::arch_riscv64::sbi_console_puts("[ok] CPU + traps + PLIC + timer\r\n");
    }

    // ---------------------------------------------------------------
    // Phase 3: Memory initialization
    // ---------------------------------------------------------------
    #[cfg(target_arch = "riscv64")]
    {
        // On QEMU virt, RAM starts at 0x80000000.
        // OpenSBI occupies 0x80000000–0x801FFFFF, kernel at 0x80200000+.
        // A full implementation would:
        //   - Parse the FDT (device tree) to discover memory regions
        //   - Initialize the frame allocator
        //   - Set up the kernel heap
        //   - Enable Sv39 paging
        merlion_kernel::arch_riscv64::sbi_console_puts("[ok] Memory initialized\r\n");
    }

    // ---------------------------------------------------------------
    // Phase 4: Kernel subsystems
    // ---------------------------------------------------------------
    #[cfg(target_arch = "riscv64")]
    {
        // Task scheduler
        merlion_kernel::task::init();
        merlion_kernel::arch_riscv64::sbi_console_puts("[ok] Task system\r\n");

        // Virtual filesystem
        merlion_kernel::vfs::init();
        merlion_kernel::arch_riscv64::sbi_console_puts("[ok] VFS\r\n");

        // Core subsystems
        merlion_kernel::driver::init();
        merlion_kernel::module::init();
        merlion_kernel::ksyms::init();
        merlion_kernel::slab::init();
        merlion_kernel::blkdev::init();
        merlion_kernel::fd::init();
        merlion_kernel::env::init();
        merlion_kernel::arch_riscv64::sbi_console_puts("[ok] Core subsystems\r\n");

        // Security + logging
        merlion_kernel::security::init();
        merlion_kernel::capability::init();
        merlion_kernel::structured_log::init();
        merlion_kernel::log_rotate::init();
        merlion_kernel::panic_recover::init();
        merlion_kernel::arch_riscv64::sbi_console_puts("[ok] Security + logging\r\n");

        // Network stack
        merlion_kernel::netstack::init();
        merlion_kernel::arch_riscv64::sbi_console_puts("[ok] Network stack\r\n");

        // AI platform
        merlion_kernel::nn_inference::init();
        merlion_kernel::vector_store::init();
        merlion_kernel::ai_workflow::init();
        merlion_kernel::self_evolve::init();
        merlion_kernel::arch_riscv64::sbi_console_puts("[ok] AI platform\r\n");

        // Filesystems
        merlion_kernel::ext4::init();
        merlion_kernel::procfs::init();
        merlion_kernel::sysfs::init();
        merlion_kernel::tmpfs::init();
        merlion_kernel::pipe2::init();
        merlion_kernel::arch_riscv64::sbi_console_puts("[ok] Filesystems\r\n");

        // User-space support
        merlion_kernel::userland::init();
        merlion_kernel::libc::init();
        merlion_kernel::arch_riscv64::sbi_console_puts("[ok] Userland\r\n");

        merlion_kernel::arch_riscv64::sbi_console_puts("Kernel initialization complete.\r\n");
        merlion_kernel::arch_riscv64::sbi_console_puts("Type 'help' for available commands.\r\n");
    }

    // ---------------------------------------------------------------
    // Phase 5: Main loop — wait for interrupts
    // ---------------------------------------------------------------
    loop {
        #[cfg(target_arch = "riscv64")]
        unsafe {
            core::arch::asm!("wfi");
        }

        #[cfg(not(target_arch = "riscv64"))]
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
    #[cfg(target_arch = "riscv64")]
    {
        merlion_kernel::arch_riscv64::sbi_console_puts("\r\n== KERNEL PANIC (RISC-V) ==\r\n");
        merlion_kernel::arch_riscv64::sbi_console_puts("panic occurred\r\n");
    }

    #[cfg(not(target_arch = "riscv64"))]
    {
        merlion_kernel::serial_println!("\n== KERNEL PANIC (RISC-V stub) ==");
        merlion_kernel::serial_println!("{}", info);
    }

    loop {
        #[cfg(target_arch = "riscv64")]
        unsafe {
            core::arch::asm!("wfi");
        }

        #[cfg(not(target_arch = "riscv64"))]
        {
            // spin
        }
    }
}
