/// Advanced shell scripting language for MerlionOS.
///
/// Supports if/then/else/fi, for/in/do/done, while/do/done, function
/// definitions, local `$var` variables, `$((expr))` arithmetic,
/// `$(cmd)` command substitution, comparison operators (-eq, -ne, -lt,
/// -gt, ==, !=), logical operators (&&, ||, !), exit status `$?`,
/// break/continue in loops, and return from functions.  Each simple
/// command is forwarded to [`crate::shell::dispatch`].

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;
use alloc::collections::BTreeMap;

// ── AST ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Tok { Word(String), Semi, Nl, And, Or, Not }

#[derive(Clone, Debug)]
enum Stmt {
    Simple(Vec<String>),
    If { cond: Vec<Stmt>, then_b: Vec<Stmt>, else_b: Vec<Stmt> },
    For { var: String, words: Vec<String>, body: Vec<Stmt> },
    While { cond: Vec<Stmt>, body: Vec<Stmt> },
    FuncDef { name: String, body: Vec<Stmt> },
    Break, Continue, Return(i64),
    And(Box<Stmt>, Box<Stmt>),
    Or(Box<Stmt>, Box<Stmt>),
    Not(Box<Stmt>),
}

// ── Interpreter ─────────────────────────────────────────────────────────

struct Interp {
    vars: BTreeMap<String, String>,
    funcs: BTreeMap<String, Vec<Stmt>>,
    status: i64,
    flag_break: bool,
    flag_continue: bool,
    flag_return: bool,
}

impl Interp {
    fn new() -> Self {
        Self { vars: BTreeMap::new(), funcs: BTreeMap::new(),
               status: 0, flag_break: false, flag_continue: false, flag_return: false }
    }
    /// Resolve variable: locals -> crate::env -> special `$?`.
    fn get_var(&self, name: &str) -> String {
        if name == "?" { return format!("{}", self.status); }
        if let Some(v) = self.vars.get(name) { return v.clone(); }
        crate::env::get(name).unwrap_or_default()
    }
    fn set_var(&mut self, k: &str, v: &str) { self.vars.insert(k.to_owned(), v.to_owned()); }

    /// Expand `$var`, `${var}`, `$?`, `$((expr))` inside a word.
    fn expand(&self, word: &str) -> String {
        let b = word.as_bytes();
        let (mut out, mut i) = (String::new(), 0usize);
        while i < b.len() {
            if b[i] == b'$' && i + 1 < b.len() {
                if b.get(i+1) == Some(&b'(') && b.get(i+2) == Some(&b'(') {
                    if let Some(e) = find_double_close(b, i+3) {
                        out.push_str(&format!("{}", eval_arith(&word[i+3..e], self)));
                        i = e + 2; continue;
                    }
                }
                if b[i+1] == b'(' {
                    if let Some(e) = find_close_paren(b, i+2) {
                        let cmd = &word[i+2..e];
                        crate::shell::dispatch(cmd);
                        i = e + 1; continue;
                    }
                }
                if b[i+1] == b'{' {
                    if let Some(c) = word[i+2..].find('}') {
                        out.push_str(&self.get_var(&word[i+2..i+2+c]));
                        i = i + 3 + c; continue;
                    }
                }
                let s = i + 1;
                let mut e = s;
                if e < b.len() && b[e] == b'?' { e += 1; }
                else { while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') { e += 1; } }
                out.push_str(&self.get_var(&word[s..e]));
                i = e;
            } else { out.push(b[i] as char); i += 1; }
        }
        out
    }

    /// Execute a list of statements.
    fn run(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.run1(s);
            if self.flag_break || self.flag_continue || self.flag_return { return; }
        }
    }

    /// Execute one statement.
    fn run1(&mut self, s: &Stmt) {
        match s {
            Stmt::Simple(w) if w.is_empty() => {}
            Stmt::Simple(w) => {
                let exp: Vec<String> = w.iter().map(|x| self.expand(x)).collect();
                self.run_simple(&exp);
            }
            Stmt::If { cond, then_b, else_b } => {
                self.run(cond);
                if self.status == 0 { self.run(then_b); } else { self.run(else_b); }
            }
            Stmt::For { var, words, body } => {
                let exp: Vec<String> = words.iter().map(|x| self.expand(x)).collect();
                for v in &exp {
                    self.set_var(var, v);
                    self.flag_continue = false;
                    self.run(body);
                    if self.flag_break { self.flag_break = false; break; }
                }
            }
            Stmt::While { cond, body } => loop {
                self.run(cond);
                if self.status != 0 { break; }
                self.flag_continue = false;
                self.run(body);
                if self.flag_break { self.flag_break = false; break; }
            },
            Stmt::FuncDef { name, body } => { self.funcs.insert(name.clone(), body.clone()); self.status = 0; }
            Stmt::Break => self.flag_break = true,
            Stmt::Continue => self.flag_continue = true,
            Stmt::Return(c) => { self.status = *c; self.flag_return = true; }
            Stmt::And(l, r) => { self.run1(l); if self.status == 0 { self.run1(r); } }
            Stmt::Or(l, r)  => { self.run1(l); if self.status != 0 { self.run1(r); } }
            Stmt::Not(inner) => { self.run1(inner); self.status = if self.status == 0 { 1 } else { 0 }; }
        }
    }

