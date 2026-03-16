/// Real-time scheduling for MerlionOS.
/// Implements EDF (Earliest Deadline First) and Rate Monotonic scheduling,
/// priority inheritance for mutex holders, and RT task management.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Maximum number of real-time tasks.
const MAX_RT_TASKS: usize = 64;

/// Maximum number of active priority inheritance boosts.
const MAX_PI_ENTRIES: usize = 32;

/// Scheduling policy for a real-time task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtPolicy {
    /// First-in-first-out with static priority.
    Fifo,
    /// Round-robin with time quantum.
    RoundRobin,
    /// Earliest Deadline First (dynamic priority).
    EDF,
    /// Rate Monotonic (shorter period = higher effective priority).
    RateMonotonic,
}

/// Execution state of a real-time task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtState {
    Ready,
    Running,
    Blocked,
    Completed,
}

/// A real-time task descriptor.
#[derive(Debug, Clone)]
pub struct RtTask {
    pub id: u32,
    pub name: String,
    pub policy: RtPolicy,
    pub priority: u8,
    pub period_ticks: u64,
    pub deadline_ticks: u64,
    pub wcet_ticks: u64,
    pub state: RtState,
    pub created_tick: u64,
    pub last_run: u64,
    pub miss_count: u32,
    pub run_count: u64,
}

/// Record of a priority inheritance boost on a single task.
#[derive(Debug, Clone)]
struct PriorityInheritance {
    holder_id: u32,
    original_priority: u8,
    boosted_priority: u8,
    resource_id: u32,
}

/// Internal state of the real-time scheduler.
struct RtSchedInner {
    tasks: Vec<RtTask>,
    pi_entries: Vec<PriorityInheritance>,
    next_id: u32,
    current_tick: u64,
    rr_index: usize,
}

impl RtSchedInner {
    fn new() -> Self {
        Self {
            tasks: Vec::new(),
            pi_entries: Vec::new(),
            next_id: 1,
            current_tick: 0,
            rr_index: 0,
        }
    }

    /// Effective priority of a task, accounting for priority inheritance.
    fn effective_priority(&self, task: &RtTask) -> u8 {
        self.pi_entries.iter()
            .filter(|pi| pi.holder_id == task.id)
            .map(|pi| pi.boosted_priority)
            .max()
            .unwrap_or(task.priority)
    }
}

/// Global RT scheduler state, protected by a spinlock.
static RT_SCHED: Mutex<Option<RtSchedInner>> = Mutex::new(None);

/// Total deadline misses across all tasks (lock-free counter).
static TOTAL_MISSES: AtomicU64 = AtomicU64::new(0);

/// Total tick count processed by the scheduler.
static TOTAL_TICKS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Task management
// ---------------------------------------------------------------------------

/// Create a new real-time task. Returns its assigned id, or `None` if the
/// task table is full or the scheduler is not initialised.
pub fn create_rt_task(
    name: &str,
    policy: RtPolicy,
    priority: u8,
    period: u64,
    deadline: u64,
    wcet: u64,
) -> Option<u32> {
    let mut sched = RT_SCHED.lock();
    let inner = sched.as_mut()?;
    if inner.tasks.len() >= MAX_RT_TASKS {
        return None;
    }
    let id = inner.next_id;
    inner.next_id += 1;
    inner.tasks.push(RtTask {
        id,
        name: String::from(name),
        policy,
        priority: priority.min(99),
        period_ticks: period,
        deadline_ticks: inner.current_tick + deadline,
        wcet_ticks: wcet,
        state: RtState::Ready,
        created_tick: inner.current_tick,
        last_run: 0,
        miss_count: 0,
        run_count: 0,
    });
    Some(id)
}

