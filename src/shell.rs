/// Interactive kernel shell.
/// Processes keyboard input and dispatches commands.

use crate::{print, println, serial_println, allocator, timer, task, process};
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
            println!("  ps      - list running tasks");
            println!("  spawn   - spawn a demo kernel task");
            println!("  run <p> - run a user program (hello, counter)");
            println!("  progs   - list available user programs");
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
        "ps" => {
            println!("  PID  STATE    NAME");
            for t in task::list() {
                let state_str = match t.state {
                    task::TaskState::Running  => "running ",
                    task::TaskState::Ready    => "ready   ",
                    task::TaskState::Finished => "finished",
                };
                println!("  {:3}  {}  {}", t.pid, state_str, t.name);
            }
        }
        "spawn" => {
            if let Some(pid) = task::spawn("demo", demo_task) {
                println!("Spawned demo task (pid {})", pid);
            } else {
                println!("Task table full!");
            }
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
        "progs" => {
            println!("Available user programs:");
            for name in process::list_programs() {
                println!("  {}", name);
            }
        }
        cmd if cmd.starts_with("run ") => {
            let prog_name = cmd[4..].trim();
            match process::run_user_program(prog_name) {
                Ok(()) => println!("Program '{}' finished.", prog_name),
                Err(e) => println!("Error: {}", e),
            }
        }
        "run" => {
            println!("Usage: run <program>");
            println!("Available: {:?}", process::list_programs());
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

/// Demo task: prints messages, yields between each, then exits.
fn demo_task() {
    for i in 1..=5 {
        serial_println!("[demo] iteration {}/5", i);
        println!("[demo] iteration {}/5", i);
        // Yield to let other tasks run
        task::yield_now();
    }
    serial_println!("[demo] done");
    println!("[demo] task complete");
}
