/// System benchmarks — measures performance of kernel subsystems.
/// Reports operations per second and latency for:
/// memory allocation, VFS operations, IPC throughput, timer precision,
/// context switch overhead, and serial I/O.

use crate::{println, timer, vfs, ipc, slab};
use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;

struct BenchResult {
    name: &'static str,
    ops: u64,
    elapsed_ticks: u64,
    unit: &'static str,
}

impl BenchResult {
    fn ops_per_sec(&self) -> u64 {
        if self.elapsed_ticks == 0 { return 0; }
        self.ops * timer::PIT_FREQUENCY_HZ / self.elapsed_ticks
    }

    fn display(&self) -> String {
        format!("  {:<24} {:>8} {} in {:>4} ticks ({} {}/s)",
            self.name, self.ops, self.unit, self.elapsed_ticks,
            self.ops_per_sec(), self.unit)
    }
}

/// Run all benchmarks.
pub fn run_all() {
    println!("\x1b[1m=== MerlionOS System Benchmark ===\x1b[0m");
    println!();

    let results = [
        bench_heap_alloc(),
        bench_heap_large(),
        bench_vfs_read(),
        bench_vfs_write(),
        bench_ipc_throughput(),
        bench_slab_alloc(),
        bench_timer_read(),
        bench_serial_write(),
    ];

    for r in &results {
        let color = if r.ops_per_sec() > 100_000 { "\x1b[32m" }
            else if r.ops_per_sec() > 10_000 { "\x1b[33m" }
            else { "\x1b[31m" };
        println!("{}{}\x1b[0m", color, r.display());
    }

    println!();
    println!("\x1b[1mBenchmark complete.\x1b[0m");
}

fn bench_heap_alloc() -> BenchResult {
    let start = timer::ticks();
    let ops = 1000u64;

    for _ in 0..ops {
        let v = alloc::boxed::Box::new(42u64);
        core::hint::black_box(&v);
        drop(v);
    }

    BenchResult {
        name: "heap alloc+free (8B)",
        ops,
        elapsed_ticks: timer::ticks() - start,
        unit: "ops",
    }
}

fn bench_heap_large() -> BenchResult {
    let start = timer::ticks();
    let ops = 100u64;

    for _ in 0..ops {
        let v: Vec<u8> = Vec::with_capacity(1024);
        core::hint::black_box(&v);
        drop(v);
    }

    BenchResult {
        name: "heap alloc+free (1K)",
        ops,
        elapsed_ticks: timer::ticks() - start,
        unit: "ops",
    }
}

fn bench_vfs_read() -> BenchResult {
    let start = timer::ticks();
    let ops = 500u64;

    for _ in 0..ops {
        let _ = vfs::cat("/proc/uptime");
    }

    BenchResult {
        name: "VFS read /proc/uptime",
        ops,
        elapsed_ticks: timer::ticks() - start,
        unit: "ops",
    }
}

fn bench_vfs_write() -> BenchResult {
    let start = timer::ticks();
    let ops = 200u64;

    for _i in 0..ops {
        let _ = vfs::write("/tmp/_bench", "benchmark test data 1234567890");
    }
    let _ = vfs::rm("/tmp/_bench");

    BenchResult {
        name: "VFS write /tmp",
        ops,
        elapsed_ticks: timer::ticks() - start,
        unit: "ops",
    }
}

fn bench_ipc_throughput() -> BenchResult {
    let ch = match ipc::create() {
        Some(id) => id,
        None => return BenchResult { name: "IPC send+recv", ops: 0, elapsed_ticks: 1, unit: "ops" },
    };

    let start = timer::ticks();
    let ops = 2000u64;

    for _ in 0..ops {
        ipc::send(ch, 0xAA);
        let _ = ipc::recv(ch);
    }

    let elapsed = timer::ticks() - start;
    ipc::destroy(ch);

    BenchResult {
        name: "IPC send+recv (1 byte)",
        ops,
        elapsed_ticks: elapsed,
        unit: "ops",
    }
}

fn bench_slab_alloc() -> BenchResult {
    let start = timer::ticks();
    let ops = 500u64;

    for _ in 0..ops {
        if let Some(ptr) = slab::alloc("fd") {
            slab::free("fd", ptr);
        }
    }

    BenchResult {
        name: "slab alloc+free (fd)",
        ops,
        elapsed_ticks: timer::ticks() - start,
        unit: "ops",
    }
}

fn bench_timer_read() -> BenchResult {
    let start = timer::ticks();
    let ops = 10000u64;

    for _ in 0..ops {
        let _ = timer::ticks();
    }

    BenchResult {
        name: "timer::ticks() read",
        ops,
        elapsed_ticks: timer::ticks() - start,
        unit: "ops",
    }
}

fn bench_serial_write() -> BenchResult {
    let start = timer::ticks();
    let ops = 200u64;

    for _ in 0..ops {
        crate::serial_println!("bench");
    }

    BenchResult {
        name: "serial println",
        ops,
        elapsed_ticks: timer::ticks() - start,
        unit: "ops",
    }
}
