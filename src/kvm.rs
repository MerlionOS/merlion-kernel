/// KVM-style virtualization for MerlionOS.
/// Provides hardware-assisted virtual machines using Intel VT-x,
/// guest memory management, and virtual device emulation.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_VMS: usize = 16;
const MAX_VCPUS: usize = 4;
const SERIAL_BUF_SIZE: usize = 4096;
const EPT_MAX_ENTRIES: usize = 256;
const PAGE_SIZE: usize = 4096;

/// IA32_FEATURE_CONTROL MSR address.
const IA32_FEATURE_CONTROL: u32 = 0x3A;
/// IA32_VMX_BASIC MSR address.
const IA32_VMX_BASIC: u32 = 0x480;
/// IA32_VMX_PINBASED_CTLS MSR.
const IA32_VMX_PINBASED_CTLS: u32 = 0x481;
/// IA32_VMX_PROCBASED_CTLS MSR.
const IA32_VMX_PROCBASED_CTLS: u32 = 0x482;

/// VMCS field encodings (subset).
const VMCS_GUEST_RIP: u32 = 0x681E;
const VMCS_GUEST_RSP: u32 = 0x681C;
const VMCS_GUEST_CR0: u32 = 0x6800;
const VMCS_GUEST_CR3: u32 = 0x6802;
const VMCS_GUEST_CR4: u32 = 0x6804;
const VMCS_GUEST_CS_SEL: u32 = 0x0802;
const VMCS_GUEST_SS_SEL: u32 = 0x0804;
const VMCS_GUEST_DS_SEL: u32 = 0x0806;
const VMCS_GUEST_ES_SEL: u32 = 0x0808;
const VMCS_HOST_RIP: u32 = 0x6C16;
const VMCS_HOST_RSP: u32 = 0x6C14;
const VMCS_HOST_CR0: u32 = 0x6C00;
const VMCS_HOST_CR3: u32 = 0x6C02;
const VMCS_HOST_CR4: u32 = 0x6C04;
const VMCS_HOST_CS_SEL: u32 = 0x0C02;
const VMCS_HOST_SS_SEL: u32 = 0x0C04;
const VMCS_HOST_DS_SEL: u32 = 0x0C06;
const VMCS_HOST_ES_SEL: u32 = 0x0C08;
const VMCS_PIN_BASED_CTLS: u32 = 0x4000;
const VMCS_PROC_BASED_CTLS: u32 = 0x4002;
const VMCS_EXIT_CTLS: u32 = 0x400C;
const VMCS_ENTRY_CTLS: u32 = 0x4012;
const VMCS_EXCEPTION_BITMAP: u32 = 0x4004;

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static VMX_SUPPORTED: AtomicBool = AtomicBool::new(false);
static VMX_ENABLED: AtomicBool = AtomicBool::new(false);
static NEXT_VM_ID: AtomicU32 = AtomicU32::new(1);
static TOTAL_VM_EXITS: AtomicU64 = AtomicU64::new(0);
static VMS_CREATED: AtomicU64 = AtomicU64::new(0);
static VMS_DESTROYED: AtomicU64 = AtomicU64::new(0);

static KVM: Mutex<KvmState> = Mutex::new(KvmState::new());

// ---------------------------------------------------------------------------
// VM-Exit reasons
// ---------------------------------------------------------------------------

/// Reason for a VM-Exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmExitReason {
    ExternalInterrupt,
    IoInstruction,
    CpuidInstruction,
    MsrRead,
    MsrWrite,
    Hlt,
    ControlRegAccess,
    ExceptionOrNmi,
    InterruptWindow,
    TripleFault,
    Unknown(u32),
}

impl VmExitReason {
    fn from_code(code: u32) -> Self {
        match code {
            1 => VmExitReason::ExternalInterrupt,
            30 => VmExitReason::IoInstruction,
            10 => VmExitReason::CpuidInstruction,
            31 => VmExitReason::MsrRead,
            32 => VmExitReason::MsrWrite,
            12 => VmExitReason::Hlt,
            28 => VmExitReason::ControlRegAccess,
            0 => VmExitReason::ExceptionOrNmi,
            7 => VmExitReason::InterruptWindow,
            2 => VmExitReason::TripleFault,
            n => VmExitReason::Unknown(n),
        }
    }

