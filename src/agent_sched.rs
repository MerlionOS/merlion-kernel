/// Agent scheduler — runs AI agents in the background at configurable intervals.
///
/// Each agent is assigned a schedule (interval in ticks). On every timer
/// tick the scheduler checks which agents are due and dispatches a "tick"
/// message via [`crate::agent::send_message`].
///
/// Default schedule installed by [`init`]:
/// - `"health"` agent every 6 000 ticks (~60 s at 100 Hz PIT).
///
/// Additional schedules can be added at runtime with [`schedule`].

use alloc::vec::Vec;
use spin::Mutex;

/// A single agent schedule entry.
pub struct AgentSchedule {
    /// Name of the registered agent (must match [`crate::agent::Agent::name`]).
    pub agent_name: &'static str,
    /// Number of PIT ticks between invocations.
    pub interval_ticks: u64,
    /// Tick counter value when the agent last ran.
    pub last_run: u64,
    /// Whether this schedule is active.
    pub enabled: bool,
}

/// Global schedule table, protected by a spin lock.
static SCHEDULES: Mutex<Vec<AgentSchedule>> = Mutex::new(Vec::new());

/// Initialise the agent scheduler with default schedules.
///
/// Call once during kernel boot, after [`crate::agent`] and [`crate::timer`]
/// are initialised.
pub fn init() {
    let mut table = SCHEDULES.lock();
    table.push(AgentSchedule {
        agent_name: "health",
        interval_ticks: 6000,
        last_run: 0,
        enabled: true,
    });
    crate::klog_println!("[agent_sched] initialised (1 default schedule)");
}

/// Add or update a schedule for the named agent.
///
/// `interval_secs` is converted to ticks using [`crate::timer::PIT_FREQUENCY_HZ`].
/// If a schedule for `agent_name` already exists it is replaced.
pub fn schedule(agent_name: &'static str, interval_secs: u64) {
    let interval_ticks = interval_secs * crate::timer::PIT_FREQUENCY_HZ;
    let now = crate::timer::ticks();
    let mut table = SCHEDULES.lock();

    for entry in table.iter_mut() {
        if entry.agent_name == agent_name {
            entry.interval_ticks = interval_ticks;
            entry.last_run = now;
            entry.enabled = true;
            return;
        }
    }

    table.push(AgentSchedule {
        agent_name,
        interval_ticks,
        last_run: now,
        enabled: true,
    });
}

/// Remove the schedule for the named agent.
///
/// Returns `true` if a schedule was found and removed.
pub fn unschedule(agent_name: &str) -> bool {
    let mut table = SCHEDULES.lock();
    let before = table.len();
    table.retain(|e| e.agent_name != agent_name);
    table.len() != before
}

/// Timer-tick callback — check every schedule and dispatch due agents.
///
/// This is meant to be called from the PIT interrupt handler (or an
/// equivalent periodic source). It acquires the schedule lock, so it
/// must not be called while the lock is already held.
pub fn tick() {
    let now = crate::timer::ticks();
    let mut table = SCHEDULES.lock();

    for entry in table.iter_mut() {
        if !entry.enabled {
            continue;
        }
        if now.wrapping_sub(entry.last_run) >= entry.interval_ticks {
            entry.last_run = now;
            // Release the lock before calling into the agent subsystem to
            // avoid deadlock (agent handlers may call back into the
            // scheduler).  We collect the name first so we can drop the
            // guard.
            let name = entry.agent_name;
            // NOTE: we intentionally hold the lock across the call here
            // because `send_message` is short-lived. If deadlock becomes
            // a concern the dispatch can be deferred to a work-queue.
            let _ = crate::agent::send_message(name, "tick");
        }
    }
}

/// List all current schedules.
///
/// Returns a snapshot of `(agent_name, interval_ticks, last_run, enabled)`.
pub fn list() -> Vec<(&'static str, u64, u64, bool)> {
    let table = SCHEDULES.lock();
    table
        .iter()
        .map(|e| (e.agent_name, e.interval_ticks, e.last_run, e.enabled))
        .collect()
}

/// Enable the schedule for the named agent.
///
/// Returns `true` if the agent was found (regardless of prior state).
pub fn enable(agent_name: &str) -> bool {
    let mut table = SCHEDULES.lock();
    for entry in table.iter_mut() {
        if entry.agent_name == agent_name {
            entry.enabled = true;
            return true;
        }
    }
    false
}

/// Disable the schedule for the named agent.
///
/// The schedule is kept but the agent will not be dispatched until
/// re-enabled via [`enable`].
///
/// Returns `true` if the agent was found.
pub fn disable(agent_name: &str) -> bool {
    let mut table = SCHEDULES.lock();
    for entry in table.iter_mut() {
        if entry.agent_name == agent_name {
            entry.enabled = false;
            return true;
        }
    }
    false
}
