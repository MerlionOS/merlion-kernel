/// Bash/Zsh compatible shell interpreter for MerlionOS.
/// Provides advanced shell scripting features: arrays, associative arrays,
/// parameter expansion, here documents, process substitution, arithmetic,
/// test expressions, case statements, and interactive features.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::borrow::ToOwned;
use alloc::format;
use alloc::collections::BTreeMap;
use spin::Mutex;
use core::sync::atomic::{AtomicU8, Ordering};

// ── Shell Mode ──────────────────────────────────────────────────────────

/// Which shell compatibility mode is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellMode {
    Bash,
    Zsh,
    Sh,
}

static SHELL_MODE: AtomicU8 = AtomicU8::new(0); // 0=Bash, 1=Zsh, 2=Sh

/// Set the active shell compatibility mode.
pub fn set_mode(mode: ShellMode) {
    let v = match mode {
        ShellMode::Bash => 0,
        ShellMode::Zsh  => 1,
        ShellMode::Sh   => 2,
    };
    SHELL_MODE.store(v, Ordering::Relaxed);
}

/// Get the active shell compatibility mode.
pub fn get_mode() -> ShellMode {
    match SHELL_MODE.load(Ordering::Relaxed) {
        1 => ShellMode::Zsh,
        2 => ShellMode::Sh,
        _ => ShellMode::Bash,
    }
}

// ── Shell Options ───────────────────────────────────────────────────────

/// Shell option flags (set -e, set -x, etc.).
#[derive(Debug, Clone)]
struct ShellOptions {
    errexit: bool,    // set -e: exit on error
    xtrace: bool,     // set -x: trace execution
    nounset: bool,    // set -u: error on undefined variable
    pipefail: bool,   // set -o pipefail
}

impl ShellOptions {
    const fn new() -> Self {
        Self { errexit: false, xtrace: false, nounset: false, pipefail: false }
    }
}

static OPTIONS: Mutex<ShellOptions> = Mutex::new(ShellOptions::new());

// ── Shell Variables ─────────────────────────────────────────────────────

struct ShellVars {
    vars: BTreeMap<String, String>,
    exported: BTreeMap<String, String>,
    locals: Vec<BTreeMap<String, String>>,
    positional: Vec<String>,
}

impl ShellVars {
    const fn new() -> Self {
        Self {
            vars: BTreeMap::new(),
            exported: BTreeMap::new(),
            locals: Vec::new(),
            positional: Vec::new(),
        }
    }

    fn get(&self, name: &str) -> Option<String> {
        // Check local scopes (innermost first)
        for scope in self.locals.iter().rev() {
            if let Some(v) = scope.get(name) {
                return Some(v.clone());
            }
        }
        if let Some(v) = self.vars.get(name) {
            return Some(v.clone());
        }
        if let Some(v) = self.exported.get(name) {
            return Some(v.clone());
        }
        crate::env::get(name)
    }

    fn set(&mut self, name: &str, value: &str) {
        self.vars.insert(name.to_owned(), value.to_owned());
    }

    fn set_local(&mut self, name: &str, value: &str) {
        if let Some(scope) = self.locals.last_mut() {
            scope.insert(name.to_owned(), value.to_owned());
        } else {
            self.vars.insert(name.to_owned(), value.to_owned());
        }
    }

    fn export(&mut self, name: &str, value: &str) {
        self.exported.insert(name.to_owned(), value.to_owned());
        crate::env::set(name, value);
    }

    fn unset(&mut self, name: &str) {
        self.vars.remove(name);
        self.exported.remove(name);
    }

    fn push_scope(&mut self) {
        self.locals.push(BTreeMap::new());
    }

    fn pop_scope(&mut self) {
        self.locals.pop();
    }
}

static VARS: Mutex<ShellVars> = Mutex::new(ShellVars::new());

// ── Arrays ──────────────────────────────────────────────────────────────

/// A shell indexed array.
pub struct ShellArray {
    elements: Vec<String>,
}

impl ShellArray {
    /// Create a new empty array.
    pub fn new() -> Self {
        Self { elements: Vec::new() }
    }

    /// Create from a list of values.
    pub fn from_values(vals: &[&str]) -> Self {
        Self { elements: vals.iter().map(|s| (*s).to_owned()).collect() }
    }

    /// Get element at index.
    pub fn get(&self, idx: usize) -> Option<&str> {
        self.elements.get(idx).map(|s| s.as_str())
    }

    /// Set element at index, extending if needed.
    pub fn set(&mut self, idx: usize, val: &str) {
        while self.elements.len() <= idx {
            self.elements.push(String::new());
        }
        self.elements[idx] = val.to_owned();
    }

    /// Append a value.
    pub fn push(&mut self, val: &str) {
        self.elements.push(val.to_owned());
    }

    /// Number of elements.
    pub fn len(&self) -> usize {
        self.elements.len()
    }

    /// All elements joined by space.
    pub fn join_all(&self) -> String {
        let mut out = String::new();
        for (i, e) in self.elements.iter().enumerate() {
            if i > 0 { out.push(' '); }
            out.push_str(e);
        }
        out
    }

    /// Get all elements as a Vec.
    pub fn all(&self) -> &[String] {
        &self.elements
    }
}

static ARRAYS: Mutex<BTreeMap<String, ShellArray>> = Mutex::new(BTreeMap::new());

/// Set an array variable.
pub fn set_array(name: &str, arr: ShellArray) {
    ARRAYS.lock().insert(name.to_owned(), arr);
}

/// Get an element from an array.
pub fn get_array_element(name: &str, idx: usize) -> Option<String> {
    ARRAYS.lock().get(name).and_then(|a| a.get(idx).map(|s| s.to_owned()))
}

/// Get array length.
pub fn get_array_len(name: &str) -> usize {
    ARRAYS.lock().get(name).map_or(0, |a| a.len())
}

/// Append to an array.
pub fn array_push(name: &str, val: &str) {
    let mut arrays = ARRAYS.lock();
    if let Some(a) = arrays.get_mut(name) {
        a.push(val);
    } else {
        let mut a = ShellArray::new();
        a.push(val);
        arrays.insert(name.to_owned(), a);
    }
}

