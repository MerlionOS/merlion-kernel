/// RISC-V 64-bit (RV64GC) architecture support for MerlionOS.
/// Provides boot code, trap handling, PLIC interrupt controller,
/// CLINT timer, and SBI (Supervisor Binary Interface) calls.
/// Targets: QEMU virt machine, SiFive boards, StarFive VisionFive 2.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use alloc::string::String;
use alloc::format;

// ---------------------------------------------------------------------------
// SBI (Supervisor Binary Interface) — the RISC-V equivalent of BIOS calls
// ---------------------------------------------------------------------------

// SBI extension IDs
const SBI_EXT_LEGACY_CONSOLE_PUTCHAR: u64 = 0x01;
const SBI_EXT_LEGACY_SHUTDOWN: u64 = 0x08;
const SBI_EXT_TIMER: u64 = 0x54494D45; // "TIME"
const SBI_EXT_BASE: u64 = 0x10;

/// Perform an SBI ecall. The RISC-V SBI convention uses:
///   a7 = extension ID, a6 = function ID, a0-a2 = arguments.
/// Returns (error, value) in (a0, a1).
pub fn sbi_call(ext: u64, fid: u64, arg0: u64, arg1: u64, arg2: u64) -> (i64, i64) {
    #[cfg(target_arch = "riscv64")]
    {
        let error: i64;
        let value: i64;
        unsafe {
            core::arch::asm!(
                "ecall",
                in("a0") arg0,
                in("a1") arg1,
                in("a2") arg2,
                in("a6") fid,
                in("a7") ext,
                lateout("a0") error,
                lateout("a1") value,
            );
        }
        (error, value)
    }
    #[cfg(not(target_arch = "riscv64"))]
    {
        let _ = (ext, fid, arg0, arg1, arg2);
        (0, 0)
    }
}

/// Legacy SBI console putchar — writes a single byte to the debug console.
pub fn sbi_console_putchar(c: u8) {
    sbi_call(SBI_EXT_LEGACY_CONSOLE_PUTCHAR, 0, c as u64, 0, 0);
}

/// Write a string to the SBI debug console character by character.
pub fn sbi_console_puts(s: &str) {
    for b in s.bytes() {
        sbi_console_putchar(b);
    }
}

/// Set the next timer interrupt via SBI TIME extension.
pub fn sbi_set_timer(stime: u64) {
    sbi_call(SBI_EXT_TIMER, 0, stime, 0, 0);
}

/// Shutdown the machine via legacy SBI call.
pub fn sbi_shutdown() -> ! {
    sbi_call(SBI_EXT_LEGACY_SHUTDOWN, 0, 0, 0, 0);
    // Should not return, but just in case:
    loop {
        #[cfg(target_arch = "riscv64")]
        unsafe { core::arch::asm!("wfi"); }
        #[cfg(not(target_arch = "riscv64"))]
        core::hint::spin_loop();
    }
}

/// Query the SBI specification version. Returns (major, minor).
pub fn sbi_get_spec_version() -> (u32, u32) {
    let (_, val) = sbi_call(SBI_EXT_BASE, 0, 0, 0, 0);
    let major = ((val as u64) >> 24) as u32;
    let minor = (val as u64 & 0xFF_FFFF) as u32;
    (major, minor)
}

/// Query the SBI implementation ID.
pub fn sbi_get_impl_id() -> i64 {
    let (_, val) = sbi_call(SBI_EXT_BASE, 1, 0, 0, 0);
    val
}

// ---------------------------------------------------------------------------
// CSR (Control and Status Register) accessors
// ---------------------------------------------------------------------------

/// Read the hart (hardware thread) ID from mhartid CSR.
/// Note: mhartid is M-mode only; in S-mode we read from a0 passed at boot.
/// This fallback reads sscratch where OpenSBI typically stores hart ID.
pub fn read_mhartid() -> u64 {
    #[cfg(target_arch = "riscv64")]
    {
        let val: u64;
        unsafe { core::arch::asm!("csrr {}, sscratch", out(reg) val); }
        val
    }
    #[cfg(not(target_arch = "riscv64"))]
    { 0 }
}

