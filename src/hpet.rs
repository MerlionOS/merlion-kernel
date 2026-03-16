/// High Precision Event Timer (HPET) driver for MerlionOS.
///
/// Provides MMIO access to the HPET for sub-microsecond timing. The base
/// address is discovered from the ACPI "HPET" table. Once initialised the
/// main counter runs continuously and can be read via `read_counter()`.

use alloc::format;
use alloc::string::String;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use x86_64::PhysAddr;
use crate::memory;

/// Memory-mapped register block for the HPET.
///
/// Registers are at fixed offsets from the base address. Each is 64 bits wide.
/// Timer config/comparator registers repeat for timers 0 through 2.
#[repr(C)]
pub struct HpetRegisters {
    /// General Capabilities and ID (offset 0x000).
    /// Bits 63:32 — clock period in femtoseconds. Bits 12:8 — num timers - 1.
    /// Bit 13 — 64-bit main counter capability.
    pub capabilities: u64,
    _reserved0: u64,
    /// General Configuration (offset 0x010). Bit 0 — enable. Bit 1 — legacy routing.
    pub config: u64,
    _reserved1: u64,
    /// General Interrupt Status (offset 0x020).
    pub interrupt_status: u64,
    _reserved2: [u64; 25],
    /// Main Counter Value (offset 0x0F0).
    pub main_counter: u64,
    _reserved3: u64,
    /// Timer 0 Configuration and Capability (offset 0x100).
    pub timer0_config: u64,
    /// Timer 0 Comparator Value (offset 0x108).
    pub timer0_comparator: u64,
    /// Timer 0 FSB Interrupt Route (offset 0x110).
    pub timer0_fsb_route: u64,
    _reserved4: u64,
    /// Timer 1 Configuration and Capability (offset 0x120).
    pub timer1_config: u64,
    /// Timer 1 Comparator Value (offset 0x128).
    pub timer1_comparator: u64,
    /// Timer 1 FSB Interrupt Route (offset 0x130).
    pub timer1_fsb_route: u64,
    _reserved5: u64,
    /// Timer 2 Configuration and Capability (offset 0x140).
    pub timer2_config: u64,
    /// Timer 2 Comparator Value (offset 0x148).
    pub timer2_comparator: u64,
    /// Timer 2 FSB Interrupt Route (offset 0x150).
    pub timer2_fsb_route: u64,
    _reserved6: u64,
}

/// Virtual address of the HPET register block (set during init).
static HPET_BASE: AtomicU64 = AtomicU64::new(0);
/// Counter clock period in femtoseconds (from capabilities register).
static PERIOD_FS: AtomicU64 = AtomicU64::new(0);
/// Number of timers available (1-based count).
static NUM_TIMERS: AtomicU64 = AtomicU64::new(0);
/// Whether the main counter supports 64-bit mode.
static IS_64BIT: AtomicBool = AtomicBool::new(false);
/// Set once initialisation has completed successfully.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// ACPI HPET Description Table body (follows the 36-byte SdtHeader).
#[repr(C, packed)]
struct AcpiHpetTable {
    pub hw_rev_id: u8,
    /// Bits 4:0 — comparator count - 1. Bit 5 — 64-bit. Bit 6 — legacy capable.
    pub info: u8,
    pub pci_vendor_id: u16,
    /// Generic Address Structure: address space (0 = memory).
    pub addr_space_id: u8,
    pub register_bit_width: u8,
    pub register_bit_offset: u8,
    pub _reserved: u8,
    /// Physical base address of the HPET register block.
    pub base_address: u64,
    pub hpet_number: u8,
    pub min_tick: u16,
    pub page_protection: u8,
}

/// Search the ACPI RSDT/XSDT for an "HPET" table and return its physical
/// base address. Returns `None` if no HPET table is present.
///
/// `rsdp_phys` — physical address of the RSDP.
/// `phys_offset` — bootloader physical-memory mapping offset.
pub fn find_hpet_base(rsdp_phys: u64, phys_offset: u64) -> Option<u64> {
    use crate::acpi_tables::SdtHeader;
    use core::mem;

    let rsdp_virt = (rsdp_phys + phys_offset) as *const u8;
    let sig: [u8; 8] = unsafe { ptr::read_unaligned(rsdp_virt as *const [u8; 8]) };
    if &sig != b"RSD PTR " { return None; }

    let revision = unsafe { ptr::read(rsdp_virt.add(15)) };
    let (sdt_phys, use_xsdt) = if revision >= 2 {
        let xsdt = unsafe { ptr::read_unaligned(rsdp_virt.add(24) as *const u64) };
        if xsdt != 0 { (xsdt, true) }
        else { (unsafe { ptr::read_unaligned(rsdp_virt.add(16) as *const u32) } as u64, false) }
    } else {
        (unsafe { ptr::read_unaligned(rsdp_virt.add(16) as *const u32) } as u64, false)
    };

    let sdt_virt = (sdt_phys + phys_offset) as *const u8;
    let sdt_hdr = unsafe { ptr::read_unaligned(sdt_virt as *const SdtHeader) };
    let hdr_sz = mem::size_of::<SdtHeader>();
    let ptr_sz: usize = if use_xsdt { 8 } else { 4 };
    let count = (sdt_hdr.length as usize - hdr_sz) / ptr_sz;

    for i in 0..count {
        let entry_phys: u64 = if use_xsdt {
            unsafe { ptr::read_unaligned(sdt_virt.add(hdr_sz + i * 8) as *const u64) }
        } else {
            unsafe { ptr::read_unaligned(sdt_virt.add(hdr_sz + i * 4) as *const u32) as u64 }
        };
        let entry_virt = (entry_phys + phys_offset) as *const u8;
        let entry_sig: [u8; 4] = unsafe { ptr::read_unaligned(entry_virt as *const [u8; 4]) };
        if &entry_sig == b"HPET" {
            let hpet = unsafe { ptr::read_unaligned(entry_virt.add(hdr_sz) as *const AcpiHpetTable) };
            if hpet.addr_space_id == 0 {
                return Some(hpet.base_address);
            }
        }
    }
    None
}

