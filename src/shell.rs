/// Interactive kernel shell with command history.
/// Supports arrow keys (up/down for history, left/right planned),
/// shift for uppercase, and output redirection (cmd > file).

use crate::{print, println, serial_println, vfs, env, ipc, task, memory, allocator, timer, rtc, smp};
use crate::keyboard::KeyEvent;
use spin::Mutex;

const MAX_INPUT: usize = 80;
const HISTORY_SIZE: usize = 16;

static SHELL: Mutex<ShellState> = Mutex::new(ShellState::new());

struct ShellState {
    buf: [u8; MAX_INPUT],
    len: usize,
    history: [[u8; MAX_INPUT]; HISTORY_SIZE],
    history_lens: [usize; HISTORY_SIZE],
    history_count: usize,
    history_pos: usize, // browsing position (history_count = current input)
}

impl ShellState {
    const fn new() -> Self {
        Self {
            buf: [0; MAX_INPUT],
            len: 0,
            history: [[0; MAX_INPUT]; HISTORY_SIZE],
            history_lens: [0; HISTORY_SIZE],
            history_count: 0,
            history_pos: 0,
        }
    }

    fn push_history(&mut self) {
        if self.len == 0 { return; }
        let idx = self.history_count % HISTORY_SIZE;
        self.history[idx][..self.len].copy_from_slice(&self.buf[..self.len]);
        self.history_lens[idx] = self.len;
        self.history_count += 1;
        self.history_pos = self.history_count;
    }

    fn clear_line(&mut self) {
        // Erase current input on screen
        for _ in 0..self.len {
            print!("\x08 \x08");
        }
        self.len = 0;
    }

    fn set_from_history(&mut self, idx: usize) {
        self.clear_line();
        let hist_idx = idx % HISTORY_SIZE;
        let hlen = self.history_lens[hist_idx];
        self.buf[..hlen].copy_from_slice(&self.history[hist_idx][..hlen]);
        self.len = hlen;
        if let Ok(s) = core::str::from_utf8(&self.buf[..self.len]) {
            print!("{}", s);
        }
    }
}

pub fn prompt() {
    print!("merlion> ");
}

/// Handle a keyboard event from the interrupt handler.
pub fn handle_key_event(event: KeyEvent) {
    let mut shell = SHELL.lock();

    match event {
        KeyEvent::Char('\n') => {
            println!();
            // Copy command to a local buffer to avoid borrow issues
            let mut cmd_buf = [0u8; MAX_INPUT];
            let cmd_len = shell.len;
            cmd_buf[..cmd_len].copy_from_slice(&shell.buf[..cmd_len]);
            shell.push_history();
            shell.len = 0;
            shell.history_pos = shell.history_count;
            drop(shell);

            let cmd = core::str::from_utf8(&cmd_buf[..cmd_len])
                .unwrap_or("")
                .trim();
            if !cmd.is_empty() {
                // Expand $VAR references
                let expanded = env::expand(cmd);
                let cmd = expanded.trim();
                // Check alias
                let resolved = if let Some(alias_cmd) = env::resolve_alias(cmd) {
                    alias_cmd
                } else {
                    alloc::string::String::from(cmd)
                };
                let cmd = resolved.trim();

                // Semicolon chaining: cmd1 ; cmd2 ; cmd3
                if cmd.contains(';') {
                    for sub in cmd.split(';') {
                        let sub = sub.trim();
                        if !sub.is_empty() {
                            execute_single(sub);
                        }
                    }
                } else if cmd.contains('|') {
                    // Pipe: cmd1 | cmd2 | cmd3
                    crate::pipe_exec::execute_pipeline(cmd);
                } else {
                    execute_single(cmd);
                }
            }
            prompt();
        }
        KeyEvent::Char('\x08') => {
            if shell.len > 0 {
                shell.len -= 1;
                print!("\x08 \x08");
            }
        }
        KeyEvent::Char(ch) if ch.is_ascii() && !ch.is_ascii_control() => {
            let len = shell.len;
            if len < MAX_INPUT {
                shell.buf[len] = ch as u8;
                shell.len = len + 1;
                print!("{}", ch);
            }
        }
        KeyEvent::ArrowUp => {
            if shell.history_pos > 0 && shell.history_count > 0 {
                let new_pos = shell.history_pos - 1;
                if new_pos < shell.history_count && shell.history_count - new_pos <= HISTORY_SIZE {
                    shell.history_pos = new_pos;
                    shell.set_from_history(new_pos);
                }
            }
        }
        KeyEvent::ArrowDown => {
            if shell.history_pos < shell.history_count {
                let new_pos = shell.history_pos + 1;
                shell.history_pos = new_pos;
                if new_pos < shell.history_count {
                    shell.set_from_history(new_pos);
                } else {
                    shell.clear_line();
                }
            }
        }
        _ => {}
    }
}

