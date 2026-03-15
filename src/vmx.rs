/// Intel VT-x (VMX) virtualization foundation for MerlionOS.
///
/// Provides CPUID-based VMX detection, VMX capability MSR reading,
/// VMCS field encoding constants, and wrappers for VMX instructions
/// (VMXON, VMCLEAR, VMPTRLD, VMLAUNCH, VMRESUME). Includes a minimal
/// `setup_vmcs()` that configures guest/host state for a trivial VM.

use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};

// ── CPUID helper ────────────────────────────────────────────────────────────

/// Issue CPUID for `leaf`. Saves/restores RBX (reserved by LLVM).
fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx);
    unsafe {
        asm!(
            "push rbx", "cpuid", "mov {ebx_out:e}, ebx", "pop rbx",
            inout("eax") leaf => eax, ebx_out = out(reg) ebx,
            out("ecx") ecx, out("edx") edx,
        );
    }
    (eax, ebx, ecx, edx)
}

/// Detect VMX support via CPUID leaf 1, ECX bit 5.
pub fn detect_vmx() -> bool {
    let (_, _, ecx, _) = cpuid(1);
    ecx & (1 << 5) != 0
}

// ── VMX capability MSRs ─────────────────────────────────────────────────────

pub const IA32_VMX_BASIC: u32           = 0x480;
pub const IA32_VMX_PINBASED_CTLS: u32   = 0x481;
pub const IA32_VMX_PROCBASED_CTLS: u32  = 0x482;
pub const IA32_VMX_EXIT_CTLS: u32       = 0x483;
pub const IA32_VMX_ENTRY_CTLS: u32      = 0x484;
pub const IA32_VMX_MISC: u32            = 0x485;
pub const IA32_VMX_CR0_FIXED0: u32      = 0x486;
pub const IA32_VMX_CR0_FIXED1: u32      = 0x487;
pub const IA32_VMX_CR4_FIXED0: u32      = 0x488;
pub const IA32_VMX_CR4_FIXED1: u32      = 0x489;
pub const IA32_VMX_PROCBASED_CTLS2: u32 = 0x48B;
pub const IA32_FEATURE_CONTROL: u32     = 0x3A;

/// Read a 64-bit model-specific register.
unsafe fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi, options(nomem, nostack));
    ((hi as u64) << 32) | lo as u64
}

/// Write a 64-bit model-specific register.
#[allow(dead_code)]
unsafe fn wrmsr(msr: u32, val: u64) {
    asm!("wrmsr", in("ecx") msr, in("eax") val as u32,
         in("edx") (val >> 32) as u32, options(nomem, nostack));
}

/// Return the VMCS revision identifier from IA32_VMX_BASIC (bits 30:0).
pub fn vmx_revision_id() -> u32 {
    unsafe { (rdmsr(IA32_VMX_BASIC) & 0x7FFF_FFFF) as u32 }
}

// ── VMCS field encoding constants ───────────────────────────────────────────

// 16-bit guest-state fields
pub const GUEST_ES_SEL: u32 = 0x0800;  pub const GUEST_CS_SEL: u32 = 0x0802;
pub const GUEST_SS_SEL: u32 = 0x0804;  pub const GUEST_DS_SEL: u32 = 0x0806;
pub const GUEST_FS_SEL: u32 = 0x0808;  pub const GUEST_GS_SEL: u32 = 0x080A;
pub const GUEST_LDTR_SEL: u32 = 0x080C; pub const GUEST_TR_SEL: u32 = 0x080E;

// 16-bit host-state fields
pub const HOST_ES_SEL: u32 = 0x0C00;  pub const HOST_CS_SEL: u32 = 0x0C02;
pub const HOST_SS_SEL: u32 = 0x0C04;  pub const HOST_DS_SEL: u32 = 0x0C06;
pub const HOST_FS_SEL: u32 = 0x0C08;  pub const HOST_GS_SEL: u32 = 0x0C0A;
pub const HOST_TR_SEL: u32 = 0x0C0C;

// 64-bit control fields
pub const CTRL_IO_BITMAP_A: u32 = 0x2000;  pub const CTRL_IO_BITMAP_B: u32 = 0x2002;
pub const CTRL_EPT_POINTER: u32 = 0x201A;

