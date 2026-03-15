/// Interactive kernel shell.
/// Processes keyboard input and dispatches commands.

use crate::{print, println, serial_println, allocator, timer, task, process, ipc, vfs, memory, driver, acpi, rtc, testutil};
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
            println!("Process commands:");
            println!("  ps         - list tasks");
            println!("  spawn      - spawn demo task");
            println!("  kill <pid> - kill a task");
            println!("  bg <prog>  - run user program (background)");
            println!("  run <prog> - run user program (blocking)");
            println!("  progs      - list user programs");
            println!("File commands:");
            println!("  ls [path]  - list directory");
            println!("  cat <path> - read file");
            println!("  write <path> <data> - write to file");
            println!("  rm <path>  - remove file");
            println!("System commands:");
            println!("  info       - system information");
            println!("  date       - current date and time");
            println!("  uptime     - time since boot");
            println!("  heap       - heap stats");
            println!("  memmap     - physical memory map");
            println!("  drivers    - list kernel drivers");
            println!("  pipe       - IPC demo");
            println!("  channels   - list IPC channels");
            println!("  dmesg      - kernel log");
            println!("  clear      - clear screen");
            println!("  test       - run kernel self-tests");
            println!("  shutdown   - power off");
            println!("  reboot     - restart");
            println!("  panic      - trigger panic");
        }
        "info" => {
            let mem = memory::stats();
            println!("\x1b[1mMerlionOS v0.1.0\x1b[0m");
            println!("Architecture: x86_64");
            println!("Physical RAM: {} KiB usable", mem.total_usable_bytes / 1024);
            println!("Heap size:    {}K", allocator::HEAP_SIZE / 1024);
            println!("PIT rate:     {} Hz", timer::PIT_FREQUENCY_HZ);
            println!("Drivers:      {}", driver::list().len());
            println!("Max tasks:    8");
        }
        "date" => {
            let dt = rtc::read();
            println!("{}", dt);
        }
        "uptime" => {
            let (h, m, s) = timer::uptime_hms();
            println!("Uptime: {:02}:{:02}:{:02} ({} ticks)", h, m, s, timer::ticks());
        }
        "heap" => {
            let stats = allocator::stats();
            println!("Heap: {} used / {} free / {} total bytes",
                stats.used, stats.free, stats.total);
        }
        "ps" => {
            println!("  PID  STATE    NAME");
            for t in task::list() {
                let st = match t.state {
                    task::TaskState::Running  => "running ",
                    task::TaskState::Ready    => "ready   ",
                    task::TaskState::Finished => "finished",
                };
                println!("  {:3}  {}  {}", t.pid, st, t.name);
            }
        }
        cmd if cmd.starts_with("kill ") => {
            if let Ok(pid) = cmd[5..].trim().parse::<usize>() {
                match task::kill(pid) {
                    Ok(()) => println!("Killed pid {}", pid),
                    Err(e) => println!("Error: {}", e),
                }
            } else {
                println!("Usage: kill <pid>");
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
            let name = cmd[4..].trim();
            match process::run_user_program(name) {
                Ok(()) => println!("Program '{}' finished.", name),
                Err(e) => println!("Error: {}", e),
            }
        }
        "run" => println!("Usage: run <program>"),
        cmd if cmd.starts_with("bg ") => {
            let name = cmd[3..].trim();
            match process::spawn_user_program(name) {
                Ok(pid) => println!("Background: '{}' (pid {})", name, pid),
                Err(e) => println!("Error: {}", e),
            }
        }

        // --- File commands ---
        "ls" => do_ls("/"),
        cmd if cmd.starts_with("ls ") => do_ls(cmd[3..].trim()),

        cmd if cmd.starts_with("cat ") => {
            let path = cmd[4..].trim();
            match vfs::cat(path) {
                Ok(content) => print!("{}", content),
                Err(e) => println!("cat: {}: {}", path, e),
            }
        }

        cmd if cmd.starts_with("write ") => {
            let rest = cmd[6..].trim();
            if let Some((path, data)) = rest.split_once(' ') {
                match vfs::write(path, data) {
                    Ok(()) => println!("Written {} bytes to {}", data.len(), path),
                    Err(e) => println!("write: {}", e),
                }
            } else {
                println!("Usage: write <path> <data>");
            }
        }

        cmd if cmd.starts_with("rm ") => {
            let path = cmd[3..].trim();
            match vfs::rm(path) {
                Ok(()) => println!("Removed {}", path),
                Err(e) => println!("rm: {}: {}", path, e),
            }
        }

        // --- System ---
        "memmap" => {
            let stats = memory::stats();
            println!("Physical memory: {} KiB usable, {} frames allocated, {} regions",
                stats.total_usable_bytes / 1024, stats.allocated_frames, stats.total_regions);
            println!();
            println!("  \x1b[1mSTART            END              SIZE      TYPE\x1b[0m");
            for r in memory::memory_map() {
                let color = match r.kind {
                    "usable" => "\x1b[32m",  // green
                    "kernel" | "kstack" | "pagetbl" => "\x1b[33m", // yellow
                    "reserved" | "ACPI" => "\x1b[90m", // gray
                    _ => "\x1b[0m",
                };
                println!("  {}{:#016x}  {:#016x}  {:>6}K  {}\x1b[0m",
                    color, r.start, r.end, r.size_kb, r.kind);
            }
        }
        "drivers" => {
            println!("  \x1b[1mNAME        KIND      STATUS\x1b[0m");
            for (name, kind, status) in driver::list() {
                println!("  {:<11} {:<9} \x1b[32m{}\x1b[0m", name, kind, status);
            }
        }
        "test" => {
            println!("\x1b[1m=== Kernel Self-Tests ===\x1b[0m");
            serial_println!("=== Kernel Self-Tests ===");
            let (passed, total) = testutil::run_all();
            serial_println!("{}/{} tests passed", passed, total);
        }
        "shutdown" => {
            println!("\x1b[33mShutting down...\x1b[0m");
            acpi::shutdown();
        }
        "reboot" => {
            println!("\x1b[33mRebooting...\x1b[0m");
            acpi::reboot();
        }
        "pipe" => run_ipc_demo(),
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
        "clear" => crate::vga::WRITER.lock().clear(),
        "panic" => panic!("user-triggered panic via shell"),
        _ => {
            println!("unknown command: {}", cmd);
            println!("type 'help' for commands");
        }
    }
}

