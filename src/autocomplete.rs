/// Enhanced shell autocomplete for MerlionOS.
/// Context-aware completion with command arguments, file paths,
/// variable names, history search, and fuzzy matching.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use alloc::borrow::ToOwned;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of history entries to search.
const MAX_HISTORY: usize = 256;

/// Maximum Levenshtein distance for fuzzy matching.
const MAX_FUZZY_DISTANCE: usize = 3;

/// Maximum number of custom completion registrations.
const MAX_CUSTOM_COMPLETIONS: usize = 64;

/// Columns to use when formatting a completion menu.
const MENU_COLUMNS: usize = 4;

/// Maximum column width in the completion menu.
const MENU_COL_WIDTH: usize = 20;

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

static COMPLETIONS_OFFERED: AtomicU64 = AtomicU64::new(0);
static COMPLETIONS_ACCEPTED: AtomicU64 = AtomicU64::new(0);
static FUZZY_CORRECTIONS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The kind of completion being offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    /// A shell command name.
    Command,
    /// A file or directory path.
    FilePath,
    /// An environment variable ($VAR).
    Variable,
    /// A username (~user).
    Username,
    /// A hostname.
    Hostname,
    /// A process ID.
    Pid,
    /// A kernel module name.
    Module,
    /// A history entry.
    History,
    /// A command argument hint.
    Argument,
    /// A fuzzy-corrected suggestion.
    Fuzzy,
}

/// A single completion candidate.
#[derive(Debug, Clone)]
pub struct Completion {
    /// The replacement text.
    pub text: String,
    /// What kind of completion this is.
    pub kind: CompletionKind,
    /// Optional brief description or hint.
    pub description: String,
    /// Fuzzy distance (0 = exact match).
    pub distance: usize,
}

/// Context of what is being completed in the input line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionContext {
    /// Completing a command name (first word).
    CommandPosition,
    /// Completing an argument to a known command.
    ArgumentPosition,
    /// Completing a file path.
    PathPosition,
    /// Completing a variable name (after $).
    VariablePosition,
}

/// Argument hint type for per-command completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgHint {
    /// Suggest file paths.
    FilePath,
    /// Suggest process IDs.
    Pid,
    /// Suggest module names.
    ModuleName,
    /// Suggest hostnames.
    Hostname,
    /// Suggest variable names.
    Variable,
    /// Suggest specific string values.
    OneOf,
    /// No specific hint.
    None,
}

