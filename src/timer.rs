/// PIT (Programmable Interval Timer) tick counter.
/// The PIT fires at ~18.2 Hz by default (channel 0, mode 3, divisor 0).
/// We reprogram it to ~100 Hz for a cleaner tick rate.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::instructions::port::Port;

static TICKS: AtomicU64 = AtomicU64::new(0);

/// PIT frequency after reprogramming.
pub const PIT_FREQUENCY_HZ: u64 = 100;
const PIT_BASE_FREQUENCY: u64 = 1_193_182;
const PIT_DIVISOR: u16 = (PIT_BASE_FREQUENCY / PIT_FREQUENCY_HZ) as u16;

/// Program PIT channel 0 to fire at PIT_FREQUENCY_HZ.
pub fn init() {
    unsafe {
        // Channel 0, lobyte/hibyte, rate generator (mode 2)
        let mut cmd = Port::<u8>::new(0x43);
        cmd.write(0x34); // 0b00_11_010_0

        let mut data = Port::<u8>::new(0x40);
        data.write((PIT_DIVISOR & 0xFF) as u8);
        data.write((PIT_DIVISOR >> 8) as u8);
    }
}

/// Called by the timer interrupt handler on each tick.
pub fn tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Total ticks since boot.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Seconds since boot (approximate).
pub fn uptime_secs() -> u64 {
    ticks() / PIT_FREQUENCY_HZ
}

/// Formatted uptime as (hours, minutes, seconds).
pub fn uptime_hms() -> (u64, u64, u64) {
    let s = uptime_secs();
    (s / 3600, (s % 3600) / 60, s % 60)
}
