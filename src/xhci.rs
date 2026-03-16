/// xHCI (USB 3.0) host controller driver for MerlionOS.
///
/// Discovers an xHCI controller via PCI (class 0C:03:30), maps BAR0 MMIO
/// registers, resets the controller, sets up command and event rings, and
/// enumerates connected USB ports.

use crate::{pci, memory, serial_println, klog_println};
use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

// PCI identification for xHCI
const XHCI_CLASS: u8 = 0x0C;
const XHCI_SUBCLASS: u8 = 0x03;
const XHCI_PROG_IF: u8 = 0x30;

// USB speed constants
pub const USB_SPEED_FULL: u8 = 1;   // 12 Mb/s
pub const USB_SPEED_LOW: u8 = 2;    // 1.5 Mb/s
pub const USB_SPEED_HIGH: u8 = 3;   // 480 Mb/s
pub const USB_SPEED_SUPER: u8 = 4;  // 5 Gb/s

/// Human-readable USB speed name.
pub fn speed_name(speed: u8) -> &'static str {
    match speed {
        USB_SPEED_FULL  => "Full (12 Mb/s)",
        USB_SPEED_LOW   => "Low (1.5 Mb/s)",
        USB_SPEED_HIGH  => "High (480 Mb/s)",
        USB_SPEED_SUPER => "Super (5 Gb/s)",
        _ => "Unknown",
    }
}

// TRB type codes (bits 15:10 of the control field)
pub const TRB_TYPE_NORMAL: u32 = 1;
pub const TRB_TYPE_SETUP_STAGE: u32 = 2;
pub const TRB_TYPE_DATA_STAGE: u32 = 3;
pub const TRB_TYPE_STATUS_STAGE: u32 = 4;
pub const TRB_TYPE_LINK: u32 = 6;
pub const TRB_TYPE_ENABLE_SLOT: u32 = 9;
pub const TRB_TYPE_ADDRESS_DEVICE: u32 = 11;
pub const TRB_TYPE_CMD_COMPLETION: u32 = 33;
pub const TRB_TYPE_PORT_STATUS_CHANGE: u32 = 34;

// Port status/control register bits
pub const PORTSC_CCS: u32 = 1 << 0;        // Current Connect Status
pub const PORTSC_PED: u32 = 1 << 1;        // Port Enabled/Disabled
pub const PORTSC_PR: u32 = 1 << 4;         // Port Reset
pub const PORTSC_PLS_MASK: u32 = 0xF << 5; // Port Link State
pub const PORTSC_PP: u32 = 1 << 9;         // Port Power
pub const PORTSC_SPEED_MASK: u32 = 0xF << 10; // Port Speed
pub const PORTSC_CSC: u32 = 1 << 17;       // Connect Status Change
pub const PORTSC_PRC: u32 = 1 << 21;       // Port Reset Change

// Operational register bits
const USBCMD_RS: u32 = 1 << 0;    // Run/Stop
const USBCMD_HCRST: u32 = 1 << 1; // Host Controller Reset
const USBSTS_CNR: u32 = 1 << 11;  // Controller Not Ready
const USBSTS_HCH: u32 = 1 << 0;   // HCHalted
const CRCR_RCS: u64 = 1 << 0;     // Command Ring Cycle State

const MAX_PORTS: usize = 16;
const CMD_RING_SIZE: usize = 32;
const EVENT_RING_SIZE: usize = 32;

/// xHCI Capability Registers at BAR0 base.
#[repr(C)]
pub struct XhciCapRegs {
    pub caplength: u8, _reserved: u8, pub hci_version: u16,
    pub hcsparams1: u32, pub hcsparams2: u32, pub hcsparams3: u32,
    pub hccparams1: u32, pub dboff: u32, pub rtsoff: u32, pub hccparams2: u32,
}

/// xHCI Operational Registers at BAR0 + caplength.
#[repr(C)]
pub struct XhciOpRegs {
    pub usbcmd: u32, pub usbsts: u32, pub pagesize: u32,
    _reserved1: [u32; 2], pub dnctrl: u32,
    pub crcr_lo: u32, pub crcr_hi: u32,
    _reserved2: [u32; 4],
    pub dcbaap_lo: u32, pub dcbaap_hi: u32, pub config: u32,
}

