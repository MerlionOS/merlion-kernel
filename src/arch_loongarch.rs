/// LoongArch 64-bit architecture support for MerlionOS.
/// Provides boot code, exception handling, and I/O for Loongson processors.
/// Targets: QEMU loongarch64-virt, Loongson 3A5000/3A6000 systems.
///
/// LoongArch is China's homegrown CPU architecture (龙芯). It is a RISC
/// design with its own ISA, CSR system, and I/O mechanisms.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use alloc::string::String;
use alloc::format;

// ---------------------------------------------------------------------------
// CSR (Control Status Registers) — LoongArch has its own CSR system
// ---------------------------------------------------------------------------

/// Current Mode — controls privilege level and global interrupt enable.
const CSR_CRMD: u32 = 0x0;
/// Pre-exception Mode — saves CRMD on exception entry.
const CSR_PRMD: u32 = 0x1;
/// Extended Unit Enable — FPU, LSX, LASX enable bits.
const CSR_EUEN: u32 = 0x2;
/// Exception Configuration — interrupt enable bits per line.
const CSR_ECFG: u32 = 0x4;
/// Exception Status — pending interrupts and exception subcode.
const CSR_ESTAT: u32 = 0x5;
/// Exception Return Address — PC to return to after exception.
const CSR_ERA: u32 = 0x6;
/// Bad Virtual Address — faulting address on TLB/address exceptions.
const CSR_BADV: u32 = 0x7;
/// Exception Entry — base address of exception handler.
const CSR_EENTRY: u32 = 0xC;
/// TLB Index — index into TLB for TLBRD/TLBWR.
const CSR_TLBIDX: u32 = 0x10;
/// TLB Entry High — VPN and ASID for TLB operations.
const CSR_TLBEHI: u32 = 0x11;
/// TLB Entry Low 0 — even page physical mapping.
const CSR_TLBELO0: u32 = 0x12;
/// TLB Entry Low 1 — odd page physical mapping.
const CSR_TLBELO1: u32 = 0x13;
/// CPU ID — processor identification.
const CSR_CPUID: u32 = 0x20;
/// Timer Configuration — periodic/one-shot timer setup.
const CSR_TCFG: u32 = 0x41;
/// Timer Value — current countdown value.
const CSR_TVAL: u32 = 0x42;
/// Timer Interrupt Clear — write 1 to clear timer interrupt.
const CSR_TICLR: u32 = 0x44;

/// Read a LoongArch CSR by number.
/// On non-loongarch64 builds, returns a stub value.
pub fn csr_read(csr: u32) -> u64 {
    #[cfg(target_arch = "loongarch64")]
    {
        let val: u64;
        unsafe {
            // LoongArch CSR read instruction: csrrd rd, csr_num
            // Since inline asm can't encode arbitrary CSR numbers directly,
            // we use a match for the CSRs we support.
            match csr {
                0x0 => core::arch::asm!("csrrd {}, 0x0", out(reg) val),
                0x1 => core::arch::asm!("csrrd {}, 0x1", out(reg) val),
                0x2 => core::arch::asm!("csrrd {}, 0x2", out(reg) val),
                0x4 => core::arch::asm!("csrrd {}, 0x4", out(reg) val),
                0x5 => core::arch::asm!("csrrd {}, 0x5", out(reg) val),
                0x6 => core::arch::asm!("csrrd {}, 0x6", out(reg) val),
                0x7 => core::arch::asm!("csrrd {}, 0x7", out(reg) val),
                0xC => core::arch::asm!("csrrd {}, 0xC", out(reg) val),
                0x10 => core::arch::asm!("csrrd {}, 0x10", out(reg) val),
                0x11 => core::arch::asm!("csrrd {}, 0x11", out(reg) val),
                0x12 => core::arch::asm!("csrrd {}, 0x12", out(reg) val),
                0x13 => core::arch::asm!("csrrd {}, 0x13", out(reg) val),
                0x20 => core::arch::asm!("csrrd {}, 0x20", out(reg) val),
                0x41 => core::arch::asm!("csrrd {}, 0x41", out(reg) val),
                0x42 => core::arch::asm!("csrrd {}, 0x42", out(reg) val),
                0x44 => core::arch::asm!("csrrd {}, 0x44", out(reg) val),
                _ => val = 0,
            }
        }
        val
    }
    #[cfg(not(target_arch = "loongarch64"))]
    {
        let _ = csr;
        0
    }
}

