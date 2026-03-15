/// Kernel task (thread) management and cooperative scheduler.
/// Tasks share the kernel address space and switch cooperatively via yield_now().
/// The timer interrupt also triggers a preemptive yield every time slice.

use alloc::boxed::Box;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;
use crate::{serial_println, klog_println};

const MAX_TASKS: usize = 8;
const TASK_STACK_SIZE: usize = 4096 * 4; // 16 KiB per task

/// Index of the currently running task.
static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// Global task table. Slot 0 is always the kernel/idle task.
static TASKS: Mutex<[TaskSlot; MAX_TASKS]> = Mutex::new([const { TaskSlot::Empty }; MAX_TASKS]);

/// Next PID to assign.
static NEXT_PID: AtomicUsize = AtomicUsize::new(1);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TaskState {
    Ready,
    Running,
    Finished,
}

pub struct TaskInfo {
    pub pid: usize,
    pub name: &'static str,
    pub state: TaskState,
}

enum TaskSlot {
    Empty,
    Occupied {
        pid: usize,
        name: &'static str,
        state: TaskState,
        rsp: u64,
        // Keep the stack allocation alive
        _stack: Option<Box<[u8; TASK_STACK_SIZE]>>,
    },
}

/// Initialize the task system. Registers the current execution context as task 0 (kernel).
pub fn init() {
    let mut tasks = TASKS.lock();
    tasks[0] = TaskSlot::Occupied {
        pid: 0,
        name: "kernel",
        state: TaskState::Running,
        rsp: 0, // will be filled on first context switch
        _stack: None, // kernel uses its own boot stack
    };
}

/// Spawn a new task. Returns the PID, or None if the task table is full.
pub fn spawn(name: &'static str, entry: fn()) -> Option<usize> {
    let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);

    // Allocate a stack for the task
    let mut stack = Box::new([0u8; TASK_STACK_SIZE]);
    let stack_top = stack.as_mut_ptr() as u64 + TASK_STACK_SIZE as u64;

    // Set up the initial stack so context_switch will "return" into task_wrapper.
    // Stack layout (growing downward):
    //   [task_wrapper address]  <- ret target
    //   [r15=0] [r14=0] [r13=0] [r12=entry] [rbp=0] [rbx=0]
    // r12 holds the entry function pointer, passed to task_wrapper.
    let init_rsp = stack_top - 56; // 7 * 8 bytes
    unsafe {
        let sp = init_rsp as *mut u64;
        sp.add(0).write(0);                            // rbx
        sp.add(1).write(0);                            // rbp
        sp.add(2).write(entry as *const () as u64);    // r12 = entry fn
        sp.add(3).write(0);                            // r13
        sp.add(4).write(0);                            // r14
        sp.add(5).write(0);                            // r15
        sp.add(6).write(task_wrapper as *const () as u64); // return address
    }

    let mut tasks = TASKS.lock();
    for slot in tasks.iter_mut().skip(1) {
        if matches!(slot, TaskSlot::Empty | TaskSlot::Occupied { state: TaskState::Finished, .. }) {
            *slot = TaskSlot::Occupied {
                pid,
                name,
                state: TaskState::Ready,
                rsp: init_rsp,
                _stack: Some(stack),
            };
            klog_println!("[task] spawned '{}' (pid {})", name, pid);
            serial_println!("[task] spawned '{}' (pid {})", name, pid);
            return Some(pid);
        }
    }
    None // table full
}

/// List all active tasks.
pub fn list() -> alloc::vec::Vec<TaskInfo> {
    let tasks = TASKS.lock();
    let mut result = alloc::vec::Vec::new();
    for slot in tasks.iter() {
        if let TaskSlot::Occupied { pid, name, state, .. } = slot {
            if *state != TaskState::Finished {
                result.push(TaskInfo { pid: *pid, name, state: *state });
            }
        }
    }
    result
}

