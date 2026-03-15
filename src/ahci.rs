/// AHCI (Advanced Host Controller Interface) SATA storage driver.
///
/// Discovers an AHCI HBA via PCI (class 01h, subclass 06h), reads BAR5
/// for MMIO registers, detects attached SATA devices, and issues
/// READ/WRITE DMA EXT commands for sector-level I/O.

use crate::{pci, memory, serial_println, klog_println};
use alloc::string::String;
use alloc::vec::Vec;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

const AHCI_CLASS: u8 = 0x01;
const AHCI_SUBCLASS: u8 = 0x06;
const MAX_PORTS: usize = 32;
const SATA_SIG_ATA: u32 = 0x0000_0101;
const PORT_DET_PRESENT: u32 = 0x3;
const PORT_IPM_ACTIVE: u32 = 0x1;
const FIS_TYPE_REG_H2D: u8 = 0x27;
const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;
const HBA_PX_CMD_ST: u32 = 1 << 0;
const HBA_PX_CMD_FRE: u32 = 1 << 4;
const HBA_PX_CMD_FR: u32 = 1 << 14;
const HBA_PX_CMD_CR: u32 = 1 << 15;
const SECTOR_SIZE: usize = 512;

/// AHCI HBA Generic Host Control registers (at BAR5 base).
#[repr(C)]
pub struct HbaMemory {
    pub cap: u32, pub ghc: u32, pub is: u32, pub pi: u32, pub vs: u32,
    pub ccc_ctl: u32, pub ccc_ports: u32, pub em_loc: u32, pub em_ctl: u32,
    pub cap2: u32, pub bohc: u32,
    _reserved: [u8; 0xA0 - 0x2C],
    _vendor: [u8; 0x100 - 0xA0],
    pub ports: [HbaPort; MAX_PORTS],
}

/// Per-port register set (0x80 bytes each, starting at BAR5 + 0x100).
#[repr(C)]
pub struct HbaPort {
    pub clb: u32, pub clbu: u32, pub fb: u32, pub fbu: u32,
    pub is: u32, pub ie: u32, pub cmd: u32, _rsv0: u32,
    pub tfd: u32, pub sig: u32, pub ssts: u32, pub sctl: u32,
    pub serr: u32, pub sact: u32, pub ci: u32, pub sntf: u32,
    pub fbs: u32, _rsv1: [u32; 11], _vendor: [u32; 4],
}

// ---------------------------------------------------------------------------
// Command list and FIS structures
// ---------------------------------------------------------------------------

/// Command Header in the Command List (32 bytes per slot).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HbaCmdHeader {
    /// Bits 0-4: CFL (FIS length in dwords), bit 6: Write, bit 5: ATAPI.
    pub flags: u16,
    /// Number of PRDT entries.
    pub prdtl: u16,
    /// Byte count transferred.
    pub prdbc: u32,
    /// Command Table base address (lo, 128-byte aligned).
    pub ctba: u32,
    /// Command Table base address (hi).
    pub ctbau: u32,
    _reserved: [u32; 4],
}

/// Physical Region Descriptor Table entry.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HbaPrdtEntry {
    pub dba: u32,       // Data Base Address (lo)
    pub dbau: u32,      // Data Base Address (hi)
    _reserved: u32,
    /// Byte count minus 1; bit 31 = interrupt on completion.
    pub dbc: u32,
}

/// Command Table (FIS + ATAPI + PRDT). Single PRDT entry for one-sector I/O.
#[repr(C)]
pub struct HbaCmdTable {
    pub cfis: [u8; 64],    // Command FIS
    pub acmd: [u8; 16],    // ATAPI command
    _reserved: [u8; 48],
    pub prdt: [HbaPrdtEntry; 1],
}

/// Register Host-to-Device FIS (20 bytes, used to issue ATA commands).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FisRegH2D {
    pub fis_type: u8, pub flags: u8, pub command: u8, pub featurel: u8,
    pub lba0: u8, pub lba1: u8, pub lba2: u8, pub device: u8,
    pub lba3: u8, pub lba4: u8, pub lba5: u8, pub featureh: u8,
    pub countl: u8, pub counth: u8, pub icc: u8, pub control: u8,
    _reserved: [u8; 4],
}

// ---------------------------------------------------------------------------
// Global driver state
// ---------------------------------------------------------------------------

static INITIALIZED: AtomicBool = AtomicBool::new(false);

struct AhciState {
    hba: *mut HbaMemory,
    active_ports: [bool; MAX_PORTS],
    port_count: usize,
}
unsafe impl Send for AhciState {}
unsafe impl Sync for AhciState {}

