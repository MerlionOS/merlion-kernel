/// USB Mass Storage Class (MSC) driver for MerlionOS.
/// Supports USB flash drives and external hard drives via
/// Bulk-Only Transport (BBB) protocol.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use crate::{blkdev, driver, serial_println, xhci};

// ---------------------------------------------------------------------------
// USB class constants
// ---------------------------------------------------------------------------

/// USB class code for Mass Storage.
const USB_CLASS_MASS_STORAGE: u8 = 0x08;
/// SCSI transparent command set subclass.
const USB_SUBCLASS_SCSI: u8 = 0x06;
/// Bulk-Only (BBB) protocol.
const USB_PROTO_BBB: u8 = 0x50;

// ---------------------------------------------------------------------------
// CBW / CSW constants (Bulk-Only Transport)
// ---------------------------------------------------------------------------

/// CBW signature: "USBC" = 0x43425355
const CBW_SIGNATURE: u32 = 0x4342_5355;
/// CSW signature: "USBS" = 0x53425355
const CSW_SIGNATURE: u32 = 0x5342_5355;
/// CBW size in bytes.
const CBW_SIZE: usize = 31;
/// CSW size in bytes.
const CSW_SIZE: usize = 13;

/// CBW direction: host-to-device (OUT).
const CBW_DIR_OUT: u8 = 0x00;
/// CBW direction: device-to-host (IN).
const CBW_DIR_IN: u8 = 0x80;

/// CSW status: command passed.
const CSW_STATUS_GOOD: u8 = 0x00;
/// CSW status: command failed.
const CSW_STATUS_FAILED: u8 = 0x01;
/// CSW status: phase error.
const CSW_STATUS_PHASE_ERROR: u8 = 0x02;

// ---------------------------------------------------------------------------
// SCSI command opcodes
// ---------------------------------------------------------------------------

const SCSI_TEST_UNIT_READY: u8 = 0x00;
const SCSI_REQUEST_SENSE: u8 = 0x03;
const SCSI_INQUIRY: u8 = 0x12;
const SCSI_MODE_SENSE_6: u8 = 0x1A;
const SCSI_READ_CAPACITY_10: u8 = 0x25;
const SCSI_READ_10: u8 = 0x28;
const SCSI_WRITE_10: u8 = 0x2A;

/// Standard sector size for USB mass storage.
const DEFAULT_BLOCK_SIZE: u32 = 512;
/// Maximum number of USB mass storage devices we track.
const MAX_DEVICES: usize = 8;

// ---------------------------------------------------------------------------
// Command Block Wrapper
// ---------------------------------------------------------------------------

/// USB Mass Storage Command Block Wrapper (31 bytes).
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Cbw {
    signature: u32,
    tag: u32,
    data_transfer_length: u32,
    flags: u8,
    lun: u8,
    cb_length: u8,
    cb: [u8; 16],
}

impl Cbw {
    fn new(tag: u32, transfer_len: u32, direction: u8, lun: u8, cmd: &[u8]) -> Self {
        let mut cb = [0u8; 16];
        let len = if cmd.len() > 16 { 16 } else { cmd.len() };
        cb[..len].copy_from_slice(&cmd[..len]);
        Self {
            signature: CBW_SIGNATURE,
            tag,
            data_transfer_length: transfer_len,
            flags: direction,
            lun,
            cb_length: len as u8,
            cb,
        }
    }

    fn to_bytes(&self) -> [u8; CBW_SIZE] {
        let mut buf = [0u8; CBW_SIZE];
        let sig = self.signature.to_le_bytes();
        buf[0..4].copy_from_slice(&sig);
        buf[4..8].copy_from_slice(&self.tag.to_le_bytes());
        buf[8..12].copy_from_slice(&self.data_transfer_length.to_le_bytes());
        buf[12] = self.flags;
        buf[13] = self.lun;
        buf[14] = self.cb_length;
        buf[15..31].copy_from_slice(&self.cb);
        buf
    }
}

