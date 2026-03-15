/// Virtio-blk driver — real block I/O through QEMU.
///
/// Uses the virtio legacy (transitional) PCI interface:
/// - PCI BAR0 provides I/O port base for device registers
/// - Single virtqueue (index 0) for block requests
/// - Each request: header (type+sector) → data → status byte
///
/// Requires QEMU flag: -drive file=disk.img,format=raw,if=virtio

use crate::{pci, virtio, serial_println, klog_println, memory};
use alloc::string::String;
use alloc::vec;
use x86_64::instructions::port::Port;
use core::sync::atomic::{AtomicBool, Ordering};

// Virtio legacy I/O port offsets (relative to BAR0)
const REG_DEVICE_FEATURES: u16 = 0;
const REG_GUEST_FEATURES: u16 = 4;
const REG_QUEUE_ADDR: u16 = 8;
const REG_QUEUE_SIZE: u16 = 12;
const REG_QUEUE_SELECT: u16 = 14;
const REG_QUEUE_NOTIFY: u16 = 16;
const REG_DEVICE_STATUS: u16 = 18;

// Virtio-blk config space (offset from BAR0 + 0x14)
const REG_BLK_CAPACITY: u16 = 0x14; // u64: capacity in 512-byte sectors

// Virtqueue size (must be power of 2)
const QUEUE_SIZE: usize = 16;

// Virtio-blk request types
const VIRTIO_BLK_T_IN: u32 = 0;   // read
const VIRTIO_BLK_T_OUT: u32 = 1;  // write

/// Virtio-blk request header.
#[repr(C)]
struct VirtioBlkReqHeader {
    type_: u32,
    reserved: u32,
    sector: u64,
}

/// Virtqueue descriptor.
#[repr(C)]
#[derive(Clone, Copy)]
struct VqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

/// Virtqueue available ring.
#[repr(C)]
struct VqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; QUEUE_SIZE],
}

/// Virtqueue used ring entry.
#[repr(C)]
#[derive(Clone, Copy)]
struct VqUsedElem {
    id: u32,
    len: u32,
}

/// Virtqueue used ring.
#[repr(C)]
struct VqUsed {
    flags: u16,
    idx: u16,
    ring: [VqUsedElem; QUEUE_SIZE],
}

/// Complete device state.
struct BlkDevice {
    io_base: u16,
    capacity: u64,
    // Virtqueue memory (allocated on heap, aligned)
    descs: *mut VqDesc,
    avail: *mut VqAvail,
    used: *mut VqUsed,
    // Tracking
    free_desc: u16,
    last_used_idx: u16,
    initialized: bool,
}

static INITIALIZED: AtomicBool = AtomicBool::new(false);

// Device state — single global instance
static mut DEVICE: BlkDevice = BlkDevice {
    io_base: 0,
    capacity: 0,
    descs: core::ptr::null_mut(),
    avail: core::ptr::null_mut(),
    used: core::ptr::null_mut(),
    free_desc: 0,
    last_used_idx: 0,
    initialized: false,
};

