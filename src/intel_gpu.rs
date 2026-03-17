/// Intel GPU detection and identification for MerlionOS.
/// Scans PCI for Intel integrated/discrete GPUs, reads device info,
/// and identifies the GPU generation and EU count.
/// Targets: Gen9 (Skylake/Kaby Lake), Gen11 (Ice Lake), Gen12/Xe (Tiger Lake+), Arc

use crate::{pci, memory, serial_println};
use alloc::string::String;
use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Intel vendor ID and known device IDs
// ---------------------------------------------------------------------------

const INTEL_VENDOR_ID: u16 = 0x8086;

/// (device_id, name, generation, EU_count)
const INTEL_GPU_IDS: &[(u16, &str, &str, u32)] = &[
    // Gen9 (Skylake)
    (0x1912, "HD 530", "Gen9 (Skylake)", 24),
    (0x1916, "HD 520", "Gen9 (Skylake)", 24),
    // Gen9.5 (Kaby Lake) — this is the MacBook Pro 2017 GPU
    (0x5912, "HD 630", "Gen9.5 (Kaby Lake)", 24),
    (0x5916, "HD 620", "Gen9.5 (Kaby Lake)", 24),
    (0x5917, "UHD 620", "Gen9.5 (Kaby Lake)", 24),
    (0x591B, "HD 630", "Gen9.5 (Kaby Lake-H)", 24),
    (0x591E, "HD 615", "Gen9.5 (Kaby Lake)", 24),
    // Gen9.5 (Coffee Lake)
    (0x3E92, "UHD 630", "Gen9.5 (Coffee Lake)", 24),
    (0x3E91, "UHD 630", "Gen9.5 (Coffee Lake)", 24),
    (0x3EA0, "UHD 620", "Gen9.5 (Whiskey Lake)", 24),
    // Gen11 (Ice Lake)
    (0x8A56, "Iris Plus G7", "Gen11 (Ice Lake)", 64),
    (0x8A52, "Iris Plus G7", "Gen11 (Ice Lake)", 64),
    // Gen12 / Xe (Tiger Lake)
    (0x9A49, "Xe (Tiger Lake)", "Gen12 (Tiger Lake)", 96),
    (0x9A40, "Xe (Tiger Lake)", "Gen12 (Tiger Lake)", 80),
    // Gen12 / Xe (Alder Lake)
    (0x4680, "Xe (Alder Lake-S)", "Gen12 (Alder Lake)", 32),
    (0x46A6, "Xe (Alder Lake-P)", "Gen12 (Alder Lake)", 96),
    // Gen12 / Xe (Raptor Lake)
    (0xA7A0, "Xe (Raptor Lake)", "Gen12 (Raptor Lake)", 32),
    // Gen12 / Xe (Meteor Lake)
    (0x7D55, "Xe (Meteor Lake)", "Gen12 (Meteor Lake)", 128),
    // Arc (Alchemist - DG2)
    (0x5690, "Arc A770", "Xe-HPG (Alchemist)", 512),
    (0x5691, "Arc A750", "Xe-HPG (Alchemist)", 448),
    (0x5692, "Arc A580", "Xe-HPG (Alchemist)", 384),
    (0x56A0, "Arc A380", "Xe-HPG (Alchemist)", 128),
    (0x56A1, "Arc A310", "Xe-HPG (Alchemist)", 96),
];

// ---------------------------------------------------------------------------
// MMIO register offsets (Intel PRM — public documentation)
// ---------------------------------------------------------------------------

