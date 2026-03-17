/// AMD GPU detection and identification for MerlionOS.
/// Scans PCI bus for AMD/ATI GPUs, reads device registers,
/// identifies the GPU family/model, and reports VRAM size.
/// Does NOT initialize the GPU for rendering — that requires
/// firmware loading and ~500K lines of driver code.

use crate::{pci, memory, serial_println};
use alloc::string::String;
use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// AMD vendor ID and known device IDs
// ---------------------------------------------------------------------------

const AMD_VENDOR_ID: u16 = 0x1002; // AMD/ATI

/// (device_id, product_name, gpu_family)
const DEVICE_IDS: &[(u16, &str, &str)] = &[
    // RDNA 3
    (0x744C, "Radeon RX 7900 XTX", "Navi 31"),
    (0x7480, "Radeon RX 7900 XT", "Navi 31"),
    (0x7460, "Radeon RX 7800 XT", "Navi 32"),
    (0x7470, "Radeon RX 7700 XT", "Navi 32"),
    (0x15BF, "Radeon RX 7600", "Navi 33"),
    // RDNA 2
    (0x73BF, "Radeon RX 6900 XT", "Navi 21"),
    (0x73DF, "Radeon RX 6800 XT", "Navi 21"),
    (0x73FF, "Radeon RX 6800", "Navi 21"),
    (0x73EF, "Radeon RX 6700 XT", "Navi 22"),
    (0x7422, "Radeon RX 6600 XT", "Navi 23"),
    (0x743F, "Radeon RX 6600", "Navi 23"),
    // RDNA 1
    (0x7310, "Radeon RX 5700 XT", "Navi 10"),
    (0x7312, "Radeon RX 5700", "Navi 10"),
    (0x7340, "Radeon RX 5500 XT", "Navi 14"),
    // GCN 5 (Vega)
    (0x687F, "Radeon RX Vega 64", "Vega 10"),
    (0x6863, "Radeon Vega FE", "Vega 10"),
    // GCN 4 (Polaris)
    (0x67DF, "Radeon RX 580", "Polaris 10"),
    (0x67EF, "Radeon Pro 560/555", "Polaris 11 (Baffin)"),
    (0x67FF, "Radeon RX 560", "Polaris 11"),
    // APU (integrated)
    (0x1636, "Radeon Vega 8 (Renoir)", "Renoir"),
    (0x164C, "Radeon 680M (Rembrandt)", "Rembrandt"),
    (0x15E7, "Radeon Vega 8 (Barcelo)", "Barcelo"),
    (0x1900, "Radeon R7 (Kaveri)", "Kaveri"),
];

// ---------------------------------------------------------------------------
// Known MMIO register offsets
// ---------------------------------------------------------------------------

const REG_MM_INDEX: u32 = 0x0000;
const REG_MM_DATA: u32 = 0x0004;
const REG_CONFIG_MEMSIZE: u32 = 0x5428;
const REG_CONFIG_APER_SIZE: u32 = 0x5430;
const REG_BIF_FB_EN: u32 = 0x0D00;
const REG_GRBM_STATUS: u32 = 0xD010;
const REG_GRBM_STATUS2: u32 = 0xD014;
const REG_CP_STAT: u32 = 0xD048;
const REG_GPU_HDP_FLUSH_DONE: u32 = 0x1630;

/// Named register table for `mmio_read_name`.
const NAMED_REGS: &[(&str, u32)] = &[
    ("MM_INDEX", REG_MM_INDEX),
    ("MM_DATA", REG_MM_DATA),
    ("CONFIG_MEMSIZE", REG_CONFIG_MEMSIZE),
    ("CONFIG_APER_SIZE", REG_CONFIG_APER_SIZE),
    ("BIF_FB_EN", REG_BIF_FB_EN),
    ("GRBM_STATUS", REG_GRBM_STATUS),
    ("GRBM_STATUS2", REG_GRBM_STATUS2),
    ("CP_STAT", REG_CP_STAT),
    ("GPU_HDP_FLUSH_DONE", REG_GPU_HDP_FLUSH_DONE),
];

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static DETECTED: AtomicBool = AtomicBool::new(false);
static BAR0_VIRT: AtomicU64 = AtomicU64::new(0);
static BAR0_SIZE_GLOBAL: AtomicU64 = AtomicU64::new(0);
static GPU_STATE: Mutex<Option<AmdGpuInfo>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// GPU info struct
// ---------------------------------------------------------------------------