// ── Associative Arrays ─────────────────────────────────────────────────

static ASSOC_ARRAYS: Mutex<BTreeMap<String, BTreeMap<String, String>>> =
    Mutex::new(BTreeMap::new());

/// Set a key in an associative array.
pub fn assoc_set(name: &str, key: &str, val: &str) {
    let mut aa = ASSOC_ARRAYS.lock();
    let map = aa.entry(name.to_owned()).or_insert_with(BTreeMap::new);
    map.insert(key.to_owned(), val.to_owned());
}

/// Get a value from an associative array.
pub fn assoc_get(name: &str, key: &str) -> Option<String> {
    ASSOC_ARRAYS.lock().get(name).and_then(|m| m.get(key).cloned())
}

// ── Parameter Expansion ─────────────────────────────────────────────────

/// Expand a `${...}` parameter expression.
///
/// Supports: `${var}`, `${var:-default}`, `${var:=default}`,
/// `${var:+alternate}`, `${var:?error}`, `${#var}`,
/// `${var#pattern}`, `${var##pattern}`, `${var%pattern}`, `${var%%pattern}`,
/// `${var/old/new}`, `${var//old/new}`, `${var^^}`, `${var,,}`.
pub fn expand_parameter(expr: &str) -> String {
    // Strip surrounding braces if present
    let inner = if expr.starts_with('{') && expr.ends_with('}') {
        &expr[1..expr.len() - 1]
    } else {
        expr
    };

    if inner.is_empty() {
        return String::new();
    }

    // ${#var} — string length
    if let Some(rest) = inner.strip_prefix('#') {
        // Check if it's an array length: #arr[@]
        if rest.ends_with("[@]") || rest.ends_with("[*]") {
            let aname = &rest[..rest.len() - 3];
            return format!("{}", get_array_len(aname));
        }
        let val = get_var(rest);
        return format!("{}", val.len());
    }

    // ${var^^} — uppercase
    if inner.ends_with("^^") {
        let name = &inner[..inner.len() - 2];
        let val = get_var(name);
        return to_upper(&val);
    }

    // ${var,,} — lowercase
    if inner.ends_with(",,") {
        let name = &inner[..inner.len() - 2];
        let val = get_var(name);
        return to_lower(&val);
    }

    // Find operator position
    // ${var:-default}, ${var:=default}, ${var:+alt}, ${var:?err}
    if let Some(colon_pos) = find_colon_op(inner) {
        let name = &inner[..colon_pos];
        let op = inner.as_bytes().get(colon_pos + 1).copied().unwrap_or(0);
        let arg = &inner[colon_pos + 2..];
        let val = get_var(name);

        return match op {
            b'-' => {
                if val.is_empty() { arg.to_owned() } else { val }
            }
            b'=' => {
                if val.is_empty() {
                    VARS.lock().set(name, arg);
                    arg.to_owned()
                } else {
                    val
                }
            }
            b'+' => {
                if !val.is_empty() { arg.to_owned() } else { String::new() }
            }
            b'?' => {
                if val.is_empty() {
                    crate::println!("bash: {}: {}", name, arg);
                    String::new()
                } else {
                    val
                }
            }
            _ => val,
        };
    }

    // ${var#pattern} / ${var##pattern}
    if let Some(hash_pos) = inner.find('#') {
        if hash_pos > 0 {
            let name = &inner[..hash_pos];
            let val = get_var(name);
            let rest = &inner[hash_pos + 1..];
            if let Some(pat) = rest.strip_prefix('#') {
                // longest prefix removal
                return remove_longest_prefix(&val, pat);
            } else {
                return remove_shortest_prefix(&val, rest);
            }
        }
    }

    // ${var%pattern} / ${var%%pattern}
    if let Some(pct_pos) = inner.find('%') {
        if pct_pos > 0 {
            let name = &inner[..pct_pos];
            let val = get_var(name);
            let rest = &inner[pct_pos + 1..];
            if let Some(pat) = rest.strip_prefix('%') {
                return remove_longest_suffix(&val, pat);
            } else {
                return remove_shortest_suffix(&val, rest);
            }
        }
    }

    // ${var/old/new} or ${var//old/new}
    if let Some(slash_pos) = inner.find('/') {
        if slash_pos > 0 {
            let name = &inner[..slash_pos];
            let val = get_var(name);
            let rest = &inner[slash_pos + 1..];
            if let Some(rest2) = rest.strip_prefix('/') {
                // global substitution
                if let Some(sep) = rest2.find('/') {
                    let old = &rest2[..sep];
                    let new = &rest2[sep + 1..];
                    return str_replace_all(&val, old, new);
                }
                return val;
            } else {
                // first substitution
                if let Some(sep) = rest.find('/') {
                    let old = &rest[..sep];
                    let new = &rest[sep + 1..];
                    return str_replace_first(&val, old, new);
                }
                return val;
            }
        }
    }

    // ${arr[idx]} — array element
    if let Some(bracket) = inner.find('[') {
        if inner.ends_with(']') {
            let name = &inner[..bracket];
            let idx_str = &inner[bracket + 1..inner.len() - 1];
            if idx_str == "@" || idx_str == "*" {
                let arrays = ARRAYS.lock();
                if let Some(a) = arrays.get(name) {
                    return a.join_all();
                }
                return String::new();
            }
            if let Ok(idx) = idx_str.parse::<usize>() {
                return get_array_element(name, idx).unwrap_or_default();
            }
            // Associative array
            return assoc_get(name, idx_str).unwrap_or_default();
        }
    }

    // Plain ${var}
    get_var(inner)
}

/// Find the position of a colon-operator (:-, :=, :+, :?) in a parameter expr.
fn find_colon_op(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    for i in 0..b.len() {
        if b[i] == b':' && i + 1 < b.len() {
            match b[i + 1] {
                b'-' | b'=' | b'+' | b'?' => return Some(i),
                _ => {}
            }
        }
    }
    None
}

fn get_var(name: &str) -> String {
    VARS.lock().get(name).unwrap_or_default()
}

// ── String helpers ──────────────────────────────────────────────────────

fn to_upper(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c >= 'a' && c <= 'z' {
            out.push((c as u8 - 32) as char);
        } else {
            out.push(c);
        }
    }
    out
}

