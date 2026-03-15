/// Virtio-net driver — real Ethernet frame TX/RX through QEMU.
///
/// Uses legacy virtio PCI transport with two virtqueues:
///   Queue 0: RX (device → guest)
///   Queue 1: TX (guest → device)
///
/// Requires QEMU: -netdev user,id=n0 -device virtio-net-pci,netdev=n0

use crate::{pci, virtio, serial_println, klog_println, memory, net};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use x86_64::instructions::port::Port;
use core::sync::atomic::{AtomicBool, Ordering};

const QUEUE_SIZE: usize = 16;

// Virtio legacy I/O port offsets
const REG_DEVICE_FEATURES: u16 = 0;
const REG_GUEST_FEATURES: u16 = 4;
const REG_QUEUE_ADDR: u16 = 8;
const REG_QUEUE_SIZE: u16 = 12;
const REG_QUEUE_SELECT: u16 = 14;
const REG_QUEUE_NOTIFY: u16 = 16;
const REG_DEVICE_STATUS: u16 = 18;

// Virtio-net config: MAC address at offset 0x14
const REG_MAC: u16 = 0x14;

/// Virtqueue descriptor.
#[repr(C)]
#[derive(Clone, Copy)]
struct VqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C)]
struct VqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; QUEUE_SIZE],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VqUsedElem { id: u32, len: u32 }

#[repr(C)]
struct VqUsed {
    flags: u16,
    idx: u16,
    ring: [VqUsedElem; QUEUE_SIZE],
}

/// Virtio-net header prepended to every frame.
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioNetHdr {
    flags: u8,
    gso_type: u8,
    hdr_len: u16,
    gso_size: u16,
    csum_start: u16,
    csum_offset: u16,
}

impl VirtioNetHdr {
    fn empty() -> Self {
        Self { flags: 0, gso_type: 0, hdr_len: 0, gso_size: 0, csum_start: 0, csum_offset: 0 }
    }
}

struct NetDevice {
    io_base: u16,
    mac: [u8; 6],
    // TX queue
    tx_descs: *mut VqDesc,
    tx_avail: *mut VqAvail,
    tx_used: *mut VqUsed,
    tx_avail_idx: u16,
    // RX queue
    rx_descs: *mut VqDesc,
    rx_avail: *mut VqAvail,
    rx_used: *mut VqUsed,
    rx_last_used: u16,
    // RX buffers (heap-allocated so virt_to_phys works with offset mapping)
    rx_bufs: *mut [[u8; 2048]; QUEUE_SIZE],
    initialized: bool,
}

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static mut DEVICE: Option<NetDevice> = None;

