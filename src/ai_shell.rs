/// AI Shell — natural language command interpreter.
/// Maps Chinese and English natural language input to kernel commands.
/// Phase A: keyword-based matching (no external LLM needed).

use alloc::string::String;

/// Try to interpret natural language input as a kernel command.
/// Returns Some(command) if a match is found, None otherwise.
pub fn interpret(input: &str) -> Option<String> {
    let input = input.trim().to_lowercase();
    let input = input.as_str();

    // Exact matches first (passthrough for known commands)
    if is_known_command(input) {
        return None; // let the shell handle it directly
    }

    // Chinese patterns
    let cmd = match_chinese(input)
        .or_else(|| match_english(input))
        .or_else(|| match_question(input));

    cmd
}

fn match_chinese(input: &str) -> Option<String> {
    // System info
    if contains_any(input, &["系统信息", "系统状态", "系统概览"]) {
        return Some(String::from("neofetch"));
    }
    if contains_any(input, &["什么时间", "几点", "当前时间", "现在时间", "日期"]) {
        return Some(String::from("date"));
    }
    if contains_any(input, &["运行时间", "开机多久", "运行了多久", "启动时间"]) {
        return Some(String::from("uptime"));
    }
    if contains_any(input, &["关机", "关闭", "关掉"]) {
        return Some(String::from("shutdown"));
    }
    if contains_any(input, &["重启", "重新启动"]) {
        return Some(String::from("reboot"));
    }
    if contains_any(input, &["清屏", "清除屏幕"]) {
        return Some(String::from("clear"));
    }

    // Process
    if contains_any(input, &["进程", "任务列表", "正在运行"]) {
        return Some(String::from("ps"));
    }
    if contains_any(input, &["杀掉", "终止", "结束进程"]) {
        // Try to extract PID
        if let Some(pid) = extract_number(input) {
            return Some(alloc::format!("kill {}", pid));
        }
    }

    // Memory
    if contains_any(input, &["内存使用", "内存状态", "内存信息"]) {
        return Some(String::from("free"));
    }
    if contains_any(input, &["内存映射", "物理内存"]) {
        return Some(String::from("memmap"));
    }
    if contains_any(input, &["堆", "堆内存"]) {
        return Some(String::from("heap"));
    }

    // Files
    if contains_any(input, &["列出文件", "显示文件", "文件列表", "目录"]) {
        if let Some(path) = extract_path(input) {
            return Some(alloc::format!("ls {}", path));
        }
        return Some(String::from("ls"));
    }
    if contains_any(input, &["读取文件", "查看文件", "打开文件", "文件内容"]) {
        if let Some(path) = extract_path(input) {
            return Some(alloc::format!("cat {}", path));
        }
    }

    // Hardware
    if contains_any(input, &["cpu信息", "处理器", "cpu"]) {
        return Some(String::from("cpuinfo"));
    }
    if contains_any(input, &["pci设备", "硬件设备", "设备列表"]) {
        return Some(String::from("lspci"));
    }
    if contains_any(input, &["网络信息", "网卡", "网络接口", "网络状态"]) {
        return Some(String::from("ifconfig"));
    }
    if contains_any(input, &["驱动", "驱动列表"]) {
        return Some(String::from("drivers"));
    }
    if contains_any(input, &["内核模块", "模块列表"]) {
        return Some(String::from("lsmod"));
    }
    if contains_any(input, &["内核日志", "系统日志"]) {
        return Some(String::from("dmesg"));
    }

    // Fun
    if contains_any(input, &["画图", "图形", "显示图形"]) {
        return Some(String::from("gfx"));
    }
    if contains_any(input, &["测试", "自测", "自检"]) {
        return Some(String::from("test"));
    }
    if contains_any(input, &["帮助", "命令列表", "怎么用", "有什么命令"]) {
        return Some(String::from("help"));
    }

    None
}

