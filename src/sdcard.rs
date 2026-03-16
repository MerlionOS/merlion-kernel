/// SD card driver for Raspberry Pi.
/// Implements EMMC/SD card initialization and block-level I/O
/// for the BCM283x EMMC controller.
/// On x86_64, simulates an SD card using an in-memory buffer.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// EMMC controller base address (BCM2837, Raspberry Pi 3).
#[cfg(target_arch = "aarch64")]
const EMMC_BASE: u64 = 0x3F300000;

/// Standard SD card block size in bytes.
const BLOCK_SIZE: u32 = 512;

/// Size of the simulated SD card in bytes (64 KiB — small to save heap).
const SIM_CARD_SIZE: usize = 64 * 1024;

/// Number of blocks in the simulated card.
const SIM_BLOCK_COUNT: u64 = (SIM_CARD_SIZE / BLOCK_SIZE as usize) as u64;

// ---------------------------------------------------------------------------
// SD command constants (documented for future aarch64 implementation)
// ---------------------------------------------------------------------------

/// CMD0: GO_IDLE_STATE — reset card to idle state.
const _CMD0_GO_IDLE: u32 = 0;
/// CMD8: SEND_IF_COND — check voltage and pattern.
const _CMD8_SEND_IF_COND: u32 = 8;
/// ACMD41: SD_SEND_OP_COND — initiate initialisation process.
const _ACMD41_SD_SEND_OP_COND: u32 = 41;
/// CMD2: ALL_SEND_CID — read card identification.
const _CMD2_ALL_SEND_CID: u32 = 2;
/// CMD3: SEND_RELATIVE_ADDR — ask card to publish RCA.
const _CMD3_SEND_RCA: u32 = 3;
/// CMD7: SELECT_CARD — select card by RCA.
const _CMD7_SELECT_CARD: u32 = 7;
/// CMD17: READ_SINGLE_BLOCK — read one block.
const _CMD17_READ_BLOCK: u32 = 17;
/// CMD24: WRITE_BLOCK — write one block.
const _CMD24_WRITE_BLOCK: u32 = 24;

/// Type of SD card detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardType {
    None,
    SD,
    SDHC,
    SDXC,
}

/// Information about the currently inserted SD card.
pub struct SdCardInfo {
    pub present: bool,
    pub card_type: CardType,
    pub capacity_mb: u32,
    pub block_size: u32,
    pub manufacturer: String,
    pub product: String,
    pub serial: u32,
    pub speed_class: u8,
}

impl SdCardInfo {
    fn default_sim() -> Self {
        Self {
            present: true,
            card_type: CardType::SDHC,
            capacity_mb: (SIM_CARD_SIZE / (1024 * 1024)) as u32,
            block_size: BLOCK_SIZE,
            manufacturer: String::from("MerlionSD"),
            product: String::from("SIM-4M"),
            serial: 0xDEAD_BEEF,
            speed_class: 10,
        }
    }
}

/// I/O and error statistics.
struct SdStats {
    blocks_read: AtomicU64,
    blocks_written: AtomicU64,
    read_errors: AtomicU64,
    write_errors: AtomicU64,
    commands_sent: AtomicU64,
}

impl SdStats {
    const fn new() -> Self {
        Self {
            blocks_read: AtomicU64::new(0),
            blocks_written: AtomicU64::new(0),
            read_errors: AtomicU64::new(0),
            write_errors: AtomicU64::new(0),
            commands_sent: AtomicU64::new(0),
        }
    }
}

static STATS: SdStats = SdStats::new();

/// The simulated SD card image (x86_64 only).
struct SimCard {
    data: Vec<u8>,
    info: SdCardInfo,
    initialised: bool,
}

static SIM_CARD: Mutex<Option<SimCard>> = Mutex::new(None);

