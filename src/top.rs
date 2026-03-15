/// `top` — live system monitor with auto-refresh.
/// Displays tasks, memory, CPU, uptime in a full-screen view.
/// Press 'q' to exit. Refreshes every second.

use crate::{timer, task, allocator, memory, smp, rtc, keyboard::KeyEvent, version};
use core::sync::atomic::{AtomicBool, Ordering};

static RUNNING: AtomicBool = AtomicBool::new(false);

pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

pub fn handle_input(event: KeyEvent) {
    if let KeyEvent::Char('q') = event {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

/// Run the top display (blocks until 'q').
pub fn run() {
    RUNNING.store(true, Ordering::SeqCst);

    while RUNNING.load(Ordering::SeqCst) {
        draw();

        // Wait ~1 second (100 ticks at 100Hz)
        let next = timer::ticks() + 100;
        while timer::ticks() < next && RUNNING.load(Ordering::SeqCst) {
            x86_64::instructions::hlt();
        }
    }

    RUNNING.store(false, Ordering::SeqCst);
}

fn draw() {
    let vga = 0xB8000 as *mut u8;
    let (h, m, s) = timer::uptime_hms();
    let ticks = timer::ticks();
    let dt = rtc::read();
    let heap = allocator::stats();
    let mem = memory::stats();
    let tasks = task::list();
    let features = smp::detect_features();

    let heap_pct = if heap.total > 0 { heap.used * 100 / heap.total } else { 0 };
    let phys_used_kb = mem.allocated_frames * 4;
    let phys_total_kb = mem.total_usable_bytes / 1024;

    // Clear screen
    for i in 0..80 * 25 {
        unsafe {
            vga.add(i * 2).write_volatile(b' ');
            vga.add(i * 2 + 1).write_volatile(0x00);
        }
    }

    // Row 0: header bar
    let header = alloc::format!(
        " top — {} | {} | up {:02}:{:02}:{:02} | {} ticks",
        version::full(), dt, h, m, s, ticks
    );
    write_row(vga, 0, &header, 0x70); // black on white

    // Row 1: CPU
    let cpu_line = alloc::format!(
        " CPU: {} | {} core(s) | APIC: {}",
        features.brand, features.logical_cores,
        if features.has_apic { "yes" } else { "no" }
    );
    write_row(vga, 1, &cpu_line, 0x0B); // cyan

    // Row 2: Memory
    let mem_line = alloc::format!(
        " Mem: {}K / {}K phys | Heap: {} / {} ({}%)",
        phys_used_kb, phys_total_kb, heap.used, heap.total, heap_pct
    );
    let mem_attr = if heap_pct > 80 { 0x0C } else { 0x0A }; // red if high, green
    write_row(vga, 2, &mem_line, mem_attr);

    // Row 3: Tasks summary
    let running = tasks.iter().filter(|t| t.state == task::TaskState::Running).count();
    let ready = tasks.iter().filter(|t| t.state == task::TaskState::Ready).count();
    let task_line = alloc::format!(
        " Tasks: {} total, {} running, {} ready",
        tasks.len(), running, ready
    );
    write_row(vga, 3, &task_line, 0x0E); // yellow

    // Row 4: separator
    write_row(vga, 4, "", 0x08);

    // Row 5: task table header
    let th = "   PID  STATE     NAME";
    write_row(vga, 5, th, 0x0F); // white bold

    // Row 6+: tasks
    for (i, t) in tasks.iter().enumerate() {
        if i + 6 >= 23 { break; }
        let state = match t.state {
            task::TaskState::Running  => "\x1b[32mrunning \x1b[0m",
            task::TaskState::Ready    => "ready   ",
            task::TaskState::Finished => "\x1b[90mfinished\x1b[0m",
        };
        let line = alloc::format!("   {:3}  {}  {}", t.pid, state, t.name);
        // Use raw write to handle state color
        let state_str = match t.state {
            task::TaskState::Running  => "running ",
            task::TaskState::Ready    => "ready   ",
            task::TaskState::Finished => "finished",
        };
        let raw = alloc::format!("   {:3}  {}  {}", t.pid, state_str, t.name);
        let attr = match t.state {
            task::TaskState::Running  => 0x0A, // green
            task::TaskState::Ready    => 0x07, // gray
            task::TaskState::Finished => 0x08, // dark gray
        };
        write_row(vga, i + 6, &raw, attr);
    }

    // Row 24: help bar
    write_row(vga, 24, " Press 'q' to exit top", 0x70);
}

fn write_row(vga: *mut u8, row: usize, text: &str, attr: u8) {
    let bytes = text.as_bytes();
    for x in 0..80 {
        let ch = bytes.get(x).copied().unwrap_or(b' ');
        unsafe {
            let off = (row * 80 + x) * 2;
            vga.add(off).write_volatile(ch);
            vga.add(off + 1).write_volatile(attr);
        }
    }
}
