/// JSON parser and serializer for MerlionOS.
///
/// Provides a lightweight, `no_std`-compatible JSON implementation with parsing,
/// serialization, and convenient accessor methods.
use alloc::string::String;
use alloc::vec::Vec;

/// Represents a JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    /// JSON `null`.
    Null,
    /// JSON boolean (`true` or `false`).
    Bool(bool),
    /// JSON integer number.
    Number(i64),
    /// JSON floating-point number stored as an i64 scaled by 1,000,000.
    Float(i64),
    /// JSON string.
    String(String),
    /// JSON array of values.
    Array(Vec<JsonValue>),
    /// JSON object as an ordered list of key-value pairs.
    Object(Vec<(String, JsonValue)>),
}

const FLOAT_SCALE: i64 = 1_000_000;

/// Parse a JSON string into a `JsonValue`.
pub fn parse(input: &str) -> Result<JsonValue, &'static str> {
    let b = input.as_bytes();
    let (value, pos) = parse_value(b, skip_ws(b, 0))?;
    if skip_ws(b, pos) != b.len() { return Err("trailing characters"); }
    Ok(value)
}

/// Serialize a `JsonValue` into a compact JSON string.
pub fn stringify(value: &JsonValue) -> String {
    let mut out = String::new();
    write_value(value, &mut out, None, 0);
    out
}

/// Serialize a `JsonValue` into a pretty-printed JSON string.
///
/// Each nesting level is indented by `indent` spaces.
pub fn stringify_pretty(value: &JsonValue, indent: usize) -> String {
    let mut out = String::new();
    write_value(value, &mut out, Some(indent), 0);
    out
}

/// Look up a key in a JSON object.
pub fn get<'a>(obj: &'a JsonValue, key: &str) -> Option<&'a JsonValue> {
    if let JsonValue::Object(pairs) = obj {
        pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    } else { None }
}

/// Convenience: retrieve a string value by key from an object.
pub fn get_str<'a>(obj: &'a JsonValue, key: &str) -> Option<&'a str> {
    match get(obj, key) { Some(JsonValue::String(s)) => Some(s.as_str()), _ => None }
}

/// Convenience: retrieve an integer value by key from an object.
pub fn get_i64(obj: &JsonValue, key: &str) -> Option<i64> {
    match get(obj, key) { Some(JsonValue::Number(n)) => Some(*n), _ => None }
}

/// Convenience: retrieve a boolean value by key from an object.
pub fn get_bool(obj: &JsonValue, key: &str) -> Option<bool> {
    match get(obj, key) { Some(JsonValue::Bool(b)) => Some(*b), _ => None }
}

// ---------------------------------------------------------------------------
// Parser internals
// ---------------------------------------------------------------------------

fn skip_ws(b: &[u8], mut p: usize) -> usize {
    while p < b.len() && matches!(b[p], b' ' | b'\t' | b'\n' | b'\r') { p += 1; }
    p
}

fn expect(b: &[u8], p: usize, ch: u8) -> Result<usize, &'static str> {
    if p < b.len() && b[p] == ch { Ok(p + 1) } else { Err("unexpected character") }
}

fn parse_value(b: &[u8], p: usize) -> Result<(JsonValue, usize), &'static str> {
    if p >= b.len() { return Err("unexpected end of input"); }
    match b[p] {
        b'"' => parse_string(b, p),
        b'{' => parse_object(b, p),
        b'[' => parse_array(b, p),
        b't' | b'f' => parse_bool(b, p),
        b'n' => parse_null(b, p),
        b'-' | b'0'..=b'9' => parse_number(b, p),
        _ => Err("unexpected character"),
    }
}

fn parse_string(b: &[u8], p: usize) -> Result<(JsonValue, usize), &'static str> {
    let (s, end) = parse_string_raw(b, p)?;
    Ok((JsonValue::String(s), end))
}

