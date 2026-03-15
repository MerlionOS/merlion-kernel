/// AI Proxy — LLM communication protocol over serial.
/// Sends inference requests to the host machine via COM2 (0x2F8)
/// and receives responses. The host runs a proxy that forwards
/// to Claude API, Ollama, or another LLM.
///
/// Protocol (JSON-like, line-delimited):
///   Request:  {"q":"<prompt>"}\n
///   Response: {"a":"<answer>"}\n
///
/// When no proxy is connected, falls back to the keyword-based
/// AI shell (ai_shell.rs).

use x86_64::instructions::port::Port;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

/// COM2 base port for AI proxy communication.
const COM2_PORT: u16 = 0x2F8;

/// Whether the AI proxy has been detected on COM2.
static PROXY_CONNECTED: AtomicBool = AtomicBool::new(false);

/// Initialize COM2 for AI proxy communication.
pub fn init() {
    unsafe {
        let mut ier = Port::<u8>::new(COM2_PORT + 1);
        let mut lcr = Port::<u8>::new(COM2_PORT + 3);
        let mut data = Port::<u8>::new(COM2_PORT);
        let mut fifo = Port::<u8>::new(COM2_PORT + 2);
        let mut mcr = Port::<u8>::new(COM2_PORT + 4);

        ier.write(0x00);  // Disable interrupts
        lcr.write(0x80);  // Enable DLAB
        data.write(0x03); // 38400 baud
        ier.write(0x00);  // Divisor hi
        lcr.write(0x03);  // 8N1
        fifo.write(0xC7); // Enable FIFO
        mcr.write(0x0B);  // RTS/DSR set
    }

    // Try to detect proxy by sending a ping
    send_raw(b"{\"q\":\"ping\"}\n");

    // Check if we get a response within a short timeout
    let start = crate::timer::ticks();
    while crate::timer::ticks() < start + 10 { // 100ms timeout
        if has_data() {
            let response = recv_line();
            if response.contains("pong") || response.contains("\"a\"") {
                PROXY_CONNECTED.store(true, Ordering::SeqCst);
                crate::serial_println!("[ai-proxy] LLM proxy detected on COM2");
                crate::klog_println!("[ai-proxy] connected");
                return;
            }
        }
        core::hint::spin_loop();
    }

    crate::serial_println!("[ai-proxy] no proxy on COM2 (using keyword fallback)");
    crate::klog_println!("[ai-proxy] no proxy, fallback mode");
}

/// Check if the AI proxy is connected.
pub fn is_connected() -> bool {
    PROXY_CONNECTED.load(Ordering::SeqCst)
}

/// Send an inference request to the LLM proxy.
/// Returns the response text, or None if no proxy is connected.
pub fn infer(prompt: &str) -> Option<String> {
    if !is_connected() {
        return None;
    }

    // Build request
    let request = alloc::format!("{{\"q\":\"{}\"}}\n", escape_json(prompt));
    send_raw(request.as_bytes());

    // Wait for response (up to 5 seconds)
    let start = crate::timer::ticks();
    let timeout = crate::timer::PIT_FREQUENCY_HZ * 5;

    while crate::timer::ticks() < start + timeout {
        if has_data() {
            let response = recv_line();
            // Parse response: {"a":"..."}
            if let Some(answer) = parse_response(&response) {
                return Some(answer);
            }
        }
        core::hint::spin_loop();
    }

    None // timeout
}

/// Send raw bytes to COM2.
fn send_raw(data: &[u8]) {
    for &byte in data {
        unsafe {
            // Wait for transmit ready
            let mut lsr = Port::<u8>::new(COM2_PORT + 5);
            while lsr.read() & 0x20 == 0 {}
            Port::new(COM2_PORT).write(byte);
        }
    }
}

/// Check if COM2 has data available.
fn has_data() -> bool {
    unsafe {
        Port::<u8>::new(COM2_PORT + 5).read() & 0x01 != 0
    }
}

/// Read a line from COM2 (up to newline or 1024 bytes).
fn recv_line() -> String {
    let mut buf = Vec::new();
    let start = crate::timer::ticks();

    while crate::timer::ticks() < start + 50 { // 500ms line timeout
        if has_data() {
            let byte = unsafe { Port::<u8>::new(COM2_PORT).read() };
            if byte == b'\n' {
                break;
            }
            if buf.len() < 1024 {
                buf.push(byte);
            }
        }
    }

    String::from_utf8(buf).unwrap_or_default()
}

/// Parse a JSON response: {"a":"answer text"}
fn parse_response(response: &str) -> Option<String> {
    // Simple parser: find "a":" and extract until closing "
    let marker = "\"a\":\"";
    let start = response.find(marker)? + marker.len();
    let end = response[start..].find('"')? + start;
    Some(unescape_json(&response[start..end]))
}

/// Escape special chars for JSON string.
fn escape_json(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

/// Unescape JSON string.
fn unescape_json(s: &str) -> String {
    let mut out = String::new();
    let mut escape = false;
    for ch in s.chars() {
        if escape {
            match ch {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                _ => { out.push('\\'); out.push(ch); }
            }
            escape = false;
        } else if ch == '\\' {
            escape = true;
        } else {
            out.push(ch);
        }
    }
    out
}

/// AI proxy status for display.
pub fn status() -> &'static str {
    if is_connected() { "connected (COM2)" } else { "offline (keyword fallback)" }
}
