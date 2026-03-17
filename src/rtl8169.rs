/// Realtek RTL8169/RTL8111 Gigabit Ethernet driver for MerlionOS.
/// The most common gigabit NIC found on consumer PC motherboards.
/// PCI Vendor: 0x10EC, Devices: 0x8168, 0x8169, 0x8136

use crate::{pci, memory, net, serial_println};
use alloc::vec::Vec;
use alloc::string::String;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// PCI identifiers
// ---------------------------------------------------------------------------

const RTL_VENDOR_ID: u16 = 0x10EC;

const SUPPORTED_DEVICES: &[(u16, &str)] = &[
    (0x8168, "RTL8111/8168"),
    (0x8169, "RTL8169"),
    (0x8136, "RTL8101/8136"),
];

// ---------------------------------------------------------------------------
// Register offsets (MMIO)
// ---------------------------------------------------------------------------

/// MAC address bytes 0-3
const REG_IDR0: u32 = 0x00;
/// MAC address bytes 4-5
const REG_IDR4: u32 = 0x04;
/// Multicast filter low
const REG_MAR0: u32 = 0x08;
/// Multicast filter high
const REG_MAR4: u32 = 0x0C;
/// Dump Tally Counter Command
const REG_DTCCR: u32 = 0x10;
/// TX Normal Priority Descriptor Start Address (64-bit)
const REG_TNPDS: u32 = 0x20;
/// TX High Priority Descriptor Start Address (64-bit)
const REG_THPDS: u32 = 0x28;
/// Command register
const REG_CMD: u32 = 0x37;
/// TX Priority Polling
const REG_TPP: u32 = 0x38;
/// Interrupt Mask Register
const REG_IMR: u32 = 0x3C;
/// Interrupt Status Register
const REG_ISR: u32 = 0x3E;
/// TX Configuration Register
const REG_TCR: u32 = 0x40;
/// RX Configuration Register
const REG_RCR: u32 = 0x44;
/// 93C46 Command Register (config unlock)
const REG_9346CR: u32 = 0x50;
/// Configuration Register 1
const REG_CONFIG1: u32 = 0x52;
/// Configuration Register 2
const REG_CONFIG2: u32 = 0x53;
/// PHY Access Register (MDIO)
const REG_PHYAR: u32 = 0x60;
/// PHY Status Register
const REG_PHYSTAT: u32 = 0x6C;
/// RX Max Size
const REG_RMS: u32 = 0xDA;
/// RX Descriptor Start Address (64-bit)
const REG_RDSAR: u32 = 0xE4;
/// Max TX Packet Size
const REG_MAX_TX_SIZE: u32 = 0xEC;

// ---------------------------------------------------------------------------
// Command register bits
// ---------------------------------------------------------------------------

/// Transmitter Enable
const CMD_TE: u8 = 1 << 2;
/// Receiver Enable
const CMD_RE: u8 = 1 << 3;
/// Software Reset
const CMD_RST: u8 = 1 << 4;

// ---------------------------------------------------------------------------
// Interrupt bits (IMR/ISR)
// ---------------------------------------------------------------------------

/// RX OK
const INT_ROK: u16 = 1 << 0;
/// RX Error
const INT_RER: u16 = 1 << 1;
/// TX OK
const INT_TOK: u16 = 1 << 2;
/// TX Error
const INT_TER: u16 = 1 << 3;
/// Link Change
const INT_LINK_CHG: u16 = 1 << 5;
/// RX FIFO Overflow
const INT_RX_OVERFLOW: u16 = 1 << 6;
/// System Error
const INT_SYS_ERR: u16 = 1 << 15;

// ---------------------------------------------------------------------------
// RX Configuration bits
// ---------------------------------------------------------------------------

/// Accept All Packets
const RCR_AAP: u32 = 1 << 0;
/// Accept Physical Match
const RCR_APM: u32 = 1 << 1;
/// Accept Multicast
const RCR_AM: u32 = 1 << 2;
/// Accept Broadcast
const RCR_AB: u32 = 1 << 3;
/// Accept Runt (< 64 byte)
const RCR_AR: u32 = 1 << 4;
/// Accept Error
const RCR_AER: u32 = 1 << 5;
/// No RX threshold (DMA immediately)
const RCR_RXFTH_NONE: u32 = 7 << 13;
/// Max DMA burst 1024 bytes
const RCR_MXDMA_1024: u32 = 6 << 8;