/// Read the supervisor status register (sstatus).
pub fn read_sstatus() -> u64 {
    #[cfg(target_arch = "riscv64")]
    {
        let val: u64;
        unsafe { core::arch::asm!("csrr {}, sstatus", out(reg) val); }
        val
    }
    #[cfg(not(target_arch = "riscv64"))]
    { 0 }
}

/// Read the supervisor interrupt-enable register (sie).
pub fn read_sie() -> u64 {
    #[cfg(target_arch = "riscv64")]
    {
        let val: u64;
        unsafe { core::arch::asm!("csrr {}, sie", out(reg) val); }
        val
    }
    #[cfg(not(target_arch = "riscv64"))]
    { 0 }
}

/// Write the supervisor interrupt-enable register (sie).
pub fn write_sie(val: u64) {
    #[cfg(target_arch = "riscv64")]
    unsafe { core::arch::asm!("csrw sie, {}", in(reg) val); }
    #[cfg(not(target_arch = "riscv64"))]
    { let _ = val; }
}

/// Read the supervisor interrupt-pending register (sip).
pub fn read_sip() -> u64 {
    #[cfg(target_arch = "riscv64")]
    {
        let val: u64;
        unsafe { core::arch::asm!("csrr {}, sip", out(reg) val); }
        val
    }
    #[cfg(not(target_arch = "riscv64"))]
    { 0 }
}

/// Read the supervisor trap vector base address register (stvec).
pub fn read_stvec() -> u64 {
    #[cfg(target_arch = "riscv64")]
    {
        let val: u64;
        unsafe { core::arch::asm!("csrr {}, stvec", out(reg) val); }
        val
    }
    #[cfg(not(target_arch = "riscv64"))]
    { 0 }
}

/// Write the supervisor trap vector base address register (stvec).
pub fn write_stvec(val: u64) {
    #[cfg(target_arch = "riscv64")]
    unsafe { core::arch::asm!("csrw stvec, {}", in(reg) val); }
    #[cfg(not(target_arch = "riscv64"))]
    { let _ = val; }
}

/// Read the supervisor trap cause register (scause).
pub fn read_scause() -> u64 {
    #[cfg(target_arch = "riscv64")]
    {
        let val: u64;
        unsafe { core::arch::asm!("csrr {}, scause", out(reg) val); }
        val
    }
    #[cfg(not(target_arch = "riscv64"))]
    { 0 }
}

/// Read the supervisor trap value register (stval).
pub fn read_stval() -> u64 {
    #[cfg(target_arch = "riscv64")]
    {
        let val: u64;
        unsafe { core::arch::asm!("csrr {}, stval", out(reg) val); }
        val
    }
    #[cfg(not(target_arch = "riscv64"))]
    { 0 }
}

/// Read the supervisor exception program counter (sepc).
pub fn read_sepc() -> u64 {
    #[cfg(target_arch = "riscv64")]
    {
        let val: u64;
        unsafe { core::arch::asm!("csrr {}, sepc", out(reg) val); }
        val
    }
    #[cfg(not(target_arch = "riscv64"))]
    { 0 }
}

/// Write the supervisor exception program counter (sepc).
pub fn write_sepc(val: u64) {
    #[cfg(target_arch = "riscv64")]
    unsafe { core::arch::asm!("csrw sepc, {}", in(reg) val); }
    #[cfg(not(target_arch = "riscv64"))]
    { let _ = val; }
}

/// Read the cycle counter via rdtime pseudo-instruction.
pub fn read_time() -> u64 {
    #[cfg(target_arch = "riscv64")]
    {
        let val: u64;
        unsafe { core::arch::asm!("rdtime {}", out(reg) val); }
        val
    }
    #[cfg(not(target_arch = "riscv64"))]
    { 0 }
}

