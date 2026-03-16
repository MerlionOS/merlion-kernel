/// System installer for MerlionOS.
/// Provides disk partitioning, filesystem creation, OS installation,
/// bootloader setup, and first-boot configuration.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Sector size in bytes.
const SECTOR_SIZE: u64 = 512;

/// Minimum disk size for installation (64 MiB).
const MIN_DISK_SIZE_MB: u64 = 64;

/// Default EFI System Partition size in MiB.
const DEFAULT_ESP_SIZE_MB: u64 = 32;

/// Default swap partition size in MiB.
const DEFAULT_SWAP_SIZE_MB: u64 = 16;

/// Maximum number of install log entries.
const MAX_LOG_ENTRIES: usize = 512;

/// Maximum partitions per disk.
const MAX_PARTITIONS: usize = 128;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Current installation phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPhase {
    /// Not started.
    Idle,
    /// Welcome screen / language selection.
    Welcome,
    /// Disk selection.
    DiskSelect,
    /// Partition layout.
    Partitioning,
    /// Formatting partitions.
    Formatting,
    /// Copying files.
    FileCopy,
    /// Installing bootloader.
    Bootloader,
    /// User and hostname setup.
    UserSetup,
    /// Network configuration.
    NetworkConfig,
    /// Package / module group selection.
    PackageSelect,
    /// Installation complete.
    Done,
    /// Installation failed.
    Failed,
}

/// Partition type identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionType {
    /// EFI System Partition (FAT16).
    EfiSystem,
    /// Linux root filesystem (ext4).
    LinuxRoot,
    /// Swap partition.
    Swap,
    /// Generic data partition.
    Data,
}

/// Filesystem type for formatting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsType {
    Fat16,
    Ext4,
    Swap,
}

/// A detected block device available for installation.
#[derive(Debug, Clone)]
pub struct DiskInfo {
    /// Device name (e.g. "sda", "nvme0", "vda").
    pub name: String,
    /// Total size in bytes.
    pub size_bytes: u64,
    /// Size in MiB.
    pub size_mb: u64,
    /// Number of sectors.
    pub sectors: u64,
    /// Whether this disk already has partitions.
    pub has_partitions: bool,
}

/// A partition definition for the installer.
#[derive(Debug, Clone)]
pub struct PartitionDef {
    /// Partition number (1-based).
    pub number: u32,
    /// Partition type.
    pub ptype: PartitionType,
    /// Size in MiB.
    pub size_mb: u64,
    /// Start LBA.
    pub start_lba: u64,
    /// End LBA (inclusive).
    pub end_lba: u64,
    /// Filesystem to create on this partition.
    pub fs: FsType,
    /// Label for the partition.
    pub label: String,
}

/// Network configuration for first boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetMode {
    /// Obtain address via DHCP.
    Dhcp,
    /// Use a static IP configuration.
    Static,
    /// No network.
    None,
}

/// Static IP configuration details.
#[derive(Debug, Clone)]
pub struct StaticNetConfig {
    /// IPv4 address as 4 octets.
    pub ip: [u8; 4],
    /// Subnet mask as 4 octets.
    pub netmask: [u8; 4],
    /// Default gateway as 4 octets.
    pub gateway: [u8; 4],
    /// DNS server as 4 octets.
    pub dns: [u8; 4],
}

/// Module group for package selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleGroup {
    /// Core system (always installed).
    Core,
    /// Networking stack.
    Network,
    /// AI and ML features.
    Ai,
    /// Development tools.
    Development,
    /// Games and demos.
    Games,
}

/// A single installation log entry.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Tick count when this entry was recorded.
    pub tick: u64,
    /// Log level: 'I' = info, 'W' = warn, 'E' = error.
    pub level: char,
    /// Log message.
    pub message: String,
}

