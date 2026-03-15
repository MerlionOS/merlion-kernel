/// Minimal kernel driver framework.
/// Drivers register themselves at boot and can be listed via the shell.
/// This is a foundation for future extensibility.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::borrow::ToOwned;
use spin::Mutex;

static DRIVERS: Mutex<Vec<DriverInfo>> = Mutex::new(Vec::new());

pub struct DriverInfo {
    pub name: String,
    pub kind: DriverKind,
    pub status: DriverStatus,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum DriverKind {
    Serial,
    Display,
    Timer,
    Keyboard,
    Block,
    Filesystem,
}

#[derive(Debug, Clone, Copy)]
pub enum DriverStatus {
    Active,
    #[allow(dead_code)]
    Inactive,
}

/// Register a driver.
pub fn register(name: &str, kind: DriverKind) {
    let mut drivers = DRIVERS.lock();
    drivers.push(DriverInfo {
        name: name.to_owned(),
        kind,
        status: DriverStatus::Active,
    });
}

/// List all registered drivers.
pub fn list() -> Vec<(String, &'static str, &'static str)> {
    let drivers = DRIVERS.lock();
    drivers
        .iter()
        .map(|d| {
            let kind = match d.kind {
                DriverKind::Serial => "serial",
                DriverKind::Display => "display",
                DriverKind::Timer => "timer",
                DriverKind::Keyboard => "keyboard",
                DriverKind::Block => "block",
                DriverKind::Filesystem => "fs",
            };
            let status = match d.status {
                DriverStatus::Active => "active",
                DriverStatus::Inactive => "inactive",
            };
            (d.name.clone(), kind, status)
        })
        .collect()
}

/// Register all built-in kernel drivers.
pub fn init() {
    register("uart0", DriverKind::Serial);
    register("vga-text", DriverKind::Display);
    register("framebuf", DriverKind::Display);
    register("pit", DriverKind::Timer);
    register("ps2-kbd", DriverKind::Keyboard);
    register("vfs", DriverKind::Filesystem);
    register("ramdisk", DriverKind::Block);
}
