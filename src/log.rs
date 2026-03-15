/// Kernel log ring buffer.
/// Captures log messages in a fixed-size circular buffer.
/// Retrievable via the `dmesg` shell command.

use core::fmt;
use spin::Mutex;

const LOG_SIZE: usize = 4096;

pub static KLOG: Mutex<KernelLog> = Mutex::new(KernelLog::new());

pub struct KernelLog {
    buf: [u8; LOG_SIZE],
    /// Write position (wraps around)
    write_pos: usize,
    /// Total bytes written (used to detect wrap)
    total_written: usize,
}

impl KernelLog {
    const fn new() -> Self {
        Self {
            buf: [0; LOG_SIZE],
            write_pos: 0,
            total_written: 0,
        }
    }

    /// Write a byte into the ring buffer.
    fn push(&mut self, byte: u8) {
        self.buf[self.write_pos] = byte;
        self.write_pos = (self.write_pos + 1) % LOG_SIZE;
        self.total_written += 1;
    }

    /// Read the log contents in order. Calls `f` with each contiguous slice.
    pub fn read(&self, mut f: impl FnMut(&[u8])) {
        if self.total_written <= LOG_SIZE {
            // Haven't wrapped yet — data is [0..write_pos)
            f(&self.buf[..self.write_pos]);
        } else {
            // Wrapped — oldest data starts at write_pos
            f(&self.buf[self.write_pos..]);
            f(&self.buf[..self.write_pos]);
        }
    }
}

impl fmt::Write for KernelLog {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.push(byte);
        }
        Ok(())
    }
}

/// Write formatted args to the kernel log.
pub fn log(args: fmt::Arguments) {
    use fmt::Write;
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        KLOG.lock().write_fmt(args).unwrap();
    });
}

/// Macro to log a message to the kernel ring buffer.
#[macro_export]
macro_rules! klog {
    ($($arg:tt)*) => {
        $crate::log::log(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! klog_println {
    ()            => { $crate::klog!("\n") };
    ($($arg:tt)*) => { $crate::klog!("{}\n", format_args!($($arg)*)) };
}