/// Initialize the virtio-blk driver.
pub fn init() {
    let devices = virtio::scan();
    let blk_dev = devices.iter().find(|d| d.device_type == virtio::VirtioDeviceType::Block);

    let dev = match blk_dev {
        Some(d) => d,
        None => {
            serial_println!("[virtio-blk] no device found (add -drive file=disk.img,format=raw,if=virtio)");
            return;
        }
    };

    serial_println!("[virtio-blk] found {}", dev.summary());

    // Read PCI BAR0 to get I/O port base
    let bar0 = pci::pci_read32(dev.pci.bus, dev.pci.device, dev.pci.function, 0x10);
    if bar0 & 1 == 0 {
        serial_println!("[virtio-blk] BAR0 is MMIO, not I/O port — unsupported");
        return;
    }
    let io_base = (bar0 & 0xFFFC) as u16;
    serial_println!("[virtio-blk] I/O base: {:#x}", io_base);

    unsafe {
        DEVICE.io_base = io_base;

        // 1. Reset device
        write_reg8(io_base, REG_DEVICE_STATUS, 0);

        // 2. Acknowledge
        write_reg8(io_base, REG_DEVICE_STATUS, virtio::VIRTIO_STATUS_ACKNOWLEDGE);

        // 3. Driver
        write_reg8(io_base, REG_DEVICE_STATUS,
            virtio::VIRTIO_STATUS_ACKNOWLEDGE | virtio::VIRTIO_STATUS_DRIVER);

        // 4. Read device features and accept none (keep it simple)
        let _features = read_reg32(io_base, REG_DEVICE_FEATURES);
        write_reg32(io_base, REG_GUEST_FEATURES, 0);

        // 5. Read capacity
        let cap_lo = read_reg32(io_base, REG_BLK_CAPACITY) as u64;
        let cap_hi = read_reg32(io_base, REG_BLK_CAPACITY + 4) as u64;
        DEVICE.capacity = cap_lo | (cap_hi << 32);
        serial_println!("[virtio-blk] capacity: {} sectors ({} KiB)",
            DEVICE.capacity, DEVICE.capacity / 2);

        // 6. Set up virtqueue 0
        write_reg16(io_base, REG_QUEUE_SELECT, 0);
        let queue_size_max = read_reg16(io_base, REG_QUEUE_SIZE);
        serial_println!("[virtio-blk] queue max size: {}", queue_size_max);

        if queue_size_max == 0 {
            serial_println!("[virtio-blk] queue not available");
            return;
        }

        // Allocate virtqueue memory (needs to be physically contiguous)
        // Use alloc for simplicity — works because our heap is identity-mapped
        // via the bootloader's physical memory offset.
        let vq_size = core::mem::size_of::<VqDesc>() * QUEUE_SIZE
            + core::mem::size_of::<VqAvail>()
            + core::mem::size_of::<VqUsed>();
        let vq_mem = vec![0u8; vq_size + 4096]; // extra for alignment
        let vq_ptr = vq_mem.as_ptr() as usize;
        let vq_aligned = (vq_ptr + 4095) & !4095; // page-align
        core::mem::forget(vq_mem); // leak — lives forever

        let descs = vq_aligned as *mut VqDesc;
        let avail = (vq_aligned + core::mem::size_of::<VqDesc>() * QUEUE_SIZE) as *mut VqAvail;
        let used_offset = vq_aligned + core::mem::size_of::<VqDesc>() * QUEUE_SIZE
            + core::mem::size_of::<VqAvail>();
        let used_aligned = (used_offset + 4095) & !4095;
        let used = used_aligned as *mut VqUsed;

        DEVICE.descs = descs;
        DEVICE.avail = avail;
        DEVICE.used = used;

        // Convert virtual address to physical for the device
        let phys_addr = vq_aligned as u64 - memory::phys_mem_offset().as_u64();
        let queue_pfn = (phys_addr / 4096) as u32;

        write_reg32(io_base, REG_QUEUE_ADDR, queue_pfn);

        // 7. Mark driver OK
        write_reg8(io_base, REG_DEVICE_STATUS,
            virtio::VIRTIO_STATUS_ACKNOWLEDGE
            | virtio::VIRTIO_STATUS_DRIVER
            | virtio::VIRTIO_STATUS_DRIVER_OK);

        DEVICE.initialized = true;
        INITIALIZED.store(true, Ordering::SeqCst);
    }

    serial_println!("[virtio-blk] driver initialized, ready for I/O");
    klog_println!("[virtio-blk] initialized, {} sectors", unsafe { DEVICE.capacity });

    crate::blkdev::register("vda", unsafe { DEVICE.capacity });
    crate::driver::register("virtio-blk", crate::driver::DriverKind::Block);
}

/// Read a 512-byte sector from disk.
pub fn read_sector(sector: u64, buf: &mut [u8; 512]) -> Result<(), &'static str> {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return Err("virtio-blk not initialized");
    }
    do_request(VIRTIO_BLK_T_IN, sector, buf)
}

/// Write a 512-byte sector to disk.
pub fn write_sector(sector: u64, buf: &[u8; 512]) -> Result<(), &'static str> {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return Err("virtio-blk not initialized");
    }
    let mut tmp = *buf;
    do_request(VIRTIO_BLK_T_OUT, sector, &mut tmp)
}

