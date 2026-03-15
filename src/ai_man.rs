/// AI-powered manual pages (Phase 64).
/// Explains any shell command using built-in knowledge.

use alloc::string::String;

/// Get a manual page for a command.
pub fn man(cmd: &str) -> String {
    match cmd {
        "help" => entry("help", "Display list of available commands", "help", "Lists all commands grouped by category."),
        "ps" => entry("ps", "List running tasks", "ps", "Shows PID, state (running/ready/finished), and name for each task."),
        "kill" => entry("kill", "Terminate a task by PID", "kill <pid>", "Sends SIGKILL to the specified task. Cannot kill PID 0 (kernel)."),
        "spawn" => entry("spawn", "Create a demo kernel task", "spawn", "Spawns a task that prints 5 iterations with yields, then exits."),
        "ls" => entry("ls", "List directory contents", "ls [path]", "Lists files and directories. Default path is /. Shows type (d=dir, -=file, c=device)."),
        "cat" => entry("cat", "Read file contents", "cat <path>", "Reads and displays the contents of a file. Works with /proc/* for live system data."),
        "write" => entry("write", "Write data to a file", "write <path> <data>", "Creates or overwrites a file. Auto-creates files in writable directories like /tmp."),
        "rm" => entry("rm", "Remove a file", "rm <path>", "Deletes a regular file. Cannot remove directories or device nodes."),
        "neofetch" => entry("neofetch", "System information display", "neofetch", "Shows MerlionOS logo with CPU, memory, uptime, date, task count."),
        "ping" => entry("ping", "Send ICMP echo requests", "ping <host>", "Pings an IP address or hostname (localhost, gateway, self). Shows RTT and packet loss."),
        "ifconfig" => entry("ifconfig", "Network interface configuration", "ifconfig", "Shows MAC address, IP, netmask, gateway, and packet statistics."),
        "memmap" => entry("memmap", "Physical memory map", "memmap", "Displays bootloader memory regions with type, address range, and size. Color-coded."),
        "gfx" => entry("gfx", "Graphics demo", "gfx", "Renders the Singapore flag using 160x50 framebuffer with half-block characters."),
        "shutdown" => entry("shutdown", "Power off the system", "shutdown", "Initiates ACPI shutdown via port 0x604."),
        "reboot" => entry("reboot", "Restart the system", "reboot", "Sends CPU reset via keyboard controller port 0x64."),
        "lsmod" => entry("lsmod", "List kernel modules", "lsmod", "Shows name, state (loaded/unloaded), version, and description of each module."),
        "modprobe" => entry("modprobe", "Load a kernel module", "modprobe <name>", "Calls the module's init() function. Available modules: hello, watchdog, memstat."),
        "monitor" => entry("monitor", "AI system health check", "monitor", "Runs the AI monitor to check heap usage, task count, and memory pressure."),
        "agents" => entry("agents", "List AI agents", "agents", "Shows registered agents: health, greeter, explain. Each has tick count and state."),
        "ask" => entry("ask", "Send message to an AI agent", "ask <agent> <message>", "The agent processes the message and returns a response.\nExamples:\n  ask greeter hello\n  ask explain page fault\n  ask health check"),
        "explain" => entry("explain", "Explain a kernel concept", "explain <topic>", "Topics: page fault, context switch, syscall, gdt, vfs, ipc, slab."),
        "test" => entry("test", "Run kernel self-tests", "test", "Runs 15 built-in tests covering heap, VFS, IPC, timer, RTC, memory."),
        "free" => entry("free", "Memory usage summary", "free", "Shows physical memory, heap, and disk usage in a table format."),
        "date" => entry("date", "Show current date and time", "date", "Reads from CMOS RTC (MC146818) via ports 0x70/0x71."),
        "env" => entry("env", "List environment variables", "env", "Shows all set variables. Default: HOSTNAME, OS, VERSION, ARCH, SHELL, HOME."),
        "config" => entry("config", "Show kernel configuration", "config", "Displays /etc/merlion.conf settings: hostname, prompt, thresholds, log level."),
        "ai" => entry("ai", "Ask the AI assistant", "ai <text>", "Sends text to LLM proxy (COM2) or falls back to keyword AI. Understands Chinese and English."),
        _ => alloc::format!("No manual entry for '{}'. Try 'help' for command list.", cmd),
    }
}

fn entry(name: &str, brief: &str, usage: &str, desc: &str) -> String {
    alloc::format!(
        "\x1b[1m{}\x1b[0m — {}\n\nUsage: \x1b[33m{}\x1b[0m\n\n{}\n",
        name, brief, usage, desc
    )
}