// ---------------------------------------------------------------------------
// Command Status Wrapper
// ---------------------------------------------------------------------------

/// USB Mass Storage Command Status Wrapper (13 bytes).
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Csw {
    signature: u32,
    tag: u32,
    data_residue: u32,
    status: u8,
}

impl Csw {
    fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < CSW_SIZE {
            return None;
        }
        let sig = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if sig != CSW_SIGNATURE {
            return None;
        }
        Some(Self {
            signature: sig,
            tag: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            data_residue: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            status: buf[12],
        })
    }
}

// ---------------------------------------------------------------------------
// SCSI Inquiry response
// ---------------------------------------------------------------------------

/// Parsed SCSI INQUIRY response.
struct InquiryData {
    vendor: [u8; 8],
    product: [u8; 16],
    revision: [u8; 4],
}

impl InquiryData {
    fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 36 {
            return None;
        }
        let mut vendor = [0u8; 8];
        let mut product = [0u8; 16];
        let mut revision = [0u8; 4];
        vendor.copy_from_slice(&buf[8..16]);
        product.copy_from_slice(&buf[16..32]);
        revision.copy_from_slice(&buf[32..36]);
        Some(Self { vendor, product, revision })
    }

    fn vendor_str(&self) -> &str {
        core::str::from_utf8(&self.vendor).unwrap_or("?").trim()
    }

    fn product_str(&self) -> &str {
        core::str::from_utf8(&self.product).unwrap_or("?").trim()
    }

    fn revision_str(&self) -> &str {
        core::str::from_utf8(&self.revision).unwrap_or("?").trim()
    }
}

// ---------------------------------------------------------------------------
// MBR / GPT partition parsing
// ---------------------------------------------------------------------------

/// MBR partition entry (16 bytes).
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct MbrPartitionEntry {
    status: u8,
    chs_first: [u8; 3],
    part_type: u8,
    chs_last: [u8; 3],
    lba_start: u32,
    sector_count: u32,
}

/// Partition info.
#[derive(Clone)]
pub struct PartitionInfo {
    pub index: u8,
    pub part_type: u8,
    pub lba_start: u64,
    pub sector_count: u64,
    pub size_mb: u64,
}

/// GPT header signature.
const GPT_SIGNATURE: u64 = 0x5452_4150_2049_4645; // "EFI PART"

/// Parse MBR partition table from the first 512-byte sector.
fn parse_mbr(sector: &[u8]) -> Vec<PartitionInfo> {
    let mut parts = Vec::new();
    if sector.len() < 512 {
        return parts;
    }
    // Check MBR boot signature
    if sector[510] != 0x55 || sector[511] != 0xAA {
        return parts;
    }
    for i in 0..4u8 {
        let offset = 446 + (i as usize) * 16;
        let ptype = sector[offset + 4];
        if ptype == 0 {
            continue;
        }
        let lba_start = u32::from_le_bytes([
            sector[offset + 8],
            sector[offset + 9],
            sector[offset + 10],
            sector[offset + 11],
        ]) as u64;
        let sector_count = u32::from_le_bytes([
            sector[offset + 12],
            sector[offset + 13],
            sector[offset + 14],
            sector[offset + 15],
        ]) as u64;
        let size_mb = (sector_count * 512) / (1024 * 1024);
        parts.push(PartitionInfo {
            index: i,
            part_type: ptype,
            lba_start,
            sector_count,
            size_mb,
        });
    }
    parts
}

/// Check if sector 1 contains a GPT header and parse basic partition entries.
fn parse_gpt_header(sector: &[u8]) -> Option<(u32, u64)> {
    if sector.len() < 92 {
        return None;
    }
    let sig = u64::from_le_bytes([
        sector[0], sector[1], sector[2], sector[3],
        sector[4], sector[5], sector[6], sector[7],
    ]);
    if sig != GPT_SIGNATURE {
        return None;
    }
    let num_entries = u32::from_le_bytes([
        sector[80], sector[81], sector[82], sector[83],
    ]);
    let entry_lba = u64::from_le_bytes([
        sector[72], sector[73], sector[74], sector[75],
        sector[76], sector[77], sector[78], sector[79],
    ]);
    Some((num_entries, entry_lba))
}