/// Submit a block I/O request via the virtqueue.
fn do_request(type_: u32, sector: u64, buf: &mut [u8; 512]) -> Result<(), &'static str> {
    unsafe {
        let io_base = DEVICE.io_base;

        // Build request header
        let mut header = VirtioBlkReqHeader {
            type_,
            reserved: 0,
            sector,
        };
        let mut status: u8 = 0xFF;

        // Set up descriptor chain: header → data → status
        let d0 = 0u16; // descriptor for header
        let d1 = 1u16; // descriptor for data
        let d2 = 2u16; // descriptor for status

        let header_phys = virt_to_phys(&header as *const _ as u64);
        let data_phys = virt_to_phys(buf.as_ptr() as u64);
        let status_phys = virt_to_phys(&status as *const _ as u64);

        // Descriptor 0: header (device reads)
        (*DEVICE.descs.add(0)) = VqDesc {
            addr: header_phys,
            len: core::mem::size_of::<VirtioBlkReqHeader>() as u32,
            flags: virtio::VIRTQ_DESC_F_NEXT,
            next: d1,
        };

        // Descriptor 1: data buffer
        let data_flags = if type_ == VIRTIO_BLK_T_IN {
            virtio::VIRTQ_DESC_F_NEXT | virtio::VIRTQ_DESC_F_WRITE // device writes to buf
        } else {
            virtio::VIRTQ_DESC_F_NEXT // device reads from buf
        };
        (*DEVICE.descs.add(1)) = VqDesc {
            addr: data_phys,
            len: 512,
            flags: data_flags,
            next: d2,
        };

        // Descriptor 2: status byte (device writes)
        (*DEVICE.descs.add(2)) = VqDesc {
            addr: status_phys,
            len: 1,
            flags: virtio::VIRTQ_DESC_F_WRITE,
            next: 0,
        };

        // Add to available ring
        let avail = &mut *DEVICE.avail;
        let avail_idx = avail.idx;
        avail.ring[(avail_idx as usize) % QUEUE_SIZE] = d0;
        core::sync::atomic::fence(Ordering::SeqCst);
        avail.idx = avail_idx.wrapping_add(1);
        core::sync::atomic::fence(Ordering::SeqCst);

        // Notify device
        write_reg16(io_base, REG_QUEUE_NOTIFY, 0);

        // Poll for completion (wait for used ring to advance)
        let deadline = crate::timer::ticks() + crate::timer::PIT_FREQUENCY_HZ; // 1 sec
        loop {
            let used = &*DEVICE.used;
            if used.idx != DEVICE.last_used_idx {
                DEVICE.last_used_idx = used.idx;
                break;
            }
            if crate::timer::ticks() > deadline {
                return Err("virtio-blk: I/O timeout");
            }
            core::hint::spin_loop();
        }

        if status == 0 {
            Ok(())
        } else {
            Err("virtio-blk: I/O error")
        }
    }
}

fn virt_to_phys(virt: u64) -> u64 {
    virt - memory::phys_mem_offset().as_u64()
}

unsafe fn read_reg32(base: u16, offset: u16) -> u32 {
    Port::<u32>::new(base + offset).read()
}
unsafe fn read_reg16(base: u16, offset: u16) -> u16 {
    Port::<u16>::new(base + offset).read()
}
unsafe fn write_reg32(base: u16, offset: u16, val: u32) {
    Port::<u32>::new(base + offset).write(val);
}
unsafe fn write_reg16(base: u16, offset: u16, val: u16) {
    Port::<u16>::new(base + offset).write(val);
}
unsafe fn write_reg8(base: u16, offset: u16, val: u8) {
    Port::<u8>::new(base + offset).write(val);
}

pub fn is_detected() -> bool {
    INITIALIZED.load(Ordering::SeqCst)
}

pub fn capacity() -> u64 {
    unsafe { DEVICE.capacity }
}

pub fn info() -> String {
    if is_detected() {
        alloc::format!("virtio-blk: {} sectors ({} KiB), ready",
            capacity(), capacity() / 2)
    } else {
        alloc::format!("virtio-blk: not detected")
    }
}
