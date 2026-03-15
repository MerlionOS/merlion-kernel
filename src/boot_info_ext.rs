/// Extended boot information display.
/// Shows UEFI/BIOS detection, boot method, and system topology.
/// Foundation for Phase 66 (UEFI boot) and Phase 67 (aarch64 port).

use alloc::string::String;
use alloc::format;

/// Detected boot method.
#[derive(Debug, Clone, Copy)]
pub enum BootMethod {
    BiosLegacy,
    Uefi,
}

/// System architecture.
#[derive(Debug, Clone, Copy)]
pub enum Architecture {
    X86_64,
    #[allow(dead_code)]
    Aarch64,
}

/// Extended boot information.
pub struct BootInfoExt {
    pub method: BootMethod,
    pub arch: Architecture,
    pub bootloader: &'static str,
    pub cmdline: &'static str,
}

/// Get current boot info.
pub fn current() -> BootInfoExt {
    BootInfoExt {
        method: BootMethod::BiosLegacy,
        arch: Architecture::X86_64,
        bootloader: "bootloader 0.9 (bootimage)",
        cmdline: "",
    }
}

/// Format boot info for display.
pub fn format_boot_info() -> String {
    let info = current();
    let method = match info.method {
        BootMethod::BiosLegacy => "BIOS (Legacy)",
        BootMethod::Uefi => "UEFI",
    };
    let arch = match info.arch {
        Architecture::X86_64 => "x86_64",
        Architecture::Aarch64 => "aarch64",
    };
    format!(
        "Boot method:  {}\nArchitecture: {}\nBootloader:   {}\nTarget:       x86_64-unknown-none\nFeatures:     no_std, no_main, abi_x86_interrupt\n",
        method, arch, info.bootloader
    )
}
