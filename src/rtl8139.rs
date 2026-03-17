/// Realtek RTL8139 Fast Ethernet driver for MerlionOS.
/// Classic 100Mbps NIC — simple, well-documented, found in old PCs.
/// PCI Vendor: 0x10EC, Device: 0x8139

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::{driver, net, pci, serial_println};

// ---------------------------------------------------------------------------
// PCI identifiers
// ---------------------------------------------------------------------------

const RTL8139_VENDOR_ID: u16 = 0x10EC;
const RTL8139_DEVICE_ID: u16 = 0x8139;

// ---------------------------------------------------------------------------
// Register offsets (I/O port based)
// ---------------------------------------------------------------------------

/// MAC address registers (6 bytes at IDR0-IDR5).
const REG_IDR0: u16 = 0x00;
/// Transmit Status of Descriptor 0 (TSD0).
const REG_TSD0: u16 = 0x10;
/// Transmit Start Address of Descriptor 0 (TSAD0).
const REG_TSAD0: u16 = 0x20;
/// Receive Buffer Start Address.
const REG_RBSTART: u16 = 0x30;
/// Command Register.
const REG_CMD: u16 = 0x37;
/// Current Address of Packet Read (CAPR).
const REG_CAPR: u16 = 0x38;
/// Current Buffer Address (CBA) — write pointer.
const REG_CBA: u16 = 0x3A;
/// Interrupt Mask Register.
const REG_IMR: u16 = 0x3C;
/// Interrupt Status Register.
const REG_ISR: u16 = 0x3E;
/// Transmit Configuration Register.
const REG_TCR: u16 = 0x40;
/// Receive Configuration Register.
const REG_RCR: u16 = 0x44;
/// Configuration Register 1.
const REG_CONFIG1: u16 = 0x52;

// ---------------------------------------------------------------------------
// Command register bits
// ---------------------------------------------------------------------------

/// Buffer Empty flag.
const CMD_BUFE: u8 = 1 << 0;
/// Transmitter Enable.
const CMD_TE: u8 = 1 << 2;
/// Receiver Enable.
const CMD_RE: u8 = 1 << 3;
/// Software Reset.
const CMD_RST: u8 = 1 << 4;

// ---------------------------------------------------------------------------
// Interrupt bits
// ---------------------------------------------------------------------------

/// Receive OK.
const INT_ROK: u16 = 1 << 0;
/// Receive Error.
const INT_RER: u16 = 1 << 1;
/// Transmit OK.
const INT_TOK: u16 = 1 << 2;
/// Transmit Error.
const INT_TER: u16 = 1 << 3;
/// RX Buffer Overflow.
const INT_RXOVW: u16 = 1 << 4;
/// Link Change.
const INT_LINK_CHG: u16 = 1 << 5;
/// Timeout.
const INT_TIMEOUT: u16 = 1 << 14;
/// System Error.
const INT_SERR: u16 = 1 << 15;

/// All interrupt bits we care about.
const INT_MASK: u16 = INT_ROK | INT_RER | INT_TOK | INT_TER
    | INT_RXOVW | INT_LINK_CHG | INT_TIMEOUT | INT_SERR;

// ---------------------------------------------------------------------------
// RCR bits
// ---------------------------------------------------------------------------

/// Accept All Packets.
const RCR_AAP: u32 = 1 << 0;
/// Accept Physical Match.
const RCR_APM: u32 = 1 << 1;
/// Accept Multicast.
const RCR_AM: u32 = 1 << 2;
/// Accept Broadcast.
const RCR_AB: u32 = 1 << 3;
/// Wrap — allow RX buffer to wrap around.
const RCR_WRAP: u32 = 1 << 7;
/// Max DMA burst size: unlimited.
const RCR_MXDMA_UNLIMITED: u32 = 0x07 << 8;
/// RX buffer length: 8K + 16 bytes (RBLEN = 00).
const RCR_RBLEN_8K: u32 = 0 << 11;

// ---------------------------------------------------------------------------
// TCR bits
// ---------------------------------------------------------------------------

/// Max DMA burst: 2048 bytes.
const TCR_MXDMA_2048: u32 = 0x07 << 8;
/// Interframe Gap Time: standard 960ns.
const TCR_IFG_STANDARD: u32 = 0x03 << 24;