/// Parse string contents, returning the decoded `String` and position after closing quote.
fn parse_string_raw(b: &[u8], p: usize) -> Result<(String, usize), &'static str> {
    let mut i = expect(b, p, b'"')?;
    let mut s = String::new();
    while i < b.len() {
        match b[i] {
            b'"' => return Ok((s, i + 1)),
            b'\\' => {
                i += 1;
                if i >= b.len() { return Err("unexpected end of escape"); }
                match b[i] {
                    b'"' => s.push('"'),  b'\\' => s.push('\\'), b'/' => s.push('/'),
                    b'b' => s.push('\u{08}'), b'f' => s.push('\u{0C}'),
                    b'n' => s.push('\n'), b'r' => s.push('\r'), b't' => s.push('\t'),
                    b'u' => {
                        if i + 4 >= b.len() { return Err("incomplete unicode escape"); }
                        let hex = core::str::from_utf8(&b[i+1..i+5])
                            .map_err(|_| "invalid unicode escape")?;
                        let cp = u16::from_str_radix(hex, 16)
                            .map_err(|_| "invalid unicode hex")?;
                        s.push(char::from_u32(cp as u32).ok_or("invalid code point")?);
                        i += 4;
                    }
                    _ => return Err("invalid escape character"),
                }
                i += 1;
            }
            ch => { s.push(ch as char); i += 1; }
        }
    }
    Err("unterminated string")
}

/// Parse a JSON number (integer or float stored as scaled i64).
fn parse_number(b: &[u8], p: usize) -> Result<(JsonValue, usize), &'static str> {
    let mut i = p;
    let neg = if i < b.len() && b[i] == b'-' { i += 1; true } else { false };
    if i >= b.len() || !b[i].is_ascii_digit() { return Err("expected digit"); }
    let mut int_part: i64 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        int_part = int_part.checked_mul(10)
            .and_then(|v| v.checked_add((b[i] - b'0') as i64))
            .ok_or("number overflow")?;
        i += 1;
    }
    // Fractional part -> Float variant
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let mut frac: i64 = 0;
        let mut fd: u32 = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            frac = frac * 10 + (b[i] - b'0') as i64;
            fd += 1; i += 1;
        }
        let mut scaled = int_part.checked_mul(FLOAT_SCALE).ok_or("number overflow")?;
        let mut fs = frac;
        for _ in fd..6 { fs *= 10; }
        for _ in 6..fd { fs /= 10; }
        scaled = scaled.checked_add(fs).ok_or("number overflow")?;
        if neg { scaled = -scaled; }
        return Ok((JsonValue::Float(scaled), skip_exp(b, i)));
    }
    if neg { int_part = -int_part; }
    Ok((JsonValue::Number(int_part), skip_exp(b, i)))
}

/// Skip optional exponent (e/E with optional sign and digits).
fn skip_exp(b: &[u8], mut p: usize) -> usize {
    if p < b.len() && (b[p] == b'e' || b[p] == b'E') {
        p += 1;
        if p < b.len() && (b[p] == b'+' || b[p] == b'-') { p += 1; }
        while p < b.len() && b[p].is_ascii_digit() { p += 1; }
    }
    p
}

fn parse_bool(b: &[u8], p: usize) -> Result<(JsonValue, usize), &'static str> {
    if b.get(p..p+4) == Some(b"true") { return Ok((JsonValue::Bool(true), p + 4)); }
    if b.get(p..p+5) == Some(b"false") { return Ok((JsonValue::Bool(false), p + 5)); }
    Err("expected boolean")
}

fn parse_null(b: &[u8], p: usize) -> Result<(JsonValue, usize), &'static str> {
    if b.get(p..p+4) == Some(b"null") { Ok((JsonValue::Null, p + 4)) }
    else { Err("expected 'null'") }
}

/// Parse a JSON object (`{ "key": value, ... }`).
fn parse_object(b: &[u8], p: usize) -> Result<(JsonValue, usize), &'static str> {
    let mut i = expect(b, p, b'{')?;
    let mut pairs: Vec<(String, JsonValue)> = Vec::new();
    i = skip_ws(b, i);
    if i < b.len() && b[i] == b'}' { return Ok((JsonValue::Object(pairs), i + 1)); }
    loop {
        i = skip_ws(b, i);
        let (key, next) = parse_string_raw(b, i)?;
        i = expect(b, skip_ws(b, next), b':')?;
        let (val, next) = parse_value(b, skip_ws(b, i))?;
        pairs.push((key, val));
        i = skip_ws(b, next);
        if i < b.len() && b[i] == b',' { i += 1; continue; }
        return Ok((JsonValue::Object(pairs), expect(b, i, b'}')?));
    }
}

