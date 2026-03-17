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

        // Intel — use intel_gpu module's table
        (0x8086, id) => crate::intel_gpu::lookup_device_name(id),

        // NVIDIA — use nvidia_gpu module's table
        (0x10DE, id) => crate::nvidia_gpu::lookup_device_name(id),

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
