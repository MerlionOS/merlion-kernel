/// Userspace process management for MerlionOS.
/// Manages user programs in /bin, provides process environment (argv, envp, cwd),
/// exit codes, wait/waitpid, and process groups.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Maximum number of concurrent user processes.
const MAX_PROCESSES: usize = 64;

/// Signal constants.
pub const SIGKILL: u32 = 9;
pub const SIGTERM: u32 = 15;
pub const SIGCONT: u32 = 18;
pub const SIGSTOP: u32 = 19;

// ---------------------------------------------------------------------------
// Process state
// ---------------------------------------------------------------------------

/// Lifecycle state of a user process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// Newly created, not yet scheduled.
    Created,
    /// Currently eligible for execution.
    Running,
    /// Voluntarily sleeping (e.g. waiting on I/O).
    Sleeping,
    /// Stopped by a signal (SIGSTOP).
    Stopped,
    /// Terminated but not yet reaped by parent.
    Zombie,
    /// Fully exited and reaped.
    Exited,
}

/// Descriptor for a single user process.
#[derive(Debug, Clone)]
pub struct UserProcess {
    pub pid: u32,
    /// Parent process ID.
    pub ppid: u32,
    pub name: String,
    /// Command-line arguments.
    pub argv: Vec<String>,
    /// Environment variables as (key, value) pairs.
    pub envp: Vec<(String, String)>,
    /// Current working directory.
    pub cwd: String,
    pub state: ProcessState,
    /// Exit code set when the process terminates.
    pub exit_code: i32,
    /// User ID.
    pub uid: u32,
    /// Group ID.
    pub gid: u32,
    /// Tick at which the process was spawned.
    pub start_tick: u64,
    /// CPU ticks consumed so far.
    pub cpu_ticks: u64,
    /// Approximate resident memory in bytes.
    pub memory_bytes: usize,
    /// Open file descriptor numbers.
    pub open_fds: Vec<u32>,
    /// Process group ID.
    pub pgid: u32,
    /// PIDs of child processes.
    pub children: Vec<u32>,
    /// umask for file creation.
    pub umask: u32,
}

// ---------------------------------------------------------------------------
// Program registry (/bin)
// ---------------------------------------------------------------------------

/// A program registered under /bin that can be exec'd.
struct Program {
    name: String,
    description: String,
    /// Kernel-mode entry point (simulated userspace).
    entry: fn(),
    /// Approximate binary size in bytes.
    size: usize,
}

/// Registry of programs available in /bin.
static PROGRAMS: Mutex<Vec<Program>> = Mutex::new(Vec::new());

/// Register a new program under /bin.
pub fn register_program(name: &str, desc: &str, entry: fn(), size: usize) {
    let mut progs = PROGRAMS.lock();
    // Avoid duplicates.
    if progs.iter().any(|p| p.name == name) {
        return;
    }
    progs.push(Program {
        name: String::from(name),
        description: String::from(desc),
        entry,
        size,
    });
}

/// List all registered programs as a formatted string.
pub fn list_programs() -> String {
    let progs = PROGRAMS.lock();
    if progs.is_empty() {
        return String::from("(no programs registered)\n");
    }
    let mut out = String::new();
    out.push_str("NAME          SIZE   DESCRIPTION\n");
    out.push_str("------------- ------ ----------------------------------\n");
    for p in progs.iter() {
        out.push_str(&format!("{:<13} {:<6} {}\n", p.name, p.size, p.description));
    }
    out
}

/// Find a program by name and return its entry point.
pub fn find_program(name: &str) -> Option<fn()> {
    let progs = PROGRAMS.lock();
    progs.iter().find(|p| p.name == name).map(|p| p.entry)
}

// ---------------------------------------------------------------------------
// Built-in programs (simulated userspace)
// ---------------------------------------------------------------------------

