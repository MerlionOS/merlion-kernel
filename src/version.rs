/// MerlionOS version and build information.
/// Centralized version string used by neofetch, uname, /proc/version, etc.

pub const NAME: &str = "MerlionOS";
pub const VERSION: &str = "55.0.0";
pub const CODENAME: &str = "Merlion";
pub const ARCH: &str = "x86_64";
pub const SLOGAN: &str = "Born for AI. Built by AI.";
pub const SLOGAN_CN: &str = "生于AI，成于AI";

pub const MODULES: usize = 217;
pub const COMMANDS: usize = 270;

/// Repository URL.
pub const REPO: &str = "https://github.com/MerlionOS/merlion-kernel";
/// License.
pub const LICENSE: &str = "MIT";

/// Full version string.
pub fn full() -> &'static str {
    concat!("MerlionOS v55.0.0 (", "x86_64", ")")
}

/// One-line banner.
pub fn banner() -> &'static str {
    "MerlionOS v55.0.0 — Born for AI. Built by AI."
}

/// Build info for display.
pub fn build_info() -> &'static str {
    concat!(
        "Language:   Rust (nightly, no_std)\n",
        "Target:     x86_64-unknown-none\n",
        "Bootloader: bootloader 0.9\n",
        "Kernel:     monolithic\n",
        "License:    MIT\n",
    )
}