/// Read the cycle counter via rdcycle pseudo-instruction.
pub fn read_cycle() -> u64 {
    #[cfg(target_arch = "riscv64")]
    {
        let val: u64;
        unsafe { core::arch::asm!("rdcycle {}", out(reg) val); }
        val
    }
    #[cfg(not(target_arch = "riscv64"))]
    { 0 }
}

/// Read the satp register (Supervisor Address Translation and Protection).
pub fn read_satp() -> u64 {
    #[cfg(target_arch = "riscv64")]
    {
        let val: u64;
        unsafe { core::arch::asm!("csrr {}, satp", out(reg) val); }
        val
    }
    #[cfg(not(target_arch = "riscv64"))]
    { 0 }
}

// ---------------------------------------------------------------------------
// Trap handling
// ---------------------------------------------------------------------------

static TRAPS_INIT: AtomicBool = AtomicBool::new(false);

// Trap cause codes (scause values)
// Interrupts (bit 63 set)
const CAUSE_SUPERVISOR_TIMER_INT: u64 = (1 << 63) | 5;
const CAUSE_SUPERVISOR_EXTERNAL_INT: u64 = (1 << 63) | 9;
// Exceptions (bit 63 clear)
const CAUSE_INSTRUCTION_MISALIGNED: u64 = 0;
const CAUSE_ILLEGAL_INSTRUCTION: u64 = 2;
const CAUSE_BREAKPOINT: u64 = 3;
const CAUSE_LOAD_PAGE_FAULT: u64 = 13;
const CAUSE_STORE_PAGE_FAULT: u64 = 15;
const CAUSE_ECALL_FROM_UMODE: u64 = 8;

/// Initialise the trap vector. Sets stvec to point to our trap handler.
/// Uses Direct mode (stvec[1:0] = 00).
pub fn init_traps() {
    if TRAPS_INIT.swap(true, Ordering::SeqCst) {
        return;
    }
    let handler = trap_handler as *const () as usize;
    write_stvec(handler as u64);

    // Enable supervisor timer and external interrupts in sie
    let sie = read_sie();
    // STIE (bit 5) = timer, SEIE (bit 9) = external
    write_sie(sie | (1 << 5) | (1 << 9));
}

/// Top-level trap handler. Called from the trap vector.
/// Dispatches based on scause.
#[no_mangle]
pub extern "C" fn trap_handler() {
    let scause = read_scause();
    let stval = read_stval();
    let sepc = read_sepc();

    if scause == CAUSE_SUPERVISOR_TIMER_INT {
        timer_handler();
    } else if scause == CAUSE_SUPERVISOR_EXTERNAL_INT {
        plic_handler();
    } else if scause == CAUSE_ECALL_FROM_UMODE {
        // Environment call from U-mode (syscall)
        // Advance sepc past the ecall instruction (4 bytes)
        write_sepc(sepc + 4);
        // Syscall dispatch would go here
    } else if scause == CAUSE_LOAD_PAGE_FAULT || scause == CAUSE_STORE_PAGE_FAULT {
        page_fault_handler(scause, stval, sepc);
    } else if scause == CAUSE_ILLEGAL_INSTRUCTION {
        illegal_instruction_handler(stval, sepc);
    } else if scause == CAUSE_INSTRUCTION_MISALIGNED {
        let _ = (stval, sepc);
        // Misaligned instruction fetch — fatal
    } else if scause == CAUSE_BREAKPOINT {
        // Advance past ebreak (compressed=2 bytes, normal=4 bytes)
        write_sepc(sepc + 2);
    } else {
        // Unknown trap
        let _ = (scause, stval, sepc);
    }
}

/// Handle a page fault.
fn page_fault_handler(cause: u64, addr: u64, pc: u64) {
    let _ = (cause, addr, pc);
    // In a full implementation, this would:
    //   - Check if the faulting address is in a valid VMA
    //   - Allocate a page and map it
    //   - Or kill the offending process
}

/// Handle an illegal instruction.
fn illegal_instruction_handler(instruction: u64, pc: u64) {
    let _ = (instruction, pc);
    // Fatal — in a full implementation this would kill the process
}

// ---------------------------------------------------------------------------
// PLIC (Platform-Level Interrupt Controller)
// ---------------------------------------------------------------------------