// 32-bit control fields
pub const CTRL_PIN_BASED: u32 = 0x4000;   pub const CTRL_PROC_BASED: u32 = 0x4002;
pub const CTRL_EXIT: u32 = 0x400C;        pub const CTRL_ENTRY: u32 = 0x4012;
pub const CTRL_PROC_BASED2: u32 = 0x401E;

// 32-bit guest-state fields
pub const GUEST_ES_LIMIT: u32 = 0x4800;   pub const GUEST_CS_LIMIT: u32 = 0x4802;
pub const GUEST_SS_LIMIT: u32 = 0x4804;   pub const GUEST_DS_LIMIT: u32 = 0x4806;
pub const GUEST_FS_LIMIT: u32 = 0x4808;   pub const GUEST_GS_LIMIT: u32 = 0x480A;
pub const GUEST_LDTR_LIMIT: u32 = 0x480C; pub const GUEST_TR_LIMIT: u32 = 0x480E;
pub const GUEST_GDTR_LIMIT: u32 = 0x4810; pub const GUEST_IDTR_LIMIT: u32 = 0x4812;
pub const GUEST_ES_ACCESS: u32 = 0x4814;  pub const GUEST_CS_ACCESS: u32 = 0x4816;
pub const GUEST_SS_ACCESS: u32 = 0x4818;  pub const GUEST_DS_ACCESS: u32 = 0x481A;
pub const GUEST_INTERRUPTIBILITY: u32 = 0x4824;
pub const GUEST_ACTIVITY: u32 = 0x4826;
pub const VM_ENTRY_INTR_INFO: u32 = 0x4016;

// Natural-width guest-state fields
pub const GUEST_CR0: u32 = 0x6800;  pub const GUEST_CR3: u32 = 0x6802;
pub const GUEST_CR4: u32 = 0x6804;  pub const GUEST_DR7: u32 = 0x681A;
pub const GUEST_RSP: u32 = 0x681C;  pub const GUEST_RIP: u32 = 0x681E;
pub const GUEST_RFLAGS: u32 = 0x6820;
pub const GUEST_ES_BASE: u32 = 0x6806;   pub const GUEST_CS_BASE: u32 = 0x6808;
pub const GUEST_DS_BASE: u32 = 0x680A;   pub const GUEST_SS_BASE: u32 = 0x680C;
pub const GUEST_FS_BASE: u32 = 0x680E;   pub const GUEST_GS_BASE: u32 = 0x6810;
pub const GUEST_LDTR_BASE: u32 = 0x6812; pub const GUEST_TR_BASE: u32 = 0x6814;
pub const GUEST_GDTR_BASE: u32 = 0x6816; pub const GUEST_IDTR_BASE: u32 = 0x6818;

// Natural-width host-state fields
pub const HOST_CR0: u32 = 0x6C00;  pub const HOST_CR3: u32 = 0x6C02;
pub const HOST_CR4: u32 = 0x6C04;  pub const HOST_RSP: u32 = 0x6C14;
pub const HOST_RIP: u32 = 0x6C16;
pub const HOST_FS_BASE: u32 = 0x6C06;   pub const HOST_GS_BASE: u32 = 0x6C08;
pub const HOST_TR_BASE: u32 = 0x6C0A;   pub const HOST_GDTR_BASE: u32 = 0x6C0C;
pub const HOST_IDTR_BASE: u32 = 0x6C0E;

// Read-only data fields
pub const VM_EXIT_REASON: u32 = 0x4402;  pub const VM_EXIT_INSTR_LEN: u32 = 0x440C;
pub const VM_EXIT_QUALIFICATION: u32 = 0x6400;
pub const GUEST_PHYS_ADDR: u32 = 0x2400;

// ── VM-exit reason codes ────────────────────────────────────────────────────

/// Common VM-exit reasons (Intel SDM Vol. 3, Appendix C).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VmExitReason {
    ExternalInterrupt = 1,
    TripleFault       = 2,
    Cpuid             = 10,
    Hlt               = 12,
    Invlpg            = 14,
    Rdtsc             = 16,
    Vmcall            = 18,
    CrAccess          = 28,
    IoInstruction     = 30,
    Rdmsr             = 31,
    Wrmsr             = 32,
    EptViolation      = 48,
    EptMisconfig      = 49,
    Xsetbv            = 55,
}

