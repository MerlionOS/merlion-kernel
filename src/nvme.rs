/// NVMe (Non-Volatile Memory Express) storage driver.
///
/// Discovers an NVMe controller via PCI (class 01h, subclass 08h, prog-if 02h),
/// reads BAR0 for MMIO registers, resets the controller, creates admin and I/O
/// queue pairs, and issues Read/Write commands for sector-level I/O.

use crate::{pci, memory, serial_println, klog_println};
use alloc::string::String;
use alloc::vec::Vec;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// PCI class for NVMe: Mass Storage (01h), NVM (08h), NVMe (02h)
// ---------------------------------------------------------------------------

const NVME_CLASS: u8 = 0x01;
const NVME_SUBCLASS: u8 = 0x08;
const NVME_PROG_IF: u8 = 0x02;

const SECTOR_SIZE: usize = 512;
const ADMIN_QUEUE_DEPTH: usize = 16;
const IO_QUEUE_DEPTH: usize = 64;
const PAGE_SIZE: usize = 4096;

// ---------------------------------------------------------------------------
// Admin command opcodes
// ---------------------------------------------------------------------------

const ADMIN_OPC_IDENTIFY: u8 = 0x06;
const ADMIN_OPC_CREATE_IO_CQ: u8 = 0x05;
const ADMIN_OPC_CREATE_IO_SQ: u8 = 0x01;

// ---------------------------------------------------------------------------
// NVM I/O command opcodes
// ---------------------------------------------------------------------------

const IO_OPC_READ: u8 = 0x02;
const IO_OPC_WRITE: u8 = 0x01;

// ---------------------------------------------------------------------------
// Controller register offsets and masks
// ---------------------------------------------------------------------------

const CC_EN: u32 = 1 << 0;
const CSTS_RDY: u32 = 1 << 0;

// ---------------------------------------------------------------------------
// NVMe controller registers (BAR0, MMIO)
// ---------------------------------------------------------------------------

/// NVMe controller register block mapped at BAR0.
///
/// Only the first registers up to ACQ are laid out here; the remainder
/// of the 4 KiB register page (doorbell stride varies) follows after.
#[repr(C)]
pub struct NvmeRegs {
    /// Controller Capabilities (offset 0x00).
    pub cap: u64,
    /// Version (offset 0x08).
    pub vs: u32,
    /// Interrupt Mask Set (offset 0x0C).
    pub intms: u32,
    /// Interrupt Mask Clear (offset 0x10).
    pub intmc: u32,
    /// Controller Configuration (offset 0x14).
    pub cc: u32,
    /// Reserved (offset 0x18).
    _rsvd0: u32,
    /// Controller Status (offset 0x1C).
    pub csts: u32,
    /// NVM Subsystem Reset (offset 0x20).
    pub nssr: u32,
    /// Admin Queue Attributes (offset 0x24).
    pub aqa: u32,
    /// Admin Submission Queue Base Address (offset 0x28).
    pub asq: u64,
    /// Admin Completion Queue Base Address (offset 0x30).
    pub acq: u64,
}

// ---------------------------------------------------------------------------
// Submission Queue Entry (64 bytes)
// ---------------------------------------------------------------------------

/// NVMe Submission Queue Entry — 64 bytes.
///
/// Used for both admin and I/O commands. The `cdw10`..`cdw15` fields carry
/// command-specific parameters.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NvmeSqe {
    /// Command Dword 0: opcode (7:0), fused (9:8), PSDT (15:14), CID (31:16).
    pub cdw0: u32,
    /// Namespace Identifier.
    pub nsid: u32,
    /// Reserved.
    pub cdw2: u32,
    /// Reserved.
    pub cdw3: u32,
    /// Metadata pointer.
    pub mptr: u64,
    /// Data pointer — PRP entry 1.
    pub prp1: u64,
    /// Data pointer — PRP entry 2.
    pub prp2: u64,
    /// Command-specific dword 10.
    pub cdw10: u32,
    /// Command-specific dword 11.
    pub cdw11: u32,
    /// Command-specific dword 12.
    pub cdw12: u32,
    /// Command-specific dword 13.
    pub cdw13: u32,
    /// Command-specific dword 14.
    pub cdw14: u32,
    /// Command-specific dword 15.
    pub cdw15: u32,
}