/// xHCI Runtime Registers (interrupter 0 register set).
#[repr(C)]
pub struct XhciRuntimeRegs {
    pub mfindex: u32, _reserved: [u32; 7],
    pub iman: u32, pub imod: u32, pub erstsz: u32, _reserved2: u32,
    pub erstba_lo: u32, pub erstba_hi: u32,
    pub erdp_lo: u32, pub erdp_hi: u32,
}

/// Port Status and Control Register set (one per port).
#[repr(C)]
pub struct XhciPortRegs {
    pub portsc: u32, pub portpmsc: u32, pub portli: u32, pub porthlpmc: u32,
}

/// Generic Transfer Request Block (16 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Trb {
    pub param_lo: u32, pub param_hi: u32, pub status: u32, pub control: u32,
}

impl Trb {
    pub const fn zero() -> Self { Trb { param_lo: 0, param_hi: 0, status: 0, control: 0 } }
    /// Encode TRB type and cycle bit into the control field.
    pub fn set_type_cycle(&mut self, trb_type: u32, cycle: bool) {
        self.control = (trb_type << 10) | (cycle as u32);
    }
    /// Extract TRB type from the control field.
    pub fn trb_type(&self) -> u32 { (self.control >> 10) & 0x3F }
    /// Extract cycle bit.
    pub fn cycle(&self) -> bool { self.control & 1 != 0 }
}

/// Event Ring Segment Table entry.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ErstEntry {
    pub base_lo: u32, pub base_hi: u32, pub size: u32, _reserved: u32,
}

/// Slot Context (32 bytes) — first entry in a Device Context.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SlotContext {
    /// Route string, speed, context entries.
    pub field1: u32,
    /// Max exit latency, root hub port, num ports.
    pub field2: u32,
    /// Parent hub slot/port, TT think time, interrupter target.
    pub field3: u32,
    /// Device address, slot state.
    pub field4: u32,
    _reserved: [u32; 4],
}

/// Endpoint Context (32 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EndpointContext {
    pub field1: u32, pub field2: u32,
    pub tr_dequeue_lo: u32, pub tr_dequeue_hi: u32,
    pub field5: u32, _reserved: [u32; 3],
}

/// Device Context: slot context + 31 endpoint contexts.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DeviceContext {
    pub slot: SlotContext,
    pub endpoints: [EndpointContext; 31],
}

// Global driver state
static INITIALIZED: AtomicBool = AtomicBool::new(false);

struct XhciState {
    base: usize,
    cap: *mut XhciCapRegs,
    op: *mut XhciOpRegs,
    runtime: *mut XhciRuntimeRegs,
    doorbell_base: usize,
    max_ports: u8,
    max_slots: u8,
    cmd_ring: [Trb; CMD_RING_SIZE],
    cmd_enqueue: usize,
    cmd_cycle: bool,
    event_ring: [Trb; EVENT_RING_SIZE],
    event_dequeue: usize,
    event_cycle: bool,
    erst: ErstEntry,
    connected_ports: [(u8, u8); MAX_PORTS],
    connected_count: usize,
}
unsafe impl Send for XhciState {}
unsafe impl Sync for XhciState {}

static mut STATE: XhciState = XhciState {
    base: 0, cap: core::ptr::null_mut(), op: core::ptr::null_mut(),
    runtime: core::ptr::null_mut(), doorbell_base: 0,
    max_ports: 0, max_slots: 0,
    cmd_ring: [Trb::zero(); CMD_RING_SIZE], cmd_enqueue: 0, cmd_cycle: true,
    event_ring: [Trb::zero(); EVENT_RING_SIZE], event_dequeue: 0, event_cycle: true,
    erst: ErstEntry { base_lo: 0, base_hi: 0, size: 0, _reserved: 0 },
    connected_ports: [(0, 0); MAX_PORTS], connected_count: 0,
};