/// Execute a single command, handling output redirection.
fn execute_single(cmd: &str) {
    if let Some((left, right)) = cmd.split_once(" > ") {
        dispatch(left.trim());
        let _ = vfs::write(right.trim(), left.trim());
    } else {
        dispatch(cmd);
    }
}

pub fn dispatch(cmd: &str) {
    serial_println!("shell: {}", cmd);

    // "help" is handled here since it calls into shell_cmds for the text
    if cmd == "help" {
        crate::shell_cmds::help_text();
        return;
    }

    if crate::shell_cmds::dispatch_process(cmd) { return; }
    if crate::shell_cmds::dispatch_file(cmd) { return; }
    if crate::shell_cmds::dispatch_system(cmd) { return; }
    if crate::shell_cmds::dispatch_ai(cmd) { return; }
    if crate::shell_cmds::dispatch_security(cmd) { return; }
    if crate::shell_cmds::dispatch_network(cmd) { return; }
    if crate::shell_cmds::dispatch_hardware(cmd) { return; }
    if crate::shell_cmds::dispatch_advanced(cmd) { return; }
    if crate::shell_cmds::dispatch_apps(cmd) { return; }
    if crate::shell_cmds::dispatch_misc(cmd) { return; }

    println!("unknown command: {}", cmd);
    println!("type 'help' for commands");
}

// --- Helper functions called from shell_cmds via super:: ---

pub(crate) fn do_ls(path: &str) {
    match vfs::ls(path) {
        Ok(entries) => {
            for (name, type_char) in entries {
                println!("  {} {}", type_char, name);
            }
        }
        Err(e) => println!("ls: {}: {}", path, e),
    }
}

pub(crate) fn demo_task() {
    for i in 1..=5 {
        serial_println!("[demo] iteration {}/5", i);
        println!("[demo] iteration {}/5", i);
        task::yield_now();
    }
    serial_println!("[demo] done");
    println!("[demo] task complete");
}

pub(crate) fn run_ipc_demo() {
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

/// Print command history (called from shell_cmds)
pub(crate) fn print_history() {
    let shell = SHELL.lock();
    let start = if shell.history_count > HISTORY_SIZE {
        shell.history_count - HISTORY_SIZE
    } else { 0 };
    for i in start..shell.history_count {
        let idx = i % HISTORY_SIZE;
        let len = shell.history_lens[idx];
        if let Ok(s) = core::str::from_utf8(&shell.history[idx][..len]) {
            println!("  {:3}  {}", i + 1, s);
        }
    }
}

pub(crate) fn neofetch() {
    let features = smp::detect_features();
    let mem = memory::stats();
    let heap = allocator::stats();
    let (h, m, s) = timer::uptime_hms();
    let dt = rtc::read();
    let user = env::get("USER").unwrap_or_else(|| alloc::string::String::from("root"));
    let host = env::get("HOSTNAME").unwrap_or_else(|| alloc::string::String::from("merlion"));
    let tasks = task::list();

    // Logo on the left, info on the right
    println!("\x1b[36m  ▄▄▄      ▄▄▄       \x1b[0m {}@{}", user, host);
    println!("\x1b[36m  ████▄  ▄████       \x1b[0m ─────────────────────");
    println!("\x1b[36m  ███▀████▀███       \x1b[0m \x1b[36mOS\x1b[0m:      MerlionOS 1.0.0");
    println!("\x1b[36m  ███  ▀▀  ███       \x1b[0m \x1b[36mMotto\x1b[0m:   Born for AI. Built by AI.");
    println!("\x1b[36m  ███      ███       \x1b[0m \x1b[36mCPU\x1b[0m:     {}", features.brand);
    println!("                      \x1b[36mMemory\x1b[0m:  {} KiB / {} KiB",
        mem.allocated_frames * 4, mem.total_usable_bytes / 1024);
    println!("                      \x1b[36mHeap\x1b[0m:    {} / {} bytes",
        heap.used, heap.total);
    println!("                      \x1b[36mUptime\x1b[0m:  {:02}:{:02}:{:02}", h, m, s);
    println!("                      \x1b[36mDate\x1b[0m:    {}", dt);
    println!("                      \x1b[36mTasks\x1b[0m:   {}", tasks.len());
    println!("                      \x1b[36mShell\x1b[0m:   msh (MerlionOS Shell)");
    println!();
    // Color palette
    print!("                      ");
    for i in 0..8u8 {
        print!("\x1b[{}m  █\x1b[0m", 30 + i);
    }
    println!();
}
