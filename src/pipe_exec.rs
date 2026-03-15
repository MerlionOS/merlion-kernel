/// Shell pipe execution — captures output of one command and feeds
/// it as input to another.
///
///   cat /proc/tasks | grep running
///   ls /proc | sort
///   cat /proc/meminfo | head 2

use alloc::string::String;
use spin::Mutex;

/// Output capture buffer. When active, println! output goes here
/// instead of (or in addition to) the VGA screen.
static CAPTURE: Mutex<Option<String>> = Mutex::new(None);

/// Start capturing VGA output.
pub fn start_capture() {
    *CAPTURE.lock() = Some(String::new());
}

/// Stop capturing and return the captured output.
pub fn stop_capture() -> String {
    CAPTURE.lock().take().unwrap_or_default()
}

/// If capturing, append text to the buffer. Returns true if captured.
pub fn try_capture(s: &str) -> bool {
    let mut cap = CAPTURE.lock();
    if let Some(ref mut buf) = *cap {
        buf.push_str(s);
        true
    } else {
        false
    }
}

/// Check if we're currently capturing.
pub fn is_capturing() -> bool {
    CAPTURE.lock().is_some()
}

/// Execute a pipeline: cmd1 | cmd2 [| cmd3 ...]
pub fn execute_pipeline(pipeline: &str) {
    let cmds: alloc::vec::Vec<&str> = pipeline.split('|').map(|s| s.trim()).collect();

    if cmds.len() < 2 {
        // No pipe, just run normally
        crate::shell::dispatch(cmds[0]);
        return;
    }

    // Execute first command, capture its output
    start_capture();
    crate::shell::dispatch(cmds[0]);
    let mut data = stop_capture();

    // For each subsequent command, feed previous output as input
    for &cmd in &cmds[1..] {
        let result = process_piped_command(cmd, &data);
        data = result;
    }

    // Print final result
    if !data.is_empty() {
        crate::println!("{}", data.trim_end());
    }
}

/// Process a command with piped input data.
fn process_piped_command(cmd: &str, input: &str) -> String {
    let cmd = cmd.trim();
    let parts: alloc::vec::Vec<&str> = cmd.splitn(2, ' ').collect();
    let name = parts[0];
    let args = if parts.len() > 1 { parts[1].trim() } else { "" };

    match name {
        "grep" => {
            if args.is_empty() { return String::from("grep: missing pattern"); }
            crate::coreutils::grep(args, input)
                .join("\n")
        }
        "head" => {
            let n = args.parse::<usize>().unwrap_or(10);
            crate::coreutils::head(input, n).join("\n")
        }
        "tail" => {
            let n = args.parse::<usize>().unwrap_or(10);
            crate::coreutils::tail(input, n).join("\n")
        }
        "sort" => {
            crate::coreutils::sort(input).join("\n")
        }
        "uniq" => {
            crate::coreutils::uniq(input).join("\n")
        }
        "wc" => {
            let lines = input.lines().count();
            let words = input.split_whitespace().count();
            let bytes = input.len();
            alloc::format!("{} lines, {} words, {} bytes", lines, words, bytes)
        }
        "rev" => {
            input.lines()
                .map(|l| crate::coreutils::rev(l))
                .collect::<alloc::vec::Vec<_>>()
                .join("\n")
        }
        "hexdump" => {
            crate::coreutils::hexdump(input.as_bytes(), 256)
        }
        _ => {
            // Unknown pipe target — just run as a command with input ignored
            start_capture();
            crate::shell::dispatch(cmd);
            stop_capture()
        }
    }
}