/// Yield the current task's time slice. Switches to the next ready task.
pub fn yield_now() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let current = CURRENT.load(Ordering::SeqCst);
        let mut tasks = TASKS.lock();

        // Find next ready task (round-robin)
        let mut next = (current + 1) % MAX_TASKS;
        let mut found = false;
        for _ in 0..MAX_TASKS {
            if let TaskSlot::Occupied { state: TaskState::Ready, .. } = &tasks[next] {
                found = true;
                break;
            }
            next = (next + 1) % MAX_TASKS;
        }

        if !found || next == current {
            return;
        }

        // Update states
        if let TaskSlot::Occupied { state, .. } = &mut tasks[current] {
            if *state == TaskState::Running {
                *state = TaskState::Ready;
            }
        }
        if let TaskSlot::Occupied { state, .. } = &mut tasks[next] {
            *state = TaskState::Running;
        }

        // Get raw pointers to RSP storage (safe: static array, interrupts disabled)
        let old_rsp = match &mut tasks[current] {
            TaskSlot::Occupied { rsp, .. } => rsp as *mut u64,
            _ => return,
        };
        let new_rsp = match &tasks[next] {
            TaskSlot::Occupied { rsp, .. } => *rsp,
            _ => return,
        };

        CURRENT.store(next, Ordering::SeqCst);
        drop(tasks); // release lock before switching

        context_switch(old_rsp, new_rsp);
    });
}

/// Exit the current task. Marks it as finished and yields.
pub fn exit() -> ! {
    {
        let current = CURRENT.load(Ordering::SeqCst);
        let mut tasks = TASKS.lock();
        if let TaskSlot::Occupied { state, pid, name, .. } = &mut tasks[current] {
            klog_println!("[task] '{}' (pid {}) exited", name, pid);
            serial_println!("[task] '{}' (pid {}) exited", name, pid);
            *state = TaskState::Finished;
        }
    }
    yield_now();
    // Should not reach here, but just in case
    loop { x86_64::instructions::hlt(); }
}

/// Get the current task's slot index.
pub fn current_slot() -> usize {
    CURRENT.load(Ordering::SeqCst)
}

/// Get the current task's PID.
pub fn current_pid() -> usize {
    let current = CURRENT.load(Ordering::SeqCst);
    let tasks = TASKS.lock();
    match &tasks[current] {
        TaskSlot::Occupied { pid, .. } => *pid,
        _ => 0,
    }
}

/// Called by the timer to attempt preemptive scheduling.
pub fn timer_tick() {
    // Only switch if there are other ready tasks
    let current = CURRENT.load(Ordering::SeqCst);
    let tasks = TASKS.lock();
    let has_others = tasks.iter().enumerate().any(|(i, slot)| {
        i != current && matches!(slot, TaskSlot::Occupied { state: TaskState::Ready, .. })
    });
    drop(tasks);

    if has_others {
        yield_now();
    }
}

/// Wrapper that runs a task's entry function, then exits cleanly.
fn task_wrapper() {
    // r12 holds the entry function pointer (set up in spawn)
    let entry: fn();
    unsafe {
        core::arch::asm!("mov {}, r12", out(reg) entry);
    }
    entry();
    exit();
}

/// Low-level context switch. Saves callee-saved registers and RSP to old_rsp,
/// then restores from new_rsp. Returns when this task is scheduled again.
///
/// # Safety
/// old_rsp must be a valid pointer. new_rsp must point to a valid saved context.
#[unsafe(naked)]
extern "C" fn context_switch(_old_rsp: *mut u64, _new_rsp: u64) {
    core::arch::naked_asm!(
            // Save callee-saved registers
            "push rbx",
            "push rbp",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            // Save current stack pointer
            "mov [rdi], rsp",
            // Switch to new stack
            "mov rsp, rsi",
            // Restore callee-saved registers
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop rbp",
            "pop rbx",
        // Return to the new task (address is on top of its stack)
        "ret",
    );
}
