/// Software RAID (0/1/5) for MerlionOS.
/// Combines multiple block devices into a single logical volume
/// with striping, mirroring, or parity.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// --- Statistics ---
static TOTAL_READS: AtomicU64 = AtomicU64::new(0);
static TOTAL_WRITES: AtomicU64 = AtomicU64::new(0);
static TOTAL_REBUILDS: AtomicU64 = AtomicU64::new(0);
static NEXT_ARRAY_ID: AtomicU32 = AtomicU32::new(1);

/// Default stripe size in sectors (64 KiB with 512-byte sectors).
const DEFAULT_STRIPE_SECTORS: u64 = 128;

/// Maximum devices per array.
const MAX_DEVICES: usize = 16;

/// Maximum arrays.
const MAX_ARRAYS: usize = 8;

// --- RAID Level ---
/// Supported RAID levels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RaidLevel {
    /// Striping: data split across N disks, no redundancy.
    Raid0,
    /// Mirroring: data duplicated on 2 disks.
    Raid1,
    /// Distributed parity: N-1 data + 1 parity, survives 1 failure.
    Raid5,
}

impl RaidLevel {
    pub fn name(&self) -> &'static str {
        match self {
            RaidLevel::Raid0 => "RAID-0 (Striping)",
            RaidLevel::Raid1 => "RAID-1 (Mirroring)",
            RaidLevel::Raid5 => "RAID-5 (Parity)",
        }
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "0" | "raid0" | "stripe" => Some(RaidLevel::Raid0),
            "1" | "raid1" | "mirror" => Some(RaidLevel::Raid1),
            "5" | "raid5" | "parity" => Some(RaidLevel::Raid5),
            _ => None,
        }
    }

    /// Minimum number of devices for this level.
    pub fn min_devices(&self) -> usize {
        match self {
            RaidLevel::Raid0 => 2,
            RaidLevel::Raid1 => 2,
            RaidLevel::Raid5 => 3,
        }
    }
}

// --- Disk Status ---
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiskStatus {
    Online,
    Degraded,
    Failed,
    Rebuilding,
}

impl DiskStatus {
    pub fn name(&self) -> &'static str {
        match self {
            DiskStatus::Online => "online",
            DiskStatus::Degraded => "degraded",
            DiskStatus::Failed => "failed",
            DiskStatus::Rebuilding => "rebuilding",
        }
    }
}

// --- RAID Device ---
/// A single device (disk) in a RAID array.
#[derive(Debug, Clone)]
pub struct RaidDisk {
    pub name: String,
    pub size_sectors: u64,
    pub status: DiskStatus,
    /// Simulated backing store.
    data: Vec<u8>,
}

impl RaidDisk {
    fn new(name: &str, size_sectors: u64) -> Self {
        let size_bytes = size_sectors as usize * 512;
        Self {
            name: String::from(name),
            size_sectors,
            status: DiskStatus::Online,
            data: vec![0u8; size_bytes],
        }
    }

    fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        if self.status == DiskStatus::Failed {
            return Err("disk failed");
        }
        let offset = lba as usize * 512;
        if offset + 512 > self.data.len() {
            return Err("read beyond disk");
        }
        let len = buf.len().min(512);
        buf[..len].copy_from_slice(&self.data[offset..offset + len]);
        Ok(())
    }

    fn write_sector(&mut self, lba: u64, data: &[u8]) -> Result<(), &'static str> {
        if self.status == DiskStatus::Failed {
            return Err("disk failed");
        }
        let offset = lba as usize * 512;
        if offset + 512 > self.data.len() {
            return Err("write beyond disk");
        }
        let len = data.len().min(512);
        self.data[offset..offset + len].copy_from_slice(&data[..len]);
        Ok(())
    }
}

// --- RAID Array ---
/// A RAID array combining multiple disks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArrayStatus {
    Active,
    Degraded,
    Failed,
    Rebuilding,
}