// ---------------------------------------------------------------------------
// TX descriptor bits (TSD registers)
// ---------------------------------------------------------------------------

/// OWN bit: set by driver, cleared by hardware when TX completes.
const TSD_OWN: u32 = 1 << 13;
/// TX OK flag (set by hardware on success).
const TSD_TOK: u32 = 1 << 15;
/// Size mask: bits 0-12 contain the packet size.
const TSD_SIZE_MASK: u32 = 0x1FFF;

/// Number of TX descriptors (RTL8139 has exactly 4).
const TX_DESC_COUNT: usize = 4;
/// Maximum TX packet size.
const TX_MAX_SIZE: usize = 1792;

// ---------------------------------------------------------------------------
// RX buffer
// ---------------------------------------------------------------------------

/// RX buffer size: 8KB + 16 byte header + 1500 byte padding for wrap.
const RX_BUF_SIZE: usize = 8192 + 16 + 1500;

/// RX packet header: status (2 bytes) + length (2 bytes).
const RX_HEADER_SIZE: usize = 4;
/// RX status: Receive OK.
const RX_STATUS_ROK: u16 = 1 << 0;

// ---------------------------------------------------------------------------
// Device state
// ---------------------------------------------------------------------------

struct Rtl8139Device {
    /// I/O port base address.
    io_base: u16,
    /// MAC address (6 bytes).
    mac: [u8; 6],
    /// PCI bus/device/function for identification.
    pci_bus: u8,
    pci_dev: u8,
    pci_func: u8,
    /// RX buffer (allocated on heap, pinned address given to NIC).
    rx_buf: Vec<u8>,
    /// Current read offset into the RX buffer.
    rx_offset: usize,
    /// TX descriptor buffers (4 buffers, each TX_MAX_SIZE bytes).
    tx_bufs: [Vec<u8>; TX_DESC_COUNT],
    /// Next TX descriptor to use (round-robin 0-3).
    tx_next: usize,
}

static DETECTED: AtomicBool = AtomicBool::new(false);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static mut DEVICE: Option<Rtl8139Device> = None;

// Statistics counters
static TX_PACKETS: AtomicU64 = AtomicU64::new(0);
static TX_BYTES: AtomicU64 = AtomicU64::new(0);
static RX_PACKETS: AtomicU64 = AtomicU64::new(0);
static RX_BYTES: AtomicU64 = AtomicU64::new(0);
static TX_ERRORS: AtomicU64 = AtomicU64::new(0);
static RX_ERRORS: AtomicU64 = AtomicU64::new(0);
static INTERRUPTS: AtomicU64 = AtomicU64::new(0);
static RX_OVERFLOWS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// I/O port helpers
// ---------------------------------------------------------------------------

fn io_read8(base: u16, reg: u16) -> u8 {
    unsafe {
        let mut port = x86_64::instructions::port::Port::<u8>::new(base + reg);
        port.read()
    }
}

fn io_write8(base: u16, reg: u16, val: u8) {
    unsafe {
        let mut port = x86_64::instructions::port::Port::<u8>::new(base + reg);
        port.write(val);
    }
}

fn io_read16(base: u16, reg: u16) -> u16 {
    unsafe {
        let mut port = x86_64::instructions::port::Port::<u16>::new(base + reg);
        port.read()
    }
}

fn io_write16(base: u16, reg: u16, val: u16) {
    unsafe {
        let mut port = x86_64::instructions::port::Port::<u16>::new(base + reg);
        port.write(val);
    }
}

fn io_read32(base: u16, reg: u16) -> u32 {
    unsafe {
        let mut port = x86_64::instructions::port::Port::<u32>::new(base + reg);
        port.read()
    }
}

