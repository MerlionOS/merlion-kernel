/// AI Syscall interface (Phase E).
/// Provides kernel-level AI services accessible via syscall or
/// direct kernel API. Syscall numbers 7-10 are reserved for AI.
///
///   7  ai_infer(prompt_ptr, prompt_len, out_ptr, out_len) → bytes written
///   8  ai_classify(text_ptr, text_len) → category_id
///   9  ai_tag(path_ptr, path_len) → number of tags added
///  10  ai_search(query_ptr, query_len, out_ptr, out_len) → result count

use alloc::string::String;
use alloc::vec::Vec;

/// AI service result.
pub struct AiResult {
    pub text: String,
    pub confidence: f32,
}

/// Infer: send a prompt to the AI engine, get a response.
pub fn infer(prompt: &str) -> String {
    // Try proxy first
    if let Some(response) = crate::ai_proxy::infer(prompt) {
        return response;
    }

    // Fallback: keyword-based response
    keyword_respond(prompt)
}

/// Classify text into categories.
pub fn classify(text: &str) -> (&'static str, u8) {
    let lower = text.to_lowercase();

    if contains_any(&lower, &["error", "fail", "panic", "crash", "bug"]) {
        return ("error", 0);
    }
    if contains_any(&lower, &["memory", "heap", "ram", "alloc", "frame"]) {
        return ("memory", 1);
    }
    if contains_any(&lower, &["process", "task", "thread", "pid", "spawn"]) {
        return ("process", 2);
    }
    if contains_any(&lower, &["file", "disk", "fs", "path", "directory"]) {
        return ("filesystem", 3);
    }
    if contains_any(&lower, &["network", "tcp", "udp", "ip", "ping", "packet"]) {
        return ("network", 4);
    }
    if contains_any(&lower, &["driver", "device", "pci", "uart", "serial"]) {
        return ("hardware", 5);
    }

    ("general", 255)
}

/// Auto-tag a file based on its path and content.
pub fn auto_tag(path: &str) -> Vec<String> {
    let mut tags = Vec::new();

    // Tag based on path
    if path.starts_with("/proc/") {
        tags.push(String::from("system"));
        tags.push(String::from("proc"));
    }
    if path.starts_with("/dev/") {
        tags.push(String::from("device"));
    }
    if path.starts_with("/tmp/") {
        tags.push(String::from("user"));
        tags.push(String::from("temporary"));
    }

    // Tag based on content if readable
    if let Ok(content) = crate::vfs::cat(path) {
        let (category, _) = classify(&content);
        tags.push(String::from(category));

        // Content-based heuristics
        if content.contains("error") || content.contains("fail") {
            tags.push(String::from("error"));
        }
        if content.len() > 1000 {
            tags.push(String::from("large"));
        }
    }

    // Apply tags to semfs
    let tag_refs: Vec<&str> = tags.iter().map(|s| s.as_str()).collect();
    crate::semfs::tag(path, &tag_refs);

    tags
}

/// Keyword-based response for when no LLM proxy is available.
fn keyword_respond(prompt: &str) -> String {
    let lower = prompt.to_lowercase();

    if contains_any(&lower, &["hello", "hi", "你好"]) {
        return String::from("Hello! I'm MerlionOS AI assistant. Try 'help' for commands.");
    }
    if contains_any(&lower, &["who are you", "你是谁", "what are you"]) {
        return String::from("I'm MerlionOS — Born for AI, Built by AI. A Rust x86_64 hobby OS.");
    }
    if contains_any(&lower, &["how are you", "你好吗"]) {
        return String::from("All systems nominal! Run 'monitor' for a health check.");
    }
    if contains_any(&lower, &["version", "版本"]) {
        return String::from("MerlionOS v1.0.0 — 43 modules, ~6900 lines of Rust.");
    }
    if contains_any(&lower, &["meaning of life", "42"]) {
        return String::from("42. But have you tried 'neofetch'?");
    }
    if contains_any(&lower, &["thank", "谢谢", "thanks"]) {
        return String::from("You're welcome! Born for AI. Built by AI.");
    }

    // Try to map to a command
    if let Some(cmd) = crate::ai_shell::interpret(prompt) {
        return alloc::format!("Try running: {}", cmd);
    }

    String::from("I don't understand yet. Connect an LLM proxy to COM2 for full AI.")
}

fn contains_any(input: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| input.contains(p))
}

/// Explain a kernel concept.
pub fn explain(topic: &str) -> String {
    let lower = topic.to_lowercase();

    match lower.as_str() {
        "page fault" | "pagefault" => String::from(
            "A page fault occurs when code accesses a virtual address that isn't \
             mapped to physical memory. The CPU triggers interrupt 14. Our handler \
             tries demand paging first; if that fails, the process is killed."
        ),
        "context switch" => String::from(
            "A context switch saves one task's CPU registers (rbx, rbp, r12-r15, rsp) \
             and loads another task's saved registers. This is how multitasking works — \
             the CPU rapidly switches between tasks."
        ),
        "syscall" | "system call" => String::from(
            "A syscall is how user programs request kernel services. In MerlionOS, \
             user code executes 'int 0x80' with rax=syscall number and rdi/rsi/rdx=args. \
             A naked trampoline saves registers and calls the Rust dispatcher."
        ),
        "gdt" => String::from(
            "The Global Descriptor Table defines memory segments. MerlionOS has: \
             null, kernel code (0x08), TSS (0x10), user data (0x23), user code (0x2B). \
             It separates kernel (ring 0) from user (ring 3) privilege levels."
        ),
        "vfs" => String::from(
            "The Virtual Filesystem provides a uniform interface to different storage: \
             /dev for devices, /proc for kernel status, /tmp for user files. \
             Each node has a type (directory, file, device) and read/write ops."
        ),
        "ipc" => String::from(
            "Inter-Process Communication lets tasks exchange data. MerlionOS uses \
             bounded channels — 64-byte ring buffers with send/recv operations. \
             The 'pipe' command demos a producer-consumer pattern."
        ),
        "slab" => String::from(
            "A slab allocator pre-divides pages into fixed-size slots for fast \
             allocation of common objects (tasks, IPC messages, file descriptors). \
             Much faster than the general heap for known-size objects."
        ),
        _ => alloc::format!("No explanation available for '{}'. Try: page fault, context switch, syscall, gdt, vfs, ipc, slab.", topic),
    }
}