/// A custom completion registration for a specific command.
struct CustomCompletion {
    command: String,
    hint: ArgHint,
    values: Vec<String>,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

struct AutocompleteState {
    history: Vec<String>,
    custom: Vec<CustomCompletion>,
    initialized: bool,
}

static STATE: Mutex<AutocompleteState> = Mutex::new(AutocompleteState {
    history: Vec::new(),
    custom: Vec::new(),
    initialized: false,
});

/// Command-to-argument-hint mapping for built-in commands.
static CMD_ARG_HINTS: &[(&str, ArgHint)] = &[
    ("cat", ArgHint::FilePath),
    ("edit", ArgHint::FilePath),
    ("head", ArgHint::FilePath),
    ("tail", ArgHint::FilePath),
    ("rm", ArgHint::FilePath),
    ("hexdump", ArgHint::FilePath),
    ("wc", ArgHint::FilePath),
    ("grep", ArgHint::FilePath),
    ("sort", ArgHint::FilePath),
    ("ls", ArgHint::FilePath),
    ("kill", ArgHint::Pid),
    ("signal", ArgHint::Pid),
    ("modprobe", ArgHint::ModuleName),
    ("rmmod", ArgHint::ModuleName),
    ("ping", ArgHint::Hostname),
    ("wget", ArgHint::Hostname),
    ("dns", ArgHint::Hostname),
    ("set", ArgHint::Variable),
    ("unset", ArgHint::Variable),
    ("env", ArgHint::Variable),
];

/// Brief man-page style descriptions for common commands.
static CMD_DESCRIPTIONS: &[(&str, &str)] = &[
    ("cat", "display file contents"),
    ("cd", "change directory"),
    ("clear", "clear the screen"),
    ("date", "show current date and time"),
    ("dmesg", "display kernel log"),
    ("echo", "print text"),
    ("edit", "open file in editor"),
    ("env", "show environment variables"),
    ("free", "display memory usage"),
    ("grep", "search text patterns"),
    ("head", "show first lines of file"),
    ("help", "list available commands"),
    ("hexdump", "hex dump of file contents"),
    ("history", "show command history"),
    ("hostname", "show or set hostname"),
    ("ifconfig", "network interface config"),
    ("kill", "terminate a process"),
    ("ls", "list directory contents"),
    ("lsmod", "list loaded modules"),
    ("lspci", "list PCI devices"),
    ("man", "display manual page"),
    ("modprobe", "load a kernel module"),
    ("neofetch", "system information"),
    ("netstat", "network statistics"),
    ("ping", "send ICMP echo request"),
    ("ps", "list running processes"),
    ("reboot", "restart the system"),
    ("rm", "remove a file"),
    ("rmmod", "unload a kernel module"),
    ("set", "set an environment variable"),
    ("shutdown", "power off the system"),
    ("sort", "sort file lines"),
    ("tail", "show last lines of file"),
    ("top", "process monitor"),
    ("uname", "system information"),
    ("unset", "remove environment variable"),
    ("uptime", "show system uptime"),
    ("watch", "run command repeatedly"),
    ("wc", "count lines/words/bytes"),
    ("wget", "download from URL"),
    ("whoami", "show current user"),
];

// ---------------------------------------------------------------------------
// Fuzzy matching — Levenshtein distance
// ---------------------------------------------------------------------------

/// Compute the Levenshtein edit distance between two strings.
/// Uses a single-row DP approach to avoid allocating a full matrix.
/// Returns `usize::MAX` if either string is empty.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    let a_len = a_bytes.len();
    let b_len = b_bytes.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    // Single-row DP: row[j] = distance(a[..i], b[..j])
    let mut row: Vec<usize> = (0..=b_len).collect();

    for i in 1..=a_len {
        let mut prev = row[0];
        row[0] = i;
        for j in 1..=b_len {
            let cost = if a_bytes[i - 1] == b_bytes[j - 1] { 0 } else { 1 };
            let val = min3(
                row[j] + 1,          // deletion
                row[j - 1] + 1,      // insertion
                prev + cost,          // substitution
            );
            prev = row[j];
            row[j] = val;
        }
    }

    row[b_len]
}

#[inline]
fn min3(a: usize, b: usize, c: usize) -> usize {
    let m = if a < b { a } else { b };
    if m < c { m } else { c }
}

/// Find fuzzy matches for `input` among `candidates`, returning those
/// within `MAX_FUZZY_DISTANCE`. Results are sorted by distance.
pub fn fuzzy_match<'a>(input: &str, candidates: &[&'a str]) -> Vec<(&'a str, usize)> {
    let mut matches: Vec<(&str, usize)> = Vec::new();
    for &cand in candidates {
        let d = levenshtein(input, cand);
        if d > 0 && d <= MAX_FUZZY_DISTANCE {
            matches.push((cand, d));
        }
    }
    // Sort by distance (insertion sort for small sets).
    for i in 1..matches.len() {
        let mut j = i;
        while j > 0 && matches[j].1 < matches[j - 1].1 {
            matches.swap(j, j - 1);
            j -= 1;
        }
    }
    matches
}

// ---------------------------------------------------------------------------
// Context detection
// ---------------------------------------------------------------------------