// ---------------------------------------------------------------------------
// USB Mass Storage device descriptor
// ---------------------------------------------------------------------------

/// Describes one detected USB mass storage device.
struct UsbMassDevice {
    /// Slot/port index on the xHCI controller.
    port: u8,
    /// USB endpoint for bulk IN.
    ep_in: u8,
    /// USB endpoint for bulk OUT.
    ep_out: u8,
    /// Logical Unit Number (usually 0).
    lun: u8,
    /// Block size in bytes.
    block_size: u32,
    /// Total number of blocks.
    block_count: u64,
    /// SCSI vendor string.
    vendor: String,
    /// SCSI product string.
    product: String,
    /// SCSI revision string.
    revision: String,
    /// Serial number (from USB descriptor).
    serial: String,
    /// Detected partitions.
    partitions: Vec<PartitionInfo>,
    /// Whether the device has been mounted.
    mounted: bool,
    /// Monotonic tag counter for CBW/CSW matching.
    next_tag: u32,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static DETECTED: AtomicBool = AtomicBool::new(false);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static DEVICES: Mutex<Vec<UsbMassDevice>> = Mutex::new(Vec::new());

// Statistics
static TOTAL_READS: AtomicU64 = AtomicU64::new(0);
static TOTAL_WRITES: AtomicU64 = AtomicU64::new(0);
static BYTES_READ: AtomicU64 = AtomicU64::new(0);
static BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);
static ERRORS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// SCSI command helpers
// ---------------------------------------------------------------------------

/// Build a SCSI INQUIRY command (6 bytes).
fn scsi_inquiry_cmd() -> [u8; 6] {
    [SCSI_INQUIRY, 0, 0, 0, 36, 0]
}

/// Build a SCSI TEST UNIT READY command (6 bytes).
fn scsi_test_unit_ready_cmd() -> [u8; 6] {
    [SCSI_TEST_UNIT_READY, 0, 0, 0, 0, 0]
}

/// Build a SCSI REQUEST SENSE command (6 bytes).
fn scsi_request_sense_cmd() -> [u8; 6] {
    [SCSI_REQUEST_SENSE, 0, 0, 0, 18, 0]
}

/// Build a SCSI READ CAPACITY(10) command (10 bytes).
fn scsi_read_capacity_cmd() -> [u8; 10] {
    let mut cmd = [0u8; 10];
    cmd[0] = SCSI_READ_CAPACITY_10;
    cmd
}

/// Build a SCSI MODE SENSE(6) command (6 bytes).
fn scsi_mode_sense_cmd(page: u8) -> [u8; 6] {
    [SCSI_MODE_SENSE_6, 0, page, 0, 192, 0]
}

/// Build a SCSI READ(10) command.
fn scsi_read10_cmd(lba: u32, block_count: u16) -> [u8; 10] {
    let lba_bytes = lba.to_be_bytes();
    let cnt = block_count.to_be_bytes();
    [
        SCSI_READ_10, 0,
        lba_bytes[0], lba_bytes[1], lba_bytes[2], lba_bytes[3],
        0,
        cnt[0], cnt[1],
        0,
    ]
}

/// Build a SCSI WRITE(10) command.
fn scsi_write10_cmd(lba: u32, block_count: u16) -> [u8; 10] {
    let lba_bytes = lba.to_be_bytes();
    let cnt = block_count.to_be_bytes();
    [
        SCSI_WRITE_10, 0,
        lba_bytes[0], lba_bytes[1], lba_bytes[2], lba_bytes[3],
        0,
        cnt[0], cnt[1],
        0,
    ]
}

// ---------------------------------------------------------------------------
// Bulk transport helpers (stubbed — requires xHCI bulk pipe support)
// ---------------------------------------------------------------------------

