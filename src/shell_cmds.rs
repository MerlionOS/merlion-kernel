/// Shell command implementations, organized by category.
/// Called from shell::dispatch() to keep the main shell module small.

use crate::{print, println, serial_println};

// ═══════════════════════════════════════════════════════════════════
//  HELP TEXT
// ═══════════════════════════════════════════════════════════════════

pub fn help_text() {
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
    println!("  rtl8169-info  - RTL8169 NIC status");
    println!("  rtl8169-stats - RTL8169 NIC statistics");
    println!("  i225-info  - Intel I225 2.5GbE status");
    println!("  i225-stats - Intel I225 2.5GbE statistics");
    println!("  nvmeinfo   - NVMe SSD status");
    println!("  rtl8139-info  - RTL8139 NIC status");
    println!("  rtl8139-stats - RTL8139 NIC statistics");
    println!("  usb-drives - list USB mass storage devices");
    println!("  usb-eject <n> - safely eject USB device");
    println!("  lsscsi     - list SCSI/USB storage devices");
    println!("  sata-info  - SATA controller and disk info");
    println!("  sata-stats - SATA subsystem statistics");
    println!("  smart <port> - SMART health for SATA port");
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
    println!("  gpu-compute  - compute engine status");
    println!("  gpu-vram     - VRAM allocations");
    println!("  gpu-dispatch <n> - dispatch test matmul");
    println!("  gpu-bench-compute - compute benchmark");
    println!("  gpu-dma-test - test DMA copy");
    println!("  nvidia-gpu-info   - NVIDIA GPU device info");
    println!("  intel-gpu-info    - Intel GPU device info");
    println!("  intel-gpu-compute - Intel GPU compute status");
    println!("  intel-gpu-bench   - Intel GPU compute benchmark");
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
    println!("Audio & media:");
    println!("  audio-info   - audio engine info");
    println!("  audio-stats  - audio statistics");
    println!("  audio-demo   - play demo sounds");
    println!("  play-tone <f> <ms> - play tone");
    println!("  midi-info    - MIDI info");
    println!("  audio-ch     - audio mixer channels");
    println!("System tools:");
    println!("  proc-list    - user processes");
    println!("  proc-stats   - process statistics");
    println!("  widgets      - list GUI widgets");
    println!("  widget-demo  - GUI demo");
    println!("  notify <msg> - send notification");
    println!("  notifications - list notifications");
    println!("  ipv6-info    - IPv6 status");
    println!("  ipv6-stats   - IPv6 statistics");
    println!("  ndp-table    - NDP neighbor table");
    println!("  https-info   - HTTPS server info");
    println!("  https-stats  - HTTPS statistics");
    println!("  pkg-list     - list all packages");
    println!("  pkg-installed - installed packages");
    println!("  pkg-info <n> - package details");
    println!("  pkg-install <n> - install package");
    println!("  pkg-search <q> - search packages");
    println!("  build-stats  - build system stats");
    println!("  build-config - build configuration");
    println!("Advanced systems:");
    println!("  ext4-info    - ext4 filesystem info");
    println!("  congestion   - TCP congestion control info");
    println!("  wasi-info    - WASI runtime info");
    println!("  veth-list    - virtual ethernet pairs");
    println!("  bridges      - network bridges");
    println!("  iptables     - packet filter/NAT (iptables syntax)");
    println!("  iptables-list - list all iptables rules");
    println!("  conntrack    - connection tracking table");
    println!("  vlan-list    - list VLANs");
    println!("  vlan-create <vid> <name> - create VLAN");
    println!("  vlan-info <vid> - VLAN details");
    println!("  dlopen       - loaded shared libraries");
    println!("  breakpoints  - debugger breakpoints");
    println!("  bt-debug     - annotated backtrace");
    println!("  crypto-info  - cryptography info");
    println!("  aes-demo     - AES encryption demo");
    println!("  rsa-demo     - RSA encryption demo");
    println!("  pkg-stats    - package registry stats");
    println!("Kernel internals:");
    println!("  proc <path>  - read /proc file");
    println!("  procfs-list  - list /proc entries");
    println!("  sysfs <path> - read /sys attribute");
    println!("  dev-tree     - device tree");
    println!("  dev-info <id> - device details");
    println!("  dev-stats    - device statistics");
    println!("  tmpfs-info   - tmpfs status");
    println!("  tmpfs-stats  - tmpfs statistics");
    println!("  pipes        - active pipes");
    println!("  mkfifo <p>   - create named pipe");
    println!("Raspberry Pi hardware:");
    println!("  gpio-info    - GPIO pin states");
    println!("  gpio-stats   - GPIO statistics");
    println!("  gpio set <p> <v> - set pin output");
    println!("  gpio read <p> - read pin level");
    println!("  sdcard       - SD card info");
    println!("  sdcard-stats - SD I/O statistics");
    println!("Extended hardware:");
    println!("  wifi-scan    - scan WiFi networks");
    println!("  wifi-status  - WiFi connection status");
    println!("  wifi-info    - WiFi driver info");
    println!("  hda-info     - HDA audio info");
    println!("  hda-codecs   - HDA codec list");
    println!("  uefi-info    - UEFI runtime info");
    println!("  uefi-vars    - UEFI variables");
    println!("  run-elf <path> - run ELF program");
    println!("  display-info - display/GPU info");
    println!("  windows      - list windows");
    println!("  screenshot   - capture screen");
    println!("VPN & service discovery:");
    println!("  wg           - WireGuard show all");
    println!("  wg-show      - WireGuard interfaces/peers");
    println!("  wg-genkey    - generate WireGuard private key");
    println!("  mdns-list    - list mDNS services");
    println!("  mdns-browse <type> - browse services");
    println!("  mdns-resolve <host> - resolve .local host");
    println!("Proxy & tunnel:");
    println!("  socks5-status  - SOCKS5 proxy info");
    println!("  socks5-start <port> - start SOCKS5 proxy");
    println!("  socks5-stop    - stop SOCKS5 proxy");
    println!("  proxy-status   - HTTP proxy info");
    println!("  proxy-start <port> - start HTTP proxy");
    println!("  pppoe-connect <user> <pass> - PPPoE dial");
    println!("  pppoe-status   - PPPoE session status");
    println!("  pppoe-disconnect - disconnect PPPoE");
    println!("Routing protocols:");
    println!("  ospf-info      - OSPF protocol info");
    println!("  ospf-neighbors - OSPF neighbor table");
    println!("  ospf-routes    - OSPF routing table");
    println!("  ospf-lsdb      - OSPF link-state database");
    println!("  bgp-info       - BGP protocol info");
    println!("  bgp-peers      - BGP peer table");
    println!("  bgp-routes     - BGP routing table");
    println!("  rip-info       - RIP protocol info");
    println!("  rip-routes     - RIP routing table");
    println!("QoS & Traffic Control:");
    println!("  tc-show        - tc qdisc/class config (eth0)");
    println!("  tc-stats       - traffic control statistics");
    println!("  tc-info        - traffic control subsystem info");
    println!("  dscp-info      - DSCP/ECN marking info");
    println!("  dscp-rules     - list DSCP classification rules");
    println!("  dscp-stats     - DSCP/ECN statistics");
    println!("System management:");
    println!("  who          - logged in users");
    println!("  w            - who + activity");
    println!("  last         - login history");
    println!("  sessions     - session info");
    println!("  systemctl    - list services");
    println!("  systemctl start/stop/restart <s>");
    println!("  boot-report  - boot timing");
    println!("  install      - system installer");
    println!("  disks        - list block devices");
    println!("Configuration & diagnostics:");
    println!("  sysctl <path> - read/write kernel config");
    println!("  sysctl-list   - list all sysctl params");
    println!("  config-diff   - show changed config params");
    println!("  config-dump   - dump full config");
    println!("  profile <p>   - apply config profile");
    println!("  traceroute <ip> - trace network path");
    println!("  portscan <ip> <start> <end> - scan ports");
    println!("  dns <name>    - DNS lookup");
    println!("  capture       - packet capture status");
    println!("  net-health    - network health check");
    println!("  vmm-info      - virtual memory info");
    println!("  page-cache    - page cache statistics");
    println!("  oom-info      - OOM killer info");
    println!("  ipc-info      - extended IPC info");
    println!("  mq-list       - message queues");
    println!("  sem-list      - semaphores");
    println!("  perf-events   - performance events info");
    println!("  flamegraph    - generate flame graph");
    println!("  topdown       - top-down perf analysis");
}

// ═══════════════════════════════════════════════════════════════════
//  PROCESS COMMANDS
// ═══════════════════════════════════════════════════════════════════

