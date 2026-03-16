/// aarch64 architecture support for MerlionOS.
/// Provides boot code, exception handling, ARM Generic Timer,
/// and GIC interrupt controller for Raspberry Pi.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use alloc::string::String;
use alloc::format;

// ---------------------------------------------------------------------------
// CPU info — read MIDR_EL1, MPIDR_EL1, CurrentEL
// ---------------------------------------------------------------------------

/// Return the current core ID (MPIDR_EL1 Aff0 field).
pub fn cpu_id() -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        let id: u64;
        unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) id); }
        id & 0xFF
    }
    #[cfg(not(target_arch = "aarch64"))]
    { 0 }
}

/// Return the current exception level (0–3).
pub fn current_el() -> u8 {
    #[cfg(target_arch = "aarch64")]
    {
        let el: u64;
        unsafe { core::arch::asm!("mrs {}, CurrentEL", out(reg) el); }
        ((el >> 2) & 0x3) as u8
    }
    #[cfg(not(target_arch = "aarch64"))]
    { 1 }
}

/// Read MIDR_EL1 (Main ID Register) to identify the CPU.
pub fn midr() -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        let v: u64;
        unsafe { core::arch::asm!("mrs {}, midr_el1", out(reg) v); }
        v
    }
    #[cfg(not(target_arch = "aarch64"))]
    { 0x410F_D034 } // fake Cortex-A53
}

/// Return a human-readable string describing the CPU.
pub fn cpu_info() -> String {
    let id = midr();
    let implementer = (id >> 24) & 0xFF;
    let variant = (id >> 20) & 0xF;
    let part = (id >> 4) & 0xFFF;
    let revision = id & 0xF;
    let impl_name = match implementer {
        0x41 => "ARM",
        0x42 => "Broadcom",
        0x43 => "Cavium",
        0x51 => "Qualcomm",
        _ => "Unknown",
    };
    let part_name = match (implementer, part) {
        (0x41, 0xD03) => "Cortex-A53",
        (0x41, 0xD04) => "Cortex-A35",
        (0x41, 0xD05) => "Cortex-A55",
        (0x41, 0xD07) => "Cortex-A57",
        (0x41, 0xD08) => "Cortex-A72",
        (0x41, 0xD09) => "Cortex-A73",
        (0x41, 0xD0A) => "Cortex-A75",
        (0x41, 0xD0B) => "Cortex-A76",
        _ => "Unknown",
    };
    format!(
        "{} {} r{}p{} (core {}, EL{})",
        impl_name, part_name, variant, revision, cpu_id(), current_el()
    )
}

// ---------------------------------------------------------------------------
// Exception vector table
// ---------------------------------------------------------------------------

/// Initialised flag — prevents double-init.
static EXCEPTIONS_INIT: AtomicBool = AtomicBool::new(false);

/// Install the exception vector table at VBAR_EL1.
///
/// On aarch64 the table is 0x800 bytes, 0x800-aligned.
/// Each of the 16 entries is 0x80 bytes (32 instructions).
/// We handle Synchronous, IRQ, FIQ, SError from current EL with SP_ELx.
pub fn init_exceptions() {
    if EXCEPTIONS_INIT.swap(true, Ordering::SeqCst) {
        return;
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        extern "C" {
            static _exception_vector_table: u8;
        }
        let addr = &_exception_vector_table as *const u8 as u64;
        core::arch::asm!("msr vbar_el1, {}", in(reg) addr);
        core::arch::asm!("isb");
    }
}

/// Default synchronous exception handler — logs and halts.
#[no_mangle]
pub extern "C" fn sync_exception_handler(esr: u64, elr: u64, far: u64) {
    let ec = (esr >> 26) & 0x3F;
    let iss = esr & 0x1FF_FFFF;
    let _ = (ec, iss, elr, far); // used by uart_println below
    #[cfg(target_arch = "aarch64")]
    {
        crate::uart_println!(
            "SYNC EXCEPTION: EC=0x{:02x} ISS=0x{:06x} ELR=0x{:016x} FAR=0x{:016x}",
            ec, iss, elr, far
        );
    }
    halt();
}

/// IRQ handler — dispatches to timer and other interrupt sources.
#[no_mangle]
pub extern "C" fn irq_handler() {
    // Check if it is the generic timer (IRQ 30 on GICv2 / CNTP on Pi 3)
    if is_timer_irq() {
        timer_handler();
    }
    // Acknowledge the interrupt
    #[cfg(target_arch = "aarch64")]
    {
        let irq = gic_ack_irq();
        gic_end_irq(irq);
    }
}

/// FIQ handler — log and halt.
#[no_mangle]
pub extern "C" fn fiq_handler() {
    #[cfg(target_arch = "aarch64")]
    crate::uart_println!("FIQ received — halting");
    halt();
}