/// Installation progress indicator.
#[derive(Debug, Clone, Copy)]
pub struct InstallProgress {
    /// Current phase.
    pub phase: InstallPhase,
    /// Percentage complete within current phase (0-100).
    pub percent: u32,
    /// Total files copied so far.
    pub files_copied: u32,
    /// Total bytes written so far.
    pub bytes_written: u64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static INSTALL_PHASE: AtomicU32 = AtomicU32::new(0); // 0 = Idle

struct InstallerState {
    disks: Vec<DiskInfo>,
    selected_disk: Option<usize>,
    partitions: Vec<PartitionDef>,
    hostname: String,
    username: String,
    timezone: String,
    net_mode: NetMode,
    static_net: Option<StaticNetConfig>,
    selected_groups: Vec<ModuleGroup>,
    log: Vec<LogEntry>,
    files_copied: u32,
    bytes_written: u64,
    initialized: bool,
}

static STATE: Mutex<InstallerState> = Mutex::new(InstallerState {
    disks: Vec::new(),
    selected_disk: None,
    partitions: Vec::new(),
    hostname: String::new(),
    username: String::new(),
    timezone: String::new(),
    net_mode: NetMode::Dhcp,
    static_net: None,
    selected_groups: Vec::new(),
    log: Vec::new(),
    files_copied: 0,
    bytes_written: 0,
    initialized: false,
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn phase_to_u32(phase: InstallPhase) -> u32 {
    match phase {
        InstallPhase::Idle => 0,
        InstallPhase::Welcome => 1,
        InstallPhase::DiskSelect => 2,
        InstallPhase::Partitioning => 3,
        InstallPhase::Formatting => 4,
        InstallPhase::FileCopy => 5,
        InstallPhase::Bootloader => 6,
        InstallPhase::UserSetup => 7,
        InstallPhase::NetworkConfig => 8,
        InstallPhase::PackageSelect => 9,
        InstallPhase::Done => 10,
        InstallPhase::Failed => 11,
    }
}

fn u32_to_phase(v: u32) -> InstallPhase {
    match v {
        1 => InstallPhase::Welcome,
        2 => InstallPhase::DiskSelect,
        3 => InstallPhase::Partitioning,
        4 => InstallPhase::Formatting,
        5 => InstallPhase::FileCopy,
        6 => InstallPhase::Bootloader,
        7 => InstallPhase::UserSetup,
        8 => InstallPhase::NetworkConfig,
        9 => InstallPhase::PackageSelect,
        10 => InstallPhase::Done,
        11 => InstallPhase::Failed,
        _ => InstallPhase::Idle,
    }
}

fn set_phase(phase: InstallPhase) {
    INSTALL_PHASE.store(phase_to_u32(phase), Ordering::SeqCst);
}

fn get_phase() -> InstallPhase {
    u32_to_phase(INSTALL_PHASE.load(Ordering::SeqCst))
}

fn log_entry(state: &mut InstallerState, level: char, msg: String) {
    let tick = crate::timer::ticks();
    if state.log.len() >= MAX_LOG_ENTRIES {
        state.log.remove(0);
    }
    state.log.push(LogEntry {
        tick,
        level,
        message: msg,
    });
}

fn group_name(g: ModuleGroup) -> &'static str {
    match g {
        ModuleGroup::Core => "core",
        ModuleGroup::Network => "network",
        ModuleGroup::Ai => "ai",
        ModuleGroup::Development => "development",
        ModuleGroup::Games => "games",
    }
}

fn fs_type_name(fs: FsType) -> &'static str {
    match fs {
        FsType::Fat16 => "FAT16",
        FsType::Ext4 => "ext4",
        FsType::Swap => "swap",
    }
}

fn part_type_name(pt: PartitionType) -> &'static str {
    match pt {
        PartitionType::EfiSystem => "EFI System",
        PartitionType::LinuxRoot => "Linux Root",
        PartitionType::Swap => "Swap",
        PartitionType::Data => "Data",
    }
}

// ---------------------------------------------------------------------------
// Disk detection
// ---------------------------------------------------------------------------