/// Scan PCI for an xHCI controller (class 0C:03:30), map BAR0 MMIO, reset
/// the controller, set up command ring + event ring, and start the HC.
pub fn init() {
    let devices = pci::scan();
    let dev = match devices.iter().find(|d| {
        d.class == XHCI_CLASS && d.subclass == XHCI_SUBCLASS && d.prog_if == XHCI_PROG_IF
    }) {
        Some(d) => d,
        None => { serial_println!("[xhci] no xHCI controller found"); return; }
    };
    serial_println!("[xhci] found {} ({})", dev.summary(), dev.vendor_name());

    // Read BAR0 for MMIO base address
    let bar0_lo = pci::pci_read32(dev.bus, dev.device, dev.function, 0x10);
    if bar0_lo & 0x1 != 0 {
        serial_println!("[xhci] BAR0 is I/O space, expected MMIO"); return;
    }
    let is_64bit = (bar0_lo >> 1) & 0x3 == 2;
    let mut bar0_phys = (bar0_lo & 0xFFFF_FFF0) as u64;
    if is_64bit {
        let bar0_hi = pci::pci_read32(dev.bus, dev.device, dev.function, 0x14);
        bar0_phys |= (bar0_hi as u64) << 32;
    }
    if bar0_phys == 0 { serial_println!("[xhci] BAR0 is zero"); return; }
    serial_println!("[xhci] BAR0 physical: {:#x}", bar0_phys);

    // Enable PCI bus mastering and memory space
    let cmd_reg = pci::pci_read32(dev.bus, dev.device, dev.function, 0x04);
    let cmd_val = (cmd_reg & 0xFFFF) as u16 | (1 << 2) | (1 << 1);
    let write_val = (cmd_reg & 0xFFFF_0000) | cmd_val as u32;
    unsafe {
        let addr: u32 = 0x8000_0000
            | ((dev.bus as u32) << 16) | ((dev.device as u32) << 11)
            | ((dev.function as u32) << 8) | 0x04;
        x86_64::instructions::port::Port::new(0xCF8).write(addr);
        x86_64::instructions::port::Port::<u32>::new(0xCFC).write(write_val);
    }

    let base = memory::phys_to_virt(x86_64::PhysAddr::new(bar0_phys)).as_u64() as usize;

    unsafe {
        let cap = base as *mut XhciCapRegs;
        let caplength = ptr::read_volatile(&(*cap).caplength) as usize;
        let hci_version = ptr::read_volatile(&(*cap).hci_version);
        let hcsparams1 = ptr::read_volatile(&(*cap).hcsparams1);
        let dboff = ptr::read_volatile(&(*cap).dboff) as usize;
        let rtsoff = ptr::read_volatile(&(*cap).rtsoff) as usize;
        let max_slots = (hcsparams1 & 0xFF) as u8;
        let max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;
        serial_println!("[xhci] version {}.{:#04x}, {} slots, {} ports",
            hci_version >> 8, hci_version & 0xFF, max_slots, max_ports);

        let op = (base + caplength) as *mut XhciOpRegs;
        let runtime = (base + rtsoff) as *mut XhciRuntimeRegs;
        STATE.base = base; STATE.cap = cap; STATE.op = op;
        STATE.runtime = runtime; STATE.doorbell_base = base + dboff;
        STATE.max_ports = max_ports; STATE.max_slots = max_slots;

        // Reset: wait for CNR clear, halt if running, then HCRST
        wait_cnr_clear(op);
        let usbcmd = ptr::read_volatile(&(*op).usbcmd);
        if usbcmd & USBCMD_RS != 0 {
            ptr::write_volatile(&mut (*op).usbcmd, usbcmd & !USBCMD_RS);
            for _ in 0..100_000 {
                if ptr::read_volatile(&(*op).usbsts) & USBSTS_HCH != 0 { break; }
                core::hint::spin_loop();
            }
        }
        ptr::write_volatile(&mut (*op).usbcmd, USBCMD_HCRST);
        for _ in 0..1_000_000 {
            if ptr::read_volatile(&(*op).usbcmd) & USBCMD_HCRST == 0 { break; }
            core::hint::spin_loop();
        }
        wait_cnr_clear(op);
        serial_println!("[xhci] controller reset complete");

        // Configure max device slots (cap at 16)
        let slots = if max_slots > 16 { 16 } else { max_slots };
        ptr::write_volatile(&mut (*op).config, slots as u32);

        // Allocate and program DCBAA (Device Context Base Address Array)
        let dcbaa_frame = match memory::alloc_frame() {
            Some(f) => f,
            None => { serial_println!("[xhci] failed to alloc DCBAA frame"); return; }
        };
        let dcbaa_phys = dcbaa_frame.start_address().as_u64();
        let dcbaa_virt = memory::phys_to_virt(dcbaa_frame.start_address()).as_mut_ptr::<u8>();
        ptr::write_bytes(dcbaa_virt, 0, 4096);
        ptr::write_volatile(&mut (*op).dcbaap_lo, dcbaa_phys as u32);
        ptr::write_volatile(&mut (*op).dcbaap_hi, (dcbaa_phys >> 32) as u32);

        // Set up Command Ring
        STATE.cmd_ring = [Trb::zero(); CMD_RING_SIZE];
        STATE.cmd_enqueue = 0; STATE.cmd_cycle = true;
        let cmd_ring_phys = virt_to_phys(&raw const STATE.cmd_ring as *const _ as usize);
        let crcr_val = cmd_ring_phys | CRCR_RCS;
        ptr::write_volatile(&mut (*op).crcr_lo, crcr_val as u32);
        ptr::write_volatile(&mut (*op).crcr_hi, (crcr_val >> 32) as u32);

        // Set up Event Ring (single segment)
        STATE.event_ring = [Trb::zero(); EVENT_RING_SIZE];
        STATE.event_dequeue = 0; STATE.event_cycle = true;
        let event_ring_phys = virt_to_phys(&raw const STATE.event_ring as *const _ as usize);
        STATE.erst = ErstEntry {
            base_lo: event_ring_phys as u32, base_hi: (event_ring_phys >> 32) as u32,
            size: EVENT_RING_SIZE as u32, _reserved: 0,
        };
        let erst_phys = virt_to_phys(&raw const STATE.erst as *const _ as usize);
        ptr::write_volatile(&mut (*runtime).erstsz, 1);
        ptr::write_volatile(&mut (*runtime).erdp_lo, event_ring_phys as u32);
        ptr::write_volatile(&mut (*runtime).erdp_hi, (event_ring_phys >> 32) as u32);
        ptr::write_volatile(&mut (*runtime).erstba_lo, erst_phys as u32);
        ptr::write_volatile(&mut (*runtime).erstba_hi, (erst_phys >> 32) as u32);

        // Start the controller
        let usbcmd = ptr::read_volatile(&(*op).usbcmd);
        ptr::write_volatile(&mut (*op).usbcmd, usbcmd | USBCMD_RS);
        for _ in 0..100_000 {
            if ptr::read_volatile(&(*op).usbsts) & USBSTS_HCH == 0 { break; }
            core::hint::spin_loop();
        }

        INITIALIZED.store(true, Ordering::SeqCst);
        serial_println!("[xhci] controller running");
        klog_println!("[xhci] initialized, {} ports", max_ports);
        crate::driver::register("xhci", crate::driver::DriverKind::Serial);
        enumerate_ports_inner();
    }
}

