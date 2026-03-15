/// Interactive kernel shell with command history.
/// Supports arrow keys (up/down for history, left/right planned),
/// shift for uppercase, and output redirection (cmd > file).

use crate::{print, println, serial_println, allocator, timer, task, process, ipc, vfs, memory, driver, acpi, rtc, testutil, framebuf, pci, ramdisk, net, netproto, smp, env, module, slab, ksyms, paging, virtio, virtio_blk, virtio_net, blkdev, fat, fd, locks, ai_shell, ai_proxy, ai_monitor, ai_syscall, ai_heal, ai_man, semfs, agent, script, signal, kconfig, tcp, elf, elf_loader, boot_info_ext, demo, snake, diskfs, editor, top, calc, coreutils, chat, fortune, bench, ahci, xhci, e1000e, ioapic, http, dhcp};
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

    match cmd {
        "help" => {
            println!("Process commands:");
            println!("  ps         - list tasks");
            println!("  spawn      - spawn demo task");
            println!("  kill <pid> - kill a task");
            println!("  signal <pid> <sig> - send signal");
            println!("  bg <prog>  - run user program (background)");
            println!("  run <prog> - run user program (blocking)");
            println!("  progs      - list user programs");
            println!("File commands:");
            println!("  ls [path]  - list directory");
            println!("  cat <path> - read file");
            println!("  write <path> <data> - write to file");
            println!("  rm <path>  - remove file");
            println!("  wc <path>  - count lines/bytes");
            println!("  edit <path> - text editor (Ctrl+S save, Ctrl+Q quit)");
            println!("  grep <pat> <path> - search in file");
            println!("  head <n> <path> - first N lines");
            println!("  tail <n> <path> - last N lines");
            println!("  sort <path> - sort lines");
            println!("  hexdump <path> - hex + ASCII dump");
            println!("  readelf    - parse kernel ELF header");
            println!("  mkelf <p>  - build ELF from user program");
            println!("  loadelf <s> <sz> - load ELF from disk sector");
            println!("  exec <path> - run shell script");
            println!("  open <p>   - open file descriptor");
            println!("  close <fd> - close file descriptor");
            println!("  lsof       - list open file descriptors");
            println!("System commands:");
            println!("  info       - system information");
            println!("  date       - current date and time");
            println!("  uptime     - time since boot");
            println!("  heap       - heap stats");
            println!("  free       - memory summary");
            println!("  slabinfo   - slab allocator caches");
            println!("  bt         - stack backtrace");
            println!("  stackcheck - verify task stack guards");
            println!("  heapcheck  - verify heap integrity");
            println!("  config     - show kernel config");
            println!("  setconf K=V - set kernel config");
            println!("  lockdemo   - spinlock vs ticket lock");
            println!("AI commands:");
            println!("  ai <text>  - ask the AI (proxy or keyword)");
            println!("  monitor    - AI system health check");
            println!("  tag <p> <t> - tag a file");
            println!("  tags <p>   - show file tags");
            println!("  search <q> - search files by tag");
            println!("  explain <t> - explain a kernel concept");
            println!("  man <cmd>  - AI manual page");
            println!("  bootinfo   - boot method and arch");
            println!("  heal       - AI self-healing diagnosis");
            println!("  agents     - list AI agents");
            println!("  ask <a> <m> - send message to an agent");
            println!("  chat       - interactive AI conversation");
            println!("  fortune    - random tip or fact");
            println!("  aistatus   - AI subsystem status");
            println!("  memmap     - physical memory map");
            println!("  drivers    - list kernel drivers");
            println!("  lsmod      - list kernel modules");
            println!("  modprobe <m> - load a module");
            println!("  rmmod <m>  - unload a module");
            println!("  modinfo <m> - module details");
            println!("  pipe       - IPC demo");
            println!("  channels   - list IPC channels");
            println!("  dmesg      - kernel log");
            println!("  clear      - clear screen");
            println!("  cpuinfo    - CPU features and cores");
            println!("  ifconfig   - network interface info");
            println!("  send <msg> - send UDP loopback packet");
            println!("  recv       - receive queued packets");
            println!("  ping <ip>  - ping an address");
            println!("  arp        - ARP table");
            println!("  arpreq <ip> - send real ARP request");
            println!("  rawping <ip> - send real ICMP ping");
            println!("  tcpconn <ip:port> - TCP connect");
            println!("  tcpsend <id> <d> - TCP send");
            println!("  tcprecv <id> - TCP receive");
            println!("  tcpclose <id> - TCP close");
            println!("  netstat    - TCP connections");
            println!("  virtio     - virtio devices");
            println!("  blkdevs    - block devices");
            println!("  diskread <s> - read sector from virtio disk");
            println!("  diskwrite <s> <d> - write to virtio disk");
            println!("  diskfmt    - format disk as MF16 (persistent)");
            println!("  diskls     - list files on disk");
            println!("  disksave <n> <d> - save file to disk");
            println!("  diskload <n> - load file from disk");
            println!("  diskrm <n>  - delete file from disk");
            println!("  diskinfo   - disk filesystem status");
            println!("  fatfmt     - format RAM disk as MF16");
            println!("  fatls      - list MF16 files");
            println!("  fatw <n> <d> - write MF16 file");
            println!("  fatr <n>   - read MF16 file");
            println!("  lspci      - list PCI devices");
            println!("  disk       - RAM disk status");
            println!("  format     - format RAM disk");
            println!("  dsave <n> <d> - save file to disk");
            println!("  dload <n>  - load file from disk");
            println!("  dls        - list disk files");
            println!("  echo <msg> - print a message");
            println!("  env        - environment variables");
            println!("  set K=V    - set variable");
            println!("  alias n=c  - set alias");
            println!("  neofetch   - system summary");
            println!("  uname      - kernel version");
            println!("  whoami     - current user");
            println!("  hostname   - system hostname");
            println!("  history    - command history");
            println!("  sleep <n>  - sleep for n seconds");
            println!("  gfx        - graphics demo (160x50)");
            println!("  test       - run kernel self-tests");
            println!("  top        - live system monitor");
            println!("  calc <exp> - calculator (+ - * / %)");
            println!("  bench      - system performance benchmark");
            println!("  about      - about MerlionOS");
            println!("  version    - version and build info");
            println!("  demo       - run full system demo");
            println!("  snake      - play Snake game!");
            println!("  shutdown   - power off");
            println!("  reboot     - restart");
            println!("  panic      - trigger panic");
            println!("Hardware:");
            println!("  wget <url> - build HTTP request (TCP pending)");
            println!("  ifup       - DHCP discover sequence");
            println!("  dns <host> - resolve hostname");
            println!("  ahciinfo   - AHCI controller status");
            println!("  usbdevs    - list USB devices");
            println!("  ioapicinfo - IOAPIC status");
            println!("  e1000info  - e1000e NIC status");
        }
        "info" => {
            let mem = memory::stats();
            println!("\x1b[1mMerlionOS v1.0.0\x1b[0m");
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
        "free" => {
            let mem = memory::stats();
            let heap = allocator::stats();
            println!("              \x1b[1mtotal       used       free\x1b[0m");
            println!("Phys:    {:>8} K  {:>8} K  {:>8} K",
                mem.total_usable_bytes / 1024,
                mem.allocated_frames * 4,
                (mem.total_usable_bytes / 1024) - (mem.allocated_frames * 4));
            println!("Heap:    {:>8}    {:>8}    {:>8}",
                heap.total, heap.used, heap.free);
            let rd = ramdisk::RAMDISK.lock();
            if rd.is_formatted() {
                let disk_total = 128 * 1024 - 16 * 512;
                let disk_used = rd.used_bytes();
                println!("Disk:    {:>8}    {:>8}    {:>8}",
                    disk_total, disk_used, disk_total - disk_used);
            }
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
        cmd if cmd.starts_with("grep ") => {
            let rest = cmd[5..].trim();
            if let Some((pattern, path)) = rest.split_once(' ') {
                match vfs::cat(path.trim()) {
                    Ok(content) => {
                        let matches = coreutils::grep_n(pattern, &content);
                        if matches.is_empty() {
                            println!("(no matches)");
                        } else {
                            for line in matches { println!("{}", line); }
                        }
                    }
                    Err(e) => println!("grep: {}: {}", path, e),
                }
            } else {
                println!("Usage: grep <pattern> <path>");
            }
        }
        cmd if cmd.starts_with("head ") => {
            let rest = cmd[5..].trim();
            if let Some((n_str, path)) = rest.split_once(' ') {
                let n = n_str.parse::<usize>().unwrap_or(10);
                match vfs::cat(path.trim()) {
                    Ok(content) => {
                        for line in coreutils::head(&content, n) { println!("{}", line); }
                    }
                    Err(e) => println!("head: {}", e),
                }
            } else {
                println!("Usage: head <n> <path>");
            }
        }
        cmd if cmd.starts_with("tail ") => {
            let rest = cmd[5..].trim();
            if let Some((n_str, path)) = rest.split_once(' ') {
                let n = n_str.parse::<usize>().unwrap_or(10);
                match vfs::cat(path.trim()) {
                    Ok(content) => {
                        for line in coreutils::tail(&content, n) { println!("{}", line); }
                    }
                    Err(e) => println!("tail: {}", e),
                }
            } else {
                println!("Usage: tail <n> <path>");
            }
        }
        cmd if cmd.starts_with("sort ") => {
            let path = cmd[5..].trim();
            match vfs::cat(path) {
                Ok(content) => {
                    for line in coreutils::sort(&content) { println!("{}", line); }
                }
                Err(e) => println!("sort: {}", e),
            }
        }
        cmd if cmd.starts_with("hexdump ") => {
            let path = cmd[8..].trim();
            match vfs::cat(path) {
                Ok(content) => print!("{}", coreutils::hexdump(content.as_bytes(), 256)),
                Err(e) => println!("hexdump: {}", e),
            }
        }
        cmd if cmd.starts_with("edit ") => {
            let path = cmd[5..].trim();
            editor::open(path);
            // Editor runs until Ctrl+Q, then keyboard handler returns to shell
            // Wait for editor to close
            while editor::is_editing() {
                x86_64::instructions::hlt();
            }
            // Restore shell display
            crate::vga::print_banner();
            println!("Editor closed.");
        }
        cmd if cmd.starts_with("wc ") => {
            let path = cmd[3..].trim();
            match vfs::cat(path) {
                Ok(content) => {
                    let lines = content.lines().count();
                    let bytes = content.len();
                    let words = content.split_whitespace().count();
                    println!("  {} lines, {} words, {} bytes  {}", lines, words, bytes, path);
                }
                Err(e) => println!("wc: {}: {}", path, e),
            }
        }
        cmd if cmd.starts_with("mkelf ") => {
            let name = cmd[6..].trim();
            match process::get_program(name) {
                Some(code) => {
                    let elf_data = elf_loader::build_elf(code);
                    println!("Built ELF for '{}': {} bytes", name, elf_data.len());

                    // Parse it back to verify
                    match elf::parse(&elf_data) {
                        Ok(info) => print!("{}", elf::format_info(&info)),
                        Err(e) => println!("Parse error: {}", e),
                    }

                    // Write to disk if available
                    if virtio_blk::is_detected() {
                        // Write ELF to disk starting at sector 100
                        let sectors = (elf_data.len() + 511) / 512;
                        for i in 0..sectors {
                            let mut buf = [0u8; 512];
                            let start = i * 512;
                            let end = (start + 512).min(elf_data.len());
                            buf[..end - start].copy_from_slice(&elf_data[start..end]);
                            let _ = virtio_blk::write_sector(100 + i as u64, &buf);
                        }
                        println!("Written to disk sectors 100-{} ({} sectors)",
                            100 + sectors - 1, sectors);
                        println!("Load with: loadelf 100 {}", elf_data.len());
                    }
                }
                None => println!("Unknown program: {} (try: hello, counter)", name),
            }
        }
        cmd if cmd.starts_with("loadelf ") => {
            let rest = cmd[8..].trim();
            if let Some((sec_str, sz_str)) = rest.split_once(' ') {
                let sector = sec_str.parse::<u64>().unwrap_or(0);
                let size = sz_str.parse::<usize>().unwrap_or(0);
                if size == 0 {
                    println!("Usage: loadelf <sector> <size>");
                } else {
                    match elf_loader::load_from_disk(sector, size) {
                        Ok(data) => {
                            println!("Loaded {} bytes from sector {}", data.len(), sector);
                            match elf::parse(&data) {
                                Ok(info) => {
                                    print!("{}", elf::format_info(&info));
                                    println!("Executing...");
                                    match elf_loader::load_and_exec("disk-elf", &data) {
                                        Ok(()) => println!("ELF execution finished."),
                                        Err(e) => println!("Exec error: {}", e),
                                    }
                                }
                                Err(e) => println!("ELF parse error: {}", e),
                            }
                        }
                        Err(e) => println!("Load error: {}", e),
                    }
                }
            } else {
                println!("Usage: loadelf <start_sector> <size_bytes>");
            }
        }
        "readelf" => {
            // Parse the running kernel binary (we can read from the kernel's own
            // memory region — the ELF header is at the beginning of the binary).
            // For demo, construct a minimal ELF header description.
            println!("Kernel binary: ELF x86_64 executable");
            println!("  Entry:   _start (bootloader entry_point! macro)");
            println!("  Format:  ELF-64, little-endian, static");
            println!("  Target:  x86_64-unknown-none");
            println!("  Modules: {} source files", 54);
        }
        cmd if cmd.starts_with("exec ") => {
            let path = cmd[5..].trim();
            match script::run_script(path) {
                Ok(n) => println!("Executed {} commands from {}", n, path),
                Err(e) => println!("exec: {}", e),
            }
        }
        cmd if cmd.starts_with("open ") => {
            let path = cmd[5..].trim();
            match fd::open(path) {
                Ok(n) => println!("fd {} opened for {}", n, path),
                Err(e) => println!("open: {}", e),
            }
        }
        cmd if cmd.starts_with("close ") => {
            if let Ok(n) = cmd[6..].trim().parse::<usize>() {
                match fd::close(n) {
                    Ok(()) => println!("fd {} closed", n),
                    Err(e) => println!("close: {}", e),
                }
            }
        }
        "lsof" => {
            let fds = fd::list_open();
            if fds.is_empty() {
                println!("No open file descriptors.");
            } else {
                println!("  \x1b[1mFD  TYPE    PATH\x1b[0m");
                for (n, path, kind) in fds {
                    println!("  {:2}  {:<7} {}", n, kind, path);
                }
            }
        }
        "slabinfo" => {
            let caches = slab::stats();
            if caches.is_empty() {
                println!("No slab caches.");
            } else {
                println!("  \x1b[1mNAME         SIZE  CAPACITY  IN_USE  ALLOC  FREE\x1b[0m");
                for c in caches {
                    println!("  {:<12} {:>4}  {:>8}  {:>6}  {:>5}  {:>4}",
                        c.name, c.obj_size, c.capacity, c.in_use, c.allocated, c.freed);
                }
            }
            let ps = paging::stats();
            println!("  Demand paging: {} faulted-in / {} preallocated",
                ps.pages_faulted_in, ps.pages_preallocated);
        }
        "lockdemo" => {
            println!("\x1b[1mLock comparison (100 acquires each):\x1b[0m");
            let stats = locks::demo();
            println!("  \x1b[1mNAME          TYPE     ACQUIRES  SPINS  AVG\x1b[0m");
            for s in stats {
                println!("  {:<13} {:<8} {:>8}  {:>5}  {:>3}",
                    s.name, s.kind, s.acquires, s.total_spins, s.avg_spins);
            }
        }
        cmd if cmd.starts_with("signal ") => {
            let rest = cmd[7..].trim();
            if let Some((pid_str, sig_str)) = rest.split_once(' ') {
                if let Ok(pid) = pid_str.trim().parse::<usize>() {
                    if let Some(sig) = signal::parse(sig_str.trim()) {
                        match signal::send_signal(pid, sig) {
                            Ok(()) => println!("Sent {} to pid {}", signal::name(sig), pid),
                            Err(e) => println!("signal: {}", e),
                        }
                    } else {
                        println!("Unknown signal: {}", sig_str);
                    }
                } else {
                    println!("Usage: signal <pid> <signal>");
                }
            } else {
                println!("Usage: signal <pid> <SIGKILL|SIGTERM|9|15>");
            }
        }
        "config" => {
            let entries = kconfig::list();
            if entries.is_empty() {
                println!("No config loaded.");
            } else {
                for (k, v) in entries {
                    println!("  {}={}", k, v);
                }
            }
        }
        cmd if cmd.starts_with("setconf ") => {
            let rest = cmd[8..].trim();
            if let Some((key, val)) = rest.split_once('=') {
                kconfig::set(key.trim(), val.trim());
                println!("{}={}", key.trim(), val.trim());
            } else {
                println!("Usage: setconf KEY=VALUE");
            }
        }
        "stackcheck" => {
            let corrupted = task::check_stack_guards();
            if corrupted.is_empty() {
                println!("\x1b[32mAll task stack guards intact.\x1b[0m");
            } else {
                for (pid, name) in corrupted {
                    println!("\x1b[31mWARNING: stack guard corrupted for '{}' (pid {})\x1b[0m", name, pid);
                }
            }
        }
        "heapcheck" => {
            let h = allocator::check_integrity();
            println!("Heap integrity:");
            println!("  Bounds OK:      {}", if h.bounds_ok { "\x1b[32myes\x1b[0m" } else { "\x1b[31mNO\x1b[0m" });
            println!("  Not exhausted:  {}", if h.not_exhausted { "\x1b[32myes\x1b[0m" } else { "\x1b[31mNO\x1b[0m" });
            println!("  Reasonable use: {}", if h.reasonable_usage { "\x1b[32myes\x1b[0m" } else { "\x1b[31mNO\x1b[0m" });
            println!("  Used/Free:      {} / {} bytes", h.used, h.free);
        }
        "bt" => {
            print!("{}", ksyms::format_backtrace());
        }
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
        "lsmod" => {
            let mods = module::list();
            if mods.is_empty() {
                println!("No modules registered.");
            } else {
                println!("  \x1b[1mNAME         STATE     VERSION  DESCRIPTION\x1b[0m");
                for m in mods {
                    let state = match m.state {
                        module::ModuleState::Loaded => "\x1b[32mloaded  \x1b[0m",
                        module::ModuleState::Unloaded => "\x1b[90munloaded\x1b[0m",
                    };
                    println!("  {:<12} {} {:<8} {}", m.name, state, m.version, m.description);
                }
            }
        }
        cmd if cmd.starts_with("modprobe ") => {
            let name = cmd[9..].trim();
            match module::load(name) {
                Ok(()) => println!("Module '{}' loaded.", name),
                Err(e) => println!("modprobe: {}", e),
            }
        }
        cmd if cmd.starts_with("rmmod ") => {
            let name = cmd[6..].trim();
            match module::unload(name) {
                Ok(()) => println!("Module '{}' unloaded.", name),
                Err(e) => println!("rmmod: {}", e),
            }
        }
        cmd if cmd.starts_with("modinfo ") => {
            let name = cmd[8..].trim();
            match module::info(name) {
                Some(m) => {
                    println!("Name:        {}", m.name);
                    println!("Description: {}", m.description);
                    println!("Version:     {}", m.version);
                    let state = match m.state {
                        module::ModuleState::Loaded => "loaded",
                        module::ModuleState::Unloaded => "unloaded",
                    };
                    println!("State:       {}", state);
                }
                None => println!("modinfo: module '{}' not found", name),
            }
        }
        "cpuinfo" => {
            println!("{}", smp::cpu_info_string());
            println!("Online CPUs: {}", smp::online_cpus());
            for (i, cpu) in smp::cpu_list() {
                println!("  CPU {} (APIC {}): online", i, cpu.apic_id);
            }
        }
        "ifconfig" => {
            let n = net::NET.lock();
            println!("{}", n.ifconfig());
        }
        cmd if cmd.starts_with("send ") => {
            let msg = cmd[5..].trim();
            let mut n = net::NET.lock();
            n.send_udp(net::Ipv4Addr::LOOPBACK, 8080, 12345, msg.as_bytes());
            println!("Sent {} bytes to 127.0.0.1:8080", msg.len());
        }
        "recv" => {
            let mut n = net::NET.lock();
            let packets = n.recv();
            if packets.is_empty() {
                println!("No packets in queue.");
            } else {
                for p in packets {
                    let data = core::str::from_utf8(&p.data).unwrap_or("(binary)");
                    println!("  {} {}:{} -> {}:{} [{}]",
                        p.protocol, p.src_ip, p.src_port, p.dst_ip, p.dst_port, data);
                }
            }
        }
        cmd if cmd.starts_with("ping ") => {
            let target = cmd[5..].trim();
            if let Some(ip) = net::resolve(target) {
                println!("PING {} ({})...", target, ip);
                let results = netproto::ping(ip, 3);
                print!("{}", netproto::format_ping(&results));
            } else {
                println!("ping: cannot resolve '{}'", target);
            }
        }
        "arp" => {
            let entries = netproto::arp_list();
            if entries.is_empty() {
                println!("ARP table empty.");
            } else {
                println!("  \x1b[1mIP ADDRESS       MAC ADDRESS\x1b[0m");
                for (ip, mac) in entries {
                    println!("  {:<16} {}", ip, mac);
                }
            }
        }
        cmd if cmd.starts_with("arpreq ") => {
            let target = cmd[7..].trim();
            if let Some(ip) = net::resolve(target) {
                match virtio_net::send_arp_request(ip) {
                    Ok(()) => println!("ARP request sent for {}", ip),
                    Err(e) => println!("arpreq: {}", e),
                }
            } else {
                println!("Cannot resolve: {}", target);
            }
        }
        cmd if cmd.starts_with("rawping ") => {
            let target = cmd[8..].trim();
            if let Some(ip) = net::resolve(target) {
                for seq in 0..3u16 {
                    match virtio_net::send_ping(ip, seq) {
                        Ok(()) => println!("ICMP echo request → {} seq={}", ip, seq),
                        Err(e) => { println!("rawping: {}", e); break; }
                    }
                    // Small delay between pings
                    let pause = crate::timer::ticks() + 50;
                    while crate::timer::ticks() < pause { x86_64::instructions::hlt(); }
                }
                println!("3 packets sent to {}", ip);
            } else {
                println!("Cannot resolve: {}", target);
            }
        }
        cmd if cmd.starts_with("tcpconn ") => {
            let target = cmd[8..].trim();
            let (ip_str, port_str) = target.rsplit_once(':').unwrap_or((target, "80"));
            if let Some(ip) = net::resolve(ip_str) {
                let port = port_str.parse::<u16>().unwrap_or(80);
                match tcp::connect(ip, port) {
                    Ok(id) => println!("Connected: conn {} to {}:{}", id, ip, port),
                    Err(e) => println!("tcpconn: {}", e),
                }
            } else {
                println!("Cannot resolve: {}", ip_str);
            }
        }
        cmd if cmd.starts_with("tcpsend ") => {
            let rest = cmd[8..].trim();
            if let Some((id_str, data)) = rest.split_once(' ') {
                if let Ok(id) = id_str.parse::<usize>() {
                    match tcp::send(id, data.as_bytes()) {
                        Ok(n) => println!("Sent {} bytes on conn {}", n, id),
                        Err(e) => println!("tcpsend: {}", e),
                    }
                }
            } else {
                println!("Usage: tcpsend <conn_id> <data>");
            }
        }
        cmd if cmd.starts_with("tcprecv ") => {
            if let Ok(id) = cmd[8..].trim().parse::<usize>() {
                match tcp::recv(id) {
                    Ok(data) if data.is_empty() => println!("(no data)"),
                    Ok(data) => {
                        if let Ok(s) = core::str::from_utf8(&data) {
                            println!("{}", s);
                        } else {
                            println!("({} bytes binary)", data.len());
                        }
                    }
                    Err(e) => println!("tcprecv: {}", e),
                }
            }
        }
        cmd if cmd.starts_with("tcpclose ") => {
            if let Ok(id) = cmd[9..].trim().parse::<usize>() {
                match tcp::close(id) {
                    Ok(()) => println!("Connection {} closed", id),
                    Err(e) => println!("tcpclose: {}", e),
                }
            }
        }
        "netstat" => {
            let conns = tcp::list();
            if conns.is_empty() {
                println!("No TCP connections.");
            } else {
                println!("  \x1b[1mID  CONNECTION\x1b[0m");
                for (id, desc) in conns {
                    println!("  {:2}  {}", id, desc);
                }
            }
        }
        "virtio" => {
            let devs = virtio::scan();
            if devs.is_empty() {
                println!("No virtio devices found.");
            } else {
                for d in devs {
                    println!("  {}", d.summary());
                }
            }
        }
        cmd if cmd.starts_with("diskread ") => {
            if let Ok(sector) = cmd[9..].trim().parse::<u64>() {
                let mut buf = [0u8; 512];
                match virtio_blk::read_sector(sector, &mut buf) {
                    Ok(()) => {
                        println!("Sector {}:", sector);
                        // Print first 64 bytes as hex
                        for row in 0..4 {
                            print!("  {:04x}: ", row * 16);
                            for col in 0..16 {
                                print!("{:02x} ", buf[row * 16 + col]);
                            }
                            print!(" ");
                            for col in 0..16 {
                                let b = buf[row * 16 + col];
                                if b >= 0x20 && b < 0x7F { print!("{}", b as char); }
                                else { print!("."); }
                            }
                            println!();
                        }
                    }
                    Err(e) => println!("diskread: {}", e),
                }
            } else {
                println!("Usage: diskread <sector_number>");
            }
        }
        cmd if cmd.starts_with("diskwrite ") => {
            let rest = cmd[10..].trim();
            if let Some((sector_str, data)) = rest.split_once(' ') {
                if let Ok(sector) = sector_str.parse::<u64>() {
                    let mut buf = [0u8; 512];
                    let bytes = data.as_bytes();
                    let len = bytes.len().min(512);
                    buf[..len].copy_from_slice(&bytes[..len]);
                    match virtio_blk::write_sector(sector, &buf) {
                        Ok(()) => println!("Written {} bytes to sector {}", len, sector),
                        Err(e) => println!("diskwrite: {}", e),
                    }
                } else {
                    println!("Usage: diskwrite <sector> <data>");
                }
            } else {
                println!("Usage: diskwrite <sector> <data>");
            }
        }
        "diskfmt" => {
            match diskfs::format() {
                Ok(()) => println!("Disk formatted as MF16 (persistent)."),
                Err(e) => println!("diskfmt: {}", e),
            }
        }
        "diskls" => {
            match diskfs::list_files() {
                Ok(files) if files.is_empty() => println!("No files on disk. Use 'diskfmt' first."),
                Ok(files) => {
                    println!("  \x1b[1mSIZE  NAME\x1b[0m");
                    for f in files {
                        println!("  {:>5}  {}", f.size, f.name);
                    }
                }
                Err(e) => println!("diskls: {}", e),
            }
        }
        cmd if cmd.starts_with("disksave ") => {
            let rest = cmd[9..].trim();
            if let Some((name, data)) = rest.split_once(' ') {
                match diskfs::write_file(name, data.as_bytes()) {
                    Ok(()) => println!("Saved '{}' to disk ({} bytes, persistent)", name, data.len()),
                    Err(e) => println!("disksave: {}", e),
                }
            } else {
                println!("Usage: disksave <name> <data>");
            }
        }
        cmd if cmd.starts_with("diskload ") => {
            let name = cmd[9..].trim();
            match diskfs::read_file(name) {
                Ok(data) => {
                    if let Ok(s) = core::str::from_utf8(&data) {
                        println!("{}", s);
                    } else {
                        println!("({} bytes, binary)", data.len());
                    }
                }
                Err(e) => println!("diskload: {}", e),
            }
        }
        cmd if cmd.starts_with("diskrm ") => {
            let name = cmd[7..].trim();
            match diskfs::delete_file(name) {
                Ok(()) => println!("Deleted '{}' from disk", name),
                Err(e) => println!("diskrm: {}", e),
            }
        }
        "diskinfo" => {
            println!("{}", diskfs::info());
        }
        "blkdevs" => {
            let devs = blkdev::list();
            if devs.is_empty() {
                println!("No block devices.");
            } else {
                println!("  \x1b[1mNAME    BLOCKS  SIZE\x1b[0m");
                for d in devs {
                    println!("  {:<7} {:>6}  {}K", d.name, d.blocks, d.size_kb);
                }
            }
        }
        "fatfmt" => {
            let mut rd = ramdisk::RAMDISK.lock();
            match fat::format(&mut rd.data) {
                Ok(()) => println!("RAM disk formatted as MF16."),
                Err(e) => println!("fatfmt: {}", e),
            }
        }
        "fatls" => {
            let rd = ramdisk::RAMDISK.lock();
            let files = fat::list_files(&rd.data);
            if files.is_empty() {
                println!("No files (use 'fatfmt' first).");
            } else {
                println!("  \x1b[1mSIZE  NAME\x1b[0m");
                for f in files {
                    println!("  {:>5}  {}", f.size, f.name);
                }
            }
        }
        cmd if cmd.starts_with("fatw ") => {
            let rest = cmd[5..].trim();
            if let Some((name, data)) = rest.split_once(' ') {
                let mut rd = ramdisk::RAMDISK.lock();
                match fat::write_file(&mut rd.data, name, data.as_bytes()) {
                    Ok(()) => println!("Written '{}' ({} bytes)", name, data.len()),
                    Err(e) => println!("fatw: {}", e),
                }
            } else {
                println!("Usage: fatw <name> <data>");
            }
        }
        cmd if cmd.starts_with("fatr ") => {
            let name = cmd[5..].trim();
            let rd = ramdisk::RAMDISK.lock();
            match fat::read_file(&rd.data, name) {
                Some(data) => {
                    if let Ok(s) = core::str::from_utf8(&data) {
                        println!("{}", s);
                    } else {
                        println!("({} bytes, binary)", data.len());
                    }
                }
                None => println!("fatr: '{}' not found", name),
            }
        }
        "lspci" => {
            let devices = pci::scan();
            if devices.is_empty() {
                println!("No PCI devices found.");
            } else {
                println!("  \x1b[1mBUS:DEV.FN  VID:DID  CLASS            VENDOR\x1b[0m");
                for d in devices {
                    println!("  {}", d.summary());
                }
            }
        }
        "format" => {
            ramdisk::RAMDISK.lock().format();
            println!("RAM disk formatted (128K, MRLN filesystem).");
        }
        "disk" => {
            let rd = ramdisk::RAMDISK.lock();
            if rd.is_formatted() {
                let files = rd.list_files();
                let used = rd.used_bytes();
                println!("RAM disk: formatted, {} files, {} bytes used / {} total",
                    files.len(), used, 128 * 1024 - 16 * 512);
            } else {
                println!("RAM disk: not formatted (use 'format' first)");
            }
        }
        "dls" => {
            let rd = ramdisk::RAMDISK.lock();
            if !rd.is_formatted() {
                println!("Disk not formatted.");
            } else {
                let files = rd.list_files();
                if files.is_empty() {
                    println!("No files on disk.");
                } else {
                    println!("  \x1b[1mSIZE  NAME\x1b[0m");
                    for (name, size) in files {
                        println!("  {:>5}  {}", size, name);
                    }
                }
            }
        }
        cmd if cmd.starts_with("dsave ") => {
            let rest = cmd[6..].trim();
            if let Some((name, data)) = rest.split_once(' ') {
                match ramdisk::RAMDISK.lock().write_file(name, data.as_bytes()) {
                    Ok(()) => println!("Saved '{}' ({} bytes)", name, data.len()),
                    Err(e) => println!("dsave: {}", e),
                }
            } else {
                println!("Usage: dsave <name> <data>");
            }
        }
        cmd if cmd.starts_with("dload ") => {
            let name = cmd[6..].trim();
            match ramdisk::RAMDISK.lock().read_file(name) {
                Some(data) => {
                    if let Ok(s) = core::str::from_utf8(&data) {
                        println!("{}", s);
                    } else {
                        println!("({} bytes, binary)", data.len());
                    }
                }
                None => println!("dload: file '{}' not found", name),
            }
        }
        cmd if cmd.starts_with("echo ") => {
            println!("{}", cmd[5..].trim());
        }
        "echo" => println!(),
        "env" => {
            for (k, v) in env::list() {
                println!("  {}={}", k, v);
            }
        }
        cmd if cmd.starts_with("set ") => {
            let rest = cmd[4..].trim();
            if let Some((key, val)) = rest.split_once('=') {
                env::set(key.trim(), val.trim());
            } else {
                println!("Usage: set KEY=VALUE");
            }
        }
        cmd if cmd.starts_with("unset ") => {
            env::unset(cmd[6..].trim());
        }
        cmd if cmd.starts_with("alias ") => {
            let rest = cmd[6..].trim();
            if let Some((name, command)) = rest.split_once('=') {
                env::set_alias(name.trim(), command.trim());
                println!("alias {}='{}'", name.trim(), command.trim());
            } else {
                println!("Usage: alias name=command");
            }
        }
        "alias" => {
            for (name, cmd) in env::list_aliases() {
                println!("  {}='{}'", name, cmd);
            }
        }
        "whoami" => {
            println!("{}", env::get("USER").unwrap_or_else(|| alloc::string::String::from("root")));
        }
        "hostname" => {
            println!("{}", env::get("HOSTNAME").unwrap_or_else(|| alloc::string::String::from("merlion")));
        }
        "uname" | "uname -a" => {
            let dt = rtc::read();
            println!("MerlionOS merlion 0.2.0 {} x86_64", dt);
        }
        "neofetch" => {
            neofetch();
        }
        "history" => {
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
        cmd if cmd.starts_with("sleep ") => {
            if let Ok(secs) = cmd[6..].trim().parse::<u64>() {
                let target = timer::ticks() + secs * timer::PIT_FREQUENCY_HZ;
                print!("Sleeping for {} second(s)...", secs);
                while timer::ticks() < target {
                    x86_64::instructions::hlt();
                }
                println!(" done.");
            } else {
                println!("Usage: sleep <seconds>");
            }
        }
        "gfx" => {
            println!("Entering graphics mode (press any key to exit)...");
            framebuf::demo();
            // Wait for a keypress (the keyboard handler will resume the shell)
            // For now, we just show it — user types any key to get back
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
        // --- AI commands ---
        cmd if cmd.starts_with("ai ") => {
            let prompt = cmd[3..].trim();
            // Try LLM proxy first, fall back to keyword AI shell
            if let Some(response) = ai_proxy::infer(prompt) {
                println!("\x1b[36m[ai]\x1b[0m {}", response);
            } else if let Some(ai_cmd) = ai_shell::interpret(prompt) {
                println!("{}", ai_shell::format_hint(prompt, &ai_cmd));
                dispatch(&ai_cmd);
            } else {
                println!("\x1b[90m[ai] Cannot interpret: \"{}\"\x1b[0m", prompt);
                println!("\x1b[90m     (Connect LLM proxy to COM2 for full AI)\x1b[0m");
            }
        }
        "monitor" => {
            println!("\x1b[1m=== AI System Monitor ===\x1b[0m");
            let alerts = ai_monitor::check();
            print!("{}", ai_monitor::format_alerts(&alerts));
        }
        cmd if cmd.starts_with("tag ") => {
            let rest = cmd[4..].trim();
            if let Some((path, tag)) = rest.split_once(' ') {
                let tags: alloc::vec::Vec<&str> = tag.split_whitespace().collect();
                semfs::tag(path, &tags);
                println!("Tagged {} with {:?}", path, tags);
            } else {
                println!("Usage: tag <path> <tag1> [tag2 ...]");
            }
        }
        cmd if cmd.starts_with("tags ") => {
            let path = cmd[5..].trim();
            let tags = semfs::get_tags(path);
            if tags.is_empty() {
                println!("No tags for {}", path);
            } else {
                println!("{}: {}", path, tags.join(", "));
            }
        }
        cmd if cmd.starts_with("search ") => {
            let query = cmd[7..].trim();
            let results = semfs::search(query);
            if results.is_empty() {
                println!("No files match '{}'", query);
            } else {
                for path in results {
                    let tags = semfs::get_tags(&path);
                    println!("  {} [{}]", path, tags.join(", "));
                }
            }
        }
        cmd if cmd.starts_with("man ") => {
            let topic = cmd[4..].trim();
            print!("{}", ai_man::man(topic));
        }
        "bootinfo" => {
            print!("{}", boot_info_ext::format_boot_info());
        }
        cmd if cmd.starts_with("explain ") => {
            let topic = cmd[8..].trim();
            println!("{}", ai_syscall::explain(topic));
        }
        "heal" => {
            println!("\x1b[1m=== AI Self-Healing Diagnosis ===\x1b[0m");
            let diagnoses = ai_heal::auto_recover();
            for d in &diagnoses {
                print!("{}", ai_heal::format_diagnosis(d));
            }
        }
        "agents" => {
            let agents = agent::list();
            if agents.is_empty() {
                println!("No agents registered.");
            } else {
                println!("  \x1b[1mNAME      STATE    TICKS  DESCRIPTION\x1b[0m");
                for a in agents {
                    let state = match a.state {
                        agent::AgentState::Running => "\x1b[32mrunning\x1b[0m",
                        agent::AgentState::Paused => "\x1b[90mpaused \x1b[0m",
                    };
                    println!("  {:<9} {} {:>5}  {}", a.name, state, a.ticks, a.description);
                }
            }
        }
        cmd if cmd.starts_with("ask ") => {
            let rest = cmd[4..].trim();
            if let Some((agent_name, msg)) = rest.split_once(' ') {
                match agent::send_message(agent_name, msg) {
                    Some(response) => println!("\x1b[36m[{}]\x1b[0m {}", agent_name, response),
                    None => println!("Agent '{}' not found or paused", agent_name),
                }
            } else {
                println!("Usage: ask <agent> <message>");
            }
        }
        "chat" => {
            chat::enter();
            // Chat runs until user types 'exit', handled by keyboard routing
            while chat::is_chatting() {
                x86_64::instructions::hlt();
            }
        }
        "fortune" => {
            println!("\x1b[33m  {}\x1b[0m", fortune::random());
        }
        "aistatus" => {
            println!("AI Proxy:   {}", ai_proxy::status());
            println!("AI Shell:   keyword engine (built-in)");
            println!("AI Agents:  {} registered", agent::list().len());
            println!("Sem. VFS:   {} tagged files", semfs::list_all().len());
        }

        "bench" => bench::run_all(),
        cmd if cmd.starts_with("calc ") => {
            let expr = cmd[5..].trim();
            match calc::eval(expr) {
                Ok(result) => println!("= {}", calc::format_number(result)),
                Err(e) => println!("calc: {}", e),
            }
        }
        "top" => {
            top::run();
            crate::vga::print_banner();
            println!("Type 'help' for commands.");
        }
        "about" => {
            println!();
            println!("\x1b[36m  ▄▄▄      ▄▄▄             ▄▄                   ▄▄▄▄▄    ▄▄▄▄▄▄▄\x1b[0m");
            println!("\x1b[36m  ████▄  ▄████             ██ ▀▀              ▄███████▄ █████▀▀▀\x1b[0m");
            println!("\x1b[36m  ███▀████▀███ ▄█▀█▄ ████▄ ██ ██  ▄███▄ ████▄ ███   ███  ▀████▄\x1b[0m");
            println!("\x1b[36m  ███  ▀▀  ███ ██▄█▀ ██ ▀▀ ██ ██  ██ ██ ██ ██ ███▄▄▄███    ▀████\x1b[0m");
            println!("\x1b[36m  ███      ███ ▀█▄▄▄ ██    ██ ██▄ ▀███▀ ██ ██  ▀█████▀  ███████▀\x1b[0m");
            println!();
            println!("  \x1b[1m{}\x1b[0m", crate::version::SLOGAN);
            println!("  {}", crate::version::SLOGAN_CN);
            println!();
            println!("  A Singapore-inspired AI-native hobby operating system.");
            println!("  Written in Rust for x86_64. Runs in QEMU.");
            println!();
            println!("  Version:  {}", crate::version::full());
            println!("  Modules:  {}", crate::version::MODULES);
            println!("  Commands: {}+", crate::version::COMMANDS);
            println!("  License:  MIT");
            println!();
            println!("  https://github.com/MerlionOS/merlion-kernel");
            println!();
        }
        "version" => {
            println!("{}", crate::version::full());
            print!("{}", crate::version::build_info());
        }
        "demo" => demo::run(),
        "snake" => {
            snake::run();
            // Restore shell screen after game
            crate::vga::print_banner();
            println!("Type 'help' for commands.");
        }
        _ if cmd.starts_with("wget ") => {
            let url = cmd[5..].trim();
            if url.is_empty() {
                println!("usage: wget <url>");
            } else {
                match http::parse_url(url) {
                    Some((host, port, path)) => {
                        let req_bytes = http::build_request("GET", &host, &path);
                        if let Ok(s) = core::str::from_utf8(&req_bytes) {
                            println!("{}", s);
                        }
                        println!("HTTP request built for {}:{}{} (sending requires TCP)", host, port, path);
                    }
                    None => println!("invalid URL: {}", url),
                }
            }
        }
        "ifup" => {
            let pkt = dhcp::discover();
            println!("DHCP Discover built ({} bytes). Sending requires virtio-net RX.", pkt.len());
        }
        _ if cmd.starts_with("dns ") => {
            let hostname = cmd[4..].trim();
            if hostname.is_empty() {
                println!("usage: dns <hostname>");
            } else {
                match dhcp::resolve(hostname) {
                    Some(ip) => println!("{} -> {}", hostname, ip),
                    None => println!("could not resolve {}", hostname),
                }
            }
        }
        "ahciinfo" => {
            if ahci::is_detected() {
                println!("{}", ahci::info());
            } else {
                println!("AHCI controller not detected");
            }
        }
        "usbdevs" => {
            if xhci::is_detected() {
                println!("{}", xhci::info());
            } else {
                println!("xHCI USB controller not detected");
            }
        }
        "ioapicinfo" => {
            println!("{}", ioapic::info());
        }
        "e1000info" => {
            if e1000e::is_detected() {
                println!("{}", e1000e::info());
            } else {
                println!("e1000e NIC not detected");
            }
        }
        "panic" => panic!("user-triggered panic via shell"),
        _ => {
            // Try AI natural language interpretation
            if let Some(ai_cmd) = ai_shell::interpret(cmd) {
                println!("{}", ai_shell::format_hint(cmd, &ai_cmd));
                dispatch(&ai_cmd);
            } else {
                println!("unknown command: {}", cmd);
                println!("type 'help' for commands");
            }
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

fn neofetch() {
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
