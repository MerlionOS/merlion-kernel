/// Comprehensive /proc filesystem for MerlionOS.
/// Provides runtime kernel information through virtual files,
/// similar to Linux's procfs. Each file is generated on-demand.
///
/// System-wide entries live under `/proc/...` and per-process entries
/// under `/proc/[pid]/...`.  A registration system allows other kernel
/// subsystems to publish additional proc files at runtime.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use alloc::borrow::ToOwned;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// Total number of procfs reads since boot.
static READ_COUNT: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Registration system
// ---------------------------------------------------------------------------

/// Generator function that produces the contents of a proc file.
type ProcGenerator = fn() -> String;

/// A registered /proc entry.
struct ProcEntry {
    path: String,
    generator: ProcGenerator,
    description: &'static str,
}

/// All registered proc entries.
static ENTRIES: Mutex<Vec<ProcEntry>> = Mutex::new(Vec::new());

/// Register a new proc entry.
///
/// `path` is relative to `/proc` (e.g. `"stat"` for `/proc/stat`).
/// `generator` is called each time the file is read.
/// `description` is a short human-readable summary shown by `list_all`.
pub fn register(path: &str, generator: ProcGenerator, description: &'static str) {
    let mut entries = ENTRIES.lock();
    // Avoid duplicates.
    for e in entries.iter() {
        if e.path == path {
            return;
        }
    }
    entries.push(ProcEntry {
        path: path.to_owned(),
        generator,
        description,
    });
}

/// Read a proc file by path (relative to `/proc`).
///
/// Returns `None` if no entry is registered for that path.
/// For per-process entries (`<pid>/status`, etc.) the path is
/// matched with the generic `self/...` handler when the pid is
/// the current task.
pub fn read(path: &str) -> Option<String> {
    READ_COUNT.fetch_add(1, Ordering::Relaxed);

    // Try exact match first.
    let entries = ENTRIES.lock();
    for e in entries.iter() {
        if e.path == path {
            let gen = e.generator;
            drop(entries);
            return Some(gen());
        }
    }
    drop(entries);

    // Per-process: if path looks like "<pid>/<file>", try to serve it.
    if let Some((pid_str, rest)) = path.split_once('/') {
        if let Ok(pid) = pid_str.parse::<usize>() {
            return read_process_entry(pid, rest);
        }
    }

    None
}

/// List all registered entry paths that start with `prefix`.
pub fn list(prefix: &str) -> Vec<String> {
    let entries = ENTRIES.lock();
    entries
        .iter()
        .filter(|e| e.path.starts_with(prefix))
        .map(|e| e.path.clone())
        .collect()
}

/// Human-readable listing of every registered proc entry.
pub fn list_all() -> String {
    let entries = ENTRIES.lock();
    let mut out = String::with_capacity(entries.len() * 60);
    out.push_str(&format!("{:<30} {}\n", "PATH", "DESCRIPTION"));
    for e in entries.iter() {
        out.push_str(&format!("{:<30} {}\n", e.path, e.description));
    }
    out
}

// ---------------------------------------------------------------------------
// Per-process entries  (/proc/[pid]/...)
// ---------------------------------------------------------------------------

/// Serve a per-process file.  `pid` is the target process and `file` is
/// one of: status, cmdline, environ, cwd, fd, maps, stat, io.
fn read_process_entry(pid: usize, file: &str) -> Option<String> {
    // Make sure the pid actually exists.
    let tasks = crate::task::list();
    let task = tasks.iter().find(|t| t.pid == pid)?;

    match file {
        "status" => Some(gen_proc_pid_status(task)),
        "cmdline" => Some(gen_proc_pid_cmdline(task)),
        "environ" => Some(gen_proc_pid_environ()),
        "cwd" => Some(gen_proc_pid_cwd()),
        "fd" => Some(gen_proc_pid_fd()),
        "maps" => Some(gen_proc_pid_maps(task)),
        "stat" => Some(gen_proc_pid_stat(task)),
        "io" => Some(gen_proc_pid_io()),
        _ => None,
    }
}

