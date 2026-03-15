/// AI HTTP client for MerlionOS v10 — Claude API deep integration.
///
/// Provides direct HTTP-based communication with the Anthropic Claude API.
/// Builds raw HTTP/1.1 POST requests, parses JSON responses, and offers
/// a high-level `ai_query` function that cascades through available
/// transports (TCP, serial proxy, keyword engine).
///
/// Because the kernel has no TLS stack yet, TCP connections to the API
/// endpoint are plain HTTP on port 443 (will not complete a TLS handshake).
/// The practical path today is the serial LLM proxy (`ai_proxy`); the
/// direct-TCP path is scaffolding for future HTTPS support.

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Claude API base URL (HTTPS — requires TLS, not yet available).
pub const API_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";

/// API host used in HTTP Host header and TCP connection.
pub const API_HOST: &str = "api.anthropic.com";

/// API path for the messages endpoint.
pub const API_PATH: &str = "/v1/messages";

/// Default model for inference requests.
pub const MODEL: &str = "claude-sonnet-4-20250514";

/// API version header value expected by Anthropic.
pub const API_VERSION: &str = "2023-06-01";

/// Maximum tokens to request in a single completion.
const MAX_TOKENS: usize = 200;

/// TCP port for HTTPS (TLS not yet implemented).
const API_PORT: u16 = 443;

// ---------------------------------------------------------------------------
// Known commands for local completion
// ---------------------------------------------------------------------------

/// Shell commands recognised by the completion engine.
static KNOWN_COMMANDS: &[&str] = &[
    "help", "info", "ps", "kill", "spawn", "clear", "echo", "cat",
    "ls", "mkdir", "touch", "rm", "cd", "pwd", "date", "uptime",
    "dmesg", "env", "set", "alias", "shutdown", "reboot", "mem",
    "exec", "mount", "umount", "df", "ping", "net", "dhcp", "dns",
    "http", "wget", "chat", "agent", "ai", "bench", "calc", "edit",
    "hexdump", "xxd", "grep", "wc", "head", "tail", "sort", "uniq",
    "sleep", "demo", "vga", "fb", "pci", "acpi", "disk", "fsck",
];

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

/// Build a raw HTTP/1.1 POST request to the Claude Messages API.
///
/// The returned `Vec<u8>` contains a complete HTTP request including
/// headers and JSON body, ready to be written to a TCP socket.
///
/// # Headers
///
/// - `Content-Type: application/json`
/// - `x-api-key: <api_key>`
/// - `anthropic-version: 2023-06-01`
/// - `Host`, `Connection: close`, `User-Agent` (standard)
///
/// # Body
///
/// ```json
/// {
///   "model": "claude-sonnet-4-20250514",
///   "max_tokens": 200,
///   "messages": [{"role": "user", "content": "<prompt>"}]
/// }
/// ```
pub fn claude_request(prompt: &str, api_key: &str) -> Vec<u8> {
    let escaped = escape_json(prompt);
    let body = format!(
        "{{\"model\":\"{MODEL}\",\"max_tokens\":{MAX_TOKENS},\
         \"messages\":[{{\"role\":\"user\",\"content\":\"{escaped}\"}}]}}"
    );

    let req = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Connection: close\r\n\
         User-Agent: MerlionOS/10.0\r\n\
         Content-Type: application/json\r\n\
         x-api-key: {key}\r\n\
         anthropic-version: {ver}\r\n\
         Content-Length: {len}\r\n\
         \r\n\
         {body}",
        path = API_PATH,
        host = API_HOST,
        key = api_key,
        ver = API_VERSION,
        len = body.len(),
        body = body,
    );

    req.into_bytes()
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