impl NvmeSqe {
    const fn zeroed() -> Self {
        Self {
            cdw0: 0, nsid: 0, cdw2: 0, cdw3: 0,
            mptr: 0, prp1: 0, prp2: 0,
            cdw10: 0, cdw11: 0, cdw12: 0,
            cdw13: 0, cdw14: 0, cdw15: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Completion Queue Entry (16 bytes)
// ---------------------------------------------------------------------------

/// NVMe Completion Queue Entry — 16 bytes.
///
/// The controller writes these to the CQ ring to indicate command completion.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NvmeCqe {
    /// Command-specific result dword 0.
    pub dw0: u32,
    /// Reserved.
    pub dw1: u32,
    /// SQ Head pointer (15:0), SQ Identifier (31:16).
    pub sq_head_sqid: u32,
    /// Command Identifier (15:0), Phase Tag (bit 16), Status Field (31:17).
    pub status_cid: u32,
}

impl NvmeCqe {
    /// Extract the Phase Tag bit from this CQE.
    pub fn phase(&self) -> bool {
        self.status_cid & (1 << 16) != 0
    }

    /// Extract the status code field (bits 31:17 → shifted right by 17).
    pub fn status_code(&self) -> u16 {
        ((self.status_cid >> 17) & 0x7FF) as u16
    }
}

// ---------------------------------------------------------------------------
// Global driver state
// ---------------------------------------------------------------------------

static INITIALIZED: AtomicBool = AtomicBool::new(false);

struct NvmeState {
    regs: *mut NvmeRegs,
    /// Doorbell stride in bytes (4 << CAP.DSTRD).
    dstrd: usize,

    // Admin queue pair
    admin_sq: *mut NvmeSqe,
    admin_cq: *mut NvmeCqe,
    admin_sq_tail: u16,
    admin_cq_head: u16,
    admin_cq_phase: bool,

    // I/O queue pair (QID = 1)
    io_sq: *mut NvmeSqe,
    io_cq: *mut NvmeCqe,
    io_sq_tail: u16,
    io_cq_head: u16,
    io_cq_phase: bool,

    /// Command identifier counter (wraps around).
    next_cid: u16,
    /// Total namespace size in 512-byte sectors (from Identify Namespace).
    ns_blocks: u64,
    /// Serial number string from Identify Controller.
    serial: [u8; 20],
    /// Model number string from Identify Controller.
    model: [u8; 40],
}

unsafe impl Send for NvmeState {}
unsafe impl Sync for NvmeState {}

static mut STATE: NvmeState = NvmeState {
    regs: core::ptr::null_mut(),
    dstrd: 0,
    admin_sq: core::ptr::null_mut(),
    admin_cq: core::ptr::null_mut(),
    admin_sq_tail: 0,
    admin_cq_head: 0,
    admin_cq_phase: true,
    io_sq: core::ptr::null_mut(),
    io_cq: core::ptr::null_mut(),
    io_sq_tail: 0,
    io_cq_head: 0,
    io_cq_phase: true,
    next_cid: 1,
    ns_blocks: 0,
    serial: [0u8; 20],
    model: [0u8; 40],
};

// ---------------------------------------------------------------------------
// Doorbell helpers
// ---------------------------------------------------------------------------

/// Write the Submission Queue `tail` doorbell for queue `qid`.
unsafe fn ring_sq_doorbell(qid: u16, tail: u16) {
    let base = STATE.regs as *mut u8;
    // SQ y Tail Doorbell offset = 0x1000 + (2y * dstrd)
    let offset = 0x1000 + (2 * qid as usize) * STATE.dstrd;
    let doorbell = base.add(offset) as *mut u32;
    ptr::write_volatile(doorbell, tail as u32);
}

/// Write the Completion Queue `head` doorbell for queue `qid`.
unsafe fn ring_cq_doorbell(qid: u16, head: u16) {
    let base = STATE.regs as *mut u8;
    // CQ y Head Doorbell offset = 0x1000 + (2y + 1) * dstrd
    let offset = 0x1000 + (2 * qid as usize + 1) * STATE.dstrd;
    let doorbell = base.add(offset) as *mut u32;
    ptr::write_volatile(doorbell, head as u32);
}

// ---------------------------------------------------------------------------
// Submit / complete helpers
// ---------------------------------------------------------------------------

/// Allocate the next command identifier.
unsafe fn alloc_cid() -> u16 {
    let cid = STATE.next_cid;
    STATE.next_cid = STATE.next_cid.wrapping_add(1);
    if STATE.next_cid == 0 { STATE.next_cid = 1; }
    cid
}

/// Submit a command on the admin queue and spin until it completes.
unsafe fn admin_submit_and_wait(cmd: &NvmeSqe) -> Result<NvmeCqe, &'static str> {
    let idx = STATE.admin_sq_tail as usize;
    ptr::write_volatile(STATE.admin_sq.add(idx), *cmd);
    STATE.admin_sq_tail = ((idx + 1) % ADMIN_QUEUE_DEPTH) as u16;
    ring_sq_doorbell(0, STATE.admin_sq_tail);

    // Spin-wait for completion
    for _ in 0..10_000_000u32 {
        let cqe = ptr::read_volatile(STATE.admin_cq.add(STATE.admin_cq_head as usize));
        if cqe.phase() == STATE.admin_cq_phase {
            // Advance CQ head
            STATE.admin_cq_head += 1;
            if STATE.admin_cq_head as usize >= ADMIN_QUEUE_DEPTH {
                STATE.admin_cq_head = 0;
                STATE.admin_cq_phase = !STATE.admin_cq_phase;
            }
            ring_cq_doorbell(0, STATE.admin_cq_head);
            if cqe.status_code() != 0 {
                return Err("nvme: admin command failed");
            }
            return Ok(cqe);
        }
        core::hint::spin_loop();
    }
    Err("nvme: admin command timeout")
}

/// Submit a command on I/O queue 1 and spin until it completes.
unsafe fn io_submit_and_wait(cmd: &NvmeSqe) -> Result<NvmeCqe, &'static str> {
    let idx = STATE.io_sq_tail as usize;
    ptr::write_volatile(STATE.io_sq.add(idx), *cmd);
    STATE.io_sq_tail = ((idx + 1) % IO_QUEUE_DEPTH) as u16;
    ring_sq_doorbell(1, STATE.io_sq_tail);