fn to_lower(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c >= 'A' && c <= 'Z' {
            out.push((c as u8 + 32) as char);
        } else {
            out.push(c);
        }
    }
    out
}

fn str_replace_first(s: &str, old: &str, new: &str) -> String {
    if let Some(pos) = s.find(old) {
        let mut out = String::with_capacity(s.len());
        out.push_str(&s[..pos]);
        out.push_str(new);
        out.push_str(&s[pos + old.len()..]);
        out
    } else {
        s.to_owned()
    }
}

fn str_replace_all(s: &str, old: &str, new: &str) -> String {
    if old.is_empty() {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut start = 0;
    while let Some(pos) = s[start..].find(old) {
        out.push_str(&s[start..start + pos]);
        out.push_str(new);
        start += pos + old.len();
    }
    out.push_str(&s[start..]);
    out
}

/// Remove shortest prefix matching simple glob pattern (supports * and ?).
fn remove_shortest_prefix(s: &str, pat: &str) -> String {
    for i in 0..=s.len() {
        if simple_glob_match(pat, &s[..i]) {
            return s[i..].to_owned();
        }
    }
    s.to_owned()
}

/// Remove longest prefix matching simple glob pattern.
fn remove_longest_prefix(s: &str, pat: &str) -> String {
    for i in (0..=s.len()).rev() {
        if simple_glob_match(pat, &s[..i]) {
            return s[i..].to_owned();
        }
    }
    s.to_owned()
}

/// Remove shortest suffix matching simple glob pattern.
fn remove_shortest_suffix(s: &str, pat: &str) -> String {
    for i in (0..=s.len()).rev() {
        if simple_glob_match(pat, &s[i..]) {
            return s[..i].to_owned();
        }
    }
    s.to_owned()
}

/// Remove longest suffix matching simple glob pattern.
fn remove_longest_suffix(s: &str, pat: &str) -> String {
    for i in 0..=s.len() {
        if simple_glob_match(pat, &s[i..]) {
            return s[..i].to_owned();
        }
    }
    s.to_owned()
}

/// Minimal glob matcher: `*` matches any sequence, `?` matches one char.
fn simple_glob_match(pat: &str, s: &str) -> bool {
    let (pb, sb) = (pat.as_bytes(), s.as_bytes());
    let (plen, slen) = (pb.len(), sb.len());
    // DP with two rows
    let mut prev = vec![false; slen + 1];
    let mut curr = vec![false; slen + 1];
    prev[0] = true;
    for pi in 0..plen {
        curr[0] = prev[0] && pb[pi] == b'*';
        for si in 0..slen {
            curr[si + 1] = match pb[pi] {
                b'*' => curr[si] || prev[si + 1],
                b'?' => prev[si],
                c    => prev[si] && sb[si] == c,
            };
        }
        core::mem::swap(&mut prev, &mut curr);
        for v in curr.iter_mut() { *v = false; }
    }
    prev[slen]
}

// ── Arithmetic Expansion ────────────────────────────────────────────────

/// Evaluate an arithmetic expression. Integer only (i64).
///
/// Supports: +, -, *, /, %, ** (power), ==, !=, <, >, <=, >=, &&, ||, !
/// Variables are resolved from shell state.
pub fn eval_arithmetic(expr: &str) -> Result<i64, &'static str> {
    let tokens = arith_tokenize(expr);
    if tokens.is_empty() {
        return Ok(0);
    }
    let mut pos = 0;
    let result = arith_or(&tokens, &mut pos);
    Ok(result)
}

#[derive(Clone, Debug)]
enum ArithTok {
    Num(i64),
    Op(u8),       // single-char op: + - * / % ( ) !
    Op2([u8; 2]), // two-char op: ** == != <= >= && ||
}

fn arith_tokenize(s: &str) -> Vec<ArithTok> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' | b'\n' => { i += 1; }
            b'0'..=b'9' => {
                let start = i;
                while i < b.len() && b[i].is_ascii_digit() { i += 1; }
                let n = parse_i64(&s[start..i]);
                out.push(ArithTok::Num(n));
            }
            b'$' => {
                i += 1;
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') { i += 1; }
                let name = &s[start..i];
                let val = get_var(name);
                out.push(ArithTok::Num(parse_i64(&val)));
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') { i += 1; }
                let name = &s[start..i];
                let val = get_var(name);
                out.push(ArithTok::Num(parse_i64(&val)));
            }
            b'*' if i + 1 < b.len() && b[i + 1] == b'*' => {
                out.push(ArithTok::Op2([b'*', b'*'])); i += 2;
            }
            b'=' if i + 1 < b.len() && b[i + 1] == b'=' => {
                out.push(ArithTok::Op2([b'=', b'='])); i += 2;
            }
            b'!' if i + 1 < b.len() && b[i + 1] == b'=' => {
                out.push(ArithTok::Op2([b'!', b'='])); i += 2;
            }
            b'<' if i + 1 < b.len() && b[i + 1] == b'=' => {
                out.push(ArithTok::Op2([b'<', b'='])); i += 2;
            }
            b'>' if i + 1 < b.len() && b[i + 1] == b'=' => {
                out.push(ArithTok::Op2([b'>', b'='])); i += 2;
            }
            b'&' if i + 1 < b.len() && b[i + 1] == b'&' => {
                out.push(ArithTok::Op2([b'&', b'&'])); i += 2;
            }
            b'|' if i + 1 < b.len() && b[i + 1] == b'|' => {
                out.push(ArithTok::Op2([b'|', b'|'])); i += 2;
            }
            c @ (b'+' | b'-' | b'*' | b'/' | b'%' | b'(' | b')' | b'!' | b'<' | b'>') => {
                out.push(ArithTok::Op(c)); i += 1;
            }
            _ => { i += 1; }
        }
    }
    out
}

fn parse_i64(s: &str) -> i64 {
    s.trim().parse::<i64>().unwrap_or(0)
}

fn arith_or(t: &[ArithTok], p: &mut usize) -> i64 {
    let mut v = arith_and(t, p);
    while *p < t.len() {
        if matches!(t[*p], ArithTok::Op2([b'|', b'|'])) {
            *p += 1;
            let r = arith_and(t, p);
            v = if v != 0 || r != 0 { 1 } else { 0 };
        } else {
            break;
        }
    }
    v
}

