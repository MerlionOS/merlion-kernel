/// Intel I225-V 2.5 Gigabit Ethernet driver for MerlionOS.
/// Found on modern motherboards (Z490, B550 and later).
/// PCI Vendor: 0x8086, Device: 0x15F3 (I225-V), 0x15F2 (I225-LM)

use crate::{pci, memory, net, serial_println};
use alloc::vec::Vec;
use alloc::string::String;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// PCI identifiers
// ---------------------------------------------------------------------------

const INTEL_VENDOR_ID: u16 = 0x8086;

const SUPPORTED_DEVICES: &[(u16, &str)] = &[
    (0x15F3, "I225-V"),
    (0x15F2, "I225-LM"),
    (0x15F4, "I225-I"),
];

// ---------------------------------------------------------------------------
// Register offsets (MMIO) — I225 uses different offsets from classic e1000
// ---------------------------------------------------------------------------

/// Device Control Register
const REG_CTRL: u32 = 0x0000;
/// Device Status Register
const REG_STATUS: u32 = 0x0008;
/// EEPROM/Flash Control
const REG_EECD: u32 = 0x0010;
/// EEPROM Read
const REG_EERD: u32 = 0x0014;
/// Extended Device Control
const REG_CTRL_EXT: u32 = 0x0018;
/// MDI Control (PHY access)
const REG_MDIC: u32 = 0x0020;
/// Flow Control Address Low
const REG_FCAL: u32 = 0x0028;
/// Flow Control Address High
const REG_FCAH: u32 = 0x002C;
/// Flow Control Type
const REG_FCT: u32 = 0x0030;
/// VLAN Ether Type
const REG_VET: u32 = 0x0038;
/// Interrupt Cause Read (I225 offset, differs from e1000!)
const REG_ICR: u32 = 0x01500;
/// Interrupt Cause Set
const REG_ICS: u32 = 0x01504;
/// Interrupt Mask Set
const REG_IMS: u32 = 0x01508;
/// Interrupt Mask Clear
const REG_IMC: u32 = 0x0150C;
/// RX Control
const REG_RCTL: u32 = 0x0100;
/// TX Control
const REG_TCTL: u32 = 0x0400;
/// RX Descriptor Base Low (queue 0)
const REG_RDBAL: u32 = 0xC000;
/// RX Descriptor Base High (queue 0)
const REG_RDBAH: u32 = 0xC004;
/// RX Descriptor Ring Length
const REG_RDLEN: u32 = 0xC008;
/// RX Descriptor Head
const REG_RDH: u32 = 0xC010;
/// RX Descriptor Tail
const REG_RDT: u32 = 0xC018;
/// TX Descriptor Base Low (queue 0)
const REG_TDBAL: u32 = 0xE000;
/// TX Descriptor Base High (queue 0)
const REG_TDBAH: u32 = 0xE004;
/// TX Descriptor Ring Length
const REG_TDLEN: u32 = 0xE008;
/// TX Descriptor Head
const REG_TDH: u32 = 0xE010;
/// TX Descriptor Tail
const REG_TDT: u32 = 0xE018;
/// Receive Address Low
const REG_RAL: u32 = 0x5400;
/// Receive Address High (+ Address Valid bit)
const REG_RAH: u32 = 0x5404;

// --- Extended statistics registers ---
const REG_GPRC: u32 = 0x4074;   // Good Packets Received Count
const REG_GPTC: u32 = 0x4080;   // Good Packets Transmitted Count
const REG_GORC_LO: u32 = 0x4088; // Good Octets Received Low
const REG_GORC_HI: u32 = 0x408C; // Good Octets Received High
const REG_GOTC_LO: u32 = 0x4090; // Good Octets Transmitted Low
const REG_GOTC_HI: u32 = 0x4094; // Good Octets Transmitted High
const REG_MPRC: u32 = 0x407C;   // Multicast Packets Received
const REG_BPRC: u32 = 0x4078;   // Broadcast Packets Received
const REG_CRCERRS: u32 = 0x4000; // CRC Error Count
const REG_RLEC: u32 = 0x4040;   // Receive Length Error Count
const REG_COLC: u32 = 0x4028;   // Collision Count
const REG_MPC: u32 = 0x4010;    // Missed Packets Count