impl VmExitReason {
    /// Convert a raw exit-reason field (bits 15:0) to a known variant.
    pub fn from_raw(raw: u32) -> Option<Self> {
        match raw & 0xFFFF {
            1  => Some(Self::ExternalInterrupt), 2  => Some(Self::TripleFault),
            10 => Some(Self::Cpuid),  12 => Some(Self::Hlt),
            14 => Some(Self::Invlpg), 16 => Some(Self::Rdtsc),
            18 => Some(Self::Vmcall), 28 => Some(Self::CrAccess),
            30 => Some(Self::IoInstruction), 31 => Some(Self::Rdmsr),
            32 => Some(Self::Wrmsr),  48 => Some(Self::EptViolation),
            49 => Some(Self::EptMisconfig), 55 => Some(Self::Xsetbv),
            _  => None,
        }
    }
}

// ── VMCS region (4 KiB, 4 KiB-aligned) ─────────────────────────────────────

/// 4 KiB-aligned VMCS region. First 4 bytes must hold the revision id.
#[repr(C, align(4096))]
pub struct VmcsRegion {
    pub data: [u8; 4096],
}

impl VmcsRegion {
    /// Create a zeroed VMCS region.
    pub const fn new() -> Self { Self { data: [0u8; 4096] } }

    /// Write the VMCS revision identifier into the first 4 bytes.
    pub fn set_revision_id(&mut self, rev: u32) {
        self.data[0..4].copy_from_slice(&rev.to_le_bytes());
    }

    /// Physical address of this region (assumes identity / offset mapping).
    pub fn phys_addr(&self) -> u64 { self.data.as_ptr() as u64 }
}

static VMX_ACTIVE: AtomicBool = AtomicBool::new(false);

// ── VMX instruction wrappers ────────────────────────────────────────────────

/// Check RFLAGS after a VMX instruction: CF=1 or ZF=1 means failure.
fn vmx_check(rflags: u64, name: &'static str) -> Result<(), &'static str> {
    if rflags & 1 != 0 || rflags & (1 << 6) != 0 { Err(name) } else { Ok(()) }
}

/// Enable VMX operation (VMXON). Caller must set CR4.VMXE first.
///
/// # Safety
/// Ring 0, CR4.VMXE set, IA32_FEATURE_CONTROL configured.
pub unsafe fn vmxon(region: &VmcsRegion) -> Result<(), &'static str> {
    let addr = region.phys_addr();
    let rf: u64;
    asm!("vmxon [{a}]", "pushfq", "pop {rf}",
         a = in(reg) &addr, rf = out(reg) rf, options(nostack));
    vmx_check(rf, "vmxon failed")?;
    VMX_ACTIVE.store(true, Ordering::SeqCst);
    Ok(())
}

/// Clear and deactivate the VMCS (VMCLEAR).
pub unsafe fn vmclear(region: &VmcsRegion) -> Result<(), &'static str> {
    let addr = region.phys_addr();
    let rf: u64;
    asm!("vmclear [{a}]", "pushfq", "pop {rf}",
         a = in(reg) &addr, rf = out(reg) rf, options(nostack));
    vmx_check(rf, "vmclear failed")
}

/// Make `region` the current VMCS on this processor (VMPTRLD).
pub unsafe fn vmptrld(region: &VmcsRegion) -> Result<(), &'static str> {
    let addr = region.phys_addr();
    let rf: u64;
    asm!("vmptrld [{a}]", "pushfq", "pop {rf}",
         a = in(reg) &addr, rf = out(reg) rf, options(nostack));
    vmx_check(rf, "vmptrld failed")
}

/// Write a VMCS field (VMWRITE). Current VMCS must be loaded.
pub unsafe fn vmwrite(field: u32, value: u64) -> Result<(), &'static str> {
    let rf: u64;
    asm!("vmwrite {v}, {f}", "pushfq", "pop {rf}",
         v = in(reg) value, f = in(reg) field as u64,
         rf = out(reg) rf, options(nostack));
    vmx_check(rf, "vmwrite failed")
}

