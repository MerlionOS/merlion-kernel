/// Dotfile and profile system for MerlionOS.
///
/// Reads and applies shell configuration from `/etc/profile`, `~/.profile`,
/// and `/etc/merlionrc`.  Supports environment exports, alias definitions,
/// PATH manipulation, PS1 prompt customisation, and arbitrary startup
/// commands.  Also provides persistent command-history via the VFS.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;

use crate::{vfs, env, shell, println};

/// Maximum number of lines we will process from a single profile file.
const MAX_PROFILE_LINES: usize = 64;

/// Default history file location inside the VFS.
const HISTORY_PATH: &str = "/tmp/.msh_history";

/// Maximum number of history entries persisted to disk.
const MAX_HISTORY_ENTRIES: usize = 128;

// ── helpers ──────────────────────────────────────────────────────────

/// Read a VFS file and return its contents, or an empty string on error.
fn read_file(path: &str) -> String {
    vfs::cat(path).unwrap_or_default()
}

/// Split file contents into individual lines, honouring both `\n` and `\r\n`.
fn lines(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|l| l.trim_end_matches('\r'))
        .collect()
}

/// Execute a single configuration line.
///
/// Recognised directives:
/// - `export VAR=value`  — sets an environment variable
/// - `alias name='cmd'`  — defines a shell alias
/// - `PS1=...`           — shorthand for prompt customisation
/// - `PATH=...`          — sets the PATH variable directly
/// - Lines starting with `#` are comments (ignored)
/// - Empty / whitespace-only lines are ignored
/// - Everything else is forwarded to `shell::dispatch` as a startup command
fn execute_line(line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return;
    }

    // export VAR=value
    if let Some(rest) = trimmed.strip_prefix("export ") {
        if let Some((key, value)) = rest.split_once('=') {
            let key = key.trim();
            let value = strip_quotes(value.trim());
            env::set(key, &value);
            return;
        }
    }

    // alias name='command' or alias name="command"
    if let Some(rest) = trimmed.strip_prefix("alias ") {
        if let Some((name, cmd)) = rest.split_once('=') {
            let name = name.trim();
            let cmd = strip_quotes(cmd.trim());
            env::set_alias(name, &cmd);
            return;
        }
    }

    // PS1=value  (prompt shorthand, no `export` prefix required)
    if let Some(value) = trimmed.strip_prefix("PS1=") {
        let value = strip_quotes(value.trim());
        env::set("PS1", &value);
        return;
    }

    // PATH=value
    if let Some(value) = trimmed.strip_prefix("PATH=") {
        let value = strip_quotes(value.trim());
        env::set("PATH", &value);
        return;
    }

    // Fallback: treat the line as a shell command to run at startup.
    shell::dispatch(trimmed);
}

/// Strip matching single or double quotes from around a value.
fn strip_quotes(s: &str) -> String {
    if s.len() >= 2 {
        if (s.starts_with('"') && s.ends_with('"'))
            || (s.starts_with('\'') && s.ends_with('\''))
        {
            return s[1..s.len() - 1].to_owned();
        }
    }
    s.to_owned()
}

/// Process every recognised line in `text`, up to `MAX_PROFILE_LINES`.
fn source_text(text: &str) {
    for (i, line) in lines(text).iter().enumerate() {
        if i >= MAX_PROFILE_LINES {
            break;
        }
        execute_line(line);
    }
}

// ── public API ───────────────────────────────────────────────────────

/// Load and execute the system-wide profile (`/etc/profile`) followed by
/// the user profile (`~/.profile`).
///
/// Called once during shell initialisation.  Each file is read from the
/// VFS and its directives are applied in order: exports, aliases, PATH
/// changes, PS1 overrides, and startup commands.
pub fn load_profile() {
    // System-wide profile
    let sys = read_file("/etc/profile");
    if !sys.is_empty() {
        source_text(&sys);
    }

    // User profile — resolve HOME to find ~/.profile
    let home = env::get("HOME").unwrap_or_else(|| "/tmp".to_owned());
    let user_profile = format!("{}/.profile", home);
    let usr = read_file(&user_profile);
    if !usr.is_empty() {
        source_text(&usr);
    }
}

/// Persist a set of shell commands into the user's `~/.profile`.
///
/// Overwrites any previous content.  Each entry in `commands` becomes one
/// line in the file (e.g. `export EDITOR=ed`, `alias ll='ls -l'`).
pub fn save_profile(commands: &[&str]) {
    let home = env::get("HOME").unwrap_or_else(|| "/tmp".to_owned());
    let path = format!("{}/.profile", home);
    let body = commands.join("\n");
    if let Err(e) = vfs::write(&path, &body) {
        println!("dotfiles: failed to save profile: {}", e);
    }
}

/// Load and execute the system-wide rc file (`/etc/merlionrc`).
///
/// This is intended to run after `load_profile()` and provides a
/// MerlionOS-specific configuration hook (analogous to `/etc/bashrc`).
pub fn load_rc() {
    let rc = read_file("/etc/merlionrc");
    if !rc.is_empty() {
        source_text(&rc);
    }
}

/// Load saved command history from the VFS into memory.
///
/// Reads `HISTORY_PATH` and returns each non-empty line as a history
/// entry.  The caller (typically the shell) can push these into its
/// internal history ring.
pub fn load_history() -> Vec<String> {
    let text = read_file(HISTORY_PATH);
    if text.is_empty() {
        return Vec::new();
    }
    lines(&text)
        .iter()
        .filter(|l| !l.is_empty())
        .take(MAX_HISTORY_ENTRIES)
        .map(|l| (*l).to_owned())
        .collect()
}

/// Persist command history to the VFS.
///
/// Writes up to `MAX_HISTORY_ENTRIES` commands, one per line, to
/// `HISTORY_PATH`.  Previous contents are overwritten.
pub fn save_history(entries: &[String]) {
    let start = if entries.len() > MAX_HISTORY_ENTRIES {
        entries.len() - MAX_HISTORY_ENTRIES
    } else {
        0
    };
    let body: String = entries[start..]
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<&str>>()
        .join("\n");
    if let Err(e) = vfs::write(HISTORY_PATH, &body) {
        println!("dotfiles: failed to save history: {}", e);
    }
}

/// Initialise the dotfile subsystem.
///
/// Creates the `/etc` directory (if absent) and empty placeholder files
/// for `/etc/profile` and `/etc/merlionrc` so that users can populate
/// them later via the shell.  Then loads all profiles and the rc file.
pub fn init() {
    // Ensure /etc exists — ignore errors if already present.
    let _ = vfs::write("/etc/profile", "# /etc/profile — system-wide shell configuration\n");
    let _ = vfs::write("/etc/merlionrc", "# /etc/merlionrc — MerlionOS rc file\n");

    load_profile();
    load_rc();
}
