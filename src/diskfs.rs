/// Persistent disk filesystem — MF16 on virtio-blk.
/// Bridges the FAT module to real disk I/O so files survive reboot.
///
/// Layout on virtio disk:
///   Sectors 0-7:   MF16 filesystem metadata (boot + FAT + directory)
///   Sectors 8+:    File data clusters
///
/// Operations read/write sectors through the virtio-blk driver.

use crate::{fat, virtio_blk, serial_println, klog_println};
use alloc::string::String;
use alloc::vec::Vec;

/// Size of the filesystem area on disk (in sectors).
/// We use the first 256 sectors (128K) of the virtio disk.
const FS_SECTORS: usize = 256;
const SECTOR_SIZE: usize = 512;
const FS_SIZE: usize = FS_SECTORS * SECTOR_SIZE; // 128K

/// Read the entire filesystem image from disk into memory.
fn read_fs_image() -> Result<Vec<u8>, &'static str> {
    if !virtio_blk::is_detected() {
        return Err("no virtio disk");
    }

    let mut image = alloc::vec![0u8; FS_SIZE];
    for i in 0..FS_SECTORS {
        let mut sector = [0u8; 512];
        virtio_blk::read_sector(i as u64, &mut sector)?;
        image[i * SECTOR_SIZE..(i + 1) * SECTOR_SIZE].copy_from_slice(&sector);
    }
    Ok(image)
}

/// Write the filesystem image back to disk.
fn write_fs_image(image: &[u8]) -> Result<(), &'static str> {
    if !virtio_blk::is_detected() {
        return Err("no virtio disk");
    }

    let sectors = image.len() / SECTOR_SIZE;
    for i in 0..sectors.min(FS_SECTORS) {
        let mut sector = [0u8; 512];
        sector.copy_from_slice(&image[i * SECTOR_SIZE..(i + 1) * SECTOR_SIZE]);
        virtio_blk::write_sector(i as u64, &sector)?;
    }
    Ok(())
}

/// Format the virtio disk with MF16 filesystem.
pub fn format() -> Result<(), &'static str> {
    let mut image = alloc::vec![0u8; FS_SIZE];
    fat::format(&mut image)?;
    write_fs_image(&image)?;
    serial_println!("[diskfs] formatted virtio disk as MF16");
    klog_println!("[diskfs] disk formatted");
    Ok(())
}

/// List files on the persistent disk.
pub fn list_files() -> Result<Vec<fat::FileInfo>, &'static str> {
    let image = read_fs_image()?;
    Ok(fat::list_files(&image))
}

/// Read a file from the persistent disk.
pub fn read_file(name: &str) -> Result<Vec<u8>, &'static str> {
    let image = read_fs_image()?;
    fat::read_file(&image, name).ok_or("file not found")
}

/// Write a file to the persistent disk.
pub fn write_file(name: &str, data: &[u8]) -> Result<(), &'static str> {
    let mut image = read_fs_image()?;
    fat::write_file(&mut image, name, data)?;
    write_fs_image(&image)?;
    serial_println!("[diskfs] wrote '{}' ({} bytes) to disk", name, data.len());
    Ok(())
}

/// Delete a file from the persistent disk.
pub fn delete_file(name: &str) -> Result<(), &'static str> {
    let mut image = read_fs_image()?;
    fat::delete_file(&mut image, name)?;
    write_fs_image(&image)?;
    serial_println!("[diskfs] deleted '{}' from disk", name);
    Ok(())
}

/// Check if the disk has an MF16 filesystem.
pub fn is_formatted() -> bool {
    if !virtio_blk::is_detected() { return false; }
    let mut sector = [0u8; 512];
    if virtio_blk::read_sector(0, &mut sector).is_err() { return false; }
    sector[0..4] == *b"MF16"
}

/// Disk filesystem info string.
pub fn info() -> String {
    if !virtio_blk::is_detected() {
        return String::from("diskfs: no virtio disk");
    }
    if is_formatted() {
        match list_files() {
            Ok(files) => {
                let total_size: u32 = files.iter().map(|f| f.size).sum();
                alloc::format!("diskfs: MF16, {} files, {} bytes", files.len(), total_size)
            }
            Err(e) => alloc::format!("diskfs: error reading: {}", e),
        }
    } else {
        String::from("diskfs: not formatted (use 'diskfmt')")
    }
}
