/// UEFI Runtime Services for MerlionOS.
/// Provides access to UEFI runtime services after ExitBootServices,
/// including variable storage, time services, and capsule updates.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_VARIABLES: usize = 256;
const MAX_VARIABLE_NAME: usize = 128;
const MAX_VARIABLE_DATA: usize = 4096;
const MAX_MEMORY_MAP_ENTRIES: usize = 64;
const MAX_CAPSULES: usize = 16;

/// Well-known UEFI variable namespace GUIDs (simulated as u128).
pub const EFI_GLOBAL_VARIABLE: u128 = 0x8BE4DF61_93CA_11D2_AA0D_00E098032B8C;
pub const EFI_IMAGE_SECURITY_DATABASE: u128 = 0xD719B2CB_3D3A_4596_A3BC_DAD00E67656F;
pub const MERLION_OS_VARIABLE: u128 = 0x4D45524C_494F_4E4F_5300_000000000001;

// ---------------------------------------------------------------------------
// UEFI variable attributes
// ---------------------------------------------------------------------------

pub const EFI_VARIABLE_NON_VOLATILE: u32 = 0x0000_0001;
pub const EFI_VARIABLE_BOOTSERVICE_ACCESS: u32 = 0x0000_0002;
pub const EFI_VARIABLE_RUNTIME_ACCESS: u32 = 0x0000_0004;
pub const EFI_VARIABLE_TIME_BASED_AUTHENTICATED: u32 = 0x0000_0020;
pub const EFI_VARIABLE_APPEND_WRITE: u32 = 0x0000_0040;

/// Default attributes for boot variables.
const BOOT_ATTRS: u32 =
    EFI_VARIABLE_NON_VOLATILE | EFI_VARIABLE_BOOTSERVICE_ACCESS | EFI_VARIABLE_RUNTIME_ACCESS;

// ---------------------------------------------------------------------------
// UEFI time
// ---------------------------------------------------------------------------

/// UEFI EFI_TIME structure (simplified).
#[derive(Debug, Clone, Copy)]
pub struct EfiTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub nanosecond: u32,
    pub timezone: i16,
    pub daylight: u8,
}

impl EfiTime {
    pub fn zero() -> Self {
        Self {
            year: 0, month: 0, day: 0,
            hour: 0, minute: 0, second: 0,
            nanosecond: 0, timezone: 0, daylight: 0,
        }
    }
}

impl core::fmt::Display for EfiTime {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day,
            self.hour, self.minute, self.second
        )
    }
}

// ---------------------------------------------------------------------------
// Reset type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetType {
    Cold,
    Warm,
    Shutdown,
    PlatformSpecific,
}

impl ResetType {
    pub fn label(self) -> &'static str {
        match self {
            ResetType::Cold => "EfiResetCold",
            ResetType::Warm => "EfiResetWarm",
            ResetType::Shutdown => "EfiResetShutdown",
            ResetType::PlatformSpecific => "EfiResetPlatformSpecific",
        }
    }
}

// ---------------------------------------------------------------------------
// UEFI variable storage
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct UefiVariable {
    name: String,
    namespace: u128,
    attributes: u32,
    data: Vec<u8>,
}

struct VariableStore {
    variables: Vec<UefiVariable>,
}

impl VariableStore {
    const fn new() -> Self {
        Self { variables: Vec::new() }
    }

    fn get(&self, name: &str, namespace: u128) -> Option<&UefiVariable> {
        self.variables.iter().find(|v| v.name == name && v.namespace == namespace)
    }

    fn set(&mut self, name: &str, namespace: u128, attrs: u32, data: &[u8]) -> Result<(), &'static str> {
        if self.variables.len() >= MAX_VARIABLES && self.get(name, namespace).is_none() {
            return Err("variable store full");
        }
        if name.len() > MAX_VARIABLE_NAME {
            return Err("variable name too long");
        }
        if data.len() > MAX_VARIABLE_DATA {
            return Err("variable data too large");
        }