fn state_char(state: crate::task::TaskState) -> char {
    match state {
        crate::task::TaskState::Running => 'R',
        crate::task::TaskState::Ready => 'S',
        crate::task::TaskState::Finished => 'Z',
    }
}

fn gen_proc_pid_status(task: &crate::task::TaskInfo) -> String {
    let heap = crate::allocator::stats();
    format!(
        "Name:\t{}\n\
         State:\t{} ({})\n\
         Pid:\t{}\n\
         Uid:\t0\n\
         Gid:\t0\n\
         VmSize:\t{} kB\n\
         VmRSS:\t{} kB\n\
         Threads:\t1\n",
        task.name,
        state_char(task.state),
        match task.state {
            crate::task::TaskState::Running => "running",
            crate::task::TaskState::Ready => "sleeping",
            crate::task::TaskState::Finished => "zombie",
        },
        task.pid,
        heap.total / 1024,
        heap.used / 1024,
    )
}

fn gen_proc_pid_cmdline(task: &crate::task::TaskInfo) -> String {
    // Kernel tasks don't have a real cmdline; use name.
    format!("{}\0", task.name)
}

fn gen_proc_pid_environ() -> String {
    // Expose kernel environment variables.
    let mut out = String::new();
    if let Some(home) = crate::env::get("HOME") {
        out.push_str(&format!("HOME={}\0", home));
    }
    if let Some(path) = crate::env::get("PATH") {
        out.push_str(&format!("PATH={}\0", path));
    }
    if let Some(user) = crate::env::get("USER") {
        out.push_str(&format!("USER={}\0", user));
    }
    if out.is_empty() {
        out.push_str("(none)\n");
    }
    out
}

fn gen_proc_pid_cwd() -> String {
    crate::env::get("PWD").unwrap_or_else(|| "/".to_owned())
}

fn gen_proc_pid_fd() -> String {
    // List open file descriptors from the global FD table.
    let mut out = String::new();
    out.push_str("FD  TYPE      PATH\n");
    // fd 0-2 are always stdin/stdout/stderr by convention.
    out.push_str("0   chr       /dev/console\n");
    out.push_str("1   chr       /dev/console\n");
    out.push_str("2   chr       /dev/console\n");
    out
}

fn gen_proc_pid_maps(task: &crate::task::TaskInfo) -> String {
    let heap_start = crate::allocator::HEAP_START;
    let heap_end = heap_start + crate::allocator::HEAP_SIZE;
    let _ = task; // pid-specific maps would need per-process VM tracking
    format!(
        "{:016x}-{:016x} rw-p 00000000 00:00 0          [heap]\n\
         ffffffff80000000-ffffffff80100000 r-xp 00000000 00:00 0          [kernel]\n",
        heap_start, heap_end,
    )
}

fn gen_proc_pid_stat(task: &crate::task::TaskInfo) -> String {
    let ticks = crate::timer::ticks();
    // Linux one-line format (simplified): pid (comm) state ppid pgrp ...
    format!(
        "{} ({}) {} 0 0 0 0 0 0 0 0 0 {} 0 0 0 0 0 1 0 {} 0 0\n",
        task.pid,
        task.name,
        state_char(task.state),
        ticks,
        ticks,
    )
}

fn gen_proc_pid_io() -> String {
    let ns = crate::net::NET.lock();
    format!(
        "rchar: {}\n\
         wchar: {}\n\
         syscr: 0\n\
         syscw: 0\n\
         read_bytes: {}\n\
         write_bytes: {}\n",
        ns.rx_bytes,
        ns.tx_bytes,
        ns.rx_bytes,
        ns.tx_bytes,
    )
}

// ---------------------------------------------------------------------------
// System-wide generators
// ---------------------------------------------------------------------------

