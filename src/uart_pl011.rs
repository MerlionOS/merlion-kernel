/// PL011 UART driver for Raspberry Pi.
/// Provides serial I/O through the ARM PL011 UART peripheral.
/// Used as the primary debug output on Raspberry Pi boards.

use core::fmt;
use spin::Mutex;

// ---------------------------------------------------------------------------
// Register definitions (offsets from UART_BASE)
// ---------------------------------------------------------------------------

/// PL011 base address — Pi 3 / QEMU raspi3b.
const UART_BASE: u64 = 0x3F20_1000;

const UART_DR: u64   = UART_BASE + 0x00;  // Data Register
const UART_FR: u64   = UART_BASE + 0x18;  // Flag Register
const UART_IBRD: u64 = UART_BASE + 0x24;  // Integer Baud Rate Divisor
const UART_FBRD: u64 = UART_BASE + 0x28;  // Fractional Baud Rate Divisor
const UART_LCRH: u64 = UART_BASE + 0x2C;  // Line Control Register
const UART_CR: u64   = UART_BASE + 0x30;  // Control Register
const UART_IMSC: u64 = UART_BASE + 0x38;  // Interrupt Mask Set/Clear
const UART_ICR: u64  = UART_BASE + 0x44;  // Interrupt Clear Register

/// TX FIFO full flag.
const FR_TXFF: u32 = 1 << 5;
/// RX FIFO empty flag.
const FR_RXFE: u32 = 1 << 4;
/// UART busy flag.
const FR_BUSY: u32 = 1 << 3;

// ---------------------------------------------------------------------------
// GPIO base for UART pin muxing
// ---------------------------------------------------------------------------

const GPIO_BASE: u64 = 0x3F20_0000;  // Pi 3

const GPFSEL1: u64      = GPIO_BASE + 0x04;
const GPPUD: u64        = GPIO_BASE + 0x94;
const GPPUDCLK0: u64    = GPIO_BASE + 0x98;

// ---------------------------------------------------------------------------
// Global UART writer behind a spinlock
// ---------------------------------------------------------------------------

pub static UART: Mutex<Pl011> = Mutex::new(Pl011);

pub struct Pl011;

impl fmt::Write for Pl011 {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        puts(s);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// GPIO setup for UART (set GPIO 14, 15 to ALT0 = UART TX/RX)
// ---------------------------------------------------------------------------

/// Configure GPIO pins 14 and 15 for UART ALT0 function.
pub fn gpio_uart_init() {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        // GPIO14 = ALT0 (bits 14:12 = 0b100), GPIO15 = ALT0 (bits 17:15 = 0b100)
        let mut sel = mmio_read(GPFSEL1);
        sel &= !((0b111 << 12) | (0b111 << 15)); // clear bits for GPIO14, GPIO15
        sel |= (0b100 << 12) | (0b100 << 15);    // ALT0
        mmio_write(GPFSEL1, sel);

        // Disable pull-up/down for GPIO 14, 15
        mmio_write(GPPUD, 0);
        delay_cycles(150);
        mmio_write(GPPUDCLK0, (1 << 14) | (1 << 15));
        delay_cycles(150);
        mmio_write(GPPUDCLK0, 0);
    }
}

// ---------------------------------------------------------------------------
// UART functions
// ---------------------------------------------------------------------------

/// Initialise the PL011 UART at 115200 baud, 8N1.
///
/// Assumes a 48 MHz UART reference clock (typical for Pi 3).
/// Baud divisor = 48_000_000 / (16 * 115200) = 26.0416..
///   IBRD = 26, FBRD = round(0.0416 * 64) = 3
pub fn init() {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        // Disable UART while configuring
        mmio_write(UART_CR, 0);

        // Set up GPIO pins for UART
        gpio_uart_init();

        // Clear all pending interrupts
        mmio_write(UART_ICR, 0x7FF);

        // Set baud rate: 115200 @ 48 MHz reference clock
        mmio_write(UART_IBRD, 26);
        mmio_write(UART_FBRD, 3);

        // 8-bit word length, enable FIFOs
        mmio_write(UART_LCRH, (1 << 4) | (1 << 5) | (1 << 6)); // FEN | WLEN 8-bit

        // Mask all interrupts (we poll)
        mmio_write(UART_IMSC, 0);

        // Enable UART, TX, RX
        mmio_write(UART_CR, (1 << 0) | (1 << 8) | (1 << 9)); // UARTEN | TXE | RXE
    }
}

/// Send a single byte, blocking until the TX FIFO has space.
pub fn putc(c: u8) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        // Wait until TX FIFO is not full
        while mmio_read(UART_FR) & FR_TXFF != 0 {
            core::hint::spin_loop();
        }
        mmio_write(UART_DR, c as u32);
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = c;
    }
}

/// Try to read a byte from the RX FIFO (non-blocking).
pub fn getc() -> Option<u8> {
    #[cfg(target_arch = "aarch64")]
    {
        let fr = unsafe { mmio_read(UART_FR) };
        if fr & FR_RXFE != 0 {
            None
        } else {
            Some(unsafe { mmio_read(UART_DR) } as u8)
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    { None }
}

/// Send a string.
pub fn puts(s: &str) {
    for b in s.bytes() {
        if b == b'\n' {
            putc(b'\r');
        }
        putc(b);
    }
}

/// Send raw bytes.
pub fn write_bytes(data: &[u8]) {
    for &b in data {
        putc(b);
    }
}

/// Write formatted output to UART (used by uart_println! macro).
pub fn write_fmt(args: fmt::Arguments) {
    use core::fmt::Write;
    let mut uart = UART.lock();
    let _ = uart.write_fmt(args);
}

// ---------------------------------------------------------------------------
// Macros
// ---------------------------------------------------------------------------

/// Print a line to the PL011 UART (aarch64 equivalent of serial_println!).
#[macro_export]
macro_rules! uart_println {
    () => { $crate::uart_pl011::puts("\r\n") };
    ($($arg:tt)*) => {
        $crate::uart_pl011::write_fmt(format_args!("{}\r\n", format_args!($($arg)*)))
    };
}

/// Print to the PL011 UART without a trailing newline.
#[macro_export]
macro_rules! uart_print {
    ($($arg:tt)*) => {
        $crate::uart_pl011::write_fmt(format_args!($($arg)*))
    };
}

// ---------------------------------------------------------------------------
// MMIO helpers
// ---------------------------------------------------------------------------

#[cfg(target_arch = "aarch64")]
unsafe fn mmio_write(reg: u64, val: u32) {
    core::ptr::write_volatile(reg as *mut u32, val);
}

#[cfg(target_arch = "aarch64")]
unsafe fn mmio_read(reg: u64) -> u32 {
    core::ptr::read_volatile(reg as *const u32)
}

/// Crude delay loop for GPIO setup timing.
fn delay_cycles(n: u64) {
    for _ in 0..n {
        #[cfg(target_arch = "aarch64")]
        unsafe { core::arch::asm!("nop"); }
        #[cfg(not(target_arch = "aarch64"))]
        core::hint::spin_loop();
    }
}