// Graphics MMIO registers
const MMIO_RENDER_RING_BASE: u32 = 0x02000;  // Render ring buffer
const MMIO_BSD_RING_BASE: u32 = 0x04000;     // Video decode ring
const MMIO_BLT_RING_BASE: u32 = 0x22000;     // Blit ring
const MMIO_VECS_RING_BASE: u32 = 0x1A000;    // Video enhance ring
const MMIO_FORCEWAKE: u32 = 0xA18C;          // Force GT wake
const MMIO_FORCEWAKE_ACK: u32 = 0x130044;
const MMIO_GT_CORE_STATUS: u32 = 0x138060;
const MMIO_RPSTAT1: u32 = 0xA01C;            // Current GPU frequency
const MMIO_RP_CTRL: u32 = 0xA024;            // Requested GPU frequency
const MMIO_RING_HEAD: u32 = 0x00;             // Ring head offset (relative)
const MMIO_RING_TAIL: u32 = 0x04;             // Ring tail offset
const MMIO_RING_START: u32 = 0x08;            // Ring start address
const MMIO_RING_CTL: u32 = 0x0C;             // Ring control
// Stolen memory detection
const MMIO_BSM: u32 = 0x5C;                  // Base of Stolen Memory (PCI config)

/// Named register table for stats dump.
const NAMED_REGS: &[(&str, u32)] = &[
    ("RENDER_RING_BASE", MMIO_RENDER_RING_BASE),
    ("BSD_RING_BASE", MMIO_BSD_RING_BASE),
    ("BLT_RING_BASE", MMIO_BLT_RING_BASE),
    ("VECS_RING_BASE", MMIO_VECS_RING_BASE),
    ("FORCEWAKE", MMIO_FORCEWAKE),
    ("FORCEWAKE_ACK", MMIO_FORCEWAKE_ACK),
    ("GT_CORE_STATUS", MMIO_GT_CORE_STATUS),
    ("RPSTAT1", MMIO_RPSTAT1),
    ("RP_CTRL", MMIO_RP_CTRL),
];

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static DETECTED: AtomicBool = AtomicBool::new(false);
static BAR0_VIRT: AtomicU64 = AtomicU64::new(0);
static BAR0_SIZE_GLOBAL: AtomicU64 = AtomicU64::new(0);
static GPU_STATE: Mutex<Option<IntelGpuInfo>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// GPU info struct
// ---------------------------------------------------------------------------

/// Detected Intel GPU information.
#[derive(Clone)]
pub struct IntelGpuInfo {
    pub found: bool,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub device_id: u16,
    pub name: String,
    pub generation: String,
    pub eu_count: u32,
    pub bar0: u64,          // GTTMMADR (MMIO + GTT)
    pub bar0_size: u64,
    pub bar2: u64,          // GMADR (aperture)
    pub bar2_size: u64,
    pub stolen_mb: u32,     // stolen memory from system RAM
    pub revision: u8,
    pub subsystem_vendor: u16,
    pub subsystem_id: u16,
}

