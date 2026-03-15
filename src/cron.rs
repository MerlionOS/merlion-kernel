/// Cron-like job scheduler for MerlionOS.
///
/// Supports standard five-field cron expressions (`minute hour day month weekday`)
/// with wildcards (`*`), lists (`1,3,5`), ranges (`1-5`), and step values (`*/10`).
///
/// Jobs are stored in a global [`CronDaemon`] protected by a [`spin::Mutex`].
/// The [`tick`] function should be called from the timer interrupt (or a periodic
/// kernel task) to check whether any jobs are due and dispatch them via
/// [`crate::shell::dispatch`].

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use crate::{rtc, serial_println};

/// A single field in a cron expression, representing which values match.
#[derive(Clone, Debug)]
pub struct CronField {
    /// Bit-set of allowed values.  Index 0 = value 0, etc.
    bits: [u64; 2], // covers 0..=127, more than enough for all fields
}

impl CronField {
    /// Create a field that matches **every** value in the given range.
    fn all(min: u8, max: u8) -> Self {
        let mut f = Self { bits: [0; 2] };
        for v in min..=max {
            f.set(v);
        }
        f
    }

    fn set(&mut self, v: u8) {
        let idx = (v / 64) as usize;
        self.bits[idx] |= 1u64 << (v % 64);
    }

    /// Returns `true` if the value is contained in this field.
    pub fn matches(&self, v: u8) -> bool {
        let idx = (v / 64) as usize;
        self.bits[idx] & (1u64 << (v % 64)) != 0
    }
}

/// A fully parsed five-field cron schedule.
#[derive(Clone, Debug)]
pub struct CronSchedule {
    /// Minute (0-59).
    pub minute: CronField,
    /// Hour (0-23).
    pub hour: CronField,
    /// Day of month (1-31).
    pub day: CronField,
    /// Month (1-12).
    pub month: CronField,
    /// Day of week (0-6, 0 = Sunday).
    pub weekday: CronField,
    /// The original pattern string for display purposes.
    pub pattern: String,
}

impl CronSchedule {
    /// Returns `true` when the given [`rtc::DateTime`] matches this schedule.
    pub fn matches(&self, dt: &rtc::DateTime) -> bool {
        self.minute.matches(dt.minute)
            && self.hour.matches(dt.hour)
            && self.day.matches(dt.day)
            && self.month.matches(dt.month)
            && self.weekday.matches(day_of_week(dt.year, dt.month, dt.day))
    }
}

/// Parse a single cron sub-expression (one comma-separated token).
///
/// Supported forms:
/// - `*`     — every value in `min..=max`
/// - `N`     — exact value
/// - `N-M`   — inclusive range
/// - `*/S`   — every S-th value starting at `min`
/// - `N-M/S` — every S-th value in range N..=M
fn parse_cron_token(token: &str, min: u8, max: u8) -> Result<CronField, &'static str> {
    // Handle step expressions: base/step
    if let Some(slash_pos) = token.find('/') {
        let base = &token[..slash_pos];
        let step: u8 = token[slash_pos + 1..]
            .parse()
            .map_err(|_| "invalid step value")?;
        if step == 0 {
            return Err("step must be > 0");
        }

        let (range_min, range_max) = if base == "*" {
            (min, max)
        } else if let Some(dash) = base.find('-') {
            let lo: u8 = base[..dash].parse().map_err(|_| "invalid range start")?;
            let hi: u8 = base[dash + 1..].parse().map_err(|_| "invalid range end")?;
            (lo, hi)
        } else {
            let start: u8 = base.parse().map_err(|_| "invalid base value")?;
            (start, max)
        };

        let mut f = CronField { bits: [0; 2] };
        let mut v = range_min;
        while v <= range_max {
            f.set(v);
            v = v.saturating_add(step);
        }
        return Ok(f);
    }

    // Range: N-M
    if let Some(dash) = token.find('-') {
        let lo: u8 = token[..dash].parse().map_err(|_| "invalid range start")?;
        let hi: u8 = token[dash + 1..].parse().map_err(|_| "invalid range end")?;
        let mut f = CronField { bits: [0; 2] };
        for v in lo..=hi {
            f.set(v);
        }
        return Ok(f);
    }

    // Wildcard
    if token == "*" {
        return Ok(CronField::all(min, max));
    }

    // Exact value
    let val: u8 = token.parse().map_err(|_| "invalid number")?;
    let mut f = CronField { bits: [0; 2] };
    f.set(val);
    Ok(f)
}

/// Parse a single cron field that may contain comma-separated values.
///
/// Example: `"1,5,10-15,*/2"`
pub fn parse_cron_expr(field: &str, min: u8, max: u8) -> Result<CronField, &'static str> {
    let mut combined = CronField { bits: [0; 2] };
    for token in field.split(',') {
        let part = parse_cron_token(token.trim(), min, max)?;
        combined.bits[0] |= part.bits[0];
        combined.bits[1] |= part.bits[1];
    }
    Ok(combined)
}