/// Write a LoongArch CSR by number.
pub fn csr_write(csr: u32, val: u64) {
    #[cfg(target_arch = "loongarch64")]
    unsafe {
        match csr {
            0x0 => core::arch::asm!("csrwr {}, 0x0", in(reg) val),
            0x1 => core::arch::asm!("csrwr {}, 0x1", in(reg) val),
            0x2 => core::arch::asm!("csrwr {}, 0x2", in(reg) val),
            0x4 => core::arch::asm!("csrwr {}, 0x4", in(reg) val),
            0x5 => core::arch::asm!("csrwr {}, 0x5", in(reg) val),
            0x6 => core::arch::asm!("csrwr {}, 0x6", in(reg) val),
            0x7 => core::arch::asm!("csrwr {}, 0x7", in(reg) val),
            0xC => core::arch::asm!("csrwr {}, 0xC", in(reg) val),
            0x10 => core::arch::asm!("csrwr {}, 0x10", in(reg) val),
            0x11 => core::arch::asm!("csrwr {}, 0x11", in(reg) val),
            0x12 => core::arch::asm!("csrwr {}, 0x12", in(reg) val),
            0x13 => core::arch::asm!("csrwr {}, 0x13", in(reg) val),
            0x20 => core::arch::asm!("csrwr {}, 0x20", in(reg) val),
            0x41 => core::arch::asm!("csrwr {}, 0x41", in(reg) val),
            0x42 => core::arch::asm!("csrwr {}, 0x42", in(reg) val),
            0x44 => core::arch::asm!("csrwr {}, 0x44", in(reg) val),
            _ => {}
        }
    }
    #[cfg(not(target_arch = "loongarch64"))]
    { let _ = (csr, val); }
}

// ---------------------------------------------------------------------------
// Exception handling
// ---------------------------------------------------------------------------

static EXCEPTIONS_INIT: AtomicBool = AtomicBool::new(false);

// LoongArch exception subcodes (from ESTAT.Ecode field, bits 21:16)
const ECODE_INT: u64 = 0x0;   // Interrupt
const ECODE_PIL: u64 = 0x1;   // Page Invalid — Load
const ECODE_PIS: u64 = 0x2;   // Page Invalid — Store
const ECODE_PIF: u64 = 0x3;   // Page Invalid — Instruction fetch
const ECODE_PME: u64 = 0x4;   // Page Modification Exception
const ECODE_SYS: u64 = 0xB;   // Syscall

/// Install the exception entry point by writing EENTRY CSR.
pub fn init_exceptions() {
    if EXCEPTIONS_INIT.swap(true, Ordering::SeqCst) {
        return;
    }
    let handler = exception_handler as *const () as usize as u64;
    csr_write(CSR_EENTRY, handler);

    // Enable interrupts in CRMD: IE (bit 2) = 1
    let crmd = csr_read(CSR_CRMD);
    csr_write(CSR_CRMD, crmd | (1 << 2));

    // Enable timer interrupt in ECFG: bit 11 = timer interrupt
    let ecfg = csr_read(CSR_ECFG);
    csr_write(CSR_ECFG, ecfg | (1 << 11));

    // Disable FPU to avoid accidental FP usage (clear EUEN.FPE, bit 0)
    csr_write(CSR_EUEN, 0);
}

/// Top-level exception handler. Dispatches based on ESTAT.
#[no_mangle]
pub extern "C" fn exception_handler() {
    let estat = csr_read(CSR_ESTAT);
    let era = csr_read(CSR_ERA);
    let badv = csr_read(CSR_BADV);
    let ecode = (estat >> 16) & 0x3F;

    match ecode {
        ECODE_INT => {
            // Interrupt — check which interrupt line
            let is_field = estat & 0x1FFF; // bits 12:0 are interrupt status
            if is_field & (1 << 11) != 0 {
                // Timer interrupt
                timer_handler();
            }
            if is_field & (1 << 2) != 0 {
                // HWI0 — external interrupt
                extioi_handler();
            }
        }
        ECODE_PIL | ECODE_PIS | ECODE_PIF => {
            page_fault_handler(ecode, badv, era);
        }
        ECODE_PME => {
            // Page modification exception — mark page dirty
            let _ = (badv, era);
        }
        ECODE_SYS => {
            // Syscall — advance ERA past the syscall instruction (4 bytes)
            csr_write(CSR_ERA, era + 4);
            // Syscall dispatch would go here
        }
        _ => {
            // Unknown exception
            let _ = (ecode, badv, era);
        }
    }
}