fn gen_stat() -> String {
    let ticks = crate::timer::ticks();
    let perf = crate::profiler::perf_stat();
    // Approximate user/system/idle splits using integer math.
    let total = if ticks == 0 { 1 } else { ticks };
    let system = perf.interrupt_count + perf.syscall_count;
    let user = perf.context_switch_count;
    let idle = total.saturating_sub(system).saturating_sub(user);
    let mut out = format!(
        "cpu  {} 0 {} 0 {} 0 0 0 0 0\n",
        user, system, idle,
    );
    // Per-CPU line (single CPU system).
    out.push_str(&format!(
        "cpu0 {} 0 {} 0 {} 0 0 0 0 0\n",
        user, system, idle,
    ));
    out.push_str(&format!("intr {}", perf.interrupt_count));
    // Pad with zeros for first 16 IRQ lines.
    for _ in 0..16 {
        out.push_str(" 0");
    }
    out.push('\n');
    out.push_str(&format!("ctxt {}\n", perf.context_switch_count));
    out.push_str(&format!("btime {}\n", 0u64)); // no RTC epoch yet
    out.push_str(&format!("processes {}\n", crate::task::list().len()));
    out.push_str(&format!("procs_running {}\n",
        crate::task::list().iter()
            .filter(|t| matches!(t.state, crate::task::TaskState::Running))
            .count()
    ));
    out.push_str(&format!("procs_blocked 0\n"));
    out
}

fn gen_loadavg() -> String {
    // Approximate load average using integer math (tasks-in-run-queue).
    let tasks = crate::task::list();
    let running = tasks.iter()
        .filter(|t| !matches!(t.state, crate::task::TaskState::Finished))
        .count();
    let total = tasks.len();
    // Express as fixed-point X.XX using integer hundredths.
    let load_100 = running as u64 * 100;
    let int_part = load_100 / 100;
    let frac_part = load_100 % 100;
    // 1/5/15 min are all the same (no exponential decay tracking yet).
    format!(
        "{}.{:02} {}.{:02} {}.{:02} {}/{} {}\n",
        int_part, frac_part,
        int_part, frac_part,
        int_part, frac_part,
        running, total,
        crate::task::current_pid(),
    )
}

fn gen_meminfo() -> String {
    let mem = crate::memory::stats();
    let heap = crate::allocator::stats();
    let total_kb = mem.total_usable_bytes / 1024;
    let alloc_kb = mem.allocated_frames * 4; // 4 KiB per frame
    let free_kb = total_kb.saturating_sub(alloc_kb);
    // Buffers and cached are approximations.
    let buffers_kb = heap.used as u64 / 1024;
    let cached_kb = alloc_kb / 4;
    let slab_total: u64 = crate::slab::stats().iter()
        .map(|s| (s.in_use * s.obj_size) as u64)
        .sum::<u64>() / 1024;

    format!(
        "MemTotal:       {:>8} kB\n\
         MemFree:        {:>8} kB\n\
         MemAvailable:   {:>8} kB\n\
         Buffers:        {:>8} kB\n\
         Cached:         {:>8} kB\n\
         SwapCached:     {:>8} kB\n\
         SwapTotal:      {:>8} kB\n\
         SwapFree:       {:>8} kB\n\
         Slab:           {:>8} kB\n\
         SReclaimable:   {:>8} kB\n\
         SUnreclaim:     {:>8} kB\n\
         KernelStack:    {:>8} kB\n\
         PageTables:     {:>8} kB\n\
         HeapTotal:      {:>8} kB\n\
         HeapUsed:       {:>8} kB\n\
         HeapFree:       {:>8} kB\n",
        total_kb,
        free_kb,
        free_kb + cached_kb,
        buffers_kb,
        cached_kb,
        0u64,
        0u64,
        0u64,
        slab_total,
        slab_total / 2,
        slab_total.saturating_sub(slab_total / 2),
        crate::task::list().len() as u64 * 16, // 16 KiB stack per task
        alloc_kb / 8,
        heap.total as u64 / 1024,
        heap.used as u64 / 1024,
        heap.free as u64 / 1024,
    )
}

