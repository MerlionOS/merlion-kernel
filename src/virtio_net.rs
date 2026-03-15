/// Virtio-net driver stub.
/// Detects virtio network devices on PCI and provides the interface
/// for real Ethernet frame transmission/reception.

use crate::{pci, virtio, serial_println, klog_println, net};
use alloc::string::String;

/// Virtio-net device state.
pub struct VirtioNetDevice {
    pub pci_device: pci::PciDevice,
    pub mac: net::MacAddr,
    pub detected: bool,
}

static mut VIRTIO_NET: Option<VirtioNetDevice> = None;

/// Probe PCI for a virtio-net device.
pub fn init() {
    let devices = virtio::scan();
    for dev in &devices {
        if dev.device_type == virtio::VirtioDeviceType::Network {
            serial_println!("[virtio-net] found {}", dev.summary());
            klog_println!("[virtio-net] detected on PCI");

            unsafe {
                VIRTIO_NET = Some(VirtioNetDevice {
                    pci_device: dev.pci.clone(),
                    mac: net::MacAddr([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]),
                    detected: true,
                });
            }

            crate::driver::register("virtio-net", crate::driver::DriverKind::Serial);
            return;
        }
    }
    serial_println!("[virtio-net] no device found");
}

/// Check if detected.
pub fn is_detected() -> bool {
    unsafe { VIRTIO_NET.as_ref().map(|d| d.detected).unwrap_or(false) }
}

/// Device info string.
pub fn info() -> String {
    if let Some(dev) = unsafe { VIRTIO_NET.as_ref() } {
        alloc::format!("virtio-net: {} ({})", dev.mac, "detected")
    } else {
        alloc::format!("virtio-net: not detected")
    }
}