/// Initialise the HPET driver from a known physical base address.
///
/// Maps the MMIO registers, reads capabilities (period, timer count, width),
/// and stores global state. Does **not** enable the counter — call
/// `enable_counter()` separately.
///
/// # Safety
/// `base_phys` must point to a valid HPET register block.
pub unsafe fn init(base_phys: u64) {
    let virt = memory::phys_to_virt(PhysAddr::new(base_phys));
    let regs = virt.as_ptr() as *const HpetRegisters;

    let caps = unsafe { ptr::read_volatile(&(*regs).capabilities) };
    let period = caps >> 32;
    let num_timers = ((caps >> 8) & 0x1F) + 1;
    let is_64bit = (caps >> 13) & 1 != 0;

    HPET_BASE.store(virt.as_u64(), Ordering::SeqCst);
    PERIOD_FS.store(period, Ordering::SeqCst);
    NUM_TIMERS.store(num_timers, Ordering::SeqCst);
    IS_64BIT.store(is_64bit, Ordering::SeqCst);
    INITIALIZED.store(true, Ordering::SeqCst);

    crate::serial_println!(
        "[hpet] base={:#x} period={}fs timers={} 64bit={}",
        base_phys, period, num_timers, is_64bit,
    );
    crate::klog_println!(
        "[hpet] init: period={}fs timers={} 64bit={}",
        period, num_timers, is_64bit,
    );
}

/// Enable the HPET main counter by setting bit 0 of the configuration register.
pub fn enable_counter() {
    let base = HPET_BASE.load(Ordering::SeqCst);
    if base == 0 { return; }
    unsafe {
        let regs = base as *mut HpetRegisters;
        let cfg = ptr::read_volatile(&(*regs).config);
        ptr::write_volatile(&mut (*regs).config, cfg | 1);
    }
    crate::serial_println!("[hpet] main counter enabled");
}

/// Read the current value of the HPET main counter.
/// Returns 0 if the HPET has not been initialised.
pub fn read_counter() -> u64 {
    let base = HPET_BASE.load(Ordering::SeqCst);
    if base == 0 { return 0; }
    unsafe {
        let regs = base as *const HpetRegisters;
        ptr::read_volatile(&(*regs).main_counter)
    }
}

/// Busy-wait for `ns` nanoseconds using the HPET counter.
///
/// Returns immediately if the HPET is uninitialised. Accuracy depends on the
/// counter period (typically 100 ns on QEMU, ~10 ns on real hardware).
pub fn nanosleep(ns: u64) {
    let period = PERIOD_FS.load(Ordering::SeqCst);
    if period == 0 { return; }
    // ticks = ns * 1_000_000 / period_fs
    let ticks = ns.saturating_mul(1_000_000) / period;
    if ticks == 0 { return; }

    let start = read_counter();
    while read_counter().wrapping_sub(start) < ticks {
        core::hint::spin_loop();
    }
}

/// Return the HPET frequency in Hz (10^15 / period_fs).
/// Returns 0 if the HPET has not been initialised.
pub fn hpet_frequency() -> u64 {
    let period = PERIOD_FS.load(Ordering::SeqCst);
    if period == 0 { return 0; }
    1_000_000_000_000_000 / period
}

/// Return a human-readable summary of the HPET configuration.
pub fn timer_info() -> String {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return String::from("HPET: not initialised");
    }
    let base = HPET_BASE.load(Ordering::SeqCst);
    let period = PERIOD_FS.load(Ordering::SeqCst);
    let timers = NUM_TIMERS.load(Ordering::SeqCst);
    let width = if IS_64BIT.load(Ordering::SeqCst) { 64 } else { 32 };
    let freq = hpet_frequency();
    let counter = read_counter();

    format!(
        "HPET Information\n\
         ----------------\n\
         Base address : {:#014x}\n\
         Period       : {} fs/tick\n\
         Frequency    : {} Hz ({}.{:03} MHz)\n\
         Timers       : {}\n\
         Counter width: {}-bit\n\
         Counter value: {:#018x} ({} ticks)",
        base, period, freq, freq / 1_000_000, (freq % 1_000_000) / 1_000,
        timers, width, counter, counter,
    )
}