fn arith_and(t: &[ArithTok], p: &mut usize) -> i64 {
    let mut v = arith_cmp(t, p);
    while *p < t.len() {
        if matches!(t[*p], ArithTok::Op2([b'&', b'&'])) {
            *p += 1;
            let r = arith_cmp(t, p);
            v = if v != 0 && r != 0 { 1 } else { 0 };
        } else {
            break;
        }
    }
    v
}

fn arith_cmp(t: &[ArithTok], p: &mut usize) -> i64 {
    let mut v = arith_add(t, p);
    while *p < t.len() {
        match &t[*p] {
            ArithTok::Op2([b'=', b'=']) => { *p += 1; let r = arith_add(t, p); v = if v == r { 1 } else { 0 }; }
            ArithTok::Op2([b'!', b'=']) => { *p += 1; let r = arith_add(t, p); v = if v != r { 1 } else { 0 }; }
            ArithTok::Op2([b'<', b'=']) => { *p += 1; let r = arith_add(t, p); v = if v <= r { 1 } else { 0 }; }
            ArithTok::Op2([b'>', b'=']) => { *p += 1; let r = arith_add(t, p); v = if v >= r { 1 } else { 0 }; }
            ArithTok::Op(b'<') => { *p += 1; let r = arith_add(t, p); v = if v < r { 1 } else { 0 }; }
            ArithTok::Op(b'>') => { *p += 1; let r = arith_add(t, p); v = if v > r { 1 } else { 0 }; }
            _ => break,
        }
    }
    v
}

fn arith_add(t: &[ArithTok], p: &mut usize) -> i64 {
    let mut v = arith_mul(t, p);
    while *p < t.len() {
        match &t[*p] {
            ArithTok::Op(b'+') => { *p += 1; v = v.wrapping_add(arith_mul(t, p)); }
            ArithTok::Op(b'-') => { *p += 1; v = v.wrapping_sub(arith_mul(t, p)); }
            _ => break,
        }
    }
    v
}

fn arith_mul(t: &[ArithTok], p: &mut usize) -> i64 {
    let mut v = arith_power(t, p);
    while *p < t.len() {
        match &t[*p] {
            ArithTok::Op(b'*') => { *p += 1; v = v.wrapping_mul(arith_power(t, p)); }
            ArithTok::Op(b'/') => { *p += 1; let d = arith_power(t, p); v = if d != 0 { v / d } else { 0 }; }
            ArithTok::Op(b'%') => { *p += 1; let d = arith_power(t, p); v = if d != 0 { v % d } else { 0 }; }
            _ => break,
        }
    }
    v
}

fn arith_power(t: &[ArithTok], p: &mut usize) -> i64 {
    let base = arith_unary(t, p);
    if *p < t.len() && matches!(t[*p], ArithTok::Op2([b'*', b'*'])) {
        *p += 1;
        let exp = arith_unary(t, p);
        return int_pow(base, exp);
    }
    base
}

fn arith_unary(t: &[ArithTok], p: &mut usize) -> i64 {
    if *p >= t.len() { return 0; }
    match &t[*p] {
        ArithTok::Op(b'-') => { *p += 1; -arith_unary(t, p) }
        ArithTok::Op(b'!') => { *p += 1; if arith_unary(t, p) == 0 { 1 } else { 0 } }
        ArithTok::Op(b'(') => {
            *p += 1;
            let v = arith_or(t, p);
            if *p < t.len() && matches!(t[*p], ArithTok::Op(b')')) { *p += 1; }
            v
        }
        ArithTok::Num(n) => { let v = *n; *p += 1; v }
        _ => { *p += 1; 0 }
    }
}

/// Integer exponentiation (no floating point).
fn int_pow(mut base: i64, mut exp: i64) -> i64 {
    if exp < 0 { return 0; }
    let mut result: i64 = 1;
    while exp > 0 {
        if exp & 1 == 1 {
            result = result.wrapping_mul(base);
        }
        base = base.wrapping_mul(base);
        exp >>= 1;
    }
    result
}

// ── Test/Conditional Expressions ────────────────────────────────────────

/// Evaluate a `[[ ... ]]` test expression.
///
/// Supports: -f, -d, -z, -n, ==, !=, =~, -eq, -ne, -lt, -gt, -le, -ge,
/// &&, ||.
pub fn eval_test(expr: &str) -> bool {
    let tokens = test_tokenize(expr);
    if tokens.is_empty() {
        return false;
    }
    let mut pos = 0;
    test_or(&tokens, &mut pos)
}

fn test_tokenize(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut i = 0;
    let b = s.as_bytes();
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' => { i += 1; }
            b'&' if i + 1 < b.len() && b[i + 1] == b'&' => {
                tokens.push("&&".to_owned()); i += 2;
            }
            b'|' if i + 1 < b.len() && b[i + 1] == b'|' => {
                tokens.push("||".to_owned()); i += 2;
            }
            b'"' | b'\'' => {
                let q = b[i]; i += 1;
                let start = i;
                while i < b.len() && b[i] != q { i += 1; }
                tokens.push(s[start..i].to_owned());
                if i < b.len() { i += 1; }
            }
            _ => {
                let start = i;
                while i < b.len() && b[i] != b' ' && b[i] != b'\t'
                    && !(b[i] == b'&' && i + 1 < b.len() && b[i + 1] == b'&')
                    && !(b[i] == b'|' && i + 1 < b.len() && b[i + 1] == b'|')
                {
                    i += 1;
                }
                let w = &s[start..i];
                if w == "[[" || w == "]]" { /* skip delimiters */ }
                else { tokens.push(w.to_owned()); }
            }
        }
    }
    tokens
}

fn test_or(t: &[String], p: &mut usize) -> bool {
    let mut v = test_and(t, p);
    while *p < t.len() && t[*p] == "||" {
        *p += 1;
        let r = test_and(t, p);
        v = v || r;
    }
    v
}

fn test_and(t: &[String], p: &mut usize) -> bool {
    let mut v = test_primary(t, p);
    while *p < t.len() && t[*p] == "&&" {
        *p += 1;
        let r = test_primary(t, p);
        v = v && r;
    }
    v
}