static mut STATE: AhciState = AhciState {
    hba: core::ptr::null_mut(),
    active_ports: [false; MAX_PORTS],
    port_count: 0,
};

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Scan PCI for an AHCI controller (class 01:06), read BAR5, enable AHCI
/// mode, and probe each implemented port for attached SATA devices.
pub fn init() {
    let devices = pci::scan();
    let dev = match devices.iter().find(|d| d.class == AHCI_CLASS && d.subclass == AHCI_SUBCLASS) {
        Some(d) => d,
        None => { serial_println!("[ahci] no AHCI controller found"); return; }
    };
    serial_println!("[ahci] found {} ({})", dev.summary(), dev.vendor_name());

    // BAR5 holds the AHCI Base Address Register (ABAR) — must be MMIO
    let bar5_lo = pci::pci_read32(dev.bus, dev.device, dev.function, 0x24);
    if bar5_lo & 0x1 != 0 {
        serial_println!("[ahci] BAR5 is I/O space, expected MMIO"); return;
    }
    let bar5_phys = (bar5_lo & 0xFFFF_F000) as u64;
    if bar5_phys == 0 {
        serial_println!("[ahci] BAR5 is zero"); return;
    }
    serial_println!("[ahci] ABAR physical: {:#x}", bar5_phys);

    let hba = memory::phys_to_virt(x86_64::PhysAddr::new(bar5_phys)).as_mut_ptr() as *mut HbaMemory;

    unsafe {
        // Enable AHCI mode (GHC.AE = bit 31)
        let ghc = ptr::read_volatile(&(*hba).ghc);
        ptr::write_volatile(&mut (*hba).ghc, ghc | (1 << 31));

        let version = ptr::read_volatile(&(*hba).vs);
        let cap = ptr::read_volatile(&(*hba).cap);
        serial_println!("[ahci] version {}.{}, max {} ports, {} cmd slots",
            (version >> 16) & 0xFFFF, version & 0xFFFF,
            (cap & 0x1F) + 1, ((cap >> 8) & 0x1F) + 1);

        STATE.hba = hba;

        // Probe implemented ports for present devices
        let pi = ptr::read_volatile(&(*hba).pi);
        let mut count = 0usize;
        for i in 0..MAX_PORTS {
            if pi & (1 << i) == 0 { continue; }
            let port = &(*hba).ports[i];
            let ssts = ptr::read_volatile(&port.ssts);
            if ssts & 0x0F == PORT_DET_PRESENT && (ssts >> 8) & 0x0F == PORT_IPM_ACTIVE {
                let sig = ptr::read_volatile(&port.sig);
                let kind = if sig == SATA_SIG_ATA { "SATA" } else { "other" };
                serial_println!("[ahci]   port {}: {} (sig={:#010x})", i, kind, sig);
                STATE.active_ports[i] = true;
                count += 1;
            }
        }
        STATE.port_count = count;

        if count == 0 { serial_println!("[ahci] no devices on any port"); return; }

        INITIALIZED.store(true, Ordering::SeqCst);
        serial_println!("[ahci] initialized, {} active port(s)", count);
        klog_println!("[ahci] initialized, {} SATA port(s) active", count);
        crate::blkdev::register("sda", 0);
        crate::driver::register("ahci", crate::driver::DriverKind::Block);
    }
}

/// Return indices of ports with active SATA devices.
pub fn detect_ports() -> Vec<usize> {
    if !INITIALIZED.load(Ordering::SeqCst) { return Vec::new(); }
    unsafe { (0..MAX_PORTS).filter(|&i| STATE.active_ports[i]).collect() }
}

// ---------------------------------------------------------------------------
// Port command engine helpers
// ---------------------------------------------------------------------------

/// Stop the command engine (clear ST and FRE, wait for CR+FR to clear).
unsafe fn stop_cmd(port: &mut HbaPort) {
    let cmd = ptr::read_volatile(&port.cmd);
    ptr::write_volatile(&mut port.cmd, cmd & !(HBA_PX_CMD_ST | HBA_PX_CMD_FRE));
    for _ in 0..1_000_000 {
        if ptr::read_volatile(&port.cmd) & (HBA_PX_CMD_FR | HBA_PX_CMD_CR) == 0 { return; }
        core::hint::spin_loop();
    }
}

/// Start the command engine (set FRE + ST after CR clears).
unsafe fn start_cmd(port: &mut HbaPort) {
    for _ in 0..1_000_000 {
        if ptr::read_volatile(&port.cmd) & HBA_PX_CMD_CR == 0 { break; }
        core::hint::spin_loop();
    }
    let cmd = ptr::read_volatile(&port.cmd);
    ptr::write_volatile(&mut port.cmd, cmd | HBA_PX_CMD_FRE | HBA_PX_CMD_ST);
}

/// Find a free command slot (not in SACT or CI).
unsafe fn find_free_slot(port: &HbaPort) -> Option<u32> {
    let busy = ptr::read_volatile(&port.sact) | ptr::read_volatile(&port.ci);
    (0..32u32).find(|&i| busy & (1 << i) == 0)
}

// ---------------------------------------------------------------------------
// Sector I/O
// ---------------------------------------------------------------------------