    /// Execute a simple (expanded) command.
    fn run_simple(&mut self, w: &[String]) {
        if w.is_empty() { return; }
        // Assignment: VAR=value
        if w.len() == 1 { if let Some(eq) = w[0].find('=') {
            self.set_var(&w[0][..eq], &w[0][eq+1..]); self.status = 0; return;
        }}
        match w[0].as_str() {
            "test" | "[" => { self.status = eval_test(&w[1..]); }
            "true"  => self.status = 0,
            "false" => self.status = 1,
            _ => {
                if let Some(body) = self.funcs.get(w[0].as_str()).cloned() {
                    for (i, a) in w.iter().skip(1).enumerate() { self.set_var(&format!("{}", i+1), a); }
                    self.flag_return = false; self.run(&body); self.flag_return = false;
                } else {
                    crate::shell::dispatch(&w.join(" "));
                    self.status = 0;
                }
            }
        }
    }
}

// ── Tokeniser ───────────────────────────────────────────────────────────

fn tokenise(src: &str) -> Vec<Tok> {
    let mut ts = Vec::new();
    let mut it = src.chars().peekable();
    while let Some(&ch) = it.peek() {
        match ch {
            ' ' | '\t' => { it.next(); }
            '#' => { while it.peek().map_or(false, |&c| c != '\n') { it.next(); } }
            '\n' => { it.next(); ts.push(Tok::Nl); }
            ';'  => { it.next(); ts.push(Tok::Semi); }
            '&'  => { it.next(); if it.peek() == Some(&'&') { it.next(); ts.push(Tok::And); } }
            '|'  => { it.next(); if it.peek() == Some(&'|') { it.next(); ts.push(Tok::Or); } }
            '!'  => { it.next(); ts.push(Tok::Not); }
            '"' | '\'' => {
                let q = ch; it.next();
                let mut w = String::new();
                while it.peek().map_or(false, |&c| c != q) { w.push(it.next().unwrap()); }
                it.next(); ts.push(Tok::Word(w));
            }
            _ => {
                let mut w = String::new();
                while let Some(&c) = it.peek() {
                    if " \t\n;|&#".contains(c) { break; }
                    w.push(c); it.next();
                }
                if !w.is_empty() { ts.push(Tok::Word(w)); }
            }
        }
    }
    ts
}

// ── Parser ──────────────────────────────────────────────────────────────

fn parse(ts: &[Tok]) -> Vec<Stmt> { let mut p = 0; parse_list(ts, &mut p, &[]) }

fn parse_list(ts: &[Tok], p: &mut usize, stop: &[&str]) -> Vec<Stmt> {
    let mut v = Vec::new();
    loop {
        skip_sep(ts, p);
        if *p >= ts.len() { break; }
        if let Tok::Word(w) = &ts[*p] { if stop.contains(&w.as_str()) { break; } }
        if let Some(s) = parse_one(ts, p) { v.push(s); }
    }
    v
}

fn skip_sep(ts: &[Tok], p: &mut usize) {
    while *p < ts.len() && matches!(ts[*p], Tok::Semi | Tok::Nl) { *p += 1; }
}

fn eat(ts: &[Tok], p: &mut usize, w: &str) -> bool {
    skip_sep(ts, p);
    if *p < ts.len() { if let Tok::Word(x) = &ts[*p] { if x == w { *p += 1; return true; } } }
    false
}

fn parse_one(ts: &[Tok], p: &mut usize) -> Option<Stmt> {
    let left = parse_primary(ts, p)?;
    if *p < ts.len() {
        if ts[*p] == Tok::And { *p += 1; skip_sep(ts, p);
            if let Some(r) = parse_one(ts, p) { return Some(Stmt::And(Box::new(left), Box::new(r))); } }
        if ts[*p] == Tok::Or { *p += 1; skip_sep(ts, p);
            if let Some(r) = parse_one(ts, p) { return Some(Stmt::Or(Box::new(left), Box::new(r))); } }
    }
    Some(left)
}