fn io_write32(base: u16, reg: u16, val: u32) {
    unsafe {
        let mut port = x86_64::instructions::port::Port::<u32>::new(base + reg);
        port.write(val);
    }
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Perform a software reset of the RTL8139.
fn soft_reset(io_base: u16) {
    io_write8(io_base, REG_CMD, CMD_RST);
    // Spin until RST bit clears (hardware clears it when reset is done).
    for _ in 0..10000 {
        if io_read8(io_base, REG_CMD) & CMD_RST == 0 {
            return;
        }
        core::hint::spin_loop();
    }
    serial_println!("[rtl8139] WARNING: reset did not complete");
}

/// Read the 6-byte MAC address from IDR0-IDR5.
fn read_mac(io_base: u16) -> [u8; 6] {
    let mut mac = [0u8; 6];
    for i in 0..6 {
        mac[i] = io_read8(io_base, REG_IDR0 + i as u16);
    }
    mac
}

/// Read BAR0 to get I/O base address from PCI config space.
fn read_io_bar(bus: u8, device: u8, func: u8) -> u16 {
    let bar0 = pci::pci_read32(bus, device, func, 0x10);
    // BAR0 bit 0 = 1 means I/O space
    (bar0 & 0xFFFC) as u16
}

/// Enable PCI bus mastering for the device.
fn enable_bus_master(bus: u8, device: u8, func: u8) {
    let cmd = pci::pci_read32(bus, device, func, 0x04);
    // Set bit 2 (bus master) in PCI command register
    pci::pci_write32(bus, device, func, 0x04, cmd | 0x04);
}

/// Link speed / duplex detection via BMSR/BMCR (basic check).
fn link_status(io_base: u16) -> (bool, &'static str, bool) {
    // The RTL8139 reports link via the Media Status Register (0x58)
    // or via CONFIG1 bits. We use a simplified approach.
    let msr = io_read8(io_base, 0x58); // Media Status Register
    let link_up = msr & 0x04 == 0; // bit 2: 0=link OK, 1=link fail
    let speed_10 = msr & 0x08 != 0; // bit 3: 1=10Mbps, 0=100Mbps
    let speed = if speed_10 { "10Mbps" } else { "100Mbps" };
    let full_duplex = msr & 0x01 == 0; // simplified
    (link_up, speed, full_duplex)
}

// ---------------------------------------------------------------------------
// Transmit
// ---------------------------------------------------------------------------

/// Send a packet via the RTL8139.
/// Data must be <= TX_MAX_SIZE bytes.
pub fn send(data: &[u8]) -> Result<(), &'static str> {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return Err("rtl8139 not initialized");
    }
    if data.len() > TX_MAX_SIZE {
        return Err("packet too large for RTL8139 TX");
    }
    if data.is_empty() {
        return Err("empty packet");
    }

    let dev = unsafe { (*(&raw mut DEVICE)).as_mut().unwrap() };
    let desc = dev.tx_next;

    // Check that the descriptor is not still owned by hardware
    let tsd_reg = REG_TSD0 + (desc as u16) * 4;
    let tsd = io_read32(dev.io_base, tsd_reg);
    if tsd & TSD_OWN != 0 {
        // Hardware hasn't finished with this descriptor yet
        return Err("TX descriptor busy");
    }

    // Copy data into TX buffer
    let tx_buf = &mut dev.tx_bufs[desc];
    tx_buf[..data.len()].copy_from_slice(data);

    // Write start address to TSAD register
    let tsad_reg = REG_TSAD0 + (desc as u16) * 4;
    let buf_phys = tx_buf.as_ptr() as u32;
    io_write32(dev.io_base, tsad_reg, buf_phys);

    // Write size to TSD register (clears OWN bit, starts TX)
    io_write32(dev.io_base, tsd_reg, data.len() as u32 & TSD_SIZE_MASK);

    dev.tx_next = (desc + 1) % TX_DESC_COUNT;

    TX_PACKETS.fetch_add(1, Ordering::Relaxed);
    TX_BYTES.fetch_add(data.len() as u64, Ordering::Relaxed);
    Ok(())
}

// ---------------------------------------------------------------------------
// Receive
// ---------------------------------------------------------------------------

