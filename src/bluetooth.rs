/// Bluetooth protocol stack for MerlionOS.
///
/// Simulated USB Bluetooth adapter driver with HCI, L2CAP, and device
/// discovery/pairing.  No real USB transport — protocol structures mirror
/// real Bluetooth for demos and future hardware bring-up.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ---- HCI (Host Controller Interface) ----

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum HciPacketType { Command = 1, AclData = 2, SyncData = 3, Event = 4 }

#[derive(Debug, Clone)]
pub struct HciCommand { pub opcode: u16, pub params: Vec<u8> }

impl HciCommand {
    pub fn new(opcode: u16, params: Vec<u8>) -> Self { Self { opcode, params } }
    pub fn ogf(&self) -> u8 { (self.opcode >> 10) as u8 }
    pub fn ocf(&self) -> u16 { self.opcode & 0x03FF }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = vec![HciPacketType::Command as u8];
        buf.push((self.opcode & 0xFF) as u8);
        buf.push((self.opcode >> 8) as u8);
        buf.push(self.params.len() as u8);
        buf.extend_from_slice(&self.params);
        buf
    }
}

// Common HCI opcodes (OGF << 10 | OCF)
pub const HCI_RESET: u16             = 0x0C03;
pub const HCI_READ_BD_ADDR: u16      = 0x1009;
pub const HCI_INQUIRY: u16           = 0x0401;
pub const HCI_CREATE_CONNECTION: u16 = 0x0405;
pub const HCI_DISCONNECT: u16        = 0x0406;
pub const HCI_ACCEPT_CONNECTION: u16 = 0x0409;

// HCI event codes
pub const HCI_EVT_COMMAND_COMPLETE: u8    = 0x0E;
pub const HCI_EVT_COMMAND_STATUS: u8      = 0x0F;
pub const HCI_EVT_INQUIRY_RESULT: u8      = 0x02;
pub const HCI_EVT_INQUIRY_COMPLETE: u8    = 0x01;
pub const HCI_EVT_CONNECTION_COMPLETE: u8 = 0x03;
pub const HCI_EVT_DISCONN_COMPLETE: u8    = 0x05;

// ---- L2CAP (Logical Link Control and Adaptation Protocol) ----

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChannelState { Closed, Config, Open, Disconnecting }

#[derive(Debug, Clone)]
struct L2capChannel { cid: u16, psm: u16, mtu: u16, state: ChannelState, remote_addr: [u8; 6] }

pub const L2CAP_PSM_SDP: u16      = 0x0001;
pub const L2CAP_PSM_RFCOMM: u16   = 0x0003;
pub const L2CAP_PSM_HID_CTRL: u16 = 0x0011;
pub const L2CAP_PSM_HID_INTR: u16 = 0x0013;

// ---- Device types and discovery ----

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BtDeviceType { Unknown, Keyboard, Mouse, Audio, Phone, Computer, Other }