pub fn init() {
    let devices = virtio::scan();
    let net_dev = devices.iter().find(|d| d.device_type == virtio::VirtioDeviceType::Network);

    let dev = match net_dev {
        Some(d) => d,
        None => {
            serial_println!("[virtio-net] no device found");
            return;
        }
    };

    serial_println!("[virtio-net] found {}", dev.summary());

    let bar0 = pci::pci_read32(dev.pci.bus, dev.pci.device, dev.pci.function, 0x10);
    if bar0 & 1 == 0 {
        serial_println!("[virtio-net] BAR0 is MMIO — unsupported");
        return;
    }
    let io_base = (bar0 & 0xFFFC) as u16;
    serial_println!("[virtio-net] I/O base: {:#x}", io_base);

    unsafe {
        // Reset
        write_reg8(io_base, REG_DEVICE_STATUS, 0);
        write_reg8(io_base, REG_DEVICE_STATUS, virtio::VIRTIO_STATUS_ACKNOWLEDGE);
        write_reg8(io_base, REG_DEVICE_STATUS,
            virtio::VIRTIO_STATUS_ACKNOWLEDGE | virtio::VIRTIO_STATUS_DRIVER);

        // Accept no features for simplicity
        let _features = read_reg32(io_base, REG_DEVICE_FEATURES);
        write_reg32(io_base, REG_GUEST_FEATURES, 0);

        // Read MAC
        let mut mac = [0u8; 6];
        for i in 0..6 {
            mac[i] = Port::<u8>::new(io_base + REG_MAC + i as u16).read();
        }
        serial_println!("[virtio-net] MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

        // Allocate RX queue (0)
        let (rx_descs, rx_avail, rx_used) = alloc_virtqueue(io_base, 0);
        // Allocate TX queue (1)
        let (tx_descs, tx_avail, tx_used) = alloc_virtqueue(io_base, 1);

        // Allocate RX buffers from physical frames (for DMA).
        // Each buffer is 2048 bytes, we need QUEUE_SIZE of them.
        // Allocate 8 pages (32K) to hold all buffers.
        let rx_frame = memory::alloc_frame().expect("no frame for RX bufs");
        let rx_bufs_virt = memory::phys_to_virt(rx_frame.start_address()).as_u64();
        // Zero it
        core::ptr::write_bytes(rx_bufs_virt as *mut u8, 0, 4096);
        // We'll use the first page for the first few buffers.
        // For simplicity, allocate additional frames for more buffers.
        let mut rx_buf_phys = [0u64; QUEUE_SIZE];
        for i in 0..QUEUE_SIZE {
            let frame = memory::alloc_frame().expect("no frame for RX buf");
            let virt = memory::phys_to_virt(frame.start_address()).as_u64();
            core::ptr::write_bytes(virt as *mut u8, 0, 2048.min(4096));
            rx_buf_phys[i] = frame.start_address().as_u64();
        }
        let rx_bufs = core::ptr::null_mut(); // not using struct-level bufs anymore

        let device = NetDevice {
            io_base, mac,
            tx_descs, tx_avail, tx_used, tx_avail_idx: 0,
            rx_descs, rx_avail, rx_used, rx_last_used: 0,
            rx_bufs,
            initialized: true,
        };

        // Pre-populate RX queue with buffers (physical addresses)
        for i in 0..QUEUE_SIZE {
            let buf_phys = rx_buf_phys[i];
            (*rx_descs.add(i)) = VqDesc {
                addr: buf_phys,
                len: 2048,
                flags: virtio::VIRTQ_DESC_F_WRITE,
                next: 0,
            };
            let avail = &mut *rx_avail;
            avail.ring[i] = i as u16;
        }
        (*rx_avail).idx = QUEUE_SIZE as u16;

        // Notify RX queue
        write_reg16(io_base, REG_QUEUE_NOTIFY, 0);

        // Driver OK
        write_reg8(io_base, REG_DEVICE_STATUS,
            virtio::VIRTIO_STATUS_ACKNOWLEDGE
            | virtio::VIRTIO_STATUS_DRIVER
            | virtio::VIRTIO_STATUS_DRIVER_OK);

        DEVICE = Some(device);
        INITIALIZED.store(true, Ordering::SeqCst);
    }

    // Update the network state with real MAC
    {
        let mut n = net::NET.lock();
        let mac = unsafe { DEVICE.as_ref().unwrap().mac };
        n.mac = net::MacAddr(mac);
    }

    serial_println!("[virtio-net] driver initialized");
    klog_println!("[virtio-net] initialized");
    crate::driver::register("virtio-net", crate::driver::DriverKind::Serial);
}

/// Send a raw Ethernet frame.
pub fn send_frame(frame: &[u8]) -> Result<(), &'static str> {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return Err("virtio-net not initialized");
    }

    unsafe {
        let dev = DEVICE.as_mut().unwrap();

        // Allocate a physical frame for the TX packet (DMA-safe)
        let hdr = VirtioNetHdr::empty();
        let desc_idx = (dev.tx_avail_idx as usize) % QUEUE_SIZE;

        let tx_frame = memory::alloc_frame().ok_or("no frame for TX")?;
        let tx_virt = memory::phys_to_virt(tx_frame.start_address()).as_u64() as *mut u8;
        let pkt_len = 10 + frame.len(); // virtio-net header + ethernet frame

        // Copy header + frame into the physical page
        let hdr_bytes = &hdr as *const VirtioNetHdr as *const u8;
        core::ptr::copy_nonoverlapping(hdr_bytes, tx_virt, 10);
        core::ptr::copy_nonoverlapping(frame.as_ptr(), tx_virt.add(10), frame.len());

        let pkt_phys = tx_frame.start_address().as_u64();

        (*dev.tx_descs.add(desc_idx)) = VqDesc {
            addr: pkt_phys,
            len: pkt_len as u32,
            flags: 0,
            next: 0,
        };

        let avail = &mut *dev.tx_avail;
        avail.ring[(dev.tx_avail_idx as usize) % QUEUE_SIZE] = desc_idx as u16;
        core::sync::atomic::fence(Ordering::SeqCst);
        avail.idx = dev.tx_avail_idx.wrapping_add(1);
        dev.tx_avail_idx = avail.idx;
        core::sync::atomic::fence(Ordering::SeqCst);

        // Notify TX queue
        write_reg16(dev.io_base, REG_QUEUE_NOTIFY, 1);

        // Update stats
        let mut n = net::NET.lock();
        n.tx_packets += 1;
        n.tx_bytes += frame.len() as u64;
    }

    Ok(())
}