// --- PTP / IEEE 1588 registers (simplified) ---
const REG_TSAUXC: u32 = 0xB640;  // Timestamp Auxiliary Control
const REG_SYSTIML: u32 = 0xB600; // System Time Low
const REG_SYSTIMH: u32 = 0xB604; // System Time High

// --- EEE (Energy Efficient Ethernet) ---
const REG_EEE_SU: u32 = 0x0E34;  // EEE Setup

// ---------------------------------------------------------------------------
// CTRL register bits
// ---------------------------------------------------------------------------

const CTRL_SLU: u32 = 1 << 6;   // Set Link Up
const CTRL_RST: u32 = 1 << 26;  // Device Reset
const CTRL_PHY_RST: u32 = 1 << 31; // PHY Reset

// ---------------------------------------------------------------------------
// RCTL register bits
// ---------------------------------------------------------------------------

const RCTL_EN: u32 = 1 << 1;       // Receiver Enable
const RCTL_UPE: u32 = 1 << 3;      // Unicast Promiscuous
const RCTL_MPE: u32 = 1 << 4;      // Multicast Promiscuous
const RCTL_BAM: u32 = 1 << 15;     // Broadcast Accept Mode
const RCTL_BSIZE_2048: u32 = 0;    // Buffer Size 2048
const RCTL_SECRC: u32 = 1 << 26;   // Strip Ethernet CRC

// ---------------------------------------------------------------------------
// TCTL register bits
// ---------------------------------------------------------------------------

const TCTL_EN: u32 = 1 << 1;       // Transmit Enable
const TCTL_PSP: u32 = 1 << 3;      // Pad Short Packets
const TCTL_CT_SHIFT: u32 = 4;      // Collision Threshold shift
const TCTL_COLD_SHIFT: u32 = 12;   // Collision Distance shift

// ---------------------------------------------------------------------------
// Interrupt bits
// ---------------------------------------------------------------------------

const INT_TXDW: u32 = 1 << 0;   // TX Descriptor Written Back
const INT_LSC: u32 = 1 << 2;    // Link Status Change
const INT_RXDW: u32 = 1 << 7;   // RX Descriptor Written Back

// ---------------------------------------------------------------------------
// MDIC register bits
// ---------------------------------------------------------------------------

const MDIC_REGADD_SHIFT: u32 = 16;
const MDIC_PHYADD_SHIFT: u32 = 21;
const MDIC_OP_READ: u32 = 2 << 26;
const MDIC_OP_WRITE: u32 = 1 << 26;
const MDIC_READY: u32 = 1 << 28;
const MDIC_ERROR: u32 = 1 << 30;

// PHY registers
const PHY_CONTROL: u32 = 0;
const PHY_STATUS: u32 = 1;
const PHY_AUTONEG_ADV: u32 = 4;
const PHY_1000T_CTRL: u32 = 9;
const PHY_2500T_CTRL: u32 = 32;  // 2.5GBASE-T specific

// PHY status bits
const PHY_STATUS_LINK: u32 = 1 << 2;

// ---------------------------------------------------------------------------
// Descriptor structures
// ---------------------------------------------------------------------------

/// Advanced TX Descriptor (I225 format, 16 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
struct I225TxDesc {
    /// Buffer physical address
    addr: u64,
    /// DTYP, DCMD, DEXT, payload length
    cmd_type_len: u32,
    /// Payload length, POPTS, status/done
    olinfo_status: u32,
}

impl I225TxDesc {
    const fn zero() -> Self {
        Self { addr: 0, cmd_type_len: 0, olinfo_status: 0 }
    }
}