/// Scan for available block devices and populate the disk list.
pub fn list_disks() -> Vec<DiskInfo> {
    let devs = crate::blkdev::list();
    devs.into_iter()
        .map(|d| DiskInfo {
            name: d.name.clone(),
            size_bytes: d.size_kb * 1024,
            size_mb: d.size_kb / 1024,
            sectors: d.blocks,
            has_partitions: false, // simplified; real impl would probe GPT
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Partition manager
// ---------------------------------------------------------------------------

/// Create a default partition layout for a disk of the given size.
/// Layout: EFI System (32 MiB) + Swap (16 MiB) + Root (remainder).
pub fn default_partition_layout(disk_size_mb: u64) -> Result<Vec<PartitionDef>, &'static str> {
    if disk_size_mb < MIN_DISK_SIZE_MB {
        return Err("disk too small for installation");
    }

    let mut parts = Vec::new();
    let mut current_lba: u64 = 2048; // Start after GPT header region.

    // EFI System Partition.
    let esp_sectors = DEFAULT_ESP_SIZE_MB * 1024 * 1024 / SECTOR_SIZE;
    parts.push(PartitionDef {
        number: 1,
        ptype: PartitionType::EfiSystem,
        size_mb: DEFAULT_ESP_SIZE_MB,
        start_lba: current_lba,
        end_lba: current_lba + esp_sectors - 1,
        fs: FsType::Fat16,
        label: String::from("EFI"),
    });
    current_lba += esp_sectors;

    // Swap partition.
    let swap_sectors = DEFAULT_SWAP_SIZE_MB * 1024 * 1024 / SECTOR_SIZE;
    parts.push(PartitionDef {
        number: 2,
        ptype: PartitionType::Swap,
        size_mb: DEFAULT_SWAP_SIZE_MB,
        start_lba: current_lba,
        end_lba: current_lba + swap_sectors - 1,
        fs: FsType::Swap,
        label: String::from("swap"),
    });
    current_lba += swap_sectors;

    // Root partition: use remaining space.
    let total_sectors = disk_size_mb * 1024 * 1024 / SECTOR_SIZE;
    let end_usable = total_sectors.saturating_sub(34); // Reserve backup GPT.
    if current_lba >= end_usable {
        return Err("not enough space for root partition");
    }
    let root_mb = (end_usable - current_lba) * SECTOR_SIZE / (1024 * 1024);
    parts.push(PartitionDef {
        number: 3,
        ptype: PartitionType::LinuxRoot,
        size_mb: root_mb,
        start_lba: current_lba,
        end_lba: end_usable - 1,
        fs: FsType::Ext4,
        label: String::from("merlion-root"),
    });

    Ok(parts)
}

/// Create a custom partition on the selected disk.
pub fn partition_disk(
    number: u32,
    ptype: PartitionType,
    size_mb: u64,
    start_lba: u64,
    fs: FsType,
    label: &str,
) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    if state.partitions.len() >= MAX_PARTITIONS {
        return Err("maximum partition count reached");
    }

    let sectors = size_mb * 1024 * 1024 / SECTOR_SIZE;
    let end_lba = start_lba + sectors - 1;

    // Check for overlaps.
    for existing in &state.partitions {
        if start_lba <= existing.end_lba && end_lba >= existing.start_lba {
            return Err("partition overlaps with existing partition");
        }
    }

    state.partitions.push(PartitionDef {
        number,
        ptype,
        size_mb,
        start_lba,
        end_lba,
        fs,
        label: String::from(label),
    });

    log_entry(
        &mut state,
        'I',
        format!("created partition {} ({} MiB, {})", number, size_mb, part_type_name(ptype)),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Filesystem formatter
// ---------------------------------------------------------------------------

/// Format a partition with the specified filesystem.
/// In this simulated installer, we log the operation and update state.
pub fn format_partition(part_number: u32) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    let part = state
        .partitions
        .iter()
        .find(|p| p.number == part_number)
        .ok_or("partition not found")?;

    let fs_name = fs_type_name(part.fs);
    let label = part.label.clone();
    let size = part.size_mb;

    log_entry(
        &mut state,
        'I',
        format!("formatting partition {} as {} (label={}, {} MiB)", part_number, fs_name, label, size),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// File copy
// ---------------------------------------------------------------------------

/// Simulate copying kernel and boot files to the target disk.
fn copy_system_files(state: &mut InstallerState) {
    // Simulated file list with sizes.
    let files: &[(&str, u64)] = &[
        ("/boot/merlion.bin", 262144),
        ("/boot/limine.cfg", 1024),
        ("/boot/limine.sys", 32768),
        ("/etc/hostname", 64),
        ("/etc/fstab", 256),
        ("/etc/profile", 512),
        ("/etc/motd", 256),
        ("/etc/passwd", 128),
        ("/etc/shadow", 128),
        ("/etc/group", 128),
        ("/etc/modules.conf", 256),
        ("/etc/network/interfaces", 256),
    ];

    for &(path, size) in files {
        state.files_copied += 1;
        state.bytes_written += size;
        log_entry(state, 'I', format!("copied {} ({} bytes)", path, size));
    }
}

// ---------------------------------------------------------------------------
// Bootloader setup
// ---------------------------------------------------------------------------

/// Generate a Limine bootloader configuration string.
fn generate_boot_config(hostname: &str) -> String {
    format!(
        "TIMEOUT=3\n\
         :MerlionOS on {}\n\
         PROTOCOL=limine\n\
         KERNEL_PATH=boot:///boot/merlion.bin\n\
         KERNEL_CMDLINE=console=ttyS0 hostname={}\n",
        hostname, hostname
    )
}

/// Simulate installing the bootloader.
fn install_bootloader(state: &mut InstallerState) {
    let hostname = if state.hostname.is_empty() {
        "merlion"
    } else {
        &state.hostname
    };
    let config = generate_boot_config(hostname);
    log_entry(
        state,
        'I',
        format!("bootloader config ({} bytes):\n{}", config.len(), config),
    );
    log_entry(state, 'I', String::from("bootloader installed to EFI System Partition"));
}

// ---------------------------------------------------------------------------
// User / hostname / timezone setup
// ---------------------------------------------------------------------------

/// Set the hostname for the installed system.
pub fn set_hostname(name: &str) -> Result<(), &'static str> {
    if name.is_empty() || name.len() > 64 {
        return Err("hostname must be 1-64 characters");
    }
    let mut state = STATE.lock();
    state.hostname = String::from(name);
    log_entry(&mut state, 'I', format!("hostname set to '{}'", name));
    Ok(())
}

/// Set the initial user account.
pub fn set_user(username: &str) -> Result<(), &'static str> {
    if username.is_empty() || username.len() > 32 {
        return Err("username must be 1-32 characters");
    }
    // Basic validation: alphanumeric + underscore.
    for c in username.bytes() {
        if !c.is_ascii_alphanumeric() && c != b'_' {
            return Err("username must be alphanumeric or underscore");
        }
    }
    let mut state = STATE.lock();
    state.username = String::from(username);
    log_entry(&mut state, 'I', format!("user '{}' will be created", username));
    Ok(())
}

/// Set the timezone.
pub fn set_timezone(tz: &str) -> Result<(), &'static str> {
    if tz.is_empty() {
        return Err("timezone cannot be empty");
    }
    let mut state = STATE.lock();
    state.timezone = String::from(tz);
    log_entry(&mut state, 'I', format!("timezone set to '{}'", tz));
    Ok(())
}

// ---------------------------------------------------------------------------
// Network configuration
// ---------------------------------------------------------------------------

/// Set network mode for first boot.
pub fn set_network(mode: NetMode, static_cfg: Option<StaticNetConfig>) {
    let mut state = STATE.lock();
    state.net_mode = mode;
    state.static_net = static_cfg;
    let mode_str = match mode {
        NetMode::Dhcp => "DHCP",
        NetMode::Static => "static",
        NetMode::None => "none",
    };
    log_entry(&mut state, 'I', format!("network configured as {}", mode_str));
}

// ---------------------------------------------------------------------------
// Package / module group selection
// ---------------------------------------------------------------------------

/// Select which module groups to install.
pub fn select_packages(groups: Vec<ModuleGroup>) {
    let mut state = STATE.lock();
    let names: Vec<&str> = groups.iter().map(|g| group_name(*g)).collect();
    log_entry(&mut state, 'I', format!("selected groups: {:?}", names));
    state.selected_groups = groups;
}

/// List available module groups with descriptions.
pub fn available_groups() -> Vec<(ModuleGroup, &'static str, &'static str)> {
    Vec::from([
        (ModuleGroup::Core, "core", "Essential kernel, shell, VFS, process management"),
        (ModuleGroup::Network, "network", "TCP/IP, DHCP, DNS, HTTP, SSH"),
        (ModuleGroup::Ai, "ai", "AI shell, agents, self-healing, inference"),
        (ModuleGroup::Development, "development", "Editor, debugger, profiler, build tools"),
        (ModuleGroup::Games, "games", "Snake, Tetris, Forth, fortune"),
    ])
}

// ---------------------------------------------------------------------------
// Installation orchestration
// ---------------------------------------------------------------------------

/// Start the full installation process.
/// This runs through all phases sequentially.
pub fn start_install() -> Result<(), &'static str> {
    if get_phase() != InstallPhase::Idle {
        return Err("installation already in progress");
    }

    // Phase 1: Welcome.
    set_phase(InstallPhase::Welcome);
    {
        let mut state = STATE.lock();
        log_entry(&mut state, 'I', String::from("=== MerlionOS Installer ==="));
        log_entry(&mut state, 'I', String::from("Welcome to MerlionOS installation"));
    }

    // Phase 2: Disk detection.
    set_phase(InstallPhase::DiskSelect);
    let disks = list_disks();
    {
        let mut state = STATE.lock();
        if disks.is_empty() {
            log_entry(&mut state, 'E', String::from("no block devices found"));
            set_phase(InstallPhase::Failed);
            return Err("no block devices found");
        }
        log_entry(&mut state, 'I', format!("found {} disk(s)", disks.len()));
        // Auto-select first disk.
        state.selected_disk = Some(0);
        state.disks = disks;
    }

    // Phase 3: Partitioning.
    set_phase(InstallPhase::Partitioning);
    {
        let mut state = STATE.lock();
        let disk_mb = state
            .disks
            .first()
            .map(|d| d.size_mb)
            .unwrap_or(0);
        match default_partition_layout(disk_mb) {
            Ok(parts) => {
                log_entry(
                    &mut state,
                    'I',
                    format!("created {} partitions", parts.len()),
                );
                state.partitions = parts;
            }
            Err(e) => {
                log_entry(&mut state, 'E', format!("partitioning failed: {}", e));
                set_phase(InstallPhase::Failed);
                return Err(e);
            }
        }
    }

    // Phase 4: Formatting.
    set_phase(InstallPhase::Formatting);
    {
        let mut state = STATE.lock();
        let part_nums: Vec<u32> = state.partitions.iter().map(|p| p.number).collect();
        for num in part_nums {
            let part = state.partitions.iter().find(|p| p.number == num).unwrap();
            let fs_name = fs_type_name(part.fs);
            let label = part.label.clone();
            let size = part.size_mb;
            log_entry(
                &mut state,
                'I',
                format!("formatting partition {} as {} (label={}, {} MiB)", num, fs_name, label, size),
            );
        }
    }

    // Phase 5: File copy.
    set_phase(InstallPhase::FileCopy);
    {
        let mut state = STATE.lock();
        copy_system_files(&mut state);
        let msg = format!(
            "file copy complete: {} files, {} bytes",
            state.files_copied, state.bytes_written
        );
        log_entry(&mut state, 'I', msg);
    }

    // Phase 6: Bootloader.
    set_phase(InstallPhase::Bootloader);
    {
        let mut state = STATE.lock();
        install_bootloader(&mut state);
    }

    // Phase 7: User setup (use defaults if not configured).
    set_phase(InstallPhase::UserSetup);
    {
        let mut state = STATE.lock();
        if state.hostname.is_empty() {
            state.hostname = String::from("merlion");
        }
        if state.username.is_empty() {
            state.username = String::from("admin");
        }
        if state.timezone.is_empty() {
            state.timezone = String::from("Asia/Singapore");
        }
        let msg = format!(
            "user='{}', hostname='{}', tz='{}'",
            state.username, state.hostname, state.timezone
        );
        log_entry(&mut state, 'I', msg);
    }

    // Phase 8: Network config.
    set_phase(InstallPhase::NetworkConfig);
    {
        let mut state = STATE.lock();
        let mode_str = match state.net_mode {
            NetMode::Dhcp => "DHCP",
            NetMode::Static => "static",
            NetMode::None => "none",
        };
        log_entry(&mut state, 'I', format!("network mode: {}", mode_str));
    }

    // Phase 9: Package selection (use defaults if not configured).
    set_phase(InstallPhase::PackageSelect);
    {
        let mut state = STATE.lock();
        if state.selected_groups.is_empty() {
            state.selected_groups = Vec::from([ModuleGroup::Core, ModuleGroup::Network]);
        }
        let names: Vec<&str> = state.selected_groups.iter().map(|g| group_name(*g)).collect();
        log_entry(&mut state, 'I', format!("installing groups: {:?}", names));
    }

    // Phase 10: Done.
    set_phase(InstallPhase::Done);
    {
        let mut state = STATE.lock();
        log_entry(&mut state, 'I', String::from("=== Installation complete ==="));
        log_entry(&mut state, 'I', String::from("Please reboot to start MerlionOS."));
    }

    Ok(())
}