impl ArrayStatus {
    pub fn name(&self) -> &'static str {
        match self {
            ArrayStatus::Active => "active",
            ArrayStatus::Degraded => "degraded",
            ArrayStatus::Failed => "FAILED",
            ArrayStatus::Rebuilding => "rebuilding",
        }
    }
}

pub struct RaidArray {
    pub id: u32,
    pub level: RaidLevel,
    pub stripe_sectors: u64,
    pub disks: Vec<RaidDisk>,
    pub status: ArrayStatus,
    pub rebuild_progress: u32, // percent 0-100
}

impl RaidArray {
    /// Usable capacity in sectors.
    pub fn capacity_sectors(&self) -> u64 {
        if self.disks.is_empty() {
            return 0;
        }
        let min_disk = self.disks.iter().map(|d| d.size_sectors).min().unwrap_or(0);
        match self.level {
            RaidLevel::Raid0 => min_disk * self.disks.len() as u64,
            RaidLevel::Raid1 => min_disk,
            RaidLevel::Raid5 => {
                if self.disks.len() < 3 { 0 }
                else { min_disk * (self.disks.len() as u64 - 1) }
            }
        }
    }

    /// Capacity in KiB.
    pub fn capacity_kb(&self) -> u64 {
        self.capacity_sectors() * 512 / 1024
    }

    /// Recompute array status from disk statuses.
    pub fn update_status(&mut self) {
        let failed = self.disks.iter().filter(|d| d.status == DiskStatus::Failed).count();
        let rebuilding = self.disks.iter().any(|d| d.status == DiskStatus::Rebuilding);
        if rebuilding {
            self.status = ArrayStatus::Rebuilding;
        } else {
            self.status = match self.level {
                RaidLevel::Raid0 => {
                    if failed > 0 { ArrayStatus::Failed } else { ArrayStatus::Active }
                }
                RaidLevel::Raid1 => {
                    if failed >= self.disks.len() { ArrayStatus::Failed }
                    else if failed > 0 { ArrayStatus::Degraded }
                    else { ArrayStatus::Active }
                }
                RaidLevel::Raid5 => {
                    if failed > 1 { ArrayStatus::Failed }
                    else if failed == 1 { ArrayStatus::Degraded }
                    else { ArrayStatus::Active }
                }
            };
        }
    }