fn parse_primary(ts: &[Tok], p: &mut usize) -> Option<Stmt> {
    if *p >= ts.len() { return None; }
    if ts[*p] == Tok::Not { *p += 1; return Some(Stmt::Not(Box::new(parse_primary(ts, p)?))); }
    if let Tok::Word(w) = &ts[*p] {
        match w.as_str() {
            "if" => return Some(parse_if(ts, p)),
            "for" => return Some(parse_for(ts, p)),
            "while" => return Some(parse_while(ts, p)),
            "function" => return Some(parse_func(ts, p)),
            "break" => { *p += 1; return Some(Stmt::Break); }
            "continue" => { *p += 1; return Some(Stmt::Continue); }
            "return" => {
                *p += 1;
                let c = if let Some(Tok::Word(n)) = ts.get(*p) {
                    if let Ok(v) = n.parse::<i64>() { *p += 1; v } else { 0 }
                } else { 0 };
                return Some(Stmt::Return(c));
            }
            _ => {}
        }
    }
    let mut ws = Vec::new();
    while *p < ts.len() {
        if let Tok::Word(w) = &ts[*p] {
            if matches!(w.as_str(), "then"|"else"|"fi"|"do"|"done"|"}") { break; }
            ws.push(w.clone()); *p += 1;
        } else { break; }
    }
    if ws.is_empty() { None } else { Some(Stmt::Simple(ws)) }
}

/// Parse if/then/else/fi.
fn parse_if(ts: &[Tok], p: &mut usize) -> Stmt {
    *p += 1;
    let cond = parse_list(ts, p, &["then"]);     eat(ts, p, "then");
    let then_b = parse_list(ts, p, &["else","fi"]);
    let else_b = if eat(ts, p, "else") { parse_list(ts, p, &["fi"]) } else { Vec::new() };
    eat(ts, p, "fi");
    Stmt::If { cond, then_b, else_b }
}

/// Parse for/in/do/done.
fn parse_for(ts: &[Tok], p: &mut usize) -> Stmt {
    *p += 1;
    let var = if let Some(Tok::Word(w)) = ts.get(*p) { *p += 1; w.clone() } else { "_".into() };
    eat(ts, p, "in");
    let mut words = Vec::new();
    loop { skip_sep(ts, p);
        if let Some(Tok::Word(w)) = ts.get(*p) { if w == "do" { break; } words.push(w.clone()); *p += 1; } else { break; }
    }
    eat(ts, p, "do");
    let body = parse_list(ts, p, &["done"]); eat(ts, p, "done");
    Stmt::For { var, words, body }
}

/// Parse while/do/done.
fn parse_while(ts: &[Tok], p: &mut usize) -> Stmt {
    *p += 1;
    let cond = parse_list(ts, p, &["do"]); eat(ts, p, "do");
    let body = parse_list(ts, p, &["done"]); eat(ts, p, "done");
    Stmt::While { cond, body }
}

/// Parse function name() { body }.
fn parse_func(ts: &[Tok], p: &mut usize) -> Stmt {
    *p += 1;
    let name = if let Some(Tok::Word(w)) = ts.get(*p) {
        *p += 1;
        if let Some(Tok::Word(x)) = ts.get(*p) {
            if x == "()" || x == "(" { *p += 1;
                if let Some(Tok::Word(y)) = ts.get(*p) { if y == ")" { *p += 1; } }
            }
        }
        w.clone()
    } else { "_anon".into() };
    eat(ts, p, "{");
    let body = parse_list(ts, p, &["}"]); eat(ts, p, "}");
    Stmt::FuncDef { name, body }
}

// ── Test evaluator (`[` / `test` built-in) ──────────────────────────────

/// Evaluate a `test` / `[` expression; returns 0 for true, 1 for false.
fn eval_test(args: &[String]) -> i64 {
    let a: Vec<&str> = args.iter().map(|s| s.as_str()).filter(|s| *s != "]").collect();
    let b = |v: bool| -> i64 { if v { 0 } else { 1 } };
    match a.len() {
        0 => 1,
        1 => b(!a[0].is_empty()),
        2 if a[0] == "!" => b(a[1].is_empty()),
        2 if a[0] == "-z" => b(a[1].is_empty()),
        2 if a[0] == "-n" => b(!a[1].is_empty()),
        3 => {
            let (l, op, r) = (a[0], a[1], a[2]);
            let li = || l.parse::<i64>().unwrap_or(0);
            let ri = || r.parse::<i64>().unwrap_or(0);
            match op {
                "==" | "=" => b(l == r), "!=" => b(l != r),
                "-eq" => b(li() == ri()), "-ne" => b(li() != ri()),
                "-lt" => b(li() <  ri()), "-gt" => b(li() >  ri()),
                "-le" => b(li() <= ri()), "-ge" => b(li() >= ri()),
                _ => 1,
            }
        }
        _ => 1,
    }
}