/// Build and send an ARP request.
pub fn send_arp_request(target_ip: net::Ipv4Addr) -> Result<(), &'static str> {
    let dev = unsafe { DEVICE.as_ref().ok_or("not initialized")? };
    let src_mac = dev.mac;
    let src_ip = net::NET.lock().ip;

    let mut frame = [0u8; 42]; // 14 ethernet + 28 ARP

    // Ethernet header
    frame[0..6].copy_from_slice(&[0xFF; 6]); // dst: broadcast
    frame[6..12].copy_from_slice(&src_mac);   // src: our MAC
    frame[12..14].copy_from_slice(&net::ETH_TYPE_ARP.to_be_bytes());

    // ARP
    frame[14..16].copy_from_slice(&1u16.to_be_bytes()); // hw type: ethernet
    frame[16..18].copy_from_slice(&0x0800u16.to_be_bytes()); // proto: IPv4
    frame[18] = 6;  // hw addr len
    frame[19] = 4;  // proto addr len
    frame[20..22].copy_from_slice(&1u16.to_be_bytes()); // op: request
    frame[22..28].copy_from_slice(&src_mac);        // sender MAC
    frame[28..32].copy_from_slice(&src_ip.0);       // sender IP
    frame[32..38].copy_from_slice(&[0; 6]);         // target MAC (unknown)
    frame[38..42].copy_from_slice(&target_ip.0);    // target IP

    serial_println!("[virtio-net] ARP who-has {} tell {}", target_ip, src_ip);
    send_frame(&frame)
}

/// Build and send an ICMP echo request (ping).
pub fn send_ping(target_ip: net::Ipv4Addr, seq: u16) -> Result<(), &'static str> {
    let dev = unsafe { DEVICE.as_ref().ok_or("not initialized")? };
    let src_mac = dev.mac;
    let src_ip = net::NET.lock().ip;

    // For simplicity, use broadcast MAC (QEMU user-net will handle it)
    let dst_mac = [0xFF; 6];

    let mut frame = [0u8; 98]; // 14 eth + 20 IP + 64 ICMP (padded)

    // Ethernet header
    frame[0..6].copy_from_slice(&dst_mac);
    frame[6..12].copy_from_slice(&src_mac);
    frame[12..14].copy_from_slice(&net::ETH_TYPE_IP.to_be_bytes());

    // IPv4 header (20 bytes)
    let ip_total_len: u16 = 20 + 8 + 56; // IP + ICMP header + data
    frame[14] = 0x45;  // version=4, IHL=5
    frame[15] = 0;     // DSCP/ECN
    frame[16..18].copy_from_slice(&ip_total_len.to_be_bytes());
    frame[18..20].copy_from_slice(&1u16.to_be_bytes()); // identification
    frame[20..22].copy_from_slice(&0u16.to_be_bytes()); // flags/fragment
    frame[22] = 64;    // TTL
    frame[23] = 1;     // protocol: ICMP
    frame[24..26].copy_from_slice(&0u16.to_be_bytes()); // checksum (filled later)
    frame[26..30].copy_from_slice(&src_ip.0);
    frame[30..34].copy_from_slice(&target_ip.0);

    // IP checksum
    let ip_cksum = ip_checksum(&frame[14..34]);
    frame[24..26].copy_from_slice(&ip_cksum.to_be_bytes());

    // ICMP echo request
    frame[34] = 8;     // type: echo request
    frame[35] = 0;     // code
    frame[36..38].copy_from_slice(&0u16.to_be_bytes()); // checksum (filled later)
    frame[38..40].copy_from_slice(&0x1234u16.to_be_bytes()); // identifier
    frame[40..42].copy_from_slice(&seq.to_be_bytes()); // sequence

    // ICMP checksum
    let icmp_cksum = ip_checksum(&frame[34..98]);
    frame[36..38].copy_from_slice(&icmp_cksum.to_be_bytes());

    serial_println!("[virtio-net] ICMP echo → {} seq={}", target_ip, seq);
    send_frame(&frame)
}