fn gen_vmstat() -> String {
    let perf = crate::profiler::perf_stat();
    let mem = crate::memory::stats();
    format!(
        "nr_free_pages {}\n\
         nr_alloc_pages {}\n\
         pgfault {}\n\
         pgalloc_normal {}\n\
         pgfree {}\n\
         pswpin 0\n\
         pswpout 0\n\
         pgsteal_direct 0\n\
         pgsteal_kswapd 0\n",
        (mem.total_usable_bytes / 4096).saturating_sub(mem.allocated_frames),
        mem.allocated_frames,
        perf.page_fault_count,
        mem.allocated_frames,
        0u64,
    )
}

fn gen_interrupts() -> String {
    let perf = crate::profiler::perf_stat();
    let ticks = crate::timer::ticks();
    let mut out = String::with_capacity(512);
    out.push_str("           CPU0\n");
    out.push_str(&format!("  0: {:>10}   PIT     timer\n", ticks));
    out.push_str(&format!("  1: {:>10}   i8042   keyboard\n",
        perf.interrupt_count.saturating_sub(ticks)));
    for irq in 2..16u32 {
        out.push_str(&format!(" {:>2}: {:>10}   none\n", irq, 0));
    }
    out.push_str(&format!("TOT: {:>10}\n", perf.interrupt_count));
    out
}

fn gen_softirqs() -> String {
    let perf = crate::profiler::perf_stat();
    format!(
        "                    CPU0\n\
         TIMER:       {:>10}\n\
         NET_TX:      {:>10}\n\
         NET_RX:      {:>10}\n\
         BLOCK:       {:>10}\n\
         TASKLET:     {:>10}\n\
         SCHED:       {:>10}\n\
         RCU:         {:>10}\n",
        perf.interrupt_count,
        crate::net::NET.lock().tx_packets,
        crate::net::NET.lock().rx_packets,
        0u64,
        0u64,
        perf.context_switch_count,
        0u64,
    )
}

fn gen_filesystems() -> String {
    let mut out = String::new();
    out.push_str("nodev\tprocfs\n");
    out.push_str("nodev\ttmpfs\n");
    out.push_str("nodev\tdevtmpfs\n");
    out.push_str("nodev\tsysfs\n");
    out.push_str("\text2\n");
    out.push_str("\text4\n");
    out.push_str("\tvfat\n");
    out.push_str("nodev\tramfs\n");
    out
}

fn gen_mounts() -> String {
    format!(
        "none / ramfs rw 0 0\n\
         none /proc procfs rw 0 0\n\
         none /dev devtmpfs rw 0 0\n\
         none /tmp tmpfs rw 0 0\n"
    )
}

fn gen_net_dev() -> String {
    let ns = crate::net::NET.lock();
    let mut out = String::with_capacity(256);
    out.push_str("Inter-|   Receive                                                |  Transmit\n");
    out.push_str(" face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n");
    out.push_str(&format!(
        "  eth0: {:>8} {:>7} {:>4} {:>4} {:>4} {:>5} {:>10} {:>9} {:>8} {:>7} {:>4} {:>4} {:>4} {:>5} {:>7} {:>10}\n",
        ns.rx_bytes, ns.rx_packets, 0, 0, 0, 0, 0, 0,
        ns.tx_bytes, ns.tx_packets, 0, 0, 0, 0, 0, 0,
    ));
    out.push_str(&format!(
        "    lo: {:>8} {:>7} {:>4} {:>4} {:>4} {:>5} {:>10} {:>9} {:>8} {:>7} {:>4} {:>4} {:>4} {:>5} {:>7} {:>10}\n",
        0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0,
    ));
    out
}

fn gen_net_tcp() -> String {
    let mut out = String::new();
    out.push_str("  sl  local_address rem_address   st tx_queue rx_queue\n");
    // No active TCP connections tracked yet; emit header only.
    out
}

fn gen_net_udp() -> String {
    let mut out = String::new();
    out.push_str("  sl  local_address rem_address   st tx_queue rx_queue\n");
    out
}

