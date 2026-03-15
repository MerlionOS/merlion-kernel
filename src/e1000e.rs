/// Intel e1000e Ethernet NIC driver for MerlionOS.
///
/// Supports Intel 82540EM (e1000) and e1000e family controllers commonly
/// emulated by QEMU and found in real hardware. Communicates with the NIC
/// via memory-mapped I/O (BAR0) and uses ring-buffer descriptor queues for
/// transmit and receive paths.
///
/// Tested device IDs:
///   - 0x100E  Intel 82540EM (QEMU default e1000)
///   - 0x10D3  Intel 82574L
///   - 0x153A  Intel I217-LM

use crate::{pci, memory, net, serial_println};
use alloc::vec::Vec;
use alloc::string::String;
use core::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Register offsets
// ---------------------------------------------------------------------------

/// Device Control Register
const REG_CTRL: u32 = 0x0000;
/// Device Status Register
const REG_STATUS: u32 = 0x0008;
/// EEPROM Read Register
const REG_EERD: u32 = 0x0014;
/// Interrupt Cause Read
const REG_ICR: u32 = 0x00C0;
/// Interrupt Mask Set/Read
const REG_IMS: u32 = 0x00D0;
/// Interrupt Mask Clear
const REG_IMC: u32 = 0x00D8;
/// Receive Control Register
const REG_RCTL: u32 = 0x0100;
/// Transmit Control Register
const REG_TCTL: u32 = 0x0400;
/// Receive Descriptor Base Address Low
const REG_RDBAL: u32 = 0x2800;
/// Receive Descriptor Base Address High
const REG_RDBAH: u32 = 0x2804;
/// Receive Descriptor Length (bytes)
const REG_RDLEN: u32 = 0x2808;
/// Receive Descriptor Head
const REG_RDH: u32 = 0x2810;
/// Receive Descriptor Tail
const REG_RDT: u32 = 0x2818;
/// Transmit Descriptor Base Address Low
const REG_TDBAL: u32 = 0x3800;
/// Transmit Descriptor Base Address High
const REG_TDBAH: u32 = 0x3804;
/// Transmit Descriptor Length (bytes)
const REG_TDLEN: u32 = 0x3808;
/// Transmit Descriptor Head
const REG_TDH: u32 = 0x3810;
/// Transmit Descriptor Tail
const REG_TDT: u32 = 0x3818;
/// Receive Address Low (MAC bytes 0-3)
const REG_RAL: u32 = 0x5400;
/// Receive Address High (MAC bytes 4-5 + AV bit)
const REG_RAH: u32 = 0x5404;

// CTRL register bits
const CTRL_SLU: u32 = 1 << 6;   // Set Link Up
const CTRL_RST: u32 = 1 << 26;  // Device Reset

// RCTL register bits
const RCTL_EN: u32 = 1 << 1;       // Receiver Enable
const RCTL_SBP: u32 = 1 << 2;      // Store Bad Packets
const RCTL_UPE: u32 = 1 << 3;      // Unicast Promiscuous
const RCTL_MPE: u32 = 1 << 4;      // Multicast Promiscuous
const RCTL_BAM: u32 = 1 << 15;     // Broadcast Accept Mode
const RCTL_BSIZE_2048: u32 = 0;    // Buffer Size 2048 (default)
const RCTL_SECRC: u32 = 1 << 26;   // Strip Ethernet CRC

// TCTL register bits
const TCTL_EN: u32 = 1 << 1;       // Transmit Enable
const TCTL_PSP: u32 = 1 << 3;      // Pad Short Packets
const TCTL_CT_SHIFT: u32 = 4;      // Collision Threshold shift
const TCTL_COLD_SHIFT: u32 = 12;   // Collision Distance shift

// TX descriptor command bits
const TXCMD_EOP: u8 = 1 << 0;  // End Of Packet
const TXCMD_IFCS: u8 = 1 << 1; // Insert FCS/CRC
const TXCMD_RS: u8 = 1 << 3;   // Report Status

// TX descriptor status bits
const TXSTAT_DD: u8 = 1 << 0;  // Descriptor Done

// RX descriptor status bits
const RXSTAT_DD: u8 = 1 << 0;  // Descriptor Done
const RXSTAT_EOP: u8 = 1 << 1; // End Of Packet

/// Number of descriptors per ring (must be multiple of 8, aligned to 128).
const RING_SIZE: usize = 32;

/// Maximum Ethernet frame size for receive buffers.
const RX_BUF_SIZE: usize = 2048;