        // Update existing or insert new
        if let Some(var) = self.variables.iter_mut().find(|v| v.name == name && v.namespace == namespace) {
            var.attributes = attrs;
            var.data = Vec::from(data);
        } else {
            self.variables.push(UefiVariable {
                name: String::from(name),
                namespace,
                attributes: attrs,
                data: Vec::from(data),
            });
        }
        Ok(())
    }

    fn delete(&mut self, name: &str, namespace: u128) -> bool {
        let before = self.variables.len();
        self.variables.retain(|v| !(v.name == name && v.namespace == namespace));
        self.variables.len() < before
    }

    fn list(&self) -> Vec<(String, u128, u32, usize)> {
        self.variables
            .iter()
            .map(|v| (v.name.clone(), v.namespace, v.attributes, v.data.len()))
            .collect()
    }

    fn total_size(&self) -> usize {
        self.variables.iter().map(|v| v.data.len()).sum()
    }
}

// ---------------------------------------------------------------------------
// Memory map entries (EFI runtime regions)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub enum EfiMemoryType {
    RuntimeServicesCode,
    RuntimeServicesData,
    BootServicesCode,
    BootServicesData,
    ConventionalMemory,
    ACPIReclaimMemory,
    ACPIMemoryNVS,
    MemoryMappedIO,
}

impl EfiMemoryType {
    pub fn label(self) -> &'static str {
        match self {
            EfiMemoryType::RuntimeServicesCode => "EfiRuntimeServicesCode",
            EfiMemoryType::RuntimeServicesData => "EfiRuntimeServicesData",
            EfiMemoryType::BootServicesCode => "EfiBootServicesCode",
            EfiMemoryType::BootServicesData => "EfiBootServicesData",
            EfiMemoryType::ConventionalMemory => "EfiConventionalMemory",
            EfiMemoryType::ACPIReclaimMemory => "EfiACPIReclaimMemory",
            EfiMemoryType::ACPIMemoryNVS => "EfiACPIMemoryNVS",
            EfiMemoryType::MemoryMappedIO => "EfiMemoryMappedIO",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EfiMemoryDescriptor {
    pub mem_type: EfiMemoryType,
    pub phys_start: u64,
    pub virt_start: u64,
    pub num_pages: u64,
    pub attribute: u64,
}

// ---------------------------------------------------------------------------
// Capsule update
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapsuleStatus {
    Pending,
    Processing,
    Success,
    Failed,
}

impl CapsuleStatus {
    pub fn label(self) -> &'static str {
        match self {
            CapsuleStatus::Pending => "Pending",
            CapsuleStatus::Processing => "Processing",
            CapsuleStatus::Success => "Success",
            CapsuleStatus::Failed => "Failed",
        }
    }
}

#[derive(Clone)]
struct CapsuleEntry {
    guid: u128,
    flags: u32,
    data_size: usize,
    status: CapsuleStatus,
}

// ---------------------------------------------------------------------------
// EFI System Table (simulated)
// ---------------------------------------------------------------------------

struct EfiSystemTable {
    firmware_vendor: &'static str,
    firmware_revision: u32,
    uefi_revision: u32,
    config_table_count: usize,
}

impl EfiSystemTable {
    const fn default() -> Self {
        Self {
            firmware_vendor: "MerlionOS Simulated UEFI",
            firmware_revision: 0x0001_0000,
            uefi_revision: 0x0002_0070, // UEFI 2.7
            config_table_count: 3,
        }
    }

    fn revision_string(&self) -> String {
        let major = (self.uefi_revision >> 16) & 0xFFFF;
        let minor = self.uefi_revision & 0xFFFF;
        format!("{}.{}", major, minor / 10)
    }
}

// ---------------------------------------------------------------------------
// Secure Boot state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecureBootMode {
    Disabled,
    SetupMode,
    UserMode,
    AuditMode,
    DeployedMode,
}

