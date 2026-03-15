/// TOML-like configuration file parser for MerlionOS.
///
/// Provides a lightweight, `no_std`-compatible TOML parser supporting sections,
/// nested tables via dotted headers, key-value pairs (strings, integers, booleans),
/// inline arrays, and `#` line comments.
use alloc::string::String;
use alloc::vec::Vec;

/// Represents a TOML value.
#[derive(Debug, Clone, PartialEq)]
pub enum TomlValue {
    /// A quoted string value.
    String(String),
    /// A 64-bit signed integer value.
    Integer(i64),
    /// A boolean value (`true` or `false`).
    Boolean(bool),
    /// An array of heterogeneous values.
    Array(Vec<TomlValue>),
    /// A table (ordered list of key-value pairs).
    Table(Vec<(String, TomlValue)>),
}

/// Parse a TOML document into a `TomlValue::Table`.
///
/// Supports `[section]` headers, `[section.subsection]` dotted headers,
/// `key = "string"`, `key = 123`, `key = true/false`, inline arrays
/// `key = [1, 2, 3]`, and `#` line comments.
pub fn parse(input: &str) -> Result<TomlValue, &'static str> {
    let mut root: Vec<(String, TomlValue)> = Vec::new();
    let mut current_path: Vec<String> = Vec::new();

    for raw_line in input.lines() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() { continue; }

        // Section header: [section] or [section.subsection]
        if line.starts_with('[') {
            let end = line.find(']').ok_or("unterminated section header")?;
            let header = line[1..end].trim();
            if header.is_empty() { return Err("empty section header"); }
            current_path = split_dotted(header);
            ensure_path(&mut root, &current_path);
            continue;
        }

        // Key = value
        let eq = line.find('=').ok_or("expected '=' in key-value pair")?;
        let key = line[..eq].trim();
        if key.is_empty() { return Err("empty key"); }
        let val_str = line[eq + 1..].trim();
        let value = parse_value(val_str)?;

        let table = descend_mut(&mut root, &current_path)?;
        table.push((String::from(key), value));
    }

    Ok(TomlValue::Table(root))
}

/// Look up a value by dotted key (e.g. `"server.port"`) in a table hierarchy.
///
/// Traverses nested `TomlValue::Table` entries for each dot-separated segment,
/// returning a reference to the final value if found.
pub fn get<'a>(table: &'a TomlValue, dotted_key: &str) -> Option<&'a TomlValue> {
    let parts = split_dotted(dotted_key);
    if parts.is_empty() { return Some(table); }
    let mut current = table;
    for (i, part) in parts.iter().enumerate() {
        match current {
            TomlValue::Table(pairs) => match pairs.iter().find(|(k, _)| k == part) {
                Some((_, v)) if i + 1 == parts.len() => return Some(v),
                Some((_, v)) => current = v,
                None => return None,
            },
            _ => return None,
        }
    }
    None
}

/// Convenience: retrieve a string value by dotted key.
pub fn get_str<'a>(table: &'a TomlValue, key: &str) -> Option<&'a str> {
    match get(table, key) { Some(TomlValue::String(s)) => Some(s.as_str()), _ => None }
}

/// Convenience: retrieve an integer value by dotted key.
pub fn get_i64(table: &TomlValue, key: &str) -> Option<i64> {
    match get(table, key) { Some(TomlValue::Integer(n)) => Some(*n), _ => None }
}

/// Convenience: retrieve a boolean value by dotted key.
pub fn get_bool(table: &TomlValue, key: &str) -> Option<bool> {
    match get(table, key) { Some(TomlValue::Boolean(b)) => Some(*b), _ => None }
}

/// Convenience: retrieve an array by dotted key.
pub fn get_array<'a>(table: &'a TomlValue, key: &str) -> Option<&'a Vec<TomlValue>> {
    match get(table, key) { Some(TomlValue::Array(a)) => Some(a), _ => None }
}

// ---------------------------------------------------------------------------
// Parser internals
// ---------------------------------------------------------------------------

/// Strip a `#` comment from a line, respecting quoted strings.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let (mut in_str, mut i) = (false, 0);
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_str = !in_str,
            b'\\' if in_str => { i += 1; }
            b'#' if !in_str => return &line[..i],
            _ => {}
        }
        i += 1;
    }
    line
}

/// Split a dotted key like `"server.network.port"` into segments.
fn split_dotted(s: &str) -> Vec<String> {
    s.split('.').map(|p| String::from(p.trim())).collect()
}

/// Parse a single TOML value from a trimmed string.
fn parse_value(s: &str) -> Result<TomlValue, &'static str> {
    if s.is_empty() { return Err("empty value"); }
    if s.starts_with('"') { return parse_string(s); }
    if s == "true" { return Ok(TomlValue::Boolean(true)); }
    if s == "false" { return Ok(TomlValue::Boolean(false)); }
    if s.starts_with('[') { return parse_array(s); }
    parse_integer(s)
}