/// Wait for the Controller Not Ready bit to clear.
unsafe fn wait_cnr_clear(op: *mut XhciOpRegs) {
    for _ in 0..1_000_000 {
        if ptr::read_volatile(&(*op).usbsts) & USBSTS_CNR == 0 { return; }
        core::hint::spin_loop();
    }
    serial_println!("[xhci] warning: CNR did not clear");
}

/// Convert a kernel virtual address back to physical.
fn virt_to_phys(virt: usize) -> u64 {
    virt as u64 - memory::phys_mem_offset().as_u64()
}

/// Check port status registers for connected devices and record their speed.
pub fn enumerate_ports() {
    if !INITIALIZED.load(Ordering::SeqCst) { return; }
    unsafe { enumerate_ports_inner(); }
}

unsafe fn enumerate_ports_inner() {
    let op_base = STATE.op as usize;
    let max = STATE.max_ports as usize;
    let mut count = 0usize;
    for i in 0..max {
        if i >= MAX_PORTS { break; }
        let port_regs = (op_base + 0x400 + 0x10 * i) as *mut XhciPortRegs;
        let portsc = ptr::read_volatile(&(*port_regs).portsc);
        if portsc & PORTSC_CCS != 0 {
            let speed = ((portsc & PORTSC_SPEED_MASK) >> 10) as u8;
            serial_println!("[xhci]   port {}: connected, speed={} ({})",
                i + 1, speed, speed_name(speed));
            STATE.connected_ports[count] = ((i + 1) as u8, speed);
            count += 1;
        }
    }
    STATE.connected_count = count;
    if count == 0 {
        serial_println!("[xhci] no devices connected");
    } else {
        serial_println!("[xhci] {} port(s) with connected devices", count);
    }
}