    for _ in 0..10_000_000u32 {
        let cqe = ptr::read_volatile(STATE.io_cq.add(STATE.io_cq_head as usize));
        if cqe.phase() == STATE.io_cq_phase {
            STATE.io_cq_head += 1;
            if STATE.io_cq_head as usize >= IO_QUEUE_DEPTH {
                STATE.io_cq_head = 0;
                STATE.io_cq_phase = !STATE.io_cq_phase;
            }
            ring_cq_doorbell(1, STATE.io_cq_head);
            if cqe.status_code() != 0 {
                return Err("nvme: I/O command failed");
            }
            return Ok(cqe);
        }
        core::hint::spin_loop();
    }
    Err("nvme: I/O command timeout")
}

// ---------------------------------------------------------------------------
// Helpers for physical address of allocated frames
// ---------------------------------------------------------------------------

/// Allocate a zeroed 4 KiB frame and return (virtual pointer, physical address).
fn alloc_zeroed_frame() -> Option<(*mut u8, u64)> {
    let frame = memory::alloc_frame()?;
    let phys = frame.start_address().as_u64();
    let virt = memory::phys_to_virt(frame.start_address());
    unsafe { ptr::write_bytes(virt.as_mut_ptr::<u8>(), 0, PAGE_SIZE); }
    Some((virt.as_mut_ptr(), phys))
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Scan PCI for an NVMe controller (class 01:08:02), map BAR0, reset the
/// controller, create admin and I/O queue pairs, and identify the namespace.
pub fn init() {
    let devices = pci::scan();
    let dev = match devices.iter().find(|d| {
        d.class == NVME_CLASS && d.subclass == NVME_SUBCLASS && d.prog_if == NVME_PROG_IF
    }) {
        Some(d) => d.clone(),
        None => { serial_println!("[nvme] no NVMe controller found"); return; }
    };
    serial_println!("[nvme] found {} ({})", dev.summary(), dev.vendor_name());

    // Enable bus-master and memory-space access in PCI Command register
    let cmd_reg = pci::pci_read32(dev.bus, dev.device, dev.function, 0x04);
    pci::pci_write32(dev.bus, dev.device, dev.function, 0x04, cmd_reg | 0x06);

    // Read BAR0 (64-bit MMIO)
    let bar0_lo = pci::pci_read32(dev.bus, dev.device, dev.function, 0x10);
    let bar0_hi = pci::pci_read32(dev.bus, dev.device, dev.function, 0x14);
    if bar0_lo & 0x1 != 0 {
        serial_println!("[nvme] BAR0 is I/O space, expected MMIO"); return;
    }
    let bar0_phys = ((bar0_hi as u64) << 32) | ((bar0_lo & 0xFFFF_FFF0) as u64);
    if bar0_phys == 0 {
        serial_println!("[nvme] BAR0 is zero"); return;
    }
    serial_println!("[nvme] BAR0 physical: {:#x}", bar0_phys);

    let regs = memory::phys_to_virt(x86_64::PhysAddr::new(bar0_phys)).as_mut_ptr() as *mut NvmeRegs;

    unsafe {
        // Read CAP to determine doorbell stride and timeout
        let cap = ptr::read_volatile(&(*regs).cap);
        let dstrd = 4usize << ((cap >> 32) & 0xF); // CAP.DSTRD
        let timeout_ms = ((cap >> 24) & 0xFF) as u32 * 500; // CAP.TO in 500 ms units
        let _mqes = (cap & 0xFFFF) as u16; // Maximum Queue Entries Supported (0-based)
        serial_println!("[nvme] CAP: dstrd={}, timeout={}ms, MQES={}", dstrd, timeout_ms, _mqes + 1);

        let vs = ptr::read_volatile(&(*regs).vs);
        serial_println!("[nvme] version {}.{}.{}",
            (vs >> 16) & 0xFF, (vs >> 8) & 0xFF, vs & 0xFF);

        STATE.regs = regs;
        STATE.dstrd = dstrd;

        // --- Step 1: Disable the controller (CC.EN = 0) ---
        let mut cc = ptr::read_volatile(&(*regs).cc);
        cc &= !CC_EN;
        ptr::write_volatile(&mut (*regs).cc, cc);

        // Wait for CSTS.RDY = 0
        for _ in 0..10_000_000u32 {
            if ptr::read_volatile(&(*regs).csts) & CSTS_RDY == 0 { break; }
            core::hint::spin_loop();
        }
        serial_println!("[nvme] controller disabled");

        // --- Step 2: Allocate admin queue pair ---
        let (asq_virt, asq_phys) = match alloc_zeroed_frame() {
            Some(v) => v, None => { serial_println!("[nvme] failed to alloc admin SQ"); return; }
        };
        let (acq_virt, acq_phys) = match alloc_zeroed_frame() {
            Some(v) => v, None => { serial_println!("[nvme] failed to alloc admin CQ"); return; }
        };

        STATE.admin_sq = asq_virt as *mut NvmeSqe;
        STATE.admin_cq = acq_virt as *mut NvmeCqe;
        STATE.admin_sq_tail = 0;
        STATE.admin_cq_head = 0;
        STATE.admin_cq_phase = true;

        // Set AQA: admin queue sizes (0-based)
        let aqa = ((ADMIN_QUEUE_DEPTH as u32 - 1) << 16) | (ADMIN_QUEUE_DEPTH as u32 - 1);
        ptr::write_volatile(&mut (*regs).aqa, aqa);
        ptr::write_volatile(&mut (*regs).asq, asq_phys);
        ptr::write_volatile(&mut (*regs).acq, acq_phys);

        // --- Step 3: Enable the controller ---
        // CC: I/O SQ entry size = 6 (64B), I/O CQ entry size = 4 (16B),
        //     MPS = 0 (4K pages), CSS = 0 (NVM command set), EN = 1
        let cc_val: u32 = (6 << 16) | (4 << 20) | (0 << 7) | CC_EN;
        ptr::write_volatile(&mut (*regs).cc, cc_val);

        // Wait for CSTS.RDY = 1
        for _ in 0..10_000_000u32 {
            if ptr::read_volatile(&(*regs).csts) & CSTS_RDY != 0 { break; }
            core::hint::spin_loop();
        }
        if ptr::read_volatile(&(*regs).csts) & CSTS_RDY == 0 {
            serial_println!("[nvme] controller failed to become ready"); return;
        }
        serial_println!("[nvme] controller enabled and ready");

        // --- Step 4: Identify Controller (CNS = 1) ---
        let (id_virt, id_phys) = match alloc_zeroed_frame() {
            Some(v) => v, None => { serial_println!("[nvme] failed to alloc identify buf"); return; }
        };

        let mut cmd = NvmeSqe::zeroed();
        let cid = alloc_cid();
        cmd.cdw0 = (ADMIN_OPC_IDENTIFY as u32) | ((cid as u32) << 16);
        cmd.prp1 = id_phys;
        cmd.cdw10 = 1; // CNS = 1 (Identify Controller)
        if admin_submit_and_wait(&cmd).is_err() {
            serial_println!("[nvme] Identify Controller failed"); return;
        }

        // Extract serial (bytes 4-23) and model (bytes 24-63)
        ptr::copy_nonoverlapping(id_virt.add(4), STATE.serial.as_mut_ptr(), 20);
        ptr::copy_nonoverlapping(id_virt.add(24), STATE.model.as_mut_ptr(), 40);
        serial_println!("[nvme] model: {}", core::str::from_utf8(&STATE.model).unwrap_or("?").trim());

        // --- Step 5: Identify Namespace 1 (CNS = 0) ---
        ptr::write_bytes(id_virt, 0, PAGE_SIZE);
        let mut cmd = NvmeSqe::zeroed();
        let cid = alloc_cid();
        cmd.cdw0 = (ADMIN_OPC_IDENTIFY as u32) | ((cid as u32) << 16);
        cmd.nsid = 1;
        cmd.prp1 = id_phys;
        cmd.cdw10 = 0; // CNS = 0 (Identify Namespace)
        if admin_submit_and_wait(&cmd).is_err() {
            serial_println!("[nvme] Identify Namespace failed"); return;
        }

        // NSZE at offset 0 (8 bytes, LE) — namespace size in logical blocks
        let nsze = ptr::read_unaligned(id_virt as *const u64);
        STATE.ns_blocks = nsze;
        serial_println!("[nvme] namespace 1: {} sectors ({} MiB)",
            nsze, nsze * SECTOR_SIZE as u64 / (1024 * 1024));

        // --- Step 6: Create I/O Completion Queue (QID 1) ---
        let (iocq_virt, iocq_phys) = match alloc_zeroed_frame() {
            Some(v) => v, None => { serial_println!("[nvme] failed to alloc I/O CQ"); return; }
        };
        STATE.io_cq = iocq_virt as *mut NvmeCqe;
        STATE.io_cq_head = 0;
        STATE.io_cq_phase = true;

        let mut cmd = NvmeSqe::zeroed();
        let cid = alloc_cid();
        cmd.cdw0 = (ADMIN_OPC_CREATE_IO_CQ as u32) | ((cid as u32) << 16);
        cmd.prp1 = iocq_phys;
        // CDW10: queue size (0-based) in upper 16 bits, QID in lower 16 bits
        cmd.cdw10 = (((IO_QUEUE_DEPTH as u32 - 1) << 16) | 1);
        // CDW11: physically contiguous (bit 0), interrupts disabled
        cmd.cdw11 = 1;
        if admin_submit_and_wait(&cmd).is_err() {
            serial_println!("[nvme] Create I/O CQ failed"); return;
        }

        // --- Step 7: Create I/O Submission Queue (QID 1, linked to CQ 1) ---
        let (iosq_virt, iosq_phys) = match alloc_zeroed_frame() {
            Some(v) => v, None => { serial_println!("[nvme] failed to alloc I/O SQ"); return; }
        };
        STATE.io_sq = iosq_virt as *mut NvmeSqe;
        STATE.io_sq_tail = 0;

        let mut cmd = NvmeSqe::zeroed();
        let cid = alloc_cid();
        cmd.cdw0 = (ADMIN_OPC_CREATE_IO_SQ as u32) | ((cid as u32) << 16);
        cmd.prp1 = iosq_phys;
        cmd.cdw10 = (((IO_QUEUE_DEPTH as u32 - 1) << 16) | 1);
        // CDW11: physically contiguous (bit 0), CQ identifier = 1 (bits 31:16)
        cmd.cdw11 = (1 << 16) | 1;
        if admin_submit_and_wait(&cmd).is_err() {
            serial_println!("[nvme] Create I/O SQ failed"); return;
        }

        serial_println!("[nvme] I/O queues created (depth {})", IO_QUEUE_DEPTH);

        INITIALIZED.store(true, Ordering::SeqCst);
        klog_println!("[nvme] initialized, {} MiB", nsze * SECTOR_SIZE as u64 / (1024 * 1024));
        crate::blkdev::register("nvme0n1", nsze);
        crate::driver::register("nvme", crate::driver::DriverKind::Block);
    }
}

// ---------------------------------------------------------------------------
// Sector I/O
// ---------------------------------------------------------------------------

/// Read a single 512-byte sector at the given LBA from namespace 1.
///
/// The caller must provide a properly aligned 512-byte buffer.
pub fn read_sector(lba: u64, buf: &mut [u8; 512]) -> Result<(), &'static str> {
    if !INITIALIZED.load(Ordering::SeqCst) { return Err("nvme: not initialized"); }

    // Allocate a DMA-safe bounce buffer (physical page)
    let (bounce_virt, bounce_phys) = alloc_zeroed_frame().ok_or("nvme: alloc failed")?;

    unsafe {
        let cid = alloc_cid();
        let mut cmd = NvmeSqe::zeroed();
        cmd.cdw0 = (IO_OPC_READ as u32) | ((cid as u32) << 16);
        cmd.nsid = 1;
        cmd.prp1 = bounce_phys;
        // CDW10/11: Starting LBA (64-bit)
        cmd.cdw10 = lba as u32;
        cmd.cdw11 = (lba >> 32) as u32;
        // CDW12: Number of Logical Blocks (0-based) = 0 means 1 sector
        cmd.cdw12 = 0;

        io_submit_and_wait(&cmd)?;
        ptr::copy_nonoverlapping(bounce_virt, buf.as_mut_ptr(), SECTOR_SIZE);
    }
    Ok(())
}