/// Parse a quoted string value, handling basic escape sequences.
fn parse_string(s: &str) -> Result<TomlValue, &'static str> {
    let bytes = s.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'"' { return Err("expected opening quote"); }
    let mut result = String::new();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => return Ok(TomlValue::String(result)),
            b'\\' => {
                i += 1;
                if i >= bytes.len() { return Err("unexpected end of escape"); }
                match bytes[i] {
                    b'"' => result.push('"'),   b'\\' => result.push('\\'),
                    b'n' => result.push('\n'),  b'r' => result.push('\r'),
                    b't' => result.push('\t'),
                    _ => return Err("invalid escape sequence"),
                }
            }
            ch => result.push(ch as char),
        }
        i += 1;
    }
    Err("unterminated string")
}

/// Parse a decimal integer, possibly with leading sign or underscores.
fn parse_integer(s: &str) -> Result<TomlValue, &'static str> {
    let mut iter = s.bytes().peekable();
    let neg = match iter.peek() {
        Some(b'-') => { iter.next(); true }
        Some(b'+') => { iter.next(); false }
        _ => false,
    };
    let mut value: i64 = 0;
    let mut any = false;
    for ch in iter {
        if ch == b'_' { continue; }
        if !ch.is_ascii_digit() { return Err("invalid integer"); }
        any = true;
        value = value.checked_mul(10)
            .and_then(|v| v.checked_add((ch - b'0') as i64))
            .ok_or("integer overflow")?;
    }
    if !any { return Err("expected digit"); }
    if neg { value = -value; }
    Ok(TomlValue::Integer(value))
}

/// Parse an inline array `[val1, val2, ...]`.
fn parse_array(s: &str) -> Result<TomlValue, &'static str> {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes[0] != b'[' { return Err("expected '['"); }
    let close = find_matching_bracket(bytes, 0)?;
    let inner = s[1..close].trim();
    if inner.is_empty() { return Ok(TomlValue::Array(Vec::new())); }
    let mut elems = Vec::new();
    for part in split_array_elements(inner)? {
        let trimmed = part.trim();
        if !trimmed.is_empty() { elems.push(parse_value(trimmed)?); }
    }
    Ok(TomlValue::Array(elems))
}

/// Find the index of the `]` matching the `[` at `start`.
fn find_matching_bracket(bytes: &[u8], start: usize) -> Result<usize, &'static str> {
    let (mut depth, mut in_str, mut i) = (0i32, false, start);
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_str = !in_str,
            b'\\' if in_str => { i += 1; }
            b'[' if !in_str => depth += 1,
            b']' if !in_str => { depth -= 1; if depth == 0 { return Ok(i); } }
            _ => {}
        }
        i += 1;
    }
    Err("unterminated array")
}

/// Split array content by top-level commas, respecting nesting and strings.
fn split_array_elements(s: &str) -> Result<Vec<&str>, &'static str> {
    let bytes = s.as_bytes();
    let (mut parts, mut start, mut depth, mut in_str, mut i) = (Vec::new(), 0, 0i32, false, 0);
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_str = !in_str,
            b'\\' if in_str => { i += 1; }
            b'[' if !in_str => depth += 1,
            b']' if !in_str => depth -= 1,
            b',' if !in_str && depth == 0 => { parts.push(&s[start..i]); start = i + 1; }
            _ => {}
        }
        i += 1;
    }
    if start <= bytes.len() { parts.push(&s[start..]); }
    Ok(parts)
}

/// Ensure all intermediate tables along `path` exist inside `root`.
fn ensure_path(root: &mut Vec<(String, TomlValue)>, path: &[String]) {
    let mut table = root;
    for seg in path {
        let idx = table.iter().position(|(k, _)| k == seg);
        let pos = match idx {
            Some(i) => i,
            None => { table.push((seg.clone(), TomlValue::Table(Vec::new()))); table.len() - 1 }
        };
        table = match &mut table[pos].1 {
            TomlValue::Table(t) => t,
            _ => return,
        };
    }
}

/// Descend into nested tables following `path`, returning a mutable ref to the
/// innermost table's pair list.
fn descend_mut<'a>(
    root: &'a mut Vec<(String, TomlValue)>,
    path: &[String],
) -> Result<&'a mut Vec<(String, TomlValue)>, &'static str> {
    let mut table = root;
    for seg in path {
        let idx = table.iter().position(|(k, _)| k == seg).ok_or("section not found")?;
        table = match &mut table[idx].1 {
            TomlValue::Table(t) => t,
            _ => return Err("expected table"),
        };
    }
    Ok(table)
}
