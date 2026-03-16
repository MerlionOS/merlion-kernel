/// Shell job control for MerlionOS.
/// Tracks background and foreground jobs, supports fg/bg/jobs commands,
/// and parses the "&" suffix for background execution.

use alloc::string::String;
use spin::Mutex;
use crate::{serial_println, klog_println, println, task};

/// Maximum number of concurrent jobs tracked by the shell.
const MAX_JOBS: usize = 16;

/// Possible states a job can be in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JobState {
    /// The job is currently executing.
    Running,
    /// The job has been stopped (e.g. via SIGSTOP / Ctrl-Z).
    Stopped,
    /// The job has finished execution.
    Done,
}

/// A single shell job entry.
#[derive(Debug, Clone)]
pub struct Job {
    /// Shell-visible job number (1-based).
    pub id: usize,
    /// Kernel task PID associated with this job.
    pub pid: usize,
    /// The original command string.
    pub command: String,
    /// Current state of the job.
    pub state: JobState,
    /// Whether the job was launched in the background.
    pub background: bool,
}

/// Global job table protected by a spinlock.
static JOB_TABLE: Mutex<JobTable> = Mutex::new(JobTable::new());

/// Table that holds all tracked jobs.
struct JobTable {
    jobs: [Option<Job>; MAX_JOBS],
    next_id: usize,
}

impl JobTable {
    /// Create an empty job table.
    const fn new() -> Self {
        Self {
            jobs: [const { None }; MAX_JOBS],
            next_id: 1,
        }
    }

    /// Find the slot index for a given job id.
    fn find_slot(&self, job_id: usize) -> Option<usize> {
        self.jobs.iter().position(|s| matches!(s, Some(j) if j.id == job_id))
    }

    /// Find the slot index for a given pid.
    fn find_by_pid(&self, pid: usize) -> Option<usize> {
        self.jobs.iter().position(|s| matches!(s, Some(j) if j.pid == pid))
    }
}

/// Register a new job in the table. Returns the assigned job id.
///
/// `pid` is the kernel task PID, `command` is the command string that was
/// executed, and `background` indicates whether it was launched with "&".
pub fn add_job(pid: usize, command: String, background: bool) -> usize {
    let mut table = JOB_TABLE.lock();
    let id = table.next_id;
    table.next_id += 1;

    // Reclaim a Done slot or grab the first empty one
    let slot_idx = table.jobs.iter().position(|s| {
        matches!(s, Some(j) if j.state == JobState::Done) || s.is_none()
    });

    if let Some(idx) = slot_idx {
        table.jobs[idx] = Some(Job {
            id, pid, command: command.clone(), state: JobState::Running, background,
        });
        serial_println!("[jobs] added job {} (pid {}) '{}'", id, pid, command);
        klog_println!("[jobs] added [{}] pid {} — {}", id, pid, command);
    } else {
        serial_println!("[jobs] job table full, cannot track pid {}", pid);
    }
    id
}

/// Bring a job to the foreground by job id.
///
/// If the job is stopped it is resumed first. The shell then yields
/// until the job finishes.
pub fn fg(job_id: usize) -> Result<(), &'static str> {
    {
        let mut table = JOB_TABLE.lock();
        let idx = table.find_slot(job_id).ok_or("no such job")?;
        let job = table.jobs[idx].as_mut().unwrap();
        if job.state == JobState::Done {
            return Err("job already finished");
        }
        job.background = false;
        if job.state == JobState::Stopped {
            job.state = JobState::Running;
            serial_println!("[jobs] resumed job {} in foreground", job_id);
        }
        println!("{}", job.command);
    }
    wait_job(job_id);
    Ok(())
}

/// Resume a stopped job in the background.
pub fn bg(job_id: usize) -> Result<(), &'static str> {
    let mut table = JOB_TABLE.lock();
    let idx = table.find_slot(job_id).ok_or("no such job")?;
    let job = table.jobs[idx].as_mut().unwrap();
    if job.state == JobState::Done {
        return Err("job already finished");
    }
    if job.state == JobState::Running && job.background {
        return Err("job already running in background");
    }
    job.state = JobState::Running;
    job.background = true;
    println!("[{}] {} &", job.id, job.command);
    serial_println!("[jobs] resumed job {} in background", job_id);
    Ok(())
}

/// Display all jobs and their states (like the `jobs` built-in).
pub fn list_jobs() {
    reap_finished();
    let table = JOB_TABLE.lock();
    let mut found = false;
    for slot in table.jobs.iter() {
        if let Some(job) = slot {
            let state_str = match job.state {
                JobState::Running => "Running",
                JobState::Stopped => "Stopped",
                JobState::Done    => "Done",
            };
            let bg = if job.background { " &" } else { "" };
            println!("[{}]  {:8}  pid {}  {}{}", job.id, state_str, job.pid, job.command, bg);
            found = true;
        }
    }
    if !found {
        println!("No jobs.");
    }
}

/// Mark a job as done by its kernel PID.
///
/// Called when the task scheduler detects that a task has exited.
pub fn mark_done(pid: usize) {
    let mut table = JOB_TABLE.lock();
    if let Some(idx) = table.find_by_pid(pid) {
        let job = table.jobs[idx].as_mut().unwrap();
        if job.state != JobState::Done {
            job.state = JobState::Done;
            serial_println!("[jobs] job {} (pid {}) done", job.id, pid);
            klog_println!("[jobs] [{}] done — {}", job.id, job.command);
            if job.background {
                crate::println!("\n[{}]+  Done  {}", job.id, job.command);
            }
        }
    }
}

/// Busy-wait (yielding the CPU) until a job finishes.
pub fn wait_job(job_id: usize) {
    loop {
        reap_finished();
        let table = JOB_TABLE.lock();
        match table.find_slot(job_id) {
            Some(i) if table.jobs[i].as_ref().unwrap().state == JobState::Done => return,
            None => return,
            _ => {}
        }
        drop(table);
        task::yield_now();
    }
}

/// Scan the kernel task list and mark finished tasks as Done.
/// Called periodically or before displaying job status.
pub fn reap_finished() {
    let tasks = task::list();
    let mut table = JOB_TABLE.lock();
    for slot in table.jobs.iter_mut() {
        if let Some(job) = slot {
            if job.state == JobState::Running {
                let alive = tasks.iter().any(|t| t.pid == job.pid);
                if !alive {
                    job.state = JobState::Done;
                    serial_println!("[jobs] reaped job {} (pid {})", job.id, job.pid);
                    if job.background {
                        crate::println!("\n[{}]+  Done  {}", job.id, job.command);
                    }
                }
            }
        }
    }
}

/// Parse a command line for a trailing "&" indicating background execution.
///
/// Returns `(trimmed_command, is_background)`. The ampersand and any
/// surrounding whitespace are stripped from the returned command string.
pub fn parse_background(input: &str) -> (&str, bool) {
    let trimmed = input.trim();
    if trimmed.ends_with('&') {
        (trimmed[..trimmed.len() - 1].trim_end(), true)
    } else {
        (trimmed, false)
    }
}