/// Attempt to receive a packet from the RTL8139 RX ring buffer.
pub fn recv() -> Option<Vec<u8>> {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return None;
    }

    let dev = unsafe { (*(&raw mut DEVICE)).as_mut()? };

    // Check if buffer is empty
    let cmd = io_read8(dev.io_base, REG_CMD);
    if cmd & CMD_BUFE != 0 {
        return None;
    }

    let offset = dev.rx_offset;
    let buf = &dev.rx_buf;

    // Read packet header: 2 bytes status + 2 bytes length
    let status = u16::from_le_bytes([
        buf[offset % RX_BUF_SIZE],
        buf[(offset + 1) % RX_BUF_SIZE],
    ]);
    let length = u16::from_le_bytes([
        buf[(offset + 2) % RX_BUF_SIZE],
        buf[(offset + 3) % RX_BUF_SIZE],
    ]) as usize;

    if status & RX_STATUS_ROK == 0 {
        RX_ERRORS.fetch_add(1, Ordering::Relaxed);
        // Skip bad packet — advance by header size
        dev.rx_offset = (offset + RX_HEADER_SIZE) % RX_BUF_SIZE;
        return None;
    }

    if length < 4 || length > 1518 + 4 {
        // Implausible length, skip
        RX_ERRORS.fetch_add(1, Ordering::Relaxed);
        dev.rx_offset = (offset + RX_HEADER_SIZE) % RX_BUF_SIZE;
        return None;
    }

    // Extract packet data (skip 4-byte header, length includes CRC)
    let data_len = length - 4; // subtract 4-byte CRC
    let mut pkt = Vec::with_capacity(data_len);
    for i in 0..data_len {
        pkt.push(buf[(offset + RX_HEADER_SIZE + i) % RX_BUF_SIZE]);
    }

    // Advance read pointer: header + length, aligned to 4 bytes
    let new_offset = (offset + RX_HEADER_SIZE + length + 3) & !3;
    dev.rx_offset = new_offset % RX_BUF_SIZE;

    // Update CAPR (Current Address of Packet Read) — offset - 16
    let capr_val = (dev.rx_offset as u16).wrapping_sub(16);
    io_write16(dev.io_base, REG_CAPR, capr_val);

    RX_PACKETS.fetch_add(1, Ordering::Relaxed);
    RX_BYTES.fetch_add(data_len as u64, Ordering::Relaxed);
    Some(pkt)
}

// ---------------------------------------------------------------------------
// Interrupt handler
// ---------------------------------------------------------------------------

/// Handle an RTL8139 interrupt. Call from the IRQ handler.
pub fn handle_interrupt() {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return;
    }

    let dev = unsafe { (*(&raw const DEVICE)).as_ref().unwrap() };
    let isr = io_read16(dev.io_base, REG_ISR);
    if isr == 0 {
        return;
    }

    INTERRUPTS.fetch_add(1, Ordering::Relaxed);

    // Acknowledge all pending bits
    io_write16(dev.io_base, REG_ISR, isr);

    if isr & INT_TOK != 0 {
        // TX completed
    }
    if isr & INT_TER != 0 {
        TX_ERRORS.fetch_add(1, Ordering::Relaxed);
    }
    if isr & INT_ROK != 0 {
        // RX available — picked up by next recv() call
    }
    if isr & INT_RER != 0 {
        RX_ERRORS.fetch_add(1, Ordering::Relaxed);
    }
    if isr & INT_RXOVW != 0 {
        RX_OVERFLOWS.fetch_add(1, Ordering::Relaxed);
        serial_println!("[rtl8139] RX buffer overflow");
    }
    if isr & INT_LINK_CHG != 0 {
        let (up, speed, _dup) = link_status(dev.io_base);
        serial_println!("[rtl8139] link change: {} {}", if up { "up" } else { "down" }, speed);
    }
    if isr & INT_SERR != 0 {
        serial_println!("[rtl8139] system error!");
    }
}

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

pub fn is_detected() -> bool {
    DETECTED.load(Ordering::SeqCst)
}

/// Return a human-readable info string.
pub fn rtl8139_info() -> String {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return format!("rtl8139: not initialised (detected={})", is_detected());
    }

    let dev = unsafe { (*(&raw const DEVICE)).as_ref().unwrap() };
    let (link_up, speed, full_dup) = link_status(dev.io_base);

    format!(
        "rtl8139: RTL8139 100Mbps NIC  MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}  link={}  speed={}  duplex={}  io=0x{:04X}",
        dev.mac[0], dev.mac[1], dev.mac[2], dev.mac[3], dev.mac[4], dev.mac[5],
        if link_up { "up" } else { "down" },
        speed,
        if full_dup { "full" } else { "half" },
        dev.io_base,
    )
}