/// Write a minimal FAT32 boot sector into the first block.
fn write_fat32_header(buf: &mut [u8]) {
    if buf.len() < 512 {
        return;
    }
    // Jump boot code
    buf[0] = 0xEB;
    buf[1] = 0x58;
    buf[2] = 0x90;
    // OEM name
    buf[3..11].copy_from_slice(b"MERLION ");
    // Bytes per sector = 512
    buf[11] = 0x00;
    buf[12] = 0x02;
    // Sectors per cluster = 8
    buf[13] = 0x08;
    // Reserved sectors = 32
    buf[14] = 0x20;
    buf[15] = 0x00;
    // Number of FATs = 2
    buf[16] = 0x02;
    // Media type = 0xF8 (fixed disk)
    buf[21] = 0xF8;
    // Sectors per track = 32
    buf[24] = 0x20;
    buf[25] = 0x00;
    // Number of heads = 64
    buf[26] = 0x40;
    buf[27] = 0x00;
    // Total sectors (32-bit) = SIM_BLOCK_COUNT
    let total = SIM_BLOCK_COUNT as u32;
    buf[32] = (total & 0xFF) as u8;
    buf[33] = ((total >> 8) & 0xFF) as u8;
    buf[34] = ((total >> 16) & 0xFF) as u8;
    buf[35] = ((total >> 24) & 0xFF) as u8;
    // FAT32: sectors per FAT
    buf[36] = 0x20;
    buf[37] = 0x00;
    buf[38] = 0x00;
    buf[39] = 0x00;
    // Root cluster = 2
    buf[44] = 0x02;
    buf[45] = 0x00;
    buf[46] = 0x00;
    buf[47] = 0x00;
    // FSInfo sector = 1
    buf[48] = 0x01;
    buf[49] = 0x00;
    // Backup boot sector = 6
    buf[50] = 0x06;
    buf[51] = 0x00;
    // Volume label in extended boot record
    buf[71..82].copy_from_slice(b"MERLION SD ");
    // FS type
    buf[82..90].copy_from_slice(b"FAT32   ");
    // Boot signature
    buf[510] = 0x55;
    buf[511] = 0xAA;
}

/// Initialise the SD card subsystem.
/// On x86_64, creates a 4 MiB in-memory simulated card.
/// On aarch64, would perform the full EMMC initialisation sequence.
pub fn init() -> Result<SdCardInfo, &'static str> {
    #[cfg(target_arch = "aarch64")]
    {
        // TODO: Real EMMC initialisation sequence:
        //   CMD0 -> CMD8 -> ACMD41 -> CMD2 -> CMD3 -> CMD7
        STATS.commands_sent.fetch_add(6, Ordering::Relaxed);
        return Err("aarch64 EMMC not yet implemented");
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        let mut data = vec![0u8; SIM_CARD_SIZE];
        write_fat32_header(&mut data);
        let info = SdCardInfo::default_sim();
        let result_info = SdCardInfo {
            present: true,
            card_type: CardType::SDHC,
            capacity_mb: info.capacity_mb,
            block_size: info.block_size,
            manufacturer: String::from("MerlionSD"),
            product: String::from("SIM-4M"),
            serial: info.serial,
            speed_class: info.speed_class,
        };
        *SIM_CARD.lock() = Some(SimCard {
            data,
            info,
            initialised: true,
        });
        Ok(result_info)
    }
}

