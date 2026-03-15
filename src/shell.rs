/// Simple kernel shell.
/// Processes keyboard input and dispatches commands.
/// Commands: help, info, clear, heap, panic

use crate::{print, println, serial_println, allocator};
use spin::Mutex;

const MAX_INPUT: usize = 80;

/// Static input buffer, written to by the keyboard interrupt handler.
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

/// Print the shell prompt.
pub fn prompt() {
    print!("merlion> ");
}

/// Called from the keyboard handler for each character.
pub fn handle_key(ch: char) {
    let mut input = INPUT.lock();

    match ch {
        '\n' => {
            println!();
            // Copy the input string for dispatch
            let cmd = core::str::from_utf8(&input.buf[..input.len])
                .unwrap_or("")
                .trim();
            if !cmd.is_empty() {
                dispatch(cmd);
            }
            input.len = 0;
            drop(input); // release lock before printing
            prompt();
        }
        '\x08' => {
            // Backspace
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
            println!("  help   - show this message");
            println!("  info   - system information");
            println!("  clear  - clear screen");
            println!("  heap   - heap allocator stats");
            println!("  panic  - trigger a kernel panic (test)");
        }
        "info" => {
            println!("MerlionOS v0.1.0");
            println!("Architecture: x86_64");
            println!("Heap size:    {}K", allocator::HEAP_SIZE / 1024);
        }
        "clear" => {
            crate::vga::WRITER.lock().clear();
        }
        "heap" => {
            let stats = allocator::stats();
            println!("Heap: {} used / {} free / {} total bytes",
                stats.used, stats.free, stats.total);
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