/// Return statistics as a human-readable string.
pub fn rtl8139_stats() -> String {
    let tx_pkt = TX_PACKETS.load(Ordering::Relaxed);
    let tx_b = TX_BYTES.load(Ordering::Relaxed);
    let rx_pkt = RX_PACKETS.load(Ordering::Relaxed);
    let rx_b = RX_BYTES.load(Ordering::Relaxed);
    let tx_err = TX_ERRORS.load(Ordering::Relaxed);
    let rx_err = RX_ERRORS.load(Ordering::Relaxed);
    let overflows = RX_OVERFLOWS.load(Ordering::Relaxed);
    let irqs = INTERRUPTS.load(Ordering::Relaxed);

    format!(
        "rtl8139 stats:\n  TX: {} packets, {} bytes, {} errors\n  RX: {} packets, {} bytes, {} errors\n  RX overflows: {}  interrupts: {}",
        tx_pkt, tx_b, tx_err,
        rx_pkt, rx_b, rx_err,
        overflows, irqs,
    )
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the RTL8139 driver: scan PCI, configure the NIC.
pub fn init() {
    driver::register("rtl8139", driver::DriverKind::Serial);

    // Scan PCI for RTL8139
    let devices = pci::scan();
    let mut found = None;
    for d in &devices {
        if d.vendor_id == RTL8139_VENDOR_ID && d.device_id == RTL8139_DEVICE_ID {
            found = Some(d.clone());
            break;
        }
    }

    let pci_dev = match found {
        Some(d) => d,
        None => {
            serial_println!("[rtl8139] no RTL8139 NIC found on PCI bus");
            return;
        }
    };

    DETECTED.store(true, Ordering::SeqCst);
    serial_println!(
        "[rtl8139] found at PCI {:02x}:{:02x}.{}",
        pci_dev.bus, pci_dev.device, pci_dev.function,
    );

    // Enable bus mastering
    enable_bus_master(pci_dev.bus, pci_dev.device, pci_dev.function);

    // Read I/O base from BAR0
    let io_base = read_io_bar(pci_dev.bus, pci_dev.device, pci_dev.function);
    serial_println!("[rtl8139] I/O base: 0x{:04X}", io_base);

    // Power on: write 0x00 to CONFIG1
    io_write8(io_base, REG_CONFIG1, 0x00);

    // Software reset
    soft_reset(io_base);

    // Read MAC address
    let mac = read_mac(io_base);
    serial_println!(
        "[rtl8139] MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
    );

    // Allocate RX buffer
    let rx_buf = alloc::vec![0u8; RX_BUF_SIZE];
    let rx_phys = rx_buf.as_ptr() as u32;

    // Set RX buffer address
    io_write32(io_base, REG_RBSTART, rx_phys);

    // Configure RCR: accept broadcast + multicast + physical match, wrap, 8K buffer
    io_write32(
        io_base, REG_RCR,
        RCR_APM | RCR_AM | RCR_AB | RCR_WRAP | RCR_MXDMA_UNLIMITED | RCR_RBLEN_8K,
    );

    // Configure TCR: standard IFG, max DMA burst
    io_write32(io_base, REG_TCR, TCR_IFG_STANDARD | TCR_MXDMA_2048);

    // Enable TX and RX
    io_write8(io_base, REG_CMD, CMD_TE | CMD_RE);

    // Set interrupt mask
    io_write16(io_base, REG_IMR, INT_MASK);

    // Allocate TX buffers
    let tx_bufs = [
        alloc::vec![0u8; TX_MAX_SIZE],
        alloc::vec![0u8; TX_MAX_SIZE],
        alloc::vec![0u8; TX_MAX_SIZE],
        alloc::vec![0u8; TX_MAX_SIZE],
    ];

    // Register MAC with network stack
    {
        let mut ns = net::NET.lock();
        ns.mac = net::MacAddr(mac);
    }

    unsafe {
        DEVICE = Some(Rtl8139Device {
            io_base,
            mac,
            pci_bus: pci_dev.bus,
            pci_dev: pci_dev.device,
            pci_func: pci_dev.function,
            rx_buf,
            rx_offset: 0,
            tx_bufs,
            tx_next: 0,
        });
    }

    INITIALIZED.store(true, Ordering::SeqCst);

    let (link_up, speed, _dup) = link_status(io_base);
    serial_println!(
        "[rtl8139] initialized — link {} at {}",
        if link_up { "up" } else { "down" },
        speed,
    );
}
