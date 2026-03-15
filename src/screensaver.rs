/// Matrix-style screensaver — falling green characters.
/// Activates after idle timeout or via 'matrix' command.
/// Press any key to exit.

use crate::{timer, keyboard::KeyEvent};
use core::sync::atomic::{AtomicBool, Ordering};

static RUNNING: AtomicBool = AtomicBool::new(false);

const COLS: usize = 80;
const ROWS: usize = 25;

pub fn is_running() -> bool { RUNNING.load(Ordering::SeqCst) }

pub fn handle_input(_event: KeyEvent) {
    RUNNING.store(false, Ordering::SeqCst);
}

pub fn run() {
    RUNNING.store(true, Ordering::SeqCst);
    let vga = 0xB8000 as *mut u8;

    // Column state: current row position for each column's "drop"
    let mut drops = [0u16; COLS];
    let mut rng: u32 = timer::ticks() as u32;

    // Initialize drops at random positions
    for col in 0..COLS {
        rng = lcg(rng);
        drops[col] = (rng % ROWS as u32) as u16;
    }

    // Clear screen to black
    for i in 0..COLS * ROWS {
        unsafe {
            vga.add(i * 2).write_volatile(b' ');
            vga.add(i * 2 + 1).write_volatile(0x00);
        }
    }

    let speed = 3; // ticks between frames

    while RUNNING.load(Ordering::SeqCst) {
        for col in 0..COLS {
            let row = drops[col] as usize;

            // Generate random character
            rng = lcg(rng);
            let ch = match (rng >> 16) % 4 {
                0 => b'0' + ((rng >> 8) % 10) as u8,           // digit
                1 => b'A' + ((rng >> 12) % 26) as u8,          // letter
                2 => [b'@', b'#', b'$', b'%', b'&', b'*', b'+', b'=']
                     [((rng >> 4) % 8) as usize],               // symbol
                _ => b'0' + ((rng >> 6) % 10) as u8,
            };

            if row < ROWS {
                // Bright green head
                let offset = (row * COLS + col) * 2;
                unsafe {
                    vga.add(offset).write_volatile(ch);
                    vga.add(offset + 1).write_volatile(0x0A); // bright green
                }

                // Dim the character above (trail effect)
                if row > 0 {
                    let above = ((row - 1) * COLS + col) * 2;
                    unsafe {
                        vga.add(above + 1).write_volatile(0x02); // dark green
                    }
                }

                // Fade out characters further up
                if row > 4 {
                    let fade = ((row - 5) * COLS + col) * 2;
                    unsafe {
                        vga.add(fade).write_volatile(b' ');
                        vga.add(fade + 1).write_volatile(0x00);
                    }
                }
            }

            // Advance drop
            drops[col] += 1;

            // Reset drop at random intervals
            rng = lcg(rng);
            if drops[col] as usize >= ROWS + 5 || (rng % 100) < 3 {
                rng = lcg(rng);
                drops[col] = 0;
                // Random speed variation: sometimes skip a position
                if rng % 3 == 0 {
                    drops[col] = (rng % 3) as u16;
                }
            }
        }

        // Wait for next frame
        let target = timer::ticks() + speed;
        while timer::ticks() < target && RUNNING.load(Ordering::SeqCst) {
            x86_64::instructions::hlt();
        }
    }

    RUNNING.store(false, Ordering::SeqCst);
}

fn lcg(state: u32) -> u32 {
    state.wrapping_mul(1103515245).wrapping_add(12345)
}
