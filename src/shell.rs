/// Interactive kernel shell with command history.
/// Supports arrow keys (up/down for history, left/right planned),
/// shift for uppercase, and output redirection (cmd > file).

use crate::{print, println, serial_println, allocator, timer, task, process, ipc, vfs, memory, driver, acpi, rtc, testutil, framebuf, pci, ramdisk, net, netproto, netstack, smp, env, module, slab, ksyms, paging, virtio, virtio_blk, virtio_net, blkdev, fat, fd, locks, ai_shell, ai_proxy, ai_monitor, ai_syscall, ai_heal, ai_man, semfs, agent, script, signal, kconfig, tcp, tcp_real, elf, elf_loader, boot_info_ext, demo, snake, diskfs, editor, top, calc, coreutils, chat, fortune, bench, ahci, nvme, xhci, e1000e, ioapic, http, dhcp, gpt, power, forth, watch, wget, screensaver};
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
            println!("  tcppoll    - poll incoming TCP segments");
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
            println!("  forth      - Forth programming language");
            println!("  watch <n> <cmd> - repeat command every N sec");
            println!("  matrix     - Matrix screensaver");
            println!("  shutdown   - power off");
            println!("  reboot     - restart");
            println!("  panic      - trigger panic");
            println!("Security commands:");
            println!("  id [user]  - show user/group identity");
            println!("  whoami     - current user name");
            println!("  su <user>  - switch user");
            println!("  sudo <cmd> - run command as root");
            println!("  passwd     - change password");
            println!("  chmod <m> <p> - change file mode");
            println!("  chown <o> <p> - change file owner");
            println!("  useradd <n> <uid> - add user");
            println!("  userdel <uid> - remove user");
            println!("  users      - list all users");
            println!("  groups     - list all groups");
            println!("  ls -l [p]  - long listing with perms");
            println!("  caps [pid] - show capabilities");
            println!("  seccomp <pid> - syscall filter info");
            println!("  audit      - security audit summary");
            println!("  mkdir <p>  - create directory");
            println!("Logging commands:");
            println!("  logquery [N] - last N structured log entries");
            println!("  logjson [N]  - log entries as JSON");
            println!("  logfilter <l> - filter by severity level");
            println!("  loglevel <l> - set minimum log level");
            println!("  auditlog [N] - audit trail entries");
            println!("  auditstats   - audit event statistics");
            println!("  logrotate    - force log rotation");
            println!("  logstatus    - log rotation status");
            println!("  remotelog    - remote syslog status/config");
            println!("Profiling commands:");
            println!("  perf         - all performance info");
            println!("  perf stat    - performance counters");
            println!("  perf record [i] - start CPU profiling");
            println!("  perf stop    - stop and show profile");
            println!("  perf top     - profile analysis");
            println!("  syscall-stats [on|off] - syscall latency");
            println!("  alloc-track [on|off] - allocation tracker");
            println!("  alloc-track leaks - potential leaks");
            println!("  alloc-track events - recent allocations");
            println!("  alloc-track pids - per-PID alloc stats");
            println!("Hardware:");
            println!("  wget <url> - fetch URL via real TCP connection");
            println!("  ifup       - DHCP discover sequence");
            println!("  dns <host> - resolve hostname");
            println!("  ahciinfo   - AHCI controller status");
            println!("  usbdevs    - list USB devices");
            println!("  ioapicinfo - IOAPIC status");
            println!("  e1000info  - e1000e NIC status");
            println!("  nvmeinfo   - NVMe SSD status");
            println!("  gptinfo    - GPT partition table (virtio disk)");
            println!("  powerinfo  - power management status");
            println!("  nicsend <msg> - send raw UDP via NIC");
            println!("Stability commands:");
            println!("  integrity  - full system integrity check");
            println!("  crashlog   - crash history");
            println!("  crashstats - crash/recovery stats");
            println!("  redzone    - memory red zone check");
            println!("  recovery on|off - panic recovery toggle");
            println!("  fuzz [N]   - run all fuzz tests");
            println!("  fuzz-vfs [N] - fuzz VFS subsystem");
            println!("  fuzz-security [N] - fuzz security");
            println!("  fuzz-ipc [N] - fuzz IPC channels");
            println!("  fuzz-parsers [N] - fuzz parsers");
            println!("  fuzz-seed <N> - set PRNG seed");
            println!("Network services:");
            println!("  http-stats   - HTTP server statistics");
            println!("  http-log [N] - HTTP access log");
            println!("  http-mw      - list HTTP middleware");
            println!("  ssh-sessions - active SSH sessions");
            println!("  ssh-stats    - SSH statistics");
            println!("  scp-list     - SCP transfer history");
            println!("  dns-zones    - list DNS zones");
            println!("  dns-zone <d> - show zone details");
            println!("  dns-cache    - DNS cache stats");
            println!("  mqtt-stats   - MQTT broker statistics");
            println!("  mqtt-clients - MQTT connected clients");
            println!("  mqtt-retained - MQTT retained messages");
            println!("  ws-conns     - WebSocket connections");
            println!("  ws-rooms     - WebSocket rooms");
            println!("  ws-stats     - WebSocket statistics");
            println!("AI platform:");
            println!("  nn-models    - list neural network models");
            println!("  nn-infer <m> <inputs> - run inference");
            println!("  nn-demo      - neural network demo");
            println!("  ml-demo      - ML training demo");
            println!("  ml-stats     - ML statistics");
            println!("  vsearch <q>  - vector semantic search");
            println!("  vstore       - vector store info");
            println!("  workflow      - list workflows");
            println!("  workflow-demo - run demo workflow");
            println!("  evolve       - AI self-analysis");
            println!("  findings     - code analysis findings");
            println!("  patches      - generated patches");
            println!("  evolve-stats - evolution statistics");
            println!("Hardware extensions:");
            println!("  gpu-info     - GPU device info");
            println!("  gpu-stats    - GPU compute statistics");
            println!("  gpu-bench    - GPU benchmark");
            println!("  gpu-buffers  - list GPU buffers");
            println!("  bt-info      - Bluetooth controller info");
            println!("  bt-scan      - scan for BT devices");
            println!("  bt-devices   - list BT devices");
            println!("  bt-stats     - Bluetooth statistics");
            println!("  dfs-info     - distributed FS info");
            println!("  dfs-nodes    - list DFS cluster nodes");
            println!("  dfs-mounts   - list remote mounts");
            println!("  dfs-stats    - DFS statistics");
            println!("  rt-tasks     - list real-time tasks");
            println!("  rt-stats     - RT scheduler statistics");
            println!("  rt-test      - schedulability test");
            println!("  services     - list microkernel services");
            println!("  ukernel      - microkernel mode status");
            println!("  ukernel-stats - microkernel statistics");
            println!("  ukernel on   - enable microkernel mode");
            println!("  ukernel off  - disable microkernel mode");
            println!("  health       - service health check");
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
        "ls -l" => {
            match crate::vfs::ls_long("/") {
                Ok(entries) => {
                    for entry in entries {
                        println!("  {}", entry);
                    }
                }
                Err(e) => println!("ls: {}", e),
            }
        }
        cmd if cmd.starts_with("ls -l ") => {
            let path = cmd[6..].trim();
            let path = if path.is_empty() { "/" } else { path };
            match crate::vfs::ls_long(path) {
                Ok(entries) => {
                    for entry in entries {
                        println!("  {}", entry);
                    }
                }
                Err(e) => println!("ls: {}", e),
            }
        }
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
                match tcp_real::connect(ip, port) {
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
                    match tcp_real::send(id, data.as_bytes()) {
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
                match tcp_real::recv(id) {
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
                match tcp_real::close(id) {
                    Ok(()) => println!("Connection {} closed", id),
                    Err(e) => println!("tcpclose: {}", e),
                }
            }
        }
        "netstat" => {
            let sockets = tcp_real::list_sockets();
            if sockets.is_empty() {
                println!("No TCP connections.");
            } else {
                println!("  \x1b[1mID  LOCAL              REMOTE             STATE\x1b[0m");
                for (id, lip, lport, rip, rport, state) in sockets {
                    println!("  {:2}  {}:{:<5}  {}:{:<5}  {:?}", id, lip, lport, rip, rport, state);
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

        // --- Security commands ---
        "id" => {
            match crate::security::id_info(None) {
                Ok(info) => println!("{}", info),
                Err(e) => println!("id: {}", e),
            }
        }
        cmd if cmd.starts_with("id ") => {
            let user = cmd[3..].trim();
            match crate::security::id_info(Some(user)) {
                Ok(info) => println!("{}", info),
                Err(e) => println!("id: {}", e),
            }
        }
        cmd if cmd.starts_with("su ") => {
            let user = cmd[3..].trim();
            // For now, no password prompt in shell - root can su freely
            match crate::security::su(user, Some("")) {
                Ok(()) => {
                    crate::env::set("USER", user);
                    println!("Switched to {}", user);
                }
                Err(e) => println!("su: {}", e),
            }
        }
        cmd if cmd.starts_with("sudo ") => {
            let subcmd = cmd[5..].trim();
            let orig_uid = crate::security::current_uid();
            let _ = orig_uid;
            // Temporarily switch to root
            match crate::security::sudo(Some(""), || {
                dispatch(subcmd);
            }) {
                Ok(()) => {},
                Err(e) => println!("sudo: {}", e),
            }
        }
        "passwd" => {
            let user = crate::security::whoami();
            match crate::security::passwd(&user, Some(""), "") {
                Ok(()) => println!("Password updated for {}", user),
                Err(e) => println!("passwd: {}", e),
            }
        }
        cmd if cmd.starts_with("passwd ") => {
            let user = cmd[7..].trim();
            match crate::security::passwd(user, Some(""), "") {
                Ok(()) => println!("Password updated for {}", user),
                Err(e) => println!("passwd: {}", e),
            }
        }
        cmd if cmd.starts_with("chmod ") => {
            let parts: alloc::vec::Vec<&str> = cmd[6..].trim().splitn(2, ' ').collect();
            if parts.len() == 2 {
                // Parse octal mode
                if let Ok(mode) = u16::from_str_radix(parts[0], 8) {
                    match crate::security::chmod(parts[1], mode) {
                        Ok(()) => println!("chmod: mode set to {:o} on {}", mode, parts[1]),
                        Err(e) => println!("chmod: {}", e),
                    }
                } else {
                    println!("chmod: invalid mode (use octal, e.g. 755)");
                }
            } else {
                println!("Usage: chmod <mode> <path>");
            }
        }
        cmd if cmd.starts_with("chown ") => {
            let parts: alloc::vec::Vec<&str> = cmd[6..].trim().splitn(2, ' ').collect();
            if parts.len() == 2 {
                let owner_parts: alloc::vec::Vec<&str> = parts[0].split(':').collect();
                let uid = owner_parts[0].parse::<u32>().unwrap_or(0);
                let gid = if owner_parts.len() > 1 {
                    owner_parts[1].parse::<u32>().unwrap_or(0)
                } else { 0 };
                match crate::security::chown(parts[1], uid, gid) {
                    Ok(()) => println!("chown: ownership changed on {}", parts[1]),
                    Err(e) => println!("chown: {}", e),
                }
            } else {
                println!("Usage: chown <uid:gid> <path>");
            }
        }
        cmd if cmd.starts_with("useradd ") => {
            let parts: alloc::vec::Vec<&str> = cmd[8..].trim().splitn(2, ' ').collect();
            if parts.len() == 2 {
                if let Ok(uid) = parts[1].parse::<u32>() {
                    match crate::security::add_user(uid, parts[0], "", &[1000]) {
                        Ok(()) => println!("useradd: added user {} (uid {})", parts[0], uid),
                        Err(e) => println!("useradd: {}", e),
                    }
                } else {
                    println!("Usage: useradd <name> <uid>");
                }
            } else {
                println!("Usage: useradd <name> <uid>");
            }
        }
        cmd if cmd.starts_with("userdel ") => {
            if let Ok(uid) = cmd[8..].trim().parse::<u32>() {
                match crate::security::remove_user(uid) {
                    Ok(()) => println!("userdel: removed uid {}", uid),
                    Err(e) => println!("userdel: {}", e),
                }
            } else {
                println!("Usage: userdel <uid>");
            }
        }
        "users" => {
            for (uid, name) in crate::security::list_users() {
                println!("  {:5} {}", uid, name);
            }
        }
        "groups" => {
            for (gid, name) in crate::security::list_groups() {
                println!("  {:5} {}", gid, name);
            }
        }
        "caps" => {
            let pid = crate::task::current_pid();
            println!("{}", crate::capability::list_caps(pid));
        }
        cmd if cmd.starts_with("caps ") => {
            if let Ok(pid) = cmd[5..].trim().parse::<usize>() {
                println!("{}", crate::capability::list_caps(pid));
            } else {
                println!("Usage: caps [pid]");
            }
        }
        cmd if cmd.starts_with("seccomp ") => {
            if let Ok(pid) = cmd[8..].trim().parse::<usize>() {
                println!("{}", crate::capability::seccomp_display(pid));
            } else {
                println!("Usage: seccomp <pid>");
            }
        }
        "audit" => {
            println!("{}", crate::capability::audit_summary());
        }
        cmd if cmd.starts_with("mkdir ") => {
            let path = cmd[6..].trim();
            match crate::vfs::mkdir(path) {
                Ok(()) => println!("mkdir: created {}", path),
                Err(e) => println!("mkdir: {}", e),
            }
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

        // --- Logging & audit commands ---
        cmd if cmd == "logquery" || cmd.starts_with("logquery ") => {
            let count = cmd.strip_prefix("logquery ").and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(20);
            let entries = crate::structured_log::query(count);
            for entry in &entries {
                println!("{}", crate::structured_log::to_text(entry));
            }
            if entries.is_empty() { println!("(no entries)"); }
        }
        cmd if cmd == "logjson" || cmd.starts_with("logjson ") => {
            let count = cmd.strip_prefix("logjson ").and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(10);
            let entries = crate::structured_log::query(count);
            println!("{}", crate::structured_log::format_json(&entries));
        }
        cmd if cmd.starts_with("logfilter ") => {
            let level = cmd.strip_prefix("logfilter ").unwrap().trim();
            let sev = match level {
                "emerg" | "emergency" => crate::structured_log::Severity::Emergency,
                "alert" => crate::structured_log::Severity::Alert,
                "crit" | "critical" => crate::structured_log::Severity::Critical,
                "err" | "error" => crate::structured_log::Severity::Error,
                "warn" | "warning" => crate::structured_log::Severity::Warning,
                "notice" => crate::structured_log::Severity::Notice,
                "info" => crate::structured_log::Severity::Info,
                "debug" => crate::structured_log::Severity::Debug,
                _ => { println!("logfilter: unknown level '{}'", level); return; }
            };
            let entries = crate::structured_log::query_by_severity(sev, 50);
            for entry in &entries {
                println!("{}", crate::structured_log::to_text(entry));
            }
            if entries.is_empty() { println!("(no entries at {} level)", level); }
        }
        cmd if cmd.starts_with("loglevel ") => {
            let level = cmd.strip_prefix("loglevel ").unwrap().trim();
            let sev = match level {
                "emerg" => crate::structured_log::Severity::Emergency,
                "alert" => crate::structured_log::Severity::Alert,
                "crit" => crate::structured_log::Severity::Critical,
                "err" | "error" => crate::structured_log::Severity::Error,
                "warn" => crate::structured_log::Severity::Warning,
                "notice" => crate::structured_log::Severity::Notice,
                "info" => crate::structured_log::Severity::Info,
                "debug" => crate::structured_log::Severity::Debug,
                _ => { println!("loglevel: unknown level"); return; }
            };
            crate::structured_log::set_min_severity(sev);
            println!("Log level set to {}", level);
        }
        cmd if cmd == "auditlog" || cmd.starts_with("auditlog ") => {
            let count = cmd.strip_prefix("auditlog ").and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(20);
            let entries = crate::structured_log::audit_trail(count);
            for entry in &entries {
                println!("{}", crate::structured_log::to_text(entry));
            }
            if entries.is_empty() { println!("(no audit entries)"); }
        }
        "logrotate" => {
            crate::log_rotate::rotate_all();
            println!("All logs rotated.");
        }
        "logstatus" => {
            println!("{}", crate::log_rotate::status());
        }
        "remotelog" => {
            println!("{}", crate::remote_log::status());
        }
        cmd if cmd.starts_with("remotelog ") => {
            let ip_str = cmd.strip_prefix("remotelog ").unwrap().trim();
            // Parse IP a.b.c.d
            let parts: alloc::vec::Vec<&str> = ip_str.split('.').collect();
            if parts.len() == 4 {
                let a = parts[0].parse::<u8>().unwrap_or(0);
                let b = parts[1].parse::<u8>().unwrap_or(0);
                let c = parts[2].parse::<u8>().unwrap_or(0);
                let d = parts[3].parse::<u8>().unwrap_or(0);
                crate::remote_log::set_server([a, b, c, d]);
                println!("Remote syslog server set to {}", ip_str);
            } else {
                println!("Usage: remotelog <ip>  (e.g. remotelog 192.168.1.100)");
            }
        }
        "auditstats" => {
            println!("{}", crate::structured_log::audit_stats());
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
        cmd if cmd.starts_with("watch ") => {
            let rest = cmd[6..].trim();
            if let Some((n_str, command)) = rest.split_once(' ') {
                let interval = n_str.parse::<u64>().unwrap_or(2);
                watch::run(interval, command.trim());
                crate::vga::print_banner();
                println!("Watch stopped.");
            } else {
                println!("Usage: watch <seconds> <command>");
            }
        }
        "forth" => {
            forth::enter();
            while forth::is_running() { x86_64::instructions::hlt(); }
            println!("Forth exited.");
        }
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
                match wget::fetch(url) {
                    Ok(response) => print!("{}", response),
                    Err(e) => println!("wget: {}", e),
                }
            }
        }
        "matrix" => {
            screensaver::run();
            crate::vga::print_banner();
            println!("Type 'help' for commands.");
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
        "nvmeinfo" => {
            if nvme::is_detected() {
                println!("{}", nvme::info());
            } else {
                println!("NVMe not detected");
            }
        }
        "gptinfo" => {
            if !virtio_blk::is_detected() {
                println!("No virtio disk to read GPT from.");
            } else {
                let mut sector = [0u8; 512];
                match virtio_blk::read_sector(1, &mut sector) {
                    Ok(()) => {
                        match gpt::parse_header(&sector) {
                            Some(hdr) => {
                                let num_ent = hdr.num_partition_entries;
                                let ent_size = hdr.partition_entry_size;
                                println!("GPT found: {} entries", num_ent);
                                // Read partition entries (sectors 2-33)
                                let mut entry_data = alloc::vec![0u8; 512 * 32];
                                for i in 0..32u64 {
                                    let mut s = [0u8; 512];
                                    let _ = virtio_blk::read_sector(2 + i, &mut s);
                                    entry_data[(i as usize) * 512..(i as usize + 1) * 512].copy_from_slice(&s);
                                }
                                let parts = gpt::parse_entries(&entry_data, num_ent, ent_size);
                                if parts.is_empty() {
                                    println!("No partitions found.");
                                } else {
                                    print!("{}", gpt::format_table(&parts));
                                }
                            }
                            None => println!("No GPT header on disk (sector 1)."),
                        }
                    }
                    Err(e) => println!("gptinfo: {}", e),
                }
            }
        }
        "powerinfo" => {
            println!("{}", power::info());
        }
        cmd if cmd.starts_with("nicsend ") => {
            let msg = cmd[8..].trim();
            let dst_ip = [10, 0, 2, 2]; // gateway
            netstack::send_udp(dst_ip, 12345, 8080, msg.as_bytes());
            println!("Sent {} bytes via NIC to 10.0.2.2:8080", msg.len());
        }
        "tcppoll" => {
            let n = tcp_real::poll_incoming();
            println!("Processed {} incoming TCP segment(s).", n);
        }
        // --- Profiling commands ---
        "perf" => {
            // Perf counters
            let counters = crate::profiler::perf_stat();
            println!("{}", crate::profiler::format_perf_counters(&counters));
            // Syscall stats
            if crate::syscall_stats::is_enabled() {
                println!("{}", crate::syscall_stats::report());
            }
            // Alloc tracker
            if crate::alloc_track::is_active() {
                println!("{}", crate::alloc_track::stats());
            }
        }
        "perf stat" => {
            let counters = crate::profiler::perf_stat();
            println!("{}", crate::profiler::format_perf_counters(&counters));
        }
        cmd if cmd == "perf record" || cmd.starts_with("perf record ") => {
            let interval = cmd.strip_prefix("perf record ")
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(1);
            crate::profiler::start_profiling(interval);
            println!("CPU profiling started (interval: {} ticks)", interval);
        }
        "perf stop" => {
            let session = crate::profiler::stop_profiling();
            let report = crate::profiler::analyze(&session);
            println!("{}", crate::profiler::format_report(&report));
        }
        "perf top" => {
            let session = crate::profiler::stop_profiling();
            if session.samples.is_empty() {
                println!("No samples collected. Use 'perf record' first.");
            } else {
                let report = crate::profiler::analyze(&session);
                println!("{}", crate::profiler::format_report(&report));
            }
        }
        "syscall-stats" => {
            println!("{}", crate::syscall_stats::report());
        }
        "syscall-stats on" => {
            crate::syscall_stats::enable();
            println!("Syscall statistics enabled.");
        }
        "syscall-stats off" => {
            crate::syscall_stats::disable();
            println!("Syscall statistics disabled.");
        }
        "alloc-track" => {
            println!("{}", crate::alloc_track::stats());
        }
        "alloc-track on" => {
            crate::alloc_track::start();
            println!("Allocation tracking started.");
        }
        "alloc-track off" => {
            crate::alloc_track::stop();
            println!("Allocation tracking stopped.");
        }
        "alloc-track leaks" => {
            println!("{}", crate::alloc_track::leaks());
        }
        cmd if cmd == "alloc-track events" || cmd.starts_with("alloc-track events ") => {
            let count = cmd.strip_prefix("alloc-track events ")
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(20);
            println!("{}", crate::alloc_track::recent_events(count));
        }
        "alloc-track pids" => {
            println!("{}", crate::alloc_track::per_pid_stats());
        }
        "crashlog" => {
            println!("{}", crate::panic_recover::crash_log());
        }
        "crashstats" => {
            println!("{}", crate::panic_recover::stats());
        }
        "integrity" => {
            println!("{}", crate::panic_recover::integrity_check());
        }
        "redzone" => {
            println!("{}", crate::panic_recover::red_zone_status());
            let violations = crate::panic_recover::check_red_zones();
            if violations.is_empty() {
                println!("All red zones intact.");
            } else {
                for (addr, kind) in &violations {
                    println!("  VIOLATION at 0x{:x}: {}", addr, kind);
                }
            }
        }
        "recovery on" => {
            crate::panic_recover::set_recovery(true);
            println!("Panic recovery enabled.");
        }
        "recovery off" => {
            crate::panic_recover::set_recovery(false);
            println!("Panic recovery disabled.");
        }
        cmd if cmd == "fuzz" || cmd.starts_with("fuzz ") => {
            let count = cmd.strip_prefix("fuzz ")
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(100);
            println!("Running fuzz tests ({} iterations per test)...", count);
            println!("{}", crate::fuzz::fuzz_all(count));
        }
        cmd if cmd == "fuzz-vfs" || cmd.starts_with("fuzz-vfs ") => {
            let count = cmd.strip_prefix("fuzz-vfs ")
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(100);
            let result = crate::fuzz::fuzz_vfs(count);
            println!("{}", crate::fuzz::format_result(&result));
        }
        cmd if cmd == "fuzz-security" || cmd.starts_with("fuzz-security ") => {
            let count = cmd.strip_prefix("fuzz-security ")
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(100);
            let result = crate::fuzz::fuzz_security(count);
            println!("{}", crate::fuzz::format_result(&result));
        }
        cmd if cmd == "fuzz-ipc" || cmd.starts_with("fuzz-ipc ") => {
            let count = cmd.strip_prefix("fuzz-ipc ")
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(100);
            let result = crate::fuzz::fuzz_ipc(count);
            println!("{}", crate::fuzz::format_result(&result));
        }
        cmd if cmd == "fuzz-parsers" || cmd.starts_with("fuzz-parsers ") => {
            let count = cmd.strip_prefix("fuzz-parsers ")
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(100);
            let result = crate::fuzz::fuzz_parsers(count);
            println!("{}", crate::fuzz::format_result(&result));
        }
        cmd if cmd.starts_with("fuzz-seed ") => {
            if let Ok(seed) = cmd.strip_prefix("fuzz-seed ").unwrap().trim().parse::<u64>() {
                crate::fuzz::seed(seed);
                println!("Fuzz PRNG seed set to {}", seed);
            } else {
                println!("Usage: fuzz-seed <number>");
            }
        }
        "http-stats" => {
            println!("{}", crate::http_middleware::server_stats());
        }
        cmd if cmd == "http-log" || cmd.starts_with("http-log ") => {
            let count = cmd.strip_prefix("http-log ")
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(20);
            println!("{}", crate::http_middleware::format_access_log(count));
        }
        "http-mw" => {
            println!("{}", crate::http_middleware::list_middleware());
        }
        "ssh-sessions" => {
            println!("{}", crate::scp::list_sessions());
        }
        "ssh-stats" => {
            println!("{}", crate::scp::session_stats());
        }
        "scp-list" => {
            println!("{}", crate::scp::list_transfers());
        }
        "dns-zones" => {
            println!("{}", crate::dns_zone::list_zones());
        }
        cmd if cmd.starts_with("dns-zone ") => {
            let domain = cmd.strip_prefix("dns-zone ").unwrap().trim();
            println!("{}", crate::dns_zone::zone_info(domain));
        }
        "dns-cache" => {
            println!("{}", crate::dns_zone::cache_stats());
        }
        "mqtt-stats" => {
            println!("{}", crate::mqtt_broker::broker_stats());
        }
        "mqtt-clients" => {
            println!("{}", crate::mqtt_broker::list_clients());
        }
        "mqtt-retained" => {
            println!("{}", crate::mqtt_broker::list_retained());
        }
        "ws-conns" => {
            println!("{}", crate::ws_server::list_connections());
        }
        "ws-rooms" => {
            println!("{}", crate::ws_server::list_rooms());
        }
        "ws-stats" => {
            println!("{}", crate::ws_server::ws_stats());
        }
        "nn-models" => { println!("{}", crate::nn_inference::list_models()); }
        "nn-demo" => { println!("{}", crate::nn_inference::demo_inference()); }
        "ml-demo" => {
            println!("{}", crate::ml_train::demo_linear());
            println!("{}", crate::ml_train::demo_classify());
        }
        "ml-stats" => { println!("{}", crate::ml_train::ml_stats()); }
        "vstore" => { println!("{}", crate::vector_store::store_stats()); println!("{}", crate::vector_store::list_documents()); }
        "workflow" => { println!("{}", crate::ai_workflow::list_workflows()); }
        "workflow-demo" => { println!("{}", crate::ai_workflow::demo()); }
        "evolve" => { println!("{}", crate::self_evolve::analyze_all()); }
        "findings" => { println!("{}", crate::self_evolve::list_findings()); }
        "patches" => { println!("{}", crate::self_evolve::list_patches()); }
        "evolve-stats" => { println!("{}", crate::self_evolve::evolve_stats()); }
        cmd if cmd.starts_with("vsearch ") => {
            let query = cmd.strip_prefix("vsearch ").unwrap().trim();
            let results = crate::vector_store::search(query, 5);
            println!("{}", crate::vector_store::format_results(&results));
        }
        cmd if cmd.starts_with("nn-infer ") => {
            let args = cmd.strip_prefix("nn-infer ").unwrap().trim();
            let parts: alloc::vec::Vec<&str> = args.splitn(2, ' ').collect();
            if parts.len() == 2 {
                let model = parts[0];
                let inputs: alloc::vec::Vec<i32> = parts[1].split(',')
                    .filter_map(|s| s.trim().parse::<i32>().ok())
                    .collect();
                match crate::nn_inference::run_inference(model, &inputs) {
                    Ok(output) => println!("Output: {:?}", output),
                    Err(e) => println!("nn-infer: {}", e),
                }
            } else {
                println!("Usage: nn-infer <model> <input1,input2,...>");
            }
        }
        "panic" => panic!("user-triggered panic via shell"),
        "gpu-info" => { println!("{}", crate::gpu::gpu_info()); }
        "gpu-stats" => { println!("{}", crate::gpu::gpu_stats()); }
        "gpu-bench" => { println!("{}", crate::gpu::benchmark()); }
        "gpu-buffers" => { println!("{}", crate::gpu::list_buffers()); }
        "bt-info" => { println!("{}", crate::bluetooth::bt_info()); }
        "bt-scan" => { crate::bluetooth::scan_start(); println!("Scanning for Bluetooth devices..."); println!("{}", crate::bluetooth::list_devices()); }
        "bt-devices" => { println!("{}", crate::bluetooth::list_devices()); }
        "bt-stats" => { println!("{}", crate::bluetooth::bt_stats()); }
        "dfs-info" => { println!("{}", crate::dfs::dfs_info()); }
        "dfs-nodes" => { println!("{}", crate::dfs::list_nodes()); }
        "dfs-mounts" => { println!("{}", crate::dfs::list_mounts()); }
        "dfs-stats" => { println!("{}", crate::dfs::dfs_stats()); }
        "rt-tasks" => { println!("{}", crate::rt_sched::list_rt_tasks()); }
        "rt-stats" => { println!("{}", crate::rt_sched::rt_stats()); }
        "rt-test" => { println!("{}", crate::rt_sched::schedulability_test()); }
        "services" => { println!("{}", crate::microkernel::list_services()); }
        "ukernel" => { println!("{}", crate::microkernel::ukernel_stats()); }
        "ukernel-stats" => { println!("{}", crate::microkernel::ukernel_stats()); }
        "ukernel on" => { crate::microkernel::enable_microkernel_mode(); println!("Microkernel mode enabled."); }
        "ukernel off" => { crate::microkernel::disable_microkernel_mode(); println!("Microkernel mode disabled."); }
        "health" => { println!("{}", crate::microkernel::health_check()); }
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
