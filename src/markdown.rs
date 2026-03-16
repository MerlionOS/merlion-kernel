/// Markdown renderer for the MerlionOS terminal.
///
/// Converts a subset of Markdown into ANSI-escaped terminal output suitable
/// for the kernel VGA/serial console.  Supported elements: headings, bold,
/// italic, inline code, fenced code blocks, lists, links, horizontal rules,
/// and blockquotes.  Designed for `#![no_std]` + `alloc`.
use alloc::string::String;

// ANSI escape sequences
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const ITALIC: &str = "\x1b[3m";
const UNDERLINE: &str = "\x1b[4m";
const DIM: &str = "\x1b[2m";
const FG_CYAN: &str = "\x1b[96m";
const FG_GREEN: &str = "\x1b[92m";
const FG_YELLOW: &str = "\x1b[93m";
const FG_GRAY: &str = "\x1b[90m";
const FG_MAGENTA: &str = "\x1b[95m";
const FG_BLUE: &str = "\x1b[94m";
const FG_WHITE: &str = "\x1b[97m";
/// Width of rendered horizontal rules.
const RULE_W: usize = 60;

/// Block-level element kind for a single Markdown line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineKind { Heading(u8), UnorderedItem, OrderedItem, HorizontalRule,
                Blockquote, CodeFence, Paragraph }

/// Classify a trimmed line into its block-level kind.
fn classify(line: &str) -> LineKind {
    if line.starts_with("```") { return LineKind::CodeFence; }
    if line.len() >= 3 {
        let b = line.as_bytes();
        let ch = b[0];
        if (ch == b'-' || ch == b'*' || ch == b'_')
            && b.iter().all(|&c| c == ch || c == b' ')
            && b.iter().filter(|&&c| c == ch).count() >= 3
        { return LineKind::HorizontalRule; }
    }
    if line.starts_with("### ") { return LineKind::Heading(3); }
    if line.starts_with("## ")  { return LineKind::Heading(2); }
    if line.starts_with("# ")   { return LineKind::Heading(1); }
    if line.starts_with("- ") || line.starts_with("* ") {
        return LineKind::UnorderedItem;
    }
    if let Some(dot) = line.find(". ") {
        let pfx = &line[..dot];
        if !pfx.is_empty() && pfx.bytes().all(|b| b.is_ascii_digit()) {
            return LineKind::OrderedItem;
        }
    }
    if line.starts_with("> ") || line == ">" { return LineKind::Blockquote; }
    LineKind::Paragraph
}

// ── Byte-level search helpers ───────────────────────────────────────────

/// Find `needle` in `bytes` starting at `from`.
fn find_byte(bytes: &[u8], needle: u8, from: usize) -> Option<usize> {
    let mut j = from;
    while j < bytes.len() { if bytes[j] == needle { return Some(j); } j += 1; }
    None
}

/// Find closing `**` starting at `from`.
fn find_double_star(bytes: &[u8], from: usize) -> Option<usize> {
    let mut j = from;
    while j + 1 < bytes.len() {
        if bytes[j] == b'*' && bytes[j + 1] == b'*' { return Some(j); }
        j += 1;
    }
    None
}

/// Find a closing single `*` (not part of `**`) starting at `from`.
fn find_single_star(bytes: &[u8], from: usize) -> Option<usize> {
    let mut j = from;
    while j < bytes.len() {
        if bytes[j] == b'*' && (j + 1 >= bytes.len() || bytes[j + 1] != b'*') {
            return Some(j);
        }
        j += 1;
    }
    None
}

/// Try to parse `[text](url)` at `pos`.  Returns `(text, url, end)`.
fn try_parse_link<'a>(bytes: &'a [u8], pos: usize) -> Option<(&'a str, &'a str, usize)> {
    let cb = find_byte(bytes, b']', pos + 1)?;
    if cb + 1 >= bytes.len() || bytes[cb + 1] != b'(' { return None; }
    let cp = find_byte(bytes, b')', cb + 2)?;
    let text = core::str::from_utf8(&bytes[pos + 1..cb]).ok()?;
    let url  = core::str::from_utf8(&bytes[cb + 2..cp]).ok()?;
    Some((text, url, cp + 1))
}

