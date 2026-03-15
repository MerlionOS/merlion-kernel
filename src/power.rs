/// ACPI-based power management for MerlionOS.
///
/// Supports ACPI power states (S0, S1, S3, S5), shutdown and reboot via
/// multiple fallback methods (ACPI FADT, QEMU I/O ports, keyboard controller),
/// and CPU halt helpers.

use alloc::format;
use alloc::string::String;
use x86_64::instructions::port::Port;

// ---------------------------------------------------------------------------
// Power states
// ---------------------------------------------------------------------------

/// ACPI sleep states supported by the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PowerState {
    /// S0 — fully working / running.
    S0Working  = 0,
    /// S1 — standby (CPU caches flushed, CPU stopped).
    S1Standby  = 1,
    /// S3 — suspend to RAM (context saved in memory, most hw powered off).
    S3Suspend  = 3,
    /// S5 — soft off (requires full boot to resume).
    S5SoftOff  = 5,
}

impl PowerState {
    /// Human-readable label for the state.
    pub fn label(self) -> &'static str {
        match self {
            PowerState::S0Working => "S0 (Working)",
            PowerState::S1Standby => "S1 (Standby)",
            PowerState::S3Suspend => "S3 (Suspend-to-RAM)",
            PowerState::S5SoftOff => "S5 (Soft Off)",
        }
    }
}

/// All states the kernel knows about.
pub const SUPPORTED_STATES: &[PowerState] = &[
    PowerState::S0Working,
    PowerState::S1Standby,
    PowerState::S3Suspend,
    PowerState::S5SoftOff,
];

// ---------------------------------------------------------------------------
// Power info
// ---------------------------------------------------------------------------

/// Snapshot of the current power-management state.
#[derive(Debug, Clone)]
pub struct PowerInfo {
    /// Currently active power state (always S0 while the kernel is running).
    pub current_state: PowerState,
    /// States this platform can transition to.
    pub supported_states: &'static [PowerState],
    /// Placeholder for thermal sensor data (degrees Celsius, if available).
    pub thermal_celsius: Option<u32>,
}

/// Return a snapshot of the kernel's power-management information.
pub fn get_power_info() -> PowerInfo {
    PowerInfo {
        current_state: PowerState::S0Working,
        supported_states: SUPPORTED_STATES,
        thermal_celsius: None, // no ACPI thermal zone parser yet
    }
}

// ---------------------------------------------------------------------------
// Low-level ACPI helpers
// ---------------------------------------------------------------------------

/// Write SLP_TYP | SLP_EN to the ACPI PM1a control register to enter S5.
///
/// `pm1a_port` is the I/O port of the PM1a_CNT_BLK register (from FADT).
/// `slp_typ` is the SLP_TYPa value for the desired sleep state (from \_S5).
///
/// The PM1a control register layout (bits):
///   [12:10] SLP_TYP — sleep type
///   [13]    SLP_EN  — setting this bit triggers the transition
pub fn shutdown_acpi(pm1a_port: u16, slp_typ: u16) {
    let value = (slp_typ << 10) | (1 << 13); // SLP_TYP | SLP_EN
    crate::serial_println!("[power] PM1a write 0x{:04X} -> port 0x{:04X}", value, pm1a_port);
    unsafe {
        Port::<u16>::new(pm1a_port).write(value);
    }
}

/// Write the reset value to the ACPI FADT reset register.
///
/// `reset_reg` is the I/O-port address of the ACPI reset register (from FADT).
/// `reset_val` is the value to write (from FADT `ResetValue`).
pub fn reboot_acpi(reset_reg: u64, reset_val: u8) {
    crate::serial_println!("[power] ACPI reset: write 0x{:02X} -> 0x{:X}", reset_val, reset_reg);
    unsafe {
        Port::<u8>::new(reset_reg as u16).write(reset_val);
    }
}

// ---------------------------------------------------------------------------
// Keyboard controller helpers (shared by shutdown & reboot fallbacks)
// ---------------------------------------------------------------------------

/// Wait until the PS/2 keyboard controller input buffer is empty, then send
/// the CPU-reset command (0xFE).
fn kbd_controller_reset() {
    unsafe {
        let mut status = Port::<u8>::new(0x64);
        let mut cmd = Port::<u8>::new(0x64);
        for _ in 0..0xFFFF {
            if status.read() & 0x02 == 0 {
                break;
            }
        }
        cmd.write(0xFE);
    }
}