/// Send a CBW and receive a CSW via bulk endpoints.
/// In a real driver this calls into the xHCI bulk transfer API.
fn bulk_command(
    _port: u8,
    _ep_out: u8,
    _ep_in: u8,
    cbw: &Cbw,
    data_buf: Option<&mut [u8]>,
    _direction: u8,
) -> Result<Csw, &'static str> {
    let _cbw_bytes = cbw.to_bytes();

    // --- stub: no real USB transport yet ---
    // In production this would:
    //   1. Send CBW on bulk OUT endpoint
    //   2. Transfer data on bulk IN or OUT
    //   3. Receive CSW on bulk IN endpoint

    if let Some(buf) = data_buf {
        // Zero-fill read buffer to indicate no data
        for b in buf.iter_mut() {
            *b = 0;
        }
    }

    Ok(Csw {
        signature: CSW_SIGNATURE,
        tag: cbw.tag,
        data_residue: 0,
        status: CSW_STATUS_GOOD,
    })
}

// ---------------------------------------------------------------------------
// Device operations
// ---------------------------------------------------------------------------

/// Issue SCSI INQUIRY to a USB mass storage device.
fn do_inquiry(dev: &mut UsbMassDevice) -> Result<InquiryData, &'static str> {
    let cmd = scsi_inquiry_cmd();
    let cbw = Cbw::new(dev.next_tag, 36, CBW_DIR_IN, dev.lun, &cmd);
    dev.next_tag += 1;

    let mut buf = [0u8; 36];
    let csw = bulk_command(dev.port, dev.ep_out, dev.ep_in, &cbw, Some(&mut buf), CBW_DIR_IN)?;
    if csw.status != CSW_STATUS_GOOD {
        return Err("INQUIRY failed");
    }
    InquiryData::from_bytes(&buf).ok_or("bad INQUIRY response")
}

/// Issue SCSI READ CAPACITY(10) to determine device geometry.
fn do_read_capacity(dev: &mut UsbMassDevice) -> Result<(u64, u32), &'static str> {
    let cmd = scsi_read_capacity_cmd();
    let cbw = Cbw::new(dev.next_tag, 8, CBW_DIR_IN, dev.lun, &cmd);
    dev.next_tag += 1;

    let mut buf = [0u8; 8];
    let csw = bulk_command(dev.port, dev.ep_out, dev.ep_in, &cbw, Some(&mut buf), CBW_DIR_IN)?;
    if csw.status != CSW_STATUS_GOOD {
        return Err("READ CAPACITY failed");
    }
    let last_lba = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as u64;
    let blk_size = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    Ok((last_lba + 1, blk_size))
}

/// Issue SCSI TEST UNIT READY.
fn do_test_unit_ready(dev: &mut UsbMassDevice) -> bool {
    let cmd = scsi_test_unit_ready_cmd();
    let cbw = Cbw::new(dev.next_tag, 0, CBW_DIR_OUT, dev.lun, &cmd);
    dev.next_tag += 1;
    match bulk_command(dev.port, dev.ep_out, dev.ep_in, &cbw, None, CBW_DIR_OUT) {
        Ok(csw) => csw.status == CSW_STATUS_GOOD,
        Err(_) => false,
    }
}

/// Issue SCSI REQUEST SENSE after an error.
fn do_request_sense(dev: &mut UsbMassDevice) -> Result<(u8, u8, u8), &'static str> {
    let cmd = scsi_request_sense_cmd();
    let cbw = Cbw::new(dev.next_tag, 18, CBW_DIR_IN, dev.lun, &cmd);
    dev.next_tag += 1;

    let mut buf = [0u8; 18];
    let csw = bulk_command(dev.port, dev.ep_out, dev.ep_in, &cbw, Some(&mut buf), CBW_DIR_IN)?;
    if csw.status != CSW_STATUS_GOOD {
        return Err("REQUEST SENSE failed");
    }
    let sense_key = buf[2] & 0x0F;
    let asc = buf[12];
    let ascq = buf[13];
    Ok((sense_key, asc, ascq))
}