/// Get current installation status.
pub fn install_status() -> InstallProgress {
    let phase = get_phase();
    let state = STATE.lock();
    let percent = match phase {
        InstallPhase::Idle => 0,
        InstallPhase::Welcome => 5,
        InstallPhase::DiskSelect => 10,
        InstallPhase::Partitioning => 20,
        InstallPhase::Formatting => 35,
        InstallPhase::FileCopy => 55,
        InstallPhase::Bootloader => 70,
        InstallPhase::UserSetup => 80,
        InstallPhase::NetworkConfig => 85,
        InstallPhase::PackageSelect => 90,
        InstallPhase::Done => 100,
        InstallPhase::Failed => 0,
    };
    InstallProgress {
        phase,
        percent,
        files_copied: state.files_copied,
        bytes_written: state.bytes_written,
    }
}

/// Get the full installation log.
pub fn install_log() -> Vec<String> {
    let state = STATE.lock();
    state
        .log
        .iter()
        .map(|e| format!("[{:>8}] [{}] {}", e.tick, e.level, e.message))
        .collect()
}

/// Format the installation log as a single string.
pub fn format_log() -> String {
    let entries = install_log();
    let mut out = String::new();
    for line in entries {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Repair / uninstall
// ---------------------------------------------------------------------------

/// Attempt to repair a broken installation by reinstalling the bootloader.
pub fn repair_bootloader() -> Result<(), &'static str> {
    let mut state = STATE.lock();
    if state.partitions.is_empty() {
        return Err("no partitions found; cannot repair");
    }
    // Check that an EFI partition exists.
    let has_esp = state
        .partitions
        .iter()
        .any(|p| p.ptype == PartitionType::EfiSystem);
    if !has_esp {
        return Err("no EFI System Partition found");
    }
    log_entry(&mut state, 'I', String::from("repairing bootloader..."));
    install_bootloader(&mut state);
    log_entry(&mut state, 'I', String::from("bootloader repair complete"));
    Ok(())
}

/// Reset installer state to allow a fresh installation.
pub fn reset() {
    set_phase(InstallPhase::Idle);
    let mut state = STATE.lock();
    state.disks.clear();
    state.selected_disk = None;
    state.partitions.clear();
    state.hostname.clear();
    state.username.clear();
    state.timezone.clear();
    state.net_mode = NetMode::Dhcp;
    state.static_net = None;
    state.selected_groups.clear();
    state.log.clear();
    state.files_copied = 0;
    state.bytes_written = 0;
}

/// Format the current partition table as a human-readable string.
pub fn format_partitions() -> String {
    let state = STATE.lock();
    if state.partitions.is_empty() {
        return String::from("No partitions defined.\n");
    }

    let mut out = String::new();
    out.push_str(&format!(
        "{:<6}{:<14}{:<14}{:<10}{:<16}{}\n",
        "#", "Start LBA", "End LBA", "Size(MiB)", "Type", "Label"
    ));
    for p in &state.partitions {
        out.push_str(&format!(
            "{:<6}{:<14}{:<14}{:<10}{:<16}{}\n",
            p.number,
            p.start_lba,
            p.end_lba,
            p.size_mb,
            part_type_name(p.ptype),
            p.label,
        ));
    }
    out
}

/// Format disk list as a human-readable string.
pub fn format_disks() -> String {
    let disks = list_disks();
    if disks.is_empty() {
        return String::from("No block devices found.\n");
    }
    let mut out = String::new();
    out.push_str(&format!("{:<10}{:<12}{:<14}{}\n", "Device", "Size(MiB)", "Sectors", "Partitioned"));
    for d in &disks {
        out.push_str(&format!(
            "{:<10}{:<12}{:<14}{}\n",
            d.name,
            d.size_mb,
            d.sectors,
            if d.has_partitions { "yes" } else { "no" },
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the installer subsystem.
pub fn init() {
    let mut state = STATE.lock();
    if state.initialized {
        return;
    }
    state.initialized = true;
    crate::klog_println!("[installer] initialized");
}
