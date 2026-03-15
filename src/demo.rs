/// Automated demo — showcases all MerlionOS capabilities.
/// Run via the `demo` shell command to walk through every subsystem.

use crate::{print, println, serial_println, task, timer};

/// Run the full system demo.
pub fn run() {
    section("MerlionOS Full System Demo");
    println!("This demo showcases the key capabilities of MerlionOS.");
    println!("Born for AI. Built by AI.\n");
    pause(50);

    // 1. System info
    section("1. System Information");
    crate::shell::dispatch("neofetch");
    pause(100);

    // 2. Hardware detection
    section("2. Hardware Detection");
    crate::shell::dispatch("cpuinfo");
    println!();
    crate::shell::dispatch("lspci");
    println!();
    crate::shell::dispatch("blkdevs");
    pause(80);

    // 3. Memory
    section("3. Memory Management");
    crate::shell::dispatch("free");
    println!();
    crate::shell::dispatch("slabinfo");
    pause(80);

    // 4. Virtual Filesystem
    section("4. Virtual Filesystem");
    println!("\x1b[90m$ ls /\x1b[0m");
    crate::shell::dispatch("ls /");
    println!();
    println!("\x1b[90m$ cat /proc/version\x1b[0m");
    crate::shell::dispatch("cat /proc/version");
    println!("\x1b[90m$ cat /proc/cpuinfo\x1b[0m");
    crate::shell::dispatch("cat /proc/cpuinfo");
    println!("\x1b[90m$ write /tmp/demo hello from MerlionOS\x1b[0m");
    crate::shell::dispatch("write /tmp/demo hello from MerlionOS");
    println!("\x1b[90m$ cat /tmp/demo\x1b[0m");
    crate::shell::dispatch("cat /tmp/demo");
    pause(80);

    // 5. Multitasking
    section("5. Preemptive Multitasking");
    println!("Spawning a background task...");
    crate::shell::dispatch("spawn");
    // Let the demo task run a couple ticks
    let wait = timer::ticks() + 30;
    while timer::ticks() < wait { x86_64::instructions::hlt(); }
    crate::shell::dispatch("ps");
    pause(80);

    // 6. Kernel modules
    section("6. Loadable Kernel Modules");
    crate::shell::dispatch("lsmod");
    println!();
    println!("\x1b[90m$ modprobe hello\x1b[0m");
    crate::shell::dispatch("modprobe hello");
    println!("\x1b[90m$ rmmod hello\x1b[0m");
    crate::shell::dispatch("rmmod hello");
    pause(80);

    // 7. AI features
    section("7. AI Native OS");
    println!("\x1b[90m$ ai 你好\x1b[0m");
    crate::shell::dispatch("ai 你好");
    println!();
    println!("\x1b[90m$ explain syscall\x1b[0m");
    crate::shell::dispatch("explain syscall");
    println!();
    println!("\x1b[90m$ monitor\x1b[0m");
    crate::shell::dispatch("monitor");
    println!();
    println!("\x1b[90m$ agents\x1b[0m");
    crate::shell::dispatch("agents");
    pause(80);

    // 8. Networking
    section("8. Networking");
    crate::shell::dispatch("ifconfig");
    println!();
    println!("\x1b[90m$ ping localhost\x1b[0m");
    crate::shell::dispatch("ping localhost");
    pause(80);

    // 9. Diagnostics
    section("9. System Diagnostics");
    crate::shell::dispatch("stackcheck");
    crate::shell::dispatch("heapcheck");
    crate::shell::dispatch("heal");
    pause(80);

    // 10. Self-tests
    section("10. Kernel Self-Tests");
    crate::shell::dispatch("test");
    pause(50);

    // Finale
    println!();
    println!("\x1b[36m══════════════════════════════════════════════════\x1b[0m");
    println!("\x1b[1m  Demo complete!\x1b[0m");
    println!();
    println!("  \x1b[36m57\x1b[0m source modules");
    println!("  \x1b[36m~9600\x1b[0m lines of Rust");
    println!("  \x1b[36m90+\x1b[0m shell commands");
    println!("  \x1b[36m77+\x1b[0m development phases");
    println!();
    println!("  Real virtio-blk disk I/O");
    println!("  Real virtio-net with ARP/ICMP");
    println!("  ELF binary loading from disk");
    println!("  AI native: NL shell, agents, self-healing");
    println!();
    println!("  \x1b[1mBorn for AI. Built by AI.\x1b[0m");
    println!("\x1b[36m══════════════════════════════════════════════════\x1b[0m");
}

fn section(title: &str) {
    println!();
    println!("\x1b[33m── {} ──\x1b[0m", title);
    println!();
}

fn pause(ticks: u64) {
    let target = timer::ticks() + ticks;
    while timer::ticks() < target {
        x86_64::instructions::hlt();
    }
}
