/// Symmetric Multiprocessing (SMP) groundwork.
/// Detects the number of CPU cores via CPUID and provides
/// per-CPU state tracking. AP (Application Processor) startup
/// is stubbed out — actual AP boot requires real-mode trampoline
/// code which is a future milestone.

use core::sync::atomic::{AtomicU32, AtomicBool, Ordering};
use spin::Mutex;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;

/// Maximum supported CPUs.
const MAX_CPUS: usize = 16;

/// Number of online CPUs (BSP starts as 1).
static CPU_COUNT: AtomicU32 = AtomicU32::new(1);

/// Per-CPU state.
static CPU_STATE: Mutex<[CpuInfo; MAX_CPUS]> = Mutex::new([const { CpuInfo::new() }; MAX_CPUS]);

/// BSP (bootstrap processor) is initialized.
static BSP_INIT: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy)]
pub struct CpuInfo {
    pub online: bool,
    pub apic_id: u8,
    pub ticks: u64,
}

impl CpuInfo {
    const fn new() -> Self {
        Self {
            online: false,
            apic_id: 0,
            ticks: 0,
        }
    }
}

/// CPU feature information from CPUID.
pub struct CpuFeatures {
    pub vendor: [u8; 12],
    pub brand: String,
    pub family: u8,
    pub model: u8,
    pub stepping: u8,
    pub max_cpuid: u32,
    pub has_apic: bool,
    pub has_x2apic: bool,
    pub has_sse: bool,
    pub has_sse2: bool,
    pub has_avx: bool,
    pub logical_cores: u8,
}

/// Read CPUID leaf. Saves/restores rbx since LLVM reserves it.
fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            inout("eax") leaf => eax,
            ebx_out = out(reg) ebx,
            out("ecx") ecx,
            out("edx") edx,
        );
    }
    (eax, ebx, ecx, edx)
}

/// Read the local APIC ID from CPUID leaf 1.
pub fn apic_id() -> u8 {
    let (_, ebx, _, _) = cpuid(1);
    ((ebx >> 24) & 0xFF) as u8
}

/// Detect CPU features via CPUID.
pub fn detect_features() -> CpuFeatures {
    let (max_cpuid, ebx, ecx, edx) = cpuid(0);

    // Vendor string from EBX, EDX, ECX (in that order)
    let mut vendor = [0u8; 12];
    vendor[0..4].copy_from_slice(&ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&edx.to_le_bytes());
    vendor[8..12].copy_from_slice(&ecx.to_le_bytes());

    // Feature flags from leaf 1
    let (eax1, ebx1, ecx1, edx1) = if max_cpuid >= 1 {
        cpuid(1)
    } else {
        (0, 0, 0, 0)
    };

    let family = ((eax1 >> 8) & 0xF) as u8;
    let model = ((eax1 >> 4) & 0xF) as u8;
    let stepping = (eax1 & 0xF) as u8;
    let logical_cores = ((ebx1 >> 16) & 0xFF) as u8;

    // Brand string from leaves 0x80000002-0x80000004
    let (max_ext, _, _, _) = cpuid(0x80000000);
    let brand = if max_ext >= 0x80000004 {
        let mut brand_bytes = [0u8; 48];
        for i in 0..3u32 {
            let (a, b, c, d) = cpuid(0x80000002 + i);
            let off = (i as usize) * 16;
            brand_bytes[off..off+4].copy_from_slice(&a.to_le_bytes());
            brand_bytes[off+4..off+8].copy_from_slice(&b.to_le_bytes());
            brand_bytes[off+8..off+12].copy_from_slice(&c.to_le_bytes());
            brand_bytes[off+12..off+16].copy_from_slice(&d.to_le_bytes());
        }
        let s = core::str::from_utf8(&brand_bytes).unwrap_or("").trim_end_matches('\0').trim();
        String::from(s)
    } else {
        String::from("Unknown")
    };

    CpuFeatures {
        vendor,
        brand,
        family,
        model,
        stepping,
        max_cpuid,
        has_apic: edx1 & (1 << 9) != 0,
        has_x2apic: ecx1 & (1 << 21) != 0,
        has_sse: edx1 & (1 << 25) != 0,
        has_sse2: edx1 & (1 << 26) != 0,
        has_avx: ecx1 & (1 << 28) != 0,
        logical_cores: if logical_cores == 0 { 1 } else { logical_cores },
    }
}

/// Initialize SMP: detect BSP, record its state.
pub fn init() {
    let id = apic_id();
    let features = detect_features();

    let mut cpus = CPU_STATE.lock();
    cpus[0] = CpuInfo {
        online: true,
        apic_id: id,
        ticks: 0,
    };

    CPU_COUNT.store(1, Ordering::SeqCst);
    BSP_INIT.store(true, Ordering::SeqCst);

    crate::serial_println!("[smp] BSP APIC ID: {}", id);
    crate::serial_println!("[smp] CPU: {}", features.brand);
    crate::serial_println!("[smp] Logical cores: {}", features.logical_cores);
    crate::klog_println!("[smp] BSP online, {} logical cores reported", features.logical_cores);
}

/// Get the number of online CPUs.
pub fn online_cpus() -> u32 {
    CPU_COUNT.load(Ordering::SeqCst)
}

/// Get info about all CPUs.
pub fn cpu_list() -> Vec<(usize, CpuInfo)> {
    let cpus = CPU_STATE.lock();
    cpus.iter()
        .enumerate()
        .filter(|(_, c)| c.online)
        .map(|(i, c)| (i, *c))
        .collect()
}

/// Display CPU information.
pub fn cpu_info_string() -> String {
    let features = detect_features();
    let vendor = core::str::from_utf8(&features.vendor).unwrap_or("?");

    format!(
        "CPU: {}\nVendor: {}\nFamily: {} Model: {} Stepping: {}\nLogical cores: {}\nAPIC: {} x2APIC: {}\nSSE: {} SSE2: {} AVX: {}",
        features.brand, vendor,
        features.family, features.model, features.stepping,
        features.logical_cores,
        if features.has_apic { "yes" } else { "no" },
        if features.has_x2apic { "yes" } else { "no" },
        if features.has_sse { "yes" } else { "no" },
        if features.has_sse2 { "yes" } else { "no" },
        if features.has_avx { "yes" } else { "no" },
    )
}
