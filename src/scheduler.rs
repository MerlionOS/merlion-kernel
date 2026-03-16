/// Advanced process scheduler for MerlionOS.
///
/// Supports round-robin, strict priority, realtime, and completely-fair (CFS)
/// scheduling policies, per-task priorities with nice/renice, tick accounting,
/// and CPU usage statistics.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::task;
use crate::timer;

/// Scheduling algorithm used by the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerPolicy {
    /// Equal time slices in circular order, ignoring priority.
    RoundRobin,
    /// Lowest numeric priority value runs first; ties broken round-robin.
    Priority,
    /// Same ranking as Priority but re-evaluated more aggressively.
    Realtime,
    /// Tasks accumulate weighted virtual runtime; smallest vruntime runs next.
    Fair,
}

/// Task priority 0-255. Lower numeric value = higher urgency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskPriority(pub u8);

impl TaskPriority {
    pub const DEFAULT: Self = Self(120);
    pub const MAX: Self = Self(0);
    pub const MIN: Self = Self(255);

    /// Create a priority from a raw value.
    pub fn new(val: u8) -> Self { Self(val) }

    /// Apply a signed nice adjustment, clamped to 0..=255.
    pub fn adjust(self, delta: i16) -> Self {
        Self((self.0 as i16).saturating_add(delta).clamp(0, 255) as u8)
    }
}

/// Cumulative scheduler statistics since boot.
#[derive(Debug, Clone)]
pub struct SchedulerStats {
    /// Total context switches performed.
    pub context_switches: u64,
    /// Total timer ticks since scheduler init.
    pub total_ticks: u64,
    /// Ticks spent in the idle task (pid 0).
    pub idle_ticks: u64,
    /// Per-task tick counters keyed by PID.
    pub per_task_ticks: BTreeMap<usize, u64>,
}

struct SchedulerInner {
    policy: SchedulerPolicy,
    priorities: BTreeMap<usize, TaskPriority>,
    ticks: BTreeMap<usize, u64>,
    vruntime: BTreeMap<usize, u64>,
    rr_cursor: usize,
    context_switches: u64,
    total_ticks: u64,
    idle_ticks: u64,
}

static SCHEDULER: Mutex<SchedulerInner> = Mutex::new(SchedulerInner {
    policy: SchedulerPolicy::RoundRobin,
    priorities: BTreeMap::new(),
    ticks: BTreeMap::new(),
    vruntime: BTreeMap::new(),
    rr_cursor: 0,
    context_switches: 0,
    total_ticks: 0,
    idle_ticks: 0,
});

/// Lock-free context-switch counter for lightweight callers.
static CTX_SWITCHES: AtomicU64 = AtomicU64::new(0);

/// Select the active scheduling policy.
pub fn set_policy(policy: SchedulerPolicy) {
    let mut s = SCHEDULER.lock();
    s.policy = policy;
    if policy == SchedulerPolicy::Fair {
        s.vruntime.clear();
    }
}

/// Return the currently active scheduling policy.
pub fn current_policy() -> SchedulerPolicy {
    SCHEDULER.lock().policy
}

/// Set the priority for a task identified by PID.
pub fn set_task_priority(pid: usize, priority: TaskPriority) {
    SCHEDULER.lock().priorities.insert(pid, priority);
}

/// Get the priority for *pid* (defaults to `TaskPriority::DEFAULT`).
pub fn get_task_priority(pid: usize) -> TaskPriority {
    SCHEDULER.lock().priorities.get(&pid).copied().unwrap_or(TaskPriority::DEFAULT)
}

/// Choose the next task to run according to the active policy.
///
/// Returns `Some(pid)` or `None` if no ready task exists.
pub fn get_next_task() -> Option<usize> {
    let active = task::list();
    if active.is_empty() { return None; }
    let current_pid = task::current_pid();
    let mut s = SCHEDULER.lock();

    let ready: alloc::vec::Vec<usize> = active.iter()
        .filter(|t| t.state == task::TaskState::Ready || t.pid == current_pid)
        .map(|t| t.pid)
        .collect();
    if ready.is_empty() { return None; }

    let chosen = match s.policy {
        SchedulerPolicy::RoundRobin => pick_rr(&ready, &mut s.rr_cursor),
        SchedulerPolicy::Priority | SchedulerPolicy::Realtime => {
            { let p = s.priorities.clone(); pick_priority(&ready, &p, &mut s.rr_cursor) }
        }
        SchedulerPolicy::Fair => { let p = s.priorities.clone(); pick_fair(&ready, &mut s.vruntime, &p) },
    };
    if chosen != current_pid {
        s.context_switches += 1;
        CTX_SWITCHES.fetch_add(1, Ordering::Relaxed);
    }
    Some(chosen)
}