/// Detected AMD GPU information.
#[derive(Clone)]
pub struct AmdGpuInfo {
    pub found: bool,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub revision: u8,
    pub name: String,
    pub family: String,
    pub pci_class: u8,
    pub pci_subclass: u8,
    pub bar0: u64,       // MMIO base (register aperture)
    pub bar0_size: u64,
    pub bar2: u64,       // VRAM aperture (framebuffer BAR)
    pub bar2_size: u64,
    pub vram_mb: u32,    // detected VRAM in MB
    pub subsystem_vendor: u16,
    pub subsystem_id: u16,
    pub irq: u8,
    pub pcie_link_speed: u8, // 1=2.5GT/s, 2=5GT/s, 3=8GT/s, 4=16GT/s, 5=32GT/s
    pub pcie_link_width: u8, // x1, x4, x8, x16
}

impl AmdGpuInfo {
    fn empty() -> Self {
        Self {
            found: false,
            bus: 0,
            device: 0,
            function: 0,
            vendor_id: 0,
            device_id: 0,
            revision: 0,
            name: String::new(),
            family: String::new(),
            pci_class: 0,
            pci_subclass: 0,
            bar0: 0,
            bar0_size: 0,
            bar2: 0,
            bar2_size: 0,
            vram_mb: 0,
            subsystem_vendor: 0,
            subsystem_id: 0,
            irq: 0,
            pcie_link_speed: 0,
            pcie_link_width: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Board vendor identification
// ---------------------------------------------------------------------------

fn board_vendor(subsys_vendor: u16) -> &'static str {
    match subsys_vendor {
        0x1002 => "AMD Reference",
        0x1043 => "ASUS",
        0x1458 => "Gigabyte",
        0x1462 => "MSI",
        0x196D => "Club 3D",
        0x1DA2 => "Sapphire",
        0x148C => "PowerColor",
        0x1682 => "XFX",
        0x1569 => "Palit",
        0x1849 => "ASRock",
        _ => "Unknown",
    }
}

/// Human-readable PCIe generation string.
fn pcie_gen_str(speed: u8) -> &'static str {
    match speed {
        1 => "Gen1",
        2 => "Gen2",
        3 => "Gen3",
        4 => "Gen4",
        5 => "Gen5",
        _ => "Unknown",
    }
}

/// PCIe speed in GT/s (as integer, no floats).
fn pcie_gts(speed: u8) -> &'static str {
    match speed {
        1 => "2.5",
        2 => "5",
        3 => "8",
        4 => "16",
        5 => "32",
        _ => "?",
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
/// Returns the size in bytes.
fn read_bar_size(bus: u8, dev: u8, func: u8, bar_offset: u8) -> u64 {
    let original = pci::pci_read32(bus, dev, func, bar_offset);
    if original == 0 {
        return 0;
    }

    // Check bar type before sizing
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

/// Walk PCI capabilities list to find capability with given ID.
/// Returns the offset of the capability in config space, or 0 if not found.
fn find_pci_capability(bus: u8, dev: u8, func: u8, cap_id: u8) -> u8 {
    // Check that capabilities list is supported (status bit 4)
    let status = (pci::pci_read32(bus, dev, func, 0x04) >> 16) as u16;
    if status & (1 << 4) == 0 {
        return 0;
    }

    // Capabilities pointer at offset 0x34
    let mut ptr = (pci::pci_read32(bus, dev, func, 0x34) & 0xFF) as u8;

    // Walk the linked list (max 48 to prevent infinite loop)
    for _ in 0..48 {
        if ptr == 0 || ptr == 0xFF {
            return 0;
        }
        // Align to dword boundary
        let aligned = ptr & 0xFC;
        let cap_reg = pci::pci_read32(bus, dev, func, aligned);
        let this_id = (cap_reg & 0xFF) as u8;
        if this_id == cap_id {
            return aligned;
        }
        ptr = ((cap_reg >> 8) & 0xFF) as u8;
    }
    0
}

/// Read PCIe link status from the PCIe capability structure.
/// Returns (speed, width).
fn read_pcie_link_info(bus: u8, dev: u8, func: u8) -> (u8, u8) {
    let pcie_cap = find_pci_capability(bus, dev, func, 0x10);
    if pcie_cap == 0 {
        return (0, 0);
    }

    // Link Status Register is at pcie_cap + 0x12 (within a dword at +0x10)
    let link_reg = pci::pci_read32(bus, dev, func, pcie_cap + 0x10);
    // Link Status is the upper 16 bits of this dword
    let link_status = (link_reg >> 16) as u16;
    let speed = (link_status & 0xF) as u8;
    let width = ((link_status >> 4) & 0x3F) as u8;
    (speed, width)
}

// ---------------------------------------------------------------------------
// VRAM size detection
// ---------------------------------------------------------------------------

/// Attempt to detect VRAM size using multiple methods.
/// Returns size in megabytes.
fn detect_vram_mb(bar0_virt: u64, bar2_size: u64, device_id: u16) -> u32 {
    // Method 1: read CONFIG_MEMSIZE register via MMIO if BAR0 is mapped
    if bar0_virt != 0 {
        let memsize = unsafe {
            let ptr = (bar0_virt + REG_CONFIG_MEMSIZE as u64) as *const u32;
            core::ptr::read_volatile(ptr)
        };
        if memsize > 0 && memsize < 0x8000_0000 {
            // CONFIG_MEMSIZE is in bytes
            return memsize / (1024 * 1024);
        }
    }

    // Method 2: BAR2 size (VRAM aperture size)
    if bar2_size > 0 {
        return (bar2_size / (1024 * 1024)) as u32;
    }

    // Method 3: fallback based on known device IDs
    match device_id {
        0x744C => 24576, // RX 7900 XTX = 24 GB
        0x7480 => 20480, // RX 7900 XT = 20 GB
        0x7460 => 16384, // RX 7800 XT = 16 GB
        0x7470 => 12288, // RX 7700 XT = 12 GB
        0x15BF => 8192,  // RX 7600 = 8 GB
        0x73BF => 16384, // RX 6900 XT = 16 GB
        0x73DF => 16384, // RX 6800 XT = 16 GB
        0x73FF => 16384, // RX 6800 = 16 GB
        0x73EF => 12288, // RX 6700 XT = 12 GB
        0x7422 => 8192,  // RX 6600 XT = 8 GB
        0x743F => 8192,  // RX 6600 = 8 GB
        0x7310 => 8192,  // RX 5700 XT = 8 GB
        0x7312 => 8192,  // RX 5700 = 8 GB
        0x7340 => 4096,  // RX 5500 XT = 4 GB
        0x687F => 8192,  // Vega 64 = 8 GB
        0x6863 => 16384, // Vega FE = 16 GB
        0x67DF => 8192,  // RX 580 = 8 GB
        0x67EF => 4096,  // Radeon Pro 560 = 4 GB
        0x67FF => 4096,  // RX 560 = 4 GB
        _ => 0,
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

/// Read a named MMIO register (e.g., "GRBM_STATUS").
pub fn mmio_read_name(name: &str) -> Option<u32> {
    for &(reg_name, offset) in NAMED_REGS {
        if reg_name == name {
            return mmio_read32(offset);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Detection and scanning
// ---------------------------------------------------------------------------

/// Scan PCI buses for AMD GPU devices. Returns info for the first one found.
pub fn detect() -> Option<AmdGpuInfo> {
    // Scan buses 0-7 (most GPUs on bus 0-3)
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

                if vendor_id != AMD_VENDOR_ID {
                    // Check multi-function
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
                let pci_subclass = ((class_reg >> 16) & 0xFF) as u8;
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

                // Found an AMD display controller — read details
                let (name, family) = lookup_device(device_id);

                // Read BARs
                let bar0 = read_bar(bus, dev, func, 0x10);
                let bar0_size = read_bar_size(bus, dev, func, 0x10);
                let bar2 = read_bar(bus, dev, func, 0x18);
                let bar2_size = read_bar_size(bus, dev, func, 0x18);

                // Subsystem vendor/device at offset 0x2C
                let subsys = pci::pci_read32(bus, dev, func, 0x2C);
                let subsystem_vendor = (subsys & 0xFFFF) as u16;
                let subsystem_id = ((subsys >> 16) & 0xFFFF) as u16;

                // IRQ at offset 0x3C
                let irq_reg = pci::pci_read32(bus, dev, func, 0x3C);
                let irq = (irq_reg & 0xFF) as u8;

                // PCIe link info
                let (pcie_link_speed, pcie_link_width) =
                    read_pcie_link_info(bus, dev, func);

                // Map BAR0 for MMIO access
                let bar0_virt = if bar0 != 0 {
                    let virt = memory::phys_to_virt(
                        x86_64::PhysAddr::new(bar0),
                    );
                    virt.as_u64()
                } else {
                    0
                };

                // Detect VRAM size
                let vram_mb = detect_vram_mb(bar0_virt, bar2_size, device_id);

                let info = AmdGpuInfo {
                    found: true,
                    bus,
                    device: dev,
                    function: func,
                    vendor_id,
                    device_id,
                    revision,
                    name: String::from(name),
                    family: String::from(family),
                    pci_class,
                    pci_subclass,
                    bar0,
                    bar0_size,
                    bar2,
                    bar2_size,
                    vram_mb,
                    subsystem_vendor,
                    subsystem_id,
                    irq,
                    pcie_link_speed,
                    pcie_link_width,
                };

                return Some(info);
            }
        }
    }
    None
}

/// Look up device name and family from device ID.
fn lookup_device(device_id: u16) -> (&'static str, &'static str) {
    for &(did, name, family) in DEVICE_IDS {
        if did == device_id {
            return (name, family);
        }
    }
    ("Unknown AMD GPU", "Unknown")
}

// ---------------------------------------------------------------------------
// Formatted info output
// ---------------------------------------------------------------------------

/// Format human-readable GPU information string.
pub fn amdgpu_info() -> String {
    let lock = GPU_STATE.lock();
    let info = match lock.as_ref() {
        Some(i) if i.found => i,
        _ => return String::from("No AMD GPU detected"),
    };

    let vendor_str = board_vendor(info.subsystem_vendor);

    let bar0_mib = info.bar0_size / (1024 * 1024);
    let bar2_mib = info.bar2_size / (1024 * 1024);
    let vram_gb = info.vram_mb / 1024;

    let pcie_str = if info.pcie_link_speed > 0 {
        format!(
            "PCIe: {} x{} ({} GT/s)",
            pcie_gen_str(info.pcie_link_speed),
            info.pcie_link_width,
            pcie_gts(info.pcie_link_speed),
        )
    } else {
        String::from("PCIe: unknown")
    };

    format!(
        "AMD GPU: {} ({})\n\
         Board: {} (subsys {:04X}:{:04X})\n\
         PCI: {:02x}:{:02x}.{}, class {:02x}/{:02x}, rev {:02X}\n\
         BAR0: 0x{:X} ({} MiB) — MMIO registers\n\
         BAR2: 0x{:X} ({} MiB) — VRAM aperture\n\
         VRAM: {} MiB ({} GB)\n\
         {}\n\
         IRQ: {}",
        info.name, info.family,
        vendor_str, info.subsystem_vendor, info.subsystem_id,
        info.bus, info.device, info.function,
        info.pci_class, info.pci_subclass, info.revision,
        info.bar0, bar0_mib,
        info.bar2, bar2_mib,
        info.vram_mb, vram_gb,
        pcie_str,
        info.irq,
    )
}

/// Format register dump for known GPU registers.
pub fn amdgpu_stats() -> String {
    if !DETECTED.load(Ordering::Relaxed) {
        return String::from("No AMD GPU detected");
    }

    let base = BAR0_VIRT.load(Ordering::Relaxed);
    if base == 0 {
        return String::from("AMD GPU: BAR0 not mapped, cannot read registers");
    }

    let mut s = String::from("AMD GPU registers:\n");
    for &(name, offset) in NAMED_REGS {
        match mmio_read32(offset) {
            Some(val) => {
                s.push_str(&format!(
                    "  0x{:04X} {:<20} = 0x{:08X}\n",
                    offset, name, val,
                ));
            }
            None => {
                s.push_str(&format!(
                    "  0x{:04X} {:<20} = <out of range>\n",
                    offset, name,
                ));
            }
        }
    }
    s
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns true if an AMD GPU was detected during init.
pub fn is_detected() -> bool {
    DETECTED.load(Ordering::Relaxed)
}

/// Initialize the AMD GPU driver: scan PCI, identify hardware, store state.
/// Look up AMD device name by device ID. Used by gpu_detect module.
pub fn lookup_device_name(dev_id: u16) -> &'static str {
    DEVICE_IDS.iter()
        .find(|(id, _, _)| *id == dev_id)
        .map(|(_, name, _)| *name)
        .unwrap_or("Unknown AMD GPU")
}

pub fn init() {
    serial_println!("[amdgpu] scanning PCI for AMD GPUs...");

    match detect() {
        Some(info) => {
            serial_println!(
                "[amdgpu] found: {} ({}) at {:02x}:{:02x}.{}",
                info.name, info.family,
                info.bus, info.device, info.function,
            );
            serial_println!(
                "[amdgpu] BAR0=0x{:X} ({}K), BAR2=0x{:X} ({}M), VRAM={}M",
                info.bar0, info.bar0_size / 1024,
                info.bar2, info.bar2_size / (1024 * 1024),
                info.vram_mb,
            );
            if info.pcie_link_speed > 0 {
                serial_println!(
                    "[amdgpu] PCIe {} x{} ({} GT/s)",
                    pcie_gen_str(info.pcie_link_speed),
                    info.pcie_link_width,
                    pcie_gts(info.pcie_link_speed),
                );
            }

            // Store BAR0 virtual address for MMIO access
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
            serial_println!("[amdgpu] no AMD GPU detected");
        }
    }
}
