/// Virtio device discovery and common types.
/// Virtio devices are identified on the PCI bus by vendor 0x1AF4.
/// This module provides the shared virtqueue abstraction.

use crate::pci;
use alloc::vec::Vec;
use alloc::string::String;

/// Virtio PCI vendor ID.
pub const VIRTIO_VENDOR: u16 = 0x1AF4;

/// Virtio device types (PCI device ID - 0x1040 for modern, or legacy IDs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VirtioDeviceType {
    Network,    // 0x1000 (legacy) or 0x1041
    Block,      // 0x1001 (legacy) or 0x1042
    Console,    // 0x1003 (legacy) or 0x1043
    Entropy,    // 0x1005 (legacy) or 0x1044
    Unknown(u16),
}

impl VirtioDeviceType {
    pub fn from_device_id(id: u16) -> Self {
        match id {
            0x1000 | 0x1041 => VirtioDeviceType::Network,
            0x1001 | 0x1042 => VirtioDeviceType::Block,
            0x1003 | 0x1043 => VirtioDeviceType::Console,
            0x1005 | 0x1044 => VirtioDeviceType::Entropy,
            _ => VirtioDeviceType::Unknown(id),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            VirtioDeviceType::Network => "virtio-net",
            VirtioDeviceType::Block => "virtio-blk",
            VirtioDeviceType::Console => "virtio-console",
            VirtioDeviceType::Entropy => "virtio-rng",
            VirtioDeviceType::Unknown(_) => "virtio-unknown",
        }
    }
}

/// Discovered virtio device info.
#[derive(Debug, Clone)]
pub struct VirtioDevice {
    pub pci: pci::PciDevice,
    pub device_type: VirtioDeviceType,
}

impl VirtioDevice {
    pub fn summary(&self) -> String {
        alloc::format!(
            "{} at PCI {:02x}:{:02x}.{} (vid:{:04x} did:{:04x})",
            self.device_type.name(),
            self.pci.bus, self.pci.device, self.pci.function,
            self.pci.vendor_id, self.pci.device_id
        )
    }
}

/// Scan PCI bus for virtio devices.
pub fn scan() -> Vec<VirtioDevice> {
    pci::scan()
        .into_iter()
        .filter(|d| d.vendor_id == VIRTIO_VENDOR)
        .map(|d| {
            let device_type = VirtioDeviceType::from_device_id(d.device_id);
            VirtioDevice { pci: d, device_type }
        })
        .collect()
}

/// Virtqueue descriptor (simplified).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

/// Virtqueue status flags.
pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;

/// Virtio device status bits.
pub const VIRTIO_STATUS_ACKNOWLEDGE: u8 = 1;
pub const VIRTIO_STATUS_DRIVER: u8 = 2;
pub const VIRTIO_STATUS_DRIVER_OK: u8 = 4;
pub const VIRTIO_STATUS_FEATURES_OK: u8 = 8;