/// Called from the timer interrupt on every PIT tick.
///
/// Updates tick accounting, idle tracking, and Fair-policy virtual runtime.
pub fn scheduler_tick() {
    let pid = task::current_pid();
    let mut s = SCHEDULER.lock();
    s.total_ticks += 1;
    *s.ticks.entry(pid).or_insert(0) += 1;
    if pid == 0 { s.idle_ticks += 1; }
    if s.policy == SchedulerPolicy::Fair {
        let w = s.priorities.get(&pid).copied().unwrap_or(TaskPriority::DEFAULT).0 as u64 + 1;
        *s.vruntime.entry(pid).or_insert(0) += w;
    }
}

/// Apply a UNIX-style nice adjustment to a task's priority.
///
/// Positive values lower urgency; negative values raise it. Clamped to 0..=255.
pub fn nice(pid: usize, adjustment: i16) {
    let mut s = SCHEDULER.lock();
    let cur = s.priorities.get(&pid).copied().unwrap_or(TaskPriority::DEFAULT);
    s.priorities.insert(pid, cur.adjust(adjustment));
}

/// Set an absolute priority for *pid* (shell `renice` command).
pub fn renice(pid: usize, new_priority: u8) {
    set_task_priority(pid, TaskPriority::new(new_priority));
}

/// CPU usage for *pid* as a percentage (0-100).
pub fn cpu_usage_percent(pid: usize) -> u8 {
    let s = SCHEDULER.lock();
    if s.total_ticks == 0 { return 0; }
    let t = s.ticks.get(&pid).copied().unwrap_or(0);
    ((t * 100) / s.total_ticks) as u8
}

/// Build a snapshot of all scheduler statistics.
pub fn stats() -> SchedulerStats {
    let s = SCHEDULER.lock();
    SchedulerStats {
        context_switches: s.context_switches,
        total_ticks: s.total_ticks,
        idle_ticks: s.idle_ticks,
        per_task_ticks: s.ticks.clone(),
    }
}

/// Pretty-print scheduler statistics for display in the shell or log.
pub fn format_scheduler_stats() -> String {
    let s = SCHEDULER.lock();
    let uptime = timer::uptime_secs();
    let busy = s.total_ticks.saturating_sub(s.idle_ticks);
    let util = if s.total_ticks > 0 { (busy * 100) / s.total_ticks } else { 0 };
    let policy_name = match s.policy {
        SchedulerPolicy::RoundRobin => "RoundRobin",
        SchedulerPolicy::Priority  => "Priority",
        SchedulerPolicy::Realtime  => "Realtime",
        SchedulerPolicy::Fair      => "Fair (CFS)",
    };

    let mut out = format!(
        "Scheduler Policy : {}\nUptime           : {} s\n\
         Context Switches : {}\nTotal Ticks      : {}\n\
         Idle  Ticks      : {}\nCPU Utilisation  : {}%\n---\n\
         PID   PRIO  TICKS   CPU%  VRUNTIME\n",
        policy_name, uptime, s.context_switches, s.total_ticks, s.idle_ticks, util,
    );
    for (&pid, &ticks) in &s.ticks {
        let prio = s.priorities.get(&pid).copied().unwrap_or(TaskPriority::DEFAULT).0;
        let cpu = if s.total_ticks > 0 { (ticks * 100) / s.total_ticks } else { 0 };
        let vrt = s.vruntime.get(&pid).copied().unwrap_or(0);
        out.push_str(&format!("{:<6}{:<6}{:<8}{:<6}{}\n", pid, prio, ticks, cpu, vrt));
    }
    out
}

// --- Internal helpers --------------------------------------------------------

/// Round-robin: advance cursor through the ready list.
fn pick_rr(ready: &[usize], cursor: &mut usize) -> usize {
    *cursor = (*cursor + 1) % ready.len();
    ready[*cursor]
}

/// Priority/Realtime: pick lowest numeric priority; ties broken round-robin.
fn pick_priority(
    ready: &[usize],
    priorities: &BTreeMap<usize, TaskPriority>,
    cursor: &mut usize,
) -> usize {
    let prio_of = |pid: &usize| priorities.get(pid).copied().unwrap_or(TaskPriority::DEFAULT).0;
    let best = ready.iter().map(prio_of).min().unwrap_or(TaskPriority::DEFAULT.0);
    let cands: alloc::vec::Vec<usize> = ready.iter().copied().filter(|p| prio_of(p) == best).collect();
    if cands.len() == 1 { return cands[0]; }
    *cursor = (*cursor + 1) % cands.len();
    cands[*cursor]
}

/// Fair (CFS-like): pick the task with the smallest virtual runtime.
fn pick_fair(
    ready: &[usize],
    vruntime: &mut BTreeMap<usize, u64>,
    priorities: &BTreeMap<usize, TaskPriority>,
) -> usize {
    let min_vrt = ready.iter().filter_map(|p| vruntime.get(p)).copied().min().unwrap_or(0);
    for &pid in ready { vruntime.entry(pid).or_insert(min_vrt); }
    ready.iter().copied().min_by_key(|&pid| {
        let vrt = vruntime.get(&pid).copied().unwrap_or(0);
        let prio = priorities.get(&pid).copied().unwrap_or(TaskPriority::DEFAULT).0 as u64;
        (vrt, prio)
    }).unwrap_or(0)
}