/// Compute IP/ICMP checksum (ones' complement sum).
fn ip_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

unsafe fn alloc_virtqueue(io_base: u16, queue_idx: u16) -> (*mut VqDesc, *mut VqAvail, *mut VqUsed) {
    write_reg16(io_base, REG_QUEUE_SELECT, queue_idx);
    let _queue_size_max = read_reg16(io_base, REG_QUEUE_SIZE);

    let vq_size = core::mem::size_of::<VqDesc>() * QUEUE_SIZE
        + core::mem::size_of::<VqAvail>()
        + core::mem::size_of::<VqUsed>();
    let vq_mem = vec![0u8; vq_size + 8192];
    let vq_ptr = vq_mem.as_ptr() as usize;
    let vq_aligned = (vq_ptr + 4095) & !4095;
    core::mem::forget(vq_mem);

    let descs = vq_aligned as *mut VqDesc;
    let avail = (vq_aligned + core::mem::size_of::<VqDesc>() * QUEUE_SIZE) as *mut VqAvail;
    let used_offset = vq_aligned + core::mem::size_of::<VqDesc>() * QUEUE_SIZE
        + core::mem::size_of::<VqAvail>();
    let used_aligned = (used_offset + 4095) & !4095;
    let used = used_aligned as *mut VqUsed;

    let phys_addr = vq_aligned as u64 - memory::phys_mem_offset().as_u64();
    let queue_pfn = (phys_addr / 4096) as u32;
    write_reg32(io_base, REG_QUEUE_ADDR, queue_pfn);

    (descs, avail, used)
}

fn virt_to_phys(virt: u64) -> u64 {
    let offset = memory::phys_mem_offset().as_u64();
    // The heap (0x4444_4444_0000) is mapped via page tables that point
    // to physical frames. For the bootloader's identity-mapped region,
    // phys = virt - offset. For heap addresses, we need to walk the
    // page table. As a workaround, we wrapping_sub to handle the math.
    virt.wrapping_sub(offset)
}

unsafe fn read_reg32(base: u16, offset: u16) -> u32 { Port::<u32>::new(base + offset).read() }
unsafe fn read_reg16(base: u16, offset: u16) -> u16 { Port::<u16>::new(base + offset).read() }
unsafe fn write_reg32(base: u16, offset: u16, val: u32) { Port::<u32>::new(base + offset).write(val); }
unsafe fn write_reg16(base: u16, offset: u16, val: u16) { Port::<u16>::new(base + offset).write(val); }
unsafe fn write_reg8(base: u16, offset: u16, val: u8) { Port::<u8>::new(base + offset).write(val); }

pub fn is_detected() -> bool { INITIALIZED.load(Ordering::SeqCst) }

pub fn info() -> String {
    if let Some(dev) = unsafe { DEVICE.as_ref() } {
        let mac = dev.mac;
        alloc::format!("virtio-net: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, ready",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5])
    } else {
        alloc::format!("virtio-net: not detected")
    }
}
