/// GPU detection for MerlionOS.
/// Scans PCI bus for all GPU vendors (AMD, Intel, NVIDIA, QEMU)
/// and reports identified hardware.

use crate::pci;
use alloc::string::String;
use alloc::format;

/// Scan PCI for ALL GPU vendors and return a summary.
pub fn scan_all_gpus() -> String {
    let mut out = String::new();
    let mut found = 0u32;

    for bus in 0u8..8 {
        for device in 0u8..32 {
            let vendor = pci::pci_read32(bus, device, 0, 0x00) as u16;
            if vendor == 0xFFFF || vendor == 0 { continue; }

            let class_rev = pci::pci_read32(bus, device, 0, 0x08);
            let class = ((class_rev >> 24) & 0xFF) as u8;

            // Class 0x03 = Display controller
            if class != 0x03 { continue; }

            let dev_id = (pci::pci_read32(bus, device, 0, 0x00) >> 16) as u16;
            let subsys = pci::pci_read32(bus, device, 0, 0x2C);
            let subsys_vendor = (subsys & 0xFFFF) as u16;

            let vendor_name = match vendor {
                0x1002 => "AMD/ATI",
                0x8086 => "Intel",
                0x10DE => "NVIDIA",
                0x1A03 => "ASPEED",
                0x1234 => "QEMU/Bochs",
                _ => "Unknown",
            };

            let gpu_name = identify_gpu(vendor, dev_id);
            let board = board_vendor(subsys_vendor);

            found += 1;
            out.push_str(&format!(
                "GPU #{}: {} {} [{:04x}:{:04x}] at {:02x}:{:02x}.0",
                found, vendor_name, gpu_name, vendor, dev_id, bus, device
            ));
            if board != "Unknown" {
                out.push_str(&format!(" ({})", board));
            }
            out.push('\n');
        }
    }

    if found == 0 {
        out.push_str("No GPU detected.\n");
    } else {
        out.push_str(&format!("Total: {} GPU(s) found.\n", found));
    }
    out
}

/// Identify GPU model by vendor + device ID.
fn identify_gpu(vendor: u16, dev_id: u16) -> &'static str {
    match (vendor, dev_id) {
        // AMD — use amdgpu module's table
        (0x1002, id) => crate::amdgpu::lookup_device_name(id),

        // Intel integrated
        (0x8086, 0x0166) => "HD 4000 (Ivy Bridge)",
        (0x8086, 0x0412) => "HD 4600 (Haswell)",
        (0x8086, 0x1912) => "HD 530 (Skylake)",
        (0x8086, 0x5916) => "HD 620 (Kaby Lake)",
        (0x8086, 0x5917) => "UHD 620 (Kaby Lake)",
        (0x8086, 0x5912) => "HD 630 (Kaby Lake)",
        (0x8086, 0x591B) => "HD 630 (Kaby Lake-H)",
        (0x8086, 0x3E92) => "UHD 630 (Coffee Lake)",
        (0x8086, 0x3EA0) => "UHD 620 (Whiskey Lake)",
        (0x8086, 0x9A49) => "Xe (Tiger Lake)",
        (0x8086, 0x4680) => "Xe (Alder Lake)",
        (0x8086, 0x46A6) => "Xe (Alder Lake-P)",
        (0x8086, 0xA7A0) => "Xe (Raptor Lake)",
        (0x8086, 0x7D55) => "Xe (Meteor Lake)",
        // Intel Arc
        (0x8086, 0x5690) => "Arc A770",
        (0x8086, 0x5691) => "Arc A750",
        (0x8086, 0x5692) => "Arc A580",
        (0x8086, 0x56A0) => "Arc A380",

        // NVIDIA
        (0x10DE, 0x2684) => "RTX 4090",
        (0x10DE, 0x2704) => "RTX 4080",
        (0x10DE, 0x2782) => "RTX 4070 Ti",
        (0x10DE, 0x2786) => "RTX 4070",
        (0x10DE, 0x2204) => "RTX 3090",
        (0x10DE, 0x2206) => "RTX 3080",
        (0x10DE, 0x2484) => "RTX 3070",
        (0x10DE, 0x2504) => "RTX 3060 Ti",
        (0x10DE, 0x2560) => "RTX 3060",
        (0x10DE, 0x1E04) => "RTX 2080 Ti",
        (0x10DE, 0x1E07) => "RTX 2080",
        (0x10DE, 0x1F08) => "RTX 2060",
        (0x10DE, 0x1B80) => "GTX 1080",
        (0x10DE, 0x1B06) => "GTX 1080 Ti",
        (0x10DE, 0x1C82) => "GTX 1050 Ti",

        // QEMU
        (0x1234, 0x1111) => "Standard VGA",
        (0x1234, _) => "VGA",

        _ => "Unknown",
    }
}

/// Board vendor from PCI subsystem vendor ID.
fn board_vendor(subsys_vendor: u16) -> &'static str {
    match subsys_vendor {
        0x1002 => "AMD Reference",
        0x106B => "Apple",
        0x1043 => "ASUS",
        0x1458 => "Gigabyte",
        0x1462 => "MSI",
        0x196D => "Club 3D",
        0x1DA2 => "Sapphire",
        0x148C => "PowerColor",
        0x1682 => "XFX",
        0x1569 => "Palit",
        0x1849 => "ASRock",
        0x103C => "HP",
        0x1028 => "Dell",
        0x17AA => "Lenovo",
        0x1025 => "Acer",
        0x8086 => "Intel",
        0x10DE => "NVIDIA",
        _ => "Unknown",
    }
}

pub fn init() {
    let summary = scan_all_gpus();
    for line in summary.lines() {
        crate::serial_println!("[gpu] {}", line);
    }
}
