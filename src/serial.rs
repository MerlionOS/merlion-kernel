/// Serial port (UART 16550) driver for COM1 at I/O port 0x3F8.
/// Used for kernel logging — output appears in the QEMU terminal.

use core::fmt;
use spin::Mutex;
use x86_64::instructions::port::Port;

const COM1_PORT: u16 = 0x3F8;

pub static SERIAL1: Mutex<SerialPort> = Mutex::new(SerialPort::new(COM1_PORT));

pub struct SerialPort {
    base: u16,
}

impl SerialPort {
    const fn new(base: u16) -> Self {
        Self { base }
    }

    /// Initialize the UART with 38400 baud, 8N1.
    pub fn init(&mut self) {
        unsafe {
            let mut ier = Port::<u8>::new(self.base + 1);
            let mut lcr = Port::<u8>::new(self.base + 3);
            let mut data = Port::<u8>::new(self.base);
            let mut fifo = Port::<u8>::new(self.base + 2);
            let mut mcr = Port::<u8>::new(self.base + 4);

            ier.write(0x00);  // Disable interrupts
            lcr.write(0x80);  // Enable DLAB (set baud rate divisor)
            data.write(0x03); // Divisor lo: 38400 baud
            ier.write(0x00);  // Divisor hi
            lcr.write(0x03);  // 8 bits, no parity, one stop bit (8N1)
            fifo.write(0xC7); // Enable FIFO, clear, 14-byte threshold
            mcr.write(0x0B);  // IRQs enabled, RTS/DSR set
        }
    }

    fn write_byte(&mut self, byte: u8) {
        unsafe {
            // Wait for transmit holding register to be empty
            let mut line_status = Port::<u8>::new(self.base + 5);
            while line_status.read() & 0x20 == 0 {}
            Port::new(self.base).write(byte);
        }
    }
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
        Ok(())
    }
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_serial_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! serial_println {
    ()            => { $crate::serial_print!("\n") };
    ($($arg:tt)*) => { $crate::serial_print!("{}\n", format_args!($($arg)*)) };
}

#[doc(hidden)]
pub fn _serial_print(args: fmt::Arguments) {
    use fmt::Write;
    use x86_64::instructions::interrupts;

    // Disable interrupts while holding the serial lock to prevent deadlock
    interrupts::without_interrupts(|| {
        SERIAL1.lock().write_fmt(args).unwrap();
    });
}