/// Advanced RX Descriptor (I225 format, 16 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
struct I225RxDesc {
    /// Buffer physical address
    addr: u64,
    /// Length, status, errors, VLAN tag (written by hardware)
    info: u64,
}

impl I225RxDesc {
    const fn zero() -> Self {
        Self { addr: 0, info: 0 }
    }
}

// TX descriptor bits
const TXD_DTYP_ADV: u32 = 1 << 20;  // Advanced descriptor type
const TXD_DCMD_DEXT: u32 = 1 << 29;  // Descriptor extension
const TXD_DCMD_RS: u32 = 1 << 27;    // Report Status
const TXD_DCMD_IFCS: u32 = 1 << 25;  // Insert FCS
const TXD_DCMD_EOP: u32 = 1 << 24;   // End of Packet
const TXD_STAT_DD: u32 = 1 << 0;     // Descriptor Done

// RX descriptor info bits
const RXD_STAT_DD: u64 = 1 << 0;     // Descriptor Done
const RXD_STAT_EOP: u64 = 1 << 1;    // End of Packet
const RXD_LEN_SHIFT: u64 = 0;
const RXD_LEN_MASK: u64 = 0xFFFF;

// ---------------------------------------------------------------------------
// Ring parameters
// ---------------------------------------------------------------------------

const RING_SIZE: usize = 256;
const RX_BUF_SIZE: usize = 2048;

// ---------------------------------------------------------------------------
// Device state
// ---------------------------------------------------------------------------

struct I225Device {
    mmio_base: u64,
    mac: [u8; 6],
    tx_ring: *mut I225TxDesc,
    rx_ring: *mut I225RxDesc,
    rx_bufs: *mut [[u8; RX_BUF_SIZE]; RING_SIZE],
    tx_cur: usize,
    rx_cur: usize,
    device_name: &'static str,
}

unsafe impl Send for I225Device {}

static DETECTED: AtomicBool = AtomicBool::new(false);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static mut DEVICE: Option<I225Device> = None;

// Software statistics
static TX_PACKETS: AtomicU64 = AtomicU64::new(0);
static TX_BYTES: AtomicU64 = AtomicU64::new(0);
static RX_PACKETS: AtomicU64 = AtomicU64::new(0);
static RX_BYTES: AtomicU64 = AtomicU64::new(0);
static TX_ERRORS: AtomicU64 = AtomicU64::new(0);
static RX_ERRORS: AtomicU64 = AtomicU64::new(0);
static LINK_CHANGES: AtomicU64 = AtomicU64::new(0);
static INTERRUPTS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// MMIO helpers
// ---------------------------------------------------------------------------

#[inline]
fn mmio_read(base: u64, reg: u32) -> u32 {
    unsafe {
        let ptr = (base + reg as u64) as *const u32;
        core::ptr::read_volatile(ptr)
    }
}

#[inline]
fn mmio_write(base: u64, reg: u32, val: u32) {
    unsafe {
        let ptr = (base + reg as u64) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }
}

// ---------------------------------------------------------------------------
// PHY (MDIC) access
// ---------------------------------------------------------------------------

fn phy_read(base: u64, reg: u32) -> u32 {
    let mdic = MDIC_OP_READ
        | ((reg & 0x1F) << MDIC_REGADD_SHIFT)
        | (1 << MDIC_PHYADD_SHIFT); // PHY address 1
    mmio_write(base, REG_MDIC, mdic);

    for _ in 0..5000 {
        core::hint::spin_loop();
        let val = mmio_read(base, REG_MDIC);
        if val & MDIC_READY != 0 {
            if val & MDIC_ERROR != 0 {
                return 0;
            }
            return val & 0xFFFF;
        }
    }
    0
}

