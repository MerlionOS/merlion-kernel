/// Interactive kernel shell.
/// Processes keyboard input and dispatches commands.

use crate::{print, println, serial_println, allocator, timer};
use spin::Mutex;

const MAX_INPUT: usize = 80;

static INPUT: Mutex<InputBuffer> = Mutex::new(InputBuffer::new());

struct InputBuffer {
    buf: [u8; MAX_INPUT],
    len: usize,
}

impl InputBuffer {
    const fn new() -> Self {
        Self { buf: [0; MAX_INPUT], len: 0 }
    }
}

pub fn prompt() {
    print!("merlion> ");
}

/// Called from the keyboard handler for each character.
pub fn handle_key(ch: char) {
    let mut input = INPUT.lock();

    match ch {
        '\n' => {
            println!();
            let cmd = core::str::from_utf8(&input.buf[..input.len])
                .unwrap_or("")
                .trim();
            if !cmd.is_empty() {
                dispatch(cmd);
            }
            input.len = 0;
            drop(input);
            prompt();
        }
        '\x08' => {
            if input.len > 0 {
                input.len -= 1;
                print!("\x08");
            }
        }
        ch if ch.is_ascii() && !ch.is_ascii_control() => {
            let len = input.len;
            if len < MAX_INPUT {
                input.buf[len] = ch as u8;
                input.len = len + 1;
                print!("{}", ch);
            }
        }
        _ => {}
    }
}

fn dispatch(cmd: &str) {
    serial_println!("shell: {}", cmd);

    match cmd {
        "help" => {
            println!("Available commands:");
            println!("  help    - show this message");
            println!("  info    - system information");
            println!("  uptime  - time since boot");
            println!("  heap    - heap allocator stats");
            println!("  dmesg   - kernel log buffer");
            println!("  clear   - clear screen");
            println!("  umode   - test user-mode transition");
            println!("  panic   - trigger a kernel panic");
        }
        "info" => {
            println!("MerlionOS v0.1.0");
            println!("Architecture: x86_64");
            println!("Heap size:    {}K", allocator::HEAP_SIZE / 1024);
            println!("PIT rate:     {} Hz", timer::PIT_FREQUENCY_HZ);
        }
        "uptime" => {
            let (h, m, s) = timer::uptime_hms();
            let ticks = timer::ticks();
            println!("Uptime: {:02}:{:02}:{:02} ({} ticks)", h, m, s, ticks);
        }
        "heap" => {
            let stats = allocator::stats();
            println!("Heap: {} used / {} free / {} total bytes",
                stats.used, stats.free, stats.total);
        }
        "dmesg" => {
            crate::log::KLOG.lock().read(|chunk| {
                if let Ok(s) = core::str::from_utf8(chunk) {
                    print!("{}", s);
                }
            });
        }
        "clear" => {
            crate::vga::WRITER.lock().clear();
        }
        "umode" => {
            println!("Entering user-mode (ring 3)...");
            crate::usermode::enter_usermode();
            println!("Returned from user-mode to kernel (ring 0).");
        }
        "panic" => {
            panic!("user-triggered panic via shell");
        }
        _ => {
            println!("unknown command: {}", cmd);
            println!("type 'help' for available commands");
        }
    }
}
