/// Shell script execution.
/// Reads a file from VFS and executes each line as a shell command.
/// Supports comments (#) and empty lines.

use alloc::string::String;
use alloc::borrow::ToOwned;
use alloc::vec::Vec;

/// Execute a script file from the VFS.
pub fn run_script(path: &str) -> Result<usize, &'static str> {
    let content = crate::vfs::cat(path).map_err(|_| "cannot read script file")?;
    let lines = parse_script(&content);

    let mut executed = 0;
    for line in &lines {
        crate::serial_println!("[script] {}", line);
        crate::shell::dispatch(line);
        executed += 1;
    }

    Ok(executed)
}

/// Parse a script: strip comments, empty lines, and whitespace.
fn parse_script(content: &str) -> Vec<String> {
    content
        .lines()
        .map(|line| {
            // Strip comments
            let line = if let Some(pos) = line.find('#') {
                &line[..pos]
            } else {
                line
            };
            line.trim().to_owned()
        })
        .filter(|line| !line.is_empty())
        .collect()
}

/// Create a default startup script at /tmp/init.sh if it doesn't exist.
pub fn create_default_init() {
    if crate::vfs::exists("/tmp/init.sh") {
        return;
    }
    let script = "\
# MerlionOS startup script
# Edit this file and run: exec /tmp/init.sh
set USER=root
set HOSTNAME=merlion
";
    let _ = crate::vfs::write("/tmp/init.sh", script);
}