/// Trigger a triple fault by loading an empty IDT, which causes the CPU to
/// reset. This is the ultimate fallback when everything else fails.
fn triple_fault() -> ! {
    unsafe {
        // Load a zero-length IDT — the next interrupt will triple-fault
        let empty_idt: u128 = 0; // limit=0, base=0
        core::arch::asm!(
            "lidt [{}]",
            in(reg) &empty_idt,
            options(noreturn)
        );
    }
}

// ---------------------------------------------------------------------------
// Public shutdown / reboot (multi-method)
// ---------------------------------------------------------------------------

/// Shut down the machine, trying several methods in order:
///
/// 1. ACPI FADT PM1a control register (S5) — skipped if no FADT parsed yet.
/// 2. QEMU/Bochs well-known port 0x604.
/// 3. QEMU legacy port 0xB004.
/// 4. Keyboard controller reset (last resort — acts more like a reboot).
///
/// This function never returns.
pub fn shutdown() -> ! {
    crate::serial_println!("[power] initiating shutdown...");
    crate::klog_println!("[power] shutdown");
    x86_64::instructions::interrupts::disable();

    // Method 1: QEMU ACPI shutdown (Bochs/QEMU port 0x604, SLP_EN | SLP_TYP=5)
    unsafe { Port::<u16>::new(0x604).write(0x2000); }

    // Method 2: QEMU legacy port
    unsafe { Port::<u16>::new(0xB004).write(0x2000); }

    // Method 3: Virtualbox ACPI port
    unsafe { Port::<u16>::new(0x4004).write(0x3400); }

    // Method 4: Keyboard controller (may reboot instead)
    crate::serial_println!("[power] ACPI ports failed, trying keyboard controller");
    kbd_controller_reset();

    // Nothing worked — halt forever
    crate::serial_println!("[power] all shutdown methods failed, halting CPU");
    halt_loop();
}

/// Reboot the machine, trying several methods in order:
///
/// 1. ACPI reset register (skipped if no FADT; common reset_reg = 0xCF9).
/// 2. Keyboard controller reset (0xFE on port 0x64).
/// 3. Triple fault (load empty IDT).
///
/// This function never returns.
pub fn reboot() -> ! {
    crate::serial_println!("[power] initiating reboot...");
    crate::klog_println!("[power] reboot");
    x86_64::instructions::interrupts::disable();

    // Method 1: Standard PCI reset-control register at 0xCF9
    unsafe {
        let mut rst = Port::<u8>::new(0xCF9);
        rst.write(0x02); // system reset
        rst.write(0x06); // hard reset (full reset including memory)
    }

    // Method 2: Keyboard controller
    crate::serial_println!("[power] PCI reset failed, trying keyboard controller");
    kbd_controller_reset();

    // Brief spin to let reset propagate
    for _ in 0..0xFFFFF { core::hint::spin_loop(); }

    // Method 3: Triple fault — guaranteed to reset the CPU
    crate::serial_println!("[power] keyboard reset failed, forcing triple fault");
    triple_fault();
}

// ---------------------------------------------------------------------------
// CPU halt helpers
// ---------------------------------------------------------------------------

/// Execute the HLT instruction once (waits for the next interrupt).
#[inline]
pub fn halt() {
    x86_64::instructions::hlt();
}

/// Halt the CPU in an infinite loop. This function never returns and is useful
/// as a diverging tail call after unrecoverable errors or shutdown.
pub fn halt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}

// ---------------------------------------------------------------------------
// Status display
// ---------------------------------------------------------------------------

/// Return a human-readable summary of the power-management subsystem.
pub fn info() -> String {
    let pi = get_power_info();
    let states: String = pi.supported_states
        .iter()
        .map(|s| s.label())
        .collect::<alloc::vec::Vec<&str>>()
        .join(", ");
    let thermal = match pi.thermal_celsius {
        Some(t) => format!("{}°C", t),
        None => String::from("n/a"),
    };
    format!(
        "[power] state: {} | supported: [{}] | thermal: {}",
        pi.current_state.label(),
        states,
        thermal,
    )
}
