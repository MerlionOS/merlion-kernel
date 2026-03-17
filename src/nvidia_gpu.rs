/// NVIDIA GPU detection and identification for MerlionOS.
/// Detects NVIDIA GPUs via PCI, reads BAR/VRAM info, identifies
/// the GPU architecture and model.
/// NOTE: Compute/rendering NOT supported — NVIDIA requires signed
/// firmware and closed-source CUDA runtime.

use crate::{pci, memory, serial_println};
use alloc::string::String;
use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// NVIDIA vendor ID and known device IDs
// ---------------------------------------------------------------------------

const NVIDIA_VENDOR_ID: u16 = 0x10DE;

/// (device_id, name, architecture, cuda_cores)
const NVIDIA_GPU_IDS: &[(u16, &str, &str, u32)] = &[
    // Ada Lovelace (RTX 40)
    (0x2684, "RTX 4090", "Ada Lovelace (AD102)", 16384),
    (0x2704, "RTX 4080 Super", "Ada Lovelace (AD103)", 10240),
    (0x2782, "RTX 4070 Ti Super", "Ada Lovelace (AD103)", 8448),
    (0x2786, "RTX 4070", "Ada Lovelace (AD104)", 5888),
    (0x2860, "RTX 4060 Ti", "Ada Lovelace (AD106)", 4352),
    (0x2882, "RTX 4060", "Ada Lovelace (AD107)", 3072),
    // Ampere (RTX 30)
    (0x2204, "RTX 3090", "Ampere (GA102)", 10496),
    (0x2206, "RTX 3080", "Ampere (GA102)", 8704),
    (0x2216, "RTX 3080 Ti", "Ampere (GA102)", 10240),
    (0x2484, "RTX 3070", "Ampere (GA104)", 5888),
    (0x2504, "RTX 3060 Ti", "Ampere (GA104)", 4864),
    (0x2560, "RTX 3060", "Ampere (GA106)", 3584),
    (0x25A0, "RTX 3050", "Ampere (GA106)", 2560),
    // Ampere (data center)
    (0x2235, "A100 80GB", "Ampere (GA100)", 6912),
    (0x20B5, "A100 40GB", "Ampere (GA100)", 6912),
    (0x2236, "A10", "Ampere (GA102)", 9216),
    (0x25B6, "A16", "Ampere (GA107)", 2560),
    // Hopper
    (0x2330, "H100 SXM", "Hopper (GH100)", 16896),
    (0x2331, "H100 PCIe", "Hopper (GH100)", 14592),
    // Turing (RTX 20)
    (0x1E04, "RTX 2080 Ti", "Turing (TU102)", 4352),
    (0x1E07, "RTX 2080", "Turing (TU104)", 2944),
    (0x1F08, "RTX 2060", "Turing (TU106)", 1920),
    (0x1F02, "RTX 2070", "Turing (TU106)", 2304),
    (0x2182, "GTX 1660 Ti", "Turing (TU116)", 1536),
    (0x2184, "GTX 1660", "Turing (TU116)", 1408),
    // Pascal (GTX 10)
    (0x1B80, "GTX 1080", "Pascal (GP104)", 2560),
    (0x1B06, "GTX 1080 Ti", "Pascal (GP102)", 3584),
    (0x1B81, "GTX 1070", "Pascal (GP104)", 1920),
    (0x1C82, "GTX 1050 Ti", "Pascal (GP107)", 768),
    (0x1C81, "GTX 1050", "Pascal (GP107)", 640),
    // Maxwell
    (0x17C8, "GTX 980 Ti", "Maxwell (GM200)", 2816),
    (0x13C0, "GTX 980", "Maxwell (GM204)", 2048),
    (0x1401, "GTX 960", "Maxwell (GM206)", 1024),
    // Data center / Workstation
    (0x1DB4, "Tesla V100 16GB", "Volta (GV100)", 5120),
    (0x1DB5, "Tesla V100 32GB", "Volta (GV100)", 5120),
    (0x1EB8, "Tesla T4", "Turing (TU104)", 2560),
    (0x26B5, "L40", "Ada Lovelace (AD102)", 18176),
    (0x27B8, "L4", "Ada Lovelace (AD104)", 7424),
    // Laptop
    (0x2520, "RTX 3060 Laptop", "Ampere (GA106)", 3840),
    (0x25A2, "RTX 3050 Laptop", "Ampere (GA107)", 2048),
    (0x2820, "RTX 4090 Laptop", "Ada Lovelace (AD103)", 9728),
    // Quadro
    (0x1E30, "Quadro RTX 6000", "Turing (TU102)", 4608),
    (0x2230, "RTX A6000", "Ampere (GA102)", 10752),
];

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static DETECTED: AtomicBool = AtomicBool::new(false);
static BAR0_VIRT: AtomicU64 = AtomicU64::new(0);
static BAR0_SIZE_GLOBAL: AtomicU64 = AtomicU64::new(0);
static GPU_STATE: Mutex<Option<NvidiaGpuInfo>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// GPU info struct
// ---------------------------------------------------------------------------