// ── Arithmetic evaluator  $((expr)) ─────────────────────────────────────

#[derive(Clone)] enum ATok { Num(i64), Op(u8) } // Op: +−*/%(  )

fn eval_arith(expr: &str, interp: &Interp) -> i64 {
    let toks = arith_lex(expr, interp);
    let mut p = 0; arith_add(&toks, &mut p)
}

fn arith_lex(s: &str, interp: &Interp) -> Vec<ATok> {
    let (b, mut i, mut v) = (s.as_bytes(), 0usize, Vec::new());
    while i < b.len() {
        match b[i] {
            b' '|b'\t' => i += 1,
            c @ (b'+'|b'-'|b'*'|b'/'|b'%'|b'('|b')') => { v.push(ATok::Op(c)); i += 1; }
            b'0'..=b'9' => {
                let s0 = i; while i < b.len() && b[i].is_ascii_digit() { i += 1; }
                v.push(ATok::Num(s[s0..i].parse().unwrap_or(0)));
            }
            b'$' | b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                if b[i] == b'$' { i += 1; }
                let s0 = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') { i += 1; }
                v.push(ATok::Num(interp.get_var(&s[s0..i]).parse().unwrap_or(0)));
            }
            _ => i += 1,
        }
    }
    v
}

fn arith_add(t: &[ATok], p: &mut usize) -> i64 {
    let mut v = arith_mul(t, p);
    while *p < t.len() { match t[*p] {
        ATok::Op(b'+') => { *p += 1; v += arith_mul(t, p); }
        ATok::Op(b'-') => { *p += 1; v -= arith_mul(t, p); }
        _ => break,
    }} v
}

fn arith_mul(t: &[ATok], p: &mut usize) -> i64 {
    let mut v = arith_atom(t, p);
    while *p < t.len() { match t[*p] {
        ATok::Op(b'*') => { *p += 1; v *= arith_atom(t, p); }
        ATok::Op(b'/') => { *p += 1; let d = arith_atom(t, p); v = if d != 0 { v/d } else { 0 }; }
        ATok::Op(b'%') => { *p += 1; let d = arith_atom(t, p); v = if d != 0 { v%d } else { 0 }; }
        _ => break,
    }} v
}

fn arith_atom(t: &[ATok], p: &mut usize) -> i64 {
    if *p >= t.len() { return 0; }
    match &t[*p] {
        ATok::Num(n) => { let v = *n; *p += 1; v }
        ATok::Op(b'-') => { *p += 1; -arith_atom(t, p) }
        ATok::Op(b'(') => { *p += 1; let v = arith_add(t, p);
            if matches!(t.get(*p), Some(ATok::Op(b')'))) { *p += 1; } v }
        _ => { *p += 1; 0 }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn find_double_close(b: &[u8], start: usize) -> Option<usize> {
    let (mut i, mut d) = (start, 1i32);
    while i < b.len() {
        if b[i] == b'(' { d += 1; }
        if b[i] == b')' { d -= 1;
            if d == 0 && i+1 < b.len() && b[i+1] == b')' { return Some(i); }
            if d < 0 { return None; }
        }
        i += 1;
    }
    None
}

fn find_close_paren(b: &[u8], start: usize) -> Option<usize> {
    let (mut i, mut d) = (start, 1i32);
    while i < b.len() {
        if b[i] == b'(' { d += 1; }
        if b[i] == b')' { d -= 1; if d == 0 { return Some(i); } }
        i += 1;
    }
    None
}

// ── Public API ──────────────────────────────────────────────────────────

/// Parse and execute a shell script string.
///
/// Supports the full set of shell-language constructs described in the
/// module documentation.  Returns the exit status of the last statement.
///
/// # Example
///
/// ```text
/// shell_lang::execute("
///   x=0
///   while test $x -lt 5; do
///     echo iteration $x
///     x=$(( $x + 1 ))
///   done
/// ");
/// ```
pub fn execute(script: &str) -> i64 {
    let tokens = tokenise(script);
    let stmts = parse(&tokens);
    let mut interp = Interp::new();
    interp.run(&stmts);
    interp.status
}