/// Supported Intel device IDs.
const SUPPORTED_DEVICES: &[(u16, &str)] = &[
    (0x100E, "82540EM (e1000)"),
    (0x10D3, "82574L (e1000e)"),
    (0x153A, "I217-LM (e1000e)"),
];

const INTEL_VENDOR_ID: u16 = 0x8086;

// ---------------------------------------------------------------------------
// Descriptor structures
// ---------------------------------------------------------------------------

/// Transmit descriptor (legacy format, 16 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
struct TxDesc {
    addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

impl TxDesc {
    const fn zero() -> Self {
        Self { addr: 0, length: 0, cso: 0, cmd: 0, status: 0, css: 0, special: 0 }
    }
}

/// Receive descriptor (legacy format, 16 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
struct RxDesc {
    addr: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

impl RxDesc {
    const fn zero() -> Self {
        Self { addr: 0, length: 0, checksum: 0, status: 0, errors: 0, special: 0 }
    }
}

// ---------------------------------------------------------------------------
// Device state
// ---------------------------------------------------------------------------

struct E1000eDevice {
    mmio_base: u64,
    mac: [u8; 6],
    tx_ring: *mut TxDesc,
    rx_ring: *mut RxDesc,
    rx_bufs: *mut [[u8; RX_BUF_SIZE]; RING_SIZE],
    tx_cur: usize,
    rx_cur: usize,
}

unsafe impl Send for E1000eDevice {}

static DETECTED: AtomicBool = AtomicBool::new(false);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static mut DEVICE: Option<E1000eDevice> = None;

// ---------------------------------------------------------------------------
// MMIO helpers
// ---------------------------------------------------------------------------

/// Read a 32-bit register from the NIC's MMIO region.
#[inline]
fn mmio_read(base: u64, reg: u32) -> u32 {
    unsafe {
        let ptr = (base + reg as u64) as *const u32;
        core::ptr::read_volatile(ptr)
    }
}

/// Write a 32-bit register in the NIC's MMIO region.
#[inline]
fn mmio_write(base: u64, reg: u32, val: u32) {
    unsafe {
        let ptr = (base + reg as u64) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }
}

// ---------------------------------------------------------------------------
// MAC address
// ---------------------------------------------------------------------------

/// Read the MAC address from the RAL/RAH registers.
///
/// The NIC stores the station address in RAL (bytes 0-3, little-endian) and
/// RAH (bytes 4-5 in the low 16 bits). This is populated from EEPROM by the
/// hardware during power-on reset.
pub fn read_mac(mmio_base: u64) -> [u8; 6] {
    let ral = mmio_read(mmio_base, REG_RAL);
    let rah = mmio_read(mmio_base, REG_RAH);
    [
        (ral & 0xFF) as u8,
        ((ral >> 8) & 0xFF) as u8,
        ((ral >> 16) & 0xFF) as u8,
        ((ral >> 24) & 0xFF) as u8,
        (rah & 0xFF) as u8,
        ((rah >> 8) & 0xFF) as u8,
    ]
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Scan PCI for an Intel e1000/e1000e NIC and initialise it.
///
/// Performs a full device reset, reads the MAC address, and sets up the TX
/// and RX descriptor rings with pre-allocated buffers. After init the device
/// is ready for `send_frame()` and `recv_frame()`.
pub fn init() {
    let devices = pci::scan();
    let nic = devices.iter().find(|d| {
        d.vendor_id == INTEL_VENDOR_ID
            && SUPPORTED_DEVICES.iter().any(|&(did, _)| did == d.device_id)
    });

    let nic = match nic {
        Some(d) => d.clone(),
        None => return,
    };

    DETECTED.store(true, Ordering::SeqCst);

    let dev_name = SUPPORTED_DEVICES
        .iter()
        .find(|&&(did, _)| did == nic.device_id)
        .map(|&(_, name)| name)
        .unwrap_or("unknown");

    serial_println!("[e1000e] Found Intel {} (device {:04x}) at {:02x}:{:02x}.{}",
        dev_name, nic.device_id, nic.bus, nic.device, nic.function);

    // Read BAR0 (Memory-mapped I/O base address)
    let bar0_raw = pci::pci_read32(nic.bus, nic.device, nic.function, 0x10);
    let bar0_phys = (bar0_raw & 0xFFFF_FFF0) as u64;

    // If 64-bit BAR, read the upper 32 bits
    let bar0_phys = if bar0_raw & 0x04 != 0 {
        let bar1 = pci::pci_read32(nic.bus, nic.device, nic.function, 0x14) as u64;
        bar0_phys | (bar1 << 32)
    } else {
        bar0_phys
    };

    let mmio_base = memory::phys_to_virt(x86_64::PhysAddr::new(bar0_phys)).as_u64();

    // Enable PCI bus-mastering (command register bit 2)
    let cmd = pci::pci_read32(nic.bus, nic.device, nic.function, 0x04);
    pci::pci_write32(nic.bus, nic.device, nic.function, 0x04, cmd | (1 << 2));

    // Reset the device
    mmio_write(mmio_base, REG_CTRL, mmio_read(mmio_base, REG_CTRL) | CTRL_RST);
    // Spin briefly while reset completes
    for _ in 0..10_000 {
        core::hint::spin_loop();
    }

    // Disable interrupts (we poll)
    mmio_write(mmio_base, REG_IMC, 0xFFFF_FFFF);
    mmio_read(mmio_base, REG_ICR); // clear pending

    // Set link up
    let ctrl = mmio_read(mmio_base, REG_CTRL);
    mmio_write(mmio_base, REG_CTRL, ctrl | CTRL_SLU);

    // Read MAC
    let mac = read_mac(mmio_base);
    serial_println!("[e1000e] MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

    // ------ Allocate and initialise RX ring ------
    let rx_ring_bytes = RING_SIZE * core::mem::size_of::<RxDesc>();
    let rx_ring_layout = alloc::alloc::Layout::from_size_align(rx_ring_bytes, 128).unwrap();
    let rx_ring = unsafe { alloc::alloc::alloc_zeroed(rx_ring_layout) as *mut RxDesc };

    let rx_bufs_layout = alloc::alloc::Layout::from_size_align(
        core::mem::size_of::<[[u8; RX_BUF_SIZE]; RING_SIZE]>(), 16,
    ).unwrap();
    let rx_bufs = unsafe { alloc::alloc::alloc_zeroed(rx_bufs_layout) as *mut [[u8; RX_BUF_SIZE]; RING_SIZE] };

    // Point each RX descriptor at its pre-allocated buffer (physical address)
    for i in 0..RING_SIZE {
        let buf_virt = unsafe { &(*rx_bufs)[i] as *const u8 as u64 };
        let buf_phys = buf_virt.wrapping_sub(memory::phys_mem_offset().as_u64());
        unsafe { (*rx_ring.add(i)).addr = buf_phys; }
    }

    let rx_ring_phys = (rx_ring as u64).wrapping_sub(memory::phys_mem_offset().as_u64()); // virt→phys
    mmio_write(mmio_base, REG_RDBAL, rx_ring_phys as u32);
    mmio_write(mmio_base, REG_RDBAH, (rx_ring_phys >> 32) as u32);
    mmio_write(mmio_base, REG_RDLEN, rx_ring_bytes as u32);
    mmio_write(mmio_base, REG_RDH, 0);
    mmio_write(mmio_base, REG_RDT, (RING_SIZE - 1) as u32);

    // Enable receiver
    mmio_write(mmio_base, REG_RCTL,
        RCTL_EN | RCTL_BAM | RCTL_BSIZE_2048 | RCTL_SECRC);

    // ------ Allocate and initialise TX ring ------
    let tx_ring_bytes = RING_SIZE * core::mem::size_of::<TxDesc>();
    let tx_ring_layout = alloc::alloc::Layout::from_size_align(tx_ring_bytes, 128).unwrap();
    let tx_ring = unsafe { alloc::alloc::alloc_zeroed(tx_ring_layout) as *mut TxDesc };

    let tx_ring_phys = (tx_ring as u64).wrapping_sub(memory::phys_mem_offset().as_u64()); // virt→phys
    mmio_write(mmio_base, REG_TDBAL, tx_ring_phys as u32);
    mmio_write(mmio_base, REG_TDBAH, (tx_ring_phys >> 32) as u32);
    mmio_write(mmio_base, REG_TDLEN, tx_ring_bytes as u32);
    mmio_write(mmio_base, REG_TDH, 0);
    mmio_write(mmio_base, REG_TDT, 0);

    // Enable transmitter
    mmio_write(mmio_base, REG_TCTL,
        TCTL_EN | TCTL_PSP | (15 << TCTL_CT_SHIFT) | (64 << TCTL_COLD_SHIFT));

    // Store device state
    unsafe {
        DEVICE = Some(E1000eDevice {
            mmio_base,
            mac,
            tx_ring,
            rx_ring,
            rx_bufs,
            tx_cur: 0,
            rx_cur: 0,
        });
    }

    INITIALIZED.store(true, Ordering::SeqCst);

    // Propagate MAC to the global network state
    {
        let mut ns = net::NET.lock();
        ns.mac = net::MacAddr(mac);
    }

    serial_println!("[e1000e] Driver initialised (TX/RX rings: {} descriptors)", RING_SIZE);
}

// ---------------------------------------------------------------------------
// Transmit
// ---------------------------------------------------------------------------

/// Transmit a raw Ethernet frame.
///
/// The caller must provide a complete Ethernet frame starting with the
/// destination MAC (no preamble/SFD). The NIC will append FCS automatically.
/// Returns `true` if the frame was queued successfully.
pub fn send_frame(frame: &[u8]) -> bool {
    if !INITIALIZED.load(Ordering::SeqCst) || frame.is_empty() || frame.len() > 1518 {
        return false;
    }

    let dev = unsafe { DEVICE.as_mut().unwrap() };
    let idx = dev.tx_cur;

    let desc = unsafe { &mut *dev.tx_ring.add(idx) };

    // Wait for previous descriptor to complete (DD bit)
    if desc.status & TXSTAT_DD == 0 && desc.cmd != 0 {
        // Ring is full — previous transmission still pending
        return false;
    }

    // Copy frame into a heap-allocated buffer so it stays alive
    let buf = frame.to_vec();
    let buf_phys = (buf.as_ptr() as u64).wrapping_sub(memory::phys_mem_offset().as_u64()); // virt→phys

    desc.addr = buf_phys;
    desc.length = frame.len() as u16;
    desc.cmd = TXCMD_EOP | TXCMD_IFCS | TXCMD_RS;
    desc.status = 0;
    desc.cso = 0;
    desc.css = 0;
    desc.special = 0;

    // Leak the buffer — the NIC reads it asynchronously via DMA.
    // A production driver would reclaim after DD is set.
    core::mem::forget(buf);

    dev.tx_cur = (idx + 1) % RING_SIZE;

    // Advance the tail pointer to notify the NIC
    mmio_write(dev.mmio_base, REG_TDT, dev.tx_cur as u32);

    true
}

// ---------------------------------------------------------------------------
// Receive
// ---------------------------------------------------------------------------

/// Poll the RX ring for a received Ethernet frame.
///
/// Returns `Some(Vec<u8>)` containing the raw Ethernet frame (starting at the
/// destination MAC) if a complete frame is available, or `None` if the ring is
/// empty. The receive buffer is recycled for the NIC to reuse.
pub fn recv_frame() -> Option<Vec<u8>> {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return None;
    }

    let dev = unsafe { DEVICE.as_mut().unwrap() };
    let idx = dev.rx_cur;

    let desc = unsafe { &mut *dev.rx_ring.add(idx) };

    // Check for a completed descriptor
    if desc.status & RXSTAT_DD == 0 {
        return None;
    }

    let len = desc.length as usize;
    let frame = unsafe {
        let buf = &(*dev.rx_bufs)[idx];
        buf[..len].to_vec()
    };

    // Reset descriptor for reuse
    desc.status = 0;
    desc.length = 0;
    desc.errors = 0;

    let old_idx = idx;
    dev.rx_cur = (idx + 1) % RING_SIZE;

    // Return the consumed descriptor to the NIC by advancing RDT
    mmio_write(dev.mmio_base, REG_RDT, old_idx as u32);

    Some(frame)
}

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

/// Returns `true` if a supported Intel e1000/e1000e NIC was found on the PCI bus.
pub fn is_detected() -> bool {
    DETECTED.load(Ordering::SeqCst)
}

/// Return a human-readable status string for the driver.
pub fn info() -> String {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return alloc::format!("e1000e: not initialised (detected={})", is_detected());
    }

    let dev = unsafe { DEVICE.as_ref().unwrap() };
    let status = mmio_read(dev.mmio_base, REG_STATUS);
    let link_up = status & 0x02 != 0;
    let speed = match (status >> 6) & 0x03 {
        0b00 => "10 Mb/s",
        0b01 => "100 Mb/s",
        _ => "1000 Mb/s",
    };

    alloc::format!(
        "e1000e: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}  link={}  speed={}  rings={}",
        dev.mac[0], dev.mac[1], dev.mac[2], dev.mac[3], dev.mac[4], dev.mac[5],
        if link_up { "up" } else { "down" },
        speed,
        RING_SIZE,
    )
}