/// Extract the first `"text":"..."` value from a Claude API JSON response.
///
/// This is a lightweight parser that avoids pulling in a full JSON library.
/// It searches for the literal pattern `"text":"` and extracts the string
/// that follows, handling basic JSON escapes (`\"`, `\\`).
///
/// Returns `None` if the pattern is not found or the data is not valid UTF-8.
pub fn parse_claude_response(data: &[u8]) -> Option<String> {
    let text = core::str::from_utf8(data).ok()?;

    // Locate the body after the HTTP header boundary, if present.
    let body = if let Some(pos) = text.find("\r\n\r\n") {
        &text[pos + 4..]
    } else {
        text
    };

    // Find "text":" — the content block in the Claude response.
    let marker = "\"text\":\"";
    let start = body.find(marker)? + marker.len();
    let rest = &body[start..];

    // Walk until the unescaped closing quote.
    let mut result = String::new();
    let mut chars = rest.chars();
    loop {
        match chars.next()? {
            '\\' => {
                // Escaped character — take the next one literally.
                let esc = chars.next()?;
                match esc {
                    'n' => result.push('\n'),
                    't' => result.push('\t'),
                    'r' => result.push('\r'),
                    _ => result.push(esc), // \", \\, etc.
                }
            }
            '"' => break, // End of the text value.
            c => result.push(c),
        }
    }

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

// ---------------------------------------------------------------------------
// High-level query
// ---------------------------------------------------------------------------

/// Send a prompt to the best available AI backend and return the response.
///
/// The function tries transports in order of preference:
///
/// 1. **Direct TCP** to `api.anthropic.com:443` — requires an API key
///    from kconfig (`ai_api_key`) or the environment (`ANTHROPIC_API_KEY`).
///    Note: HTTPS/TLS is **not yet implemented**, so this path will
///    currently fail the handshake. It is included as scaffolding.
///
/// 2. **Serial LLM proxy** via `ai_proxy::infer` — the host-side proxy
///    forwards to Claude or another model over the real network.
///
/// 3. **Keyword engine** via `ai_syscall::infer` — a local pattern-match
///    fallback that works entirely offline.
///
/// Returns `Err` only if *all three* transports fail to produce a result.
pub fn ai_query(prompt: &str) -> Result<String, &'static str> {
    // --- Attempt 1: direct TCP with Claude API key --------------------------
    if let Some(api_key) = get_api_key() {
        let request_bytes = claude_request(prompt, &api_key);

        // Try a raw TCP connection (HTTPS not supported yet).
        crate::serial_println!(
            "[ai_http] WARNING: TLS not available; attempting plain TCP to {}:{}",
            API_HOST,
            API_PORT
        );

        if let Some(response_bytes) = tcp_send(API_HOST, API_PORT, &request_bytes) {
            if let Some(answer) = parse_claude_response(&response_bytes) {
                return Ok(answer);
            }
            crate::serial_println!("[ai_http] failed to parse Claude API response");
        } else {
            crate::serial_println!("[ai_http] TCP connection to Claude API failed");
        }
    }

    // --- Attempt 2: serial LLM proxy ---------------------------------------
    if let Some(answer) = crate::ai_proxy::infer(prompt) {
        return Ok(answer);
    }

    // --- Attempt 3: keyword engine ------------------------------------------
    let fallback = crate::ai_syscall::infer(prompt);
    if !fallback.is_empty() {
        return Ok(fallback);
    }

    Err("ai_http: all inference backends unavailable")
}

// ---------------------------------------------------------------------------
// Command completion
// ---------------------------------------------------------------------------

/// Suggest a completion for a partial shell command.
///
/// Performs prefix matching against `KNOWN_COMMANDS`. If exactly one command
/// matches the prefix, the remaining suffix is returned. If multiple
/// commands match, the longest common prefix of the candidates (beyond
/// what the user typed) is returned. Returns `None` when there are no
/// matches or the input is already a complete command.
///
/// # Example
///
/// ```ignore
/// assert_eq!(ai_complete("shut"), Some("down".to_owned()));
/// assert_eq!(ai_complete("ec"),   Some("ho".to_owned()));
/// ```
pub fn ai_complete(partial_cmd: &str) -> Option<String> {
    if partial_cmd.is_empty() {
        return None;
    }

    let lower = partial_cmd.to_lowercase();
    let candidates: Vec<&str> = KNOWN_COMMANDS
        .iter()
        .copied()
        .filter(|cmd| cmd.starts_with(&*lower) && *cmd != lower)
        .collect();

    if candidates.is_empty() {
        return None;
    }

    if candidates.len() == 1 {
        // Single match — return the remaining suffix.
        return Some(candidates[0][lower.len()..].to_owned());
    }

    // Multiple matches — compute the longest common prefix beyond input.
    let first = candidates[0].as_bytes();
    let mut common_len = first.len();
    for c in &candidates[1..] {
        let b = c.as_bytes();
        common_len = common_len.min(b.len());
        for i in lower.len()..common_len {
            if first[i] != b[i] {
                common_len = i;
                break;
            }
        }
    }

    if common_len > lower.len() {
        Some(candidates[0][lower.len()..common_len].to_owned())
    } else {
        // Ambiguous — nothing further to complete automatically.
        None
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Retrieve the API key from kconfig or the environment.
fn get_api_key() -> Option<String> {
    // Prefer kconfig (persisted kernel configuration).
    if let Some(key) = crate::kconfig::get("ai_api_key") {
        if !key.is_empty() {
            return Some(key);
        }
    }
    // Fallback to environment variable.
    if let Some(key) = crate::env::get("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            return Some(key);
        }
    }
    None
}

/// Attempt a raw TCP send/receive cycle.
///
/// Returns `Some(response_bytes)` on success, `None` if the network stack
/// is unavailable or the connection fails.
fn tcp_send(_host: &str, _port: u16, _data: &[u8]) -> Option<Vec<u8>> {
    // TODO: integrate with the kernel TCP stack (tcp.rs / net.rs).
    // Currently a stub — direct HTTPS requires a TLS layer we don't have.
    None
}

/// Escape a string for inclusion in a JSON string literal.
///
/// Handles `"`, `\`, and control characters (`\n`, `\r`, `\t`).
fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}