/// QEMU virt machine PLIC base address.
const PLIC_BASE: u64 = 0x0C00_0000;

/// PLIC priority register offset for IRQ n: base + n*4
const PLIC_PRIORITY_OFFSET: u64 = 0x0000;
/// PLIC pending register offset
const PLIC_PENDING_OFFSET: u64 = 0x1000;
/// PLIC enable register offset for context 1 (S-mode, hart 0)
const PLIC_SENABLE_OFFSET: u64 = 0x2080;
/// PLIC threshold register for context 1
const PLIC_STHRESHOLD_OFFSET: u64 = 0x20_1000;
/// PLIC claim/complete register for context 1
const PLIC_SCLAIM_OFFSET: u64 = 0x20_1004;

/// Initialise the PLIC. Set threshold to 0 (accept all priorities).
pub fn plic_init() {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        let threshold = (PLIC_BASE + PLIC_STHRESHOLD_OFFSET) as *mut u32;
        core::ptr::write_volatile(threshold, 0);
    }
}

/// Enable a specific IRQ on the PLIC for S-mode, hart 0.
pub fn plic_enable(irq: u32) {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        let reg_idx = irq / 32;
        let bit = irq % 32;
        let addr = (PLIC_BASE + PLIC_SENABLE_OFFSET + (reg_idx as u64) * 4) as *mut u32;
        let val = core::ptr::read_volatile(addr);
        core::ptr::write_volatile(addr, val | (1 << bit));
    }
    #[cfg(not(target_arch = "riscv64"))]
    { let _ = irq; }
}

/// Set the priority for a given IRQ (1–7, higher = more urgent).
pub fn plic_set_priority(irq: u32, priority: u32) {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        let addr = (PLIC_BASE + PLIC_PRIORITY_OFFSET + (irq as u64) * 4) as *mut u32;
        core::ptr::write_volatile(addr, priority & 0x7);
    }
    #[cfg(not(target_arch = "riscv64"))]
    { let _ = (irq, priority); }
}

/// Claim the highest-priority pending interrupt. Returns IRQ number (0 = none).
pub fn plic_claim() -> u32 {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        let addr = (PLIC_BASE + PLIC_SCLAIM_OFFSET) as *const u32;
        core::ptr::read_volatile(addr)
    }
    #[cfg(not(target_arch = "riscv64"))]
    { 0 }
}

/// Signal completion of interrupt handling for the given IRQ.
pub fn plic_complete(irq: u32) {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        let addr = (PLIC_BASE + PLIC_SCLAIM_OFFSET) as *mut u32;
        core::ptr::write_volatile(addr, irq);
    }
    #[cfg(not(target_arch = "riscv64"))]
    { let _ = irq; }
}

/// Handle an external interrupt from the PLIC.
fn plic_handler() {
    let irq = plic_claim();
    if irq == 0 {
        return; // spurious
    }
    // Dispatch based on IRQ number
    // IRQ 10 = UART0 on QEMU virt
    match irq {
        10 => {
            // UART interrupt — read character
        }
        _ => {
            // Unknown IRQ
        }
    }
    plic_complete(irq);
}

// ---------------------------------------------------------------------------
// CLINT Timer
// ---------------------------------------------------------------------------

/// QEMU virt machine CLINT base address.
const CLINT_BASE: u64 = 0x0200_0000;
/// mtime register offset within CLINT.
const CLINT_MTIME_OFFSET: u64 = 0xBFF8;
/// Timer frequency on QEMU virt (10 MHz).
const TIMER_FREQ: u64 = 10_000_000;
/// Target tick rate in Hz.
const TIMER_HZ: u64 = 100;
/// Timer interval in timer ticks.
const TIMER_INTERVAL: u64 = TIMER_FREQ / TIMER_HZ;

static TICK_COUNT: AtomicU64 = AtomicU64::new(0);