// ---------------------------------------------------------------------------
// TX Configuration bits
// ---------------------------------------------------------------------------

/// Max DMA burst 1024 bytes
const TCR_MXDMA_1024: u32 = 6 << 8;
/// Interframe gap 96-bit time (standard)
const TCR_IFG_STD: u32 = 3 << 24;

// ---------------------------------------------------------------------------
// 9346CR unlock values
// ---------------------------------------------------------------------------

/// Unlock config registers for writing
const CFG_UNLOCK: u8 = 0xC0;
/// Lock config registers
const CFG_LOCK: u8 = 0x00;

// ---------------------------------------------------------------------------
// Descriptor bits
// ---------------------------------------------------------------------------

/// Descriptor owned by NIC (hardware sets/clears)
const DESC_OWN: u32 = 1 << 31;
/// End of Ring — last descriptor in ring, wrap to start
const DESC_EOR: u32 = 1 << 30;
/// First Segment of frame
const DESC_FS: u32 = 1 << 29;
/// Last Segment of frame
const DESC_LS: u32 = 1 << 28;
/// Large Send (TSO)
const DESC_LGSEN: u32 = 1 << 27;
/// IP checksum offload
const DESC_IPCS: u32 = 1 << 18;
/// UDP checksum offload
const DESC_UDPCS: u32 = 1 << 17;
/// TCP checksum offload
const DESC_TCPCS: u32 = 1 << 16;

/// VLAN tag available (opts2)
const DESC_VLAN_TAG: u32 = 1 << 16;

/// Length mask in opts1
const DESC_LEN_MASK: u32 = 0x3FFF;

// ---------------------------------------------------------------------------
// Ring parameters
// ---------------------------------------------------------------------------

const TX_RING_SIZE: usize = 256;
const RX_RING_SIZE: usize = 256;
const RX_BUF_SIZE: usize = 2048;
const MAX_TX_SIZE: usize = 1792;

// ---------------------------------------------------------------------------
// Descriptor structure
// ---------------------------------------------------------------------------

/// Hardware TX/RX descriptor (16 bytes, shared layout).
#[repr(C)]
#[derive(Clone, Copy)]
struct RtlDescriptor {
    /// OWN, EOR, FS, LS, length/flags
    opts1: u32,
    /// VLAN tag, checksum offload flags
    opts2: u32,
    /// Buffer physical address low 32 bits
    addr_low: u32,
    /// Buffer physical address high 32 bits
    addr_high: u32,
}

impl RtlDescriptor {
    const fn zero() -> Self {
        Self { opts1: 0, opts2: 0, addr_low: 0, addr_high: 0 }
    }
}

// ---------------------------------------------------------------------------
// MDIO / PHY constants
// ---------------------------------------------------------------------------

/// PHYAR write flag
const PHYAR_WRITE: u32 = 1 << 31;
/// PHY register: Basic Mode Control
const PHY_BMCR: u32 = 0;
/// PHY register: Basic Mode Status
const PHY_BMSR: u32 = 1;
/// PHY register: Gigabit Status
const PHY_GSTAT: u32 = 0x0A;
/// BMSR link status bit
const BMSR_LINK: u32 = 1 << 2;
/// BMSR auto-negotiation complete
const BMSR_AN_COMPLETE: u32 = 1 << 5;

// ---------------------------------------------------------------------------
// Link speed encoding in PHYSTAT register
// ---------------------------------------------------------------------------

const PHYSTAT_LINK: u32 = 1 << 1;
const PHYSTAT_1000: u32 = 1 << 4;
const PHYSTAT_100: u32 = 1 << 3;
const PHYSTAT_10: u32 = 0; // neither bit set
const PHYSTAT_FULL_DUPLEX: u32 = 1 << 0;

// ---------------------------------------------------------------------------
// WoL bits (CONFIG1)
// ---------------------------------------------------------------------------

const CONFIG1_PM_EN: u8 = 1 << 0;

// ---------------------------------------------------------------------------
// Device state
// ---------------------------------------------------------------------------