/// Detect what kind of completion the user needs based on cursor position.
fn detect_context(input: &str, cursor_pos: usize) -> (CompletionContext, String, String) {
    let before_cursor = if cursor_pos <= input.len() {
        &input[..cursor_pos]
    } else {
        input
    };

    let trimmed = before_cursor.trim_start();

    // Check for variable completion.
    if let Some(dollar_pos) = trimmed.rfind('$') {
        let after_dollar = &trimmed[dollar_pos + 1..];
        if !after_dollar.contains(' ') {
            return (
                CompletionContext::VariablePosition,
                String::new(),
                String::from(after_dollar),
            );
        }
    }

    // Split into words.
    let parts: Vec<&str> = trimmed.split_whitespace().collect();

    if parts.is_empty() {
        return (CompletionContext::CommandPosition, String::new(), String::new());
    }

    if parts.len() == 1 && before_cursor.ends_with(|c: char| !c.is_whitespace()) {
        // Still typing the command name.
        return (
            CompletionContext::CommandPosition,
            String::new(),
            String::from(parts[0]),
        );
    }

    // We have a command and are in argument position.
    let cmd = String::from(parts[0]);
    let partial_arg = if before_cursor.ends_with(' ') {
        String::new()
    } else {
        String::from(*parts.last().unwrap_or(&""))
    };

    // If the partial arg looks like a path (contains /), use path context.
    if partial_arg.contains('/') {
        return (CompletionContext::PathPosition, cmd, partial_arg);
    }

    (CompletionContext::ArgumentPosition, cmd, partial_arg)
}

/// Look up the argument hint for a given command.
fn arg_hint_for(cmd: &str) -> ArgHint {
    // Check built-in hints.
    for &(c, hint) in CMD_ARG_HINTS {
        if c == cmd {
            return hint;
        }
    }
    // Check custom registrations.
    let state = STATE.lock();
    for reg in &state.custom {
        if reg.command == cmd {
            return reg.hint;
        }
    }
    ArgHint::None
}

/// Look up a brief description for a command.
fn cmd_description(cmd: &str) -> &'static str {
    for &(c, desc) in CMD_DESCRIPTIONS {
        if c == cmd {
            return desc;
        }
    }
    ""
}

// ---------------------------------------------------------------------------
// Completion sources
// ---------------------------------------------------------------------------

/// Complete command names using the basic completion module.
fn complete_commands(partial: &str) -> Vec<Completion> {
    let matches = crate::completion::complete(partial);
    matches
        .iter()
        .map(|&name| {
            let desc = cmd_description(name);
            Completion {
                text: String::from(name),
                kind: CompletionKind::Command,
                description: String::from(desc),
                distance: 0,
            }
        })
        .collect()
}

/// Complete file paths using VFS.
fn complete_paths(partial: &str) -> Vec<Completion> {
    let paths = crate::completion::complete_path(partial);
    paths
        .into_iter()
        .map(|p| Completion {
            text: p,
            kind: CompletionKind::FilePath,
            description: String::new(),
            distance: 0,
        })
        .collect()
}

/// Complete environment variable names.
fn complete_variables(partial: &str) -> Vec<Completion> {
    let vars = crate::env::list();
    vars.into_iter()
        .filter(|(name, _)| name.starts_with(partial))
        .map(|(name, val)| Completion {
            text: name.clone(),
            kind: CompletionKind::Variable,
            description: format!("={}", val),
            distance: 0,
        })
        .collect()
}

/// Complete process IDs from the task list.
fn complete_pids(partial: &str) -> Vec<Completion> {
    let mut results = Vec::new();
    let tasks = crate::task::list();
    for t in tasks {
        let (pid, name) = (t.pid, String::from(t.name));
        let pid_str = format!("{}", pid);
        if pid_str.starts_with(partial) || partial.is_empty() {
            results.push(Completion {
                text: pid_str,
                kind: CompletionKind::Pid,
                description: name,
                distance: 0,
            });
        }
    }
    results
}

