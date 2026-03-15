/// Environment variables and command aliases.
/// Provides a simple key-value store accessible from the shell.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use spin::Mutex;

const MAX_VARS: usize = 32;
const MAX_ALIASES: usize = 16;

static ENV: Mutex<EnvStore> = Mutex::new(EnvStore::new());

struct Entry {
    key: String,
    value: String,
}

struct EnvStore {
    vars: Vec<Entry>,
    aliases: Vec<Entry>,
}

impl EnvStore {
    const fn new() -> Self {
        Self {
            vars: Vec::new(),
            aliases: Vec::new(),
        }
    }
}

/// Initialize default environment variables.
pub fn init() {
    set("HOSTNAME", "merlion");
    set("OS", "MerlionOS");
    set("VERSION", "0.2.0");
    set("ARCH", "x86_64");
    set("SHELL", "/bin/msh");
    set("HOME", "/tmp");
    set("PS1", "merlion> ");
}

/// Set an environment variable.
pub fn set(key: &str, value: &str) {
    let mut env = ENV.lock();
    if env.vars.len() >= MAX_VARS {
        // Overwrite oldest if full
        return;
    }
    // Update existing
    for entry in env.vars.iter_mut() {
        if entry.key == key {
            entry.value = value.to_owned();
            return;
        }
    }
    // Add new
    env.vars.push(Entry {
        key: key.to_owned(),
        value: value.to_owned(),
    });
}

/// Get an environment variable.
pub fn get(key: &str) -> Option<String> {
    let env = ENV.lock();
    env.vars.iter().find(|e| e.key == key).map(|e| e.value.clone())
}

/// Remove an environment variable.
pub fn unset(key: &str) {
    let mut env = ENV.lock();
    env.vars.retain(|e| e.key != key);
}

/// List all environment variables.
pub fn list() -> Vec<(String, String)> {
    let env = ENV.lock();
    env.vars.iter().map(|e| (e.key.clone(), e.value.clone())).collect()
}

/// Set a command alias.
pub fn set_alias(name: &str, command: &str) {
    let mut env = ENV.lock();
    if env.aliases.len() >= MAX_ALIASES {
        return;
    }
    for entry in env.aliases.iter_mut() {
        if entry.key == name {
            entry.value = command.to_owned();
            return;
        }
    }
    env.aliases.push(Entry {
        key: name.to_owned(),
        value: command.to_owned(),
    });
}

/// Resolve an alias. Returns the command if found.
pub fn resolve_alias(name: &str) -> Option<String> {
    let env = ENV.lock();
    env.aliases.iter().find(|e| e.key == name).map(|e| e.value.clone())
}

/// List all aliases.
pub fn list_aliases() -> Vec<(String, String)> {
    let env = ENV.lock();
    env.aliases.iter().map(|e| (e.key.clone(), e.value.clone())).collect()
}

/// Expand $VAR references in a string.
pub fn expand(input: &str) -> String {
    let mut result = String::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' {
            let mut var_name = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_alphanumeric() || c == '_' {
                    var_name.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            if let Some(val) = get(&var_name) {
                result.push_str(&val);
            } else {
                result.push('$');
                result.push_str(&var_name);
            }
        } else {
            result.push(ch);
        }
    }

    result
}