/// Read a single block from the SD card.
pub fn read_block(lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
    if buf.len() < BLOCK_SIZE as usize {
        return Err("buffer too small");
    }

    #[cfg(target_arch = "aarch64")]
    {
        STATS.commands_sent.fetch_add(1, Ordering::Relaxed);
        // TODO: Send CMD17 with LBA address
        return Err("aarch64 EMMC not yet implemented");
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        let lock = SIM_CARD.lock();
        let card = lock.as_ref().ok_or("SD card not initialised")?;
        if !card.initialised {
            STATS.read_errors.fetch_add(1, Ordering::Relaxed);
            return Err("card not initialised");
        }
        if lba >= SIM_BLOCK_COUNT {
            STATS.read_errors.fetch_add(1, Ordering::Relaxed);
            return Err("LBA out of range");
        }
        let offset = (lba as usize) * (BLOCK_SIZE as usize);
        let end = offset + BLOCK_SIZE as usize;
        buf[..BLOCK_SIZE as usize].copy_from_slice(&card.data[offset..end]);
        STATS.blocks_read.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Write a single block to the SD card.
pub fn write_block(lba: u64, data: &[u8]) -> Result<(), &'static str> {
    if data.len() < BLOCK_SIZE as usize {
        return Err("data too small (need 512 bytes)");
    }

    #[cfg(target_arch = "aarch64")]
    {
        STATS.commands_sent.fetch_add(1, Ordering::Relaxed);
        // TODO: Send CMD24 with LBA address
        return Err("aarch64 EMMC not yet implemented");
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        let mut lock = SIM_CARD.lock();
        let card = lock.as_mut().ok_or("SD card not initialised")?;
        if !card.initialised {
            STATS.write_errors.fetch_add(1, Ordering::Relaxed);
            return Err("card not initialised");
        }
        if lba >= SIM_BLOCK_COUNT {
            STATS.write_errors.fetch_add(1, Ordering::Relaxed);
            return Err("LBA out of range");
        }
        let offset = (lba as usize) * (BLOCK_SIZE as usize);
        let end = offset + BLOCK_SIZE as usize;
        card.data[offset..end].copy_from_slice(&data[..BLOCK_SIZE as usize]);
        STATS.blocks_written.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Read multiple contiguous blocks from the SD card.
pub fn read_blocks(lba: u64, count: u32, buf: &mut [u8]) -> Result<(), &'static str> {
    let needed = (count as usize) * (BLOCK_SIZE as usize);
    if buf.len() < needed {
        return Err("buffer too small for requested blocks");
    }
    for i in 0..count {
        let off = (i as usize) * (BLOCK_SIZE as usize);
        read_block(lba + i as u64, &mut buf[off..off + BLOCK_SIZE as usize])?;
    }
    Ok(())
}

/// Return a formatted summary of the SD card.
pub fn sdcard_info() -> String {
    let lock = SIM_CARD.lock();
    match lock.as_ref() {
        None => String::from("SD card not initialised\n"),
        Some(card) => {
            let ct = match card.info.card_type {
                CardType::None => "None",
                CardType::SD => "SD",
                CardType::SDHC => "SDHC",
                CardType::SDXC => "SDXC",
            };
            format!(
                "SD Card Information:\n\
                 \x20 Present:       {}\n\
                 \x20 Type:          {}\n\
                 \x20 Capacity:      {} MiB\n\
                 \x20 Block size:    {} bytes\n\
                 \x20 Manufacturer:  {}\n\
                 \x20 Product:       {}\n\
                 \x20 Serial:        {:#010X}\n\
                 \x20 Speed class:   {}\n\
                 \x20 Total blocks:  {}\n\
                 \x20 Mode:          simulated (x86_64)\n",
                if card.info.present { "yes" } else { "no" },
                ct,
                card.info.capacity_mb,
                card.info.block_size,
                card.info.manufacturer,
                card.info.product,
                card.info.serial,
                card.info.speed_class,
                SIM_BLOCK_COUNT,
            )
        }
    }
}

/// Return I/O statistics for the SD card.
pub fn sdcard_stats() -> String {
    let br = STATS.blocks_read.load(Ordering::Relaxed);
    let bw = STATS.blocks_written.load(Ordering::Relaxed);
    let re = STATS.read_errors.load(Ordering::Relaxed);
    let we = STATS.write_errors.load(Ordering::Relaxed);
    let cs = STATS.commands_sent.load(Ordering::Relaxed);
    let bytes_read = br * BLOCK_SIZE as u64;
    let bytes_written = bw * BLOCK_SIZE as u64;
    format!(
        "SD Card Statistics:\n\
         \x20 Blocks read:    {} ({} bytes)\n\
         \x20 Blocks written: {} ({} bytes)\n\
         \x20 Read errors:    {}\n\
         \x20 Write errors:   {}\n\
         \x20 Commands sent:  {}\n",
        br, bytes_read, bw, bytes_written, re, we, cs,
    )
}