/// Read the mtime register from the CLINT.
pub fn read_mtime() -> u64 {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        let addr = (CLINT_BASE + CLINT_MTIME_OFFSET) as *const u64;
        core::ptr::read_volatile(addr)
    }
    #[cfg(not(target_arch = "riscv64"))]
    { 0 }
}

/// Initialise the timer. Sets the first timer interrupt 10ms from now via SBI.
pub fn timer_init() {
    let time = read_mtime();
    sbi_set_timer(time + TIMER_INTERVAL);
}

/// Handle a timer interrupt. Increments the tick counter and schedules
/// the next timer interrupt.
pub fn timer_handler() {
    TICK_COUNT.fetch_add(1, Ordering::Relaxed);
    // Schedule next timer interrupt
    let time = read_mtime();
    sbi_set_timer(time + TIMER_INTERVAL);
}

/// Return the number of timer ticks since boot.
pub fn ticks() -> u64 {
    TICK_COUNT.load(Ordering::Relaxed)
}

/// Return uptime in seconds.
pub fn uptime_secs() -> u64 {
    ticks() / TIMER_HZ
}

// ---------------------------------------------------------------------------
// MMU (Sv39 page tables)
// ---------------------------------------------------------------------------

/// Sv39 mode constant for satp register.
const SATP_MODE_SV39: u64 = 8;

/// Write the satp register to enable/configure virtual memory.
///   mode: 0=bare (no translation), 8=Sv39, 9=Sv48
///   asid: Address Space ID (up to 16 bits)
///   ppn:  Physical Page Number of the root page table
pub fn satp_write(mode: u8, asid: u16, ppn: u64) {
    let val = ((mode as u64) << 60) | ((asid as u64) << 44) | ppn;
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("csrw satp, {}", in(reg) val);
        // Flush the TLB
        core::arch::asm!("sfence.vma");
    }
    #[cfg(not(target_arch = "riscv64"))]
    { let _ = val; }
}

/// Initialise Sv39 paging. For now, leave paging disabled (bare mode).
/// A full implementation would build a 3-level page table and enable Sv39.
pub fn mmu_init() {
    // Leave in bare mode until proper page tables are constructed
    // satp_write(SATP_MODE_SV39 as u8, 0, root_ppn);
    let _ = SATP_MODE_SV39;
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

/// Halt the CPU — loops WFI forever.
pub fn halt() -> ! {
    loop {
        #[cfg(target_arch = "riscv64")]
        unsafe { core::arch::asm!("wfi"); }
        #[cfg(not(target_arch = "riscv64"))]
        core::hint::spin_loop();
    }
}

/// Execute WFI (Wait For Interrupt).
pub fn wfi() {
    #[cfg(target_arch = "riscv64")]
    unsafe { core::arch::asm!("wfi"); }
    #[cfg(not(target_arch = "riscv64"))]
    core::hint::spin_loop();
}

/// Memory fence — full barrier.
pub fn fence() {
    #[cfg(target_arch = "riscv64")]
    unsafe { core::arch::asm!("fence"); }
    #[cfg(not(target_arch = "riscv64"))]
    core::sync::atomic::fence(Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// CPU info
// ---------------------------------------------------------------------------

/// Return a human-readable description of this hart.
pub fn cpu_info() -> String {
    let (major, minor) = sbi_get_spec_version();
    let impl_id = sbi_get_impl_id();
    let impl_name = match impl_id {
        0 => "BBL",
        1 => "OpenSBI",
        2 => "Xvisor",
        3 => "KVM",
        4 => "RustSBI",
        _ => "Unknown",
    };
    format!(
        "RISC-V RV64GC hart {} | SBI {}.{} ({}) | ticks {}",
        read_mhartid(),
        major,
        minor,
        impl_name,
        ticks()
    )
}

/// Return architecture summary string.
pub fn arch_info() -> String {
    format!(
        "riscv64: {} | timer {} Hz | uptime {}s",
        cpu_info(),
        TIMER_HZ,
        uptime_secs()
    )
}

/// Initialise all RISC-V architecture components.
pub fn init() {
    init_traps();
    plic_init();
    timer_init();
    mmu_init();
}