impl IntelGpuInfo {
    fn empty() -> Self {
        Self {
            found: false,
            bus: 0,
            device: 0,
            function: 0,
            device_id: 0,
            name: String::new(),
            generation: String::new(),
            eu_count: 0,
            bar0: 0,
            bar0_size: 0,
            bar2: 0,
            bar2_size: 0,
            stolen_mb: 0,
            revision: 0,
            subsystem_vendor: 0,
            subsystem_id: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Board vendor identification
// ---------------------------------------------------------------------------

fn board_vendor(subsys_vendor: u16) -> &'static str {
    match subsys_vendor {
        0x8086 => "Intel",
        0x106B => "Apple",
        0x1043 => "ASUS",
        0x1028 => "Dell",
        0x103C => "HP",
        0x17AA => "Lenovo",
        0x1025 => "Acer",
        0x1458 => "Gigabyte",
        0x1462 => "MSI",
        0x1849 => "ASRock",
        _ => "Unknown",
    }
}

// ---------------------------------------------------------------------------
// PCI helpers
// ---------------------------------------------------------------------------

/// Read a PCI BAR (32-bit or 64-bit) and return the base address.
fn read_bar(bus: u8, dev: u8, func: u8, bar_offset: u8) -> u64 {
    let raw = pci::pci_read32(bus, dev, func, bar_offset);
    if raw == 0 {
        return 0;
    }

    // Check if memory BAR (bit 0 = 0)
    if raw & 1 != 0 {
        // I/O BAR — mask low 2 bits
        return (raw & !0x3) as u64;
    }

    let bar_type = (raw >> 1) & 0x3;
    let base_low = (raw & !0xF) as u64;

    if bar_type == 2 {
        // 64-bit BAR: upper 32 bits in next BAR register
        let high = pci::pci_read32(bus, dev, func, bar_offset + 4) as u64;
        base_low | (high << 32)
    } else {
        base_low
    }
}

/// Determine the size of a PCI BAR by writing all-ones and reading back.
fn read_bar_size(bus: u8, dev: u8, func: u8, bar_offset: u8) -> u64 {
    let original = pci::pci_read32(bus, dev, func, bar_offset);
    if original == 0 {
        return 0;
    }

    let is_64bit = (original & 1 == 0) && ((original >> 1) & 0x3) == 2;

    // Disable I/O and memory decoding while we probe
    let cmd = pci::pci_read32(bus, dev, func, 0x04);
    pci::pci_write32(bus, dev, func, 0x04, cmd & !0x3);

    // Write all-ones, read back, restore
    pci::pci_write32(bus, dev, func, bar_offset, 0xFFFF_FFFF);
    let readback = pci::pci_read32(bus, dev, func, bar_offset);
    pci::pci_write32(bus, dev, func, bar_offset, original);

    if is_64bit {
        let original_hi = pci::pci_read32(bus, dev, func, bar_offset + 4);
        pci::pci_write32(bus, dev, func, bar_offset + 4, 0xFFFF_FFFF);
        let readback_hi = pci::pci_read32(bus, dev, func, bar_offset + 4);
        pci::pci_write32(bus, dev, func, bar_offset + 4, original_hi);

        // Restore command register
        pci::pci_write32(bus, dev, func, 0x04, cmd);

        if readback == 0 {
            return 0;
        }

        let mask_lo = (readback & !0xF) as u64;
        let mask_hi = readback_hi as u64;
        let mask = mask_lo | (mask_hi << 32);
        if mask == 0 {
            return 0;
        }
        (!mask).wrapping_add(1)
    } else {
        // Restore command register
        pci::pci_write32(bus, dev, func, 0x04, cmd);

        if readback == 0 {
            return 0;
        }
        let mask = (readback & !0xF) as u32;
        if mask == 0 {
            return 0;
        }
        (!mask).wrapping_add(1) as u64
    }
}

// ---------------------------------------------------------------------------
// Stolen memory detection
// ---------------------------------------------------------------------------

/// Detect stolen memory size from PCI config space.
/// Intel iGPUs steal system RAM for GPU use; the size is encoded
/// in the GMS (Graphics Mode Select) field of the GMCH register.
fn detect_stolen_mb(bus: u8, dev: u8, func: u8) -> u32 {
    // GMCH Graphics Control Register at offset 0x50
    let ggc = pci::pci_read32(bus, dev, func, 0x50);
    // GMS field is bits [15:8] (Gen6+)
    let gms = ((ggc >> 8) & 0xFF) as u32;

    // GMS encoding (Gen9+):
    //   0x00 = no stolen memory
    //   0x01 = 32 MB
    //   0x02 = 64 MB
    //   0x03 = 96 MB
    //   ...
    //   each increment = 32 MB
    //   0xF0+ = reserved
    if gms == 0 || gms >= 0xF0 {
        return 0;
    }

    gms * 32
}

// ---------------------------------------------------------------------------
// GPU frequency
// ---------------------------------------------------------------------------

/// Read current GPU frequency from RPSTAT1 register.
/// Returns frequency in MHz, or 0 if not accessible.
pub fn read_gpu_frequency() -> u32 {
    let base = BAR0_VIRT.load(Ordering::Relaxed);
    let size = BAR0_SIZE_GLOBAL.load(Ordering::Relaxed);
    if base == 0 || (MMIO_RPSTAT1 as u64) + 4 > size {
        return 0;
    }

    let rpstat1 = unsafe {
        let ptr = (base + MMIO_RPSTAT1 as u64) as *const u32;
        core::ptr::read_volatile(ptr)
    };

    // Gen9+: current frequency is in bits [23:17], multiply by 50/3 (~16.67 MHz units)
    // Simplified: bits [23:17] * 100 / 6
    let cur_ratio = (rpstat1 >> 17) & 0x7F;
    // Each unit is approximately 16.67 MHz (50/3)
    // Use integer math: ratio * 50 / 3
    if cur_ratio > 0 {
        cur_ratio * 50 / 3
    } else {
        0
    }
}

/// Read the max (RP0) GPU frequency from RP_CTRL register.
pub fn read_max_frequency() -> u32 {
    let base = BAR0_VIRT.load(Ordering::Relaxed);
    let size = BAR0_SIZE_GLOBAL.load(Ordering::Relaxed);
    if base == 0 || (MMIO_RP_CTRL as u64) + 4 > size {
        return 0;
    }

    let rp_ctrl = unsafe {
        let ptr = (base + MMIO_RP_CTRL as u64) as *const u32;
        core::ptr::read_volatile(ptr)
    };

    // Requested frequency is in bits [23:17]
    let req_ratio = (rp_ctrl >> 17) & 0x7F;
    if req_ratio > 0 {
        req_ratio * 50 / 3
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// ForceWake — wake GPU from power saving
// ---------------------------------------------------------------------------

/// Assert ForceWake to bring the GT out of RC6 power saving.
/// Must be done before reading most GPU registers.
pub fn forcewake_get() {
    let base = BAR0_VIRT.load(Ordering::Relaxed);
    let size = BAR0_SIZE_GLOBAL.load(Ordering::Relaxed);
    if base == 0 || (MMIO_FORCEWAKE as u64) + 4 > size {
        return;
    }

    // Write 1 to ForceWake to wake the GT
    unsafe {
        let ptr = (base + MMIO_FORCEWAKE as u64) as *mut u32;
        core::ptr::write_volatile(ptr, 1);
    }

    // Poll ForceWake ACK — wait until GT acknowledges wakeup
    if (MMIO_FORCEWAKE_ACK as u64) + 4 <= size {
        for _ in 0..1000u32 {
            let ack = unsafe {
                let ptr = (base + MMIO_FORCEWAKE_ACK as u64) as *const u32;
                core::ptr::read_volatile(ptr)
            };
            if ack & 1 != 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }
}

/// Release ForceWake to allow GT to enter power saving.
pub fn forcewake_put() {
    let base = BAR0_VIRT.load(Ordering::Relaxed);
    let size = BAR0_SIZE_GLOBAL.load(Ordering::Relaxed);
    if base == 0 || (MMIO_FORCEWAKE as u64) + 4 > size {
        return;
    }

    unsafe {
        let ptr = (base + MMIO_FORCEWAKE as u64) as *mut u32;
        core::ptr::write_volatile(ptr, 0);
    }
}

// ---------------------------------------------------------------------------
// MMIO register access (public API)
// ---------------------------------------------------------------------------

/// Read a 32-bit MMIO register at the given offset from BAR0.
pub fn mmio_read32(offset: u32) -> Option<u32> {
    let base = BAR0_VIRT.load(Ordering::Relaxed);
    let size = BAR0_SIZE_GLOBAL.load(Ordering::Relaxed);
    if base == 0 || (offset as u64) + 4 > size {
        return None;
    }
    let val = unsafe {
        let ptr = (base + offset as u64) as *const u32;
        core::ptr::read_volatile(ptr)
    };
    Some(val)
}

/// Write a 32-bit MMIO register at the given offset from BAR0.
pub fn mmio_write32(offset: u32, val: u32) {
    let base = BAR0_VIRT.load(Ordering::Relaxed);
    let size = BAR0_SIZE_GLOBAL.load(Ordering::Relaxed);
    if base == 0 || (offset as u64) + 4 > size {
        return;
    }
    unsafe {
        let ptr = (base + offset as u64) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }
}

// ---------------------------------------------------------------------------
// Detection and scanning
// ---------------------------------------------------------------------------

/// Look up device name, generation, and EU count from device ID.
fn lookup_device(device_id: u16) -> (&'static str, &'static str, u32) {
    for &(did, name, gen, eu) in INTEL_GPU_IDS {
        if did == device_id {
            return (name, gen, eu);
        }
    }
    ("Unknown Intel GPU", "Unknown", 0)
}

/// Look up Intel device name by device ID. Used by gpu_detect module.
pub fn lookup_device_name(dev_id: u16) -> &'static str {
    INTEL_GPU_IDS.iter()
        .find(|(id, _, _, _)| *id == dev_id)
        .map(|(_, name, _, _)| *name)
        .unwrap_or("Unknown Intel GPU")
}

/// Scan PCI buses for Intel GPU devices. Returns info for the first one found.
pub fn detect() -> Option<IntelGpuInfo> {
    for bus in 0..8u8 {
        for dev in 0..32u8 {
            for func in 0..8u8 {
                let vendor_device = pci::pci_read32(bus, dev, func, 0x00);
                let vendor_id = (vendor_device & 0xFFFF) as u16;

                if vendor_id == 0xFFFF {
                    if func == 0 {
                        break;
                    }
                    continue;
                }

                if vendor_id != INTEL_VENDOR_ID {
                    if func == 0 {
                        let hdr = pci::pci_read32(bus, dev, 0, 0x0C);
                        if (hdr >> 16) & 0x80 == 0 {
                            break;
                        }
                    }
                    continue;
                }

                let device_id = ((vendor_device >> 16) & 0xFFFF) as u16;

                // Read class/subclass
                let class_reg = pci::pci_read32(bus, dev, func, 0x08);
                let pci_class = ((class_reg >> 24) & 0xFF) as u8;
                let revision = (class_reg & 0xFF) as u8;

                // We want display controllers (class 0x03)
                if pci_class != 0x03 {
                    if func == 0 {
                        let hdr = pci::pci_read32(bus, dev, 0, 0x0C);
                        if (hdr >> 16) & 0x80 == 0 {
                            break;
                        }
                    }
                    continue;
                }

                // Found an Intel display controller — read details
                let (name, generation, eu_count) = lookup_device(device_id);

                // Read BARs
                let bar0 = read_bar(bus, dev, func, 0x10);
                let bar0_size = read_bar_size(bus, dev, func, 0x10);
                let bar2 = read_bar(bus, dev, func, 0x18);
                let bar2_size = read_bar_size(bus, dev, func, 0x18);

                // Subsystem vendor/device at offset 0x2C
                let subsys = pci::pci_read32(bus, dev, func, 0x2C);
                let subsystem_vendor = (subsys & 0xFFFF) as u16;
                let subsystem_id = ((subsys >> 16) & 0xFFFF) as u16;

                // Stolen memory
                let stolen_mb = detect_stolen_mb(bus, dev, func);

                let info = IntelGpuInfo {
                    found: true,
                    bus,
                    device: dev,
                    function: func,
                    device_id,
                    name: String::from(name),
                    generation: String::from(generation),
                    eu_count,
                    bar0,
                    bar0_size,
                    bar2,
                    bar2_size,
                    stolen_mb,
                    revision,
                    subsystem_vendor,
                    subsystem_id,
                };

                return Some(info);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Formatted info output
// ---------------------------------------------------------------------------

/// Format human-readable GPU information string.
pub fn intel_gpu_info() -> String {
    let lock = GPU_STATE.lock();
    let info = match lock.as_ref() {
        Some(i) if i.found => i,
        _ => return String::from("No Intel GPU detected"),
    };

    let vendor_str = board_vendor(info.subsystem_vendor);
    let bar0_mib = info.bar0_size / (1024 * 1024);
    let bar2_mib = info.bar2_size / (1024 * 1024);

    let cur_freq = read_gpu_frequency();
    let max_freq = read_max_frequency();

    let freq_str = if cur_freq > 0 || max_freq > 0 {
        format!("Frequency: {} MHz (current), {} MHz (requested)", cur_freq, max_freq)
    } else {
        String::from("Frequency: not available (BAR0 not mapped)")
    };

    format!(
        "Intel GPU: {} ({})\n\
         Generation: {}\n\
         EU count: {}\n\
         Board: {} (subsys {:04X}:{:04X})\n\
         PCI: {:02x}:{:02x}.{}, rev {:02X}\n\
         BAR0: 0x{:X} ({} MiB) — MMIO + GTT\n\
         BAR2: 0x{:X} ({} MiB) — Graphics aperture\n\
         Stolen memory: {} MB\n\
         {}",
        info.name, info.device_id,
        info.generation,
        info.eu_count,
        vendor_str, info.subsystem_vendor, info.subsystem_id,
        info.bus, info.device, info.function, info.revision,
        info.bar0, bar0_mib,
        info.bar2, bar2_mib,
        info.stolen_mb,
        freq_str,
    )
}

/// Format register dump for known GPU registers.
pub fn intel_gpu_stats() -> String {
    if !DETECTED.load(Ordering::Relaxed) {
        return String::from("No Intel GPU detected");
    }

    let base = BAR0_VIRT.load(Ordering::Relaxed);
    if base == 0 {
        return String::from("Intel GPU: BAR0 not mapped, cannot read registers");
    }

    let mut s = String::from("Intel GPU registers:\n");
    for &(name, offset) in NAMED_REGS {
        match mmio_read32(offset) {
            Some(val) => {
                s.push_str(&format!(
                    "  0x{:06X} {:<20} = 0x{:08X}\n",
                    offset, name, val,
                ));
            }
            None => {
                s.push_str(&format!(
                    "  0x{:06X} {:<20} = <out of range>\n",
                    offset, name,
                ));
            }
        }
    }

    // Add ring buffer status
    for &(ring_name, ring_base) in &[
        ("Render", MMIO_RENDER_RING_BASE),
        ("BSD", MMIO_BSD_RING_BASE),
        ("BLT", MMIO_BLT_RING_BASE),
        ("VECS", MMIO_VECS_RING_BASE),
    ] {
        let head = mmio_read32(ring_base + MMIO_RING_HEAD).unwrap_or(0);
        let tail = mmio_read32(ring_base + MMIO_RING_TAIL).unwrap_or(0);
        let ctl = mmio_read32(ring_base + MMIO_RING_CTL).unwrap_or(0);
        let enabled = if ctl & 1 != 0 { "enabled" } else { "disabled" };
        s.push_str(&format!(
            "  {} ring: head=0x{:X} tail=0x{:X} ctl=0x{:X} ({})\n",
            ring_name, head, tail, ctl, enabled,
        ));
    }

    s
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns true if an Intel GPU was detected during init.
pub fn is_detected() -> bool {
    DETECTED.load(Ordering::Relaxed)
}

/// Initialize the Intel GPU driver: scan PCI, identify hardware, store state.
pub fn init() {
    serial_println!("[intel_gpu] scanning PCI for Intel GPUs...");

    match detect() {
        Some(info) => {
            serial_println!(
                "[intel_gpu] found: {} ({}) at {:02x}:{:02x}.{}",
                info.name, info.generation,
                info.bus, info.device, info.function,
            );
            serial_println!(
                "[intel_gpu] {} EUs, BAR0=0x{:X} ({}K), BAR2=0x{:X} ({}M), stolen={}M",
                info.eu_count,
                info.bar0, info.bar0_size / 1024,
                info.bar2, info.bar2_size / (1024 * 1024),
                info.stolen_mb,
            );

            // Map BAR0 for MMIO access
            if info.bar0 != 0 {
                let virt = memory::phys_to_virt(
                    x86_64::PhysAddr::new(info.bar0),
                );
                BAR0_VIRT.store(virt.as_u64(), Ordering::Relaxed);
                BAR0_SIZE_GLOBAL.store(info.bar0_size, Ordering::Relaxed);
            }

            DETECTED.store(true, Ordering::Relaxed);
            *GPU_STATE.lock() = Some(info);
        }
        None => {
            serial_println!("[intel_gpu] no Intel GPU detected");
        }
    }
}
