/// `watch` — execute a command repeatedly at a fixed interval.
/// Like Linux `watch`: clears screen, runs command, shows timestamp.
///
///   watch 2 uptime      — run 'uptime' every 2 seconds
///   watch 1 ps          — run 'ps' every second
///   watch 5 free        — run 'free' every 5 seconds
///
/// Press 'q' to stop.

use crate::{println, print, timer, rtc, keyboard::KeyEvent};
use core::sync::atomic::{AtomicBool, Ordering};

static RUNNING: AtomicBool = AtomicBool::new(false);

pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

pub fn handle_input(event: KeyEvent) {
    if let KeyEvent::Char('q') = event {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

/// Run a command every `interval` seconds until 'q' is pressed.
pub fn run(interval: u64, command: &str) {
    RUNNING.store(true, Ordering::SeqCst);

    let ticks_per_interval = interval * timer::PIT_FREQUENCY_HZ;
    let mut iteration = 0u64;

    while RUNNING.load(Ordering::SeqCst) {
        // Clear screen
        let vga = 0xB8000 as *mut u8;
        for i in 0..80 * 25 {
            unsafe {
                vga.add(i * 2).write_volatile(b' ');
                vga.add(i * 2 + 1).write_volatile(0x07);
            }
        }
        // Reset VGA writer position
        {
            let mut w = crate::vga::WRITER.lock();
            w.set_attr(crate::vga::color_attr(crate::vga::Color::LightGray, crate::vga::Color::Black));
        }

        // Header bar
        let dt = rtc::read();
        let header = alloc::format!(
            " Every {}s: {}  |  {}  |  #{}  |  q=quit",
            interval, command, dt, iteration
        );
        // Write header in inverse colors
        for (x, byte) in header.bytes().enumerate() {
            if x >= 80 { break; }
            unsafe {
                vga.add(x * 2).write_volatile(byte);
                vga.add(x * 2 + 1).write_volatile(0x70); // inverse
            }
        }
        for x in header.len()..80 {
            unsafe {
                vga.add(x * 2).write_volatile(b' ');
                vga.add(x * 2 + 1).write_volatile(0x70);
            }
        }

        // Position cursor at row 2
        {
            let mut w = crate::vga::WRITER.lock();
            // Hack: set internal position by writing newlines
            // Better: directly set row/col
        }
        // Write output starting at row 2 by setting VGA writer
        // For simplicity, dispatch command which writes to VGA
        // We need to reset writer to row 2, col 0
        reset_cursor_to(2, 0);
        crate::shell::dispatch(command);

        iteration += 1;

        // Wait for interval
        let target = timer::ticks() + ticks_per_interval;
        while timer::ticks() < target && RUNNING.load(Ordering::SeqCst) {
            x86_64::instructions::hlt();
        }
    }

    RUNNING.store(false, Ordering::SeqCst);
}

fn reset_cursor_to(row: usize, col: usize) {
    // Directly manipulate the VGA writer's internal state
    // Since Writer fields are private, we use a workaround:
    // clear and write empty lines to reach the target row
    let mut w = crate::vga::WRITER.lock();
    // Access via the public clear + write_byte methods
    drop(w); // we already cleared the screen above
    // The next println! will write at wherever the writer cursor is
    // Since we cleared the VGA buffer manually, reset writer
    // For now, output goes after the header naturally
}
