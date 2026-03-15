// completion.rs — Tab completion engine for the MerlionOS shell

use alloc::string::String;
use alloc::vec::Vec;

/// All known shell commands, sorted alphabetically.
static COMMANDS: &[&str] = &[
    "about",
    "agents",
    "ahciinfo",
    "ai",
    "aistatus",
    "alias",
    "arp",
    "arpreq",
    "ask",
    "bench",
    "blkdevs",
    "bt",
    "calc",
    "cat",
    "channels",
    "chat",
    "clear",
    "close",
    "config",
    "cpuinfo",
    "date",
    "demo",
    "diskfmt",
    "diskinfo",
    "diskload",
    "diskls",
    "diskread",
    "diskrm",
    "disksave",
    "diskwrite",
    "dmesg",
    "dns",
    "drivers",
    "e1000info",
    "echo",
    "edit",
    "env",
    "exec",
    "explain",
    "fatfmt",
    "fatls",
    "fatr",
    "fatw",
    "format",
    "forth",
    "fortune",
    "free",
    "gfx",
    "gptinfo",
    "grep",
    "head",
    "heal",
    "heap",
    "heapcheck",
    "help",
    "hexdump",
    "history",
    "hostname",
    "ifconfig",
    "ifup",
    "info",
    "ioapicinfo",
    "kill",
    "loadelf",
    "lockdemo",
    "ls",
    "lsmod",
    "lsof",
    "lspci",
    "man",
    "matrix",
    "memmap",
    "mkelf",
    "modprobe",
    "monitor",
    "neofetch",
    "netstat",
    "nicsend",
    "nvmeinfo",
    "open",
    "panic",
    "ping",
    "pipe",
    "powerinfo",
    "ps",
    "rawping",
    "readelf",
    "reboot",
    "rm",
    "rmmod",
    "search",
    "set",
    "setconf",
    "shutdown",
    "signal",
    "slabinfo",
    "sleep",
    "snake",
    "sort",
    "spawn",
    "stackcheck",
    "tag",
    "tags",
    "tail",
    "tcpclose",
    "tcpconn",
    "tcprecv",
    "tcpsend",
    "test",
    "top",
    "uname",
    "unset",
    "uptime",
    "usbdevs",
    "version",
    "watch",
    "wc",
    "wget",
    "whoami",
    "write",
];

/// Return all commands whose name starts with `partial`.
pub fn complete(partial: &str) -> Vec<&'static str> {
    COMMANDS
        .iter()
        .filter(|cmd| cmd.starts_with(partial))
        .copied()
        .collect()
}

/// Complete a file path using the VFS.
///
/// If `partial` contains a `/`, we split into directory and prefix, list the
/// directory via `crate::vfs::ls()`, and return entries matching the prefix.
/// Otherwise we list `/` and filter.
pub fn complete_path(partial: &str) -> Vec<String> {
    let (dir, prefix) = match partial.rfind('/') {
        Some(pos) => {
            let d = if pos == 0 { "/" } else { &partial[..pos] };
            let p = &partial[pos + 1..];
            (d, p)
        }
        None => ("/", partial),
    };

    let entries = match crate::vfs::ls(dir) {
        Ok(list) => list,
        Err(_) => return Vec::new(),
    };

    let dir_slash = if dir == "/" { "/" } else { dir };

    entries
        .iter()
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, kind)| {
            let mut path = String::new();
            if dir_slash == "/" {
                path.push('/');
            } else {
                path.push_str(dir_slash);
                path.push('/');
            }
            path.push_str(name);
            if *kind == 'd' {
                path.push('/');
            }
            path
        })
        .collect()
}

/// Return a single completion if exactly one command matches.
pub fn complete_one(partial: &str) -> Option<&'static str> {
    let matches = complete(partial);
    if matches.len() == 1 {
        Some(matches[0])
    } else {
        None
    }
}

/// Format a list of completion matches for display, space-separated.
pub fn format_completions(matches: &[&str]) -> String {
    let mut out = String::new();
    for (i, m) in matches.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(m);
    }
    out
}

/// Find the longest common prefix among all matches.
pub fn longest_common_prefix(matches: &[&str]) -> String {
    if matches.is_empty() {
        return String::new();
    }
    let first = matches[0];
    let mut len = first.len();
    for m in &matches[1..] {
        len = len.min(m.len());
        for (i, (a, b)) in first.bytes().zip(m.bytes()).enumerate() {
            if a != b {
                len = len.min(i);
                break;
            }
        }
    }
    String::from(&first[..len])
}
