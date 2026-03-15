/// Core utilities — Unix-like text processing commands.
/// grep, head, tail, hexdump, xxd, sort, uniq, tee, wc (extended).

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;

/// grep: search for pattern in text, return matching lines.
pub fn grep(pattern: &str, text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.contains(pattern))
        .map(|l| l.to_owned())
        .collect()
}

/// grep with line numbers.
pub fn grep_n(pattern: &str, text: &str) -> Vec<String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| line.contains(pattern))
        .map(|(i, l)| alloc::format!("{:>4}: {}", i + 1, l))
        .collect()
}

/// grep -i: case-insensitive.
pub fn grep_i(pattern: &str, text: &str) -> Vec<String> {
    let pat_lower = pattern.to_lowercase();
    text.lines()
        .filter(|line| line.to_lowercase().contains(&pat_lower))
        .map(|l| l.to_owned())
        .collect()
}

/// grep -c: count matching lines.
pub fn grep_c(pattern: &str, text: &str) -> usize {
    text.lines().filter(|line| line.contains(pattern)).count()
}

/// head: first N lines.
pub fn head(text: &str, n: usize) -> Vec<String> {
    text.lines().take(n).map(|l| l.to_owned()).collect()
}

/// tail: last N lines.
pub fn tail(text: &str, n: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].iter().map(|l| (*l).to_owned()).collect()
}

/// sort: sort lines alphabetically.
pub fn sort(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.lines().map(|l| l.to_owned()).collect();
    // Simple bubble sort (no std sort available)
    let len = lines.len();
    for i in 0..len {
        for j in 0..len.saturating_sub(i + 1) {
            if lines[j] > lines[j + 1] {
                lines.swap(j, j + 1);
            }
        }
    }
    lines
}

/// uniq: remove consecutive duplicate lines.
pub fn uniq(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut prev: Option<&str> = None;
    for line in text.lines() {
        if prev != Some(line) {
            result.push(line.to_owned());
        }
        prev = Some(line);
    }
    result
}

/// hexdump: format bytes as hex + ASCII (16 bytes per line).
pub fn hexdump(data: &[u8], max_bytes: usize) -> String {
    let mut out = String::new();
    let limit = data.len().min(max_bytes);

    for row in 0..(limit + 15) / 16 {
        let offset = row * 16;
        if offset >= limit { break; }

        // Offset
        out.push_str(&alloc::format!("{:08x}  ", offset));

        // Hex bytes
        for col in 0..16 {
            if offset + col < limit {
                out.push_str(&alloc::format!("{:02x} ", data[offset + col]));
            } else {
                out.push_str("   ");
            }
            if col == 7 { out.push(' '); }
        }

        out.push_str(" |");

        // ASCII
        for col in 0..16 {
            if offset + col < limit {
                let b = data[offset + col];
                if b >= 0x20 && b < 0x7F {
                    out.push(b as char);
                } else {
                    out.push('.');
                }
            }
        }
        out.push_str("|\n");
    }

    out
}

/// xxd: compact hex dump (single line per 16 bytes, no ASCII).
pub fn xxd(data: &[u8], max_bytes: usize) -> String {
    let mut out = String::new();
    let limit = data.len().min(max_bytes);

    for row in 0..(limit + 15) / 16 {
        let offset = row * 16;
        if offset >= limit { break; }

        out.push_str(&alloc::format!("{:08x}: ", offset));
        for col in 0..16 {
            if offset + col < limit {
                out.push_str(&alloc::format!("{:02x}", data[offset + col]));
            }
            if col % 2 == 1 { out.push(' '); }
        }
        out.push('\n');
    }

    out
}

/// rev: reverse a string.
pub fn rev(text: &str) -> String {
    text.chars().rev().collect()
}

/// tr: translate characters (simple single-char replacement).
pub fn tr(text: &str, from: char, to: char) -> String {
    text.chars().map(|c| if c == from { to } else { c }).collect()
}

/// cut: extract fields by delimiter.
pub fn cut(text: &str, delim: char, field: usize) -> Vec<String> {
    text.lines()
        .map(|line| {
            line.split(delim)
                .nth(field)
                .unwrap_or("")
                .to_owned()
        })
        .collect()
}