/// Parse a JSON array (`[ value, ... ]`).
fn parse_array(b: &[u8], p: usize) -> Result<(JsonValue, usize), &'static str> {
    let mut i = expect(b, p, b'[')?;
    let mut elems: Vec<JsonValue> = Vec::new();
    i = skip_ws(b, i);
    if i < b.len() && b[i] == b']' { return Ok((JsonValue::Array(elems), i + 1)); }
    loop {
        let (val, next) = parse_value(b, skip_ws(b, i))?;
        elems.push(val);
        i = skip_ws(b, next);
        if i < b.len() && b[i] == b',' { i += 1; continue; }
        return Ok((JsonValue::Array(elems), expect(b, i, b']')?));
    }
}

// ---------------------------------------------------------------------------
// Serializer internals
// ---------------------------------------------------------------------------

fn write_indent(out: &mut String, ind: Option<usize>, depth: usize) {
    if let Some(n) = ind { for _ in 0..depth * n { out.push(' '); } }
}

fn write_nl(out: &mut String, ind: Option<usize>) {
    if ind.is_some() { out.push('\n'); }
}

/// Write a JSON string with proper escaping.
fn write_escaped(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""), '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"), '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"), '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let n = c as u32;
                out.push_str("\\u00");
                out.push(char::from_digit((n >> 4) & 0xF, 16).unwrap_or('0'));
                out.push(char::from_digit(n & 0xF, 16).unwrap_or('0'));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Write an i64 to the output string without `format!`.
fn write_i64(out: &mut String, n: i64) {
    if n == 0 { out.push('0'); return; }
    if n < 0 {
        out.push('-');
        if n == i64::MIN { out.push_str("9223372036854775808"); return; }
        write_i64(out, -n); return;
    }
    let mut buf = [0u8; 20];
    let mut p = 20;
    let mut v = n;
    while v > 0 { p -= 1; buf[p] = b'0' + (v % 10) as u8; v /= 10; }
    for &b in &buf[p..20] { out.push(b as char); }
}

/// Recursively serialize a `JsonValue`.
fn write_value(value: &JsonValue, out: &mut String, ind: Option<usize>, depth: usize) {
    match value {
        JsonValue::Null => out.push_str("null"),
        JsonValue::Bool(true) => out.push_str("true"),
        JsonValue::Bool(false) => out.push_str("false"),
        JsonValue::Number(n) => write_i64(out, *n),
        JsonValue::Float(scaled) => {
            let neg = *scaled < 0;
            let abs = if neg { -(*scaled) } else { *scaled };
            if neg { out.push('-'); }
            write_i64(out, abs / FLOAT_SCALE);
            out.push('.');
            let frac_str = alloc::format!("{:06}", abs % FLOAT_SCALE);
            let trimmed = frac_str.trim_end_matches('0');
            out.push_str(if trimmed.is_empty() { "0" } else { trimmed });
        }
        JsonValue::String(s) => write_escaped(out, s),
        JsonValue::Array(elems) => {
            out.push('[');
            if elems.is_empty() { out.push(']'); return; }
            write_nl(out, ind);
            for (i, elem) in elems.iter().enumerate() {
                write_indent(out, ind, depth + 1);
                write_value(elem, out, ind, depth + 1);
                if i + 1 < elems.len() { out.push(','); }
                write_nl(out, ind);
            }
            write_indent(out, ind, depth);
            out.push(']');
        }
        JsonValue::Object(pairs) => {
            out.push('{');
            if pairs.is_empty() { out.push('}'); return; }
            write_nl(out, ind);
            for (i, (key, val)) in pairs.iter().enumerate() {
                write_indent(out, ind, depth + 1);
                write_escaped(out, key);
                out.push(':');
                if ind.is_some() { out.push(' '); }
                write_value(val, out, ind, depth + 1);
                if i + 1 < pairs.len() { out.push(','); }
                write_nl(out, ind);
            }
            write_indent(out, ind, depth);
            out.push('}');
        }
    }
}