fn prog_hello()  { crate::serial_println!("Hello, world!"); }
fn prog_echo()   { crate::serial_println!("echo: (no args)"); }
fn prog_cat()    { crate::serial_println!("cat: (no file)"); }
fn prog_ls()     { crate::serial_println!("ls: /"); }
fn prog_sleep()  { /* no-op sleep */ }
fn prog_true()   { /* exits 0 */ }
fn prog_false()  { /* exits 1 */ }

// ---------------------------------------------------------------------------
// Process table
// ---------------------------------------------------------------------------

/// Global process table protected by a spinlock.
static PROCESS_TABLE: Mutex<Vec<UserProcess>> = Mutex::new(Vec::new());

/// Monotonically increasing PID counter.
static NEXT_PID: AtomicU32 = AtomicU32::new(1);

/// Total number of processes ever created.
static TOTAL_CREATED: AtomicU64 = AtomicU64::new(0);

/// Global tick counter used for start_tick (callers should bump this).
static TICK: AtomicU64 = AtomicU64::new(0);

/// Advance the global tick by one (call from timer interrupt).
pub fn tick() {
    TICK.fetch_add(1, Ordering::Relaxed);
}

/// Allocate the next PID.
fn alloc_pid() -> u32 {
    NEXT_PID.fetch_add(1, Ordering::Relaxed)
}

/// Spawn a new process with the given name, arguments, and environment.
/// Returns the PID on success.
pub fn spawn_process(name: &str, argv: &[&str], envp: &[(&str, &str)]) -> Result<u32, &'static str> {
    let mut table = PROCESS_TABLE.lock();
    let alive = table.iter().filter(|p| p.state != ProcessState::Exited).count();
    if alive >= MAX_PROCESSES {
        return Err("process table full");
    }

    let pid = alloc_pid();
    let proc = UserProcess {
        pid,
        ppid: 0,
        name: String::from(name),
        argv: argv.iter().map(|s| String::from(*s)).collect(),
        envp: envp.iter().map(|(k, v)| (String::from(*k), String::from(*v))).collect(),
        cwd: String::from("/"),
        state: ProcessState::Created,
        exit_code: 0,
        uid: 1000,
        gid: 1000,
        start_tick: TICK.load(Ordering::Relaxed),
        cpu_ticks: 0,
        memory_bytes: 4096,
        open_fds: Vec::new(),
        pgid: pid,
        children: Vec::new(),
        umask: 0o022,
    };
    table.push(proc);
    TOTAL_CREATED.fetch_add(1, Ordering::Relaxed);
    Ok(pid)
}

/// Load and execute a program from /bin by path (e.g. "/bin/hello" or just "hello").
/// Returns the PID of the new process.
pub fn exec_program(path: &str, argv: &[&str]) -> Result<u32, &'static str> {
    let name = path.strip_prefix("/bin/").unwrap_or(path);
    let _entry = find_program(name).ok_or("program not found in /bin")?;

    let pid = spawn_process(name, argv, &[])?;

    // Mark the process as running.
    let mut table = PROCESS_TABLE.lock();
    if let Some(p) = table.iter_mut().find(|p| p.pid == pid) {
        p.state = ProcessState::Running;
    }

    // In a real kernel we would jump to `entry`; here we just record it.
    crate::serial_println!("[userland] exec /bin/{} as pid {}", name, pid);
    crate::klog_println!("[userland] exec /bin/{} pid={}", name, pid);
    Ok(pid)
}

/// Terminate a process with the given exit code.
pub fn exit_process(pid: u32, code: i32) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(p) = table.iter_mut().find(|p| p.pid == pid) {
        p.exit_code = code;
        p.state = ProcessState::Zombie;
        crate::serial_println!("[userland] pid {} exited with code {}", pid, code);
    }
}