/// Read a single 512-byte sector from the given AHCI port.
pub fn read_sector(port: usize, sector: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), &'static str> {
    if !INITIALIZED.load(Ordering::SeqCst) { return Err("ahci: not initialized"); }
    unsafe {
        if port >= MAX_PORTS || !STATE.active_ports[port] { return Err("ahci: invalid port"); }
        issue_cmd(port, sector, buf.as_mut_ptr(), false)
    }
}

/// Write a single 512-byte sector to the given AHCI port.
pub fn write_sector(port: usize, sector: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), &'static str> {
    if !INITIALIZED.load(Ordering::SeqCst) { return Err("ahci: not initialized"); }
    unsafe {
        if port >= MAX_PORTS || !STATE.active_ports[port] { return Err("ahci: invalid port"); }
        issue_cmd(port, sector, buf.as_ptr() as *mut u8, true)
    }
}

/// Build and submit a READ/WRITE DMA EXT command via the port's command list.
unsafe fn issue_cmd(port_idx: usize, sector: u64, data: *mut u8, write: bool) -> Result<(), &'static str> {
    let hba = &mut *STATE.hba;
    let port = &mut hba.ports[port_idx];

    // Clear pending interrupt bits
    ptr::write_volatile(&mut port.is, u32::MAX);

    let slot = find_free_slot(port).ok_or("ahci: no free command slot")?;

    // Locate command header for this slot in the command list
    let clb = ptr::read_volatile(&port.clb) as u64
        | ((ptr::read_volatile(&port.clbu) as u64) << 32);
    let cmd_header = &mut *(memory::phys_to_virt(x86_64::PhysAddr::new(clb))
        .as_mut_ptr() as *mut HbaCmdHeader).add(slot as usize);

    let fis_dwords = (core::mem::size_of::<FisRegH2D>() / 4) as u16;
    cmd_header.flags = fis_dwords | if write { 1 << 6 } else { 0 };
    cmd_header.prdtl = 1;
    cmd_header.prdbc = 0;

    // Locate command table
    let ctba = cmd_header.ctba as u64 | ((cmd_header.ctbau as u64) << 32);
    let cmd_table = &mut *(memory::phys_to_virt(x86_64::PhysAddr::new(ctba))
        .as_mut_ptr() as *mut HbaCmdTable);

    // Zero the command FIS region, then fill Register H2D FIS
    ptr::write_bytes(cmd_table.cfis.as_mut_ptr(), 0, 64);
    let fis = &mut *(cmd_table.cfis.as_mut_ptr() as *mut FisRegH2D);
    fis.fis_type = FIS_TYPE_REG_H2D;
    fis.flags = 0x80; // command bit
    fis.command = if write { ATA_CMD_WRITE_DMA_EXT } else { ATA_CMD_READ_DMA_EXT };
    fis.device = 1 << 6; // LBA mode
    fis.lba0 = (sector & 0xFF) as u8;
    fis.lba1 = ((sector >> 8) & 0xFF) as u8;
    fis.lba2 = ((sector >> 16) & 0xFF) as u8;
    fis.lba3 = ((sector >> 24) & 0xFF) as u8;
    fis.lba4 = ((sector >> 32) & 0xFF) as u8;
    fis.lba5 = ((sector >> 40) & 0xFF) as u8;
    fis.countl = 1;
    fis.counth = 0;

    // PRDT: single entry pointing to the caller's data buffer
    let data_phys = data as u64 - memory::phys_mem_offset().as_u64();
    cmd_table.prdt[0].dba = data_phys as u32;
    cmd_table.prdt[0].dbau = (data_phys >> 32) as u32;
    cmd_table.prdt[0].dbc = (SECTOR_SIZE as u32) - 1;

    // Issue the command and spin until completion
    ptr::write_volatile(&mut port.ci, 1 << slot);
    for _ in 0..10_000_000u32 {
        if ptr::read_volatile(&port.ci) & (1 << slot) == 0 {
            if ptr::read_volatile(&port.tfd) & 0x01 != 0 {
                return Err("ahci: task file error");
            }
            return Ok(());
        }
        if ptr::read_volatile(&port.is) & (1 << 30) != 0 {
            return Err("ahci: fatal error during I/O");
        }
        core::hint::spin_loop();
    }
    Err("ahci: I/O timeout")
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Return whether the AHCI driver is active.
pub fn is_detected() -> bool { INITIALIZED.load(Ordering::SeqCst) }

/// Human-readable AHCI subsystem status.
pub fn info() -> String {
    if !is_detected() { return String::from("ahci: not detected"); }
    unsafe {
        let hba = &*STATE.hba;
        let v = ptr::read_volatile(&hba.vs);
        let cap = ptr::read_volatile(&hba.cap);
        let ports = detect_ports();
        let list: Vec<String> = ports.iter().map(|p| alloc::format!("{}", p)).collect();
        alloc::format!("ahci: v{}.{}, {} of {} ports active [{}]",
            (v >> 16) & 0xFFFF, v & 0xFFFF,
            STATE.port_count, (cap & 0x1F) + 1, list.join(", "))
    }
}