    /// Read a logical sector.
    pub fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        TOTAL_READS.fetch_add(1, Ordering::Relaxed);
        match self.level {
            RaidLevel::Raid0 => self.read_raid0(lba, buf),
            RaidLevel::Raid1 => self.read_raid1(lba, buf),
            RaidLevel::Raid5 => self.read_raid5(lba, buf),
        }
    }

    /// Write a logical sector.
    pub fn write_sector(&mut self, lba: u64, data: &[u8]) -> Result<(), &'static str> {
        TOTAL_WRITES.fetch_add(1, Ordering::Relaxed);
        match self.level {
            RaidLevel::Raid0 => self.write_raid0(lba, data),
            RaidLevel::Raid1 => self.write_raid1(lba, data),
            RaidLevel::Raid5 => self.write_raid5(lba, data),
        }
    }

    // --- RAID 0 (Striping) ---
    fn read_raid0(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let n = self.disks.len() as u64;
        let stripe = lba / self.stripe_sectors;
        let disk_idx = (stripe % n) as usize;
        let disk_lba = (stripe / n) * self.stripe_sectors + (lba % self.stripe_sectors);
        self.disks[disk_idx].read_sector(disk_lba, buf)
    }

    fn write_raid0(&mut self, lba: u64, data: &[u8]) -> Result<(), &'static str> {
        let n = self.disks.len() as u64;
        let stripe = lba / self.stripe_sectors;
        let disk_idx = (stripe % n) as usize;
        let disk_lba = (stripe / n) * self.stripe_sectors + (lba % self.stripe_sectors);
        self.disks[disk_idx].write_sector(disk_lba, data)
    }

    // --- RAID 1 (Mirroring) ---
    fn read_raid1(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        // Read from first online disk
        for disk in &self.disks {
            if disk.status == DiskStatus::Online || disk.status == DiskStatus::Degraded {
                return disk.read_sector(lba, buf);
            }
        }
        Err("all mirrors failed")
    }

    fn write_raid1(&mut self, lba: u64, data: &[u8]) -> Result<(), &'static str> {
        let mut success = false;
        for disk in &mut self.disks {
            if disk.status != DiskStatus::Failed {
                if disk.write_sector(lba, data).is_ok() {
                    success = true;
                }
            }
        }
        if success { Ok(()) } else { Err("all mirrors failed on write") }
    }

    // --- RAID 5 (Distributed Parity) ---
    fn read_raid5(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let n = self.disks.len() as u64;
        let data_disks = n - 1;
        let stripe = lba / self.stripe_sectors;
        let stripe_group = stripe / data_disks;
        let stripe_in_group = stripe % data_disks;

        // Parity disk rotates: parity_disk = stripe_group % n
        let parity_disk = (stripe_group % n) as usize;
        // Map logical stripe to physical disk (skip parity disk)
        let mut phys_disk = stripe_in_group as usize;
        if phys_disk >= parity_disk {
            phys_disk += 1;
        }
        let disk_lba = stripe_group * self.stripe_sectors + (lba % self.stripe_sectors);

        if self.disks[phys_disk].status != DiskStatus::Failed {
            return self.disks[phys_disk].read_sector(disk_lba, buf);
        }

        // Reconstruct from parity XOR of all other disks
        let mut reconstructed = vec![0u8; 512];
        for (i, disk) in self.disks.iter().enumerate() {
            if i == phys_disk {
                continue;
            }
            let mut tmp = [0u8; 512];
            disk.read_sector(disk_lba, &mut tmp)?;
            for j in 0..512 {
                reconstructed[j] ^= tmp[j];
            }
        }
        let len = buf.len().min(512);
        buf[..len].copy_from_slice(&reconstructed[..len]);
        Ok(())
    }

    fn write_raid5(&mut self, lba: u64, data: &[u8]) -> Result<(), &'static str> {
        let n = self.disks.len() as u64;
        let data_disks = n - 1;
        let stripe = lba / self.stripe_sectors;
        let stripe_group = stripe / data_disks;
        let stripe_in_group = stripe % data_disks;

        let parity_disk = (stripe_group % n) as usize;
        let mut phys_disk = stripe_in_group as usize;
        if phys_disk >= parity_disk {
            phys_disk += 1;
        }
        let disk_lba = stripe_group * self.stripe_sectors + (lba % self.stripe_sectors);

        // Write data to the target disk
        self.disks[phys_disk].write_sector(disk_lba, data)?;

        // Recompute parity: XOR all data disks
        let mut parity = vec![0u8; 512];
        for (i, disk) in self.disks.iter().enumerate() {
            if i == parity_disk {
                continue;
            }
            let mut tmp = [0u8; 512];
            let _ = disk.read_sector(disk_lba, &mut tmp);
            for j in 0..512 {
                parity[j] ^= tmp[j];
            }
        }
        self.disks[parity_disk].write_sector(disk_lba, &parity)?;
        Ok(())
    }

    /// Rebuild a failed disk (simulate by XOR reconstruction).
    pub fn rebuild(&mut self) -> Result<(), &'static str> {
        let failed_idx = self.disks.iter().position(|d| d.status == DiskStatus::Failed)
            .ok_or("no failed disk to rebuild")?;

        match self.level {
            RaidLevel::Raid0 => return Err("RAID-0 cannot rebuild"),
            RaidLevel::Raid1 => {
                // Copy from any online disk
                let source_idx = self.disks.iter().position(|d| d.status == DiskStatus::Online)
                    .ok_or("no online source disk")?;
                let sectors = self.disks[source_idx].size_sectors;
                self.disks[failed_idx].status = DiskStatus::Rebuilding;
                self.disks[failed_idx].data = vec![0u8; sectors as usize * 512];
                for lba in 0..sectors {
                    let mut buf = [0u8; 512];
                    self.disks[source_idx].read_sector(lba, &mut buf)?;
                    self.disks[failed_idx].write_sector(lba, &buf)?;
                }
            }
            RaidLevel::Raid5 => {
                let sectors = self.disks[failed_idx].size_sectors;
                self.disks[failed_idx].status = DiskStatus::Rebuilding;
                self.disks[failed_idx].data = vec![0u8; sectors as usize * 512];
                for lba in 0..sectors {
                    let mut reconstructed = [0u8; 512];
                    for (i, disk) in self.disks.iter().enumerate() {
                        if i == failed_idx {
                            continue;
                        }
                        let mut tmp = [0u8; 512];
                        let _ = disk.read_sector(lba, &mut tmp);
                        for j in 0..512 {
                            reconstructed[j] ^= tmp[j];
                        }
                    }
                    self.disks[failed_idx].write_sector(lba, &reconstructed)?;
                }
            }
        }
        self.disks[failed_idx].status = DiskStatus::Online;
        self.rebuild_progress = 100;
        self.update_status();
        TOTAL_REBUILDS.fetch_add(1, Ordering::Relaxed);
        crate::serial_println!("[raid] Array {} rebuild complete", self.id);
        Ok(())
    }
}