fn do_ls(path: &str) {
    match vfs::ls(path) {
        Ok(entries) => {
            for (name, type_char) in entries {
                println!("  {} {}", type_char, name);
            }
        }
        Err(e) => println!("ls: {}: {}", path, e),
    }
}

fn demo_task() {
    for i in 1..=5 {
        serial_println!("[demo] iteration {}/5", i);
        println!("[demo] iteration {}/5", i);
        task::yield_now();
    }
    serial_println!("[demo] done");
    println!("[demo] task complete");
}

fn run_ipc_demo() {
    let ch_id = match ipc::create() {
        Some(id) => id,
        None => { println!("Failed to create channel"); return; }
    };
    println!("IPC demo: channel {} created", ch_id);
    DEMO_CHANNEL.store(ch_id, core::sync::atomic::Ordering::SeqCst);
    task::spawn("producer", producer_task);
    task::spawn("consumer", consumer_task);
    println!("Spawned producer and consumer tasks.");
}

static DEMO_CHANNEL: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

fn producer_task() {
    let ch = DEMO_CHANNEL.load(core::sync::atomic::Ordering::SeqCst);
    for msg in ["hello", "from", "producer"] {
        ipc::send_str(ch, msg);
        ipc::send(ch, b'\n');
        println!("[producer] sent: {}", msg);
        task::yield_now();
    }
    ipc::send(ch, 0);
    println!("[producer] done");
}

fn consumer_task() {
    let ch = DEMO_CHANNEL.load(core::sync::atomic::Ordering::SeqCst);
    task::yield_now();
    loop {
        let data = ipc::recv_all(ch);
        if data.is_empty() { task::yield_now(); continue; }
        if data.contains('\0') {
            let clean: alloc::string::String = data.chars().filter(|&c| c != '\0').collect();
            if !clean.is_empty() { println!("[consumer] received: {}", clean.trim()); }
            break;
        }
        println!("[consumer] received: {}", data.trim());
        task::yield_now();
    }
    ipc::destroy(ch);
    println!("[consumer] done, channel closed");
}