/// Write a single 512-byte sector at the given LBA to namespace 1.
///
/// The caller must provide exactly 512 bytes of data.
pub fn write_sector(lba: u64, buf: &[u8; 512]) -> Result<(), &'static str> {
    if !INITIALIZED.load(Ordering::SeqCst) { return Err("nvme: not initialized"); }

    let (bounce_virt, bounce_phys) = alloc_zeroed_frame().ok_or("nvme: alloc failed")?;

    unsafe {
        ptr::copy_nonoverlapping(buf.as_ptr(), bounce_virt, SECTOR_SIZE);

        let cid = alloc_cid();
        let mut cmd = NvmeSqe::zeroed();
        cmd.cdw0 = (IO_OPC_WRITE as u32) | ((cid as u32) << 16);
        cmd.nsid = 1;
        cmd.prp1 = bounce_phys;
        cmd.cdw10 = lba as u32;
        cmd.cdw11 = (lba >> 32) as u32;
        cmd.cdw12 = 0; // 1 sector (0-based count)

        io_submit_and_wait(&cmd)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Status / info
// ---------------------------------------------------------------------------

/// Return whether the NVMe driver is active and a device was found.
pub fn is_detected() -> bool {
    INITIALIZED.load(Ordering::SeqCst)
}

/// Human-readable NVMe subsystem status string.
pub fn info() -> String {
    if !is_detected() {
        return String::from("nvme: not detected");
    }
    unsafe {
        let vs = ptr::read_volatile(&(*STATE.regs).vs);
        let model = core::str::from_utf8(&STATE.model).unwrap_or("?").trim();
        alloc::format!(
            "nvme: v{}.{}.{}, model=\"{}\", ns1={} sectors ({} MiB)",
            (vs >> 16) & 0xFF, (vs >> 8) & 0xFF, vs & 0xFF,
            model, STATE.ns_blocks,
            STATE.ns_blocks * SECTOR_SIZE as u64 / (1024 * 1024),
        )
    }
}
