/// PCI bus enumeration via I/O ports 0xCF8/0xCFC.
/// Scans bus 0 for devices and reports vendor/device IDs and class codes.

use x86_64::instructions::port::Port;
use alloc::vec::Vec;
use alloc::string::String;

const PCI_CONFIG_ADDR: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;

#[derive(Debug, Clone)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
}

impl PciDevice {
    /// Human-readable class description.
    pub fn class_name(&self) -> &'static str {
        match (self.class, self.subclass) {
            (0x00, _) => "Unclassified",
            (0x01, 0x00) => "SCSI controller",
            (0x01, 0x01) => "IDE controller",
            (0x01, 0x06) => "SATA controller",
            (0x01, _) => "Storage",
            (0x02, 0x00) => "Ethernet",
            (0x02, _) => "Network",
            (0x03, 0x00) => "VGA controller",
            (0x03, _) => "Display",
            (0x04, _) => "Multimedia",
            (0x05, _) => "Memory",
            (0x06, 0x00) => "Host bridge",
            (0x06, 0x01) => "ISA bridge",
            (0x06, 0x04) => "PCI bridge",
            (0x06, _) => "Bridge",
            (0x07, _) => "Communication",
            (0x08, _) => "System peripheral",
            (0x0C, 0x03) => "USB controller",
            (0x0C, _) => "Serial bus",
            _ => "Other",
        }
    }

    /// Vendor name lookup for common vendors.
    pub fn vendor_name(&self) -> &'static str {
        match self.vendor_id {
            0x8086 => "Intel",
            0x1AF4 => "Red Hat (virtio)",
            0x1234 => "QEMU",
            0x10EC => "Realtek",
            0x1022 => "AMD",
            _ => "Unknown",
        }
    }

    pub fn summary(&self) -> String {
        alloc::format!(
            "{:02x}:{:02x}.{} {:04x}:{:04x} {} [{}]",
            self.bus, self.device, self.function,
            self.vendor_id, self.device_id,
            self.class_name(), self.vendor_name()
        )
    }
}

/// Read a 32-bit value from PCI configuration space.
fn pci_read32(bus: u8, device: u8, func: u8, offset: u8) -> u32 {
    let addr: u32 = 0x8000_0000
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);

    unsafe {
        Port::new(PCI_CONFIG_ADDR).write(addr);
        Port::<u32>::new(PCI_CONFIG_DATA).read()
    }
}

/// Scan PCI bus 0 for all devices.
pub fn scan() -> Vec<PciDevice> {
    let mut devices = Vec::new();

    for device in 0..32u8 {
        for function in 0..8u8 {
            let vendor_device = pci_read32(0, device, function, 0x00);
            let vendor_id = (vendor_device & 0xFFFF) as u16;

            if vendor_id == 0xFFFF {
                if function == 0 { break; } // no device here
                continue;
            }

            let device_id = ((vendor_device >> 16) & 0xFFFF) as u16;
            let class_reg = pci_read32(0, device, function, 0x08);
            let class = ((class_reg >> 24) & 0xFF) as u8;
            let subclass = ((class_reg >> 16) & 0xFF) as u8;
            let prog_if = ((class_reg >> 8) & 0xFF) as u8;

            devices.push(PciDevice {
                bus: 0,
                device,
                function,
                vendor_id,
                device_id,
                class,
                subclass,
                prog_if,
            });

            // Check if multi-function device
            if function == 0 {
                let header = pci_read32(0, device, 0, 0x0C);
                if (header >> 16) & 0x80 == 0 {
                    break; // single-function device
                }
            }
        }
    }

    devices
}