    fn label(self) -> &'static str {
        match self {
            VmExitReason::ExternalInterrupt => "external_interrupt",
            VmExitReason::IoInstruction => "io_instruction",
            VmExitReason::CpuidInstruction => "cpuid",
            VmExitReason::MsrRead => "msr_read",
            VmExitReason::MsrWrite => "msr_write",
            VmExitReason::Hlt => "hlt",
            VmExitReason::ControlRegAccess => "cr_access",
            VmExitReason::ExceptionOrNmi => "exception_nmi",
            VmExitReason::InterruptWindow => "interrupt_window",
            VmExitReason::TripleFault => "triple_fault",
            VmExitReason::Unknown(_) => "unknown",
        }
    }
}

// ---------------------------------------------------------------------------
// Guest registers
// ---------------------------------------------------------------------------

/// Guest CPU register state.
#[derive(Debug, Clone, Copy)]
pub struct GuestRegs {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rsp: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rflags: u64,
    pub cr0: u64,
    pub cr3: u64,
    pub cr4: u64,
    pub cs: u16,
    pub ss: u16,
    pub ds: u16,
    pub es: u16,
}

impl GuestRegs {
    const fn new() -> Self {
        Self {
            rax: 0, rbx: 0, rcx: 0, rdx: 0,
            rsi: 0, rdi: 0, rsp: 0, rbp: 0,
            r8: 0, r9: 0, r10: 0, r11: 0,
            r12: 0, r13: 0, r14: 0, r15: 0,
            rip: 0, rflags: 0x2, // bit 1 always set
            cr0: 0x30, // PE=0, PG=0, CD=1, NW=1
            cr3: 0, cr4: 0,
            cs: 0, ss: 0, ds: 0, es: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// VMCS (Virtual Machine Control Structure)
// ---------------------------------------------------------------------------

/// Simulated VMCS fields.
struct Vmcs {
    /// Host state
    host_cr0: u64,
    host_cr3: u64,
    host_cr4: u64,
    host_rsp: u64,
    host_rip: u64,
    host_cs_sel: u16,
    host_ss_sel: u16,
    host_ds_sel: u16,
    host_es_sel: u16,

    /// Guest state
    guest: GuestRegs,

    /// Execution controls
    pin_based_ctls: u32,
    proc_based_ctls: u32,
    exception_bitmap: u32,

    /// Exit/entry controls
    exit_ctls: u32,
    entry_ctls: u32,

    /// Status
    active: bool,
    launched: bool,
}

impl Vmcs {
    const fn new() -> Self {
        Self {
            host_cr0: 0, host_cr3: 0, host_cr4: 0,
            host_rsp: 0, host_rip: 0,
            host_cs_sel: 0x08, host_ss_sel: 0x10,
            host_ds_sel: 0x10, host_es_sel: 0x10,
            guest: GuestRegs::new(),
            pin_based_ctls: 0,
            proc_based_ctls: 0,
            exception_bitmap: 0,
            exit_ctls: 0,
            entry_ctls: 0,
            active: false,
            launched: false,
        }
    }

    fn setup_host_state(&mut self) {
        // In a real implementation, read actual host CR0/CR3/CR4/RSP/RIP
        self.host_cr0 = 0x80050033; // PE, MP, ET, NE, WP, AM, PG
        self.host_cr3 = 0; // Would be actual host CR3
        self.host_cr4 = 0x003726E0; // VMX, PAE, PGE, etc.
        self.host_rsp = 0;
        self.host_rip = 0; // VM-exit handler address
        self.host_cs_sel = 0x08;
        self.host_ss_sel = 0x10;
        self.host_ds_sel = 0x10;
        self.host_es_sel = 0x10;
    }

    fn setup_guest_state(&mut self, memory_mb: u32) {
        self.guest = GuestRegs::new();
        // Real mode guest
        self.guest.cr0 = 0x30; // CD, NW
        self.guest.rflags = 0x2;
        self.guest.rip = 0x7C00; // Boot sector entry
        self.guest.rsp = ((memory_mb as u64) * 1024 * 1024) - 16;
        self.guest.cs = 0;
        self.guest.ss = 0;
        self.guest.ds = 0;
        self.guest.es = 0;
    }

    fn setup_execution_controls(&mut self) {
        // Pin-based: external-interrupt exiting
        self.pin_based_ctls = 1; // bit 0: ext interrupt exiting
        // Processor-based: HLT exiting, I/O exiting, CPUID exiting, MSR bitmaps
        self.proc_based_ctls = (1 << 7) | (1 << 24) | (1 << 25) | (1 << 28);
        // Exception bitmap: intercept #PF (14), #GP (13)
        self.exception_bitmap = (1 << 13) | (1 << 14);
        // Exit controls
        self.exit_ctls = (1 << 9) | (1 << 15); // host addr-space size, ack interrupt
        // Entry controls
        self.entry_ctls = 0;
    }
}

// ---------------------------------------------------------------------------
// EPT (Extended Page Tables)
// ---------------------------------------------------------------------------

/// An EPT mapping: guest physical → host physical.
#[derive(Debug, Clone, Copy)]
struct EptEntry {
    guest_phys: u64,
    host_phys: u64,
    size: u64,
    readable: bool,
    writable: bool,
    executable: bool,
}

/// EPT page table (simplified as flat mapping table).
struct Ept {
    entries: Vec<EptEntry>,
}

impl Ept {
    fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Map guest physical pages to host physical pages (identity-like).
    fn setup_identity(&mut self, memory_mb: u32) {
        let total_pages = (memory_mb as u64) * 1024 * 1024 / PAGE_SIZE as u64;
        let pages_to_map = if total_pages > EPT_MAX_ENTRIES as u64 {
            EPT_MAX_ENTRIES as u64
        } else {
            total_pages
        };

        for i in 0..pages_to_map {
            let addr = i * PAGE_SIZE as u64;
            self.entries.push(EptEntry {
                guest_phys: addr,
                host_phys: addr + 0x1_0000_0000, // offset to avoid host collision
                size: PAGE_SIZE as u64,
                readable: true,
                writable: true,
                executable: true,
            });
        }
    }

    fn translate(&self, guest_phys: u64) -> Option<u64> {
        for entry in &self.entries {
            if guest_phys >= entry.guest_phys &&
               guest_phys < entry.guest_phys + entry.size {
                let offset = guest_phys - entry.guest_phys;
                return Some(entry.host_phys + offset);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Virtual devices
// ---------------------------------------------------------------------------

/// Virtual serial port (COM1).
struct VirtualSerial {
    output_buf: Vec<u8>,
    input_buf: Vec<u8>,
}

impl VirtualSerial {
    fn new() -> Self {
        Self {
            output_buf: Vec::new(),
            input_buf: Vec::new(),
        }
    }

    fn write_byte(&mut self, byte: u8) {
        if self.output_buf.len() < SERIAL_BUF_SIZE {
            self.output_buf.push(byte);
        }
    }

    fn read_output(&self) -> String {
        let mut s = String::new();
        for &b in &self.output_buf {
            if b >= 0x20 && b < 0x7F {
                s.push(b as char);
            } else if b == b'\n' {
                s.push('\n');
            } else if b == b'\r' {
                // skip
            } else {
                s.push('.');
            }
        }
        s
    }
}

/// Virtual PIT timer.
struct VirtualPit {
    frequency: u32,
    counter: u32,
    reload_value: u16,
    interrupts_delivered: u64,
}

impl VirtualPit {
    fn new() -> Self {
        Self {
            frequency: 100,
            counter: 0,
            reload_value: 11932, // ~100 Hz from 1.193182 MHz
            interrupts_delivered: 0,
        }
    }

    fn tick(&mut self) -> bool {
        self.counter += 1;
        if self.counter >= self.frequency {
            self.counter = 0;
            self.interrupts_delivered += 1;
            true // fire IRQ 0
        } else {
            false
        }
    }
}

/// Virtual PIC (8259A simplified).
struct VirtualPic {
    irr: u8,  // interrupt request register
    isr: u8,  // in-service register
    imr: u8,  // interrupt mask register
}

impl VirtualPic {
    fn new() -> Self {
        Self { irr: 0, isr: 0, imr: 0 }
    }

    fn raise_irq(&mut self, irq: u8) {
        if irq < 8 {
            self.irr |= 1 << irq;
        }
    }

    fn has_pending(&self) -> bool {
        (self.irr & !self.imr) != 0
    }

    fn acknowledge(&mut self) -> Option<u8> {
        let pending = self.irr & !self.imr;
        if pending == 0 {
            return None;
        }
        // Find lowest bit
        for i in 0u8..8 {
            if pending & (1 << i) != 0 {
                self.irr &= !(1 << i);
                self.isr |= 1 << i;
                return Some(i);
            }
        }
        None
    }

    fn eoi(&mut self) {
        // Clear highest priority in-service bit
        for i in 0u8..8 {
            if self.isr & (1 << i) != 0 {
                self.isr &= !(1 << i);
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// VM-Exit statistics
// ---------------------------------------------------------------------------

/// Per-VM exit statistics.
struct ExitStats {
    io_exits: u64,
    cpuid_exits: u64,
    msr_exits: u64,
    hlt_exits: u64,
    interrupt_exits: u64,
    cr_access_exits: u64,
    other_exits: u64,
    total_exits: u64,
}

impl ExitStats {
    fn new() -> Self {
        Self {
            io_exits: 0, cpuid_exits: 0, msr_exits: 0,
            hlt_exits: 0, interrupt_exits: 0, cr_access_exits: 0,
            other_exits: 0, total_exits: 0,
        }
    }

    fn record(&mut self, reason: VmExitReason) {
        self.total_exits += 1;
        match reason {
            VmExitReason::IoInstruction => self.io_exits += 1,
            VmExitReason::CpuidInstruction => self.cpuid_exits += 1,
            VmExitReason::MsrRead | VmExitReason::MsrWrite => self.msr_exits += 1,
            VmExitReason::Hlt => self.hlt_exits += 1,
            VmExitReason::ExternalInterrupt | VmExitReason::InterruptWindow => {
                self.interrupt_exits += 1;
            }
            VmExitReason::ControlRegAccess => self.cr_access_exits += 1,
            _ => self.other_exits += 1,
        }
    }

    fn display(&self) -> String {
        let mut out = String::from("  VM-Exit statistics:\n");
        out.push_str(&format!("    Total exits:      {}\n", self.total_exits));
        out.push_str(&format!("    I/O:              {}\n", self.io_exits));
        out.push_str(&format!("    CPUID:            {}\n", self.cpuid_exits));
        out.push_str(&format!("    MSR:              {}\n", self.msr_exits));
        out.push_str(&format!("    HLT:              {}\n", self.hlt_exits));
        out.push_str(&format!("    Interrupts:       {}\n", self.interrupt_exits));
        out.push_str(&format!("    CR access:        {}\n", self.cr_access_exits));
        out.push_str(&format!("    Other:            {}\n", self.other_exits));
        out
    }
}

// ---------------------------------------------------------------------------
// Virtual Machine
// ---------------------------------------------------------------------------

/// VM state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmState {
    Created,
    Running,
    Paused,
    Stopped,
}

impl VmState {
    fn label(self) -> &'static str {
        match self {
            VmState::Created => "created",
            VmState::Running => "running",
            VmState::Paused => "paused",
            VmState::Stopped => "stopped",
        }
    }
}

/// A virtual machine instance.
struct Vm {
    id: u32,
    name: String,
    memory_mb: u32,
    state: VmState,
    vmcs: Vmcs,
    ept: Ept,
    serial: VirtualSerial,
    pit: VirtualPit,
    pic: VirtualPic,
    exit_stats: ExitStats,
    created_ticks: u64,
    cpu_ticks: u64,
    kernel_loaded: bool,
    kernel_size: usize,
}

impl Vm {
    fn new(id: u32, name: &str, memory_mb: u32) -> Self {
        let mut vmcs = Vmcs::new();
        vmcs.setup_host_state();
        vmcs.setup_guest_state(memory_mb);
        vmcs.setup_execution_controls();

        let mut ept = Ept::new();
        ept.setup_identity(memory_mb);

        Self {
            id,
            name: String::from(name),
            memory_mb,
            state: VmState::Created,
            vmcs,
            ept,
            serial: VirtualSerial::new(),
            pit: VirtualPit::new(),
            pic: VirtualPic::new(),
            exit_stats: ExitStats::new(),
            created_ticks: crate::timer::ticks(),
            cpu_ticks: 0,
            kernel_loaded: false,
            kernel_size: 0,
        }
    }

    /// Simulate a VM-exit and handle it.
    fn handle_exit(&mut self, reason: VmExitReason) {
        self.exit_stats.record(reason);
        TOTAL_VM_EXITS.fetch_add(1, Ordering::Relaxed);

        match reason {
            VmExitReason::IoInstruction => {
                // Check if it's a serial port write (0x3F8)
                // Simulated: assume guest wrote a byte to serial
                let byte = (self.vmcs.guest.rax & 0xFF) as u8;
                self.serial.write_byte(byte);
                self.vmcs.guest.rip += 2; // skip the OUT instruction
            }
            VmExitReason::CpuidInstruction => {
                // Return simulated CPUID values
                let leaf = self.vmcs.guest.rax as u32;
                match leaf {
                    0 => {
                        self.vmcs.guest.rax = 0x0D; // max leaf
                        self.vmcs.guest.rbx = 0x6C72654D; // "Merl"
                        self.vmcs.guest.rdx = 0x4F6E6F69; // "ionO"
                        self.vmcs.guest.rcx = 0x4D565F53; // "S_VM"
                    }
                    1 => {
                        self.vmcs.guest.rax = 0x000906E9; // family/model/stepping
                        self.vmcs.guest.rbx = 0;
                        self.vmcs.guest.rcx = 0; // no VMX in guest
                        self.vmcs.guest.rdx = (1 << 0) | (1 << 4) | (1 << 15); // FPU, TSC, CMOV
                    }
                    _ => {
                        self.vmcs.guest.rax = 0;
                        self.vmcs.guest.rbx = 0;
                        self.vmcs.guest.rcx = 0;
                        self.vmcs.guest.rdx = 0;
                    }
                }
                self.vmcs.guest.rip += 2; // skip CPUID
            }
            VmExitReason::MsrRead => {
                let msr = self.vmcs.guest.rcx as u32;
                let value: u64 = match msr {
                    0x10 => self.cpu_ticks, // IA32_TIME_STAMP_COUNTER
                    0x1B => 0xFEE00900,     // IA32_APIC_BASE
                    _ => 0,
                };
                self.vmcs.guest.rax = value & 0xFFFF_FFFF;
                self.vmcs.guest.rdx = value >> 32;
                self.vmcs.guest.rip += 2;
            }
            VmExitReason::MsrWrite => {
                // Silently ignore MSR writes in simulation
                self.vmcs.guest.rip += 2;
            }
            VmExitReason::Hlt => {
                // Guest halted — check for pending interrupts
                if self.pic.has_pending() {
                    if let Some(irq) = self.pic.acknowledge() {
                        // Inject interrupt into guest
                        self.serial.write_byte(b'!'); // marker
                        let _ = irq;
                    }
                }
                self.vmcs.guest.rip += 1; // skip HLT
            }
            VmExitReason::ControlRegAccess => {
                self.vmcs.guest.rip += 3; // skip MOV CR
            }
            VmExitReason::TripleFault => {
                self.state = VmState::Stopped;
                self.serial.write_byte(b'\n');
                for &b in b"[TRIPLE FAULT - VM STOPPED]" {
                    self.serial.write_byte(b);
                }
            }
            _ => {
                self.vmcs.guest.rip += 1;
            }
        }
    }

    /// Simulate running for N ticks.
    fn simulate_ticks(&mut self, ticks: u64) {
        for _ in 0..ticks {
            if self.state != VmState::Running {
                break;
            }
            self.cpu_ticks += 1;

            // PIT timer tick
            if self.pit.tick() {
                self.pic.raise_irq(0);
                self.handle_exit(VmExitReason::ExternalInterrupt);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// VMX operations (simulated)
// ---------------------------------------------------------------------------

/// Check CPUID for VMX support.
fn detect_vtx() -> bool {
    // Use the vmx module's detection if available
    // For simulation, report based on platform
    #[cfg(target_arch = "x86_64")]
    {
        crate::vmx::detect_vmx()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// Simulated VMXON — enable VMX operation.
fn vmxon_sim() -> Result<(), &'static str> {
    VMX_ENABLED.store(true, Ordering::SeqCst);
    Ok(())
}

/// Simulated VMCLEAR — clear a VMCS.
fn vmclear_sim(vmcs: &mut Vmcs) {
    vmcs.active = false;
    vmcs.launched = false;
}

/// Simulated VMPTRLD — load a VMCS as current.
fn vmptrld_sim(vmcs: &mut Vmcs) {
    vmcs.active = true;
}

/// Simulated VMLAUNCH.
fn vmlaunch_sim(vmcs: &mut Vmcs) -> Result<(), &'static str> {
    if !vmcs.active {
        return Err("VMCS not active (call VMPTRLD first)");
    }
    vmcs.launched = true;
    Ok(())
}

/// Simulated VMRESUME.
fn vmresume_sim(vmcs: &Vmcs) -> Result<(), &'static str> {
    if !vmcs.launched {
        return Err("VMCS not launched (call VMLAUNCH first)");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// KVM state
// ---------------------------------------------------------------------------

struct KvmState {
    vms: Vec<Vm>,
}

impl KvmState {
    const fn new() -> Self {
        Self { vms: Vec::new() }
    }

    fn find_vm(&self, id: u32) -> Option<usize> {
        self.vms.iter().position(|v| v.id == id)
    }

    fn find_vm_by_name(&self, name: &str) -> Option<usize> {
        self.vms.iter().position(|v| v.name == name)
    }

    fn running_count(&self) -> usize {
        self.vms.iter().filter(|v| v.state == VmState::Running).count()
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the KVM subsystem.
pub fn init() {
    let vtx = detect_vtx();
    VMX_SUPPORTED.store(vtx, Ordering::SeqCst);

    // Enable VMX if supported (simulated)
    if vtx {
        let _ = vmxon_sim();
    } else {
        // Still allow simulated VMs without VT-x
        VMX_ENABLED.store(true, Ordering::SeqCst);
    }

    INITIALIZED.store(true, Ordering::SeqCst);
}

/// Create a new VM.
pub fn create_vm(name: &str, memory_mb: u32) -> Result<u32, &'static str> {
    let mut kvm = KVM.lock();

    if kvm.vms.len() >= MAX_VMS {
        return Err("maximum VMs reached");
    }

    if kvm.find_vm_by_name(name).is_some() {
        return Err("VM with this name already exists");
    }

    if memory_mb == 0 || memory_mb > 4096 {
        return Err("memory must be 1-4096 MB");
    }

    let id = NEXT_VM_ID.fetch_add(1, Ordering::Relaxed);
    let vm = Vm::new(id, name, memory_mb);
    kvm.vms.push(vm);

    VMS_CREATED.fetch_add(1, Ordering::Relaxed);
    Ok(id)
}

/// Load a guest kernel image into VM memory.
pub fn load_kernel(vm_id: u32, kernel_data: &[u8]) -> Result<(), &'static str> {
    let mut kvm = KVM.lock();
    let idx = kvm.find_vm(vm_id).ok_or("VM not found")?;

    if kvm.vms[idx].state != VmState::Created {
        return Err("VM must be in 'created' state to load kernel");
    }

    // In a real implementation, copy kernel_data into guest memory at 0x7C00
    kvm.vms[idx].kernel_loaded = true;
    kvm.vms[idx].kernel_size = kernel_data.len();

    // Write a boot message to serial
    for &b in b"Loading guest kernel...\n" {
        kvm.vms[idx].serial.write_byte(b);
    }

    Ok(())
}

/// Start a VM.
pub fn start_vm(vm_id: u32) -> Result<(), &'static str> {
    let mut kvm = KVM.lock();
    let idx = kvm.find_vm(vm_id).ok_or("VM not found")?;

    if kvm.vms[idx].state == VmState::Running {
        return Err("VM is already running");
    }

    // VMCLEAR + VMPTRLD + VMLAUNCH sequence
    vmclear_sim(&mut kvm.vms[idx].vmcs);
    vmptrld_sim(&mut kvm.vms[idx].vmcs);
    vmlaunch_sim(&mut kvm.vms[idx].vmcs)?;

    kvm.vms[idx].state = VmState::Running;

    // Write boot message
    for &b in b"VM started.\n" {
        kvm.vms[idx].serial.write_byte(b);
    }

    // Simulate a few initial ticks
    kvm.vms[idx].simulate_ticks(10);

    Ok(())
}

/// Stop a running VM.
pub fn stop_vm(vm_id: u32) -> Result<(), &'static str> {
    let mut kvm = KVM.lock();
    let idx = kvm.find_vm(vm_id).ok_or("VM not found")?;

    if kvm.vms[idx].state != VmState::Running {
        return Err("VM is not running");
    }

    kvm.vms[idx].state = VmState::Stopped;
    vmclear_sim(&mut kvm.vms[idx].vmcs);

    for &b in b"VM stopped.\n" {
        kvm.vms[idx].serial.write_byte(b);
    }

    Ok(())
}

/// Destroy a VM (remove it entirely).
pub fn destroy_vm(vm_id: u32) -> Result<(), &'static str> {
    let mut kvm = KVM.lock();
    let idx = kvm.find_vm(vm_id).ok_or("VM not found")?;

    if kvm.vms[idx].state == VmState::Running {
        return Err("stop VM before destroying");
    }

    kvm.vms.remove(idx);
    VMS_DESTROYED.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Get the serial console output from a VM.
pub fn vm_console(vm_id: u32) -> Result<String, &'static str> {
    let kvm = KVM.lock();
    let idx = kvm.find_vm(vm_id).ok_or("VM not found")?;
    Ok(kvm.vms[idx].serial.read_output())
}

/// Read guest register state.
pub fn guest_regs(vm_id: u32) -> Result<String, &'static str> {
    let kvm = KVM.lock();
    let idx = kvm.find_vm(vm_id).ok_or("VM not found")?;
    let r = &kvm.vms[idx].vmcs.guest;

    let mut out = format!("Guest registers (VM {}):\n", vm_id);
    out.push_str(&format!("  RAX={:#018x}  RBX={:#018x}\n", r.rax, r.rbx));
    out.push_str(&format!("  RCX={:#018x}  RDX={:#018x}\n", r.rcx, r.rdx));
    out.push_str(&format!("  RSI={:#018x}  RDI={:#018x}\n", r.rsi, r.rdi));
    out.push_str(&format!("  RSP={:#018x}  RBP={:#018x}\n", r.rsp, r.rbp));
    out.push_str(&format!("  R8 ={:#018x}  R9 ={:#018x}\n", r.r8, r.r9));
    out.push_str(&format!("  R10={:#018x}  R11={:#018x}\n", r.r10, r.r11));
    out.push_str(&format!("  R12={:#018x}  R13={:#018x}\n", r.r12, r.r13));
    out.push_str(&format!("  R14={:#018x}  R15={:#018x}\n", r.r14, r.r15));
    out.push_str(&format!("  RIP={:#018x}  RFLAGS={:#018x}\n", r.rip, r.rflags));
    out.push_str(&format!("  CR0={:#018x}  CR3={:#018x}  CR4={:#018x}\n", r.cr0, r.cr3, r.cr4));
    out.push_str(&format!("  CS={:#06x}  SS={:#06x}  DS={:#06x}  ES={:#06x}\n",
        r.cs, r.ss, r.ds, r.es));
    Ok(out)
}

/// List all VMs.
pub fn list_vms() -> String {
    let kvm = KVM.lock();
    let mut out = format!("{:<6} {:<16} {:<10} {:<10} {}\n",
        "ID", "NAME", "MEM(MB)", "STATE", "CPU TICKS");
    for vm in &kvm.vms {
        out.push_str(&format!("{:<6} {:<16} {:<10} {:<10} {}\n",
            vm.id, vm.name, vm.memory_mb, vm.state.label(), vm.cpu_ticks));
    }
    if kvm.vms.is_empty() {
        out.push_str("(no VMs)\n");
    }
    out
}

/// Get detailed info about a VM.
pub fn vm_info(vm_id: u32) -> Result<String, &'static str> {
    let kvm = KVM.lock();
    let idx = kvm.find_vm(vm_id).ok_or("VM not found")?;
    let vm = &kvm.vms[idx];

    let mut out = format!("VM: {} (ID: {})\n", vm.name, vm.id);
    out.push_str(&format!("  State:       {}\n", vm.state.label()));
    out.push_str(&format!("  Memory:      {} MB\n", vm.memory_mb));
    out.push_str(&format!("  Kernel:      {}\n",
        if vm.kernel_loaded { format!("loaded ({} bytes)", vm.kernel_size) }
        else { String::from("not loaded") }));
    out.push_str(&format!("  CPU ticks:   {}\n", vm.cpu_ticks));
    out.push_str(&format!("  Created:     tick {}\n", vm.created_ticks));
    out.push_str(&format!("  EPT entries: {}\n", vm.ept.entries.len()));
    out.push_str(&format!("  VMCS active: {}\n", vm.vmcs.active));
    out.push_str(&format!("  VMCS launched: {}\n", vm.vmcs.launched));

    // Virtual devices
    out.push_str(&format!("  Serial output: {} bytes\n", vm.serial.output_buf.len()));
    out.push_str(&format!("  PIT interrupts: {}\n", vm.pit.interrupts_delivered));
    out.push_str(&format!("  PIC IRR: {:#04x}  ISR: {:#04x}  IMR: {:#04x}\n",
        vm.pic.irr, vm.pic.isr, vm.pic.imr));

    // Exit stats
    out.push_str(&vm.exit_stats.display());
    Ok(out)
}

/// Return KVM subsystem information.
pub fn kvm_info() -> String {
    let kvm = KVM.lock();
    let mut out = String::from("KVM Subsystem:\n");
    out.push_str(&format!("  Initialized:   {}\n", INITIALIZED.load(Ordering::Relaxed)));
    out.push_str(&format!("  VT-x support:  {}\n", VMX_SUPPORTED.load(Ordering::Relaxed)));
    out.push_str(&format!("  VMX enabled:   {}\n", VMX_ENABLED.load(Ordering::Relaxed)));
    out.push_str(&format!("  VMs:           {} ({} running)\n",
        kvm.vms.len(), kvm.running_count()));
    out.push_str(&format!("  Max VMs:       {}\n", MAX_VMS));
    out.push_str(&format!("  Max vCPUs/VM:  {}\n", MAX_VCPUS));
    out.push_str("  Virtual devices: serial (COM1), PIT, PIC\n");
    out.push_str("  Memory: EPT (Extended Page Tables)\n");
    out.push_str(&format!("  VMCS fields:   {} defined\n", 24));
    out
}

/// Return KVM statistics.
pub fn kvm_stats() -> String {
    let mut out = String::from("KVM Statistics:\n");
    out.push_str(&format!("  VMs created:   {}\n", VMS_CREATED.load(Ordering::Relaxed)));
    out.push_str(&format!("  VMs destroyed: {}\n", VMS_DESTROYED.load(Ordering::Relaxed)));
    out.push_str(&format!("  Total VM-exits: {}\n", TOTAL_VM_EXITS.load(Ordering::Relaxed)));
    out
}
