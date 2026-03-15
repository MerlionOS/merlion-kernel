/// Comprehensive system information — one-shot system report.
/// Combines all subsystem info into a single formatted output.

use alloc::string::String;
use alloc::format;
use crate::{version, smp, timer, rtc, allocator, memory, task, driver, module, net, e1000e, virtio_blk, ahci, nvme, xhci};

/// Generate a complete system information report.
pub fn full_report() -> String {
    let mut r = String::new();
    let features = smp::detect_features();
    let mem = memory::stats();
    let heap = allocator::stats();
    let (h, m, s) = timer::uptime_hms();
    let dt = rtc::read();
    let tasks = task::list();
    let drivers = driver::list();
    let modules = module::list();
    let n = net::NET.lock();

    r.push_str(&format!("\x1b[1m{}\x1b[0m\n", version::banner()));
    r.push_str(&format!("{}\n\n", version::SLOGAN));

    // Hardware
    r.push_str("\x1b[36m── Hardware ──\x1b[0m\n");
    r.push_str(&format!("  CPU:      {}\n", features.brand));
    r.push_str(&format!("  Cores:    {} logical\n", features.logical_cores));
    r.push_str(&format!("  APIC:     {} | x2APIC: {}\n",
        if features.has_apic { "yes" } else { "no" },
        if features.has_x2apic { "yes" } else { "no" }));
    r.push_str(&format!("  SSE:      {} | AVX: {}\n",
        if features.has_sse { "yes" } else { "no" },
        if features.has_avx { "yes" } else { "no" }));

    // Memory
    r.push_str("\n\x1b[36m── Memory ──\x1b[0m\n");
    r.push_str(&format!("  Physical: {} KiB usable, {} frames allocated\n",
        mem.total_usable_bytes / 1024, mem.allocated_frames));
    r.push_str(&format!("  Heap:     {} / {} bytes ({}%)\n",
        heap.used, heap.total,
        if heap.total > 0 { heap.used * 100 / heap.total } else { 0 }));

    // Storage
    r.push_str("\n\x1b[36m── Storage ──\x1b[0m\n");
    if virtio_blk::is_detected() { r.push_str(&format!("  virtio-blk: {} sectors\n", virtio_blk::capacity())); }
    if ahci::is_detected() { r.push_str(&format!("  AHCI: {}\n", ahci::info())); }
    if nvme::is_detected() { r.push_str(&format!("  NVMe: {}\n", nvme::info())); }
    if !virtio_blk::is_detected() && !ahci::is_detected() && !nvme::is_detected() {
        r.push_str("  (none detected)\n");
    }

    // Network
    r.push_str("\n\x1b[36m── Network ──\x1b[0m\n");
    r.push_str(&format!("  MAC:      {}\n", n.mac));
    r.push_str(&format!("  IP:       {} / {}\n", n.ip, n.netmask));
    r.push_str(&format!("  Gateway:  {}\n", n.gateway));
    if e1000e::is_detected() { r.push_str("  NIC:      Intel e1000e\n"); }
    r.push_str(&format!("  TX/RX:    {} / {} packets\n", n.tx_packets, n.rx_packets));
    drop(n);

    // USB
    r.push_str("\n\x1b[36m── USB ──\x1b[0m\n");
    if xhci::is_detected() { r.push_str(&format!("  xHCI: {}\n", xhci::info())); }
    else { r.push_str("  (no xHCI controller)\n"); }

    // Software
    r.push_str("\n\x1b[36m── Software ──\x1b[0m\n");
    r.push_str(&format!("  Modules:  {} source files\n", version::MODULES));
    r.push_str(&format!("  Commands: {}+\n", version::COMMANDS));
    r.push_str(&format!("  Tasks:    {} running\n", tasks.len()));
    r.push_str(&format!("  Drivers:  {} registered\n", drivers.len()));
    r.push_str(&format!("  KModules: {} loaded\n",
        modules.iter().filter(|m| m.state == crate::module::ModuleState::Loaded).count()));

    // Time
    r.push_str("\n\x1b[36m── Time ──\x1b[0m\n");
    r.push_str(&format!("  Date:     {}\n", dt));
    r.push_str(&format!("  Uptime:   {:02}:{:02}:{:02}\n", h, m, s));
    r.push_str(&format!("  Ticks:    {}\n", timer::ticks()));

    r
}