/// Handle a page fault (load, store, or instruction fetch).
fn page_fault_handler(ecode: u64, addr: u64, pc: u64) {
    let _ = (ecode, addr, pc);
    // In a full implementation:
    //   - Look up the faulting address in the process VMA list
    //   - Allocate a physical page and install a TLB entry
    //   - Or terminate the faulting process
}

/// Handle external interrupts dispatched via EXTIOI.
fn extioi_handler() {
    let irq = extioi_claim();
    if irq == 0 {
        return; // spurious
    }
    match irq {
        2 => {
            // UART interrupt on QEMU virt
        }
        _ => {}
    }
    extioi_complete(irq);
}

// ---------------------------------------------------------------------------
// Timer
// ---------------------------------------------------------------------------

/// Timer frequency — Loongson 3A5000 uses a 100 MHz stable counter.
/// QEMU virt also defaults to 100 MHz.
const LA_TIMER_FREQ: u64 = 100_000_000;
/// Target tick rate in Hz.
const TIMER_HZ: u64 = 100;
/// Timer interval in timer ticks for 100 Hz.
const TIMER_INTERVAL: u64 = LA_TIMER_FREQ / TIMER_HZ;

static TICK_COUNT: AtomicU64 = AtomicU64::new(0);

/// Initialise the hardware timer for periodic interrupts at 100 Hz.
/// TCFG format: bits[1:0] = En | Periodic, bits[63:2] = InitVal.
pub fn timer_init() {
    // Enable=1, Periodic=1, InitVal = TIMER_INTERVAL
    let tcfg = (TIMER_INTERVAL << 2) | 0x3;
    csr_write(CSR_TCFG, tcfg);
}