impl SecureBootMode {
    pub fn label(self) -> &'static str {
        match self {
            SecureBootMode::Disabled => "Disabled",
            SecureBootMode::SetupMode => "SetupMode",
            SecureBootMode::UserMode => "UserMode",
            SecureBootMode::AuditMode => "AuditMode",
            SecureBootMode::DeployedMode => "DeployedMode",
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static VARIABLE_STORE: Mutex<VariableStore> = Mutex::new(VariableStore::new());
static MEMORY_MAP: Mutex<Vec<EfiMemoryDescriptor>> = Mutex::new(Vec::new());
static CAPSULES: Mutex<Vec<CapsuleEntry>> = Mutex::new(Vec::new());
static SYSTEM_TABLE: Mutex<EfiSystemTable> = Mutex::new(EfiSystemTable::default());
static SECURE_BOOT_MODE: Mutex<SecureBootMode> = Mutex::new(SecureBootMode::Disabled);

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static GET_VARIABLE_CALLS: AtomicU64 = AtomicU64::new(0);
static SET_VARIABLE_CALLS: AtomicU64 = AtomicU64::new(0);
static GET_TIME_CALLS: AtomicU64 = AtomicU64::new(0);
static RESET_CALLS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the UEFI runtime services subsystem.
pub fn init() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }

    // Populate default boot variables
    let store = &VARIABLE_STORE;
    {
        let mut s = store.lock();
        // BootOrder: boot entry 0x0000 first
        let _ = s.set("BootOrder", EFI_GLOBAL_VARIABLE, BOOT_ATTRS, &[0x00, 0x00]);
        // BootCurrent
        let _ = s.set("BootCurrent", EFI_GLOBAL_VARIABLE, BOOT_ATTRS, &[0x00, 0x00]);
        // Timeout (5 seconds)
        let _ = s.set("Timeout", EFI_GLOBAL_VARIABLE, BOOT_ATTRS, &[0x05, 0x00]);
        // SecureBoot = 0 (disabled)
        let _ = s.set("SecureBoot", EFI_GLOBAL_VARIABLE, BOOT_ATTRS, &[0x00]);
        // SetupMode = 1 (in setup mode)
        let _ = s.set("SetupMode", EFI_GLOBAL_VARIABLE, BOOT_ATTRS, &[0x01]);
        // Platform key (PK) — empty placeholder
        let _ = s.set("PK", EFI_IMAGE_SECURITY_DATABASE, BOOT_ATTRS, &[]);
        // Key exchange keys (KEK) — empty placeholder
        let _ = s.set("KEK", EFI_IMAGE_SECURITY_DATABASE, BOOT_ATTRS, &[]);
        // Authorized signatures database (db)
        let _ = s.set("db", EFI_IMAGE_SECURITY_DATABASE, BOOT_ATTRS, &[]);
        // Forbidden signatures database (dbx)
        let _ = s.set("dbx", EFI_IMAGE_SECURITY_DATABASE, BOOT_ATTRS, &[]);
        // OS-specific variables
        let _ = s.set("OSName", MERLION_OS_VARIABLE, BOOT_ATTRS, b"MerlionOS");
        let _ = s.set("OSVersion", MERLION_OS_VARIABLE, BOOT_ATTRS, b"0.63.0");
    }

    // Populate simulated EFI memory map
    {
        let mut mmap = MEMORY_MAP.lock();
        mmap.push(EfiMemoryDescriptor {
            mem_type: EfiMemoryType::RuntimeServicesCode,
            phys_start: 0x0000_0000_FED0_0000,
            virt_start: 0xFFFF_8000_FED0_0000,
            num_pages: 16,
            attribute: 0x8000_0000_0000_000F,
        });
        mmap.push(EfiMemoryDescriptor {
            mem_type: EfiMemoryType::RuntimeServicesData,
            phys_start: 0x0000_0000_FEE0_0000,
            virt_start: 0xFFFF_8000_FEE0_0000,
            num_pages: 32,
            attribute: 0x8000_0000_0000_000F,
        });
        mmap.push(EfiMemoryDescriptor {
            mem_type: EfiMemoryType::ACPIReclaimMemory,
            phys_start: 0x0000_0000_BFEF_0000,
            virt_start: 0xFFFF_8000_BFEF_0000,
            num_pages: 8,
            attribute: 0x8000_0000_0000_000F,
        });
        mmap.push(EfiMemoryDescriptor {
            mem_type: EfiMemoryType::ACPIMemoryNVS,
            phys_start: 0x0000_0000_BFF0_0000,
            virt_start: 0xFFFF_8000_BFF0_0000,
            num_pages: 64,
            attribute: 0x8000_0000_0000_000F,
        });
        mmap.push(EfiMemoryDescriptor {
            mem_type: EfiMemoryType::MemoryMappedIO,
            phys_start: 0x0000_0000_FEC0_0000,
            virt_start: 0xFFFF_8000_FEC0_0000,
            num_pages: 1,
            attribute: 0x8000_0000_0000_0001,
        });
    }

