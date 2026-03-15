/// ACPI power management (QEMU-specific).
/// Provides shutdown and reboot via well-known I/O ports.

use x86_64::instructions::port::Port;

/// Shutdown the machine via QEMU's debug exit device or ACPI.
/// In QEMU, writing to port 0x604 triggers an ACPI shutdown.
/// The isa-debug-exit device at port 0xf4 is another option.
pub fn shutdown() -> ! {
    crate::serial_println!("[acpi] initiating shutdown...");
    crate::klog_println!("[acpi] shutdown");

    // Try QEMU ACPI shutdown (Bochs/QEMU port 0x604)
    unsafe {
        Port::<u16>::new(0x604).write(0x2000);
    }

    // Fallback: QEMU older versions use port 0xB004
    unsafe {
        Port::<u16>::new(0xB004).write(0x2000);
    }

    // If neither worked, halt
    crate::serial_println!("[acpi] shutdown failed, halting");
    loop {
        x86_64::instructions::hlt();
    }
}

/// Reboot the machine via the keyboard controller reset line.
/// Pulsing the CPU reset line via port 0x64 (PS/2 controller).
pub fn reboot() -> ! {
    crate::serial_println!("[acpi] initiating reboot...");
    crate::klog_println!("[acpi] reboot");

    unsafe {
        // Wait for keyboard controller input buffer to be empty
        let mut status = Port::<u8>::new(0x64);
        let mut cmd = Port::<u8>::new(0x64);

        // Disable interrupts
        x86_64::instructions::interrupts::disable();

        // Wait for controller
        loop {
            if status.read() & 0x02 == 0 {
                break;
            }
        }

        // Send reset command (0xFE = CPU reset)
        cmd.write(0xFE);
    }

    // If that didn't work, triple fault
    loop {
        x86_64::instructions::hlt();
    }
}