fn gen_net_arp() -> String {
    let mut out = String::new();
    out.push_str("IP address       HW type     Flags       HW address            Mask     Device\n");
    for (ip, mac) in crate::netproto::arp_list() {
        out.push_str(&format!(
            "{:<16} 0x1         0x2         {}   *        eth0\n",
            ip, mac,
        ));
    }
    // Always include the gateway.
    let ns = crate::net::NET.lock();
    out.push_str(&format!(
        "{:<16} 0x1         0x2         ff:ff:ff:ff:ff:ff   *        eth0\n",
        ns.gateway,
    ));
    out
}

fn gen_diskstats() -> String {
    let mut out = String::new();
    for dev in crate::blkdev::list() {
        // major minor name reads rd_merged rd_sectors rd_ms writes wr_merged wr_sectors wr_ms io_cur io_ms wr_io_ms
        out.push_str(&format!(
            "   1    0 {} 0 0 0 0 0 0 0 0 0 0 0\n",
            dev.name,
        ));
    }
    out
}

fn gen_partitions() -> String {
    let mut out = String::new();
    out.push_str("major minor  #blocks  name\n\n");
    for dev in crate::blkdev::list() {
        out.push_str(&format!(
            "   1     0    {:>7}  {}\n",
            dev.size_kb, dev.name,
        ));
    }
    out
}

fn gen_crypto() -> String {
    format!(
        "name         : sha256\n\
         driver       : sha256-generic\n\
         module       : kernel\n\
         priority     : 100\n\
         type         : shash\n\
         blocksize    : 64\n\
         digestsize   : 32\n\
         \n\
         name         : hmac(sha256)\n\
         driver       : hmac-sha256-generic\n\
         module       : kernel\n\
         priority     : 100\n\
         type         : shash\n\
         blocksize    : 64\n\
         digestsize   : 32\n\
         \n\
         name         : xor\n\
         driver       : xor-generic\n\
         module       : kernel\n\
         priority     : 50\n\
         type         : cipher\n\
         blocksize    : 1\n\
         digestsize   : 0\n"
    )
}

fn gen_cmdline() -> String {
    // Kernel command line (mirrors QEMU arguments).
    format!("console=ttyS0 earlyprintk=serial root=/dev/ram0 rw\n")
}

fn gen_config() -> String {
    let mut out = String::new();
    out.push_str("#\n# MerlionOS Kernel Configuration\n#\n");
    out.push_str("CONFIG_X86_64=y\n");
    out.push_str("CONFIG_SMP=y\n");
    out.push_str("CONFIG_PREEMPT=y\n");
    out.push_str("CONFIG_NO_STD=y\n");
    out.push_str("CONFIG_HEAP_SIZE=65536\n");
    out.push_str(&format!("CONFIG_MAX_TASKS={}\n", 8));
    out.push_str("CONFIG_PIT_HZ=100\n");
    out.push_str("CONFIG_NET=y\n");
    out.push_str("CONFIG_VIRTIO_BLK=y\n");
    out.push_str("CONFIG_VIRTIO_NET=y\n");
    out.push_str("CONFIG_E1000E=y\n");
    out.push_str("CONFIG_EXT2=y\n");
    out.push_str("CONFIG_EXT4=y\n");
    out.push_str("CONFIG_FAT=y\n");
    out.push_str("CONFIG_PROCFS=y\n");
    out.push_str("CONFIG_VFS=y\n");
    out.push_str("CONFIG_SLAB=y\n");
    out.push_str("CONFIG_MODULES=y\n");
    out.push_str("CONFIG_ACPI=y\n");
    out.push_str("CONFIG_FRAMEBUFFER=y\n");
    out.push_str("CONFIG_CRYPTO_SHA256=y\n");
    out
}

fn gen_kallsyms() -> String {
    // Simplified: dump what the ksyms module has registered.
    let mut out = String::new();
    // We cannot iterate SYMBOLS directly (private), so register key
    // known addresses.  In a real kernel this comes from the linker.
    out.push_str(&format!("{:016x} T kernel_main\n",
        crate::ksyms::lookup(0xFFFFFFFF80000000)
            .map(|(_, off)| 0xFFFFFFFF80000000u64.wrapping_sub(off))
            .unwrap_or(0xFFFFFFFF80000000)));
    out.push_str(&format!("{:016x} T timer_tick\n",
        crate::timer::tick as *const () as u64));
    out.push_str(&format!("{:016x} T task_yield\n",
        crate::task::yield_now as *const () as u64));
    out.push_str(&format!("{:016x} T task_spawn\n",
        crate::task::spawn as *const () as u64));
    out
}