/// Handle a timer interrupt. Clear the interrupt and increment tick count.
pub fn timer_handler() {
    // Write 1 to TICLR to clear the timer interrupt
    csr_write(CSR_TICLR, 1);
    TICK_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Return the number of timer ticks since boot.
pub fn ticks() -> u64 {
    TICK_COUNT.load(Ordering::Relaxed)
}

/// Return uptime in seconds.
pub fn uptime_secs() -> u64 {
    ticks() / TIMER_HZ
}

/// Read the current timer countdown value.
pub fn timer_value() -> u64 {
    csr_read(CSR_TVAL)
}

// ---------------------------------------------------------------------------
// UART (NS16550 compatible)
// ---------------------------------------------------------------------------

/// LoongArch QEMU virt UART base address.
const UART_BASE: u64 = 0x1FE0_01E0;

// NS16550 register offsets
const UART_THR: u64 = 0x00; // Transmit Holding Register (write)
const UART_RBR: u64 = 0x00; // Receive Buffer Register (read)
const UART_IER: u64 = 0x01; // Interrupt Enable Register
const UART_FCR: u64 = 0x02; // FIFO Control Register (write)
const UART_LCR: u64 = 0x03; // Line Control Register
const UART_MCR: u64 = 0x04; // Modem Control Register
const UART_LSR: u64 = 0x05; // Line Status Register
const UART_DLL: u64 = 0x00; // Divisor Latch Low (when DLAB=1)
const UART_DLH: u64 = 0x01; // Divisor Latch High (when DLAB=1)

// LSR bits
const LSR_DR: u8 = 1 << 0;    // Data Ready
const LSR_THRE: u8 = 1 << 5;  // Transmitter Holding Register Empty

static UART_INIT_DONE: AtomicBool = AtomicBool::new(false);

/// Initialise the NS16550 UART at UART_BASE.
pub fn uart_init() {
    if UART_INIT_DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    #[cfg(target_arch = "loongarch64")]
    unsafe {
        // Disable interrupts
        uart_write_reg(UART_IER, 0x00);
        // Enable DLAB to set baud rate divisor
        uart_write_reg(UART_LCR, 0x80);
        // Set divisor to 1 (115200 baud assuming 1.8432 MHz clock)
        uart_write_reg(UART_DLL, 0x01);
        uart_write_reg(UART_DLH, 0x00);
        // 8 data bits, no parity, 1 stop bit (8N1), clear DLAB
        uart_write_reg(UART_LCR, 0x03);
        // Enable FIFO, clear TX/RX, 14-byte trigger
        uart_write_reg(UART_FCR, 0xC7);
        // DTR + RTS + OUT2
        uart_write_reg(UART_MCR, 0x0B);
        // Enable receive interrupt
        uart_write_reg(UART_IER, 0x01);
    }
}

/// Write a byte to a UART register.
#[cfg(target_arch = "loongarch64")]
unsafe fn uart_write_reg(offset: u64, val: u8) {
    let addr = (UART_BASE + offset) as *mut u8;
    core::ptr::write_volatile(addr, val);
}

/// Read a byte from a UART register.
#[cfg(target_arch = "loongarch64")]
unsafe fn uart_read_reg(offset: u64) -> u8 {
    let addr = (UART_BASE + offset) as *const u8;
    core::ptr::read_volatile(addr)
}

/// Write a single character to the UART. Spins until the TX holding
/// register is empty.
pub fn uart_putc(c: u8) {
    #[cfg(target_arch = "loongarch64")]
    unsafe {
        // Wait for THR to be empty
        while uart_read_reg(UART_LSR) & LSR_THRE == 0 {
            core::hint::spin_loop();
        }
        uart_write_reg(UART_THR, c);
    }
    #[cfg(not(target_arch = "loongarch64"))]
    { let _ = c; }
}

/// Try to read a character from the UART. Returns None if no data is ready.
pub fn uart_getc() -> Option<u8> {
    #[cfg(target_arch = "loongarch64")]
    unsafe {
        if uart_read_reg(UART_LSR) & LSR_DR != 0 {
            Some(uart_read_reg(UART_RBR))
        } else {
            None
        }
    }
    #[cfg(not(target_arch = "loongarch64"))]
    { None }
}

/// Write a string to the UART, converting \n to \r\n.
pub fn uart_puts(s: &str) {
    for b in s.bytes() {
        if b == b'\n' {
            uart_putc(b'\r');
        }
        uart_putc(b);
    }
}

// ---------------------------------------------------------------------------
// IOCSR (I/O Control and Status Registers) — LoongArch-specific I/O
// ---------------------------------------------------------------------------

/// Read a 64-bit value from an IOCSR address.
pub fn iocsr_read(addr: u32) -> u64 {
    #[cfg(target_arch = "loongarch64")]
    {
        let val: u64;
        unsafe {
            core::arch::asm!(
                "iocsrrd.d {}, {}",
                out(reg) val,
                in(reg) addr,
            );
        }
        val
    }
    #[cfg(not(target_arch = "loongarch64"))]
    {
        let _ = addr;
        0
    }
}

/// Write a 64-bit value to an IOCSR address.
pub fn iocsr_write(addr: u32, val: u64) {
    #[cfg(target_arch = "loongarch64")]
    unsafe {
        core::arch::asm!(
            "iocsrwr.d {}, {}",
            in(reg) val,
            in(reg) addr,
        );
    }
    #[cfg(not(target_arch = "loongarch64"))]
    { let _ = (addr, val); }
}

// ---------------------------------------------------------------------------
// EXT Interrupt Controller (EXTIOI)
// ---------------------------------------------------------------------------

/// EXTIOI base address (memory-mapped, QEMU virt).
const EXTIOI_BASE: u64 = 0x1FE0_1400;
/// Enable register offset.
const EXTIOI_EN_OFFSET: u64 = 0x00;
/// Status register offset.
const EXTIOI_STATUS_OFFSET: u64 = 0x20;

/// Initialise the Extended I/O Interrupt Controller.
pub fn extioi_init() {
    // On QEMU virt, EXTIOI is auto-configured.
    // On real hardware, we'd program routing and enable bits here.
    #[cfg(target_arch = "loongarch64")]
    unsafe {
        // Enable all 256 interrupt lines in 8x32-bit enable registers
        for i in 0..8u64 {
            let addr = (EXTIOI_BASE + EXTIOI_EN_OFFSET + i * 4) as *mut u32;
            core::ptr::write_volatile(addr, 0xFFFF_FFFF);
        }
    }
}

/// Enable a specific IRQ on the EXTIOI controller.
pub fn extioi_enable(irq: u32) {
    #[cfg(target_arch = "loongarch64")]
    unsafe {
        let reg_idx = (irq / 32) as u64;
        let bit = irq % 32;
        let addr = (EXTIOI_BASE + EXTIOI_EN_OFFSET + reg_idx * 4) as *mut u32;
        let val = core::ptr::read_volatile(addr);
        core::ptr::write_volatile(addr, val | (1 << bit));
    }
    #[cfg(not(target_arch = "loongarch64"))]
    { let _ = irq; }
}

/// Claim the highest-priority pending interrupt. Returns IRQ number.
pub fn extioi_claim() -> u32 {
    #[cfg(target_arch = "loongarch64")]
    unsafe {
        // Scan status registers to find first pending IRQ
        for i in 0..8u64 {
            let addr = (EXTIOI_BASE + EXTIOI_STATUS_OFFSET + i * 4) as *const u32;
            let status = core::ptr::read_volatile(addr);
            if status != 0 {
                let bit = status.trailing_zeros();
                return (i as u32) * 32 + bit;
            }
        }
        0 // no interrupt pending
    }
    #[cfg(not(target_arch = "loongarch64"))]
    { 0 }
}

/// Signal completion of interrupt handling for the given IRQ.
/// On EXTIOI, we clear the status bit by writing 1 to it.
pub fn extioi_complete(irq: u32) {
    #[cfg(target_arch = "loongarch64")]
    unsafe {
        let reg_idx = (irq / 32) as u64;
        let bit = irq % 32;
        let addr = (EXTIOI_BASE + EXTIOI_STATUS_OFFSET + reg_idx * 4) as *mut u32;
        core::ptr::write_volatile(addr, 1 << bit);
    }
    #[cfg(not(target_arch = "loongarch64"))]
    { let _ = irq; }
}

// ---------------------------------------------------------------------------
// CPU info
// ---------------------------------------------------------------------------

/// Read the CPU ID from the CPUID CSR.
pub fn cpu_id() -> u32 {
    csr_read(CSR_CPUID) as u32
}

/// Return a human-readable description of the CPU.
pub fn cpu_info() -> String {
    let id = cpu_id();
    let core_id = id & 0x1FF;        // bits 8:0 = core number
    let cluster = (id >> 9) & 0xF;   // bits 12:9 = cluster
    // On Loongson 3A5000/3A6000, we can identify the model from CPUCFG
    // For now, report what we can read.
    format!(
        "LoongArch64 core {} cluster {} (CPUID 0x{:08x}) | ticks {}",
        core_id, cluster, id, ticks()
    )
}

/// Return architecture summary string.
pub fn arch_info() -> String {
    format!(
        "loongarch64: {} | timer {} Hz | uptime {}s",
        cpu_info(),
        TIMER_HZ,
        uptime_secs()
    )
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

/// Halt the CPU — loops idle forever.
pub fn halt() -> ! {
    loop {
        #[cfg(target_arch = "loongarch64")]
        unsafe { core::arch::asm!("idle 0"); }
        #[cfg(not(target_arch = "loongarch64"))]
        core::hint::spin_loop();
    }
}

/// Execute the idle instruction (low-power wait).
pub fn idle() {
    #[cfg(target_arch = "loongarch64")]
    unsafe { core::arch::asm!("idle 0"); }
    #[cfg(not(target_arch = "loongarch64"))]
    core::hint::spin_loop();
}

/// Initialise all LoongArch architecture components.
pub fn init() {
    uart_init();
    init_exceptions();
    extioi_init();
    timer_init();
}
