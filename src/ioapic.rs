/// I/O APIC driver for MerlionOS.
///
/// The I/O APIC receives external interrupt signals (keyboard, timer, PCI
/// devices, etc.) and routes them to one or more local APICs via redirection
/// table entries.  Each entry maps an IRQ pin to an interrupt vector and a
/// destination processor.
///
/// MMIO registers:
///   base + 0x00  IOREGSEL  — selects the indirect register index
///   base + 0x10  IOWIN     — read/write window for the selected register

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use alloc::format;
use alloc::string::String;

use crate::memory;
use x86_64::PhysAddr;

// ---------------------------------------------------------------------------
// MMIO layout
// ---------------------------------------------------------------------------

/// Offset of the register-select register from the I/O APIC base.
const IOREGSEL_OFFSET: u64 = 0x00;

/// Offset of the data window register from the I/O APIC base.
const IOWIN_OFFSET: u64 = 0x10;

// ---------------------------------------------------------------------------
// Indirect register indices
// ---------------------------------------------------------------------------

/// I/O APIC identification register.
const IOAPIC_REG_ID: u32 = 0x00;

/// I/O APIC version register (bits 7:0 = version, bits 23:16 = max redir entry).
const IOAPIC_REG_VER: u32 = 0x01;

/// I/O APIC arbitration register.
const IOAPIC_REG_ARB: u32 = 0x02;

/// Base index of the redirection table.  Entry N uses registers
/// `IOAPIC_REG_REDIR_BASE + 2*N` (low 32 bits) and
/// `IOAPIC_REG_REDIR_BASE + 2*N + 1` (high 32 bits).
const IOAPIC_REG_REDIR_BASE: u32 = 0x10;

// ---------------------------------------------------------------------------
// Redirection entry bit-fields
// ---------------------------------------------------------------------------

/// Interrupt mask bit (bit 16 of the low dword).  1 = masked.
const REDIR_MASK_BIT: u32 = 1 << 16;

/// Delivery mode shift (bits 10:8).
const REDIR_DELIVERY_SHIFT: u32 = 8;

/// Destination mode bit (bit 11).  0 = physical, 1 = logical.
#[allow(dead_code)]
const REDIR_DESTMODE_BIT: u32 = 1 << 11;

/// Pin polarity bit (bit 13).  0 = active high, 1 = active low.
#[allow(dead_code)]
const REDIR_POLARITY_BIT: u32 = 1 << 13;

/// Trigger mode bit (bit 15).  0 = edge, 1 = level.
#[allow(dead_code)]
const REDIR_TRIGGER_BIT: u32 = 1 << 15;

/// Destination field shift in the high dword (bits 31:24).
const REDIR_DEST_SHIFT: u32 = 24;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Virtual base address of the mapped I/O APIC registers.
static IOAPIC_VIRT_BASE: AtomicU64 = AtomicU64::new(0);

/// Set to `true` after successful initialisation.
static IOAPIC_INIT: AtomicBool = AtomicBool::new(false);

/// Maximum redirection entry index (0-based).
static IOAPIC_MAX_ENTRY: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Low-level MMIO helpers
// ---------------------------------------------------------------------------

/// Read an indirect I/O APIC register.
///
/// Writes `reg` to IOREGSEL then reads IOWIN.
pub fn read_reg(reg: u32) -> u32 {
    let base = IOAPIC_VIRT_BASE.load(Ordering::Relaxed);
    assert!(base != 0, "ioapic: not initialised");
    unsafe {
        let sel = base as *mut u32;
        let win = (base + IOWIN_OFFSET) as *mut u32;
        core::ptr::write_volatile(sel, reg);
        core::ptr::read_volatile(win)
    }
}