fn gen_locks() -> String {
    let mut out = String::new();
    out.push_str("POSIX  ADVISORY  WRITE 0 00:00:0 0 EOF\n");
    // No real file locks tracked yet.
    out
}

fn gen_timer_list() -> String {
    let ticks = crate::timer::ticks();
    let uptime = crate::timer::uptime_secs();
    format!(
        "Timer List Version: v0.1\n\
         HRTIMER_MAX_CLOCK_BASES: 1\n\
         now at {} ticks ({} seconds)\n\
         \n\
         cpu: 0\n\
          clock 0:\n\
           .base: PIT\n\
           .resolution: {} ns\n\
           active timers:\n\
            <none>\n",
        ticks,
        uptime,
        1_000_000_000u64 / crate::timer::PIT_FREQUENCY_HZ,
    )
}

fn gen_buddyinfo() -> String {
    let mem = crate::memory::stats();
    let free_pages = (mem.total_usable_bytes / 4096).saturating_sub(mem.allocated_frames);
    // Simulate buddy allocator order distribution.
    // Spread free pages across orders 0-10 (mostly small).
    let order0 = free_pages / 2;
    let order1 = free_pages / 4;
    let order2 = free_pages / 8;
    let order3 = free_pages / 16;
    let higher = free_pages.saturating_sub(order0 + order1 + order2 + order3);
    format!(
        "Node 0, zone   Normal {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6}\n",
        order0, order1, order2, order3, higher, 0, 0, 0, 0, 0, 0,
    )
}

fn gen_slabinfo() -> String {
    let mut out = String::new();
    out.push_str("slabinfo - version: 2.1\n");
    out.push_str("# name            <active_objs> <num_objs> <objsize> <objperslab> <pagesperslab>\n");
    for s in crate::slab::stats() {
        out.push_str(&format!(
            "{:<18} {:>5} {:>5} {:>7} {:>8} {:>5}\n",
            s.name, s.in_use, s.capacity, s.obj_size,
            s.capacity, 1,
        ));
    }
    out
}

fn gen_zoneinfo() -> String {
    let mem = crate::memory::stats();
    let total_pages = mem.total_usable_bytes / 4096;
    let free_pages = total_pages.saturating_sub(mem.allocated_frames);
    format!(
        "Node 0, zone   Normal\n\
         pages free     {}\n\
         min      16\n\
         low      32\n\
         high     48\n\
         spanned  {}\n\
         present  {}\n\
         managed  {}\n",
        free_pages,
        total_pages,
        total_pages,
        total_pages,
    )
}

fn gen_version() -> String {
    format!(
        "{} version {} ({}) #1 SMP {}\n",
        crate::version::NAME,
        crate::version::VERSION,
        crate::version::ARCH,
        crate::version::CODENAME,
    )
}

fn gen_uptime() -> String {
    let secs = crate::timer::uptime_secs();
    let ticks = crate::timer::ticks();
    let centisecs = (ticks % crate::timer::PIT_FREQUENCY_HZ) * 100
        / crate::timer::PIT_FREQUENCY_HZ;
    // Linux format: uptime_secs.cc idle_secs.cc
    format!("{}.{:02} {}.{:02}\n", secs, centisecs, secs, centisecs)
}