/// Read blocks from a USB mass storage device.
pub fn read_blocks(device_idx: usize, lba: u32, count: u16, buf: &mut [u8]) -> Result<(), &'static str> {
    let mut devs = DEVICES.lock();
    let dev = devs.get_mut(device_idx).ok_or("invalid device index")?;
    let transfer_len = (count as u32) * dev.block_size;
    if buf.len() < transfer_len as usize {
        return Err("buffer too small");
    }

    let cmd = scsi_read10_cmd(lba, count);
    let cbw = Cbw::new(dev.next_tag, transfer_len, CBW_DIR_IN, dev.lun, &cmd);
    dev.next_tag += 1;

    let csw = bulk_command(
        dev.port, dev.ep_out, dev.ep_in, &cbw,
        Some(&mut buf[..transfer_len as usize]), CBW_DIR_IN,
    )?;
    if csw.status != CSW_STATUS_GOOD {
        ERRORS.fetch_add(1, Ordering::Relaxed);
        return Err("READ(10) failed");
    }
    TOTAL_READS.fetch_add(count as u64, Ordering::Relaxed);
    BYTES_READ.fetch_add(transfer_len as u64, Ordering::Relaxed);
    Ok(())
}

/// Write blocks to a USB mass storage device.
pub fn write_blocks(device_idx: usize, lba: u32, count: u16, data: &[u8]) -> Result<(), &'static str> {
    let mut devs = DEVICES.lock();
    let dev = devs.get_mut(device_idx).ok_or("invalid device index")?;
    let transfer_len = (count as u32) * dev.block_size;
    if data.len() < transfer_len as usize {
        return Err("data buffer too small");
    }

    let cmd = scsi_write10_cmd(lba, count);
    let cbw = Cbw::new(dev.next_tag, transfer_len, CBW_DIR_OUT, dev.lun, &cmd);
    dev.next_tag += 1;

    // For write, we send data after CBW — here we just issue the command
    let csw = bulk_command(dev.port, dev.ep_out, dev.ep_in, &cbw, None, CBW_DIR_OUT)?;
    if csw.status != CSW_STATUS_GOOD {
        ERRORS.fetch_add(1, Ordering::Relaxed);
        return Err("WRITE(10) failed");
    }
    TOTAL_WRITES.fetch_add(count as u64, Ordering::Relaxed);
    BYTES_WRITTEN.fetch_add(transfer_len as u64, Ordering::Relaxed);
    Ok(())
}

// ---------------------------------------------------------------------------
// Partition parsing
// ---------------------------------------------------------------------------

/// Read the first sector (MBR) and optionally sector 1 (GPT) to find partitions.
fn parse_partitions(device_idx: usize) -> Vec<PartitionInfo> {
    let mut sector = [0u8; 512];
    if read_blocks(device_idx, 0, 1, &mut sector).is_err() {
        return Vec::new();
    }

    // Try MBR first
    let mbr_parts = parse_mbr(&sector);

    // Check for protective MBR (type 0xEE) indicating GPT
    let has_gpt_protective = mbr_parts.iter().any(|p| p.part_type == 0xEE);

    if has_gpt_protective {
        // Read LBA 1 for GPT header
        let mut gpt_sector = [0u8; 512];
        if read_blocks(device_idx, 1, 1, &mut gpt_sector).is_ok() {
            if let Some((_num_entries, _entry_lba)) = parse_gpt_header(&gpt_sector) {
                serial_println!("[usb_mass] GPT detected with {} partition entries", _num_entries);
                // For now, return MBR partitions as fallback
                // Full GPT entry parsing would read sectors starting at entry_lba
            }
        }
    }

    mbr_parts
}

// ---------------------------------------------------------------------------
// Auto-mount stub
// ---------------------------------------------------------------------------