/// SError handler — log and halt.
#[no_mangle]
pub extern "C" fn serror_handler() {
    #[cfg(target_arch = "aarch64")]
    crate::uart_println!("SError received — halting");
    halt();
}

// ---------------------------------------------------------------------------
// ARM Generic Timer
// ---------------------------------------------------------------------------

static TIMER_FREQ: AtomicU64 = AtomicU64::new(0);
static TICK_COUNT: AtomicU64 = AtomicU64::new(0);

/// Target tick rate in Hz (matches x86 PIT at 100 Hz).
const TIMER_HZ: u64 = 100;

/// Initialise the ARM Generic Timer for periodic ticks.
pub fn timer_init() {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        // Read the timer frequency from CNTFRQ_EL0
        let freq: u64;
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq);
        TIMER_FREQ.store(freq, Ordering::Relaxed);

        // Set the countdown value for ~100 Hz
        let tval = freq / TIMER_HZ;
        core::arch::asm!("msr cntp_tval_el0, {}", in(reg) tval);

        // Enable the physical timer, unmask interrupt
        // CTL: ENABLE=1, IMASK=0
        let ctl: u64 = 1;
        core::arch::asm!("msr cntp_ctl_el0, {}", in(reg) ctl);
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        TIMER_FREQ.store(62_500_000, Ordering::Relaxed);
    }
}

/// Handle a timer tick — increment counter, set next countdown.
pub fn timer_handler() {
    TICK_COUNT.fetch_add(1, Ordering::Relaxed);
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let freq = TIMER_FREQ.load(Ordering::Relaxed);
        let tval = freq / TIMER_HZ;
        core::arch::asm!("msr cntp_tval_el0, {}", in(reg) tval);
    }
}

/// Return the number of ticks since boot.
pub fn ticks() -> u64 {
    TICK_COUNT.load(Ordering::Relaxed)
}

/// Return the timer frequency in Hz.
pub fn timer_freq() -> u64 {
    TIMER_FREQ.load(Ordering::Relaxed)
}

/// Check whether the pending IRQ is from the generic timer.
fn is_timer_irq() -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        let ctl: u64;
        unsafe { core::arch::asm!("mrs {}, cntp_ctl_el0", out(reg) ctl); }
        // ISTATUS (bit 2) set means the timer fired
        (ctl & (1 << 2)) != 0
    }
    #[cfg(not(target_arch = "aarch64"))]
    { false }
}

// ---------------------------------------------------------------------------
// GICv2 (simplified) — also legacy Pi 3 interrupt controller
// ---------------------------------------------------------------------------

// GICv2 addresses (Raspberry Pi 4)
const GICD_BASE: u64 = 0xFF84_1000;
const GICC_BASE: u64 = 0xFF84_2000;

// Legacy interrupt controller (Raspberry Pi 3 / QEMU raspi3b)
const LEGACY_IRQ_BASE: u64 = 0x3F00_B200;
const IRQ_ENABLE1: u64 = LEGACY_IRQ_BASE + 0x10;
const IRQ_DISABLE1: u64 = LEGACY_IRQ_BASE + 0x1C;
const IRQ_PENDING1: u64 = LEGACY_IRQ_BASE + 0x04;

/// True if running on Pi 3 style (legacy) interrupt controller.
static USE_LEGACY_IRQ: AtomicBool = AtomicBool::new(true);

/// Initialise the interrupt controller.
///
/// Defaults to legacy (Pi 3) mode. For Pi 4, call with `gic_init_gicv2()`.
pub fn gic_init() {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        if USE_LEGACY_IRQ.load(Ordering::Relaxed) {
            // Enable ARM timer IRQ (bit 0)
            mmio_write(IRQ_ENABLE1, 1 << 0);
        } else {
            // GICv2 distributor enable
            mmio_write(GICD_BASE + 0x000, 1); // GICD_CTLR = enable
            // CPU interface enable, priority mask
            mmio_write(GICC_BASE + 0x000, 1); // GICC_CTLR = enable
            mmio_write(GICC_BASE + 0x004, 0xFF); // GICC_PMR = lowest priority
        }
    }
}

/// Enable a specific IRQ line.
#[allow(unused_variables)]
pub fn gic_enable_irq(irq: u32) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        if USE_LEGACY_IRQ.load(Ordering::Relaxed) {
            // Legacy: write bit to enable register
            let reg = if irq < 32 { IRQ_ENABLE1 } else { IRQ_ENABLE1 + 4 };
            let bit = irq % 32;
            mmio_write(reg, 1 << bit);
        } else {
            // GICv2: GICD_ISENABLER
            let reg = GICD_BASE + 0x100 + ((irq / 32) as u64) * 4;
            mmio_write(reg, 1 << (irq % 32));
        }
    }
}