/// Reap a zombie process and return its exit code, or `None` if the process
/// is still running or does not exist.
pub fn wait_pid(pid: u32) -> Option<i32> {
    let mut table = PROCESS_TABLE.lock();
    if let Some(p) = table.iter_mut().find(|p| p.pid == pid) {
        if p.state == ProcessState::Zombie {
            let code = p.exit_code;
            p.state = ProcessState::Exited;
            // Remove from parent's children list.
            if let Some(parent) = table.iter_mut().find(|pp| pp.children.contains(&pid)) {
                parent.children.retain(|&c| c != pid);
            }
            return Some(code);
        }
    }
    None
}

/// Send a signal to a process.
pub fn kill_process(pid: u32, signal: u32) -> Result<(), &'static str> {
    send_signal(pid, signal)
}

/// Return a clone of the process descriptor, if it exists.
pub fn get_process(pid: u32) -> Option<UserProcess> {
    let table = PROCESS_TABLE.lock();
    table.iter().find(|p| p.pid == pid).cloned()
}

/// Return a formatted process listing (like `ps`).
pub fn list_processes() -> String {
    let table = PROCESS_TABLE.lock();
    if table.is_empty() {
        return String::from("(no processes)\n");
    }
    let mut out = String::new();
    out.push_str("PID  PPID  PGID  STATE    UID   MEM(B)  NAME\n");
    out.push_str("---- ----- ----- -------- ----- ------- ---------------\n");
    for p in table.iter() {
        if p.state == ProcessState::Exited {
            continue;
        }
        let state = match p.state {
            ProcessState::Created  => "CREATED ",
            ProcessState::Running  => "RUNNING ",
            ProcessState::Sleeping => "SLEEPING",
            ProcessState::Stopped  => "STOPPED ",
            ProcessState::Zombie   => "ZOMBIE  ",
            ProcessState::Exited   => "EXITED  ",
        };
        out.push_str(&format!(
            "{:<4} {:<5} {:<5} {} {:<5} {:<7} {}\n",
            p.pid, p.ppid, p.pgid, state, p.uid, p.memory_bytes, p.name,
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Process environment helpers
// ---------------------------------------------------------------------------

/// Set an environment variable for a process.
pub fn set_env(pid: u32, key: &str, value: &str) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(p) = table.iter_mut().find(|p| p.pid == pid) {
        if let Some(entry) = p.envp.iter_mut().find(|(k, _)| k == key) {
            entry.1 = String::from(value);
        } else {
            p.envp.push((String::from(key), String::from(value)));
        }
    }
}

/// Get an environment variable for a process.
pub fn get_env(pid: u32, key: &str) -> Option<String> {
    let table = PROCESS_TABLE.lock();
    table.iter()
        .find(|p| p.pid == pid)
        .and_then(|p| p.envp.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()))
}

/// Set the current working directory for a process.
pub fn set_cwd(pid: u32, path: &str) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(p) = table.iter_mut().find(|p| p.pid == pid) {
        p.cwd = String::from(path);
    }
}

/// Get the current working directory for a process.
pub fn get_cwd(pid: u32) -> String {
    let table = PROCESS_TABLE.lock();
    table.iter()
        .find(|p| p.pid == pid)
        .map(|p| p.cwd.clone())
        .unwrap_or_else(|| String::from("/"))
}

/// Set the umask for a process.
pub fn set_umask(pid: u32, mask: u32) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(p) = table.iter_mut().find(|p| p.pid == pid) {
        p.umask = mask;
    }
}

// ---------------------------------------------------------------------------
// Signal delivery
// ---------------------------------------------------------------------------