fn test_primary(t: &[String], p: &mut usize) -> bool {
    if *p >= t.len() { return false; }

    let tok = t[*p].as_str();
    match tok {
        "!" => {
            *p += 1;
            !test_primary(t, p)
        }
        "-f" => {
            *p += 1;
            if *p < t.len() { let path = &t[*p]; *p += 1; file_exists(path) } else { false }
        }
        "-d" => {
            *p += 1;
            if *p < t.len() { let path = &t[*p]; *p += 1; dir_exists(path) } else { false }
        }
        "-e" => {
            *p += 1;
            if *p < t.len() { let path = &t[*p]; *p += 1; file_exists(path) || dir_exists(path) } else { false }
        }
        "-z" => {
            *p += 1;
            if *p < t.len() { let s = &t[*p]; *p += 1; s.is_empty() } else { true }
        }
        "-n" => {
            *p += 1;
            if *p < t.len() { let s = &t[*p]; *p += 1; !s.is_empty() } else { false }
        }
        _ => {
            // Could be: str1 == str2, str1 != str2, str1 =~ regex,
            //           n1 -eq n2, etc.
            let left = &t[*p]; *p += 1;
            if *p >= t.len() {
                // single word: true if non-empty
                return !left.is_empty();
            }
            let op = &t[*p]; *p += 1;
            if *p >= t.len() { return false; }
            let right = &t[*p]; *p += 1;

            match op.as_str() {
                "==" | "=" => left == right,
                "!=" => left != right,
                "=~" => simple_regex_match(right, left),
                "-eq" => parse_i64(left) == parse_i64(right),
                "-ne" => parse_i64(left) != parse_i64(right),
                "-lt" => parse_i64(left) < parse_i64(right),
                "-gt" => parse_i64(left) > parse_i64(right),
                "-le" => parse_i64(left) <= parse_i64(right),
                "-ge" => parse_i64(left) >= parse_i64(right),
                _ => false,
            }
        }
    }
}

fn file_exists(path: &str) -> bool {
    crate::vfs::cat(path).is_ok()
}

fn dir_exists(path: &str) -> bool {
    crate::vfs::ls(path).is_ok()
}

/// Simple regex match: supports basic patterns (literal, ., *, ^, $).
fn simple_regex_match(pattern: &str, text: &str) -> bool {
    if let Ok(re) = crate::regex::Regex::compile(pattern) {
        re.is_match(text)
    } else {
        // Fallback: literal substring match
        text.contains(pattern)
    }
}

// ── Case Statement ──────────────────────────────────────────────────────

/// Evaluate a case statement.
///
/// `var` is the value to match.  `cases` is a list of (patterns, body)
/// where patterns is a Vec of alternative patterns and body is a string
/// to return.  Returns the body of the first matching case, or None.
pub fn eval_case(var: &str, cases: &[(Vec<String>, String)]) -> Option<String> {
    for (patterns, body) in cases {
        for pat in patterns {
            if pat == "*" || pat == var || simple_glob_match(pat, var) {
                return Some(body.clone());
            }
        }
    }
    None
}

// ── Here Documents ──────────────────────────────────────────────────────

