/// Sysfs (/sys) virtual filesystem and device model for MerlionOS.
/// Provides a hierarchical view of the kernel's device tree, bus topology,
/// and driver bindings through virtual files.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::borrow::ToOwned;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ── Device model types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType { PCI, USB, Platform, Virtual, Block, Network, Input, Display, Audio }

impl DeviceType {
    pub fn label(self) -> &'static str {
        match self {
            Self::PCI => "pci", Self::USB => "usb", Self::Platform => "platform",
            Self::Virtual => "virtual", Self::Block => "block", Self::Network => "network",
            Self::Input => "input", Self::Display => "display", Self::Audio => "audio",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusType { PCI, USB, Platform, Virtual, ISA }

impl BusType {
    pub fn label(self) -> &'static str {
        match self {
            Self::PCI => "pci", Self::USB => "usb", Self::Platform => "platform",
            Self::Virtual => "virtual", Self::ISA => "isa",
        }
    }
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "pci" => Some(Self::PCI), "usb" => Some(Self::USB),
            "platform" => Some(Self::Platform), "virtual" => Some(Self::Virtual),
            "isa" => Some(Self::ISA), _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState { D0Active, D1Sleep, D2Sleep, D3Off, Unknown }

impl PowerState {
    pub fn label(self) -> &'static str {
        match self {
            Self::D0Active => "D0 (active)", Self::D1Sleep => "D1 (sleep)",
            Self::D2Sleep => "D2 (sleep)", Self::D3Off => "D3 (off)", Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Device {
    pub id: u32,
    pub name: String,
    pub device_type: DeviceType,
    pub bus: BusType,
    pub driver: Option<String>,
    pub vendor_id: u16,
    pub device_id: u16,
    pub parent: Option<u32>,
    pub children: Vec<u32>,
    pub properties: Vec<(String, String)>,
    pub power_state: PowerState,
}

// ── Global state ────────────────────────────────────────────────────────────

static DEVICES: Mutex<Vec<Device>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU32 = AtomicU32::new(1);
static SYSCTL: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

// ── Device operations ───────────────────────────────────────────────────────

/// Register a device and return its assigned id.
pub fn register_device(mut dev: Device) -> u32 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    dev.id = id;
    let mut devs = DEVICES.lock();
    if let Some(pid) = dev.parent {
        if let Some(p) = devs.iter_mut().find(|d| d.id == pid) { p.children.push(id); }
    }
    devs.push(dev);
    id
}

/// Remove a device by id.
pub fn unregister_device(id: u32) {
    let mut devs = DEVICES.lock();
    if let Some(d) = devs.iter().find(|d| d.id == id) {
        let pid = d.parent;
        if let Some(pid) = pid {
            if let Some(p) = devs.iter_mut().find(|d| d.id == pid) {
                p.children.retain(|&c| c != id);
            }
        }
    }
    devs.retain(|d| d.id != id);
}

/// Find a device by name.
pub fn find_device(name: &str) -> Option<Device> {
    DEVICES.lock().iter().find(|d| d.name == name).cloned()
}

/// Find all devices on a given bus type.
pub fn find_by_bus(bus: BusType) -> Vec<Device> {
    DEVICES.lock().iter().filter(|d| d.bus == bus).cloned().collect()
}

/// Bind a driver name to a device.
pub fn bind_driver(device_id: u32, driver_name: &str) {
    if let Some(d) = DEVICES.lock().iter_mut().find(|d| d.id == device_id) {
        d.driver = Some(driver_name.to_owned());
    }
}

/// Unbind the current driver from a device.
pub fn unbind_driver(device_id: u32) {
    if let Some(d) = DEVICES.lock().iter_mut().find(|d| d.id == device_id) {
        d.driver = None;
    }
}

/// Change the power state of a device.
pub fn set_power_state(device_id: u32, state: PowerState) {
    if let Some(d) = DEVICES.lock().iter_mut().find(|d| d.id == device_id) {
        d.power_state = state;
    }
}

/// Return a human-readable device info string.
pub fn device_info(id: u32) -> String {
    let devs = DEVICES.lock();
    match devs.iter().find(|d| d.id == id) {
        Some(d) => {
            let mut s = format!("Device #{}: {}\n  Type: {}  Bus: {}  Vendor: 0x{:04x}  DevID: 0x{:04x}\n",
                d.id, d.name, d.device_type.label(), d.bus.label(), d.vendor_id, d.device_id);
            s.push_str(&format!("  Driver: {}  Power: {}\n",
                d.driver.as_deref().unwrap_or("(none)"), d.power_state.label()));
            if let Some(pid) = d.parent { s.push_str(&format!("  Parent: #{}\n", pid)); }
            if !d.children.is_empty() {
                let k: Vec<String> = d.children.iter().map(|c| format!("#{}", c)).collect();
                s.push_str(&format!("  Children: {}\n", k.join(", ")));
            }
            for (k, v) in &d.properties { s.push_str(&format!("  {}={}\n", k, v)); }
            s
        }
        None => format!("Device #{} not found\n", id),
    }
}

/// Produce a hierarchical device tree display.
pub fn device_tree() -> String {
    let devs = DEVICES.lock();
    let mut out = String::from("Device Tree\n");
    for root in devs.iter().filter(|d| d.parent.is_none()) {
        tree_recurse(&devs, root, &mut out, 0);
    }
    out
}

fn tree_recurse(devs: &[Device], dev: &Device, out: &mut String, depth: usize) {
    for _ in 0..depth { out.push_str("  "); }
    let drv = dev.driver.as_deref().unwrap_or("");
    out.push_str(&format!("|- {} [{}] ({}){}\n", dev.name, dev.device_type.label(),
        dev.bus.label(), if drv.is_empty() { String::new() } else { format!(" driver={}", drv) }));
    for &cid in &dev.children {
        if let Some(c) = devs.iter().find(|d| d.id == cid) { tree_recurse(devs, c, out, depth + 1); }
    }
}

// ── Sysctl (kernel parameters) ─────────────────────────────────────────────

fn sysctl_init() {
    let mut sc = SYSCTL.lock();
    if !sc.is_empty() { return; }
    for (k, v) in [("hostname","merlion"),("osrelease","0.1.0"),
                    ("version","MerlionOS 0.1.0 (x86_64)"),("domainname","(none)")] {
        sc.push((k.to_owned(), v.to_owned()));
    }
}

pub fn sysctl_read(name: &str) -> Option<String> {
    SYSCTL.lock().iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
}

pub fn sysctl_write(name: &str, value: &str) {
    let mut sc = SYSCTL.lock();
    if let Some(e) = sc.iter_mut().find(|(k, _)| k == name) { e.1 = value.to_owned(); }
    else { sc.push((name.to_owned(), value.to_owned())); }
}

// ── Sysfs read / write / list ───────────────────────────────────────────────

fn class_dtype(name: &str) -> Option<DeviceType> {
    match name { "net" => Some(DeviceType::Network), "block" => Some(DeviceType::Block),
                 "input" => Some(DeviceType::Input), _ => None }
}

/// Read a sysfs attribute by path.
pub fn sysfs_read(path: &str) -> Option<String> {
    let p = path.trim_start_matches("/sys/").trim_end_matches('/');
    if let Some(param) = p.strip_prefix("kernel/") { return sysctl_read(param); }
    if p == "power/state" { return Some(power_info()); }
    if let Some(id_s) = p.strip_prefix("devices/") {
        if let Ok(id) = id_s.parse::<u32>() {
            if DEVICES.lock().iter().any(|d| d.id == id) { return Some(device_info(id)); }
        }
        return None;
    }
    if let Some(rest) = p.strip_prefix("bus/") {
        let parts: Vec<&str> = rest.splitn(3, '/').collect();
        if parts.len() >= 2 && parts[1] == "devices" {
            if let Some(bt) = BusType::from_str(parts[0]) {
                if parts.len() == 3 {
                    if let Ok(id) = parts[2].parse::<u32>() {
                        if DEVICES.lock().iter().any(|d| d.id == id && d.bus == bt) {
                            return Some(device_info(id));
                        }
                    }
                    return None;
                }
                let mut s = String::new();
                for d in &find_by_bus(bt) {
                    s.push_str(&format!("{} (0x{:04x}:0x{:04x})\n", d.name, d.vendor_id, d.device_id));
                }
                return Some(s);
            }
        }
        return None;
    }
    if let Some(cls) = p.strip_prefix("class/") {
        if let Some(dt) = class_dtype(cls.trim_end_matches('/')) {
            let devs = DEVICES.lock();
            let mut s = String::new();
            for d in devs.iter().filter(|d| d.device_type == dt) { s.push_str(&format!("{}\n", d.name)); }
            return Some(s);
        }
        return None;
    }
    None
}

/// Write to a writable sysfs attribute.
pub fn sysfs_write(path: &str, value: &str) -> Result<(), &'static str> {
    let p = path.trim_start_matches("/sys/").trim_end_matches('/');
    if let Some(param) = p.strip_prefix("kernel/") { sysctl_write(param, value); return Ok(()); }
    if p == "power/state" {
        return match value {
            "suspend" => { suspend(); Ok(()) }
            "resume"  => { resume(); Ok(()) }
            _ => Err("invalid power state: use 'suspend' or 'resume'"),
        };
    }
    Err("read-only or unknown sysfs path")
}

/// List entries at a sysfs directory path.
pub fn sysfs_list(path: &str) -> Vec<String> {
    let p = path.trim_start_matches("/sys/").trim_end_matches('/');
    if p.is_empty() || p == "sys" {
        return vec!["devices","bus","class","kernel","power"].into_iter().map(|s| s.to_owned()).collect();
    }
    if p == "devices" { return DEVICES.lock().iter().map(|d| format!("{}", d.id)).collect(); }
    if p == "bus" {
        return vec!["pci","usb","platform","virtual","isa"].into_iter().map(|s| s.to_owned()).collect();
    }
    if let Some(rest) = p.strip_prefix("bus/") {
        let parts: Vec<&str> = rest.splitn(2, '/').collect();
        if parts.len() == 1 { return vec!["devices".to_owned()]; }
        if parts[1] == "devices" {
            if let Some(bt) = BusType::from_str(parts[0]) {
                return DEVICES.lock().iter().filter(|d| d.bus == bt).map(|d| format!("{}", d.id)).collect();
            }
        }
    }
    if p == "class" {
        return vec!["net","block","input"].into_iter().map(|s| s.to_owned()).collect();
    }
    if let Some(cls) = p.strip_prefix("class/") {
        if let Some(dt) = class_dtype(cls) {
            return DEVICES.lock().iter().filter(|d| d.device_type == dt).map(|d| d.name.clone()).collect();
        }
    }
    if p == "kernel" { return SYSCTL.lock().iter().map(|(k,_)| k.clone()).collect(); }
    if p == "power" { return vec!["state".to_owned()]; }
    Vec::new()
}

// ── Power management ────────────────────────────────────────────────────────

/// Suspend all devices (transition to D1Sleep).
pub fn suspend() -> String {
    let mut devs = DEVICES.lock();
    let mut n: u32 = 0;
    for d in devs.iter_mut().filter(|d| d.power_state == PowerState::D0Active) {
        d.power_state = PowerState::D1Sleep; n += 1;
    }
    format!("Suspended {} device(s)\n", n)
}

/// Resume all devices (transition to D0Active).
pub fn resume() -> String {
    let mut devs = DEVICES.lock();
    let mut n: u32 = 0;
    for d in devs.iter_mut().filter(|d| d.power_state != PowerState::D0Active) {
        d.power_state = PowerState::D0Active; n += 1;
    }
    format!("Resumed {} device(s)\n", n)
}

/// Power state summary of all devices.
pub fn power_info() -> String {
    let devs = DEVICES.lock();
    let mut s = String::from("Power States\n");
    if devs.is_empty() { s.push_str("  (no devices registered)\n"); return s; }
    for d in devs.iter() { s.push_str(&format!("  {:<20} {}\n", d.name, d.power_state.label())); }
    s
}

// ── Statistics and info ─────────────────────────────────────────────────────

/// Summary statistics of the device model.
pub fn device_stats() -> String {
    let devs = DEVICES.lock();
    let mut s = String::from("Sysfs Device Statistics\n");
    s.push_str(&format!("  Total devices:   {}\n", devs.len()));
    for bt in [BusType::PCI, BusType::USB, BusType::Platform, BusType::Virtual] {
        s.push_str(&format!("  {:<16} {}\n", format!("{}:", bt.label()),
            devs.iter().filter(|d| d.bus == bt).count()));
    }
    s.push_str(&format!("  Driver bound:    {}\n", devs.iter().filter(|d| d.driver.is_some()).count()));
    s.push_str(&format!("  Active (D0):     {}\n",
        devs.iter().filter(|d| d.power_state == PowerState::D0Active).count()));
    s
}

/// Overall sysfs information.
pub fn sysfs_info() -> String {
    let devs = DEVICES.lock();
    let sc = SYSCTL.lock();
    let mut s = String::from("Sysfs (/sys) Virtual Filesystem\n");
    s.push_str(&format!("  Registered devices: {}\n  Kernel parameters:  {}\n", devs.len(), sc.len()));
    s.push_str("  Paths:\n    /sys/devices/   /sys/bus/   /sys/class/\n");
    s.push_str("    /sys/kernel/    /sys/power/\n");
    s
}

// ── Initialization ──────────────────────────────────────────────────────────

fn dev(name: &str, dtype: DeviceType, bus: BusType, driver: Option<&str>,
       vid: u16, did: u16, parent: Option<u32>, props: &[(&str, &str)]) -> Device {
    Device {
        id: 0, name: name.to_owned(), device_type: dtype, bus,
        driver: driver.map(|s| s.to_owned()), vendor_id: vid, device_id: did, parent,
        children: Vec::new(),
        properties: props.iter().map(|(k,v)| ((*k).to_owned(),(*v).to_owned())).collect(),
        power_state: PowerState::D0Active,
    }
}

/// Initialize sysfs with default kernel parameters and known devices.
pub fn init() {
    sysctl_init();
    let root = register_device(dev("platform", DeviceType::Platform, BusType::Platform,
        None, 0, 0, None, &[("description","platform root bus")]));
    let pci = register_device(dev("pci0000:00", DeviceType::PCI, BusType::PCI,
        Some("pcieport"), 0x8086, 0x29c0, Some(root), &[("class","0x060000")]));
    register_device(dev("vga0", DeviceType::Display, BusType::PCI,
        Some("bochs-drm"), 0x1234, 0x1111, Some(pci), &[("class","0x030000")]));
    register_device(dev("eth0", DeviceType::Network, BusType::PCI,
        Some("virtio-net"), 0x1af4, 0x1000, Some(pci), &[("class","0x020000")]));
    register_device(dev("vda", DeviceType::Block, BusType::PCI,
        Some("virtio-blk"), 0x1af4, 0x1001, Some(pci), &[("class","0x010000")]));
    register_device(dev("ps2-keyboard", DeviceType::Input, BusType::ISA,
        Some("i8042"), 0, 0, Some(root), &[("irq","1")]));
    register_device(dev("serial0", DeviceType::Platform, BusType::ISA,
        Some("16550A"), 0, 0, Some(root), &[("iobase","0x3F8"),("irq","4")]));
    register_device(dev("pit-timer", DeviceType::Virtual, BusType::Platform,
        Some("pit"), 0, 0, Some(root), &[("frequency","100")]));
    let usb = register_device(dev("usb0", DeviceType::USB, BusType::PCI,
        Some("xhci_hcd"), 0x8086, 0xa12f, Some(pci), &[("class","0x0c0330")]));
    register_device(dev("usb-hub0", DeviceType::USB, BusType::USB,
        Some("hub"), 0x1d6b, 0x0003, Some(usb), &[("speed","5000"),("ports","4")]));
    register_device(dev("audio0", DeviceType::Audio, BusType::PCI,
        Some("intel-hda"), 0x8086, 0x2668, Some(pci), &[("class","0x040300")]));
}
