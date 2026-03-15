/// Virtio-blk driver stub.
/// Provides the interface for real block I/O through QEMU's virtio-blk
/// device. Currently detects the device on PCI and reports its presence.
/// Full I/O requires virtqueue setup with DMA-capable memory regions.

use crate::{pci, virtio, serial_println, klog_println};
use alloc::string::String;

/// Virtio-blk device state.
pub struct VirtioBlkDevice {
    pub pci_device: pci::PciDevice,
    pub capacity_sectors: u64,
    pub detected: bool,
}

static mut VIRTIO_BLK: Option<VirtioBlkDevice> = None;

/// Probe PCI for a virtio-blk device.
pub fn init() {
    let devices = virtio::scan();
    for dev in &devices {
        if dev.device_type == virtio::VirtioDeviceType::Block {
            serial_println!("[virtio-blk] found {} ", dev.summary());
            klog_println!("[virtio-blk] detected on PCI");

            unsafe {
                VIRTIO_BLK = Some(VirtioBlkDevice {
                    pci_device: dev.pci.clone(),
                    capacity_sectors: 0, // would read from device config
                    detected: true,
                });
            }

            crate::blkdev::register("vda", 2048); // 1 MiB virtual disk
            crate::driver::register("virtio-blk", crate::driver::DriverKind::Block);
            return;
        }
    }
    serial_println!("[virtio-blk] no device found (add -drive to QEMU)");
}

/// Check if a virtio-blk device was detected.
pub fn is_detected() -> bool {
    unsafe { VIRTIO_BLK.as_ref().map(|d| d.detected).unwrap_or(false) }
}

/// Device info string.
pub fn info() -> String {
    if is_detected() {
        alloc::format!("virtio-blk: detected (vda)")
    } else {
        alloc::format!("virtio-blk: not detected")
    }
}