    crate::serial_println!("[uefi-rt] runtime services initialized ({} default variables)", {
        VARIABLE_STORE.lock().variables.len()
    });
}

// ---------------------------------------------------------------------------
// Variable access API
// ---------------------------------------------------------------------------

/// Get a UEFI variable by name (searches EFI_GLOBAL first, then OS namespace).
pub fn get_variable(name: &str) -> Result<Vec<u8>, &'static str> {
    GET_VARIABLE_CALLS.fetch_add(1, Ordering::Relaxed);
    let store = VARIABLE_STORE.lock();
    if let Some(var) = store.get(name, EFI_GLOBAL_VARIABLE) {
        return Ok(var.data.clone());
    }
    if let Some(var) = store.get(name, EFI_IMAGE_SECURITY_DATABASE) {
        return Ok(var.data.clone());
    }
    if let Some(var) = store.get(name, MERLION_OS_VARIABLE) {
        return Ok(var.data.clone());
    }
    Err("variable not found")
}

/// Get a UEFI variable from a specific namespace.
pub fn get_variable_ns(name: &str, namespace: u128) -> Result<Vec<u8>, &'static str> {
    GET_VARIABLE_CALLS.fetch_add(1, Ordering::Relaxed);
    let store = VARIABLE_STORE.lock();
    store.get(name, namespace)
        .map(|v| v.data.clone())
        .ok_or("variable not found")
}

/// Set a UEFI variable (uses EFI_GLOBAL namespace by default).
pub fn set_variable(name: &str, data: &[u8]) -> Result<(), &'static str> {
    SET_VARIABLE_CALLS.fetch_add(1, Ordering::Relaxed);
    let mut store = VARIABLE_STORE.lock();
    store.set(name, MERLION_OS_VARIABLE, BOOT_ATTRS, data)
}

/// Set a UEFI variable with full control over namespace and attributes.
pub fn set_variable_full(name: &str, namespace: u128, attrs: u32, data: &[u8]) -> Result<(), &'static str> {
    SET_VARIABLE_CALLS.fetch_add(1, Ordering::Relaxed);
    let mut store = VARIABLE_STORE.lock();
    store.set(name, namespace, attrs, data)
}

/// Delete a UEFI variable.
pub fn delete_variable(name: &str, namespace: u128) -> bool {
    let mut store = VARIABLE_STORE.lock();
    store.delete(name, namespace)
}

/// List all UEFI variables: (name, namespace, attributes, data_size).
pub fn list_variables() -> Vec<(String, u128, u32, usize)> {
    VARIABLE_STORE.lock().list()
}