/// Read a VMCS field (VMREAD). Current VMCS must be loaded.
pub unsafe fn vmread(field: u32) -> Result<u64, &'static str> {
    let val: u64; let rf: u64;
    asm!("vmread {v}, {f}", "pushfq", "pop {rf}",
         v = out(reg) val, f = in(reg) field as u64,
         rf = out(reg) rf, options(nostack));
    vmx_check(rf, "vmread failed")?;
    Ok(val)
}

/// Launch a VM from the current VMCS. Returns only on failure.
pub unsafe fn vmlaunch() -> Result<(), &'static str> {
    let rf: u64;
    asm!("vmlaunch", "pushfq", "pop {rf}", rf = out(reg) rf, options(nostack));
    vmx_check(rf, "vmlaunch failed")
}

/// Resume guest execution from the current VMCS. Returns only on failure.
pub unsafe fn vmresume() -> Result<(), &'static str> {
    let rf: u64;
    asm!("vmresume", "pushfq", "pop {rf}", rf = out(reg) rf, options(nostack));
    vmx_check(rf, "vmresume failed")
}

// ── Minimal VMCS setup ─────────────────────────────────────────────────────

/// Configure a minimal VMCS for a flat 64-bit guest.
///
/// Sets guest RIP/RSP to the supplied values and host RIP/RSP for VM-exit
/// re-entry. Reads current CR0/CR3/CR4/GDT/IDT for both host and guest.
/// A production hypervisor would additionally configure EPT, MSR bitmaps,
/// I/O bitmaps, exception bitmaps, etc.
///
/// # Safety
/// A current VMCS must be loaded via `vmptrld`. Addresses must be valid.
pub unsafe fn setup_vmcs(
    guest_rip: u64, guest_rsp: u64,
    host_rip: u64,  host_rsp: u64,
) -> Result<(), &'static str> {
    // Read capability MSRs and compute allowed-1 / required-1 control bits.
    let pin   = rdmsr(IA32_VMX_PINBASED_CTLS);
    let proc  = rdmsr(IA32_VMX_PROCBASED_CTLS);
    let exit  = rdmsr(IA32_VMX_EXIT_CTLS);
    let entry = rdmsr(IA32_VMX_ENTRY_CTLS);

    let pin_ctl  = (pin as u32) & (pin >> 32) as u32;
    let proc_ctl = (proc as u32) & (proc >> 32) as u32;
    let exit_ctl = ((exit as u32) | (1 << 9)) & (exit >> 32) as u32;   // 64-bit host
    let entr_ctl = ((entry as u32) | (1 << 9)) & (entry >> 32) as u32; // IA-32e guest

    vmwrite(CTRL_PIN_BASED, pin_ctl as u64)?;
    vmwrite(CTRL_PROC_BASED, proc_ctl as u64)?;
    vmwrite(CTRL_EXIT, exit_ctl as u64)?;
    vmwrite(CTRL_ENTRY, entr_ctl as u64)?;

    // Current host register state
    let cr0: u64; let cr3: u64; let cr4: u64;
    asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
    asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
    asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));

    // Host state
    vmwrite(HOST_CR0, cr0)?; vmwrite(HOST_CR3, cr3)?; vmwrite(HOST_CR4, cr4)?;
    vmwrite(HOST_RSP, host_rsp)?; vmwrite(HOST_RIP, host_rip)?;
    vmwrite(HOST_CS_SEL, 0x08)?; vmwrite(HOST_SS_SEL, 0x10)?;
    vmwrite(HOST_DS_SEL, 0)?; vmwrite(HOST_ES_SEL, 0)?;
    vmwrite(HOST_FS_SEL, 0)?; vmwrite(HOST_GS_SEL, 0)?;
    vmwrite(HOST_TR_SEL, 0x10)?;

    let mut gdtr = [0u64; 2]; let mut idtr = [0u64; 2];
    asm!("sgdt [{}]", in(reg) gdtr.as_mut_ptr(), options(nostack));
    asm!("sidt [{}]", in(reg) idtr.as_mut_ptr(), options(nostack));
    let gdt_base = gdtr[0] >> 16;
    let idt_base = idtr[0] >> 16;
    vmwrite(HOST_GDTR_BASE, gdt_base)?; vmwrite(HOST_IDTR_BASE, idt_base)?;
    vmwrite(HOST_FS_BASE, 0)?; vmwrite(HOST_GS_BASE, 0)?;
    vmwrite(HOST_TR_BASE, 0)?;

    // Guest state — flat 64-bit, mirrors host CRs
    vmwrite(GUEST_CR0, cr0)?; vmwrite(GUEST_CR3, cr3)?;
    vmwrite(GUEST_CR4, cr4)?; vmwrite(GUEST_DR7, 0x400)?;
    vmwrite(GUEST_RSP, guest_rsp)?; vmwrite(GUEST_RIP, guest_rip)?;
    vmwrite(GUEST_RFLAGS, 0x2)?;

    let code_ar: u64 = 0xA09B; // 64-bit present exec/read
    let data_ar: u64 = 0xC093; // 32-bit gran. present read/write

    // CS
    vmwrite(GUEST_CS_SEL, 0x08)?; vmwrite(GUEST_CS_BASE, 0)?;
    vmwrite(GUEST_CS_LIMIT, 0xFFFF_FFFF)?; vmwrite(GUEST_CS_ACCESS, code_ar)?;
    // SS
    vmwrite(GUEST_SS_SEL, 0x10)?; vmwrite(GUEST_SS_BASE, 0)?;
    vmwrite(GUEST_SS_LIMIT, 0xFFFF_FFFF)?; vmwrite(GUEST_SS_ACCESS, data_ar)?;
    // DS
    vmwrite(GUEST_DS_SEL, 0x10)?; vmwrite(GUEST_DS_BASE, 0)?;
    vmwrite(GUEST_DS_LIMIT, 0xFFFF_FFFF)?; vmwrite(GUEST_DS_ACCESS, data_ar)?;
    // ES
    vmwrite(GUEST_ES_SEL, 0x10)?; vmwrite(GUEST_ES_BASE, 0)?;
    vmwrite(GUEST_ES_LIMIT, 0xFFFF_FFFF)?; vmwrite(GUEST_ES_ACCESS, data_ar)?;
    // FS / GS (null)
    vmwrite(GUEST_FS_SEL, 0)?; vmwrite(GUEST_FS_BASE, 0)?;
    vmwrite(GUEST_FS_LIMIT, 0xFFFF_FFFF)?;
    vmwrite(GUEST_GS_SEL, 0)?; vmwrite(GUEST_GS_BASE, 0)?;
    vmwrite(GUEST_GS_LIMIT, 0xFFFF_FFFF)?;
    // LDTR / TR
    vmwrite(GUEST_LDTR_SEL, 0)?;  vmwrite(GUEST_LDTR_BASE, 0)?;
    vmwrite(GUEST_LDTR_LIMIT, 0)?;
    vmwrite(GUEST_TR_SEL, 0)?;  vmwrite(GUEST_TR_BASE, 0)?;
    vmwrite(GUEST_TR_LIMIT, 0xFF)?;
    // GDTR / IDTR
    vmwrite(GUEST_GDTR_BASE, gdt_base)?; vmwrite(GUEST_GDTR_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_IDTR_BASE, idt_base)?; vmwrite(GUEST_IDTR_LIMIT, 0xFFFF)?;
    // No pending events
    vmwrite(GUEST_INTERRUPTIBILITY, 0)?;
    vmwrite(GUEST_ACTIVITY, 0)?;
    vmwrite(VM_ENTRY_INTR_INFO, 0)?;
    // VMCS link pointer (required, -1 = none)
    vmwrite(0x2800, 0xFFFF_FFFF_FFFF_FFFF)?;
    Ok(())
}

/// Check whether VMX operation is active on this CPU.
pub fn is_vmx_active() -> bool { VMX_ACTIVE.load(Ordering::SeqCst) }

/// Initialize VMX: detect support and log status.
pub fn init() {
    if detect_vmx() {
        crate::serial_println!("[vmx] VMX supported — revision {:#x}", vmx_revision_id());
    } else {
        crate::serial_println!("[vmx] VMX not supported on this CPU");
    }
}