/// Process a here-document.
///
/// Performs variable expansion on `content` unless `delimiter` is quoted
/// (e.g. `'EOF'` or `"EOF"`).  Returns the expanded text.
pub fn process_heredoc(delimiter: &str, content: &str) -> String {
    // If delimiter is quoted, no expansion
    let quoted = delimiter.starts_with('\'') || delimiter.starts_with('"');
    if quoted {
        return content.to_owned();
    }
    // Perform variable expansion on each line
    let mut out = String::new();
    for line in content.split('\n') {
        out.push_str(&expand_variables(line));
        out.push('\n');
    }
    // Remove trailing newline if content didn't end with one
    if !content.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Expand `$VAR` and `${VAR}` references in a string.
fn expand_variables(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' && i + 1 < b.len() {
            if b[i + 1] == b'{' {
                // Find closing brace
                if let Some(end) = s[i + 2..].find('}') {
                    let expr = &s[i + 2..i + 2 + end];
                    out.push_str(&expand_parameter(&format!("{{{}}}", expr)));
                    i = i + 3 + end;
                    continue;
                }
            }
            // Simple $VAR
            let start = i + 1;
            let mut end = start;
            while end < b.len() && (b[end].is_ascii_alphanumeric() || b[end] == b'_') {
                end += 1;
            }
            if end > start {
                let name = &s[start..end];
                out.push_str(&get_var(name));
                i = end;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

// ── Command Substitution ────────────────────────────────────────────────

/// Detect and handle `$(command)` and `` `command` `` substitution.
/// Returns the input with substitutions replaced by their (simulated) output.
pub fn expand_command_subst(s: &str) -> String {
    let mut out = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' && i + 1 < b.len() && b[i + 1] == b'(' {
            // Find matching close paren
            let mut depth = 1;
            let mut j = i + 2;
            while j < b.len() && depth > 0 {
                if b[j] == b'(' { depth += 1; }
                if b[j] == b')' { depth -= 1; }
                j += 1;
            }
            let cmd = &s[i + 2..j - 1];
            // Execute the command; output captured via shell dispatch
            crate::shell::dispatch(cmd);
            // In kernel mode we can't easily capture output, so we leave a marker
            out.push_str("[cmd]");
            i = j;
            continue;
        }
        if b[i] == b'`' {
            let start = i + 1;
            if let Some(end) = s[start..].find('`') {
                let cmd = &s[start..start + end];
                crate::shell::dispatch(cmd);
                out.push_str("[cmd]");
                i = start + end + 1;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

// ── Process Control ─────────────────────────────────────────────────────

/// Execute a command in the background (non-blocking).
pub fn exec_background(cmd: &str) {
    // Dispatch immediately — in kernel mode, true background is limited
    crate::shell::dispatch(cmd);
    crate::println!("[background] started: {}", cmd);
}

/// Wait for all background tasks.
pub fn wait_all() {
    crate::task::yield_now();
    crate::println!("[wait] all background jobs complete");
}

// ── Signal Trapping ─────────────────────────────────────────────────────

static TRAPS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// Register a trap handler.
pub fn trap_set(signal: &str, action: &str) {
    TRAPS.lock().insert(signal.to_owned(), action.to_owned());
}

/// Get the trap action for a signal.
pub fn trap_get(signal: &str) -> Option<String> {
    TRAPS.lock().get(signal).cloned()
}

// ── Builtins ────────────────────────────────────────────────────────────

/// Execute a builtin command.  Returns `Some(exit_status)` if the command
/// was a recognized builtin, `None` otherwise.
pub fn run_builtin(args: &[&str]) -> Option<i64> {
    if args.is_empty() { return None; }
    match args[0] {
        "echo" => {
            Some(builtin_echo(args))
        }
        "printf" => {
            Some(builtin_printf(args))
        }
        "read" => {
            Some(builtin_read(args))
        }
        "export" => {
            Some(builtin_export(args))
        }
        "unset" => {
            Some(builtin_unset(args))
        }
        "local" => {
            Some(builtin_local(args))
        }
        "return" => {
            // handled by shell_lang, but recognize it
            Some(0)
        }
        "shift" => {
            Some(builtin_shift(args))
        }
        "trap" => {
            Some(builtin_trap(args))
        }
        "type" => {
            Some(builtin_type(args))
        }
        "source" | "." => {
            Some(builtin_source(args))
        }
        "let" => {
            Some(builtin_let(args))
        }
        "test" | "[" => {
            let expr = args[1..].join(" ");
            Some(if eval_test(&expr) { 0 } else { 1 })
        }
        "set" => {
            Some(builtin_set(args))
        }
        _ => None,
    }
}

fn builtin_echo(args: &[&str]) -> i64 {
    let mut no_newline = false;
    let mut escape = false;
    let mut start = 1;

    while start < args.len() {
        match args[start] {
            "-n" => { no_newline = true; start += 1; }
            "-e" => { escape = true; start += 1; }
            "-ne" | "-en" => { no_newline = true; escape = true; start += 1; }
            _ => break,
        }
    }

    let text = args[start..].join(" ");
    if escape {
        let expanded = expand_echo_escapes(&text);
        crate::print!("{}", expanded);
    } else {
        crate::print!("{}", text);
    }
    if !no_newline {
        crate::println!();
    }
    0
}

/// Expand echo -e escape sequences.
fn expand_echo_escapes(s: &str) -> String {
    let mut out = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            match b[i + 1] {
                b'n' => { out.push('\n'); i += 2; }
                b't' => { out.push('\t'); i += 2; }
                b'r' => { out.push('\r'); i += 2; }
                b'\\' => { out.push('\\'); i += 2; }
                b'a' => { out.push('\x07'); i += 2; }
                b'b' => { out.push('\x08'); i += 2; }
                b'0' => {
                    // Octal: \0nnn
                    i += 2;
                    let mut val: u8 = 0;
                    let mut count = 0;
                    while i < b.len() && b[i] >= b'0' && b[i] <= b'7' && count < 3 {
                        val = val * 8 + (b[i] - b'0');
                        i += 1;
                        count += 1;
                    }
                    out.push(val as char);
                }
                _ => { out.push('\\'); out.push(b[i + 1] as char); i += 2; }
            }
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

fn builtin_printf(args: &[&str]) -> i64 {
    if args.len() < 2 { return 1; }
    let fmt = args[1];
    let mut arg_idx = 2;
    let mut out = String::new();
    let b = fmt.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 1 < b.len() {
            let arg_val = if arg_idx < args.len() { args[arg_idx] } else { "" };
            match b[i + 1] {
                b's' => { out.push_str(arg_val); arg_idx += 1; i += 2; }
                b'd' => { out.push_str(&format!("{}", parse_i64(arg_val))); arg_idx += 1; i += 2; }
                b'%' => { out.push('%'); i += 2; }
                b'x' => {
                    let n = parse_i64(arg_val);
                    out.push_str(&format!("{:x}", n));
                    arg_idx += 1; i += 2;
                }
                b'o' => {
                    let n = parse_i64(arg_val);
                    out.push_str(&format!("{:o}", n));
                    arg_idx += 1; i += 2;
                }
                _ => { out.push(b[i] as char); i += 1; }
            }
        } else if b[i] == b'\\' && i + 1 < b.len() {
            match b[i + 1] {
                b'n' => { out.push('\n'); i += 2; }
                b't' => { out.push('\t'); i += 2; }
                _ => { out.push(b[i + 1] as char); i += 2; }
            }
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    crate::print!("{}", out);
    0
}

fn builtin_read(args: &[&str]) -> i64 {
    // Simplified: read -p prompt var
    let mut var_name = "REPLY";
    let mut idx = 1;
    while idx < args.len() {
        match args[idx] {
            "-p" => { idx += 1; /* skip prompt text */ idx += 1; }
            "-r" => { idx += 1; } // raw mode (ignore backslash)
            _ => { var_name = args[idx]; break; }
        }
    }
    // In kernel mode we can't read interactively, store empty
    VARS.lock().set(var_name, "");
    0
}

fn builtin_export(args: &[&str]) -> i64 {
    for arg in &args[1..] {
        if let Some(eq) = arg.find('=') {
            let key = &arg[..eq];
            let val = &arg[eq + 1..];
            VARS.lock().export(key, val);
        } else {
            // Export existing variable
            let val = get_var(arg);
            VARS.lock().export(arg, &val);
        }
    }
    0
}

fn builtin_unset(args: &[&str]) -> i64 {
    for arg in &args[1..] {
        VARS.lock().unset(arg);
    }
    0
}

fn builtin_local(args: &[&str]) -> i64 {
    for arg in &args[1..] {
        if let Some(eq) = arg.find('=') {
            VARS.lock().set_local(&arg[..eq], &arg[eq + 1..]);
        } else {
            VARS.lock().set_local(arg, "");
        }
    }
    0
}

fn builtin_shift(args: &[&str]) -> i64 {
    let n = if args.len() > 1 { parse_i64(args[1]) as usize } else { 1 };
    let mut vars = VARS.lock();
    if n <= vars.positional.len() {
        vars.positional.drain(..n);
    }
    0
}

fn builtin_trap(args: &[&str]) -> i64 {
    if args.len() < 3 {
        // List traps
        let traps = TRAPS.lock();
        for (sig, action) in traps.iter() {
            crate::println!("trap -- '{}' {}", action, sig);
        }
        return 0;
    }
    let action = args[1];
    for sig in &args[2..] {
        trap_set(sig, action);
    }
    0
}

fn builtin_type(args: &[&str]) -> i64 {
    for arg in &args[1..] {
        let kind = classify_command(arg);
        crate::println!("{} is {}", arg, kind);
    }
    0
}

fn classify_command(name: &str) -> &'static str {
    // Check builtins
    let builtins = [
        "echo", "printf", "read", "test", "[", "export", "unset",
        "local", "return", "shift", "trap", "type", "source", ".",
        "let", "set", "true", "false", "cd", "pwd", "exit",
    ];
    if builtins.contains(&name) {
        return "a shell builtin";
    }
    // Check if it's a known shell command
    "a shell command"
}

fn builtin_source(args: &[&str]) -> i64 {
    if args.len() < 2 { return 1; }
    let path = args[1];
    if let Ok(content) = crate::vfs::cat(path) {
        crate::shell_lang::execute(&content);
        0
    } else {
        crate::println!("source: {}: No such file", path);
        1
    }
}

fn builtin_let(args: &[&str]) -> i64 {
    let expr = args[1..].join(" ");
    // Handle assignment: let x=5+3
    if let Some(eq) = expr.find('=') {
        let name = expr[..eq].trim();
        let rhs = &expr[eq + 1..];
        match eval_arithmetic(rhs) {
            Ok(val) => {
                VARS.lock().set(name, &format!("{}", val));
                0
            }
            Err(_) => 1,
        }
    } else {
        match eval_arithmetic(&expr) {
            Ok(val) => if val != 0 { 0 } else { 1 },
            Err(_) => 1,
        }
    }
}

fn builtin_set(args: &[&str]) -> i64 {
    if args.len() < 2 {
        // Show all options
        let opts = OPTIONS.lock();
        crate::println!("errexit  (set -e)  {}", if opts.errexit  { "on" } else { "off" });
        crate::println!("xtrace   (set -x)  {}", if opts.xtrace   { "on" } else { "off" });
        crate::println!("nounset  (set -u)  {}", if opts.nounset  { "on" } else { "off" });
        crate::println!("pipefail           {}", if opts.pipefail { "on" } else { "off" });
        return 0;
    }
    for arg in &args[1..] {
        match *arg {
            "-e" => OPTIONS.lock().errexit = true,
            "+e" => OPTIONS.lock().errexit = false,
            "-x" => OPTIONS.lock().xtrace = true,
            "+x" => OPTIONS.lock().xtrace = false,
            "-u" => OPTIONS.lock().nounset = true,
            "+u" => OPTIONS.lock().nounset = false,
            "-o" => {
                // Next arg is the option name — handled below
            }
            "+o" => {}
            "pipefail" => OPTIONS.lock().pipefail = true,
            _ => {}
        }
    }
    // Handle "set -o pipefail"
    let joined = args[1..].join(" ");
    if joined.contains("-o pipefail") {
        OPTIONS.lock().pipefail = true;
    }
    if joined.contains("+o pipefail") {
        OPTIONS.lock().pipefail = false;
    }
    0
}

// ── Zsh-specific Features ───────────────────────────────────────────────

static ZSH_OPTIONS: Mutex<BTreeMap<String, bool>> = Mutex::new(BTreeMap::new());

/// `setopt` — set a Zsh shell option.
pub fn setopt(name: &str) {
    ZSH_OPTIONS.lock().insert(name.to_owned(), true);
}

/// `unsetopt` — unset a Zsh shell option.
pub fn unsetopt(name: &str) {
    ZSH_OPTIONS.lock().insert(name.to_owned(), false);
}

/// Check if a Zsh option is set.
pub fn is_option_set(name: &str) -> bool {
    ZSH_OPTIONS.lock().get(name).copied().unwrap_or(false)
}

/// Apply glob qualifiers (Zsh feature): `*(.)` files only, `*(/)` dirs only.
pub fn apply_glob_qualifier(pattern: &str) -> Vec<String> {
    let mut results = Vec::new();
    // Check for trailing qualifier
    if pattern.ends_with("(.)") {
        let base = &pattern[..pattern.len() - 3];
        // List directory and filter to files
        if let Ok(entries) = crate::vfs::ls(base) {
            for (name, type_char) in entries {
                if type_char == '-' {
                    results.push(name);
                }
            }
        }
    } else if pattern.ends_with("(/)") {
        let base = &pattern[..pattern.len() - 3];
        if let Ok(entries) = crate::vfs::ls(base) {
            for (name, type_char) in entries {
                if type_char == 'd' {
                    results.push(name);
                }
            }
        }
    }
    results
}

// ── Interactive Features ────────────────────────────────────────────────

/// Expand PS1-style prompt escapes.
///
/// `\u` — username, `\h` — hostname, `\w` — working directory,
/// `\$` — # for root, $ for user, `\n` — newline,
/// `\[\e[NNm\]` — ANSI color (passed through).
pub fn expand_prompt(ps1: &str) -> String {
    let mut out = String::new();
    let b = ps1.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            match b[i + 1] {
                b'u' => {
                    out.push_str(&get_var_or("USER", "root"));
                    i += 2;
                }
                b'h' => {
                    let host = get_var_or("HOSTNAME", "merlion");
                    // Short hostname (before first dot)
                    if let Some(dot) = host.find('.') {
                        out.push_str(&host[..dot]);
                    } else {
                        out.push_str(&host);
                    }
                    i += 2;
                }
                b'H' => {
                    out.push_str(&get_var_or("HOSTNAME", "merlion"));
                    i += 2;
                }
                b'w' => {
                    out.push_str(&get_var_or("PWD", "/"));
                    i += 2;
                }
                b'W' => {
                    let pwd = get_var_or("PWD", "/");
                    if let Some(slash) = pwd.rfind('/') {
                        out.push_str(&pwd[slash + 1..]);
                    } else {
                        out.push_str(&pwd);
                    }
                    i += 2;
                }
                b'$' => {
                    let uid = get_var_or("UID", "0");
                    out.push(if uid == "0" { '#' } else { '$' });
                    i += 2;
                }
                b'n' => { out.push('\n'); i += 2; }
                b'[' => { i += 2; } // begin non-printing
                b']' => { i += 2; } // end non-printing
                b'e' => { out.push('\x1b'); i += 2; }
                _ => { out.push(b[i + 1] as char); i += 2; }
            }
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

fn get_var_or(name: &str, default: &str) -> String {
    let v = get_var(name);
    if v.is_empty() {
        crate::env::get(name).unwrap_or_else(|| default.to_owned())
    } else {
        v
    }
}

/// Expand history references: `!!` (last command), `!n` (command n).
pub fn expand_history(input: &str, last_cmd: &str) -> String {
    if input == "!!" {
        return last_cmd.to_owned();
    }
    if input.starts_with('!') && input.len() > 1 {
        let rest = &input[1..];
        if rest.chars().all(|c| c.is_ascii_digit()) {
            // !n — we can't actually look up by number without history access
            return input.to_owned();
        }
    }
    // ^old^new — quick substitution
    if input.starts_with('^') {
        let parts: Vec<&str> = input[1..].splitn(3, '^').collect();
        if parts.len() >= 2 {
            return str_replace_first(last_cmd, parts[0], parts[1]);
        }
    }
    input.to_owned()
}

// ── Script Execution ────────────────────────────────────────────────────

/// Detect shebang and determine shell mode.
pub fn detect_shebang(script: &str) -> Option<ShellMode> {
    if let Some(first_line) = script.lines().next() {
        if first_line.starts_with("#!") {
            let shebang = first_line[2..].trim();
            if shebang.contains("bash") {
                return Some(ShellMode::Bash);
            } else if shebang.contains("zsh") {
                return Some(ShellMode::Zsh);
            } else if shebang.contains("sh") {
                return Some(ShellMode::Sh);
            }
        }
    }
    None
}

/// Check if xtrace (set -x) is enabled and trace a command.
pub fn trace_command(cmd: &str) {
    if OPTIONS.lock().xtrace {
        crate::serial_println!("+ {}", cmd);
        crate::println!("+ {}", cmd);
    }
}

/// Check if errexit (set -e) is enabled.
pub fn check_errexit(status: i64) -> bool {
    if OPTIONS.lock().errexit && status != 0 {
        crate::println!("bash: exit on error (status {})", status);
        return true; // should exit
    }
    false
}

/// Check if nounset (set -u) is enabled.
pub fn check_nounset(name: &str) -> bool {
    if OPTIONS.lock().nounset {
        let val = get_var(name);
        if val.is_empty() && crate::env::get(name).is_none() {
            crate::println!("bash: {}: unbound variable", name);
            return true; // error
        }
    }
    false
}

// ── Global State & API ──────────────────────────────────────────────────

/// Return a human-readable info string about the bash module.
pub fn bash_info() -> String {
    let mode = get_mode();
    let mode_str = match mode {
        ShellMode::Bash => "bash",
        ShellMode::Zsh  => "zsh",
        ShellMode::Sh   => "sh",
    };
    let opts = OPTIONS.lock();
    format!(
        "Shell mode: {}\nOptions: errexit={} xtrace={} nounset={} pipefail={}\nArrays: {} indexed, {} associative",
        mode_str,
        opts.errexit, opts.xtrace, opts.nounset, opts.pipefail,
        ARRAYS.lock().len(),
        ASSOC_ARRAYS.lock().len(),
    )
}

/// Initialize the bash module.
pub fn init() {
    set_mode(ShellMode::Bash);
    // Set default variables
    let mut vars = VARS.lock();
    vars.set("BASH_VERSION", "5.2.0-merlion");
    vars.set("SHELL", "/bin/bash");
    vars.set("HISTSIZE", "1000");
    vars.set("HISTFILE", "/tmp/.bash_history");
    vars.set("PS1", "\\u@\\h:\\w\\$ ");
    vars.set("PS2", "> ");
}

// ── Shell command handlers (called from shell.rs dispatch) ──────────────

/// Handle `bash` command — switch to bash mode.
pub fn cmd_bash() {
    set_mode(ShellMode::Bash);
    crate::println!("Switched to bash mode ({})", "5.2.0-merlion");
}

/// Handle `zsh` command — switch to zsh mode.
pub fn cmd_zsh() {
    set_mode(ShellMode::Zsh);
    crate::println!("Switched to zsh mode");
}

/// Handle `sh` command — switch to POSIX sh mode.
pub fn cmd_sh() {
    set_mode(ShellMode::Sh);
    crate::println!("Switched to POSIX sh mode");
}

/// Handle `set` command from shell dispatch.
pub fn cmd_set(args: &str) {
    let parts: Vec<&str> = args.split_whitespace().collect();
    let parts_with_set: Vec<&str> = core::iter::once("set").chain(parts.iter().copied()).collect();
    builtin_set(&parts_with_set);
}

/// Handle `let` command from shell dispatch.
pub fn cmd_let(args: &str) {
    let parts: Vec<&str> = args.split_whitespace().collect();
    let parts_with_let: Vec<&str> = core::iter::once("let").chain(parts.iter().copied()).collect();
    builtin_let(&parts_with_let);
}

/// Handle `test` command from shell dispatch.
pub fn cmd_test(args: &str) {
    let result = eval_test(args);
    crate::println!("{}", if result { "true (0)" } else { "false (1)" });
}

/// Handle `type` command from shell dispatch.
pub fn cmd_type(args: &str) {
    let parts: Vec<&str> = args.split_whitespace().collect();
    let parts_with_type: Vec<&str> = core::iter::once("type").chain(parts.iter().copied()).collect();
    builtin_type(&parts_with_type);
}

/// Handle `export` command from shell dispatch.
pub fn cmd_export(args: &str) {
    let parts: Vec<&str> = args.split_whitespace().collect();
    let parts_with_export: Vec<&str> = core::iter::once("export").chain(parts.iter().copied()).collect();
    builtin_export(&parts_with_export);
}