/// Get the next variable name (for iteration). Returns (name, namespace).
pub fn get_next_variable_name(after: &str) -> Option<(String, u128)> {
    let store = VARIABLE_STORE.lock();
    let mut found_current = after.is_empty();
    for var in &store.variables {
        if found_current {
            return Some((var.name.clone(), var.namespace));
        }
        if var.name == after {
            found_current = true;
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Namespace helpers
// ---------------------------------------------------------------------------

/// Format a UEFI GUID namespace for display.
pub fn namespace_label(ns: u128) -> &'static str {
    if ns == EFI_GLOBAL_VARIABLE {
        "EFI_GLOBAL_VARIABLE"
    } else if ns == EFI_IMAGE_SECURITY_DATABASE {
        "EFI_IMAGE_SECURITY_DATABASE"
    } else if ns == MERLION_OS_VARIABLE {
        "MERLION_OS_VARIABLE"
    } else {
        "UNKNOWN"
    }
}

fn format_attrs(attrs: u32) -> String {
    let mut parts = Vec::new();
    if attrs & EFI_VARIABLE_NON_VOLATILE != 0 { parts.push("NV"); }
    if attrs & EFI_VARIABLE_BOOTSERVICE_ACCESS != 0 { parts.push("BS"); }
    if attrs & EFI_VARIABLE_RUNTIME_ACCESS != 0 { parts.push("RT"); }
    if attrs & EFI_VARIABLE_TIME_BASED_AUTHENTICATED != 0 { parts.push("AT"); }
    if attrs & EFI_VARIABLE_APPEND_WRITE != 0 { parts.push("AW"); }
    if parts.is_empty() {
        String::from("none")
    } else {
        let mut s = String::new();
        for (i, p) in parts.iter().enumerate() {
            if i > 0 { s.push('+'); }
            s.push_str(p);
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Time services
// ---------------------------------------------------------------------------

/// Get the current UEFI time (wraps the CMOS RTC).
pub fn get_uefi_time() -> EfiTime {
    GET_TIME_CALLS.fetch_add(1, Ordering::Relaxed);
    let rtc = crate::rtc::read();
    EfiTime {
        year: rtc.year,
        month: rtc.month,
        day: rtc.day,
        hour: rtc.hour,
        minute: rtc.minute,
        second: rtc.second,
        nanosecond: 0,
        timezone: 0, // UTC
        daylight: 0,
    }
}

// ---------------------------------------------------------------------------
// Reset system
// ---------------------------------------------------------------------------

/// Request a system reset via UEFI ResetSystem.
pub fn reset_system(reset_type: ResetType) -> ! {
    RESET_CALLS.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[uefi-rt] ResetSystem({})", reset_type.label());
    match reset_type {
        ResetType::Cold | ResetType::Warm | ResetType::PlatformSpecific => {
            crate::power::reboot();
        }
        ResetType::Shutdown => {
            crate::power::shutdown();
        }
    }
}

// ---------------------------------------------------------------------------
// Secure Boot
// ---------------------------------------------------------------------------

/// Get the current Secure Boot mode.
pub fn secure_boot_mode() -> SecureBootMode {
    *SECURE_BOOT_MODE.lock()
}

/// Enable Secure Boot (simulated — transitions from SetupMode to UserMode).
pub fn enable_secure_boot() -> Result<(), &'static str> {
    let mut mode = SECURE_BOOT_MODE.lock();
    if *mode == SecureBootMode::UserMode || *mode == SecureBootMode::DeployedMode {
        return Err("Secure Boot already enabled");
    }
    *mode = SecureBootMode::UserMode;

    let mut store = VARIABLE_STORE.lock();
    let _ = store.set("SecureBoot", EFI_GLOBAL_VARIABLE, BOOT_ATTRS, &[0x01]);
    let _ = store.set("SetupMode", EFI_GLOBAL_VARIABLE, BOOT_ATTRS, &[0x00]);
    // Install a placeholder PK
    let _ = store.set("PK", EFI_IMAGE_SECURITY_DATABASE, BOOT_ATTRS, b"MERLION-PK-PLACEHOLDER");
    Ok(())
}

/// Disable Secure Boot.
pub fn disable_secure_boot() {
    let mut mode = SECURE_BOOT_MODE.lock();
    *mode = SecureBootMode::Disabled;

    let mut store = VARIABLE_STORE.lock();
    let _ = store.set("SecureBoot", EFI_GLOBAL_VARIABLE, BOOT_ATTRS, &[0x00]);
    let _ = store.set("SetupMode", EFI_GLOBAL_VARIABLE, BOOT_ATTRS, &[0x01]);
}

// ---------------------------------------------------------------------------
// Capsule update
// ---------------------------------------------------------------------------

/// Submit a firmware capsule update (simulated).
pub fn submit_capsule(guid: u128, flags: u32, data_size: usize) -> Result<usize, &'static str> {
    let mut capsules = CAPSULES.lock();
    if capsules.len() >= MAX_CAPSULES {
        return Err("capsule queue full");
    }
    let id = capsules.len();
    capsules.push(CapsuleEntry {
        guid,
        flags,
        data_size,
        status: CapsuleStatus::Pending,
    });
    crate::serial_println!("[uefi-rt] capsule #{} submitted ({} bytes)", id, data_size);
    Ok(id)
}

/// Process all pending capsules (simulated — marks them as successful).
pub fn process_capsules() -> usize {
    let mut capsules = CAPSULES.lock();
    let mut processed = 0usize;
    for cap in capsules.iter_mut() {
        if cap.status == CapsuleStatus::Pending {
            cap.status = CapsuleStatus::Processing;
            // Simulate processing
            cap.status = CapsuleStatus::Success;
            processed += 1;
        }
    }
    processed
}

/// Get capsule status by index.
pub fn capsule_status(index: usize) -> Option<CapsuleStatus> {
    CAPSULES.lock().get(index).map(|c| c.status)
}

// ---------------------------------------------------------------------------
// Memory map
// ---------------------------------------------------------------------------

/// Get the UEFI runtime memory map.
pub fn memory_map() -> Vec<EfiMemoryDescriptor> {
    MEMORY_MAP.lock().clone()
}

/// Format the memory map for display.
pub fn format_memory_map() -> String {
    let map = MEMORY_MAP.lock();
    let mut out = format!("UEFI Memory Map ({} entries):\n", map.len());
    for desc in map.iter() {
        out.push_str(&format!(
            "  {:<28} phys={:#014x} virt={:#018x} pages={:<4} attr={:#018x}\n",
            desc.mem_type.label(),
            desc.phys_start,
            desc.virt_start,
            desc.num_pages,
            desc.attribute,
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// System Table
// ---------------------------------------------------------------------------

/// Get the EFI System Table firmware vendor string.
pub fn firmware_vendor() -> &'static str {
    SYSTEM_TABLE.lock().firmware_vendor
}

/// Get the EFI System Table UEFI revision as a string.
pub fn uefi_revision() -> String {
    SYSTEM_TABLE.lock().revision_string()
}

// ---------------------------------------------------------------------------
// Variable browser
// ---------------------------------------------------------------------------

/// Format all variables for display (variable browser).
pub fn browse_variables() -> String {
    let store = VARIABLE_STORE.lock();
    let vars = store.list();
    let mut out = format!("UEFI Variables ({} total, {} bytes):\n", vars.len(), store.total_size());
    out.push_str(&format!("{:<20} {:<30} {:<10} {:<6}\n", "Name", "Namespace", "Attrs", "Size"));
    out.push_str(&format!("{}\n", "-".repeat(70)));
    for (name, ns, attrs, size) in &vars {
        out.push_str(&format!(
            "{:<20} {:<30} {:<10} {} B\n",
            name,
            namespace_label(*ns),
            format_attrs(*attrs),
            size,
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Info / stats
// ---------------------------------------------------------------------------

/// Summary of the UEFI runtime services subsystem.
pub fn uefi_info() -> String {
    let st = SYSTEM_TABLE.lock();
    let store = VARIABLE_STORE.lock();
    let capsules = CAPSULES.lock();
    let mmap = MEMORY_MAP.lock();
    let sb = secure_boot_mode();

    format!(
        "[uefi-rt] firmware: {} rev {:#x} | UEFI {} | variables: {} | \
         memory map: {} entries | capsules: {} | Secure Boot: {}",
        st.firmware_vendor,
        st.firmware_revision,
        st.revision_string(),
        store.variables.len(),
        mmap.len(),
        capsules.len(),
        sb.label(),
    )
}

/// Runtime service call statistics.
pub fn uefi_stats() -> String {
    let get_var = GET_VARIABLE_CALLS.load(Ordering::Relaxed);
    let set_var = SET_VARIABLE_CALLS.load(Ordering::Relaxed);
    let get_time = GET_TIME_CALLS.load(Ordering::Relaxed);
    let resets = RESET_CALLS.load(Ordering::Relaxed);
    let store = VARIABLE_STORE.lock();
    let capsules = CAPSULES.lock();
    let success = capsules.iter().filter(|c| c.status == CapsuleStatus::Success).count();

    format!(
        "[uefi-rt] GetVariable: {} | SetVariable: {} | GetTime: {} | \
         ResetSystem: {} | vars: {}/{} ({} bytes) | capsules: {}/{} ok",
        get_var, set_var, get_time, resets,
        store.variables.len(), MAX_VARIABLES, store.total_size(),
        success, capsules.len(),
    )
}