/// Complete kernel module names.
fn complete_modules(partial: &str) -> Vec<Completion> {
    let mods: Vec<String> = crate::module::list().into_iter().map(|m| m.name).collect();
    mods.into_iter()
        .filter(|name| name.starts_with(partial))
        .map(|name| Completion {
            text: name,
            kind: CompletionKind::Module,
            description: String::from("kernel module"),
            distance: 0,
        })
        .collect()
}

/// Search command history for entries matching `partial`.
fn complete_history(partial: &str) -> Vec<Completion> {
    let state = STATE.lock();
    let mut results = Vec::new();
    for entry in state.history.iter().rev() {
        if entry.starts_with(partial) || entry.contains(partial) {
            results.push(Completion {
                text: entry.clone(),
                kind: CompletionKind::History,
                description: String::from("history"),
                distance: 0,
            });
            if results.len() >= 10 {
                break;
            }
        }
    }
    results
}

// ---------------------------------------------------------------------------
// History management
// ---------------------------------------------------------------------------

/// Add a command to the history buffer.
pub fn add_history(cmd: &str) {
    if cmd.is_empty() {
        return;
    }
    let mut state = STATE.lock();
    // Avoid duplicate consecutive entries.
    if let Some(last) = state.history.last() {
        if last == cmd {
            return;
        }
    }
    if state.history.len() >= MAX_HISTORY {
        state.history.remove(0);
    }
    state.history.push(String::from(cmd));
}

/// Reverse-search history for a pattern (Ctrl+R style).
/// Returns matching entries most-recent first.
pub fn reverse_search(pattern: &str) -> Vec<String> {
    let state = STATE.lock();
    let mut results = Vec::new();
    for entry in state.history.iter().rev() {
        if entry.contains(pattern) {
            results.push(entry.clone());
            if results.len() >= 10 {
                break;
            }
        }
    }
    results
}

// ---------------------------------------------------------------------------
// Custom completion registration
// ---------------------------------------------------------------------------

/// Register a custom completion for a specific command.
pub fn register_completions(cmd: &str, hint: ArgHint, values: Vec<String>) {
    let mut state = STATE.lock();
    if state.custom.len() >= MAX_CUSTOM_COMPLETIONS {
        return;
    }
    // Update existing registration if present.
    for reg in &mut state.custom {
        if reg.command == cmd {
            reg.hint = hint;
            reg.values = values;
            return;
        }
    }
    state.custom.push(CustomCompletion {
        command: String::from(cmd),
        hint,
        values,
    });
}