// --- Global State ---
static ARRAYS: Mutex<Vec<RaidArray>> = Mutex::new(Vec::new());
static INITIALIZED: spin::Once = spin::Once::new();

/// Initialize the RAID subsystem.
pub fn init() {
    INITIALIZED.call_once(|| {
        crate::serial_println!("[raid] Software RAID subsystem initialized");
    });
}

/// Create a new RAID array with the given level and device specs.
/// Each device is specified as "name:size_kb".
pub fn create_array(level: RaidLevel, device_specs: &[(&str, u64)]) -> Result<u32, &'static str> {
    if device_specs.len() < level.min_devices() {
        return Err("not enough devices for this RAID level");
    }
    if device_specs.len() > MAX_DEVICES {
        return Err("too many devices");
    }
    let mut arrays = ARRAYS.lock();
    if arrays.len() >= MAX_ARRAYS {
        return Err("maximum number of arrays reached");
    }

    let id = NEXT_ARRAY_ID.fetch_add(1, Ordering::Relaxed);
    let disks: Vec<RaidDisk> = device_specs.iter().map(|(name, size_kb)| {
        RaidDisk::new(name, *size_kb * 2) // KB to 512-byte sectors
    }).collect();

    let mut array = RaidArray {
        id,
        level,
        stripe_sectors: DEFAULT_STRIPE_SECTORS,
        disks,
        status: ArrayStatus::Active,
        rebuild_progress: 0,
    };
    array.update_status();

    crate::serial_println!("[raid] Created array md{}: {} with {} disks",
        id, level.name(), array.disks.len());
    arrays.push(array);
    Ok(id)
}

/// Add a device to an existing array (for RAID 5 expansion, etc.).
pub fn add_device(array_id: u32, name: &str, size_kb: u64) -> Result<(), &'static str> {
    let mut arrays = ARRAYS.lock();
    let array = arrays.iter_mut().find(|a| a.id == array_id)
        .ok_or("array not found")?;
    if array.disks.len() >= MAX_DEVICES {
        return Err("too many devices");
    }
    array.disks.push(RaidDisk::new(name, size_kb * 2));
    array.update_status();
    Ok(())
}

