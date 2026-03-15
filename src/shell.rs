/// Interactive kernel shell.
/// Processes keyboard input and dispatches commands.

use crate::{print, println, serial_println, allocator, timer, task, process, ipc};
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
            println!("  help     - show this message");
            println!("  info     - system information");
            println!("  uptime   - time since boot");
            println!("  heap     - heap allocator stats");
            println!("  ps       - list running tasks");
            println!("  spawn    - spawn a demo kernel task");
            println!("  run <p>  - run user program (blocking)");
            println!("  bg <p>   - run user program (background)");
            println!("  progs    - list user programs");
            println!("  pipe     - IPC demo (producer/consumer)");
            println!("  channels - list IPC channels");
            println!("  dmesg    - kernel log buffer");
            println!("  clear    - clear screen");
            println!("  panic    - trigger a kernel panic");
        }
        "info" => {
            println!("MerlionOS v0.1.0");
            println!("Architecture: x86_64");
            println!("Heap size:    {}K", allocator::HEAP_SIZE / 1024);
            println!("PIT rate:     {} Hz", timer::PIT_FREQUENCY_HZ);
            println!("Max tasks:    8");
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
            println!("Usage: run <program>  or  bg <program>");
        }
        cmd if cmd.starts_with("bg ") => {
            let prog_name = cmd[3..].trim();
            match process::spawn_user_program(prog_name) {
                Ok(pid) => println!("Background: '{}' (pid {})", prog_name, pid),
                Err(e) => println!("Error: {}", e),
            }
        }
        "pipe" => {
            run_ipc_demo();
        }
        "channels" => {
            let chs = ipc::list();
            if chs.is_empty() {
                println!("No active channels.");
            } else {
                println!("  ID  PENDING");
                for ch in chs {
                    println!("  {:2}  {} bytes", ch.id, ch.pending);
                }
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
        task::yield_now();
    }
    serial_println!("[demo] done");
    println!("[demo] task complete");
}

/// IPC demo: spawn a producer and consumer task communicating via a channel.
fn run_ipc_demo() {
    let ch_id = match ipc::create() {
        Some(id) => id,
        None => { println!("Failed to create channel"); return; }
    };

    println!("IPC demo: channel {} created", ch_id);
    serial_println!("[ipc] channel {} created for demo", ch_id);

    // Store channel ID in a static so tasks can access it
    DEMO_CHANNEL.store(ch_id, core::sync::atomic::Ordering::SeqCst);

    task::spawn("producer", producer_task);
    task::spawn("consumer", consumer_task);
    println!("Spawned producer and consumer tasks.");
}

static DEMO_CHANNEL: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

fn producer_task() {
    let ch = DEMO_CHANNEL.load(core::sync::atomic::Ordering::SeqCst);
    let messages = ["hello", "from", "producer"];
    for msg in messages {
        ipc::send_str(ch, msg);
        ipc::send(ch, b'\n');
        serial_println!("[producer] sent: {}", msg);
        println!("[producer] sent: {}", msg);
        task::yield_now();
    }
    // Send EOF marker
    ipc::send(ch, 0);
    serial_println!("[producer] done");
    println!("[producer] done");
}

fn consumer_task() {
    let ch = DEMO_CHANNEL.load(core::sync::atomic::Ordering::SeqCst);
    // Give producer a head start
    task::yield_now();

    loop {
        let data = ipc::recv_all(ch);
        if data.is_empty() {
            task::yield_now();
            continue;
        }
        // Check for EOF (null byte)
        if data.contains('\0') {
            let clean: alloc::string::String = data.chars().filter(|&c| c != '\0').collect();
            if !clean.is_empty() {
                serial_println!("[consumer] received: {}", clean.trim());
                println!("[consumer] received: {}", clean.trim());
            }
            break;
        }
        serial_println!("[consumer] received: {}", data.trim());
        println!("[consumer] received: {}", data.trim());
        task::yield_now();
    }

    // Cleanup
    ipc::destroy(ch);
    serial_println!("[consumer] channel closed, done");
    println!("[consumer] done, channel closed");
}