/// Remove a real-time task by id. Returns `true` if found and removed.
pub fn remove_rt_task(id: u32) -> bool {
    let mut sched = RT_SCHED.lock();
    let inner = match sched.as_mut() {
        Some(i) => i,
        None => return false,
    };
    // Also remove any PI entries for this task.
    inner.pi_entries.retain(|pi| pi.holder_id != id);
    if let Some(pos) = inner.tasks.iter().position(|t| t.id == id) {
        inner.tasks.remove(pos);
        true
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Priority inheritance protocol
// ---------------------------------------------------------------------------

/// Boost `holder_id`'s priority to match `blocker_id`'s priority when the
/// holder is blocking a higher-priority task on `resource_id`.
pub fn priority_boost(holder_id: u32, blocker_id: u32, resource_id: u32) {
    let mut sched = RT_SCHED.lock();
    let inner = match sched.as_mut() {
        Some(i) => i,
        None => return,
    };
    if inner.pi_entries.len() >= MAX_PI_ENTRIES {
        return;
    }
    let holder_pri = inner.tasks.iter()
        .find(|t| t.id == holder_id)
        .map(|t| t.priority)
        .unwrap_or(0);
    let blocker_pri = inner.tasks.iter()
        .find(|t| t.id == blocker_id)
        .map(|t| inner.effective_priority(t))
        .unwrap_or(0);

    if blocker_pri > holder_pri {
        inner.pi_entries.push(PriorityInheritance {
            holder_id,
            original_priority: holder_pri,
            boosted_priority: blocker_pri,
            resource_id,
        });
    }
}

/// Restore `holder_id`'s priority by removing all inheritance entries for it.
pub fn priority_restore(holder_id: u32) {
    let mut sched = RT_SCHED.lock();
    let inner = match sched.as_mut() {
        Some(i) => i,
        None => return,
    };
    inner.pi_entries.retain(|pi| pi.holder_id != holder_id);
}

// ---------------------------------------------------------------------------
// Scheduling
// ---------------------------------------------------------------------------

/// Pick the next task to run according to each task's policy.
///
/// Strategy:
/// - EDF tasks: earliest absolute deadline wins.
/// - RateMonotonic tasks: shortest period wins.
/// - Fifo/RoundRobin tasks: highest effective priority wins (RR rotates
///   among equal-priority tasks).
///
/// Returns the task id of the chosen task, or `None` if no ready task exists.
pub fn schedule_next() -> Option<u32> {
    let mut sched = RT_SCHED.lock();
    let inner = sched.as_mut()?;

    // Collect ready task indices.
    let ready: Vec<usize> = inner.tasks.iter().enumerate()
        .filter(|(_, t)| t.state == RtState::Ready)
        .map(|(i, _)| i)
        .collect();

    if ready.is_empty() {
        return None;
    }

    // Separate by policy; pick the best candidate from each class, then
    // choose the overall winner by effective priority / urgency.

    let mut best_idx: Option<usize> = None;
    let mut best_score: u64 = u64::MAX; // lower is more urgent

    for &idx in &ready {
        let task = &inner.tasks[idx];
        let eff_pri = inner.effective_priority(task);
        let score = match task.policy {
            RtPolicy::EDF => task.deadline_ticks,
            RtPolicy::RateMonotonic => task.period_ticks,
            RtPolicy::Fifo => (100u64).saturating_sub(eff_pri as u64),
            RtPolicy::RoundRobin => {
                // Bias toward round-robin rotation index.
                let base = (100u64).saturating_sub(eff_pri as u64);
                if idx == inner.rr_index % inner.tasks.len() {
                    base.saturating_sub(1)
                } else {
                    base
                }
            }
        };
        if best_idx.is_none() || score < best_score {
            best_score = score;
            best_idx = Some(idx);
        }
    }

    if let Some(idx) = best_idx {
        inner.tasks[idx].state = RtState::Running;
        inner.tasks[idx].run_count += 1;
        inner.tasks[idx].last_run = inner.current_tick;
        inner.rr_index = idx + 1;
        Some(inner.tasks[idx].id)
    } else {
        None
    }
}

/// Check all tasks for deadline misses. A task misses its deadline if it
/// is still `Ready` (not yet started) past its `deadline_ticks`.
pub fn check_deadlines() {
    let mut sched = RT_SCHED.lock();
    let inner = match sched.as_mut() {
        Some(i) => i,
        None => return,
    };
    let now = inner.current_tick;
    for task in &mut inner.tasks {
        if task.state == RtState::Ready && now > task.deadline_ticks {
            task.miss_count += 1;
            // Advance deadline to next period.
            task.deadline_ticks = now + task.period_ticks;
            TOTAL_MISSES.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Called from the timer interrupt. Advances the tick counter, resets
/// completed/running tasks back to ready if their period has elapsed,
/// and checks deadlines.
pub fn tick() {
    let mut sched = RT_SCHED.lock();
    let inner = match sched.as_mut() {
        Some(i) => i,
        None => return,
    };
    inner.current_tick += 1;
    let now = inner.current_tick;
    TOTAL_TICKS.fetch_add(1, Ordering::Relaxed);

    // Reset periodic tasks whose period has elapsed.
    for task in &mut inner.tasks {
        if task.state == RtState::Running || task.state == RtState::Completed {
            if task.period_ticks > 0 && now >= task.last_run + task.period_ticks {
                task.state = RtState::Ready;
                task.deadline_ticks = now + task.period_ticks;
            }
        }
    }

    // Check for deadline misses (inline to avoid double-lock).
    for task in &mut inner.tasks {
        if task.state == RtState::Ready && now > task.deadline_ticks {
            task.miss_count += 1;
            task.deadline_ticks = now + task.period_ticks;
            TOTAL_MISSES.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------
// Analysis & info
// ---------------------------------------------------------------------------

/// Rate Monotonic schedulability test using the Liu & Layland utilization
/// bound: U <= n * (2^(1/n) - 1).  Returns a formatted analysis string.
pub fn schedulability_test() -> String {
    let sched = RT_SCHED.lock();
    let inner = match sched.as_ref() {
        Some(i) => i,
        None => return "(rt_sched not initialised)\n".into(),
    };
    let rm_tasks: Vec<&RtTask> = inner.tasks.iter()
        .filter(|t| t.policy == RtPolicy::RateMonotonic && t.period_ticks > 0)
        .collect();
    if rm_tasks.is_empty() {
        return "No Rate Monotonic tasks to analyse.\n".into();
    }

    // Compute total utilisation U = sum(wcet / period) using integer
    // arithmetic scaled by 1000 for three-decimal precision.
    let n = rm_tasks.len();
    let mut u_scaled: u64 = 0;
    for t in &rm_tasks {
        u_scaled += (t.wcet_ticks * 1000) / t.period_ticks;
    }

    // Utilization bound: n * (2^(1/n) - 1)
    // For small n, use precomputed values (scaled by 1000).
    let bound_scaled: u64 = match n {
        1 => 1000,  // 1.000
        2 => 828,   // 0.828
        3 => 780,   // 0.780
        4 => 757,   // 0.757
        5 => 743,   // 0.743
        _ => 693,   // ln(2) ~= 0.693 (limit as n -> inf)
    };

    let pass = u_scaled <= bound_scaled;
    format!(
        "RM Schedulability Test:\n  Tasks: {}\n  Total utilisation: {}.{}%\n  \
         Bound (n={}): {}.{}%\n  Result: {}\n",
        n,
        u_scaled / 10, u_scaled % 10,
        n,
        bound_scaled / 10, bound_scaled % 10,
        if pass { "SCHEDULABLE" } else { "NOT GUARANTEED" },
    )
}

/// Return a formatted list of all real-time tasks.
pub fn list_rt_tasks() -> String {
    let sched = RT_SCHED.lock();
    let inner = match sched.as_ref() {
        Some(i) => i,
        None => return "(rt_sched not initialised)\n".into(),
    };
    if inner.tasks.is_empty() {
        return "(no RT tasks)\n".into();
    }
    let mut out = String::new();
    out.push_str("ID   NAME             POLICY  PRI  PERIOD   DEADLINE WCET  STATE     MISSES RUNS\n");
    out.push_str("---- ---------------- ------- ---- -------- -------- ----- --------- ------ ----\n");
    for t in &inner.tasks {
        let policy = match t.policy {
            RtPolicy::Fifo          => "FIFO   ",
            RtPolicy::RoundRobin    => "RR     ",
            RtPolicy::EDF           => "EDF    ",
            RtPolicy::RateMonotonic => "RM     ",
        };
        let state = match t.state {
            RtState::Ready     => "Ready    ",
            RtState::Running   => "Running  ",
            RtState::Blocked   => "Blocked  ",
            RtState::Completed => "Complete ",
        };
        let eff_pri = inner.effective_priority(t);
        let pri_str = if eff_pri != t.priority {
            format!("{}*", eff_pri)
        } else {
            format!("{}", t.priority)
        };
        out.push_str(&format!(
            "{:<4} {:<16} {} {:<4} {:<8} {:<8} {:<5} {} {:<6} {}\n",
            t.id, t.name, policy, pri_str,
            t.period_ticks, t.deadline_ticks, t.wcet_ticks,
            state, t.miss_count, t.run_count,
        ));
    }
    out
}

/// Return overall RT scheduler statistics.
pub fn rt_stats() -> String {
    let sched = RT_SCHED.lock();
    let inner = match sched.as_ref() {
        Some(i) => i,
        None => return "(rt_sched not initialised)\n".into(),
    };
    let total = inner.tasks.len();
    let ready = inner.tasks.iter().filter(|t| t.state == RtState::Ready).count();
    let running = inner.tasks.iter().filter(|t| t.state == RtState::Running).count();
    let blocked = inner.tasks.iter().filter(|t| t.state == RtState::Blocked).count();
    let misses: u32 = inner.tasks.iter().map(|t| t.miss_count).sum();
    let runs: u64 = inner.tasks.iter().map(|t| t.run_count).sum();
    let pi_active = inner.pi_entries.len();
    let ticks = TOTAL_TICKS.load(Ordering::Relaxed);
    format!(
        "RT Scheduler stats:\n  Tasks: {} total ({} ready, {} running, {} blocked)\n  \
         Deadline misses: {}\n  Total runs: {}\n  Active PI boosts: {}\n  \
         Ticks processed: {}\n  Current tick: {}\n",
        total, ready, running, blocked,
        misses, runs, pi_active, ticks, inner.current_tick,
    )
}

/// Initialise the real-time scheduler.
pub fn init() {
    *RT_SCHED.lock() = Some(RtSchedInner::new());
}