/// Get custom completion values for a command.
fn custom_values(cmd: &str, partial: &str) -> Vec<Completion> {
    let state = STATE.lock();
    for reg in &state.custom {
        if reg.command == cmd {
            return reg
                .values
                .iter()
                .filter(|v| v.starts_with(partial))
                .map(|v| Completion {
                    text: v.clone(),
                    kind: CompletionKind::Argument,
                    description: String::new(),
                    distance: 0,
                })
                .collect();
        }
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// Completion menu formatting
// ---------------------------------------------------------------------------

/// Format completion matches into a multi-column display string.
pub fn format_menu(completions: &[Completion]) -> String {
    if completions.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let count = completions.len();
    let cols = MENU_COLUMNS;
    let rows = (count + cols - 1) / cols;

    for row in 0..rows {
        for col in 0..cols {
            let idx = row + col * rows;
            if idx >= count {
                break;
            }
            let entry = &completions[idx];
            let label = if entry.description.is_empty() {
                entry.text.clone()
            } else {
                format!("{} ({})", entry.text, entry.description)
            };
            // Pad to column width.
            let padded = if label.len() < MENU_COL_WIDTH {
                let mut s = label;
                while s.len() < MENU_COL_WIDTH {
                    s.push(' ');
                }
                s
            } else {
                let mut s = String::new();
                for (i, c) in label.chars().enumerate() {
                    if i >= MENU_COL_WIDTH - 1 {
                        break;
                    }
                    s.push(c);
                }
                s.push(' ');
                s
            };
            out.push_str(&padded);
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Main completion API
// ---------------------------------------------------------------------------

/// Perform context-aware completion on the input at the given cursor position.
/// Returns a list of completion candidates sorted by relevance.
pub fn complete(input: &str, cursor_pos: usize) -> Vec<Completion> {
    let (context, cmd, partial) = detect_context(input, cursor_pos);
    #[allow(unused_assignments)]
    let mut results = Vec::new();

    match context {
        CompletionContext::CommandPosition => {
            results = complete_commands(&partial);
            // If no exact prefix matches, try fuzzy.
            if results.is_empty() && !partial.is_empty() {
                let cmds = crate::completion::complete("");
                let fuzzy = fuzzy_match(&partial, &cmds);
                for (name, dist) in fuzzy {
                    FUZZY_CORRECTIONS.fetch_add(1, Ordering::Relaxed);
                    results.push(Completion {
                        text: String::from(name),
                        kind: CompletionKind::Fuzzy,
                        description: format!("did you mean? (dist={})", dist),
                        distance: dist,
                    });
                }
            }
        }
        CompletionContext::ArgumentPosition => {
            let hint = arg_hint_for(&cmd);
            match hint {
                ArgHint::FilePath => {
                    results = complete_paths(&partial);
                }
                ArgHint::Pid => {
                    results = complete_pids(&partial);
                }
                ArgHint::ModuleName => {
                    results = complete_modules(&partial);
                }
                ArgHint::Variable => {
                    results = complete_variables(&partial);
                }
                ArgHint::Hostname | ArgHint::None => {
                    // Try custom completions first, then fall back to paths.
                    results = custom_values(&cmd, &partial);
                    if results.is_empty() {
                        results = complete_paths(&partial);
                    }
                }
                ArgHint::OneOf => {
                    results = custom_values(&cmd, &partial);
                }
            }
        }
        CompletionContext::PathPosition => {
            results = complete_paths(&partial);
        }
        CompletionContext::VariablePosition => {
            results = complete_variables(&partial);
        }
    }

    COMPLETIONS_OFFERED.fetch_add(results.len() as u64, Ordering::Relaxed);
    results
}

/// Notify the system that a completion was accepted by the user.
pub fn accept_completion() {
    COMPLETIONS_ACCEPTED.fetch_add(1, Ordering::Relaxed);
}

/// Get the man-page hint for a command (brief description).
pub fn man_hint(cmd: &str) -> Option<&'static str> {
    let desc = cmd_description(cmd);
    if desc.is_empty() {
        None
    } else {
        Some(desc)
    }
}

/// Return autocomplete statistics.
pub fn stats() -> (u64, u64, u64) {
    (
        COMPLETIONS_OFFERED.load(Ordering::Relaxed),
        COMPLETIONS_ACCEPTED.load(Ordering::Relaxed),
        FUZZY_CORRECTIONS.load(Ordering::Relaxed),
    )
}

/// Format statistics as a human-readable string.
pub fn format_stats() -> String {
    let (offered, accepted, fuzzy) = stats();
    format!(
        "Autocomplete stats:\n  Offered:   {}\n  Accepted:  {}\n  Fuzzy:     {}\n",
        offered, accepted, fuzzy
    )
}

/// Initialize the autocomplete subsystem.
pub fn init() {
    let mut state = STATE.lock();
    if state.initialized {
        return;
    }

    // Register some default custom completions.
    state.custom.push(CustomCompletion {
        command: "chmod".to_owned(),
        hint: ArgHint::OneOf,
        values: Vec::from([
            String::from("644"),
            String::from("755"),
            String::from("700"),
            String::from("600"),
            String::from("777"),
            String::from("400"),
        ]),
    });

    state.custom.push(CustomCompletion {
        command: "config".to_owned(),
        hint: ArgHint::OneOf,
        values: Vec::from([
            String::from("get"),
            String::from("set"),
            String::from("list"),
            String::from("reset"),
        ]),
    });

    state.initialized = true;
    crate::klog_println!("[autocomplete] initialized");
}
