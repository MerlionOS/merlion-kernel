/// Block device abstraction layer.
/// Provides a unified interface for block I/O, backing both the
/// RAM disk and future virtio-blk devices.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::borrow::ToOwned;
use spin::Mutex;

pub const BLOCK_SIZE: usize = 512;

/// Block device trait.
pub trait BlockDevice: Send + Sync {
    fn name(&self) -> &str;
    fn block_size(&self) -> usize { BLOCK_SIZE }
    fn block_count(&self) -> u64;
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str>;
    fn write_block(&mut self, lba: u64, data: &[u8]) -> Result<(), &'static str>;
    fn size_bytes(&self) -> u64 { self.block_count() * self.block_size() as u64 }
}

static DEVICES: Mutex<Vec<BlockDevEntry>> = Mutex::new(Vec::new());

struct BlockDevEntry {
    name: String,
    blocks: u64,
    size_kb: u64,
}

/// Register a block device for tracking.
pub fn register(name: &str, blocks: u64) {
    DEVICES.lock().push(BlockDevEntry {
        name: name.to_owned(),
        blocks,
        size_kb: blocks * BLOCK_SIZE as u64 / 1024,
    });
}

/// Block device info for display.
pub struct BlkDevInfo {
    pub name: String,
    pub blocks: u64,
    pub size_kb: u64,
}

/// List registered block devices.
pub fn list() -> Vec<BlkDevInfo> {
    DEVICES.lock().iter().map(|d| BlkDevInfo {
        name: d.name.clone(),
        blocks: d.blocks,
        size_kb: d.size_kb,
    }).collect()
}

/// Initialize: register the built-in RAM disk as a block device.
pub fn init() {
    // RAM disk: 128K = 256 blocks of 512 bytes
    register("rd0", 256);
    crate::klog_println!("[blkdev] registered rd0 (128K RAM disk)");
}