/// Deliver a signal to the given process.
pub fn send_signal(pid: u32, signal: u32) -> Result<(), &'static str> {
    let mut table = PROCESS_TABLE.lock();
    let proc = table.iter_mut().find(|p| p.pid == pid)
        .ok_or("process not found")?;

    match signal {
        SIGKILL => {
            proc.exit_code = -9;
            proc.state = ProcessState::Zombie;
            crate::serial_println!("[userland] SIGKILL → pid {}", pid);
        }
        SIGTERM => {
            proc.exit_code = -15;
            proc.state = ProcessState::Zombie;
            crate::serial_println!("[userland] SIGTERM → pid {}", pid);
        }
        SIGSTOP => {
            if proc.state == ProcessState::Running {
                proc.state = ProcessState::Stopped;
                crate::serial_println!("[userland] SIGSTOP → pid {}", pid);
            }
        }
        SIGCONT => {
            if proc.state == ProcessState::Stopped {
                proc.state = ProcessState::Running;
                crate::serial_println!("[userland] SIGCONT → pid {}", pid);
            }
        }
        _ => return Err("unknown signal"),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Process groups
// ---------------------------------------------------------------------------

/// Create a new process group with the given PGID.  The calling process
/// (identified by `leader_pid`) becomes the group leader.
pub fn create_group(leader_pid: u32, pgid: u32) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(p) = table.iter_mut().find(|p| p.pid == leader_pid) {
        p.pgid = pgid;
    }
}

/// Move a process into an existing process group.
pub fn join_group(pid: u32, pgid: u32) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(p) = table.iter_mut().find(|p| p.pid == pid) {
        p.pgid = pgid;
    }
}

/// List all active process groups.
pub fn list_groups() -> String {
    let table = PROCESS_TABLE.lock();
    let mut seen: Vec<u32> = Vec::new();
    let mut out = String::new();
    out.push_str("PGID  MEMBERS\n");
    out.push_str("----- -------\n");
    for p in table.iter() {
        if p.state == ProcessState::Exited {
            continue;
        }
        if !seen.contains(&p.pgid) {
            seen.push(p.pgid);
            let members: Vec<String> = table.iter()
                .filter(|q| q.pgid == p.pgid && q.state != ProcessState::Exited)
                .map(|q| format!("{}", q.pid))
                .collect();
            out.push_str(&format!("{:<5} {}\n", p.pgid, members.join(", ")));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// Return a summary of process table statistics.
pub fn process_stats() -> String {
    let table = PROCESS_TABLE.lock();
    let total = TOTAL_CREATED.load(Ordering::Relaxed);
    let running = table.iter().filter(|p| p.state == ProcessState::Running).count();
    let zombies = table.iter().filter(|p| p.state == ProcessState::Zombie).count();
    let sleeping = table.iter().filter(|p| p.state == ProcessState::Sleeping).count();
    let stopped = table.iter().filter(|p| p.state == ProcessState::Stopped).count();
    format!(
        "Process statistics:\n  Total created: {}\n  Running: {}\n  Sleeping: {}\n  Stopped: {}\n  Zombie: {}\n  Next PID: {}\n",
        total, running, sleeping, stopped, zombies,
        NEXT_PID.load(Ordering::Relaxed),
    )
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the userland subsystem: register built-in programs and create
/// the init process (PID 1).
pub fn init() {
    // Register built-in programs under /bin.
    register_program("hello", "Print hello world", prog_hello, 64);
    register_program("echo",  "Echo arguments to stdout", prog_echo, 128);
    register_program("cat",   "Concatenate and display files", prog_cat, 256);
    register_program("ls",    "List directory contents", prog_ls, 256);
    register_program("sleep", "Sleep for a duration", prog_sleep, 64);
    register_program("true",  "Exit with status 0", prog_true, 32);
    register_program("false", "Exit with status 1", prog_false, 32);

    // Create the init process (PID 1).
    let pid = spawn_process("init", &["init"], &[
        ("PATH", "/bin"),
        ("HOME", "/"),
        ("TERM", "merlion"),
    ]).expect("failed to create init process");

    // Mark init as running.
    {
        let mut table = PROCESS_TABLE.lock();
        if let Some(p) = table.iter_mut().find(|p| p.pid == pid) {
            p.ppid = 0;
            p.uid = 0;
            p.gid = 0;
            p.state = ProcessState::Running;
        }
    }

    crate::serial_println!("[userland] initialised — {} programs, init pid={}", 7, pid);
    crate::klog_println!("[userland] subsystem ready");
}