fn phy_write(base: u64, reg: u32, data: u16) {
    let mdic = MDIC_OP_WRITE
        | ((reg & 0x1F) << MDIC_REGADD_SHIFT)
        | (1 << MDIC_PHYADD_SHIFT)
        | (data as u32);
    mmio_write(base, REG_MDIC, mdic);

    for _ in 0..5000 {
        core::hint::spin_loop();
        let val = mmio_read(base, REG_MDIC);
        if val & MDIC_READY != 0 {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// MAC address
// ---------------------------------------------------------------------------

fn read_mac(base: u64) -> [u8; 6] {
    let ral = mmio_read(base, REG_RAL);
    let rah = mmio_read(base, REG_RAH);
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
// Link status
// ---------------------------------------------------------------------------

fn link_status(base: u64) -> (bool, &'static str, bool) {
    let status = mmio_read(base, REG_STATUS);
    let up = status & 0x02 != 0;
    // I225 speed encoding in STATUS bits 7:6
    let speed = match (status >> 6) & 0x03 {
        0b00 => "10 Mb/s",
        0b01 => "100 Mb/s",
        0b10 => "1000 Mb/s",
        _ => "2500 Mb/s",
    };
    let full_duplex = status & 0x01 != 0;
    (up, speed, full_duplex)
}

// ---------------------------------------------------------------------------
// Auto-negotiation for 2.5GBASE-T
// ---------------------------------------------------------------------------

/// Configure PHY auto-negotiation to advertise 10/100/1000/2500 Mbps.
fn configure_autoneg(base: u64) {
    // Advertise 10/100 Mbps
    let adv = phy_read(base, PHY_AUTONEG_ADV);
    phy_write(base, PHY_AUTONEG_ADV, (adv | 0x1E0) as u16); // 10/100 full+half

    // Advertise 1000 Mbps
    let gig = phy_read(base, PHY_1000T_CTRL);
    phy_write(base, PHY_1000T_CTRL, (gig | 0x0300) as u16); // 1000 full+half

    // Advertise 2.5G (I225-specific PHY register)
    phy_write(base, PHY_2500T_CTRL, 0x0001); // Enable 2.5GBASE-T

    // Restart auto-negotiation
    let ctrl = phy_read(base, PHY_CONTROL);
    phy_write(base, PHY_CONTROL, (ctrl | (1 << 9) | (1 << 12)) as u16);
}

// ---------------------------------------------------------------------------
// Flow control (PAUSE frames)
// ---------------------------------------------------------------------------

fn configure_flow_control(base: u64) {
    // Set standard PAUSE frame MAC (01:80:C2:00:00:01)
    mmio_write(base, REG_FCAL, 0x00C28001);
    mmio_write(base, REG_FCAH, 0x0100);
    mmio_write(base, REG_FCT, 0x8808);
}

// ---------------------------------------------------------------------------
// Energy Efficient Ethernet (EEE)
// ---------------------------------------------------------------------------

/// Enable Energy Efficient Ethernet for low-power idle.
fn enable_eee(base: u64) {
    // Set EEE advertisement via PHY MDIO
    // EEE is negotiated during auto-negotiation
    let eee_su = mmio_read(base, REG_EEE_SU);
    mmio_write(base, REG_EEE_SU, eee_su | 0x01); // Enable EEE
}

// ---------------------------------------------------------------------------
// PTP / IEEE 1588 timestamping (simplified)
// ---------------------------------------------------------------------------

/// Read the hardware PTP system time (low 64 bits, nanosecond resolution).
pub fn ptp_read_time() -> u64 {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return 0;
    }
    let dev = unsafe { (*(&raw const DEVICE)).as_ref().unwrap() };
    let lo = mmio_read(dev.mmio_base, REG_SYSTIML) as u64;
    let hi = mmio_read(dev.mmio_base, REG_SYSTIMH) as u64;
    (hi << 32) | lo
}

/// Enable PTP timestamping on the I225.
fn enable_ptp(base: u64) {
    // Enable the timestamp auxiliary control
    let auxc = mmio_read(base, REG_TSAUXC);
    mmio_write(base, REG_TSAUXC, auxc | 0x01); // Enable PTP
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Scan PCI for an Intel I225 NIC and initialise it.
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

    serial_println!("[i225] Found Intel {} (device {:04x}) at {:02x}:{:02x}.{}",
        dev_name, nic.device_id, nic.bus, nic.device, nic.function);

    // Read BAR0 for MMIO base address
    let bar0_raw = pci::pci_read32(nic.bus, nic.device, nic.function, 0x10);
    let bar0_phys = (bar0_raw & 0xFFFF_FFF0) as u64;

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

    // Device reset
    mmio_write(mmio_base, REG_CTRL, mmio_read(mmio_base, REG_CTRL) | CTRL_RST);
    for _ in 0..100_000 {
        core::hint::spin_loop();
        if mmio_read(mmio_base, REG_CTRL) & CTRL_RST == 0 {
            break;
        }
    }

    // Disable all interrupts during setup
    mmio_write(mmio_base, REG_IMC, 0xFFFF_FFFF);
    let _ = mmio_read(mmio_base, REG_ICR); // clear pending

    // Set link up
    let ctrl = mmio_read(mmio_base, REG_CTRL);
    mmio_write(mmio_base, REG_CTRL, ctrl | CTRL_SLU);

    // Read MAC
    let mac = read_mac(mmio_base);
    serial_println!("[i225] MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

    // Configure PHY for 2.5GBASE-T auto-negotiation
    configure_autoneg(mmio_base);

    // Configure flow control
    configure_flow_control(mmio_base);

    // Enable Energy Efficient Ethernet
    enable_eee(mmio_base);

    // Enable PTP timestamping
    enable_ptp(mmio_base);

    // --- Allocate RX descriptor ring ---
    let rx_ring_bytes = RING_SIZE * core::mem::size_of::<I225RxDesc>();
    let rx_ring_layout = alloc::alloc::Layout::from_size_align(rx_ring_bytes, 128).unwrap();
    let rx_ring = unsafe { alloc::alloc::alloc_zeroed(rx_ring_layout) as *mut I225RxDesc };

    let rx_bufs_layout = alloc::alloc::Layout::from_size_align(
        core::mem::size_of::<[[u8; RX_BUF_SIZE]; RING_SIZE]>(), 16,
    ).unwrap();
    let rx_bufs = unsafe {
        alloc::alloc::alloc_zeroed(rx_bufs_layout) as *mut [[u8; RX_BUF_SIZE]; RING_SIZE]
    };

    // Point each RX descriptor at its buffer
    for i in 0..RING_SIZE {
        let buf_virt = unsafe { &(*rx_bufs)[i] as *const u8 as u64 };
        let buf_phys = buf_virt.wrapping_sub(memory::phys_mem_offset().as_u64());
        unsafe {
            (*rx_ring.add(i)).addr = buf_phys;
            (*rx_ring.add(i)).info = 0;
        }
    }

    let rx_ring_phys = (rx_ring as u64).wrapping_sub(memory::phys_mem_offset().as_u64());
    mmio_write(mmio_base, REG_RDBAL, rx_ring_phys as u32);
    mmio_write(mmio_base, REG_RDBAH, (rx_ring_phys >> 32) as u32);
    mmio_write(mmio_base, REG_RDLEN, rx_ring_bytes as u32);
    mmio_write(mmio_base, REG_RDH, 0);
    mmio_write(mmio_base, REG_RDT, (RING_SIZE - 1) as u32);

    // Enable receiver
    mmio_write(mmio_base, REG_RCTL,
        RCTL_EN | RCTL_BAM | RCTL_BSIZE_2048 | RCTL_SECRC);

    // --- Allocate TX descriptor ring ---
    let tx_ring_bytes = RING_SIZE * core::mem::size_of::<I225TxDesc>();
    let tx_ring_layout = alloc::alloc::Layout::from_size_align(tx_ring_bytes, 128).unwrap();
    let tx_ring = unsafe { alloc::alloc::alloc_zeroed(tx_ring_layout) as *mut I225TxDesc };

    let tx_ring_phys = (tx_ring as u64).wrapping_sub(memory::phys_mem_offset().as_u64());
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
        DEVICE = Some(I225Device {
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

    // Enable interrupts: TX done, RX done, link status change
    mmio_write(mmio_base, REG_IMS, INT_TXDW | INT_RXDW | INT_LSC);

    let (link_up, speed, full_dup) = link_status(mmio_base);
    serial_println!("[i225] link={} speed={} duplex={} rings={}",
        if link_up { "up" } else { "down" },
        speed,
        if full_dup { "full" } else { "half" },
        RING_SIZE);
}

// ---------------------------------------------------------------------------
// Transmit
// ---------------------------------------------------------------------------

/// Transmit a raw Ethernet frame. Returns `true` if queued successfully.
pub fn send(data: &[u8]) -> bool {
    if !INITIALIZED.load(Ordering::SeqCst) || data.is_empty() || data.len() > 1518 {
        return false;
    }

    let dev = unsafe { (*(&raw mut DEVICE)).as_mut().unwrap() };
    let idx = dev.tx_cur;
    let desc = unsafe { &mut *dev.tx_ring.add(idx) };

    // Check if previous TX is still pending
    if desc.cmd_type_len != 0 && desc.olinfo_status & TXD_STAT_DD == 0 {
        TX_ERRORS.fetch_add(1, Ordering::Relaxed);
        return false;
    }

    let buf = data.to_vec();
    let buf_phys = (buf.as_ptr() as u64).wrapping_sub(memory::phys_mem_offset().as_u64());

    desc.addr = buf_phys;
    desc.cmd_type_len = TXD_DCMD_EOP | TXD_DCMD_IFCS | TXD_DCMD_RS | TXD_DCMD_DEXT
        | TXD_DTYP_ADV | (data.len() as u32);
    desc.olinfo_status = (data.len() as u32) << 14; // PAYLEN

    core::mem::forget(buf);

    dev.tx_cur = (idx + 1) % RING_SIZE;
    mmio_write(dev.mmio_base, REG_TDT, dev.tx_cur as u32);

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

    // Check for completed descriptor
    if desc.info & RXD_STAT_DD == 0 {
        return None;
    }

    // Must be end-of-packet
    if desc.info & RXD_STAT_EOP == 0 {
        desc.info = 0;
        dev.rx_cur = (idx + 1) % RING_SIZE;
        return None;
    }

    let len = ((desc.info >> RXD_LEN_SHIFT) & RXD_LEN_MASK) as usize;
    if len == 0 || len > RX_BUF_SIZE {
        RX_ERRORS.fetch_add(1, Ordering::Relaxed);
        desc.info = 0;
        dev.rx_cur = (idx + 1) % RING_SIZE;
        return None;
    }

    let frame = unsafe {
        let buf = &(*dev.rx_bufs)[idx];
        buf[..len].to_vec()
    };

    // Reset descriptor
    desc.info = 0;

    let old_idx = idx;
    dev.rx_cur = (idx + 1) % RING_SIZE;

    // Return consumed descriptor to NIC
    mmio_write(dev.mmio_base, REG_RDT, old_idx as u32);

    RX_PACKETS.fetch_add(1, Ordering::Relaxed);
    RX_BYTES.fetch_add(len as u64, Ordering::Relaxed);

    Some(frame)
}

// ---------------------------------------------------------------------------
// Interrupt handler
// ---------------------------------------------------------------------------

/// Handle an I225 interrupt.
pub fn handle_interrupt() {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return;
    }

    let dev = unsafe { (*(&raw const DEVICE)).as_ref().unwrap() };
    let icr = mmio_read(dev.mmio_base, REG_ICR);

    if icr == 0 {
        return;
    }

    INTERRUPTS.fetch_add(1, Ordering::Relaxed);

    if icr & INT_LSC != 0 {
        LINK_CHANGES.fetch_add(1, Ordering::Relaxed);
        let (up, speed, _dup) = link_status(dev.mmio_base);
        serial_println!("[i225] Link change: {} {}", if up { "up" } else { "down" }, speed);
    }
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
pub fn i225_info() -> String {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return alloc::format!("i225: not initialised (detected={})", is_detected());
    }

    let dev = unsafe { (*(&raw const DEVICE)).as_ref().unwrap() };
    let (link_up, speed, full_dup) = link_status(dev.mmio_base);

    alloc::format!(
        "i225: Intel {} MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}  link={}  speed={}  duplex={}  rings={}",
        dev.device_name,
        dev.mac[0], dev.mac[1], dev.mac[2], dev.mac[3], dev.mac[4], dev.mac[5],
        if link_up { "up" } else { "down" },
        speed,
        if full_dup { "full" } else { "half" },
        RING_SIZE,
    )
}

/// Return statistics as a human-readable string, combining hardware + software counters.
pub fn i225_stats() -> String {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return alloc::format!("i225: not initialised");
    }

    let dev = unsafe { (*(&raw const DEVICE)).as_ref().unwrap() };

    // Read hardware statistics registers
    let hw_gprc = mmio_read(dev.mmio_base, REG_GPRC);
    let hw_gptc = mmio_read(dev.mmio_base, REG_GPTC);
    let hw_gorc_lo = mmio_read(dev.mmio_base, REG_GORC_LO);
    let hw_gorc_hi = mmio_read(dev.mmio_base, REG_GORC_HI);
    let hw_gotc_lo = mmio_read(dev.mmio_base, REG_GOTC_LO);
    let hw_gotc_hi = mmio_read(dev.mmio_base, REG_GOTC_HI);
    let hw_crc = mmio_read(dev.mmio_base, REG_CRCERRS);
    let hw_rlec = mmio_read(dev.mmio_base, REG_RLEC);
    let hw_mpc = mmio_read(dev.mmio_base, REG_MPC);
    let hw_colc = mmio_read(dev.mmio_base, REG_COLC);
    let hw_mprc = mmio_read(dev.mmio_base, REG_MPRC);
    let hw_bprc = mmio_read(dev.mmio_base, REG_BPRC);

    let gorc = ((hw_gorc_hi as u64) << 32) | (hw_gorc_lo as u64);
    let gotc = ((hw_gotc_hi as u64) << 32) | (hw_gotc_lo as u64);

    // Software counters
    let sw_tx = TX_PACKETS.load(Ordering::Relaxed);
    let sw_rx = RX_PACKETS.load(Ordering::Relaxed);
    let tx_err = TX_ERRORS.load(Ordering::Relaxed);
    let rx_err = RX_ERRORS.load(Ordering::Relaxed);
    let lnk = LINK_CHANGES.load(Ordering::Relaxed);
    let irqs = INTERRUPTS.load(Ordering::Relaxed);

    alloc::format!(
        "i225 stats:\n  HW TX: {} pkts, {} bytes  |  HW RX: {} pkts, {} bytes\n  SW TX: {} pkts, {} err  |  SW RX: {} pkts, {} err\n  multicast: {}  broadcast: {}  CRC err: {}  length err: {}\n  missed: {}  collisions: {}  link changes: {}  interrupts: {}",
        hw_gptc, gotc, hw_gprc, gorc,
        sw_tx, tx_err, sw_rx, rx_err,
        hw_mprc, hw_bprc, hw_crc, hw_rlec,
        hw_mpc, hw_colc, lnk, irqs,
    )
}