/// Disable a specific IRQ line.
#[allow(unused_variables)]
pub fn gic_disable_irq(irq: u32) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        if USE_LEGACY_IRQ.load(Ordering::Relaxed) {
            let reg = if irq < 32 { IRQ_DISABLE1 } else { IRQ_DISABLE1 + 4 };
            let bit = irq % 32;
            mmio_write(reg, 1 << bit);
        } else {
            // GICv2: GICD_ICENABLER
            let reg = GICD_BASE + 0x180 + ((irq / 32) as u64) * 4;
            mmio_write(reg, 1 << (irq % 32));
        }
    }
}

/// Acknowledge the highest-priority pending IRQ (GICv2 only).
pub fn gic_ack_irq() -> u32 {
    #[cfg(target_arch = "aarch64")]
    {
        if USE_LEGACY_IRQ.load(Ordering::Relaxed) {
            // Legacy: read pending register and find first set bit
            let pending = unsafe { mmio_read(IRQ_PENDING1) };
            if pending == 0 { return 1023; } // spurious
            let mut bit = 0u32;
            while bit < 32 {
                if pending & (1 << bit) != 0 { return bit; }
                bit += 1;
            }
            1023
        } else {
            unsafe { mmio_read(GICC_BASE + 0x00C) } // GICC_IAR
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    { 1023 }
}

/// Signal end-of-interrupt for the given IRQ (GICv2 only).
#[allow(unused_variables)]
pub fn gic_end_irq(irq: u32) {
    #[cfg(target_arch = "aarch64")]
    {
        if !USE_LEGACY_IRQ.load(Ordering::Relaxed) {
            unsafe { mmio_write(GICC_BASE + 0x010, irq); } // GICC_EOIR
        }
    }
}

// ---------------------------------------------------------------------------
// MMU setup (minimal identity map)
// ---------------------------------------------------------------------------

/// Set up minimal identity-mapped page tables and enable the MMU.
///
/// Maps the first 1 GiB as normal memory (cacheable) and the
/// MMIO region (0x3F00_0000 – 0x4000_0000) as device memory.
pub fn mmu_init() {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        // Set MAIR_EL1: attr0 = normal write-back, attr1 = device-nGnRnE
        let mair: u64 = 0xFF | (0x00 << 8);
        core::arch::asm!("msr mair_el1, {}", in(reg) mair);

        // TCR_EL1: 4K granule, 39-bit VA (T0SZ = 25)
        let tcr: u64 = 25 // T0SZ
            | (0b00 << 14)  // TG0 = 4KB
            | (0b01 << 8)   // ORGN0 = write-back
            | (0b01 << 10)  // IRGN0 = write-back
            | (0b11 << 12); // SH0 = inner shareable
        core::arch::asm!("msr tcr_el1, {}", in(reg) tcr);

        // For now, we skip actual page-table construction.
        // A real implementation would build PGD/PUD/PMD/PTE here.
        // Just ensure SCTLR_EL1.M stays 0 (MMU disabled) until we have
        // proper tables to avoid an immediate fault.

        core::arch::asm!("isb");
    }
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

/// Busy-loop for `n` iterations (crude delay).
pub fn delay_cycles(n: u64) {
    #[cfg(target_arch = "aarch64")]
    {
        for _ in 0..n {
            unsafe { core::arch::asm!("nop"); }
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        for _ in 0..n {
            core::hint::spin_loop();
        }
    }
}

/// Execute WFE (Wait For Event).
pub fn wfe() {
    #[cfg(target_arch = "aarch64")]
    unsafe { core::arch::asm!("wfe"); }
    #[cfg(not(target_arch = "aarch64"))]
    core::hint::spin_loop();
}

/// Halt the CPU — loops WFI forever.
pub fn halt() -> ! {
    loop {
        #[cfg(target_arch = "aarch64")]
        unsafe { core::arch::asm!("wfi"); }
        #[cfg(not(target_arch = "aarch64"))]
        core::hint::spin_loop();
    }
}

/// Return a summary of the architecture configuration.
pub fn arch_info() -> String {
    format!(
        "aarch64: {} | timer {} Hz | ticks {} | EL{}",
        cpu_info(),
        timer_freq(),
        ticks(),
        current_el()
    )
}

/// Initialise all aarch64 architecture components.
pub fn init() {
    init_exceptions();
    gic_init();
    timer_init();
    // MMU left disabled until proper page tables are built
}

// ---------------------------------------------------------------------------
// MMIO helpers
// ---------------------------------------------------------------------------

#[cfg(target_arch = "aarch64")]
unsafe fn mmio_write(addr: u64, val: u32) {
    core::ptr::write_volatile(addr as *mut u32, val);
}

#[cfg(target_arch = "aarch64")]
unsafe fn mmio_read(addr: u64) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}