struct Rtl8169Device {
    mmio_base: u64,
    mac: [u8; 6],
    tx_ring: *mut RtlDescriptor,
    rx_ring: *mut RtlDescriptor,
    rx_bufs: *mut [[u8; RX_BUF_SIZE]; RX_RING_SIZE],
    tx_cur: usize,
    rx_cur: usize,
    device_name: &'static str,
}

unsafe impl Send for Rtl8169Device {}

static DETECTED: AtomicBool = AtomicBool::new(false);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static mut DEVICE: Option<Rtl8169Device> = None;

// Statistics counters
static TX_PACKETS: AtomicU64 = AtomicU64::new(0);
static TX_BYTES: AtomicU64 = AtomicU64::new(0);
static RX_PACKETS: AtomicU64 = AtomicU64::new(0);
static RX_BYTES: AtomicU64 = AtomicU64::new(0);
static TX_ERRORS: AtomicU64 = AtomicU64::new(0);
static RX_ERRORS: AtomicU64 = AtomicU64::new(0);
static RX_CRC_ERRORS: AtomicU64 = AtomicU64::new(0);
static COLLISIONS: AtomicU64 = AtomicU64::new(0);
static LINK_CHANGES: AtomicU64 = AtomicU64::new(0);
static INTERRUPTS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// MMIO helpers
// ---------------------------------------------------------------------------

#[inline]
fn mmio_read8(base: u64, reg: u32) -> u8 {
    unsafe {
        let ptr = (base + reg as u64) as *const u8;
        core::ptr::read_volatile(ptr)
    }
}

#[inline]
fn mmio_write8(base: u64, reg: u32, val: u8) {
    unsafe {
        let ptr = (base + reg as u64) as *mut u8;
        core::ptr::write_volatile(ptr, val);
    }
}

#[inline]
fn mmio_read16(base: u64, reg: u32) -> u16 {
    unsafe {
        let ptr = (base + reg as u64) as *const u16;
        core::ptr::read_volatile(ptr)
    }
}

#[inline]
fn mmio_write16(base: u64, reg: u32, val: u16) {
    unsafe {
        let ptr = (base + reg as u64) as *mut u16;
        core::ptr::write_volatile(ptr, val);
    }
}

#[inline]
fn mmio_read32(base: u64, reg: u32) -> u32 {
    unsafe {
        let ptr = (base + reg as u64) as *const u32;
        core::ptr::read_volatile(ptr)
    }
}

#[inline]
fn mmio_write32(base: u64, reg: u32, val: u32) {
    unsafe {
        let ptr = (base + reg as u64) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }
}

// ---------------------------------------------------------------------------
// PHY (MDIO) access
// ---------------------------------------------------------------------------

/// Read a PHY register via MDIO.
fn phy_read(base: u64, reg: u32) -> u32 {
    let val = (reg & 0x1F) << 16;
    mmio_write32(base, REG_PHYAR, val);
    // Wait for read to complete (bit 31 set by hardware)
    for _ in 0..2000 {
        core::hint::spin_loop();
        let r = mmio_read32(base, REG_PHYAR);
        if r & PHYAR_WRITE != 0 {
            return r & 0xFFFF;
        }
    }
    0
}

