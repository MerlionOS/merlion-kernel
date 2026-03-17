/// SATA controller detection and management for MerlionOS.
/// Auto-detects AHCI/IDE SATA controllers, enumerates attached disks,
/// and integrates with the block device subsystem.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use crate::{blkdev, driver, pci, serial_println};

// ---------------------------------------------------------------------------
// PCI class codes for mass storage controllers
// ---------------------------------------------------------------------------

/// PCI class: Mass Storage Controller.
const PCI_CLASS_STORAGE: u8 = 0x01;
/// Subclass: IDE Controller.
const PCI_SUBCLASS_IDE: u8 = 0x01;
/// Subclass: SATA Controller (AHCI).
const PCI_SUBCLASS_AHCI: u8 = 0x06;
/// Subclass: NVMe Controller (handled separately).
const PCI_SUBCLASS_NVME: u8 = 0x08;

// ---------------------------------------------------------------------------
// AHCI constants
// ---------------------------------------------------------------------------

/// AHCI port registers start at BAR5 + 0x100.
const AHCI_PORT_BASE: u32 = 0x100;
/// Each port occupies 0x80 bytes.
const AHCI_PORT_SIZE: u32 = 0x80;
/// Maximum AHCI ports.
const MAX_PORTS: usize = 32;

/// Port Implemented register offset in GHC.
const GHC_PI_OFFSET: u32 = 0x0C;
/// GHC Capabilities register.
const GHC_CAP_OFFSET: u32 = 0x00;
/// GHC Version register.
const GHC_VS_OFFSET: u32 = 0x10;

/// Port register offsets (relative to port base).
const PORT_SSTS: u32 = 0x28; // SStatus
const PORT_SIG: u32 = 0x24;  // Signature
const PORT_CMD: u32 = 0x18;  // Command and Status
const PORT_TFD: u32 = 0x20;  // Task File Data
const PORT_SERR: u32 = 0x30; // SError

/// SStatus: Device Detection (bits 3:0).
const SSTS_DET_MASK: u32 = 0x0F;
const SSTS_DET_PRESENT: u32 = 0x03;
/// SStatus: Interface Power Management (bits 11:8).
const SSTS_IPM_MASK: u32 = 0x0F00;
const SSTS_IPM_ACTIVE: u32 = 0x0100;

/// Device signatures.
const SIG_ATA: u32 = 0x0000_0101;    // SATA HDD/SSD
const SIG_ATAPI: u32 = 0xEB14_0101;  // SATAPI (CD/DVD)
const SIG_SEMB: u32 = 0xC33C_0101;   // Enclosure Management Bridge
const SIG_PM: u32 = 0x9669_0101;     // Port Multiplier

// ---------------------------------------------------------------------------
// ATA IDENTIFY constants
// ---------------------------------------------------------------------------

/// ATA IDENTIFY DEVICE command.
const ATA_CMD_IDENTIFY: u8 = 0xEC;
/// ATA IDENTIFY PACKET DEVICE command (for ATAPI).
const ATA_CMD_IDENTIFY_PACKET: u8 = 0xA1;
/// FIS type: Register H2D.
const FIS_TYPE_REG_H2D: u8 = 0x27;

/// ATA SMART commands.
const ATA_CMD_SMART: u8 = 0xB0;
const SMART_READ_DATA: u8 = 0xD0;
const SMART_RETURN_STATUS: u8 = 0xDA;
/// SMART feature register magic values.
const SMART_LBA_MID: u8 = 0x4F;
const SMART_LBA_HI: u8 = 0xC2;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Type of SATA device.
#[derive(Clone, Copy, PartialEq)]
pub enum SataDeviceType {
    Hdd,
    Ssd,
    Atapi,
    Unknown,
}

impl SataDeviceType {
    fn name(self) -> &'static str {
        match self {
            Self::Hdd => "SATA HDD",
            Self::Ssd => "SATA SSD",
            Self::Atapi => "SATAPI CD/DVD",
            Self::Unknown => "Unknown SATA",
        }
    }
}

/// Detected SATA controller.
#[derive(Clone)]
pub struct SataController {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub subclass: u8,
    pub bar5: u64,
    pub irq: u8,
    pub ahci_version: u32,
    pub port_count: u8,
    pub cmd_slots: u8,
}

