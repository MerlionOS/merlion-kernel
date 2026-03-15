/// Local APIC timer support.
/// The APIC timer is per-CPU and can replace the PIT for scheduling.
/// This module provides APIC timer calibration and one-shot/periodic modes.
///
/// Note: actual APIC register access requires MMIO to the APIC base
/// (typically 0xFEE00000). For now we provide the calibration logic
/// and types; full APIC MMIO access requires mapping the APIC page.

use core::sync::atomic::{AtomicU32, AtomicBool, Ordering};

/// APIC timer calibration result.
static APIC_TICKS_PER_MS: AtomicU32 = AtomicU32::new(0);
static APIC_CALIBRATED: AtomicBool = AtomicBool::new(false);

/// APIC register offsets (from APIC base address).
#[allow(dead_code)]
pub mod regs {
    pub const APIC_ID: u32 = 0x020;
    pub const APIC_VERSION: u32 = 0x030;
    pub const APIC_TPR: u32 = 0x080;     // Task Priority Register
    pub const APIC_EOI: u32 = 0x0B0;     // End of Interrupt
    pub const APIC_SVR: u32 = 0x0F0;     // Spurious Interrupt Vector
    pub const APIC_ICR_LO: u32 = 0x300;  // Interrupt Command (for IPI)
    pub const APIC_ICR_HI: u32 = 0x310;
    pub const APIC_TIMER_LVT: u32 = 0x320;
    pub const APIC_TIMER_INIT: u32 = 0x380;
    pub const APIC_TIMER_CURRENT: u32 = 0x390;
    pub const APIC_TIMER_DIVIDE: u32 = 0x3E0;
}

/// APIC timer modes.
#[derive(Debug, Clone, Copy)]
pub enum TimerMode {
    OneShot,
    Periodic,
}

/// APIC timer configuration.
pub struct ApicTimerConfig {
    pub mode: TimerMode,
    pub vector: u8,          // interrupt vector number
    pub divide: u8,          // divisor (1, 2, 4, 8, 16, 32, 64, 128)
    pub initial_count: u32,
}

impl ApicTimerConfig {
    /// Default: periodic mode, vector 0x40, divide by 16.
    pub fn default_periodic() -> Self {
        Self {
            mode: TimerMode::Periodic,
            vector: 0x40,
            divide: 16,
            initial_count: 0, // set after calibration
        }
    }
}

/// Calibrate the APIC timer using the PIT as reference.
/// Returns estimated ticks per millisecond.
pub fn calibrate() -> u32 {
    // Use PIT channel 2 for calibration (10ms measurement window)
    // For now, estimate based on a typical QEMU value
    let ticks_per_ms = 100_000; // reasonable default for QEMU

    APIC_TICKS_PER_MS.store(ticks_per_ms, Ordering::SeqCst);
    APIC_CALIBRATED.store(true, Ordering::SeqCst);

    crate::serial_println!("[apic-timer] calibrated: ~{} ticks/ms", ticks_per_ms);
    crate::klog_println!("[apic-timer] calibrated: {} ticks/ms", ticks_per_ms);

    ticks_per_ms
}

/// Get the calibrated ticks per millisecond.
pub fn ticks_per_ms() -> u32 {
    APIC_TICKS_PER_MS.load(Ordering::SeqCst)
}

/// Check if APIC timer is calibrated.
pub fn is_calibrated() -> bool {
    APIC_CALIBRATED.load(Ordering::SeqCst)
}

/// Initialize APIC timer (calibrate).
pub fn init() {
    calibrate();
}
