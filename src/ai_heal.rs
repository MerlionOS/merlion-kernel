/// AI Self-Healing Kernel (Phase F).
/// When errors or anomalies are detected, the AI subsystem attempts
/// to diagnose the root cause and suggest or apply recovery actions.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

/// Diagnosis result from the AI healer.
pub struct Diagnosis {
    pub symptom: String,
    pub cause: String,
    pub action: String,
    pub auto_fixed: bool,
}

/// Diagnose a page fault and suggest recovery.
pub fn diagnose_page_fault(fault_addr: u64, rip: u64) -> Diagnosis {
    // Null pointer dereference
    if fault_addr < 0x1000 {
        return Diagnosis {
            symptom: format!("Page fault at {:#x} (null region)", fault_addr),
            cause: String::from("Likely null pointer dereference"),
            action: String::from("Kill the faulting task; check for uninitialized pointers"),
            auto_fixed: false,
        };
    }

    // Stack overflow (near task stack boundaries)
    if (fault_addr & 0xFFF) == 0 && fault_addr > 0x1000 {
        return Diagnosis {
            symptom: format!("Page fault at page boundary {:#x}", fault_addr),
            cause: String::from("Possible stack overflow (guard page hit)"),
            action: String::from("Increase task stack size or reduce recursion depth"),
            auto_fixed: false,
        };
    }

    // User space region
    if fault_addr < 0x800000 {
        return Diagnosis {
            symptom: format!("Page fault in user region at {:#x}", fault_addr),
            cause: String::from("User process accessed unmapped memory"),
            action: String::from("Terminate the user process"),
            auto_fixed: false,
        };
    }

    Diagnosis {
        symptom: format!("Page fault at {:#x}, RIP={:#x}", fault_addr, rip),
        cause: String::from("Unknown — address not in any known region"),
        action: String::from("Check memory map with 'memmap' command"),
        auto_fixed: false,
    }
}

/// Diagnose heap exhaustion and suggest recovery.
pub fn diagnose_heap_exhaustion() -> Diagnosis {
    let stats = crate::allocator::stats();
    let tasks = crate::task::list();

    let mut largest_suspect = String::from("unknown");
    if tasks.len() > 3 {
        largest_suspect = String::from("high task count (possible leak)");
    }

    Diagnosis {
        symptom: format!("Heap exhaustion: {}/{} bytes used", stats.used, stats.total),
        cause: format!("Suspected: {}", largest_suspect),
        action: String::from("Kill non-essential tasks; check for allocation leaks"),
        auto_fixed: false,
    }
}

/// Run automatic recovery actions based on system state.
pub fn auto_recover() -> Vec<Diagnosis> {
    let mut actions = Vec::new();

    // Check for finished tasks that can be cleaned up
    let tasks = crate::task::list();
    let finished: Vec<_> = tasks.iter()
        .filter(|t| t.state == crate::task::TaskState::Finished && t.pid != 0)
        .collect();

    if !finished.is_empty() {
        actions.push(Diagnosis {
            symptom: format!("{} finished tasks still in table", finished.len()),
            cause: String::from("Task slots not reclaimed after exit"),
            action: format!("Slots will be reused on next spawn"),
            auto_fixed: true,
        });
    }

    // Check heap fragmentation
    let stats = crate::allocator::stats();
    if stats.used > 0 && stats.free < stats.total / 10 {
        actions.push(diagnose_heap_exhaustion());
    }

    if actions.is_empty() {
        actions.push(Diagnosis {
            symptom: String::from("No issues detected"),
            cause: String::from("—"),
            action: String::from("System is healthy"),
            auto_fixed: false,
        });
    }

    actions
}

/// Format a diagnosis for display.
pub fn format_diagnosis(d: &Diagnosis) -> String {
    let fixed = if d.auto_fixed { " \x1b[32m[auto-fixed]\x1b[0m" } else { "" };
    format!(
        "  \x1b[33mSymptom\x1b[0m:  {}\n  \x1b[33mCause\x1b[0m:    {}\n  \x1b[33mAction\x1b[0m:   {}{}\n",
        d.symptom, d.cause, d.action, fixed
    )
}