/// Partition type ID for FAT32 (LBA).
const PART_TYPE_FAT32_LBA: u8 = 0x0C;
/// Partition type ID for FAT32 (CHS).
const PART_TYPE_FAT32_CHS: u8 = 0x0B;
/// Partition type ID for Linux (ext2/3/4).
const PART_TYPE_LINUX: u8 = 0x83;

fn partition_type_name(ptype: u8) -> &'static str {
    match ptype {
        0x01 => "FAT12",
        0x04 | 0x06 | 0x0E => "FAT16",
        PART_TYPE_FAT32_CHS | PART_TYPE_FAT32_LBA => "FAT32",
        PART_TYPE_LINUX => "Linux",
        0x82 => "Linux swap",
        0x07 => "NTFS/exFAT",
        0xEE => "GPT protective",
        0xEF => "EFI System",
        _ => "Unknown",
    }
}

/// Attempt to auto-mount detected partitions.
fn auto_mount(device_idx: usize, partitions: &[PartitionInfo]) {
    for part in partitions {
        let type_name = partition_type_name(part.part_type);
        serial_println!(
            "[usb_mass] dev{} partition {}: type 0x{:02X} ({}) LBA {} size {} MiB",
            device_idx, part.index, part.part_type, type_name,
            part.lba_start, part.size_mb,
        );
        match part.part_type {
            PART_TYPE_FAT32_CHS | PART_TYPE_FAT32_LBA => {
                serial_println!("[usb_mass]   -> would mount as FAT32 on /mnt/usb{}", device_idx);
            }
            PART_TYPE_LINUX => {
                serial_println!("[usb_mass]   -> would mount as ext4 on /mnt/usb{}", device_idx);
            }
            _ => {
                serial_println!("[usb_mass]   -> unsupported filesystem type");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Device enumeration
// ---------------------------------------------------------------------------

/// Probe a USB port for mass storage class device.
fn probe_device(_port: u8) -> Option<UsbMassDevice> {
    // In production, we would query the xHCI device descriptor for:
    //   bInterfaceClass    == 0x08 (Mass Storage)
    //   bInterfaceSubClass == 0x06 (SCSI)
    //   bInterfaceProtocol == 0x50 (BBB)
    // For now, we check if the xHCI controller reports a device on this port.
    let _ = (USB_CLASS_MASS_STORAGE, USB_SUBCLASS_SCSI, USB_PROTO_BBB);

    if !xhci::is_detected() {
        return None;
    }

    // Stub: no real enumeration yet — would query xHCI slot descriptors
    None
}

/// Scan all USB ports for mass storage devices.
pub fn scan_devices() -> usize {
    if !xhci::is_detected() {
        return 0;
    }

    let mut count = 0usize;
    for port in 0..16u8 {
        if let Some(mut dev) = probe_device(port) {
            // Issue INQUIRY
            if let Ok(inq) = do_inquiry(&mut dev) {
                dev.vendor = String::from(inq.vendor_str());
                dev.product = String::from(inq.product_str());
                dev.revision = String::from(inq.revision_str());
            }
            // Test Unit Ready
            let _ = do_test_unit_ready(&mut dev);
            // Read Capacity
            if let Ok((blocks, blk_sz)) = do_read_capacity(&mut dev) {
                dev.block_count = blocks;
                dev.block_size = blk_sz;
            }

            let idx = {
                let mut devs = DEVICES.lock();
                let idx = devs.len();
                devs.push(dev);
                idx
            };

            // Parse partitions
            let partitions = parse_partitions(idx);
            auto_mount(idx, &partitions);

            {
                let mut devs = DEVICES.lock();
                if let Some(d) = devs.get_mut(idx) {
                    d.partitions = partitions;
                    // Register with blkdev subsystem
                    let name = format!("usb{}", idx);
                    blkdev::register(&name, d.block_count);
                    serial_println!(
                        "[usb_mass] {} {} {} — {} blocks ({} MiB)",
                        d.vendor, d.product, d.revision,
                        d.block_count,
                        (d.block_count * d.block_size as u64) / (1024 * 1024),
                    );
                }
            }

            count += 1;
        }
    }
    count
}

/// Safely eject a USB mass storage device.
pub fn eject(device_idx: usize) -> Result<(), &'static str> {
    let mut devs = DEVICES.lock();
    let dev = devs.get_mut(device_idx).ok_or("invalid device index")?;

    // In production: flush caches, unmount filesystems, send SCSI
    // START STOP UNIT with LoEj=1, then release the USB interface.
    dev.mounted = false;
    serial_println!("[usb_mass] ejected device {}: {} {}", device_idx, dev.vendor, dev.product);
    Ok(())
}

// ---------------------------------------------------------------------------
// Device listing
// ---------------------------------------------------------------------------

/// Info about a single USB mass storage device (for display).
pub struct UsbMassDevInfo {
    pub index: usize,
    pub vendor: String,
    pub product: String,
    pub serial: String,
    pub block_size: u32,
    pub block_count: u64,
    pub capacity_mb: u64,
    pub partitions: usize,
    pub mounted: bool,
}

/// List all detected USB mass storage devices.
pub fn list_devices() -> Vec<UsbMassDevInfo> {
    let devs = DEVICES.lock();
    devs.iter().enumerate().map(|(i, d)| UsbMassDevInfo {
        index: i,
        vendor: d.vendor.clone(),
        product: d.product.clone(),
        serial: d.serial.clone(),
        block_size: d.block_size,
        block_count: d.block_count,
        capacity_mb: (d.block_count * d.block_size as u64) / (1024 * 1024),
        partitions: d.partitions.len(),
        mounted: d.mounted,
    }).collect()
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn is_detected() -> bool {
    DETECTED.load(Ordering::SeqCst)
}

/// Return a human-readable info string.
pub fn usb_mass_info() -> String {
    let devs = DEVICES.lock();
    if devs.is_empty() {
        return format!("usb_mass: no devices detected");
    }
    let mut s = format!("usb_mass: {} device(s)\n", devs.len());
    for (i, d) in devs.iter().enumerate() {
        s.push_str(&format!(
            "  usb{}: {} {} rev={} blocks={} blk_sz={} cap={}MiB parts={} mounted={}\n",
            i, d.vendor, d.product, d.revision,
            d.block_count, d.block_size,
            (d.block_count * d.block_size as u64) / (1024 * 1024),
            d.partitions.len(), d.mounted,
        ));
        for p in &d.partitions {
            s.push_str(&format!(
                "    p{}: type=0x{:02X}({}) LBA={} sectors={} size={}MiB\n",
                p.index, p.part_type, partition_type_name(p.part_type),
                p.lba_start, p.sector_count, p.size_mb,
            ));
        }
    }
    s
}

/// Return statistics as a human-readable string.
pub fn usb_mass_stats() -> String {
    let reads = TOTAL_READS.load(Ordering::Relaxed);
    let writes = TOTAL_WRITES.load(Ordering::Relaxed);
    let br = BYTES_READ.load(Ordering::Relaxed);
    let bw = BYTES_WRITTEN.load(Ordering::Relaxed);
    let errs = ERRORS.load(Ordering::Relaxed);
    let dev_count = DEVICES.lock().len();

    format!(
        "usb_mass stats:\n  Devices: {}\n  Reads: {} ({} bytes)\n  Writes: {} ({} bytes)\n  Errors: {}",
        dev_count, reads, br, writes, bw, errs,
    )
}

/// Initialize the USB mass storage subsystem.
pub fn init() {
    // Register driver
    driver::register("usb_mass", driver::DriverKind::Block);

    // Scan for devices if xHCI is present
    let found = scan_devices();
    if found > 0 {
        DETECTED.store(true, Ordering::SeqCst);
    }

    INITIALIZED.store(true, Ordering::SeqCst);
    serial_println!("[usb_mass] initialized, {} device(s) found", found);
}