fn gen_cpuinfo() -> String {
    let features = crate::smp::detect_features();
    let mut out = String::with_capacity(512);
    out.push_str(&format!("processor\t: 0\n"));
    out.push_str(&format!("model name\t: {}\n", features.brand));
    out.push_str(&format!("cpu family\t: {}\n", features.family));
    out.push_str(&format!("model\t\t: {}\n", features.model));
    out.push_str(&format!("stepping\t: {}\n", features.stepping));
    out.push_str(&format!("cpu cores\t: {}\n", features.logical_cores));
    out.push_str(&format!("apic id\t\t: {}\n", crate::smp::apic_id()));
    let mut flags = String::new();
    if features.has_sse { flags.push_str(" sse"); }
    if features.has_sse2 { flags.push_str(" sse2"); }
    if features.has_avx { flags.push_str(" avx"); }
    if features.has_apic { flags.push_str(" apic"); }
    if features.has_x2apic { flags.push_str(" x2apic"); }
    out.push_str(&format!("flags\t\t:{}\n", flags));
    out.push_str(&format!("bogomips\t: {}\n",
        crate::timer::PIT_FREQUENCY_HZ * 2));
    out.push_str("\n");
    out
}

fn gen_modules() -> String {
    let mut out = String::new();
    for m in crate::module::list() {
        let state = match m.state {
            crate::module::ModuleState::Loaded => "Live",
            crate::module::ModuleState::Unloaded => "Unloaded",
        };
        out.push_str(&format!("{} 0 - {} 0x0\n", m.name, state));
    }
    out
}

fn gen_self_status() -> String {
    let pid = crate::task::current_pid();
    let tasks = crate::task::list();
    if let Some(task) = tasks.iter().find(|t| t.pid == pid) {
        gen_proc_pid_status(task)
    } else {
        format!("pid: {}\n", pid)
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Register all system-wide /proc entries.  Call once at boot after the
/// timer, allocator, task, net, and profiler subsystems are initialised.
pub fn init() {
    // System-wide entries.
    register("stat",          gen_stat,         "CPU and kernel statistics");
    register("loadavg",       gen_loadavg,      "Load average (1/5/15 min)");
    register("meminfo",       gen_meminfo,      "Detailed memory information");
    register("vmstat",        gen_vmstat,       "Virtual memory statistics");
    register("interrupts",    gen_interrupts,   "IRQ counts per interrupt line");
    register("softirqs",      gen_softirqs,     "Software interrupt counts");
    register("filesystems",   gen_filesystems,  "Registered filesystem types");
    register("mounts",        gen_mounts,       "Mounted filesystems");
    register("net/dev",       gen_net_dev,      "Network interface statistics");
    register("net/tcp",       gen_net_tcp,      "TCP connection table");
    register("net/udp",       gen_net_udp,      "UDP endpoint table");
    register("net/arp",       gen_net_arp,      "ARP cache");
    register("diskstats",     gen_diskstats,    "Block device I/O statistics");
    register("partitions",    gen_partitions,   "Disk partitions");
    register("crypto",        gen_crypto,       "Available crypto algorithms");
    register("cmdline",       gen_cmdline,      "Kernel command line");
    register("config",        gen_config,       "Kernel configuration");
    register("kallsyms",      gen_kallsyms,     "Kernel symbol table");
    register("locks",         gen_locks,        "Active file locks");
    register("timer_list",    gen_timer_list,   "Active kernel timers");
    register("buddyinfo",     gen_buddyinfo,    "Memory allocator buddy info");
    register("slabinfo",      gen_slabinfo,     "Slab allocator statistics");
    register("zoneinfo",      gen_zoneinfo,     "Memory zone information");
    register("version",       gen_version,      "Kernel version string");
    register("uptime",        gen_uptime,       "System uptime in seconds");
    register("cpuinfo",       gen_cpuinfo,      "CPU information");
    register("modules",       gen_modules,      "Loaded kernel modules");
    register("self/status",   gen_self_status,  "Current process status");

    crate::serial_println!("[procfs] registered {} entries", list("").len());
}

// ---------------------------------------------------------------------------
// Module info
// ---------------------------------------------------------------------------

/// Return a summary of procfs state: number of entries and total reads.
pub fn procfs_info() -> String {
    let count = {
        let entries = ENTRIES.lock();
        entries.len()
    };
    let reads = READ_COUNT.load(Ordering::Relaxed);
    format!(
        "procfs: {} registered entries, {} total reads\n",
        count, reads,
    )
}