impl BtDeviceType {
    fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown", Self::Keyboard => "Keyboard",
            Self::Mouse => "Mouse",     Self::Audio => "Audio",
            Self::Phone => "Phone",     Self::Computer => "Computer",
            Self::Other => "Other",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BtDevice {
    pub addr: [u8; 6],
    pub name: String,
    pub device_class: u32,
    pub rssi: i8,
    pub paired: bool,
    pub connected: bool,
    pub last_seen: u64,
    pub device_type: BtDeviceType,
}

impl BtDevice {
    fn addr_string(&self) -> String {
        format!("{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            self.addr[0], self.addr[1], self.addr[2],
            self.addr[3], self.addr[4], self.addr[5])
    }
}

fn mk_device(addr: [u8; 6], name: &str, class: u32, rssi: i8, dt: BtDeviceType) -> BtDevice {
    BtDevice { addr, name: String::from(name), device_class: class,
        rssi, paired: false, connected: false, last_seen: 0, device_type: dt }
}

// ---- Controller state ----

struct BtController {
    initialized: bool,
    local_addr: [u8; 6],
    local_name: String,
    discoverable: bool,
    scanning: bool,
    devices: Vec<BtDevice>,
    channels: Vec<L2capChannel>,
    next_cid: u16,
}

impl BtController {
    const fn new() -> Self {
        Self { initialized: false, local_addr: [0; 6], local_name: String::new(),
            discoverable: false, scanning: false, devices: Vec::new(),
            channels: Vec::new(), next_cid: 0x0040 }
    }
    fn alloc_cid(&mut self) -> u16 {
        let cid = self.next_cid;
        self.next_cid = self.next_cid.wrapping_add(1).max(0x0040);
        cid
    }
    fn find_device(&self, addr: &[u8; 6]) -> Option<usize> {
        self.devices.iter().position(|d| d.addr == *addr)
    }
}

static BT: Mutex<BtController> = Mutex::new(BtController::new());

static HCI_COMMANDS_SENT: AtomicU64 = AtomicU64::new(0);
static HCI_EVENTS_RECV: AtomicU64   = AtomicU64::new(0);
static ACL_TX_PACKETS: AtomicU64    = AtomicU64::new(0);
static ACL_RX_PACKETS: AtomicU64    = AtomicU64::new(0);
static ACL_TX_BYTES: AtomicU64      = AtomicU64::new(0);

// ---- Helpers ----

fn parse_addr(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 { return None; }
    let mut addr = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        addr[i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(addr)
}

fn hci_send_command(cmd: &HciCommand) {
    let _bytes = cmd.to_bytes();
    HCI_COMMANDS_SENT.fetch_add(1, Ordering::Relaxed);
    // Simulated immediate command-complete event
    HCI_EVENTS_RECV.fetch_add(1, Ordering::Relaxed);
}

// ---- Public API ----

/// Initialize the Bluetooth controller (simulated USB BT adapter detection).
pub fn init() {
    let mut bt = BT.lock();
    if bt.initialized { return; }

    hci_send_command(&HciCommand::new(HCI_RESET, vec![]));
    hci_send_command(&HciCommand::new(HCI_READ_BD_ADDR, vec![]));
    bt.local_addr = [0x00, 0x1A, 0x7D, 0xDA, 0x71, 0x01];
    bt.local_name = String::from("MerlionOS-BT");

    // Seed simulated nearby devices
    bt.devices.push(mk_device([0xAA,0xBB,0xCC,0x01,0x02,0x03], "Keychron K8 Pro",      0x002540, -45, BtDeviceType::Keyboard));
    bt.devices.push(mk_device([0x11,0x22,0x33,0x44,0x55,0x66], "Logitech MX Master 3", 0x002580, -52, BtDeviceType::Mouse));
    bt.devices.push(mk_device([0xDE,0xAD,0xBE,0xEF,0x00,0x01], "AirPods Pro",          0x200404, -38, BtDeviceType::Audio));
    bt.devices.push(mk_device([0xFE,0xED,0xFA,0xCE,0xCA,0xFE], "Pixel 9",              0x5A020C, -61, BtDeviceType::Phone));
    bt.initialized = true;
}

/// Start scanning for nearby Bluetooth devices.
pub fn scan_start() -> &'static str {
    let mut bt = BT.lock();
    if !bt.initialized { return "bluetooth: controller not initialized"; }
    if bt.scanning { return "bluetooth: scan already in progress"; }
    hci_send_command(&HciCommand::new(HCI_INQUIRY, vec![0x33, 0x8B, 0x9E, 0x08, 0x00]));
    bt.scanning = true;
    "bluetooth: scanning for devices..."
}

/// Stop scanning.
pub fn scan_stop() -> &'static str {
    let mut bt = BT.lock();
    if !bt.scanning { return "bluetooth: no scan in progress"; }
    bt.scanning = false;
    "bluetooth: scan stopped"
}

/// List discovered devices.
pub fn list_devices() -> String {
    let bt = BT.lock();
    if !bt.initialized { return String::from("bluetooth: controller not initialized"); }
    if bt.devices.is_empty() { return String::from("bluetooth: no devices found"); }
    let mut out = String::from("Bluetooth devices:\n");
    out.push_str("  ADDR               NAME                     TYPE       RSSI  PAIRED  CONN\n");
    for d in &bt.devices {
        out.push_str(&format!("  {}  {:<24} {:<10} {:>4}  {:<6}  {}\n",
            d.addr_string(), d.name, d.device_type.label(), d.rssi,
            if d.paired { "yes" } else { " no" },
            if d.connected { "yes" } else { " no" }));
    }
    out
}

/// Pair with a device by address string ("AA:BB:CC:DD:EE:FF").
pub fn pair(addr_str: &str) -> String {
    let addr = match parse_addr(addr_str) { Some(a) => a, None => return String::from("bluetooth: invalid address format") };
    let mut bt = BT.lock();
    let idx = match bt.find_device(&addr) { Some(i) => i, None => return String::from("bluetooth: device not found") };
    if bt.devices[idx].paired { return format!("bluetooth: {} already paired", addr_str); }
    bt.devices[idx].paired = true;
    format!("bluetooth: paired with {} ({})", bt.devices[idx].name, addr_str)
}

/// Remove pairing with a device.
pub fn unpair(addr_str: &str) -> String {
    let addr = match parse_addr(addr_str) { Some(a) => a, None => return String::from("bluetooth: invalid address format") };
    let mut bt = BT.lock();
    let idx = match bt.find_device(&addr) { Some(i) => i, None => return String::from("bluetooth: device not found") };
    if !bt.devices[idx].paired { return format!("bluetooth: {} is not paired", addr_str); }
    if bt.devices[idx].connected {
        bt.devices[idx].connected = false;
        bt.channels.retain(|ch| ch.remote_addr != addr);
        hci_send_command(&HciCommand::new(HCI_DISCONNECT, vec![]));
    }
    bt.devices[idx].paired = false;
    format!("bluetooth: unpaired {}", addr_str)
}