fn match_english(input: &str) -> Option<String> {
    // Natural language English patterns
    if contains_any(input, &["show me system", "system info", "about this"]) {
        return Some(String::from("neofetch"));
    }
    if contains_any(input, &["what time", "current time", "show date", "what day"]) {
        return Some(String::from("date"));
    }
    if contains_any(input, &["how long", "uptime", "been running"]) {
        return Some(String::from("uptime"));
    }
    if contains_any(input, &["shut down", "power off", "turn off"]) {
        return Some(String::from("shutdown"));
    }
    if contains_any(input, &["list process", "running process", "show process", "what is running"]) {
        return Some(String::from("ps"));
    }
    if contains_any(input, &["how much memory", "memory usage", "ram usage"]) {
        return Some(String::from("free"));
    }
    if contains_any(input, &["list files", "show files", "what files"]) {
        return Some(String::from("ls"));
    }
    if contains_any(input, &["cpu info", "what cpu", "processor info"]) {
        return Some(String::from("cpuinfo"));
    }
    if contains_any(input, &["network info", "show network", "ip address"]) {
        return Some(String::from("ifconfig"));
    }
    if contains_any(input, &["list driver", "show driver"]) {
        return Some(String::from("drivers"));
    }
    if contains_any(input, &["kernel log", "show log", "system log"]) {
        return Some(String::from("dmesg"));
    }
    if contains_any(input, &["clear screen", "clean screen"]) {
        return Some(String::from("clear"));
    }
    if contains_any(input, &["run test", "self test", "run diagnostic"]) {
        return Some(String::from("test"));
    }
    if contains_any(input, &["show help", "what can you do", "help me", "commands"]) {
        return Some(String::from("help"));
    }

    // Ping pattern: "ping X"
    if input.starts_with("ping ") {
        return None; // already a valid command
    }
    if contains_any(input, &["can i reach", "is .* online", "check connection"]) {
        return Some(String::from("ping localhost"));
    }

    None
}

fn match_question(input: &str) -> Option<String> {
    // Common question patterns
    if input.ends_with('?') || input.ends_with('？') {
        if contains_any(input, &["who am i", "我是谁"]) {
            return Some(String::from("whoami"));
        }
        if contains_any(input, &["where am i", "我在哪", "主机名"]) {
            return Some(String::from("hostname"));
        }
        if contains_any(input, &["what os", "什么系统", "什么操作系统"]) {
            return Some(String::from("uname"));
        }
    }
    None
}

// --- Helper functions ---

fn contains_any(input: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| input.contains(p))
}

fn extract_number(input: &str) -> Option<usize> {
    input.split_whitespace()
        .find_map(|w| w.parse::<usize>().ok())
}

fn extract_path(input: &str) -> Option<&str> {
    input.split_whitespace()
        .find(|w| w.starts_with('/'))
}

/// Check if input is already a known shell command (don't reinterpret).
fn is_known_command(input: &str) -> bool {
    let first_word = input.split_whitespace().next().unwrap_or("");
    matches!(first_word,
        "help" | "info" | "ps" | "spawn" | "kill" | "bg" | "run" | "progs"
        | "ls" | "cat" | "write" | "rm" | "open" | "close" | "lsof"
        | "date" | "uptime" | "heap" | "free" | "memmap" | "drivers"
        | "lsmod" | "modprobe" | "rmmod" | "modinfo"
        | "cpuinfo" | "ifconfig" | "send" | "recv" | "ping" | "arp"
        | "lspci" | "virtio" | "blkdevs"
        | "disk" | "format" | "dls" | "dsave" | "dload"
        | "fatfmt" | "fatls" | "fatw" | "fatr"
        | "echo" | "env" | "set" | "unset" | "alias"
        | "neofetch" | "uname" | "whoami" | "hostname"
        | "sleep" | "history" | "gfx" | "test"
        | "slabinfo" | "lockdemo" | "bt"
        | "clear" | "shutdown" | "reboot" | "panic"
        | "pipe" | "channels" | "dmesg"
    )
}

/// Format an AI interpretation message for the user.
pub fn format_hint(original: &str, command: &str) -> String {
    alloc::format!("\x1b[90m[ai] \"{}\" → {}\x1b[0m", original, command)
}