/// Write an indirect I/O APIC register.
///
/// Writes `reg` to IOREGSEL then writes `val` to IOWIN.
pub fn write_reg(reg: u32, val: u32) {
    let base = IOAPIC_VIRT_BASE.load(Ordering::Relaxed);
    assert!(base != 0, "ioapic: not initialised");
    unsafe {
        let sel = base as *mut u32;
        let win = (base + IOWIN_OFFSET) as *mut u32;
        core::ptr::write_volatile(sel, reg);
        core::ptr::write_volatile(win, val);
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the I/O APIC driver.
///
/// `base_addr` is the physical address of the I/O APIC MMIO region
/// (typically `0xFEC0_0000`).  The function translates it to a virtual
/// address via `crate::memory::phys_to_virt`, reads the version register,
/// and masks all IRQ lines by default.
pub fn init(base_addr: u64, _phys_offset: u64) {
    let virt = memory::phys_to_virt(PhysAddr::new(base_addr));
    IOAPIC_VIRT_BASE.store(virt.as_u64(), Ordering::SeqCst);

    let ver = read_reg(IOAPIC_REG_VER);
    let max_entry = ((ver >> 16) & 0xFF) as u64;
    IOAPIC_MAX_ENTRY.store(max_entry, Ordering::SeqCst);

    // Mask every redirection entry on startup.
    for i in 0..=max_entry as u8 {
        mask_irq(i);
    }

    IOAPIC_INIT.store(true, Ordering::SeqCst);

    let id = read_reg(IOAPIC_REG_ID) >> 24;
    crate::serial_println!(
        "[ioapic] id={}, version=0x{:02x}, max_irqs={}",
        id,
        ver & 0xFF,
        max_entry + 1,
    );
}

// ---------------------------------------------------------------------------
// IRQ configuration
// ---------------------------------------------------------------------------

/// Configure a redirection entry for the given IRQ pin.
///
/// Sets the entry to deliver a **fixed** interrupt with the specified
/// `vector` to the local APIC identified by `dest_apic` (physical
/// destination mode, edge-triggered, active-high, unmasked).
pub fn set_irq(irq: u8, vector: u8, dest_apic: u8) {
    let max = IOAPIC_MAX_ENTRY.load(Ordering::Relaxed) as u8;
    assert!(irq <= max, "ioapic: irq {} out of range (max {})", irq, max);

    let low: u32 = (vector as u32)
        | (0 << REDIR_DELIVERY_SHIFT); // fixed delivery
    let high: u32 = (dest_apic as u32) << REDIR_DEST_SHIFT;

    let reg_low = IOAPIC_REG_REDIR_BASE + 2 * (irq as u32);
    let reg_high = reg_low + 1;

    write_reg(reg_high, high);
    write_reg(reg_low, low); // unmasked — bit 16 is 0
}

/// Mask (disable) an individual IRQ pin.
pub fn mask_irq(irq: u8) {
    let reg_low = IOAPIC_REG_REDIR_BASE + 2 * (irq as u32);
    let val = read_reg(reg_low);
    write_reg(reg_low, val | REDIR_MASK_BIT);
}

/// Unmask (enable) an individual IRQ pin.
pub fn unmask_irq(irq: u8) {
    let reg_low = IOAPIC_REG_REDIR_BASE + 2 * (irq as u32);
    let val = read_reg(reg_low);
    write_reg(reg_low, val & !REDIR_MASK_BIT);
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// Return a human-readable summary of the I/O APIC state.
///
/// Includes the APIC ID, hardware version, and maximum number of IRQ lines.
pub fn info() -> String {
    if !IOAPIC_INIT.load(Ordering::Relaxed) {
        return String::from("I/O APIC: not initialised");
    }

    let ver = read_reg(IOAPIC_REG_VER);
    let id = read_reg(IOAPIC_REG_ID) >> 24;
    let max_irqs = ((ver >> 16) & 0xFF) + 1;
    let version = ver & 0xFF;
    let arb = read_reg(IOAPIC_REG_ARB) >> 24;

    format!(
        "I/O APIC id={}, version=0x{:02x}, arbitration={}, max_irqs={}",
        id, version, arb, max_irqs,
    )
}