/// Write a PHY register via MDIO.
fn phy_write(base: u64, reg: u32, data: u16) {
    let val = PHYAR_WRITE | ((reg & 0x1F) << 16) | (data as u32);
    mmio_write32(base, REG_PHYAR, val);
    for _ in 0..2000 {
        core::hint::spin_loop();
        let r = mmio_read32(base, REG_PHYAR);
        if r & PHYAR_WRITE == 0 {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// MAC address
// ---------------------------------------------------------------------------

fn read_mac(base: u64) -> [u8; 6] {
    let lo = mmio_read32(base, REG_IDR0);
    let hi = mmio_read32(base, REG_IDR4);
    [
        (lo & 0xFF) as u8,
        ((lo >> 8) & 0xFF) as u8,
        ((lo >> 16) & 0xFF) as u8,
        ((lo >> 24) & 0xFF) as u8,
        (hi & 0xFF) as u8,
        ((hi >> 8) & 0xFF) as u8,
    ]
}

fn write_mac(base: u64, mac: &[u8; 6]) {
    // Unlock config registers
    mmio_write8(base, REG_9346CR, CFG_UNLOCK);

    let lo = (mac[0] as u32)
        | ((mac[1] as u32) << 8)
        | ((mac[2] as u32) << 16)
        | ((mac[3] as u32) << 24);
    let hi = (mac[4] as u32) | ((mac[5] as u32) << 8);

    mmio_write32(base, REG_IDR0, lo);
    mmio_write32(base, REG_IDR4, hi);

    // Lock config registers
    mmio_write8(base, REG_9346CR, CFG_LOCK);
}

// ---------------------------------------------------------------------------
// Link status
// ---------------------------------------------------------------------------

/// Query the link status from the PHY status register.
fn link_status(base: u64) -> (bool, &'static str, bool) {
    let phystat = mmio_read32(base, REG_PHYSTAT);
    let up = phystat & PHYSTAT_LINK != 0;
    let speed = if phystat & PHYSTAT_1000 != 0 {
        "1000 Mb/s"
    } else if phystat & PHYSTAT_100 != 0 {
        "100 Mb/s"
    } else {
        "10 Mb/s"
    };
    let full_duplex = phystat & PHYSTAT_FULL_DUPLEX != 0;
    (up, speed, full_duplex)
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Scan PCI for a Realtek RTL8169/8111 NIC and initialise it.
pub fn init() {
    let devices = pci::scan();
    let nic = devices.iter().find(|d| {
        d.vendor_id == RTL_VENDOR_ID
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

    serial_println!("[rtl8169] Found Realtek {} (device {:04x}) at {:02x}:{:02x}.{}",
        dev_name, nic.device_id, nic.bus, nic.device, nic.function);

    // Read BAR0 for MMIO base address
    let bar0_raw = pci::pci_read32(nic.bus, nic.device, nic.function, 0x10);
    let bar0_phys = (bar0_raw & 0xFFFF_FFF0) as u64;

    // If 64-bit BAR, read upper 32 bits
    let bar0_phys = if bar0_raw & 0x04 != 0 {
        let bar1 = pci::pci_read32(nic.bus, nic.device, nic.function, 0x14) as u64;
        bar0_phys | (bar1 << 32)
    } else {
        bar0_phys
    };

    let mmio_base = memory::phys_to_virt(x86_64::PhysAddr::new(bar0_phys)).as_u64();

    // Enable PCI bus mastering
    let cmd = pci::pci_read32(nic.bus, nic.device, nic.function, 0x04);
    pci::pci_write32(nic.bus, nic.device, nic.function, 0x04, cmd | (1 << 2));

    // --- Software reset ---
    mmio_write8(mmio_base, REG_CMD, CMD_RST);
    for _ in 0..100_000 {
        core::hint::spin_loop();
        if mmio_read8(mmio_base, REG_CMD) & CMD_RST == 0 {
            break;
        }
    }

    // Unlock config registers
    mmio_write8(mmio_base, REG_9346CR, CFG_UNLOCK);

    // Read MAC address
    let mac = read_mac(mmio_base);
    serial_println!("[rtl8169] MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

    // --- Configure RX ---
    // Accept broadcast + physical match + multicast, no threshold, max DMA burst
    mmio_write32(mmio_base, REG_RCR,
        RCR_APM | RCR_AB | RCR_AM | RCR_RXFTH_NONE | RCR_MXDMA_1024);

    // Set RX max size
    mmio_write16(mmio_base, REG_RMS, RX_BUF_SIZE as u16);

    // --- Configure TX ---
    mmio_write32(mmio_base, REG_TCR, TCR_MXDMA_1024 | TCR_IFG_STD);
    mmio_write8(mmio_base, REG_MAX_TX_SIZE, (MAX_TX_SIZE / 128) as u8);

    // --- Accept all multicast ---
    mmio_write32(mmio_base, REG_MAR0, 0xFFFF_FFFF);
    mmio_write32(mmio_base, REG_MAR4, 0xFFFF_FFFF);

    // --- Allocate TX descriptor ring ---
    let tx_ring_bytes = TX_RING_SIZE * core::mem::size_of::<RtlDescriptor>();
    let tx_ring_layout = alloc::alloc::Layout::from_size_align(tx_ring_bytes, 256).unwrap();
    let tx_ring = unsafe { alloc::alloc::alloc_zeroed(tx_ring_layout) as *mut RtlDescriptor };

    // Mark last TX descriptor with EOR
    unsafe {
        (*tx_ring.add(TX_RING_SIZE - 1)).opts1 = DESC_EOR;
    }

    // --- Allocate RX descriptor ring ---
    let rx_ring_bytes = RX_RING_SIZE * core::mem::size_of::<RtlDescriptor>();
    let rx_ring_layout = alloc::alloc::Layout::from_size_align(rx_ring_bytes, 256).unwrap();
    let rx_ring = unsafe { alloc::alloc::alloc_zeroed(rx_ring_layout) as *mut RtlDescriptor };

    // Allocate RX buffers
    let rx_bufs_layout = alloc::alloc::Layout::from_size_align(
        core::mem::size_of::<[[u8; RX_BUF_SIZE]; RX_RING_SIZE]>(), 16,
    ).unwrap();
    let rx_bufs = unsafe {
        alloc::alloc::alloc_zeroed(rx_bufs_layout) as *mut [[u8; RX_BUF_SIZE]; RX_RING_SIZE]
    };

    // Point each RX descriptor at its buffer and give ownership to NIC
    for i in 0..RX_RING_SIZE {
        let buf_virt = unsafe { &(*rx_bufs)[i] as *const u8 as u64 };
        let buf_phys = buf_virt.wrapping_sub(memory::phys_mem_offset().as_u64());
        let mut flags = DESC_OWN | (RX_BUF_SIZE as u32 & DESC_LEN_MASK);
        if i == RX_RING_SIZE - 1 {
            flags |= DESC_EOR;
        }
        unsafe {
            let desc = &mut *rx_ring.add(i);
            desc.opts1 = flags;
            desc.opts2 = 0;
            desc.addr_low = buf_phys as u32;
            desc.addr_high = (buf_phys >> 32) as u32;
        }
    }

    // Set descriptor base addresses
    let tx_ring_phys = (tx_ring as u64).wrapping_sub(memory::phys_mem_offset().as_u64());
    mmio_write32(mmio_base, REG_TNPDS, tx_ring_phys as u32);
    mmio_write32(mmio_base, REG_TNPDS + 4, (tx_ring_phys >> 32) as u32);

    let rx_ring_phys = (rx_ring as u64).wrapping_sub(memory::phys_mem_offset().as_u64());
    mmio_write32(mmio_base, REG_RDSAR, rx_ring_phys as u32);
    mmio_write32(mmio_base, REG_RDSAR + 4, (rx_ring_phys >> 32) as u32);

    // Lock config registers
    mmio_write8(mmio_base, REG_9346CR, CFG_LOCK);

    // Enable TX + RX
    mmio_write8(mmio_base, REG_CMD, CMD_TE | CMD_RE);

    // Enable interrupts: TX OK, RX OK, link change, system error
    mmio_write16(mmio_base, REG_IMR, INT_ROK | INT_TOK | INT_LINK_CHG | INT_SYS_ERR);

    // Clear any pending interrupts
    let _ = mmio_read16(mmio_base, REG_ISR);
    mmio_write16(mmio_base, REG_ISR, 0xFFFF);

    // Store device
    unsafe {
        DEVICE = Some(Rtl8169Device {
            mmio_base,
            mac,
            tx_ring,
            rx_ring,
            rx_bufs,
            tx_cur: 0,
            rx_cur: 0,
            device_name: dev_name,
        });
    }

    INITIALIZED.store(true, Ordering::SeqCst);

    // Propagate MAC to global network state
    {
        let mut ns = net::NET.lock();
        ns.mac = net::MacAddr(mac);
    }

    let (link_up, speed, full_dup) = link_status(mmio_base);
    serial_println!("[rtl8169] link={} speed={} duplex={} TX/RX rings: {}/{}",
        if link_up { "up" } else { "down" },
        speed,
        if full_dup { "full" } else { "half" },
        TX_RING_SIZE, RX_RING_SIZE);
}

// ---------------------------------------------------------------------------
// Transmit
// ---------------------------------------------------------------------------

/// Transmit a raw Ethernet frame. Returns `true` if queued successfully.
pub fn send(data: &[u8]) -> bool {
    if !INITIALIZED.load(Ordering::SeqCst) || data.is_empty() || data.len() > MAX_TX_SIZE {
        return false;
    }

    let dev = unsafe { (*(&raw mut DEVICE)).as_mut().unwrap() };
    let idx = dev.tx_cur;
    let desc = unsafe { &mut *dev.tx_ring.add(idx) };

    // Check if descriptor is still owned by NIC
    if desc.opts1 & DESC_OWN != 0 {
        TX_ERRORS.fetch_add(1, Ordering::Relaxed);
        return false;
    }

    // Copy frame to heap buffer for DMA
    let buf = data.to_vec();
    let buf_phys = (buf.as_ptr() as u64).wrapping_sub(memory::phys_mem_offset().as_u64());

    let mut flags = DESC_OWN | DESC_FS | DESC_LS | (data.len() as u32 & DESC_LEN_MASK);
    if idx == TX_RING_SIZE - 1 {
        flags |= DESC_EOR;
    }

    desc.addr_low = buf_phys as u32;
    desc.addr_high = (buf_phys >> 32) as u32;
    desc.opts2 = 0;
    desc.opts1 = flags;

    // Leak buffer — NIC reads asynchronously via DMA
    core::mem::forget(buf);

    dev.tx_cur = (idx + 1) % TX_RING_SIZE;

    // Trigger TX poll
    mmio_write8(dev.mmio_base, REG_TPP, 0x40);

    TX_PACKETS.fetch_add(1, Ordering::Relaxed);
    TX_BYTES.fetch_add(data.len() as u64, Ordering::Relaxed);

    true
}

/// Transmit with TX checksum offload (IP + TCP/UDP).
pub fn send_with_csum(data: &[u8], tcp: bool) -> bool {
    if !INITIALIZED.load(Ordering::SeqCst) || data.is_empty() || data.len() > MAX_TX_SIZE {
        return false;
    }

    let dev = unsafe { (*(&raw mut DEVICE)).as_mut().unwrap() };
    let idx = dev.tx_cur;
    let desc = unsafe { &mut *dev.tx_ring.add(idx) };

    if desc.opts1 & DESC_OWN != 0 {
        TX_ERRORS.fetch_add(1, Ordering::Relaxed);
        return false;
    }

    let buf = data.to_vec();
    let buf_phys = (buf.as_ptr() as u64).wrapping_sub(memory::phys_mem_offset().as_u64());

    let mut flags = DESC_OWN | DESC_FS | DESC_LS | (data.len() as u32 & DESC_LEN_MASK);
    if idx == TX_RING_SIZE - 1 {
        flags |= DESC_EOR;
    }

    // Checksum offload via opts2
    let csum_flags = DESC_IPCS | if tcp { DESC_TCPCS } else { DESC_UDPCS };

    desc.addr_low = buf_phys as u32;
    desc.addr_high = (buf_phys >> 32) as u32;
    desc.opts2 = csum_flags;
    desc.opts1 = flags;

    core::mem::forget(buf);

    dev.tx_cur = (idx + 1) % TX_RING_SIZE;
    mmio_write8(dev.mmio_base, REG_TPP, 0x40);

    TX_PACKETS.fetch_add(1, Ordering::Relaxed);
    TX_BYTES.fetch_add(data.len() as u64, Ordering::Relaxed);

    true
}

/// Transmit with VLAN tag insertion.
pub fn send_vlan(data: &[u8], vlan_id: u16) -> bool {
    if !INITIALIZED.load(Ordering::SeqCst) || data.is_empty() || data.len() > MAX_TX_SIZE {
        return false;
    }

    let dev = unsafe { (*(&raw mut DEVICE)).as_mut().unwrap() };
    let idx = dev.tx_cur;
    let desc = unsafe { &mut *dev.tx_ring.add(idx) };

    if desc.opts1 & DESC_OWN != 0 {
        TX_ERRORS.fetch_add(1, Ordering::Relaxed);
        return false;
    }

    let buf = data.to_vec();
    let buf_phys = (buf.as_ptr() as u64).wrapping_sub(memory::phys_mem_offset().as_u64());

    let mut flags = DESC_OWN | DESC_FS | DESC_LS | (data.len() as u32 & DESC_LEN_MASK);
    if idx == TX_RING_SIZE - 1 {
        flags |= DESC_EOR;
    }

    desc.addr_low = buf_phys as u32;
    desc.addr_high = (buf_phys >> 32) as u32;
    desc.opts2 = DESC_VLAN_TAG | (vlan_id as u32 & 0xFFFF);
    desc.opts1 = flags;

    core::mem::forget(buf);

    dev.tx_cur = (idx + 1) % TX_RING_SIZE;
    mmio_write8(dev.mmio_base, REG_TPP, 0x40);

    TX_PACKETS.fetch_add(1, Ordering::Relaxed);
    TX_BYTES.fetch_add(data.len() as u64, Ordering::Relaxed);

    true
}

// ---------------------------------------------------------------------------
// Receive
// ---------------------------------------------------------------------------

/// Poll the RX ring for a received Ethernet frame.
pub fn recv() -> Option<Vec<u8>> {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return None;
    }

    let dev = unsafe { (*(&raw mut DEVICE)).as_mut().unwrap() };
    let idx = dev.rx_cur;
    let desc = unsafe { &mut *dev.rx_ring.add(idx) };

    // Check if NIC has released this descriptor (OWN cleared)
    if desc.opts1 & DESC_OWN != 0 {
        return None;
    }

    let opts1 = desc.opts1;

    // Check for errors in the frame
    if opts1 & (1 << 21) != 0 {
        // RX error bit set
        RX_ERRORS.fetch_add(1, Ordering::Relaxed);
        // Check for CRC error specifically
        if opts1 & (1 << 19) != 0 {
            RX_CRC_ERRORS.fetch_add(1, Ordering::Relaxed);
        }
        // Recycle descriptor
        let mut new_flags = DESC_OWN | (RX_BUF_SIZE as u32 & DESC_LEN_MASK);
        if idx == RX_RING_SIZE - 1 {
            new_flags |= DESC_EOR;
        }
        desc.opts1 = new_flags;
        dev.rx_cur = (idx + 1) % RX_RING_SIZE;
        return None;
    }

    // Must be first and last segment (we don't handle jumbo spanning)
    if opts1 & DESC_FS == 0 || opts1 & DESC_LS == 0 {
        let mut new_flags = DESC_OWN | (RX_BUF_SIZE as u32 & DESC_LEN_MASK);
        if idx == RX_RING_SIZE - 1 {
            new_flags |= DESC_EOR;
        }
        desc.opts1 = new_flags;
        dev.rx_cur = (idx + 1) % RX_RING_SIZE;
        return None;
    }

    let len = (opts1 & DESC_LEN_MASK) as usize;
    if len == 0 || len > RX_BUF_SIZE {
        let mut new_flags = DESC_OWN | (RX_BUF_SIZE as u32 & DESC_LEN_MASK);
        if idx == RX_RING_SIZE - 1 {
            new_flags |= DESC_EOR;
        }
        desc.opts1 = new_flags;
        dev.rx_cur = (idx + 1) % RX_RING_SIZE;
        return None;
    }

    // Copy frame data
    let frame = unsafe {
        let buf = &(*dev.rx_bufs)[idx];
        buf[..len].to_vec()
    };

    // Extract VLAN tag if present (for future use)
    let _vlan_tag = if desc.opts2 & DESC_VLAN_TAG != 0 {
        Some((desc.opts2 & 0xFFFF) as u16)
    } else {
        None
    };

    // Recycle descriptor
    let mut new_flags = DESC_OWN | (RX_BUF_SIZE as u32 & DESC_LEN_MASK);
    if idx == RX_RING_SIZE - 1 {
        new_flags |= DESC_EOR;
    }
    desc.opts1 = new_flags;
    desc.opts2 = 0;

    dev.rx_cur = (idx + 1) % RX_RING_SIZE;

    RX_PACKETS.fetch_add(1, Ordering::Relaxed);
    RX_BYTES.fetch_add(len as u64, Ordering::Relaxed);

    Some(frame)
}

// ---------------------------------------------------------------------------
// Interrupt handler
// ---------------------------------------------------------------------------

/// Handle an RTL8169 interrupt. Call from the interrupt dispatcher.
pub fn handle_interrupt() {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return;
    }

    let dev = unsafe { (*(&raw const DEVICE)).as_ref().unwrap() };
    let isr = mmio_read16(dev.mmio_base, REG_ISR);

    if isr == 0 {
        return;
    }

    INTERRUPTS.fetch_add(1, Ordering::Relaxed);

    // Clear all pending interrupt bits
    mmio_write16(dev.mmio_base, REG_ISR, isr);

    if isr & INT_TOK != 0 {
        // TX completed — descriptors freed automatically
    }

    if isr & INT_TER != 0 {
        TX_ERRORS.fetch_add(1, Ordering::Relaxed);
    }

    if isr & INT_ROK != 0 {
        // RX available — will be picked up by next recv() call
    }

    if isr & INT_RER != 0 {
        RX_ERRORS.fetch_add(1, Ordering::Relaxed);
    }

    if isr & INT_LINK_CHG != 0 {
        LINK_CHANGES.fetch_add(1, Ordering::Relaxed);
        let (up, speed, _dup) = link_status(dev.mmio_base);
        serial_println!("[rtl8169] Link change: {} {}", if up { "up" } else { "down" }, speed);
    }

    if isr & INT_RX_OVERFLOW != 0 {
        RX_ERRORS.fetch_add(1, Ordering::Relaxed);
        serial_println!("[rtl8169] RX FIFO overflow");
    }

    if isr & INT_SYS_ERR != 0 {
        serial_println!("[rtl8169] System error!");
    }
}

// ---------------------------------------------------------------------------
// Wake-on-LAN
// ---------------------------------------------------------------------------

/// Enable Wake-on-LAN magic packet detection.
pub fn enable_wol() {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return;
    }
    let dev = unsafe { (*(&raw const DEVICE)).as_ref().unwrap() };

    mmio_write8(dev.mmio_base, REG_9346CR, CFG_UNLOCK);
    let cfg1 = mmio_read8(dev.mmio_base, REG_CONFIG1);
    mmio_write8(dev.mmio_base, REG_CONFIG1, cfg1 | CONFIG1_PM_EN);
    mmio_write8(dev.mmio_base, REG_9346CR, CFG_LOCK);

    serial_println!("[rtl8169] Wake-on-LAN enabled");
}

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

pub fn is_detected() -> bool {
    DETECTED.load(Ordering::SeqCst)
}

pub fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::SeqCst)
}

/// Return a human-readable info string.
pub fn rtl8169_info() -> String {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return alloc::format!("rtl8169: not initialised (detected={})", is_detected());
    }

    let dev = unsafe { (*(&raw const DEVICE)).as_ref().unwrap() };
    let (link_up, speed, full_dup) = link_status(dev.mmio_base);

    alloc::format!(
        "rtl8169: {} MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}  link={}  speed={}  duplex={}  rings={}/{}",
        dev.device_name,
        dev.mac[0], dev.mac[1], dev.mac[2], dev.mac[3], dev.mac[4], dev.mac[5],
        if link_up { "up" } else { "down" },
        speed,
        if full_dup { "full" } else { "half" },
        TX_RING_SIZE, RX_RING_SIZE,
    )
}

/// Return statistics as a human-readable string.
pub fn rtl8169_stats() -> String {
    let tx_pkt = TX_PACKETS.load(Ordering::Relaxed);
    let tx_b = TX_BYTES.load(Ordering::Relaxed);
    let rx_pkt = RX_PACKETS.load(Ordering::Relaxed);
    let rx_b = RX_BYTES.load(Ordering::Relaxed);
    let tx_err = TX_ERRORS.load(Ordering::Relaxed);
    let rx_err = RX_ERRORS.load(Ordering::Relaxed);
    let crc_err = RX_CRC_ERRORS.load(Ordering::Relaxed);
    let coll = COLLISIONS.load(Ordering::Relaxed);
    let lnk = LINK_CHANGES.load(Ordering::Relaxed);
    let irqs = INTERRUPTS.load(Ordering::Relaxed);

    alloc::format!(
        "rtl8169 stats:\n  TX: {} packets, {} bytes, {} errors\n  RX: {} packets, {} bytes, {} errors\n  CRC errors: {}  collisions: {}  link changes: {}  interrupts: {}",
        tx_pkt, tx_b, tx_err,
        rx_pkt, rx_b, rx_err,
        crc_err, coll, lnk, irqs,
    )
}