/// Parse a full `"* * * * *"` cron pattern into a [`CronSchedule`].
pub fn parse_schedule(pattern: &str) -> Result<CronSchedule, &'static str> {
    let fields: Vec<&str> = pattern.split_whitespace().collect();
    if fields.len() != 5 {
        return Err("cron pattern must have exactly 5 fields");
    }

    Ok(CronSchedule {
        minute: parse_cron_expr(fields[0], 0, 59)?,
        hour: parse_cron_expr(fields[1], 0, 23)?,
        day: parse_cron_expr(fields[2], 1, 31)?,
        month: parse_cron_expr(fields[3], 1, 12)?,
        weekday: parse_cron_expr(fields[4], 0, 6)?,
        pattern: String::from(pattern),
    })
}

/// Returns the day of week for a given date: 0 = Sunday, 1 = Monday, ..., 6 = Saturday.
fn day_of_week(year: u16, month: u8, day: u8) -> u8 {
    static OFFSETS: [i8; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut y = year as i32;
    if month < 3 {
        y -= 1;
    }
    let m = month as i32;
    let d = day as i32;
    ((y + y / 4 - y / 100 + y / 400 + OFFSETS[(m - 1) as usize] as i32 + d) % 7) as u8
}

/// A single scheduled job.
pub struct CronEntry {
    /// Unique job identifier.
    pub id: u64,
    /// Parsed schedule.
    pub schedule: CronSchedule,
    /// Shell command to execute when the job fires.
    pub command: String,
    /// Whether the job is active.
    pub enabled: bool,
    /// Last time (as an RTC snapshot) the job actually ran.
    pub last_run: Option<rtc::DateTime>,
    /// Next expected fire time (informational; matching is recomputed each tick).
    pub next_run: Option<rtc::DateTime>,
}

/// The global cron daemon state.
pub struct CronDaemon {
    jobs: Vec<CronEntry>,
    next_id: u64,
    /// Tracks the last minute we evaluated so we fire at most once per minute.
    last_tick_minute: Option<u8>,
}

impl CronDaemon {
    /// Create an empty daemon.
    const fn new() -> Self {
        Self {
            jobs: Vec::new(),
            next_id: 1,
            last_tick_minute: None,
        }
    }
}

/// Global cron daemon instance.
static DAEMON: Mutex<CronDaemon> = Mutex::new(CronDaemon::new());

/// Register a new cron job and return its unique id.
///
/// `schedule` is a standard five-field cron pattern (e.g. `"*/5 * * * *"`).
/// `command` is the shell command dispatched when the job fires.
pub fn add_job(schedule: &str, command: &str) -> Result<u64, &'static str> {
    let parsed = parse_schedule(schedule)?;
    let mut daemon = DAEMON.lock();
    let id = daemon.next_id;
    daemon.next_id += 1;

    daemon.jobs.push(CronEntry {
        id,
        schedule: parsed,
        command: String::from(command),
        enabled: true,
        last_run: None,
        next_run: None,
    });

    serial_println!("cron: added job {} '{}' schedule '{}'", id, command, schedule);
    Ok(id)
}

/// Remove a job by its id.  Returns `true` if the job existed.
pub fn remove_job(id: u64) -> bool {
    let mut daemon = DAEMON.lock();
    let before = daemon.jobs.len();
    daemon.jobs.retain(|j| j.id != id);
    let removed = daemon.jobs.len() < before;
    if removed {
        serial_println!("cron: removed job {}", id);
    }
    removed
}

/// Return a snapshot of all registered jobs (id, pattern, command, enabled).
pub fn list_jobs() -> Vec<(u64, String, String, bool)> {
    let daemon = DAEMON.lock();
    daemon
        .jobs
        .iter()
        .map(|j| {
            (
                j.id,
                j.schedule.pattern.clone(),
                j.command.clone(),
                j.enabled,
            )
        })
        .collect()
}

/// Check all jobs against the current RTC time and run any that are due.
///
/// This should be called periodically (e.g. every PIT tick or every second).
/// It reads the RTC once, and skips evaluation if the minute has not changed
/// since the last check — cron resolution is one minute.
pub fn tick() {
    let now = rtc::read();

    // Only evaluate once per minute to avoid duplicate firings.
    {
        let daemon = DAEMON.lock();
        if let Some(last) = daemon.last_tick_minute {
            if last == now.minute {
                return;
            }
        }
    }

    // Collect commands to run while holding the lock, then release before dispatch.
    let mut to_run: Vec<String> = Vec::new();

    {
        let mut daemon = DAEMON.lock();
        daemon.last_tick_minute = Some(now.minute);

        for job in daemon.jobs.iter_mut() {
            if !job.enabled {
                continue;
            }
            if job.schedule.matches(&now) {
                serial_println!(
                    "cron: firing job {} cmd='{}' at {}",
                    job.id,
                    job.command,
                    now
                );
                job.last_run = Some(now);
                to_run.push(job.command.clone());
            }
        }
    }

    // Dispatch outside the lock to avoid deadlocks with shell/serial.
    for cmd in &to_run {
        run_job(cmd);
    }
}

/// Dispatch a cron command through the kernel shell.
fn run_job(command: &str) {
    crate::shell::dispatch(command);
}