/// Detected NVIDIA GPU information.
#[derive(Clone)]
pub struct NvidiaGpuInfo {
    pub found: bool,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub device_id: u16,
    pub name: String,
    pub architecture: String,
    pub cuda_cores: u32,
    pub bar0: u64,          // MMIO registers
    pub bar0_size: u64,
    pub bar1: u64,          // Framebuffer/VRAM
    pub bar1_size: u64,
    pub vram_mb: u32,
    pub revision: u8,
    pub subsystem_vendor: u16,
    pub subsystem_id: u16,
    pub pcie_link_speed: u8,
    pub pcie_link_width: u8,
    pub firmware_signed: bool, // always true for Maxwell+
}

impl NvidiaGpuInfo {
    fn empty() -> Self {
        Self {
            found: false,
            bus: 0,
            device: 0,
            function: 0,
            device_id: 0,
            name: String::new(),
            architecture: String::new(),
            cuda_cores: 0,
            bar0: 0,
            bar0_size: 0,
            bar1: 0,
            bar1_size: 0,
            vram_mb: 0,
            revision: 0,
            subsystem_vendor: 0,
            subsystem_id: 0,
            pcie_link_speed: 0,
            pcie_link_width: 0,
            firmware_signed: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Board vendor identification
// ---------------------------------------------------------------------------

fn board_vendor(subsys_vendor: u16) -> &'static str {
    match subsys_vendor {
        0x10DE => "NVIDIA Reference",
        0x1043 => "ASUS",
        0x1462 => "MSI",
        0x1458 => "Gigabyte",
        0x3842 => "EVGA",
        0x19DA => "Zotac",
        0x196E => "PNY",
        0x10B0 => "Gainward",
        0x1569 => "Palit",
        0x1B4C => "Inno3D",
        0x7377 => "Colorful",
        0x1048 => "Galax",
        0x1028 => "Dell",
        0x103C => "HP",
        0x17AA => "Lenovo",
        0x1025 => "Acer",
        0x1849 => "ASRock",
        _ => "Unknown",
    }
}

// ---------------------------------------------------------------------------
// Architecture detection
// ---------------------------------------------------------------------------

/// Detect GPU architecture family from device ID upper byte.
fn detect_architecture(dev_id: u16) -> &'static str {
    match dev_id >> 8 {
        0x26 | 0x27 | 0x28 => "Ada Lovelace",
        0x23 => "Hopper",
        0x22 | 0x20 | 0x25 => "Ampere",
        0x1E | 0x1F | 0x21 => "Turing",
        0x1B | 0x1C | 0x1D => "Pascal",
        0x13 | 0x14 | 0x17 => "Maxwell",
        0x0F | 0x11 | 0x12 => "Kepler",
        _ => "Unknown",
    }
}

/// Returns true if the GPU architecture requires signed firmware.
/// All NVIDIA GPUs since Maxwell (2014) require signed firmware.
fn requires_signed_firmware(dev_id: u16) -> bool {
    match detect_architecture(dev_id) {
        "Kepler" => false,
        _ => true, // Maxwell and newer all require signed firmware
    }
}

// ---------------------------------------------------------------------------
// PCIe helpers
// ---------------------------------------------------------------------------

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

/// PCIe speed in GT/s (as string, no floats).
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

/// Walk PCI capabilities list to find capability with given ID.
fn find_pci_capability(bus: u8, dev: u8, func: u8, cap_id: u8) -> u8 {
    let status = (pci::pci_read32(bus, dev, func, 0x04) >> 16) as u16;
    if status & (1 << 4) == 0 {
        return 0;
    }

    let mut ptr = (pci::pci_read32(bus, dev, func, 0x34) & 0xFF) as u8;

    for _ in 0..48 {
        if ptr == 0 || ptr == 0xFF {
            return 0;
        }
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

/// Read PCIe link status. Returns (speed, width).
fn read_pcie_link_info(bus: u8, dev: u8, func: u8) -> (u8, u8) {
    let pcie_cap = find_pci_capability(bus, dev, func, 0x10);
    if pcie_cap == 0 {
        return (0, 0);
    }

    let link_reg = pci::pci_read32(bus, dev, func, pcie_cap + 0x10);
    let link_status = (link_reg >> 16) as u16;
    let speed = (link_status & 0xF) as u8;
    let width = ((link_status >> 4) & 0x3F) as u8;
    (speed, width)
}

// ---------------------------------------------------------------------------
// VRAM detection
// ---------------------------------------------------------------------------

/// Detect VRAM size using BAR1 size or fallback table.
/// Note: cannot read NV_PMC registers without firmware loaded.
fn detect_vram_mb(bar1_size: u64, device_id: u16) -> u32 {
    // Method 1: BAR1 size (framebuffer aperture)
    if bar1_size > 0 {
        let mb = (bar1_size / (1024 * 1024)) as u32;
        // BAR1 often equals VRAM size on modern GPUs
        if mb > 0 {
            return mb;
        }
    }

    // Method 2: fallback table by device ID (VRAM in MB)
    match device_id {
        // Ada Lovelace
        0x2684 => 24576, // RTX 4090 = 24 GB
        0x2704 => 16384, // RTX 4080 Super = 16 GB
        0x2782 => 16384, // RTX 4070 Ti Super = 16 GB
        0x2786 => 12288, // RTX 4070 = 12 GB
        0x2860 => 16384, // RTX 4060 Ti = 16 GB
        0x2882 => 8192,  // RTX 4060 = 8 GB
        // Ampere
        0x2204 => 24576, // RTX 3090 = 24 GB
        0x2206 => 10240, // RTX 3080 = 10 GB
        0x2216 => 12288, // RTX 3080 Ti = 12 GB
        0x2484 => 8192,  // RTX 3070 = 8 GB
        0x2504 => 8192,  // RTX 3060 Ti = 8 GB
        0x2560 => 12288, // RTX 3060 = 12 GB
        0x25A0 => 8192,  // RTX 3050 = 8 GB
        // Data center
        0x2235 => 81920, // A100 80GB
        0x20B5 => 40960, // A100 40GB
        0x2236 => 24576, // A10 = 24 GB
        0x25B6 => 16384, // A16 = 16 GB
        0x2330 => 81920, // H100 SXM = 80 GB
        0x2331 => 81920, // H100 PCIe = 80 GB
        // Turing
        0x1E04 => 11264, // RTX 2080 Ti = 11 GB
        0x1E07 => 8192,  // RTX 2080 = 8 GB
        0x1F08 => 6144,  // RTX 2060 = 6 GB
        0x1F02 => 8192,  // RTX 2070 = 8 GB
        0x2182 => 6144,  // GTX 1660 Ti = 6 GB
        0x2184 => 6144,  // GTX 1660 = 6 GB
        // Pascal
        0x1B80 => 8192,  // GTX 1080 = 8 GB
        0x1B06 => 11264, // GTX 1080 Ti = 11 GB
        0x1B81 => 8192,  // GTX 1070 = 8 GB
        0x1C82 => 4096,  // GTX 1050 Ti = 4 GB
        0x1C81 => 2048,  // GTX 1050 = 2 GB
        // Maxwell
        0x17C8 => 6144,  // GTX 980 Ti = 6 GB
        0x13C0 => 4096,  // GTX 980 = 4 GB
        0x1401 => 2048,  // GTX 960 = 2 GB
        // Volta / Tesla
        0x1DB4 => 16384, // Tesla V100 16GB
        0x1DB5 => 32768, // Tesla V100 32GB
        0x1EB8 => 16384, // Tesla T4 = 16 GB
        // Ada data center
        0x26B5 => 49152, // L40 = 48 GB
        0x27B8 => 24576, // L4 = 24 GB
        // Laptop
        0x2520 => 6144,  // RTX 3060 Laptop = 6 GB
        0x25A2 => 4096,  // RTX 3050 Laptop = 4 GB
        0x2820 => 16384, // RTX 4090 Laptop = 16 GB
        // Quadro
        0x1E30 => 24576, // Quadro RTX 6000 = 24 GB
        0x2230 => 49152, // RTX A6000 = 48 GB
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Device lookup
// ---------------------------------------------------------------------------

/// Look up device name, architecture, and CUDA core count from device ID.
fn lookup_device(device_id: u16) -> (&'static str, &'static str, u32) {
    for &(did, name, arch, cores) in NVIDIA_GPU_IDS {
        if did == device_id {
            return (name, arch, cores);
        }
    }
    ("Unknown NVIDIA GPU", detect_architecture(device_id), 0)
}

/// Look up NVIDIA device name by device ID. Used by gpu_detect module.
pub fn lookup_device_name(dev_id: u16) -> &'static str {
    NVIDIA_GPU_IDS.iter()
        .find(|(id, _, _, _)| *id == dev_id)
        .map(|(_, name, _, _)| *name)
        .unwrap_or("Unknown NVIDIA GPU")
}

// ---------------------------------------------------------------------------
// Detection and scanning
// ---------------------------------------------------------------------------

/// Scan PCI buses for NVIDIA GPU devices. Returns info for the first one found.
pub fn detect() -> Option<NvidiaGpuInfo> {
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

                if vendor_id != NVIDIA_VENDOR_ID {
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

                // Found an NVIDIA display controller — read details
                let (name, architecture, cuda_cores) = lookup_device(device_id);

                // Read BARs: BAR0 = MMIO registers, BAR1 = framebuffer/VRAM
                let bar0 = read_bar(bus, dev, func, 0x10);
                let bar0_size = read_bar_size(bus, dev, func, 0x10);
                let bar1 = read_bar(bus, dev, func, 0x14);
                let bar1_size = read_bar_size(bus, dev, func, 0x14);

                // Subsystem vendor/device at offset 0x2C
                let subsys = pci::pci_read32(bus, dev, func, 0x2C);
                let subsystem_vendor = (subsys & 0xFFFF) as u16;
                let subsystem_id = ((subsys >> 16) & 0xFFFF) as u16;

                // PCIe link info
                let (pcie_link_speed, pcie_link_width) =
                    read_pcie_link_info(bus, dev, func);

                // Detect VRAM
                let vram_mb = detect_vram_mb(bar1_size, device_id);

                // Firmware signing required since Maxwell (2014)
                let firmware_signed = requires_signed_firmware(device_id);

                let info = NvidiaGpuInfo {
                    found: true,
                    bus,
                    device: dev,
                    function: func,
                    device_id,
                    name: String::from(name),
                    architecture: String::from(architecture),
                    cuda_cores,
                    bar0,
                    bar0_size,
                    bar1,
                    bar1_size,
                    vram_mb,
                    revision,
                    subsystem_vendor,
                    subsystem_id,
                    pcie_link_speed,
                    pcie_link_width,
                    firmware_signed,
                };

                return Some(info);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Compute status
// ---------------------------------------------------------------------------

/// Returns a message explaining why compute is not available.
pub fn compute_status() -> String {
    String::from(
        "NVIDIA compute: NOT AVAILABLE\n\
         Reason: NVIDIA GPUs require signed firmware (since Maxwell, 2014)\n\
         and CUDA runtime (closed-source) for compute operations.\n\
         The open-source nouveau driver has minimal compute support.\n\
         For GPU compute, use AMD (open docs) or Intel (open PRM)."
    )
}

// ---------------------------------------------------------------------------
// Formatted info output
// ---------------------------------------------------------------------------

/// Format human-readable GPU information string.
pub fn nvidia_gpu_info() -> String {
    let lock = GPU_STATE.lock();
    let info = match lock.as_ref() {
        Some(i) if i.found => i,
        _ => return String::from("No NVIDIA GPU detected"),
    };

    let vendor_str = board_vendor(info.subsystem_vendor);
    let bar0_mib = info.bar0_size / (1024 * 1024);
    let bar1_mib = info.bar1_size / (1024 * 1024);
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

    let fw_str = if info.firmware_signed {
        "Firmware: signed (compute locked without NVIDIA cooperation)"
    } else {
        "Firmware: unsigned (pre-Maxwell, limited open-source support)"
    };

    format!(
        "NVIDIA GPU: {} [{:04X}]\n\
         Architecture: {}\n\
         CUDA cores: {}\n\
         Board: {} (subsys {:04X}:{:04X})\n\
         PCI: {:02x}:{:02x}.{}, rev {:02X}\n\
         BAR0: 0x{:X} ({} MiB) — MMIO registers\n\
         BAR1: 0x{:X} ({} MiB) — Framebuffer/VRAM\n\
         VRAM: {} MiB ({} GB)\n\
         {}\n\
         {}\n\
         {}",
        info.name, info.device_id,
        info.architecture,
        info.cuda_cores,
        vendor_str, info.subsystem_vendor, info.subsystem_id,
        info.bus, info.device, info.function, info.revision,
        info.bar0, bar0_mib,
        info.bar1, bar1_mib,
        info.vram_mb, vram_gb,
        pcie_str,
        fw_str,
        compute_status(),
    )
}

/// Format GPU stats (limited — no register access without firmware).
pub fn nvidia_gpu_stats() -> String {
    if !DETECTED.load(Ordering::Relaxed) {
        return String::from("No NVIDIA GPU detected");
    }

    let lock = GPU_STATE.lock();
    let info = match lock.as_ref() {
        Some(i) if i.found => i,
        _ => return String::from("No NVIDIA GPU detected"),
    };

    format!(
        "NVIDIA GPU status:\n\
         Device: {} ({})\n\
         CUDA cores: {}\n\
         VRAM: {} MB\n\
         BAR0 mapped: {}\n\
         Note: Cannot read GPU registers — firmware not loaded.\n\
         NVIDIA requires signed firmware blobs for hardware initialization.\n\
         Register access (NV_PMC, NV_PFIFO, etc.) is not possible without\n\
         a loaded and authenticated firmware image.",
        info.name,
        info.architecture,
        info.cuda_cores,
        info.vram_mb,
        if info.bar0 != 0 { "yes (read-only, pre-firmware)" } else { "no" },
    )
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns true if an NVIDIA GPU was detected during init.
pub fn is_detected() -> bool {
    DETECTED.load(Ordering::Relaxed)
}

/// Initialize NVIDIA GPU detection: scan PCI, identify hardware, store state.
pub fn init() {
    serial_println!("[nvidia_gpu] scanning PCI for NVIDIA GPUs...");

    match detect() {
        Some(info) => {
            serial_println!(
                "[nvidia_gpu] found: {} ({}) at {:02x}:{:02x}.{}",
                info.name, info.architecture,
                info.bus, info.device, info.function,
            );
            serial_println!(
                "[nvidia_gpu] {} CUDA cores, BAR0=0x{:X} ({}K), BAR1=0x{:X} ({}M), VRAM={}M",
                info.cuda_cores,
                info.bar0, info.bar0_size / 1024,
                info.bar1, info.bar1_size / (1024 * 1024),
                info.vram_mb,
            );
            if info.pcie_link_speed > 0 {
                serial_println!(
                    "[nvidia_gpu] PCIe {} x{} ({} GT/s)",
                    pcie_gen_str(info.pcie_link_speed),
                    info.pcie_link_width,
                    pcie_gts(info.pcie_link_speed),
                );
            }
            if info.firmware_signed {
                serial_println!(
                    "[nvidia_gpu] firmware signing required — compute NOT available"
                );
            }

            // Map BAR0 for potential read-only MMIO access
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
            serial_println!("[nvidia_gpu] no NVIDIA GPU detected");
        }
    }
}