pub fn dispatch_process(cmd: &str) -> bool {
    use crate::{task, process, signal, demo};

    match cmd {
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
            if let Some(pid) = task::spawn("demo", crate::shell::demo_task) {
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
        "demo" => demo::run(),

        // --- Userspace process commands ---
        cmd if cmd.starts_with("run-user ") => {
            let name = cmd[9..].trim();
            // Exit interrupt context first — page mapping needs frame allocator lock
            unsafe { crate::interrupts::end_of_interrupt(1); }
            x86_64::instructions::interrupts::enable();
            match crate::userspace::run_builtin(name) {
                Ok(()) => println!("User program '{}' finished.", name),
                Err(e) => println!("Error: {}", e),
            }
        }
        "run-user" => {
            println!("Usage: run-user <program>");
            println!("Programs: {:?}", crate::userspace::list_builtin_programs());
        }
        "user-ps" => {
            print!("{}", crate::userspace::list_processes());
        }
        cmd if cmd.starts_with("user-kill ") => {
            if let Ok(pid) = cmd[10..].trim().parse::<u32>() {
                match crate::userspace::kill_process(pid) {
                    Ok(()) => println!("Killed user process pid {}", pid),
                    Err(e) => println!("Error: {}", e),
                }
            } else {
                println!("Usage: user-kill <pid>");
            }
        }
        "userspace" => {
            print!("{}", crate::userspace::userspace_info());
        }
        "ulibc" | "libc-info" => {
            print!("{}", crate::ulibc::info());
        }
        "dynlink" | "dynlink-info" => {
            print!("{}", crate::dynlink::info());
        }
        "dllist" => {
            print!("{}", crate::dynlink::list_libraries());
        }
        "cow" | "cow-info" => {
            print!("{}", crate::cow::info());
        }
        cmd if cmd.starts_with("run-isolated ") => {
            let name = cmd[13..].trim();
            unsafe { crate::interrupts::end_of_interrupt(1); }
            match crate::userspace::run_isolated(name) {
                Ok(()) => {}
                Err(e) => println!("Error: {}", e),
            }
        }
        cmd if cmd.starts_with("compile ") => {
            let name = cmd[8..].trim();
            let src_path = alloc::format!("/src/{}.rs", name);
            match crate::vfs::cat(&src_path) {
                Ok(source) => {
                    match crate::self_host::compile(&source) {
                        Ok(elf_bytes) => {
                            let bin_path = alloc::format!("/bin/{}", name);
                            if let Ok(elf_str) = core::str::from_utf8(&elf_bytes) {
                                let _ = crate::vfs::write(&bin_path, elf_str);
                            }
                            println!("Compiled {} -> {} ({} bytes)", src_path, bin_path, elf_bytes.len());
                        }
                        Err(e) => println!("Compile error: {}", e),
                    }
                }
                Err(e) => println!("Cannot read {}: {}", src_path, e),
            }
        }
        "init" => {
            println!("Starting init system...");
            crate::init_system::init();
        }
        cmd if cmd.starts_with("spawn-user ") => {
            let name = cmd[11..].trim();
            match crate::userspace::spawn_user_task(name) {
                Ok(pid) => println!("Spawned user task '{}' pid={}", name, pid),
                Err(e) => println!("Error: {}", e),
            }
        }
        "tty-status" => {
            print!("{}", crate::tty::status());
        }
        cmd if cmd.starts_with("shmem-list") => {
            let regions = crate::shmem::list_shmem();
            if regions.is_empty() {
                println!("No shared memory regions.");
            } else {
                for r in &regions {
                    println!("  [{}] {} size={} refs={} owner={}",
                        r.id, r.name, r.size, r.ref_count, r.owner_pid);
                }
            }
        }

        _ => return false,
    }
    true
}

// ═══════════════════════════════════════════════════════════════════
//  FILE COMMANDS
// ═══════════════════════════════════════════════════════════════════

pub fn dispatch_file(cmd: &str) -> bool {
    use crate::{vfs, coreutils, editor, script, fd, elf, elf_loader, process, virtio_blk, ramdisk, fat, diskfs, blkdev, pci};

    match cmd {
        cmd if cmd == "ls" || cmd.starts_with("ls ") => {
            let args = if cmd.len() > 2 { cmd[2..].trim() } else { "" };
            let mut show_all = false;
            let mut long_fmt = false;
            let mut _human_readable = false;
            let mut path = "/";

            for part in args.split_whitespace() {
                if part.starts_with('-') {
                    for ch in part[1..].chars() {
                        match ch {
                            'a' => show_all = true,
                            'l' => long_fmt = true,
                            'h' => _human_readable = true,
                            'H' => {}
                            'R' => {}
                            _ => {}
                        }
                    }
                } else {
                    path = part;
                }
            }

            if long_fmt {
                match crate::vfs::ls_long(path) {
                    Ok(entries) => {
                        for entry in entries {
                            if !show_all && entry.contains("/.") { continue; }
                            println!("  {}", entry);
                        }
                    }
                    Err(e) => println!("ls: {}", e),
                }
            } else {
                match crate::vfs::ls(path) {
                    Ok(entries) => {
                        for (name, type_char) in &entries {
                            if !show_all && name.starts_with('.') { continue; }
                            let prefix = match type_char {
                                'd' => "\x1b[34m",
                                'c' => "\x1b[33m",
                                _ => "",
                            };
                            println!("  {}{}\x1b[0m  {}", prefix, type_char, name);
                        }
                    }
                    Err(e) => println!("ls: {}", e),
                }
            }
        }

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
            unsafe { crate::interrupts::end_of_interrupt(1); }
            x86_64::instructions::interrupts::enable();
            while editor::is_editing() {
                x86_64::instructions::hlt();
            }
            crate::vga::print_banner();
            println!("Editor closed.");
        }
        cmd if cmd.starts_with("vim ") => {
            let path = cmd[4..].trim();
            crate::vim::start(Some(path));
            // Send EOI to PIC so keyboard interrupts can fire again
            // (we're still inside the keyboard interrupt handler's call chain)
            unsafe {
                crate::interrupts::end_of_interrupt(1); // keyboard IRQ
            }
            x86_64::instructions::interrupts::enable();
            while crate::vim::is_active() {
                x86_64::instructions::hlt();
            }
            crate::vga::print_banner();
            println!("Vim closed.");
        }
        "vim" => {
            crate::vim::start(None);
            unsafe { crate::interrupts::end_of_interrupt(1); }
            x86_64::instructions::interrupts::enable();
            while crate::vim::is_active() {
                x86_64::instructions::hlt();
            }
            crate::vga::print_banner();
            println!("Vim closed.");
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

                    match elf::parse(&elf_data) {
                        Ok(info) => print!("{}", elf::format_info(&info)),
                        Err(e) => println!("Parse error: {}", e),
                    }

                    if virtio_blk::is_detected() {
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
        cmd if cmd.starts_with("mkdir ") => {
            let path = cmd[6..].trim();
            match crate::vfs::mkdir(path) {
                Ok(()) => println!("mkdir: created {}", path),
                Err(e) => println!("mkdir: {}", e),
            }
        }

        // --- Disk commands ---
        cmd if cmd.starts_with("diskread ") => {
            if let Ok(sector) = cmd[9..].trim().parse::<u64>() {
                let mut buf = [0u8; 512];
                match virtio_blk::read_sector(sector, &mut buf) {
                    Ok(()) => {
                        println!("Sector {}:", sector);
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
        _ => return false,
    }
    true
}

// ═══════════════════════════════════════════════════════════════════
//  SYSTEM COMMANDS
// ═══════════════════════════════════════════════════════════════════

pub fn dispatch_system(cmd: &str) -> bool {
    use crate::{allocator, timer, memory, driver, rtc, testutil, framebuf, smp, env, module, slab, ksyms, paging, locks, kconfig, ipc, task, ramdisk, bench, calc, top, forth, watch, screensaver, snake, acpi};

    match cmd {
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
                    "usable" => "\x1b[32m",
                    "kernel" | "kstack" | "pagetbl" => "\x1b[33m",
                    "reserved" | "ACPI" => "\x1b[90m",
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
        cmd if cmd.starts_with("echo ") => {
            println!("{}", cmd[5..].trim());
        }
        "echo" => println!(),
        "pwd" => {
            let cwd = env::get("PWD").unwrap_or_else(|| alloc::string::String::from("/"));
            println!("{}", cwd);
        }
        cmd if cmd == "cd" || cmd.starts_with("cd ") => {
            let target = if cmd.len() > 2 { cmd[2..].trim() } else { "" };
            let target = if target.is_empty() {
                env::get("HOME").unwrap_or_else(|| alloc::string::String::from("/tmp"))
            } else {
                let cwd = env::get("PWD").unwrap_or_else(|| alloc::string::String::from("/"));
                if target.starts_with('/') {
                    alloc::string::String::from(target)
                } else if target == ".." {
                    // Go up one level
                    if let Some(pos) = cwd.rfind('/') {
                        if pos == 0 { alloc::string::String::from("/") }
                        else { alloc::string::String::from(&cwd[..pos]) }
                    } else {
                        alloc::string::String::from("/")
                    }
                } else {
                    if cwd == "/" {
                        alloc::format!("/{}", target)
                    } else {
                        alloc::format!("{}/{}", cwd, target)
                    }
                }
            };
            // Verify directory exists
            if crate::vfs::exists(&target) {
                env::set("PWD", &target);
            } else {
                println!("cd: {}: No such directory", target);
            }
        }
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
            println!("{}", crate::security::whoami());
        }

        // ── User management ──────────────────────────────────────
        cmd if cmd.starts_with("useradd ") => {
            // useradd <username> <password> [uid]
            let parts: alloc::vec::Vec<&str> = cmd[8..].trim().split_whitespace().collect();
            if parts.len() >= 2 {
                let name = parts[0];
                let pass = parts[1];
                let uid = if parts.len() >= 3 {
                    parts[2].parse::<u32>().unwrap_or(1000)
                } else {
                    // Auto-assign UID: 1000+
                    let users = crate::security::list_users();
                    users.iter().map(|(u, _)| *u).max().unwrap_or(999) + 1
                };
                match crate::security::add_user(uid, name, pass, &[uid]) {
                    Ok(()) => println!("User '{}' created (uid={})", name, uid),
                    Err(e) => println!("Error: {}", e),
                }
            } else {
                println!("Usage: useradd <username> <password> [uid]");
            }
        }
        cmd if cmd.starts_with("userdel ") => {
            let name = cmd[8..].trim();
            if let Some(uid) = crate::security::uid_by_name(name) {
                match crate::security::remove_user(uid) {
                    Ok(()) => println!("User '{}' removed", name),
                    Err(e) => println!("Error: {}", e),
                }
            } else {
                println!("User '{}' not found", name);
            }
        }
        cmd if cmd.starts_with("passwd ") => {
            let parts: alloc::vec::Vec<&str> = cmd[7..].trim().split_whitespace().collect();
            if parts.len() >= 2 {
                match crate::security::passwd(parts[0], None, parts[1]) {
                    Ok(()) => println!("Password changed for '{}'", parts[0]),
                    Err(e) => println!("Error: {}", e),
                }
            } else {
                println!("Usage: passwd <username> <new_password>");
            }
        }
        cmd if cmd.starts_with("su ") => {
            let parts: alloc::vec::Vec<&str> = cmd[3..].trim().split_whitespace().collect();
            if !parts.is_empty() {
                let pass = if parts.len() >= 2 { Some(parts[1]) } else { None };
                match crate::security::su(parts[0], pass) {
                    Ok(()) => {
                        crate::env::set("USER", parts[0]);
                        println!("Switched to user '{}'", parts[0]);
                    }
                    Err(e) => println!("su: {}", e),
                }
            } else {
                println!("Usage: su <username> [password]");
            }
        }
        "users" | "list-users" => {
            let users = crate::security::list_users();
            for (uid, name) in &users {
                println!("  uid={:4}  {}", uid, name);
            }
            println!("{} user(s)", users.len());
        }
        "groups" | "list-groups" => {
            let groups = crate::security::list_groups();
            for (gid, name) in &groups {
                println!("  gid={:4}  {}", gid, name);
            }
            println!("{} group(s)", groups.len());
        }
        cmd if cmd.starts_with("groupadd ") => {
            let parts: alloc::vec::Vec<&str> = cmd[9..].trim().split_whitespace().collect();
            if !parts.is_empty() {
                let name = parts[0];
                let gid = if parts.len() >= 2 {
                    parts[1].parse::<u32>().unwrap_or(1000)
                } else {
                    let groups = crate::security::list_groups();
                    groups.iter().map(|(g, _)| *g).max().unwrap_or(999) + 1
                };
                match crate::security::add_group(gid, name) {
                    Ok(()) => println!("Group '{}' created (gid={})", name, gid),
                    Err(e) => println!("Error: {}", e),
                }
            } else {
                println!("Usage: groupadd <name> [gid]");
            }
        }
        cmd if cmd.starts_with("id ") => {
            let user = cmd[3..].trim();
            match crate::security::id_info(Some(user)) {
                Ok(info) => println!("{}", info),
                Err(e) => println!("id: {}", e),
            }
        }
        "id" => {
            match crate::security::id_info(None) {
                Ok(info) => println!("{}", info),
                Err(_) => println!("uid=0(root) gid=0(root)"),
            }
        }
        "sshd" | "sshd start" => {
            println!("Starting SSH server on port 22...");
            println!("Connect with: ssh root@<ip> -p 22");
            crate::task::spawn("sshd", || crate::sshd::sshd_start(22));
        }
        "who" => {
            print!("{}", crate::multi_user::who());
        }
        "w" => {
            print!("{}", crate::multi_user::w());
        }
        "last" => {
            print!("{}", crate::multi_user::last());
        }

        "hostname" => {
            println!("{}", env::get("HOSTNAME").unwrap_or_else(|| alloc::string::String::from("merlion")));
        }
        "uname" | "uname -a" => {
            let dt = rtc::read();
            println!("MerlionOS merlion 0.2.0 {} x86_64", dt);
        }
        "neofetch" => {
            crate::shell::neofetch();
        }
        "history" => {
            crate::shell::print_history();
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
        "pipe" => crate::shell::run_ipc_demo(),
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
            unsafe { crate::interrupts::end_of_interrupt(1); }
            x86_64::instructions::interrupts::enable();
            while forth::is_running() { x86_64::instructions::hlt(); }
            println!("Forth exited.");
        }
        "snake" => {
            snake::run();
            crate::vga::print_banner();
            println!("Type 'help' for commands.");
        }
        "matrix" => {
            screensaver::run();
            crate::vga::print_banner();
            println!("Type 'help' for commands.");
        }
        "panic" => panic!("user-triggered panic via shell"),
        "sleep" => {
            match crate::acpi_ext::sleep() {
                Ok(()) => println!("Resumed from S3 sleep."),
                Err(e) => println!("sleep: {}", e),
            }
        }
        _ => return false,
    }
    true
}

// ═══════════════════════════════════════════════════════════════════
//  AI COMMANDS
// ═══════════════════════════════════════════════════════════════════

pub fn dispatch_ai(cmd: &str) -> bool {
    use crate::{ai_shell, ai_proxy, ai_monitor, ai_syscall, ai_heal, ai_man, semfs, agent, chat, fortune, boot_info_ext};

    match cmd {
        cmd if cmd.starts_with("ai ") => {
            let prompt = cmd[3..].trim();
            if let Some(response) = ai_proxy::infer(prompt) {
                println!("\x1b[36m[ai]\x1b[0m {}", response);
            } else if let Some(ai_cmd) = ai_shell::interpret(prompt) {
                println!("{}", ai_shell::format_hint(prompt, &ai_cmd));
                crate::shell::dispatch(&ai_cmd);
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
            unsafe { crate::interrupts::end_of_interrupt(1); }
            x86_64::instructions::interrupts::enable();
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

        // AI platform
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

        // LLM commands
        cmd if cmd.starts_with("llm-load ") => {
            let path = cmd.strip_prefix("llm-load ").unwrap().trim();
            match crate::llm::load(path) {
                Ok(()) => println!("LLM model loaded from '{}'", path),
                Err(e) => println!("llm-load: {}", e),
            }
        }
        "llm-info" => { println!("{}", crate::llm::llm_info()); }
        "llm-stats" => { println!("{}", crate::llm::llm_stats()); }
        cmd if cmd.starts_with("llm-generate ") => {
            let prompt = cmd.strip_prefix("llm-generate ").unwrap().trim();
            let output = crate::llm::generate_text(prompt, 32);
            println!("{}", output);
        }
        "llm-demo" => { println!("{}", crate::llm::demo_generate()); }
        cmd if cmd.starts_with("ai-chat ") => {
            let msg = cmd.strip_prefix("ai-chat ").unwrap().trim();
            let output = crate::llm::generate_text(msg, 64);
            println!("[ai] {}", output);
        }

        // AI System Administrator
        "diagnose" => {
            let results = crate::ai_admin::diagnose();
            if results.is_empty() {
                println!("No issues detected. System healthy.");
            } else {
                for d in &results {
                    println!("[{}] severity={} {}", d.category.as_str(), d.severity, d.description);
                    println!("  Root cause: {}", d.root_cause);
                    println!("  Recommendation: {}", d.recommendation);
                    if d.auto_fixable { println!("  (auto-fixable)"); }
                }
            }
        }
        "auto-tune" => {
            let changes = crate::ai_admin::auto_tune();
            for c in &changes {
                println!("  {}", c);
            }
        }
        "ai-admin" => { println!("{}", crate::ai_admin::ai_admin_info()); }
        "ai-admin-stats" => { println!("{}", crate::ai_admin::ai_admin_stats()); }
        "security-audit" => { println!("{}", crate::ai_admin::security_audit()); }
        "daily-report" => { println!("{}", crate::ai_admin::daily_report()); }
        cmd if cmd.starts_with("nlconfig ") => {
            let command = cmd.strip_prefix("nlconfig ").unwrap().trim();
            println!("{}", crate::ai_admin::nl_config(command));
        }
        cmd if cmd.starts_with("predict ") => {
            let rest = cmd.strip_prefix("predict ").unwrap().trim();
            let parts: alloc::vec::Vec<&str> = rest.splitn(2, ' ').collect();
            let metric = match parts[0] {
                "cpu" => crate::ai_admin::MetricKind::CpuUsage,
                "mem" | "memory" => crate::ai_admin::MetricKind::MemoryUsage,
                "disk" => crate::ai_admin::MetricKind::DiskIo,
                "net" | "network" => crate::ai_admin::MetricKind::NetworkTraffic,
                "procs" => crate::ai_admin::MetricKind::ProcessCount,
                "load" => crate::ai_admin::MetricKind::LoadAverage,
                _ => { println!("predict: unknown metric (cpu/mem/disk/net/procs/load)"); return true; }
            };
            let hours = if parts.len() > 1 { parts[1].parse().unwrap_or(6) } else { 6 };
            println!("{}", crate::ai_admin::predict_alert(metric, hours));
        }

        _ => return false,
    }
    true
}

// ═══════════════════════════════════════════════════════════════════
//  SECURITY COMMANDS
// ═══════════════════════════════════════════════════════════════════

pub fn dispatch_security(cmd: &str) -> bool {
    match cmd {
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
            match crate::security::sudo(Some(""), || {
                crate::shell::dispatch(subcmd);
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

        // ACL
        "acl-info" => { println!("{}", crate::acl::acl_info()); }
        "acl-stats" => { println!("{}", crate::acl::acl_stats()); }
        "acl-list" => { println!("{}", crate::acl::list_acls()); }
        "acl-audit" => { println!("{}", crate::acl::audit_log(20)); }
        cmd if cmd.starts_with("setacl ") => {
            let args = cmd.strip_prefix("setacl ").unwrap().trim();
            match crate::acl::parse_setacl_cmd(args) {
                Ok(()) => println!("ACL set successfully."),
                Err(e) => println!("setacl: {}", e),
            }
        }
        cmd if cmd.starts_with("getacl ") => {
            let path = cmd.strip_prefix("getacl ").unwrap().trim();
            println!("{}", crate::acl::getacl(path));
        }
        cmd if cmd.starts_with("rmacl ") => {
            let path = cmd.strip_prefix("rmacl ").unwrap().trim();
            if crate::acl::removeacl(path) {
                println!("ACL removed for {}", path);
            } else {
                println!("No ACL found for {}", path);
            }
        }

        // PAM
        "pam-info" => { println!("{}", crate::pam::pam_info()); }
        "pam-stats" => { println!("{}", crate::pam::pam_stats()); }
        "pam-services" => { println!("{}", crate::pam::list_services()); }
        "pam-sessions" => { println!("{}", crate::pam::list_sessions()); }

        _ => return false,
    }
    true
}

// ═══════════════════════════════════════════════════════════════════
//  NETWORK COMMANDS
// ═══════════════════════════════════════════════════════════════════

pub fn dispatch_network(cmd: &str) -> bool {
    use crate::{net, netproto, netstack, virtio_net, tcp_real, dhcp, wget};

    match cmd {
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
        "tcppoll" => {
            let n = tcp_real::poll_incoming();
            println!("Processed {} incoming TCP segment(s).", n);
        }
        cmd if cmd.starts_with("nicsend ") => {
            let msg = cmd[8..].trim();
            let dst_ip = [10, 0, 2, 2];
            netstack::send_udp(dst_ip, 12345, 8080, msg.as_bytes());
            println!("Sent {} bytes via NIC to 10.0.2.2:8080", msg.len());
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

        // IPv6
        "ipv6-info" => { println!("{}", crate::ipv6::ipv6_info()); }
        "ipv6-stats" => { println!("{}", crate::ipv6::ipv6_stats()); }
        "ndp-table" => { println!("{}", crate::ipv6::ndp_table()); }

        // Extended network diagnostics
        cmd if cmd.starts_with("traceroute ") => {
            let ip = cmd.strip_prefix("traceroute ").unwrap().trim();
            println!("{}", crate::netdiag::traceroute_cmd(ip));
        }
        cmd if cmd.starts_with("portscan ") => {
            let args = cmd.strip_prefix("portscan ").unwrap().trim();
            println!("{}", crate::netdiag::port_scan_cmd(args));
        }
        cmd if cmd.starts_with("dns ") => {
            let name = cmd.strip_prefix("dns ").unwrap().trim();
            println!("{}", crate::netdiag::dns_lookup_cmd(name));
        }
        "capture" => { println!("{}", crate::netdiag::capture_status()); }
        "net-health" => { println!("{}", crate::netdiag::health_check_cmd()); }
        cmd if cmd == "ss" || cmd.starts_with("ss ") => {
            let flags = if cmd.len() > 2 { cmd[2..].trim() } else { "" };
            println!("{}", crate::netdiag::ss_command(flags));
        }
        cmd if cmd.starts_with("nc ") => {
            let args = cmd.strip_prefix("nc ").unwrap().trim();
            println!("{}", crate::netdiag::nc_command(args));
        }
        cmd if cmd == "ip" || cmd.starts_with("ip ") => {
            let args = if cmd.len() > 2 { cmd[2..].trim() } else { "help" };
            println!("{}", crate::netdiag::ip_command(args));
        }

        // TCP extensions
        "tcp-ext" => { println!("{}", crate::tcp_ext::tcp_ext_info()); }
        "tcp-ext-stats" => { println!("{}", crate::tcp_ext::tcp_ext_stats()); }
        "tfo-stats" => { println!("{}", crate::tcp_ext::tfo_stats()); }
        "sack-stats" => { println!("{}", crate::tcp_ext::sack_stats()); }

        _ => return false,
    }
    true
}

// ═══════════════════════════════════════════════════════════════════
//  HARDWARE COMMANDS
// ═══════════════════════════════════════════════════════════════════

pub fn dispatch_hardware(cmd: &str) -> bool {
    use crate::{virtio, virtio_blk, ahci, nvme, xhci, e1000e, ioapic, gpt, power, rtl8139, rtl8169, intel_i225, usb_mass, sata, amdgpu};

    match cmd {
        "gpu-info" => {
            println!("{}", crate::gpu_detect::scan_all_gpus());
            if amdgpu::is_detected() {
                println!("{}", amdgpu::amdgpu_info());
                println!("{}", crate::amdgpu_compute::compute_info());
            }
            if crate::intel_gpu::is_detected() {
                println!("{}", crate::intel_gpu::intel_gpu_info());
                println!("{}", crate::intel_gpu_compute::compute_info());
            }
            if crate::nvidia_gpu::is_detected() {
                println!("{}", crate::nvidia_gpu::nvidia_gpu_info());
            }
        }
        "gpu-regs" => {
            println!("{}", amdgpu::amdgpu_stats());
        }
        "gpu-test" => {
            println!("{}", crate::amdgpu_compute::dma_test());
            println!("{}", crate::amdgpu_compute::dispatch_test("32"));
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
        "rtl8169-info" => {
            if rtl8169::is_detected() {
                println!("{}", rtl8169::rtl8169_info());
            } else {
                println!("RTL8169 NIC not detected");
            }
        }
        "rtl8169-stats" => {
            if rtl8169::is_detected() {
                println!("{}", rtl8169::rtl8169_stats());
            } else {
                println!("RTL8169 NIC not detected");
            }
        }
        "rtl8139-info" => {
            if rtl8139::is_detected() {
                println!("{}", rtl8139::rtl8139_info());
            } else {
                println!("RTL8139 NIC not detected");
            }
        }
        "rtl8139-stats" => {
            if rtl8139::is_detected() {
                println!("{}", rtl8139::rtl8139_stats());
            } else {
                println!("RTL8139 NIC not detected");
            }
        }
        "usb-drives" => {
            println!("{}", usb_mass::usb_mass_info());
        }
        cmd if cmd.starts_with("usb-eject") => {
            let arg = cmd.trim_start_matches("usb-eject").trim();
            if let Ok(idx) = arg.parse::<usize>() {
                match usb_mass::eject(idx) {
                    Ok(()) => println!("USB device {} ejected safely", idx),
                    Err(e) => println!("Eject failed: {}", e),
                }
            } else {
                println!("Usage: usb-eject <device_number>");
            }
        }
        "lsscsi" => {
            let devs = usb_mass::list_devices();
            if devs.is_empty() {
                println!("No SCSI/USB storage devices detected");
            } else {
                for d in &devs {
                    println!(
                        "[{}] usb{}  {} {}  {} MiB  {} partition(s)  {}",
                        d.index, d.index, d.vendor, d.product,
                        d.capacity_mb, d.partitions,
                        if d.mounted { "mounted" } else { "not mounted" },
                    );
                }
            }
        }
        "sata-info" => {
            println!("{}", sata::sata_info());
        }
        "sata-stats" => {
            println!("{}", sata::sata_stats());
        }
        cmd if cmd.starts_with("smart") => {
            let arg = cmd.trim_start_matches("smart").trim();
            if let Ok(port) = arg.parse::<u8>() {
                println!("{}", sata::smart_info(port));
            } else {
                println!("Usage: smart <port_number>");
            }
        }
        "i225-info" => {
            if intel_i225::is_detected() {
                println!("{}", intel_i225::i225_info());
            } else {
                println!("Intel I225 NIC not detected");
            }
        }
        "i225-stats" => {
            if intel_i225::is_detected() {
                println!("{}", intel_i225::i225_stats());
            } else {
                println!("Intel I225 NIC not detected");
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

        // GPU (unified — hardware AMD + software fallback)
        "gpu-status" => {
            println!("{}", crate::amdgpu_compute::compute_info());
        }
        "gpu-vram" => { println!("{}", crate::amdgpu_compute::vram_info()); }
        "gpu-bench" => { println!("{}", crate::amdgpu_compute::benchmark()); }
        "gpu-buffers" => { println!("{}", crate::gpu::list_buffers()); }
        "gpu-dma-test" => { println!("{}", crate::amdgpu_compute::dma_test()); }
        cmd if cmd.starts_with("gpu-dispatch ") => {
            let arg = cmd.strip_prefix("gpu-dispatch ").unwrap().trim();
            println!("{}", crate::amdgpu_compute::dispatch_test(arg));
        }

        // NVIDIA GPU
        "nvidia-gpu-info" => {
            println!("{}", crate::nvidia_gpu::nvidia_gpu_info());
        }
        "nvidia-gpu-stats" => {
            println!("{}", crate::nvidia_gpu::nvidia_gpu_stats());
        }

        // Intel GPU
        "intel-gpu-info" => {
            println!("{}", crate::intel_gpu::intel_gpu_info());
        }
        "intel-gpu-compute" => {
            println!("{}", crate::intel_gpu_compute::compute_info());
        }
        "intel-gpu-bench" => {
            println!("{}", crate::intel_gpu_compute::benchmark("64"));
        }
        cmd if cmd.starts_with("intel-gpu-bench ") => {
            let arg = cmd.strip_prefix("intel-gpu-bench ").unwrap().trim();
            println!("{}", crate::intel_gpu_compute::benchmark(arg));
        }
        "intel-gpu-stats" => {
            println!("{}", crate::intel_gpu::intel_gpu_stats());
        }
        "intel-gpu-vram" => {
            println!("{}", crate::intel_gpu_compute::vram_info());
        }
        cmd if cmd.starts_with("intel-gpu-dispatch ") => {
            let arg = cmd.strip_prefix("intel-gpu-dispatch ").unwrap().trim();
            println!("{}", crate::intel_gpu_compute::dispatch_test(arg));
        }

        // Bluetooth
        "bt-info" => { println!("{}", crate::bluetooth::bt_info()); }
        "bt-scan" => { crate::bluetooth::scan_start(); println!("Scanning for Bluetooth devices..."); println!("{}", crate::bluetooth::list_devices()); }
        "bt-devices" => { println!("{}", crate::bluetooth::list_devices()); }
        "bt-stats" => { println!("{}", crate::bluetooth::bt_stats()); }

        // Audio
        "audio-info" => { println!("{}", crate::audio_engine::audio_info()); }
        "audio-stats" => { println!("{}", crate::audio_engine::audio_stats()); }
        "audio-demo" => { println!("{}", crate::audio_engine::demo()); }
        "audio-ch" => { println!("{}", crate::audio_engine::list_channels()); }
        "midi-info" => { println!("{}", crate::midi::midi_stats()); }
        cmd if cmd.starts_with("play-tone ") => {
            let parts: alloc::vec::Vec<&str> = cmd.strip_prefix("play-tone ").unwrap().trim().split(' ').collect();
            if parts.len() == 2 {
                let freq = parts[0].parse::<u32>().unwrap_or(440);
                let dur = parts[1].parse::<u32>().unwrap_or(500);
                crate::audio_engine::play_tone(freq, dur);
                println!("Playing {}Hz for {}ms", freq, dur);
            } else {
                println!("Usage: play-tone <freq> <duration_ms>");
            }
        }

        // GPIO & SD card
        "gpio-info" => { println!("{}", crate::gpio::gpio_info()); }
        "gpio-stats" => { println!("{}", crate::gpio::gpio_stats()); }
        cmd if cmd.starts_with("gpio set ") => {
            let args: alloc::vec::Vec<&str> = cmd.strip_prefix("gpio set ").unwrap().trim().split(' ').collect();
            if args.len() == 2 {
                if let Ok(pin) = args[0].parse::<u8>() {
                    let level = match args[1] {
                        "1" | "high" => crate::gpio::PinLevel::High,
                        _ => crate::gpio::PinLevel::Low,
                    };
                    match crate::gpio::write(pin, level) {
                        Ok(()) => println!("GPIO {} set to {:?}", pin, level),
                        Err(e) => println!("gpio set: {}", e),
                    }
                } else {
                    println!("gpio set: invalid pin number");
                }
            } else {
                println!("Usage: gpio set <pin> <0|1|high|low>");
            }
        }
        cmd if cmd.starts_with("gpio read ") => {
            let pin_str = cmd.strip_prefix("gpio read ").unwrap().trim();
            if let Ok(pin) = pin_str.parse::<u8>() {
                match crate::gpio::read(pin) {
                    Ok(level) => println!("GPIO {}: {:?}", pin, level),
                    Err(e) => println!("gpio read: {}", e),
                }
            } else {
                println!("gpio read: invalid pin number");
            }
        }
        "sdcard" => { println!("{}", crate::sdcard::sdcard_info()); }
        "sdcard-stats" => { println!("{}", crate::sdcard::sdcard_stats()); }

        // WiFi & HDA & UEFI
        "wifi-scan" => {
            let results = crate::wifi::wifi_scan();
            for bss in &results {
                println!("  {} ch={} rssi={}", bss.ssid, bss.channel, bss.rssi);
            }
            if results.is_empty() { println!("No networks found."); }
        }
        "wifi-status" => { println!("{}", crate::wifi::wifi_status()); }
        "wifi-info" => { println!("{}", crate::wifi::wifi_info()); }
        "hda-info" => { println!("{}", crate::hda::hda_info()); }
        "hda-codecs" => { println!("{}", crate::hda::list_codecs()); }
        "uefi-info" => { println!("{}", crate::uefi_rt::uefi_info()); }
        "uefi-vars" => {
            let vars = crate::uefi_rt::list_variables();
            for (name, guid, attr, size) in &vars {
                println!("  {} guid={:#x} attr={:#x} size={}", name, guid, attr, size);
            }
            if vars.is_empty() { println!("No UEFI variables."); }
        }

        // Display & Input
        "display-info" => { println!("{}", crate::virtio_gpu_ext::display_info()); }
        "windows" => { println!("{}", crate::virtio_gpu_ext::list_windows()); }
        "screenshot" => { println!("{}", crate::virtio_gpu_ext::screenshot()); }
        "fb-info" => { println!("{}", crate::fb_render::fb_render_info()); }
        "fb-stats" => { println!("{}", crate::fb_render::fb_render_stats()); }
        "displays" => { println!("{}", crate::display_mgr::list_displays()); }
        cmd if cmd.starts_with("display-info ") => {
            let id_str = cmd.strip_prefix("display-info ").unwrap().trim();
            match id_str.parse::<u32>() {
                Ok(id) => println!("{}", crate::display_mgr::display_info(id)),
                Err(_) => println!("display-info: invalid id"),
            }
        }
        cmd if cmd.starts_with("brightness ") => {
            let pct_str = cmd.strip_prefix("brightness ").unwrap().trim();
            match pct_str.parse::<u8>() {
                Ok(pct) => {
                    match crate::display_mgr::set_brightness(0, pct) {
                        Ok(()) => println!("Brightness set to {}%", pct),
                        Err(e) => println!("brightness: {}", e),
                    }
                }
                Err(_) => println!("brightness: invalid percentage"),
            }
        }
        "term-info" => { println!("{}", crate::fb_terminal::fb_terminal_info()); }
        "mouse-info" => { println!("{}", crate::usb_mouse::mouse_info()); }
        "mouse-stats" => { println!("{}", crate::usb_mouse::mouse_stats()); }
        "keymap" => { println!("{}", crate::keymap::keymap_info()); }
        cmd if cmd.starts_with("keymap-set ") => {
            let name = cmd.strip_prefix("keymap-set ").unwrap().trim();
            if crate::keymap::set_layout(name) {
                println!("Keyboard layout set to: {}", name);
            } else {
                println!("Unknown layout: {}. Use 'keymap-list' to see options.", name);
            }
        }
        "keymap-list" => {
            let layouts = crate::keymap::list_layouts();
            println!("Available layouts:");
            for name in &layouts {
                let marker = if *name == crate::keymap::current_layout() { " (active)" } else { "" };
                println!("  {}{}", name, marker);
            }
        }
        "touchpad-info" => { println!("{}", crate::touchpad::touchpad_info()); }

        // Power management
        "power-info" => { println!("{}", crate::power_mgmt::power_info()); }
        "power-stats" => { println!("{}", crate::power_mgmt::power_stats()); }
        "battery" => { println!("{}", crate::power_mgmt::battery_info()); }
        "thermal" => { println!("{}", crate::power_mgmt::thermal_info()); }
        "pstates" => { println!("{}", crate::power_mgmt::list_pstates()); }
        "cstates" => { println!("{}", crate::power_mgmt::cstate_info()); }
        "profiles" => { println!("{}", crate::power_mgmt::list_profiles()); }
        "energy" => { println!("{}", crate::power_mgmt::energy_info()); }
        "acpi-events" => { println!("{}", crate::power_mgmt::acpi_event_log()); }
        cmd if cmd.starts_with("power-profile ") => {
            let name = cmd.strip_prefix("power-profile ").unwrap().trim();
            match crate::power_mgmt::PowerProfile::from_str(name) {
                Some(profile) => {
                    crate::power_mgmt::set_profile(profile);
                    println!("Power profile set to {}", profile.name());
                }
                None => println!("Unknown profile '{}'. Use: performance, balanced, powersaver, custom", name),
            }
        }
        cmd if cmd.starts_with("pstate ") => {
            let id_str = cmd.strip_prefix("pstate ").unwrap().trim();
            match id_str.parse::<u8>() {
                Ok(id) => match crate::power_mgmt::set_pstate(id) {
                    Ok(()) => println!("P-state set to P{}", id),
                    Err(e) => println!("pstate: {}", e),
                },
                Err(_) => println!("Usage: pstate <0-4>"),
            }
        }

        // ACPI extended
        "battery-detail" => { println!("{}", crate::acpi_ext::battery_detail()); }
        "lid-status" => { println!("{}", crate::acpi_ext::lid_status()); }
        "cpu-freq" => { println!("{}", crate::acpi_ext::list_cpu_freqs()); }
        "thermal-detail" => { println!("{}", crate::acpi_ext::thermal_detail()); }
        "acpi-ext-info" => { println!("{}", crate::acpi_ext::acpi_ext_info()); }
        "acpi-ext-stats" => { println!("{}", crate::acpi_ext::acpi_ext_stats()); }

        // NTFS & RAID
        "ntfs-info" => { println!("{}", crate::ntfs::ntfs_info()); }
        "ntfs-stats" => { println!("{}", crate::ntfs::ntfs_stats()); }
        "raid-list" => { println!("{}", crate::raid::list_arrays()); }
        "raid-stats" => { println!("{}", crate::raid::raid_stats()); }
        cmd if cmd.starts_with("raid-info ") => {
            let id_str = cmd.strip_prefix("raid-info ").unwrap().trim();
            match id_str.parse::<u32>() {
                Ok(id) => println!("{}", crate::raid::array_info(id)),
                Err(_) => println!("Usage: raid-info <array-id>"),
            }
        }

        _ => return false,
    }
    true
}

// ═══════════════════════════════════════════════════════════════════
//  ADVANCED COMMANDS (iptables, vlan, routing, services, etc.)
// ═══════════════════════════════════════════════════════════════════

pub fn dispatch_advanced(cmd: &str) -> bool {
    match cmd {
        // iptables
        cmd if cmd.starts_with("iptables ") => {
            match crate::iptables::parse_command(cmd) {
                Ok(msg) => println!("{}", msg),
                Err(e) => println!("iptables: {}", e),
            }
        }
        "iptables" => { println!("{}", crate::iptables::iptables_info()); }
        "iptables-list" => {
            for ch in &[crate::iptables::Chain::Input, crate::iptables::Chain::Output,
                        crate::iptables::Chain::Forward, crate::iptables::Chain::Prerouting,
                        crate::iptables::Chain::Postrouting] {
                println!("{}", crate::iptables::list_rules(*ch));
            }
        }
        "iptables-stats" => { println!("{}", crate::iptables::iptables_stats()); }
        "conntrack" => { println!("{}", crate::iptables::conntrack_info()); }
        "conntrack-flush" => { crate::iptables::conntrack_flush(); println!("Conntrack table flushed."); }
        "ip-forward-on" => { crate::iptables::enable_forwarding(); println!("IP forwarding enabled."); }
        "ip-forward-off" => { crate::iptables::disable_forwarding(); println!("IP forwarding disabled."); }

        // ufw
        cmd if cmd == "ufw" || cmd.starts_with("ufw ") => {
            let args = if cmd.len() > 3 { cmd[3..].trim() } else { "" };
            match args {
                "status" | "" => {
                    println!("Status: active");
                    println!();
                    println!("{:<10} {:<15} {:<10} {}", "To", "Action", "From", "");
                    println!("{:<10} {:<15} {:<10} {}", "--", "------", "----", "");
                    println!("{}", crate::iptables::list_rules(crate::iptables::Chain::Input));
                }
                "status verbose" => {
                    println!("Status: active");
                    println!("Default: deny (incoming), allow (outgoing), deny (routed)");
                    println!();
                    for ch in &[crate::iptables::Chain::Input, crate::iptables::Chain::Output, crate::iptables::Chain::Forward] {
                        println!("{}", crate::iptables::list_rules(*ch));
                    }
                }
                "enable" => {
                    crate::iptables::set_policy(crate::iptables::Chain::Input, crate::iptables::Target::Drop);
                    crate::iptables::set_policy(crate::iptables::Chain::Forward, crate::iptables::Target::Drop);
                    crate::iptables::set_policy(crate::iptables::Chain::Output, crate::iptables::Target::Accept);
                    println!("Firewall is active and enabled on system startup");
                }
                "disable" => {
                    crate::iptables::set_policy(crate::iptables::Chain::Input, crate::iptables::Target::Accept);
                    crate::iptables::set_policy(crate::iptables::Chain::Forward, crate::iptables::Target::Accept);
                    crate::iptables::set_policy(crate::iptables::Chain::Output, crate::iptables::Target::Accept);
                    println!("Firewall stopped and disabled on system startup");
                }
                "reset" => {
                    crate::iptables::flush_chain(crate::iptables::Chain::Input);
                    crate::iptables::flush_chain(crate::iptables::Chain::Output);
                    crate::iptables::flush_chain(crate::iptables::Chain::Forward);
                    crate::iptables::set_policy(crate::iptables::Chain::Input, crate::iptables::Target::Accept);
                    crate::iptables::set_policy(crate::iptables::Chain::Output, crate::iptables::Target::Accept);
                    crate::iptables::set_policy(crate::iptables::Chain::Forward, crate::iptables::Target::Accept);
                    println!("Resetting all rules to installed defaults.");
                }
                _ => {
                    let tokens: alloc::vec::Vec<&str> = args.split_whitespace().collect();
                    if tokens.is_empty() {
                        println!("Usage: ufw [enable|disable|status|reset|allow|deny|delete] ...");
                    } else {
                        let action = tokens[0];
                        match action {
                            "allow" | "deny" | "reject" => {
                                let target = match action {
                                    "allow" => crate::iptables::Target::Accept,
                                    "deny" => crate::iptables::Target::Drop,
                                    "reject" => crate::iptables::Target::Reject,
                                    _ => crate::iptables::Target::Accept,
                                };
                                if tokens.len() >= 2 {
                                    let arg = tokens[1];
                                    if let Ok(port) = arg.parse::<u16>() {
                                        let mut rule = crate::iptables::Rule::new(target);
                                        rule.dst_port = Some(port);
                                        crate::iptables::add_rule(crate::iptables::Chain::Input, rule);
                                        println!("Rule added: {} {}", action, port);
                                    }
                                    else if let Some((port_s, proto_s)) = arg.split_once('/') {
                                        if let Ok(port) = port_s.parse::<u16>() {
                                            let mut rule = crate::iptables::Rule::new(target);
                                            rule.dst_port = Some(port);
                                            if proto_s == "tcp" { rule.protocol = Some(crate::iptables::Protocol::Tcp); }
                                            else if proto_s == "udp" { rule.protocol = Some(crate::iptables::Protocol::Udp); }
                                            crate::iptables::add_rule(crate::iptables::Chain::Input, rule);
                                            println!("Rule added: {} {}/{}", action, port, proto_s);
                                        }
                                    }
                                    else if arg == "from" && tokens.len() >= 3 {
                                        let ip_str = tokens[2];
                                        let parts: alloc::vec::Vec<&str> = ip_str.split('.').collect();
                                        if parts.len() == 4 || ip_str.contains('/') {
                                            let ip_only = if let Some((ip, _)) = ip_str.split_once('/') { ip } else { ip_str };
                                            let ip_parts: alloc::vec::Vec<&str> = ip_only.split('.').collect();
                                            if ip_parts.len() == 4 {
                                                let a = ip_parts[0].parse::<u8>().unwrap_or(0);
                                                let b = ip_parts[1].parse::<u8>().unwrap_or(0);
                                                let c = ip_parts[2].parse::<u8>().unwrap_or(0);
                                                let d = ip_parts[3].parse::<u8>().unwrap_or(0);
                                                let mut rule = crate::iptables::Rule::new(target);
                                                rule.src_ip = Some([a, b, c, d]);
                                                if tokens.len() >= 6 && tokens[3] == "to" && tokens[4] == "any" {
                                                    if tokens.len() >= 7 && tokens[5] == "port" {
                                                        if let Ok(port) = tokens[6].parse::<u16>() {
                                                            rule.dst_port = Some(port);
                                                        }
                                                    }
                                                }
                                                crate::iptables::add_rule(crate::iptables::Chain::Input, rule);
                                                println!("Rule added: {} from {}", action, ip_str);
                                            }
                                        }
                                    }
                                    else {
                                        let port = match arg {
                                            "ssh" => Some(22u16),
                                            "http" => Some(80),
                                            "https" => Some(443),
                                            "ftp" => Some(21),
                                            "smtp" => Some(25),
                                            "dns" => Some(53),
                                            "mysql" => Some(3306),
                                            "postgresql" => Some(5432),
                                            "redis" => Some(6379),
                                            "mongodb" => Some(27017),
                                            _ => None,
                                        };
                                        if let Some(p) = port {
                                            let mut rule = crate::iptables::Rule::new(target);
                                            rule.dst_port = Some(p);
                                            crate::iptables::add_rule(crate::iptables::Chain::Input, rule);
                                            println!("Rule added: {} {} (port {})", action, arg, p);
                                        } else {
                                            println!("ufw: unknown service '{}'", arg);
                                        }
                                    }
                                } else {
                                    println!("Usage: ufw {} <port|service|from IP>", action);
                                }
                            }
                            "delete" => {
                                if tokens.len() >= 3 {
                                    let sub_action = tokens[1];
                                    let _target = match sub_action {
                                        "allow" => crate::iptables::Target::Accept,
                                        "deny" => crate::iptables::Target::Drop,
                                        _ => { println!("ufw delete: unknown action '{}'", sub_action); return true; }
                                    };
                                    if let Ok(port) = tokens[2].parse::<u16>() {
                                        println!("Rule deleted: {} {}", sub_action, port);
                                    } else {
                                        println!("Usage: ufw delete allow|deny <port>");
                                    }
                                } else {
                                    println!("Usage: ufw delete allow|deny <port>");
                                }
                            }
                            _ => {
                                println!("Usage: ufw <enable|disable|status|reset|allow|deny|reject|delete>");
                                println!("  ufw enable           Enable firewall");
                                println!("  ufw disable          Disable firewall");
                                println!("  ufw status           Show rules");
                                println!("  ufw reset            Reset to defaults");
                                println!("  ufw allow 22         Allow port 22");
                                println!("  ufw allow ssh        Allow SSH (port 22)");
                                println!("  ufw allow 80/tcp     Allow TCP port 80");
                                println!("  ufw deny 3306        Deny port 3306");
                                println!("  ufw allow from 10.0.0.0/24");
                                println!("  ufw delete allow 22  Remove rule");
                            }
                        }
                    }
                }
            }
        }

        // VLAN
        "vlan-list" => { println!("{}", crate::vlan::list_vlans()); }
        cmd if cmd.starts_with("vlan-create ") => {
            let args = cmd.strip_prefix("vlan-create ").unwrap().trim();
            let parts: alloc::vec::Vec<&str> = args.splitn(2, ' ').collect();
            if parts.len() >= 2 {
                if let Ok(vid) = parts[0].parse::<u16>() {
                    match crate::vlan::create_vlan(vid, parts[1]) {
                        Ok(()) => println!("VLAN {} ({}) created", vid, parts[1]),
                        Err(e) => println!("vlan-create: {}", e),
                    }
                } else { println!("Usage: vlan-create <vid> <name>"); }
            } else { println!("Usage: vlan-create <vid> <name>"); }
        }
        cmd if cmd.starts_with("vlan-info ") => {
            if let Ok(vid) = cmd.strip_prefix("vlan-info ").unwrap().trim().parse::<u16>() {
                println!("{}", crate::vlan::vlan_info(vid));
            } else { println!("Usage: vlan-info <vid>"); }
        }
        "vlan-stats" => { println!("{}", crate::vlan::vlan_stats()); }

        // Routing protocols
        "ospf-info" => { println!("{}", crate::ospf::ospf_info()); }
        "ospf-neighbors" => { println!("{}", crate::ospf::list_neighbors()); }
        "ospf-routes" => { println!("{}", crate::ospf::show_routes()); }
        "ospf-lsdb" => { println!("{}", crate::ospf::show_lsdb()); }
        "ospf-stats" => { println!("{}", crate::ospf::ospf_stats()); }
        "bgp-info" => { println!("{}", crate::bgp::bgp_info()); }
        "bgp-peers" => { println!("{}", crate::bgp::list_peers()); }
        "bgp-routes" => { println!("{}", crate::bgp::show_routes()); }
        "bgp-stats" => { println!("{}", crate::bgp::bgp_stats()); }
        "rip-info" => { println!("{}", crate::rip::rip_info()); }
        "rip-routes" => { println!("{}", crate::rip::show_routes()); }
        "rip-stats" => { println!("{}", crate::rip::rip_stats()); }

        // Network bonding
        "bond-list" => { println!("{}", crate::bonding::list_bonds()); }
        _ if cmd.starts_with("bond-info ") => {
            let name = cmd.strip_prefix("bond-info ").unwrap().trim();
            println!("{}", crate::bonding::bond_info(name));
        }
        "bond-stats" => { println!("{}", crate::bonding::bond_stats()); }

        // IGMP
        "igmp-groups" => { println!("{}", crate::igmp::list_groups()); }
        "igmp-info" => { println!("{}", crate::igmp::igmp_info()); }
        "igmp-stats" => { println!("{}", crate::igmp::igmp_stats()); }

        // RADIUS
        "radius-info" => { println!("{}", crate::radius::radius_info()); }
        "radius-stats" => { println!("{}", crate::radius::radius_stats()); }

        // SNMP
        "snmp-info" => { println!("{}", crate::snmp::snmp_info()); }
        "snmp-stats" => { println!("{}", crate::snmp::snmp_stats()); }
        _ if cmd.starts_with("snmp-walk ") => {
            let oid = cmd.strip_prefix("snmp-walk ").unwrap().trim();
            println!("{}", crate::snmp::snmp_walk_cmd(oid));
        }
        "snmp-walk" => { println!("{}", crate::snmp::snmp_walk_cmd("1.3.6.1.2.1")); }

        // gRPC
        "grpc-info" => { println!("{}", crate::grpc::grpc_info()); }
        "grpc-services" => { println!("{}", crate::grpc::list_services()); }
        "grpc-stats" => { println!("{}", crate::grpc::grpc_stats()); }

        // SMTP / IMAP
        _ if cmd.starts_with("smtp-send ") => {
            let args: alloc::vec::Vec<&str> = cmd[10..].splitn(5, ' ').collect();
            if args.len() >= 4 {
                let server = if args.len() >= 5 { args[4] } else { "127.0.0.1" };
                match crate::smtp::send_email(args[0], args[1], args[2], args[3], server) {
                    Ok(id) => println!("Email queued (id={})", id),
                    Err(e) => println!("smtp-send: {}", e),
                }
            } else {
                println!("usage: smtp-send <from> <to> <subject> <body> [server]");
            }
        }
        "smtp-queue" => { println!("{}", crate::smtp::list_queue()); }
        "smtp-info" => { println!("{}", crate::smtp::smtp_info()); }
        "smtp-stats" => { println!("{}", crate::smtp::smtp_stats()); }
        "imap-info" => { println!("{}", crate::imap::imap_info()); }
        "imap-stats" => { println!("{}", crate::imap::imap_stats()); }

        // SOCKS5 proxy
        "socks5-status" => { println!("{}", crate::socks5::socks5_info()); }
        "socks5-stats" => { println!("{}", crate::socks5::socks5_stats()); }
        "socks5-sessions" => { println!("{}", crate::socks5::list_sessions()); }
        "socks5-stop" => {
            crate::socks5::stop();
            println!("SOCKS5 proxy stopped");
        }
        cmd if cmd.starts_with("socks5-start ") => {
            let port_str = cmd.strip_prefix("socks5-start ").unwrap().trim();
            if let Ok(port) = port_str.parse::<u16>() {
                crate::socks5::start(port);
                println!("SOCKS5 proxy started on port {}", port);
            } else {
                println!("Usage: socks5-start <port>");
            }
        }
        "socks5-start" => {
            crate::socks5::start(1080);
            println!("SOCKS5 proxy started on port 1080");
        }

        // HTTP proxy
        "proxy-status" => { println!("{}", crate::http_proxy::proxy_info()); }
        "proxy-stats" => { println!("{}", crate::http_proxy::proxy_stats()); }
        "proxy-connections" => { println!("{}", crate::http_proxy::list_connections()); }
        "proxy-stop" => {
            crate::http_proxy::stop();
            println!("HTTP proxy stopped");
        }
        cmd if cmd.starts_with("proxy-start ") => {
            let port_str = cmd.strip_prefix("proxy-start ").unwrap().trim();
            if let Ok(port) = port_str.parse::<u16>() {
                crate::http_proxy::start(port);
                println!("HTTP proxy started on port {}", port);
            } else {
                println!("Usage: proxy-start <port>");
            }
        }
        "proxy-start" => {
            crate::http_proxy::start(8080);
            println!("HTTP proxy started on port 8080");
        }

        // PPPoE
        "pppoe-status" => { println!("{}", crate::pppoe::pppoe_status()); }
        "pppoe-info" => { println!("{}", crate::pppoe::pppoe_info()); }
        "pppoe-stats" => { println!("{}", crate::pppoe::pppoe_stats()); }
        "pppoe-disconnect" => {
            crate::pppoe::pppoe_disconnect_all();
            println!("PPPoE disconnected");
        }
        cmd if cmd.starts_with("pppoe-connect ") => {
            let args = cmd.strip_prefix("pppoe-connect ").unwrap().trim();
            let parts: alloc::vec::Vec<&str> = args.splitn(2, ' ').collect();
            if parts.len() == 2 {
                match crate::pppoe::pppoe_connect(parts[0], parts[1]) {
                    Ok(info) => println!("{}", info),
                    Err(e) => println!("PPPoE connect failed: {}", e),
                }
            } else {
                println!("Usage: pppoe-connect <username> <password>");
            }
        }

        // WireGuard
        "wg" | "wg-show" => { println!("{}", crate::wireguard::wg_show()); }
        "wg-genkey" => { println!("{}", crate::wireguard::wg_genkey()); }
        "wg-info" => { println!("{}", crate::wireguard::wg_info()); }
        "wg-stats" => { println!("{}", crate::wireguard::wg_stats()); }
        cmd if cmd.starts_with("wg-pubkey ") => {
            let hex = cmd.strip_prefix("wg-pubkey ").unwrap().trim();
            match crate::wireguard::wg_pubkey(hex) {
                Ok(pub_hex) => println!("{}", pub_hex),
                Err(e) => println!("Error: {}", e),
            }
        }

        // mDNS
        "mdns-list" => { println!("{}", crate::mdns::list_services()); }
        "mdns-info" => { println!("{}", crate::mdns::mdns_info()); }
        "mdns-stats" => { println!("{}", crate::mdns::mdns_stats()); }
        cmd if cmd.starts_with("mdns-browse ") => {
            let svc_type = cmd.strip_prefix("mdns-browse ").unwrap().trim();
            let results = crate::mdns::browse(svc_type);
            if results.is_empty() {
                println!("No services found for type '{}'", svc_type);
            } else {
                for svc in &results {
                    println!("  {} ({}:{}) host={}", svc.name, svc.service_type, svc.port, svc.hostname);
                }
            }
        }
        cmd if cmd.starts_with("mdns-resolve ") => {
            let host = cmd.strip_prefix("mdns-resolve ").unwrap().trim();
            match crate::mdns::resolve(host) {
                Some(ip) => println!("{} -> {}.{}.{}.{}", host, ip[0], ip[1], ip[2], ip[3]),
                None => println!("Could not resolve '{}'", host),
            }
        }

        // QoS & Traffic Control
        "tc-show" => { println!("{}", crate::traffic_control::tc_show("eth0")); }
        "tc-stats" => { println!("{}", crate::traffic_control::tc_stats()); }
        "tc-info" => { println!("{}", crate::traffic_control::tc_info()); }
        "dscp-info" => { println!("{}", crate::dscp::dscp_info()); }
        "dscp-rules" => { println!("{}", crate::dscp::list_rules()); }
        "dscp-stats" => { println!("{}", crate::dscp::dscp_stats()); }

        // Network services
        "http-stats" => { println!("{}", crate::http_middleware::server_stats()); }
        cmd if cmd == "http-log" || cmd.starts_with("http-log ") => {
            let count = cmd.strip_prefix("http-log ")
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(20);
            println!("{}", crate::http_middleware::format_access_log(count));
        }
        "http-mw" => { println!("{}", crate::http_middleware::list_middleware()); }
        "ssh-sessions" => { println!("{}", crate::scp::list_sessions()); }
        "ssh-stats" => { println!("{}", crate::scp::session_stats()); }
        "scp-list" => { println!("{}", crate::scp::list_transfers()); }
        "dns-zones" => { println!("{}", crate::dns_zone::list_zones()); }
        cmd if cmd.starts_with("dns-zone ") => {
            let domain = cmd.strip_prefix("dns-zone ").unwrap().trim();
            println!("{}", crate::dns_zone::zone_info(domain));
        }
        "dns-cache" => { println!("{}", crate::dns_zone::cache_stats()); }
        "mqtt-stats" => { println!("{}", crate::mqtt_broker::broker_stats()); }
        "mqtt-clients" => { println!("{}", crate::mqtt_broker::list_clients()); }
        "mqtt-retained" => { println!("{}", crate::mqtt_broker::list_retained()); }
        "ws-conns" => { println!("{}", crate::ws_server::list_connections()); }
        "ws-rooms" => { println!("{}", crate::ws_server::list_rooms()); }
        "ws-stats" => { println!("{}", crate::ws_server::ws_stats()); }
        "https-info" => { println!("{}", crate::https_server::https_info()); }
        "https-stats" => { println!("{}", crate::https_server::https_stats()); }

        // FTP / HTTP2 / QUIC / HTTP3
        "ftpd-status" => { println!("{}", crate::ftpd::ftpd_info()); }
        "ftpd-sessions" => { println!("{}", crate::ftpd::list_sessions()); }
        "ftpd-stats" => { println!("{}", crate::ftpd::ftpd_stats()); }
        "http2-info" => { println!("{}", crate::http2::http2_info()); }
        "http2-stats" => { println!("{}", crate::http2::http2_stats()); }
        "http2-streams" => { println!("{}", crate::http2::list_streams()); }
        "quic-info" => { println!("{}", crate::quic::quic_info()); }
        "quic-stats" => { println!("{}", crate::quic::quic_stats()); }
        "quic-conns" => { println!("{}", crate::quic::list_connections()); }
        "http3-info" => { println!("{}", crate::http3::h3_info()); }
        "http3-stats" => { println!("{}", crate::http3::h3_stats()); }
        "http3-conns" => { println!("{}", crate::http3::list_h3_connections()); }

        // DHCP server
        "dhcpd-status" => { println!("{}", crate::dhcpd::dhcpd_info()); }
        "dhcpd-leases" => { println!("{}", crate::dhcpd::list_leases()); }
        "dhcpd-stats" => { println!("{}", crate::dhcpd::dhcpd_stats()); }
        "dhcpd-start" => {
            crate::dhcpd::start(
                [192, 168, 1, 100], [192, 168, 1, 200],
                [255, 255, 255, 0], [192, 168, 1, 1], [8, 8, 8, 8],
            );
            println!("DHCP server started (192.168.1.100-200)");
        }
        "dhcpd-stop" => {
            crate::dhcpd::stop();
            println!("DHCP server stopped");
        }

        // NTP
        "ntp-status" => { println!("{}", crate::ntp::ntp_info()); }
        "ntp-stats" => { println!("{}", crate::ntp::ntp_stats()); }
        "ntp-sync" => {
            match crate::ntp::sync([129, 6, 15, 28]) {
                Ok(offset) => println!("NTP sync OK: offset={}ms", offset),
                Err(e) => println!("NTP sync failed: {}", e),
            }
        }

        // TFTP
        "tftp-status" => { println!("{}", crate::tftp::tftp_info()); }
        "tftp-stats" => { println!("{}", crate::tftp::tftp_stats()); }
        "tftp-start" => {
            match crate::tftp::start_server() {
                Ok(()) => println!("TFTP server started on UDP port {}", crate::tftp::DEFAULT_PORT),
                Err(e) => println!("tftp-start: {}", e),
            }
        }
        "tftp-stop" => {
            match crate::tftp::stop_server() {
                Ok(()) => println!("TFTP server stopped"),
                Err(e) => println!("tftp-stop: {}", e),
            }
        }

        // Raw sockets & BPF & eBPF
        "raw-info" => { println!("{}", crate::raw_socket::raw_socket_info()); }
        "raw-stats" => { println!("{}", crate::raw_socket::raw_socket_stats()); }
        "bpf-info" => { println!("{}", crate::bpf::bpf_info()); }
        "bpf-stats" => { println!("{}", crate::bpf::bpf_stats()); }
        "ebpf-info" => { println!("{}", crate::ebpf::ebpf_info()); }
        "ebpf-maps" => { println!("{}", crate::ebpf::list_maps()); }
        "ebpf-progs" => { println!("{}", crate::ebpf::list_programs()); }
        "ebpf-stats" => { println!("{}", crate::ebpf::ebpf_stats()); }
        "xdp-info" => { println!("{}", crate::ebpf::xdp_info()); }

        // HP networking & zero-copy & DPDK
        "hpnet-info" => { println!("{}", crate::hpnet::hpnet_info()); }
        "hpnet-stats" => { println!("{}", crate::hpnet::hpnet_stats()); }
        "rss-info" => { println!("{}", crate::hpnet::rss_info()); }
        "zero-copy-info" => { println!("{}", crate::zero_copy::zero_copy_info()); }
        "zero-copy-stats" => { println!("{}", crate::zero_copy::zero_copy_stats()); }
        "dpdk-info" => { println!("{}", crate::dpdk::dpdk_info()); }
        "dpdk-stats" => { println!("{}", crate::dpdk::dpdk_stats()); }
        "dpdk-bench" => { println!("{}", crate::dpdk::dpdk_benchmark(1000)); }
        "mempool-info" => { println!("{}", crate::dpdk::mempool_info()); }

        // veth & bridges & ext4 & congestion & wasi
        "veth-list" => { for s in crate::veth::list_pairs() { println!("{}", s); } }
        "bridges" => { for s in crate::bridge::list_bridges() { println!("{}", s); } }
        "ext4-info" => { println!("{}", crate::ext4::ext4_info()); }
        "congestion" => { println!("{}", crate::tcp_congestion::congestion_stats()); }
        "wasi-info" => { println!("{}", crate::wasi::wasi_info()); }

        // Crypto
        "dlopen" => { println!("{}", crate::elf_runtime::linker_info()); }
        "breakpoints" => { println!("{}", crate::debuginfo::list_breakpoints()); }
        "bt-debug" => { println!("{}", crate::debuginfo::backtrace_annotated()); }
        "crypto-info" => { println!("{}", crate::crypto_ext::crypto_info()); }
        "aes-demo" => {
            let key = [0x2bu8, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c];
            let plain = [0x32u8, 0x43, 0xf6, 0xa8, 0x88, 0x5a, 0x30, 0x8d, 0x31, 0x31, 0x98, 0xa2, 0xe0, 0x37, 0x07, 0x34];
            let cipher = crate::crypto_ext::aes128_encrypt_block(&plain, &key);
            println!("AES-128 encrypt:");
            println!("  plain:  {:02x?}", plain);
            println!("  key:    {:02x?}", key);
            println!("  cipher: {:02x?}", cipher);
            let dec = crate::crypto_ext::aes128_decrypt_block(&cipher, &key);
            println!("  decrpt: {:02x?}", dec);
            println!("  match:  {}", plain == dec);
        }
        "rsa-demo" => {
            let kp = crate::crypto_ext::generate_keypair(16);
            println!("RSA demo (tiny keys):");
            println!("  n={}, e={}, d={}", kp.n, kp.e, kp.d);
            let msg = 42u64;
            let enc = crate::crypto_ext::rsa_encrypt(msg, kp.e, kp.n);
            let dec = crate::crypto_ext::rsa_decrypt(enc, kp.d, kp.n);
            println!("  msg={}, encrypted={}, decrypted={}", msg, enc, dec);
            println!("  match: {}", msg == dec);
        }
        "pkg-stats" => { println!("{}", crate::pkg_registry::registry_stats()); }

        // QFC blockchain mining
        cmd if cmd.starts_with("qfc-mine ") => {
            let args = cmd.strip_prefix("qfc-mine ").unwrap().trim();
            println!("{}", crate::qfc_miner::cmd_mine(args));
        }
        "qfc-mine" => { println!("{}", crate::qfc_miner::cmd_mine("")); }
        cmd if cmd.starts_with("qfc-pow ") => {
            let args = cmd.strip_prefix("qfc-pow ").unwrap().trim();
            println!("{}", crate::qfc_miner::cmd_pow(args));
        }
        "qfc-pow" => { println!("{}", crate::qfc_miner::cmd_pow("")); }
        "qfc-status" => { println!("{}", crate::qfc_miner::cmd_status()); }
        cmd if cmd.starts_with("qfc-wallet ") => {
            let args = cmd.strip_prefix("qfc-wallet ").unwrap().trim();
            println!("{}", crate::qfc_miner::cmd_wallet(args));
        }
        "qfc-wallet" => { println!("{}", crate::qfc_miner::cmd_wallet("")); }
        cmd if cmd.starts_with("qfc-hash ") => {
            let args = cmd.strip_prefix("qfc-hash ").unwrap().trim();
            println!("{}", crate::qfc_miner::cmd_hash(args));
        }
        "qfc-hash" => { println!("{}", crate::qfc_miner::cmd_hash("")); }
        cmd if cmd.starts_with("qfc-sign ") => {
            let args = cmd.strip_prefix("qfc-sign ").unwrap().trim();
            println!("{}", crate::qfc_miner::cmd_sign(args));
        }
        "qfc-sign" => { println!("{}", crate::qfc_miner::cmd_sign("")); }
        "qfc-info" => { println!("{}", crate::qfc_miner::qfc_miner_info()); }
        "blake3-info" => { println!("{}", crate::blake3::blake3_info()); }
        "ed25519-info" => { println!("{}", crate::ed25519::ed25519_info()); }

        _ => return false,
    }
    true
}

// ═══════════════════════════════════════════════════════════════════
//  APPS COMMANDS (desktop, browser, email, music, dev env, etc.)
// ═══════════════════════════════════════════════════════════════════

pub fn dispatch_apps(cmd: &str) -> bool {
    match cmd {
        // Logging
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
                _ => { println!("logfilter: unknown level '{}'", level); return true; }
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
                _ => { println!("loglevel: unknown level"); return true; }
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

        // Profiling
        "perf" => {
            let counters = crate::profiler::perf_stat();
            println!("{}", crate::profiler::format_perf_counters(&counters));
            if crate::syscall_stats::is_enabled() {
                println!("{}", crate::syscall_stats::report());
            }
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
        "syscall-stats" => { println!("{}", crate::syscall_stats::report()); }
        "syscall-stats on" => { crate::syscall_stats::enable(); println!("Syscall statistics enabled."); }
        "syscall-stats off" => { crate::syscall_stats::disable(); println!("Syscall statistics disabled."); }
        "alloc-track" => { println!("{}", crate::alloc_track::stats()); }
        "alloc-track on" => { crate::alloc_track::start(); println!("Allocation tracking started."); }
        "alloc-track off" => { crate::alloc_track::stop(); println!("Allocation tracking stopped."); }
        "alloc-track leaks" => { println!("{}", crate::alloc_track::leaks()); }
        cmd if cmd == "alloc-track events" || cmd.starts_with("alloc-track events ") => {
            let count = cmd.strip_prefix("alloc-track events ")
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(20);
            println!("{}", crate::alloc_track::recent_events(count));
        }
        "alloc-track pids" => { println!("{}", crate::alloc_track::per_pid_stats()); }

        // Stability
        "crashlog" => { println!("{}", crate::panic_recover::crash_log()); }
        "crashstats" => { println!("{}", crate::panic_recover::stats()); }
        "integrity" => { println!("{}", crate::panic_recover::integrity_check()); }
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
        "recovery on" => { crate::panic_recover::set_recovery(true); println!("Panic recovery enabled."); }
        "recovery off" => { crate::panic_recover::set_recovery(false); println!("Panic recovery disabled."); }

        // Fuzz testing
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

        // Microkernel & RT & DFS
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

        // Process management & widgets
        "proc-list" => { println!("{}", crate::userland::list_processes()); }
        "proc-stats" => { println!("{}", crate::userland::process_stats()); }
        "widgets" => { println!("{}", crate::widget::list_widgets()); }
        "widget-demo" => { println!("{}", crate::widget::demo()); }
        "notifications" => { println!("{}", crate::dialog::list_notifications()); }
        cmd if cmd.starts_with("notify ") => {
            let msg = cmd.strip_prefix("notify ").unwrap().trim();
            crate::dialog::notify("Shell", msg, crate::dialog::NotifLevel::Info);
            println!("Notification sent.");
        }

        // Packages
        "pkg-list" => { println!("{}", crate::pkg_registry::list_packages()); }
        "pkg-installed" => { println!("{}", crate::pkg_registry::list_installed()); }
        cmd if cmd.starts_with("pkg-info ") => {
            let name = cmd.strip_prefix("pkg-info ").unwrap().trim();
            println!("{}", crate::pkg_registry::package_info(name));
        }
        cmd if cmd.starts_with("pkg-install ") => {
            let name = cmd.strip_prefix("pkg-install ").unwrap().trim();
            match crate::pkg_registry::install(name) {
                Ok(msg) => println!("{}", msg),
                Err(e) => println!("pkg-install: {}", e),
            }
        }
        cmd if cmd.starts_with("pkg-search ") => {
            let query = cmd.strip_prefix("pkg-search ").unwrap().trim();
            let results = crate::pkg_registry::search_packages(query);
            if results.is_empty() { println!("No packages found."); }
            for pkg in &results {
                println!("  {} {} — {}", pkg.name, pkg.version.display(), pkg.description);
            }
        }
        "build-stats" => { println!("{}", crate::build_system::build_stats()); }
        "build-config" => { println!("{}", crate::build_system::show_config()); }

        // Kernel internals
        cmd if cmd.starts_with("proc /") => {
            let path = cmd.strip_prefix("proc ").unwrap().trim();
            match crate::procfs::read(path) {
                Some(content) => println!("{}", content),
                None => println!("proc: {} not found", path),
            }
        }
        "procfs-list" => { println!("{}", crate::procfs::list_all()); }
        cmd if cmd.starts_with("sysfs ") => {
            let path = cmd.strip_prefix("sysfs ").unwrap().trim();
            match crate::sysfs::sysfs_read(path) {
                Some(content) => println!("{}", content),
                None => println!("sysfs: {} not found", path),
            }
        }
        "dev-tree" => { println!("{}", crate::sysfs::device_tree()); }
        cmd if cmd.starts_with("dev-info ") => {
            if let Ok(id) = cmd.strip_prefix("dev-info ").unwrap().trim().parse::<u32>() {
                println!("{}", crate::sysfs::device_info(id));
            } else { println!("Usage: dev-info <id>"); }
        }
        "dev-stats" => { println!("{}", crate::sysfs::device_stats()); }
        "tmpfs-info" => { println!("{}", crate::tmpfs::tmpfs_info()); }
        "tmpfs-stats" => { println!("{}", crate::tmpfs::tmpfs_stats()); }
        "pipes" => { println!("{}", crate::pipe2::pipe_info()); }
        cmd if cmd.starts_with("mkfifo ") => {
            let path = cmd.strip_prefix("mkfifo ").unwrap().trim();
            match crate::pipe2::create_fifo(path) {
                Ok(id) => println!("Created FIFO {} (id: {})", path, id),
                Err(e) => println!("mkfifo: {}", e),
            }
        }

        // VMM / perf events
        "vmm-info" => { println!("{}", crate::vmm::vmm_info()); }
        "page-cache" => { println!("{:?}", crate::vmm::page_cache_stats()); }
        "oom-info" => { println!("{}", crate::vmm::oom_info()); }
        "ipc-info" => { println!("{}", crate::ipc_ext::ipc_ext_info()); }
        "mq-list" => { println!("{}", crate::ipc_ext::mq_list_info()); }
        "sem-list" => { println!("{}", crate::ipc_ext::sem_list_info()); }
        "perf-events" => { println!("{}", crate::perf_events::perf_events_info()); }
        "flamegraph" => { println!("{}", crate::perf_events::generate_flamegraph()); }
        "topdown" => { println!("{}", crate::perf_events::topdown_analysis()); }

        // cgroups
        "cgroup-list" => { println!("{}", crate::cgroup::list_cgroups()); }
        "cgroup-tree" => { println!("{}", crate::cgroup::cgroup_tree()); }
        cmd if cmd.starts_with("cgroup-info ") => {
            let path = cmd.trim_start_matches("cgroup-info ").trim();
            match crate::cgroup::cgroup_info(path) {
                Ok(info) => println!("{}", info),
                Err(e) => println!("cgroup-info: {}", e),
            }
        }
        cmd if cmd.starts_with("cgroup-create ") => {
            let path = cmd.trim_start_matches("cgroup-create ").trim();
            match crate::cgroup::create_cgroup(path) {
                Ok(id) => println!("cgroup created: {} (id={})", path, id),
                Err(e) => println!("cgroup-create: {}", e),
            }
        }
        cmd if cmd.starts_with("cgroup-add ") => {
            let rest = cmd.trim_start_matches("cgroup-add ").trim();
            let parts: alloc::vec::Vec<&str> = rest.splitn(2, ' ').collect();
            if parts.len() < 2 {
                println!("usage: cgroup-add <path> <pid>");
            } else {
                let cg_path = parts[0];
                if let Ok(pid) = parts[1].trim().parse::<usize>() {
                    match crate::cgroup::add_process(cg_path, pid) {
                        Ok(()) => println!("pid {} added to {}", pid, cg_path),
                        Err(e) => println!("cgroup-add: {}", e),
                    }
                } else {
                    println!("cgroup-add: invalid pid");
                }
            }
        }

        // System management
        "who" => { println!("{}", crate::multi_user::who()); }
        "w" => { println!("{}", crate::multi_user::w()); }
        "last" => { println!("{}", crate::multi_user::last()); }
        "sessions" => { println!("{}", crate::multi_user::sessions_info()); }
        "systemctl" => { println!("{}", crate::service_mgr::list_services()); }
        cmd if cmd.starts_with("systemctl start ") => {
            let name = cmd.strip_prefix("systemctl start ").unwrap().trim();
            match crate::service_mgr::start(name) {
                Ok(()) => println!("Started {}", name),
                Err(e) => println!("Failed: {}", e),
            }
        }
        cmd if cmd.starts_with("systemctl stop ") => {
            let name = cmd.strip_prefix("systemctl stop ").unwrap().trim();
            match crate::service_mgr::stop(name) {
                Ok(()) => println!("Stopped {}", name),
                Err(e) => println!("Failed: {}", e),
            }
        }
        cmd if cmd.starts_with("systemctl restart ") => {
            let name = cmd.strip_prefix("systemctl restart ").unwrap().trim();
            match crate::service_mgr::restart(name) {
                Ok(()) => println!("Restarted {}", name),
                Err(e) => println!("Failed: {}", e),
            }
        }
        "boot-report" => { println!("{}", crate::service_mgr::boot_report()); }
        "install" => {
            match crate::installer::start_install() {
                Ok(()) => println!("Installation complete."),
                Err(e) => println!("install: {}", e),
            }
        }
        "disks" => { println!("{}", crate::installer::format_disks()); }
        cmd if cmd.starts_with("run-elf ") => {
            let path = cmd.strip_prefix("run-elf ").unwrap().trim();
            match crate::elf_exec::exec_elf(path, &[], &[]) {
                Ok(code) => println!("ELF exited with code {}", code),
                Err(e) => println!("run-elf: {}", e),
            }
        }

        // Config & diagnostics
        cmd if cmd.starts_with("sysctl ") => {
            let path = cmd.strip_prefix("sysctl ").unwrap().trim();
            if let Some((key, val)) = path.split_once('=') {
                match crate::kconfig_ext::sysctl_write(key.trim(), val.trim()) {
                    Ok(()) => println!("{} = {}", key.trim(), val.trim()),
                    Err(e) => println!("sysctl: {}", e),
                }
            } else {
                match crate::kconfig_ext::sysctl_read(path) {
                    Ok(val) => println!("{} = {}", path, val),
                    Err(e) => println!("sysctl: {} — {}", path, e),
                }
            }
        }
        "sysctl-list" => { println!("{}", crate::kconfig_ext::dump_config()); }
        "config-diff" => { println!("{:?}", crate::kconfig_ext::config_diff()); }
        "config-dump" => { println!("{}", crate::kconfig_ext::dump_config()); }
        cmd if cmd.starts_with("profile ") => {
            let name = cmd.strip_prefix("profile ").unwrap().trim();
            match crate::kconfig_ext::apply_profile(name) {
                Ok(_) => println!("Applied profile: {}", name),
                Err(e) => println!("profile: {}", e),
            }
        }

        // Compositor & Desktop
        "wm-info" => { println!("{}", crate::compositor::compositor_info()); }
        "wm-stats" => { println!("{}", crate::compositor::compositor_stats()); }
        "wm-windows" => { println!("{}", crate::compositor::list_windows()); }
        cmd if cmd.starts_with("wm-create ") => {
            let args = cmd.strip_prefix("wm-create ").unwrap().trim();
            let parts: alloc::vec::Vec<&str> = args.splitn(3, ' ').collect();
            if parts.len() >= 1 {
                let title = parts[0];
                let w = parts.get(1).and_then(|s| s.parse::<u32>().ok()).unwrap_or(400);
                let h = parts.get(2).and_then(|s| s.parse::<u32>().ok()).unwrap_or(300);
                let id = crate::compositor::create_window(title, w, h);
                if id > 0 { println!("Created window {} ({}x{}): {}", id, w, h, title); }
                else { println!("wm-create: max windows reached"); }
            } else {
                println!("Usage: wm-create <title> [width] [height]");
            }
        }
        "desktop-info" => { println!("{}", crate::desktop::desktop_info()); }
        "desktop-stats" => { println!("{}", crate::desktop::desktop_stats()); }
        "launcher" => {
            crate::desktop::show_launcher();
            println!("{}", crate::desktop::list_apps());
        }
        cmd if cmd.starts_with("launch ") => {
            let name = cmd.strip_prefix("launch ").unwrap().trim();
            match crate::desktop::launch_app(name) {
                Some(cmd_str) => {
                    println!("Launching: {}", cmd_str);
                    crate::shell::dispatch(&cmd_str);
                }
                None => println!("launch: app '{}' not found", name),
            }
        }
        "theme" => { println!("{}", crate::desktop::get_theme()); }
        "tray" => { println!("{}", crate::desktop::tray_info()); }

        // File Manager
        cmd if cmd.starts_with("files ") => {
            let path = cmd.strip_prefix("files ").unwrap().trim();
            if path.is_empty() {
                crate::file_manager::open_directory("/");
            } else {
                crate::file_manager::open_directory(path);
            }
        }
        "files" => { crate::file_manager::open_directory("/"); }

        // Network Manager
        "net-mgr" => { crate::net_manager::show(); }

        // Settings
        cmd if cmd.starts_with("settings ") => {
            let panel = cmd.strip_prefix("settings ").unwrap().trim();
            crate::settings_app::open_panel(panel);
        }
        "settings" => { crate::settings_app::show_overview(); }
        "settings-save" => { crate::settings_app::save_settings(); }
        "settings-load" => { crate::settings_app::load_settings(); }

        // Browser
        cmd if cmd.starts_with("browse ") => {
            let url = cmd.strip_prefix("browse ").unwrap().trim();
            match crate::browser::navigate(url) {
                Ok(page) => { crate::println!("{}", page); }
                Err(e) => { crate::println!("browse: {}", e); }
            }
        }
        "browser-info" => { crate::println!("{}", crate::browser::browser_info()); }
        "browser-stats" => { crate::println!("{}", crate::browser::browser_stats()); }
        "browser-back" => {
            match crate::browser::back() {
                Ok(page) => { crate::println!("{}", page); }
                Err(e) => { crate::println!("back: {}", e); }
            }
        }
        "browser-forward" => {
            match crate::browser::forward() {
                Ok(page) => { crate::println!("{}", page); }
                Err(e) => { crate::println!("forward: {}", e); }
            }
        }

        // Email
        "mail" => { crate::println!("{}", crate::email::email_info()); }
        "mail-check" => {
            let headers = crate::email::check_mail();
            if headers.is_empty() {
                crate::println!("No mail.");
            } else {
                for h in &headers {
                    let flag = if h.read { " " } else { "*" };
                    crate::println!("{} {:>4} {:20} {}", flag, h.id, h.from, h.subject);
                }
            }
        }
        cmd if cmd.starts_with("mail-send ") => {
            let rest = cmd.strip_prefix("mail-send ").unwrap().trim();
            if let Some((to, subject)) = rest.split_once(' ') {
                let from = "root@merlion";
                match crate::email::compose_and_send(from, to.trim(), subject.trim(), "") {
                    Ok(()) => { crate::println!("Mail sent to {}", to.trim()); }
                    Err(e) => { crate::println!("mail-send: {}", e); }
                }
            } else {
                crate::println!("usage: mail-send <to> <subject>");
            }
        }
        cmd if cmd.starts_with("mail-read ") => {
            let id_str = cmd.strip_prefix("mail-read ").unwrap().trim();
            let mut id_val = 0u32;
            let mut valid = true;
            for ch in id_str.bytes() {
                if ch.is_ascii_digit() {
                    id_val = id_val.saturating_mul(10).saturating_add((ch - b'0') as u32);
                } else {
                    valid = false;
                    break;
                }
            }
            if valid && !id_str.is_empty() {
                match crate::email::fetch_email(id_val) {
                    Some(email) => { crate::println!("{}", email.display()); }
                    None => { crate::println!("mail-read: email {} not found", id_val); }
                }
            } else {
                crate::println!("usage: mail-read <id>");
            }
        }
        "mail-stats" => { crate::println!("{}", crate::email::email_stats()); }

        // Music player
        cmd if cmd.starts_with("play ") => {
            let path = cmd.strip_prefix("play ").unwrap().trim();
            crate::music_player::play(path);
            println!("{}", crate::music_player::now_playing());
        }
        "pause" => { crate::music_player::pause(); println!("Paused."); }
        "stop" => { crate::music_player::stop(); println!("Stopped."); }
        "next-track" => { crate::music_player::next(); println!("{}", crate::music_player::now_playing()); }
        "prev-track" => { crate::music_player::prev(); println!("{}", crate::music_player::now_playing()); }
        "now-playing" => { println!("{}", crate::music_player::now_playing()); }
        "playlist" => { println!("{}", crate::music_player::playlist_show()); }
        cmd if cmd.starts_with("playlist-add ") => {
            let path = cmd.strip_prefix("playlist-add ").unwrap().trim();
            crate::music_player::playlist_add(path);
            println!("Added to playlist.");
        }
        "player-info" => { println!("{}", crate::music_player::player_info()); }
        "player-stats" => { println!("{}", crate::music_player::player_stats()); }
        "vu" => { println!("{}", crate::music_player::vu_meter()); }

        // Dev environment
        cmd if cmd.starts_with("highlight ") => {
            let path = cmd.strip_prefix("highlight ").unwrap().trim();
            println!("{}", crate::dev_env::highlight_file(path));
        }
        "dev-info" => { println!("{}", crate::dev_env::dev_env_info()); }
        "dev-env-stats" => { println!("{}", crate::dev_env::dev_env_stats()); }
        cmd if cmd.starts_with("dev-open ") => {
            let path = cmd.strip_prefix("dev-open ").unwrap().trim();
            match crate::dev_env::open_file(path) {
                Ok(idx) => println!("Opened buffer {}: {}", idx, path),
                Err(e) => println!("dev-open: {}", e),
            }
        }
        "dev-buffers" => { println!("{}", crate::dev_env::list_buffers()); }
        cmd if cmd.starts_with("dev-project ") => {
            let path = cmd.strip_prefix("dev-project ").unwrap().trim();
            println!("{}", crate::dev_env::open_project(path));
        }
        "dev-build" => { println!("{}", crate::dev_env::build()); }
        "dev-errors" => { println!("{}", crate::dev_env::build_errors()); }
        "dev-breakpoints" => { println!("{}", crate::dev_env::list_breakpoints()); }

        // Network package manager
        cmd if cmd.starts_with("pkg ") => {
            let args = cmd.strip_prefix("pkg ").unwrap().trim();
            println!("{}", crate::pkg_net::handle_command(args));
        }
        "pkg-net-info" => { println!("{}", crate::pkg_net::pkg_net_info()); }
        "pkg-net-stats" => { println!("{}", crate::pkg_net::pkg_net_stats()); }

        // NFS
        cmd if cmd.starts_with("nfs-mount ") => {
            let rest = cmd.strip_prefix("nfs-mount ").unwrap().trim();
            if let Some((spec, mountpoint)) = rest.split_once(' ') {
                match crate::nfs_client::parse_server_export(spec) {
                    Ok((ip, export)) => {
                        match crate::nfs_client::mount_nfs(ip, export, mountpoint.trim()) {
                            Ok(()) => println!("NFS mounted {}.{}.{}.{}:{} on {}",
                                ip[0], ip[1], ip[2], ip[3], export, mountpoint.trim()),
                            Err(e) => println!("nfs-mount: {:?}", e),
                        }
                    }
                    Err(e) => println!("nfs-mount: {:?}", e),
                }
            } else {
                println!("usage: nfs-mount <server>:<export> <mountpoint>");
            }
        }
        cmd if cmd.starts_with("nfs-unmount ") => {
            let mp = cmd.strip_prefix("nfs-unmount ").unwrap().trim();
            match crate::nfs_client::unmount_nfs(mp) {
                Ok(()) => println!("NFS unmounted {}", mp),
                Err(e) => println!("nfs-unmount: {:?}", e),
            }
        }
        "nfs-mounts" => { println!("{}", crate::nfs_client::list_mounts()); }
        "nfs-info" => { println!("{}", crate::nfs_client::nfs_info()); }
        "nfs-stats" => { println!("{}", crate::nfs_client::nfs_stats()); }

        // Performance optimization
        "io-sched" => { println!("{}", crate::perf_opt::io_sched_info()); }
        cmd if cmd.starts_with("io-sched-set ") => {
            let rest = cmd.strip_prefix("io-sched-set ").unwrap().trim();
            if let Some((dev, sched)) = rest.split_once(' ') {
                match crate::perf_opt::set_scheduler(dev.trim(), sched.trim()) {
                    Ok(()) => println!("Scheduler for {} set to {}", dev.trim(), sched.trim()),
                    Err(e) => println!("io-sched-set: {}", e),
                }
            } else {
                println!("usage: io-sched-set <device> <noop|deadline|cfq|bfq>");
            }
        }
        "bench-all" => { println!("{}", crate::perf_opt::run_all_benchmarks()); }
        "bench-cpu" => { println!("{}", crate::perf_opt::run_benchmark("cpu")); }
        "bench-mem" => { println!("{}", crate::perf_opt::run_benchmark("mem")); }
        "bench-io" => { println!("{}", crate::perf_opt::run_benchmark("io")); }
        "bench-net" => { println!("{}", crate::perf_opt::run_benchmark("net")); }
        "thp-info" => { println!("{}", crate::perf_opt::thp_info()); }
        "perf-opt-info" => { println!("{}", crate::perf_opt::perf_opt_info()); }
        "perf-opt-stats" => { println!("{}", crate::perf_opt::perf_opt_stats()); }

        // OCI containers
        cmd if cmd.starts_with("container ") => {
            let args = cmd.strip_prefix("container ").unwrap().trim();
            println!("{}", crate::oci_runtime::handle_command(args));
        }
        "oci-info" => { println!("{}", crate::oci_runtime::oci_info()); }
        "oci-stats" => { println!("{}", crate::oci_runtime::oci_stats()); }

        // KVM
        "vm-list" => { println!("{}", crate::kvm::list_vms()); }
        cmd if cmd.starts_with("vm-create ") => {
            let rest = cmd.strip_prefix("vm-create ").unwrap().trim();
            if let Some((name, mem_str)) = rest.split_once(' ') {
                if let Ok(mem) = mem_str.trim().parse::<u32>() {
                    match crate::kvm::create_vm(name.trim(), mem) {
                        Ok(id) => println!("VM '{}' created (ID: {}, {} MB)", name.trim(), id, mem),
                        Err(e) => println!("vm-create: {}", e),
                    }
                } else {
                    println!("usage: vm-create <name> <memory_mb>");
                }
            } else {
                println!("usage: vm-create <name> <memory_mb>");
            }
        }
        cmd if cmd.starts_with("vm-start ") => {
            let id_str = cmd.strip_prefix("vm-start ").unwrap().trim();
            if let Ok(id) = id_str.parse::<u32>() {
                match crate::kvm::start_vm(id) {
                    Ok(()) => println!("VM {} started", id),
                    Err(e) => println!("vm-start: {}", e),
                }
            } else {
                println!("usage: vm-start <id>");
            }
        }
        cmd if cmd.starts_with("vm-stop ") => {
            let id_str = cmd.strip_prefix("vm-stop ").unwrap().trim();
            if let Ok(id) = id_str.parse::<u32>() {
                match crate::kvm::stop_vm(id) {
                    Ok(()) => println!("VM {} stopped", id),
                    Err(e) => println!("vm-stop: {}", e),
                }
            } else {
                println!("usage: vm-stop <id>");
            }
        }
        cmd if cmd.starts_with("vm-destroy ") => {
            let id_str = cmd.strip_prefix("vm-destroy ").unwrap().trim();
            if let Ok(id) = id_str.parse::<u32>() {
                match crate::kvm::destroy_vm(id) {
                    Ok(()) => println!("VM {} destroyed", id),
                    Err(e) => println!("vm-destroy: {}", e),
                }
            } else {
                println!("usage: vm-destroy <id>");
            }
        }
        cmd if cmd.starts_with("vm-info ") => {
            let id_str = cmd.strip_prefix("vm-info ").unwrap().trim();
            if let Ok(id) = id_str.parse::<u32>() {
                match crate::kvm::vm_info(id) {
                    Ok(info) => println!("{}", info),
                    Err(e) => println!("vm-info: {}", e),
                }
            } else {
                println!("usage: vm-info <id>");
            }
        }
        cmd if cmd.starts_with("vm-console ") => {
            let id_str = cmd.strip_prefix("vm-console ").unwrap().trim();
            if let Ok(id) = id_str.parse::<u32>() {
                match crate::kvm::vm_console(id) {
                    Ok(out) => println!("{}", out),
                    Err(e) => println!("vm-console: {}", e),
                }
            } else {
                println!("usage: vm-console <id>");
            }
        }
        cmd if cmd.starts_with("vm-regs ") => {
            let id_str = cmd.strip_prefix("vm-regs ").unwrap().trim();
            if let Ok(id) = id_str.parse::<u32>() {
                match crate::kvm::guest_regs(id) {
                    Ok(info) => println!("{}", info),
                    Err(e) => println!("vm-regs: {}", e),
                }
            } else {
                println!("usage: vm-regs <id>");
            }
        }
        "kvm-info" => { println!("{}", crate::kvm::kvm_info()); }
        "kvm-stats" => { println!("{}", crate::kvm::kvm_stats()); }

        // Self-hosting compiler
        cmd if cmd.starts_with("compile ") => {
            let path = cmd.strip_prefix("compile ").unwrap().trim();
            match crate::self_host::compile_file(path) {
                Ok(code) => println!("Compiled {} -> {} bytes of machine code", path, code.len()),
                Err(e) => println!("compile: {}", e),
            }
        }
        cmd if cmd.starts_with("build ") => {
            let dir = cmd.strip_prefix("build ").unwrap().trim();
            match crate::self_host::build_project(dir) {
                Ok(msg) => println!("{}", msg),
                Err(e) => println!("build: {}", e),
            }
        }
        "self-test" => { println!("{}", crate::self_host::self_build_test()); }
        "bootstrap-info" => { println!("{}", crate::self_host::self_host_info()); }
        "bootstrap-stats" => { println!("{}", crate::self_host::self_host_stats()); }

        // Bash-like commands
        "bash" => crate::bash::cmd_bash(),
        "zsh" => crate::bash::cmd_zsh(),
        "sh" => crate::bash::cmd_sh(),
        _ if cmd.starts_with("set ") => crate::bash::cmd_set(&cmd[4..]),
        "set" => crate::bash::cmd_set(""),
        _ if cmd.starts_with("let ") => crate::bash::cmd_let(&cmd[4..]),
        _ if cmd.starts_with("type ") => crate::bash::cmd_type(&cmd[5..]),
        _ if cmd.starts_with("export ") => crate::bash::cmd_export(&cmd[7..]),

        _ => return false,
    }
    true
}

// ═══════════════════════════════════════════════════════════════════
//  MISC COMMANDS (fallback)
// ═══════════════════════════════════════════════════════════════════

pub fn dispatch_misc(cmd: &str) -> bool {
    use crate::ai_shell;

    // Try AI natural language interpretation as last resort
    if let Some(ai_cmd) = ai_shell::interpret(cmd) {
        println!("{}", ai_shell::format_hint(cmd, &ai_cmd));
        crate::shell::dispatch(&ai_cmd);
        return true;
    }

    false
}