/// Remove a device from an array (mark as failed).
pub fn remove_device(array_id: u32, device_name: &str) -> Result<(), &'static str> {
    let mut arrays = ARRAYS.lock();
    let array = arrays.iter_mut().find(|a| a.id == array_id)
        .ok_or("array not found")?;
    let disk = array.disks.iter_mut().find(|d| d.name == device_name)
        .ok_or("device not found in array")?;
    disk.status = DiskStatus::Failed;
    array.update_status();
    Ok(())
}

/// Destroy an array.
pub fn destroy_array(array_id: u32) -> Result<(), &'static str> {
    let mut arrays = ARRAYS.lock();
    let idx = arrays.iter().position(|a| a.id == array_id)
        .ok_or("array not found")?;
    arrays.remove(idx);
    crate::serial_println!("[raid] Destroyed array md{}", array_id);
    Ok(())
}

/// Read from a RAID array.
pub fn raid_read(array_id: u32, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
    let arrays = ARRAYS.lock();
    let array = arrays.iter().find(|a| a.id == array_id)
        .ok_or("array not found")?;
    if array.status == ArrayStatus::Failed {
        return Err("array is in FAILED state");
    }
    array.read_sector(lba, buf)
}

/// Write to a RAID array.
pub fn raid_write(array_id: u32, lba: u64, data: &[u8]) -> Result<(), &'static str> {
    let mut arrays = ARRAYS.lock();
    let array = arrays.iter_mut().find(|a| a.id == array_id)
        .ok_or("array not found")?;
    if array.status == ArrayStatus::Failed {
        return Err("array is in FAILED state");
    }
    array.write_sector(lba, data)
}

/// Rebuild a degraded array.
pub fn rebuild(array_id: u32) -> Result<(), &'static str> {
    let mut arrays = ARRAYS.lock();
    let array = arrays.iter_mut().find(|a| a.id == array_id)
        .ok_or("array not found")?;
    array.rebuild()
}

/// List all arrays.
pub fn list_arrays() -> String {
    let arrays = ARRAYS.lock();
    if arrays.is_empty() {
        return String::from("No RAID arrays configured.");
    }
    let mut out = String::from("RAID Arrays:\n");
    for a in arrays.iter() {
        out.push_str(&format!(
            "  md{}: {} [{}] {} disks, {} KB\n",
            a.id, a.level.name(), a.status.name(),
            a.disks.len(), a.capacity_kb(),
        ));
    }
    out
}

/// Detailed info for one array.
pub fn array_info(array_id: u32) -> String {
    let arrays = ARRAYS.lock();
    match arrays.iter().find(|a| a.id == array_id) {
        Some(a) => {
            let mut out = format!(
                "RAID Array md{}:\n\
                 Level:           {}\n\
                 Status:          {}\n\
                 Stripe Size:     {} sectors ({} KB)\n\
                 Capacity:        {} sectors ({} KB)\n\
                 Rebuild:         {}%\n\
                 Disks:           {}\n",
                a.id,
                a.level.name(),
                a.status.name(),
                a.stripe_sectors, a.stripe_sectors * 512 / 1024,
                a.capacity_sectors(), a.capacity_kb(),
                a.rebuild_progress,
                a.disks.len(),
            );
            for (i, d) in a.disks.iter().enumerate() {
                out.push_str(&format!(
                    "  [{}] {} - {} sectors ({} KB) [{}]\n",
                    i, d.name, d.size_sectors, d.size_sectors * 512 / 1024, d.status.name(),
                ));
            }
            out
        }
        None => format!("Array md{} not found", array_id),
    }
}

/// RAID subsystem statistics.
pub fn raid_stats() -> String {
    let arrays = ARRAYS.lock();
    format!(
        "RAID Statistics:\n\
         Arrays:          {}\n\
         Total reads:     {}\n\
         Total writes:    {}\n\
         Total rebuilds:  {}",
        arrays.len(),
        TOTAL_READS.load(Ordering::Relaxed),
        TOTAL_WRITES.load(Ordering::Relaxed),
        TOTAL_REBUILDS.load(Ordering::Relaxed),
    )
}
