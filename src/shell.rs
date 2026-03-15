/// Interactive kernel shell with command history.
/// Supports arrow keys (up/down for history, left/right planned),
/// shift for uppercase, and output redirection (cmd > file).

use crate::{print, println, serial_println, allocator, timer, task, process, ipc, vfs, memory, driver, acpi, rtc, testutil, framebuf, pci, ramdisk, net, netproto, smp, env, module, slab, ksyms, paging, virtio, blkdev, fat, fd, locks, ai_shell};
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

                if let Some((left, right)) = cmd.split_once(" > ") {
                    dispatch(left.trim());
                    let _ = vfs::write(right.trim(), left.trim());
                } else {
                    dispatch(cmd);
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
            println!("  lockdemo   - spinlock vs ticket lock");
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
            println!("  virtio     - virtio devices");
            println!("  blkdevs    - block devices");
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
            println!("  shutdown   - power off");
            println!("  reboot     - restart");
            println!("  panic      - trigger panic");
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