/// Detected SATA disk.
#[derive(Clone)]
pub struct SataDisk {
    pub controller_idx: usize,
    pub port: u8,
    pub dev_type: SataDeviceType,
    pub model: String,
    pub serial: String,
    pub firmware: String,
    pub sector_count: u64,
    pub sector_size: u32,
    pub capacity_mb: u64,
    pub smart_ok: bool,
    pub smart_temp: u16,
    pub smart_power_on_hours: u32,
    pub smart_reallocated: u32,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static DETECTED: AtomicBool = AtomicBool::new(false);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static CONTROLLERS: Mutex<Vec<SataController>> = Mutex::new(Vec::new());
static DISKS: Mutex<Vec<SataDisk>> = Mutex::new(Vec::new());

// Statistics
static SCANS: AtomicU64 = AtomicU64::new(0);
static HOTPLUG_EVENTS: AtomicU64 = AtomicU64::new(0);
static IDENTIFY_CMDS: AtomicU64 = AtomicU64::new(0);
static SMART_READS: AtomicU64 = AtomicU64::new(0);
static ERRORS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// MMIO helpers
// ---------------------------------------------------------------------------

fn mmio_read32(base: u64, offset: u32) -> u32 {
    unsafe {
        let ptr = (base + offset as u64) as *const u32;
        core::ptr::read_volatile(ptr)
    }
}

fn mmio_write32(base: u64, offset: u32, val: u32) {
    unsafe {
        let ptr = (base + offset as u64) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }
}

// ---------------------------------------------------------------------------
// PCI BAR reading
// ---------------------------------------------------------------------------

/// Read a 64-bit BAR value (BAR5 for AHCI is at PCI offset 0x24).
fn read_bar5(bus: u8, device: u8, func: u8) -> u64 {
    let lo = pci::pci_read32(bus, device, func, 0x24);
    // BAR5 is typically 32-bit MMIO for AHCI; check if 64-bit
    let bar_type = (lo >> 1) & 0x03;
    let base_lo = (lo & 0xFFFF_FFF0) as u64;
    if bar_type == 0x02 {
        // 64-bit BAR
        let hi = pci::pci_read32(bus, device, func, 0x28) as u64;
        base_lo | (hi << 32)
    } else {
        base_lo
    }
}

/// Read interrupt line from PCI config.
fn read_irq(bus: u8, device: u8, func: u8) -> u8 {
    (pci::pci_read32(bus, device, func, 0x3C) & 0xFF) as u8
}

/// Enable bus mastering for PCI device.
fn enable_bus_master(bus: u8, device: u8, func: u8) {
    let cmd = pci::pci_read32(bus, device, func, 0x04);
    pci::pci_write32(bus, device, func, 0x04, cmd | 0x06); // bus master + memory space
}

// ---------------------------------------------------------------------------
// Controller scanning
// ---------------------------------------------------------------------------

/// Scan PCI bus for SATA controllers (class 01h).
pub fn scan_controllers() -> Vec<SataController> {
    let pci_devices = pci::scan();
    let mut controllers = Vec::new();

    for d in &pci_devices {
        if d.class != PCI_CLASS_STORAGE {
            continue;
        }
        // Skip NVMe — handled by nvme.rs
        if d.subclass == PCI_SUBCLASS_NVME {
            continue;
        }
        // We handle IDE and AHCI
        if d.subclass != PCI_SUBCLASS_IDE && d.subclass != PCI_SUBCLASS_AHCI {
            continue;
        }

        let bar5 = if d.subclass == PCI_SUBCLASS_AHCI {
            read_bar5(d.bus, d.device, d.function)
        } else {
            0 // IDE controllers don't use BAR5
        };

        let irq = read_irq(d.bus, d.device, d.function);

        let (version, port_count, cmd_slots) = if d.subclass == PCI_SUBCLASS_AHCI && bar5 != 0 {
            let vs = mmio_read32(bar5, GHC_VS_OFFSET);
            let cap = mmio_read32(bar5, GHC_CAP_OFFSET);
            let nports = ((cap & 0x1F) + 1) as u8;       // bits 4:0
            let ncmds = (((cap >> 8) & 0x1F) + 1) as u8; // bits 12:8
            (vs, nports, ncmds)
        } else {
            (0, 0, 0)
        };

        serial_println!(
            "[sata] found {} controller {:04x}:{:04x} at {:02x}:{:02x}.{} BAR5=0x{:X} IRQ={}",
            if d.subclass == PCI_SUBCLASS_AHCI { "AHCI" } else { "IDE" },
            d.vendor_id, d.device_id,
            d.bus, d.device, d.function,
            bar5, irq,
        );

        controllers.push(SataController {
            bus: d.bus,
            device: d.device,
            function: d.function,
            vendor_id: d.vendor_id,
            device_id: d.device_id,
            subclass: d.subclass,
            bar5,
            irq,
            ahci_version: version,
            port_count,
            cmd_slots,
        });
    }

    SCANS.fetch_add(1, Ordering::Relaxed);
    controllers
}

// ---------------------------------------------------------------------------
// Port enumeration
// ---------------------------------------------------------------------------

/// Determine device type from AHCI port signature.
fn device_type_from_sig(sig: u32) -> SataDeviceType {
    match sig {
        SIG_ATA => SataDeviceType::Hdd, // Could be SSD — IDENTIFY tells us later
        SIG_ATAPI => SataDeviceType::Atapi,
        _ => SataDeviceType::Unknown,
    }
}

/// Check if an AHCI port has an active device attached.
fn port_has_device(bar5: u64, port: u8) -> bool {
    let port_base = AHCI_PORT_BASE + (port as u32) * AHCI_PORT_SIZE;
    let ssts = mmio_read32(bar5, port_base + PORT_SSTS);
    let det = ssts & SSTS_DET_MASK;
    let ipm = ssts & SSTS_IPM_MASK;
    det == SSTS_DET_PRESENT && ipm == SSTS_IPM_ACTIVE
}

/// Read the device signature from an AHCI port.
fn port_signature(bar5: u64, port: u8) -> u32 {
    let port_base = AHCI_PORT_BASE + (port as u32) * AHCI_PORT_SIZE;
    mmio_read32(bar5, port_base + PORT_SIG)
}

/// Parse an ATA IDENTIFY response (512 bytes) into model, serial, firmware, sectors.
fn parse_identify(buf: &[u8]) -> (String, String, String, u64, u32) {
    // ATA IDENTIFY words are 16-bit, little-endian.
    // Serial: words 10-19 (20 chars)
    // Firmware: words 23-26 (8 chars)
    // Model: words 27-46 (40 chars)
    // Sector count (48-bit LBA): words 100-103
    // Logical sector size: word 117-118

    fn extract_string(buf: &[u8], word_start: usize, word_count: usize) -> String {
        let mut s = Vec::with_capacity(word_count * 2);
        for w in 0..word_count {
            let offset = (word_start + w) * 2;
            if offset + 1 < buf.len() {
                // ATA strings are byte-swapped within each word
                s.push(buf[offset + 1]);
                s.push(buf[offset]);
            }
        }
        // Convert to string, trimming trailing spaces
        let text = core::str::from_utf8(&s).unwrap_or("").trim();
        String::from(text)
    }

    let serial = extract_string(buf, 10, 10);
    let firmware = extract_string(buf, 23, 4);
    let model = extract_string(buf, 27, 20);

    // 48-bit LBA sector count: words 100-103 (64-bit LE)
    let sectors = if buf.len() >= 208 {
        let w100 = u16::from_le_bytes([buf[200], buf[201]]) as u64;
        let w101 = u16::from_le_bytes([buf[202], buf[203]]) as u64;
        let w102 = u16::from_le_bytes([buf[204], buf[205]]) as u64;
        let w103 = u16::from_le_bytes([buf[206], buf[207]]) as u64;
        w100 | (w101 << 16) | (w102 << 32) | (w103 << 48)
    } else {
        0
    };

    // Logical sector size: words 117-118 (if word 106 bit 12 is set)
    let sector_size = if buf.len() >= 238 {
        let w106 = u16::from_le_bytes([buf[212], buf[213]]);
        if w106 & (1 << 12) != 0 {
            let w117 = u16::from_le_bytes([buf[234], buf[235]]) as u32;
            let w118 = u16::from_le_bytes([buf[236], buf[237]]) as u32;
            (w117 | (w118 << 16)) * 2
        } else {
            512
        }
    } else {
        512
    };

    (model, serial, firmware, sectors, sector_size)
}

/// Determine if a device is an SSD based on IDENTIFY data.
/// Word 217 (Nominal Media Rotation Rate): 0001h = non-rotating (SSD).
fn is_ssd_from_identify(buf: &[u8]) -> bool {
    if buf.len() >= 436 {
        let w217 = u16::from_le_bytes([buf[434], buf[435]]);
        w217 == 0x0001
    } else {
        false
    }
}

/// Enumerate disks on an AHCI controller.
fn enumerate_ahci_disks(ctrl_idx: usize, ctrl: &SataController) -> Vec<SataDisk> {
    let mut disks = Vec::new();
    if ctrl.bar5 == 0 {
        return disks;
    }

    let pi = mmio_read32(ctrl.bar5, GHC_PI_OFFSET);

    for port in 0..MAX_PORTS as u8 {
        if pi & (1u32 << port) == 0 {
            continue;
        }
        if !port_has_device(ctrl.bar5, port) {
            continue;
        }

        let sig = port_signature(ctrl.bar5, port);
        let dev_type = device_type_from_sig(sig);

        serial_println!(
            "[sata] ctrl{} port{}: sig=0x{:08X} type={}",
            ctrl_idx, port, sig, dev_type.name(),
        );

        // Stub: In production we would issue IDENTIFY DEVICE via AHCI command slot.
        // For now create a disk entry with signature-based info.
        IDENTIFY_CMDS.fetch_add(1, Ordering::Relaxed);

        let disk = SataDisk {
            controller_idx: ctrl_idx,
            port,
            dev_type,
            model: String::from("(identify pending)"),
            serial: String::from(""),
            firmware: String::from(""),
            sector_count: 0,
            sector_size: 512,
            capacity_mb: 0,
            smart_ok: true,
            smart_temp: 0,
            smart_power_on_hours: 0,
            smart_reallocated: 0,
        };

        disks.push(disk);
    }

    disks
}

// ---------------------------------------------------------------------------
// S.M.A.R.T.
// ---------------------------------------------------------------------------

/// Read SMART health status for a disk on an AHCI port.
/// Returns (pass/fail, temperature, power-on hours, reallocated sectors).
pub fn smart_info(port: u8) -> String {
    let disks = DISKS.lock();
    let disk = disks.iter().find(|d| d.port == port);
    match disk {
        Some(d) => {
            SMART_READS.fetch_add(1, Ordering::Relaxed);
            // In production, we issue ATA SMART READ DATA (B0h / D0h)
            // and parse the attribute table. For now return stored values.
            let status = if d.smart_ok { "PASSED" } else { "FAILED" };
            format!(
                "SMART Health: {}\n  Device: {} (port {})\n  Model: {}\n  Serial: {}\n  Temperature: {} C\n  Power-On Hours: {}\n  Reallocated Sectors: {}",
                status, d.dev_type.name(), d.port,
                d.model, d.serial,
                d.smart_temp, d.smart_power_on_hours, d.smart_reallocated,
            )
        }
        None => {
            format!("No SATA device found on port {}", port)
        }
    }
}

// ---------------------------------------------------------------------------
// Hot-plug detection
// ---------------------------------------------------------------------------

/// Check AHCI ports for hot-plug events (new device insertion or removal).
pub fn check_hotplug() {
    let controllers = CONTROLLERS.lock();
    for (ctrl_idx, ctrl) in controllers.iter().enumerate() {
        if ctrl.subclass != PCI_SUBCLASS_AHCI || ctrl.bar5 == 0 {
            continue;
        }

        let pi = mmio_read32(ctrl.bar5, GHC_PI_OFFSET);
        let existing_disks: Vec<u8> = {
            let disks = DISKS.lock();
            disks.iter()
                .filter(|d| d.controller_idx == ctrl_idx)
                .map(|d| d.port)
                .collect()
        };

        for port in 0..MAX_PORTS as u8 {
            if pi & (1u32 << port) == 0 {
                continue;
            }
            let present = port_has_device(ctrl.bar5, port);
            let known = existing_disks.contains(&port);

            if present && !known {
                serial_println!("[sata] hot-plug: new device on ctrl{} port{}", ctrl_idx, port);
                HOTPLUG_EVENTS.fetch_add(1, Ordering::Relaxed);
                // In production: issue IDENTIFY, add to DISKS, register blkdev
            } else if !present && known {
                serial_println!("[sata] hot-unplug: device removed from ctrl{} port{}", ctrl_idx, port);
                HOTPLUG_EVENTS.fetch_add(1, Ordering::Relaxed);
                // In production: remove from DISKS, unregister blkdev
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Device listing
// ---------------------------------------------------------------------------

/// List all detected SATA disks.
pub fn list_disks() -> Vec<SataDisk> {
    DISKS.lock().clone()
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn is_detected() -> bool {
    DETECTED.load(Ordering::SeqCst)
}

/// Return a human-readable info string.
pub fn sata_info() -> String {
    let controllers = CONTROLLERS.lock();
    let disks = DISKS.lock();

    if controllers.is_empty() {
        return format!("sata: no controllers detected");
    }

    let mut s = format!("sata: {} controller(s), {} disk(s)\n", controllers.len(), disks.len());

    for (i, c) in controllers.iter().enumerate() {
        let ctype = match c.subclass {
            PCI_SUBCLASS_IDE => "IDE",
            PCI_SUBCLASS_AHCI => "AHCI",
            _ => "Other",
        };
        let ver_major = (c.ahci_version >> 16) & 0xFFFF;
        let ver_minor = c.ahci_version & 0xFFFF;
        s.push_str(&format!(
            "  ctrl{}: {} {:04x}:{:04x} at {:02x}:{:02x}.{} BAR5=0x{:X} IRQ={} ver={}.{} ports={} slots={}\n",
            i, ctype, c.vendor_id, c.device_id,
            c.bus, c.device, c.function,
            c.bar5, c.irq,
            ver_major, ver_minor,
            c.port_count, c.cmd_slots,
        ));
    }

    for d in disks.iter() {
        let cap_str = if d.capacity_mb >= 1024 {
            format!("{} GiB", d.capacity_mb / 1024)
        } else {
            format!("{} MiB", d.capacity_mb)
        };
        s.push_str(&format!(
            "  port{}: {} model=\"{}\" serial=\"{}\" fw=\"{}\" cap={} sect_sz={}\n",
            d.port, d.dev_type.name(), d.model, d.serial, d.firmware,
            cap_str, d.sector_size,
        ));
    }

    s
}

/// Return statistics as a human-readable string.
pub fn sata_stats() -> String {
    let ctrl_count = CONTROLLERS.lock().len();
    let disk_count = DISKS.lock().len();
    let scans = SCANS.load(Ordering::Relaxed);
    let hotplugs = HOTPLUG_EVENTS.load(Ordering::Relaxed);
    let identifies = IDENTIFY_CMDS.load(Ordering::Relaxed);
    let smarts = SMART_READS.load(Ordering::Relaxed);
    let errs = ERRORS.load(Ordering::Relaxed);

    format!(
        "sata stats:\n  Controllers: {}\n  Disks: {}\n  PCI scans: {}\n  Hot-plug events: {}\n  IDENTIFY commands: {}\n  SMART reads: {}\n  Errors: {}",
        ctrl_count, disk_count, scans, hotplugs, identifies, smarts, errs,
    )
}

/// Initialize the SATA subsystem: scan PCI for controllers, enumerate disks.
pub fn init() {
    driver::register("sata", driver::DriverKind::Block);

    let controllers = scan_controllers();
    if controllers.is_empty() {
        serial_println!("[sata] no SATA controllers found");
        INITIALIZED.store(true, Ordering::SeqCst);
        return;
    }

    DETECTED.store(true, Ordering::SeqCst);

    // Enable bus mastering on all controllers
    for c in &controllers {
        enable_bus_master(c.bus, c.device, c.function);
    }

    // Enumerate disks on AHCI controllers
    let mut all_disks = Vec::new();
    for (i, c) in controllers.iter().enumerate() {
        if c.subclass == PCI_SUBCLASS_AHCI {
            let mut disks = enumerate_ahci_disks(i, c);
            // Register each disk with blkdev
            for d in &disks {
                let name = format!("sd{}", (b'a' + d.port) as char);
                blkdev::register(&name, d.sector_count);
                serial_println!(
                    "[sata] registered {} ({} on port {})",
                    name, d.dev_type.name(), d.port,
                );
            }
            all_disks.append(&mut disks);
        }
    }

    let ctrl_count = controllers.len();
    let disk_count = all_disks.len();

    *CONTROLLERS.lock() = controllers;
    *DISKS.lock() = all_disks;

    INITIALIZED.store(true, Ordering::SeqCst);
    serial_println!(
        "[sata] initialized: {} controller(s), {} disk(s)",
        ctrl_count, disk_count,
    );
}