/// Post a TRB to the command ring and ring the host controller doorbell.
#[allow(dead_code)]
unsafe fn post_command(trb_type: u32) {
    let idx = STATE.cmd_enqueue;
    let cycle = STATE.cmd_cycle;
    STATE.cmd_ring[idx] = Trb::zero();
    STATE.cmd_ring[idx].set_type_cycle(trb_type, cycle);
    STATE.cmd_enqueue += 1;
    if STATE.cmd_enqueue >= CMD_RING_SIZE - 1 {
        // Link TRB wraps back to ring start; toggle cycle (bit 1)
        let link_phys = virt_to_phys(&raw const STATE.cmd_ring as *const _ as usize);
        STATE.cmd_ring[CMD_RING_SIZE - 1].param_lo = link_phys as u32;
        STATE.cmd_ring[CMD_RING_SIZE - 1].param_hi = (link_phys >> 32) as u32;
        STATE.cmd_ring[CMD_RING_SIZE - 1].status = 0;
        STATE.cmd_ring[CMD_RING_SIZE - 1].control =
            (TRB_TYPE_LINK << 10) | (cycle as u32) | (1 << 1);
        STATE.cmd_cycle = !STATE.cmd_cycle;
        STATE.cmd_enqueue = 0;
    }
    // Ring doorbell 0 (host controller command)
    ptr::write_volatile(STATE.doorbell_base as *mut u32, 0);
}

/// Poll the event ring for a completion event. Returns completion code or None.
#[allow(dead_code)]
unsafe fn poll_event(timeout_iters: u32) -> Option<u32> {
    for _ in 0..timeout_iters {
        let idx = STATE.event_dequeue;
        let trb = ptr::read_volatile(&STATE.event_ring[idx]);
        if trb.cycle() == STATE.event_cycle {
            let completion_code = (trb.status >> 24) & 0xFF;
            STATE.event_dequeue += 1;
            if STATE.event_dequeue >= EVENT_RING_SIZE {
                STATE.event_dequeue = 0;
                STATE.event_cycle = !STATE.event_cycle;
            }
            // Advance ERDP (bit 3 = Event Handler Busy clear)
            let erdp_phys = virt_to_phys(
                &STATE.event_ring[STATE.event_dequeue] as *const _ as usize);
            ptr::write_volatile(&mut (*STATE.runtime).erdp_lo, erdp_phys as u32 | (1 << 3));
            ptr::write_volatile(&mut (*STATE.runtime).erdp_hi, (erdp_phys >> 32) as u32);
            return Some(completion_code);
        }
        core::hint::spin_loop();
    }
    None
}

/// Return whether the xHCI driver has been initialized.
pub fn is_detected() -> bool { INITIALIZED.load(Ordering::SeqCst) }

/// Human-readable xHCI subsystem status.
pub fn info() -> String {
    if !is_detected() { return String::from("xhci: not detected"); }
    unsafe {
        let ver = ptr::read_volatile(&(*STATE.cap).hci_version);
        let ports: Vec<String> = (0..STATE.connected_count)
            .map(|i| {
                let (port, spd) = STATE.connected_ports[i];
                format!("port{}={}", port, speed_name(spd))
            })
            .collect();
        let max_ports = core::ptr::read_volatile(&raw const STATE.max_ports);
        let connected_count = core::ptr::read_volatile(&raw const STATE.connected_count);
        if ports.is_empty() {
            format!("xhci: v{}.{:#04x}, {} ports, no devices",
                ver >> 8, ver & 0xFF, max_ports)
        } else {
            format!("xhci: v{}.{:#04x}, {} ports, {} connected [{}]",
                ver >> 8, ver & 0xFF, max_ports,
                connected_count, ports.join(", "))
        }
    }
}

/// Return a list of (port_number, speed) for connected USB devices.
pub fn connected_devices() -> Vec<(u8, u8)> {
    if !is_detected() { return Vec::new(); }
    unsafe {
        (0..STATE.connected_count).map(|i| STATE.connected_ports[i]).collect()
    }
}