/// Connect to a paired device.
pub fn connect(addr_str: &str) -> String {
    let addr = match parse_addr(addr_str) { Some(a) => a, None => return String::from("bluetooth: invalid address format") };
    let mut bt = BT.lock();
    let idx = match bt.find_device(&addr) { Some(i) => i, None => return String::from("bluetooth: device not found") };
    if !bt.devices[idx].paired { return format!("bluetooth: {} not paired — pair first", addr_str); }
    if bt.devices[idx].connected { return format!("bluetooth: {} already connected", addr_str); }
    hci_send_command(&HciCommand::new(HCI_CREATE_CONNECTION, vec![]));
    let cid = bt.alloc_cid();
    bt.channels.push(L2capChannel { cid, psm: L2CAP_PSM_RFCOMM, mtu: 672, state: ChannelState::Open, remote_addr: addr });
    bt.devices[idx].connected = true;
    format!("bluetooth: connected to {} (CID 0x{:04X})", bt.devices[idx].name, cid)
}

/// Disconnect from a device.
pub fn disconnect(addr_str: &str) -> String {
    let addr = match parse_addr(addr_str) { Some(a) => a, None => return String::from("bluetooth: invalid address format") };
    let mut bt = BT.lock();
    let idx = match bt.find_device(&addr) { Some(i) => i, None => return String::from("bluetooth: device not found") };
    if !bt.devices[idx].connected { return format!("bluetooth: {} is not connected", addr_str); }
    hci_send_command(&HciCommand::new(HCI_DISCONNECT, vec![]));
    bt.channels.retain(|ch| ch.remote_addr != addr);
    bt.devices[idx].connected = false;
    format!("bluetooth: disconnected from {}", bt.devices[idx].name)
}

/// Send data to a connected device over L2CAP.
pub fn send_data(addr_str: &str, data: &[u8]) -> String {
    let addr = match parse_addr(addr_str) { Some(a) => a, None => return String::from("bluetooth: invalid address format") };
    let bt = BT.lock();
    let idx = match bt.find_device(&addr) { Some(i) => i, None => return String::from("bluetooth: device not found") };
    if !bt.devices[idx].connected { return format!("bluetooth: {} is not connected", addr_str); }
    let ch = match bt.channels.iter().find(|ch| ch.remote_addr == addr && ch.state == ChannelState::Open) {
        Some(c) => c,
        None => return String::from("bluetooth: no open L2CAP channel"),
    };
    if data.len() > ch.mtu as usize { return format!("bluetooth: payload exceeds MTU ({})", ch.mtu); }
    ACL_TX_PACKETS.fetch_add(1, Ordering::Relaxed);
    ACL_TX_BYTES.fetch_add(data.len() as u64, Ordering::Relaxed);
    format!("bluetooth: sent {} bytes to {} (CID 0x{:04X})", data.len(), bt.devices[idx].name, ch.cid)
}

/// Return controller information.
pub fn bt_info() -> String {
    let bt = BT.lock();
    if !bt.initialized { return String::from("bluetooth: controller not initialized"); }
    let addr = format!("{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        bt.local_addr[0], bt.local_addr[1], bt.local_addr[2],
        bt.local_addr[3], bt.local_addr[4], bt.local_addr[5]);
    let n_p = bt.devices.iter().filter(|d| d.paired).count();
    let n_c = bt.devices.iter().filter(|d| d.connected).count();
    let n_ch = bt.channels.iter().filter(|c| c.state == ChannelState::Open).count();
    format!("Bluetooth Controller\n  Name:         {}\n  Address:      {}\n  Discoverable: {}\n\
             \x20 Scanning:     {}\n  Devices:      {} discovered, {} paired, {} connected\n\
             \x20 L2CAP ch:     {} open",
        bt.local_name, addr, bt.discoverable, bt.scanning, bt.devices.len(), n_p, n_c, n_ch)
}

/// Return Bluetooth stack statistics.
pub fn bt_stats() -> String {
    format!("Bluetooth Statistics\n  HCI commands sent:   {}\n  HCI events received: {}\n\
             \x20 ACL TX packets:      {}\n  ACL RX packets:      {}\n  ACL TX bytes:        {}",
        HCI_COMMANDS_SENT.load(Ordering::Relaxed), HCI_EVENTS_RECV.load(Ordering::Relaxed),
        ACL_TX_PACKETS.load(Ordering::Relaxed), ACL_RX_PACKETS.load(Ordering::Relaxed),
        ACL_TX_BYTES.load(Ordering::Relaxed))
}