/// Push a byte sub-slice (valid UTF-8) onto `out`.
fn push_slice(out: &mut String, bytes: &[u8], from: usize, to: usize) {
    if let Ok(s) = core::str::from_utf8(&bytes[from..to]) { out.push_str(s); }
}

// ── Inline span rendering ───────────────────────────────────────────────

/// Render inline Markdown spans (bold, italic, code, links) into ANSI output.
fn render_inline(text: &str, out: &mut String) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        // Inline code `...`
        if bytes[i] == b'`' {
            if let Some(end) = find_byte(bytes, b'`', i + 1) {
                out.push_str(FG_GRAY); out.push('`');
                push_slice(out, bytes, i + 1, end);
                out.push('`'); out.push_str(RESET);
                i = end + 1; continue;
            }
        }
        // Link [text](url)
        if bytes[i] == b'[' {
            if let Some((lt, url, after)) = try_parse_link(bytes, i) {
                out.push_str(UNDERLINE); out.push_str(FG_CYAN);
                out.push_str(lt); out.push_str(RESET);
                out.push_str(FG_MAGENTA); out.push_str(DIM);
                out.push_str(" ("); out.push_str(url); out.push(')');
                out.push_str(RESET);
                i = after; continue;
            }
        }
        // Bold **...**
        if i + 1 < len && bytes[i] == b'*' && bytes[i + 1] == b'*' {
            if let Some(end) = find_double_star(bytes, i + 2) {
                out.push_str(BOLD); out.push_str(FG_WHITE);
                push_slice(out, bytes, i + 2, end);
                out.push_str(RESET);
                i = end + 2; continue;
            }
        }
        // Italic *...*
        if bytes[i] == b'*' && (i + 1 >= len || bytes[i + 1] != b'*') {
            if let Some(end) = find_single_star(bytes, i + 1) {
                out.push_str(ITALIC);
                push_slice(out, bytes, i + 1, end);
                out.push_str(RESET);
                i = end + 1; continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
}

// ── Block-level helpers ─────────────────────────────────────────────────

/// Strip heading prefix (`# `, `## `, `### `) and return content.
fn strip_heading(line: &str, level: u8) -> &str {
    let skip = level as usize + 1;
    if line.len() > skip { &line[skip..] } else { "" }
}

/// Strip unordered-list prefix (`- ` or `* `).
fn strip_ul(line: &str) -> &str { if line.len() > 2 { &line[2..] } else { "" } }

/// Strip ordered-list prefix, returning `(number, content)`.
fn strip_ol(line: &str) -> (&str, &str) {
    if let Some(d) = line.find(". ") {
        (&line[..d], if line.len() > d + 2 { &line[d + 2..] } else { "" })
    } else { ("1", line) }
}

/// Strip blockquote prefix (`> ` or bare `>`).
fn strip_bq(line: &str) -> &str {
    if line.starts_with("> ") { &line[2..] } else if line == ">" { "" } else { line }
}

/// Emit a dim horizontal box-drawing rule.
fn render_rule(out: &mut String) {
    out.push_str(DIM);
    for _ in 0..RULE_W { out.push('\u{2500}'); }
    out.push_str(RESET);
    out.push('\n');
}

// ── Public API ──────────────────────────────────────────────────────────

/// Convert a Markdown string into ANSI-coloured terminal output.
///
/// Headings are bold with per-level colours (h1=cyan, h2=green, h3=yellow).
/// Code is rendered in gray.  Bold text uses bright white.  Links show the
/// display text underlined in cyan followed by a dim magenta URL hint.
/// List items get unicode bullets or their original numbering.  Blockquotes
/// are prefixed with a blue vertical bar and rendered in italic.
pub fn render_markdown(md: &str) -> String {
    let mut out = String::new();
    let mut in_code_block = false;

    for line in md.lines() {
        let trimmed = line.trim();

        // Inside a fenced code block — render verbatim in gray
        if in_code_block {
            if trimmed.starts_with("```") {
                in_code_block = false;
                out.push_str(RESET);
                continue;
            }
            out.push_str(FG_GRAY);
            out.push_str("    ");
            out.push_str(line);
            out.push_str(RESET);
            out.push('\n');
            continue;
        }

        if trimmed.is_empty() { out.push('\n'); continue; }

        match classify(trimmed) {
            LineKind::CodeFence => {
                in_code_block = true;
                let lang = trimmed.trim_start_matches('`').trim();
                if !lang.is_empty() {
                    out.push_str(DIM); out.push_str(FG_GRAY);
                    out.push_str("  ["); out.push_str(lang);
                    out.push_str("]\n"); out.push_str(RESET);
                }
            }
            LineKind::Heading(level) => {
                let content = strip_heading(trimmed, level);
                out.push_str(BOLD);
                match level {
                    1 => out.push_str(FG_CYAN),
                    2 => out.push_str(FG_GREEN),
                    _ => out.push_str(FG_YELLOW),
                }
                render_inline(content, &mut out);
                out.push_str(RESET);
                out.push('\n');
                // h1 gets a double-line underline
                if level == 1 {
                    out.push_str(DIM);
                    for _ in 0..content.len().min(RULE_W) { out.push('\u{2550}'); }
                    out.push_str(RESET);
                    out.push('\n');
                }
            }
            LineKind::HorizontalRule => render_rule(&mut out),
            LineKind::UnorderedItem => {
                out.push_str("  \u{2022} ");
                render_inline(strip_ul(trimmed), &mut out);
                out.push('\n');
            }
            LineKind::OrderedItem => {
                let (num, content) = strip_ol(trimmed);
                out.push_str("  "); out.push_str(num); out.push_str(". ");
                render_inline(content, &mut out);
                out.push('\n');
            }
            LineKind::Blockquote => {
                out.push_str(FG_BLUE);
                out.push_str("  \u{2502} ");
                out.push_str(RESET);
                out.push_str(ITALIC);
                render_inline(strip_bq(trimmed), &mut out);
                out.push_str(RESET);
                out.push('\n');
            }
            LineKind::Paragraph => {
                render_inline(trimmed, &mut out);
                out.push('\n');
            }
        }
    }
    if in_code_block { out.push_str(RESET); }
    out
}

/// Strip all Markdown syntax, returning plain text without ANSI escapes.
///
/// Useful for computing display widths or accessibility output.
pub fn strip_markdown(md: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    for line in md.lines() {
        let t = line.trim();
        if in_code {
            if t.starts_with("```") { in_code = false; }
            else { out.push_str(line); out.push('\n'); }
            continue;
        }
        if t.starts_with("```") { in_code = true; continue; }
        if t.is_empty() { out.push('\n'); continue; }
        let content = match classify(t) {
            LineKind::Heading(lv)   => strip_heading(t, lv),
            LineKind::UnorderedItem => strip_ul(t),
            LineKind::OrderedItem   => { let (_, c) = strip_ol(t); c }
            LineKind::Blockquote    => strip_bq(t),
            LineKind::HorizontalRule | LineKind::CodeFence => continue,
            LineKind::Paragraph     => t,
        };
        strip_inline(content, &mut out);
        out.push('\n');
    }
    out
}

/// Strip inline Markdown markers, appending plain text to `out`.
fn strip_inline(text: &str, out: &mut String) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        if bytes[i] == b'`' {
            if let Some(e) = find_byte(bytes, b'`', i + 1) {
                push_slice(out, bytes, i + 1, e); i = e + 1; continue;
            }
        }
        if bytes[i] == b'[' {
            if let Some((lt, _, after)) = try_parse_link(bytes, i) {
                out.push_str(lt); i = after; continue;
            }
        }
        if i + 1 < len && bytes[i] == b'*' && bytes[i + 1] == b'*' {
            if let Some(e) = find_double_star(bytes, i + 2) {
                push_slice(out, bytes, i + 2, e); i = e + 2; continue;
            }
        }
        if bytes[i] == b'*' && (i + 1 >= len || bytes[i + 1] != b'*') {
            if let Some(e) = find_single_star(bytes, i + 1) {
                push_slice(out, bytes, i + 1, e); i = e + 1; continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
}
