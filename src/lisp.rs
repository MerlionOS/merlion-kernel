/// Lisp interpreter — a minimal Scheme-like language inside MerlionOS.
///
/// Adds a third scripting language alongside Forth and WASM.
/// Supports s-expression parsing, lexical environments, lambdas,
/// and standard list primitives.

use alloc::boxed::Box;
///
///   merlion> lisp
///   lisp> (+ 1 2)
///   3
///   lisp> (define square (lambda (x) (* x x)))
///   lisp> (square 7)
///   49
///   lisp> exit

use crate::{print, println, keyboard::KeyEvent};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::borrow::ToOwned;
use alloc::format;

const MAX_INPUT: usize = 256;
static RUNNING: AtomicBool = AtomicBool::new(false);
static INPUT: Mutex<InputBuf> = Mutex::new(InputBuf { buf: [0; MAX_INPUT], len: 0 });
static ENV: Mutex<Option<LispEnv>> = Mutex::new(None);
struct InputBuf { buf: [u8; MAX_INPUT], len: usize }

/// Every Lisp datum is a `LispValue`.
#[derive(Clone)]
pub enum LispValue {
    /// The empty value / false.
    Nil,
    /// 64-bit signed integer.
    Int(i64),
    /// Heap-allocated string.
    Str(String),
    /// Symbol (identifier).
    Symbol(String),
    /// Proper list stored as a Vec.
    List(Vec<LispValue>),
    /// Built-in function pointer.
    Func(fn(&[LispValue], &mut LispEnv) -> Result<LispValue, String>),
    /// User-defined lambda: (params, body, captured_env_bindings).
    Lambda(Vec<String>, Box<LispValue>, Vec<(String, LispValue)>),
}

impl core::fmt::Display for LispValue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Nil => write!(f, "nil"),
            Self::Int(n) => write!(f, "{}", n),
            Self::Str(s) => write!(f, "\"{}\"", s),
            Self::Symbol(s) => write!(f, "{}", s),
            Self::List(v) => {
                write!(f, "(")?;
                for (i, e) in v.iter().enumerate() {
                    if i > 0 { write!(f, " ")?; } write!(f, "{}", e)?;
                }
                write!(f, ")")
            }
            Self::Func(_) => write!(f, "<builtin>"),
            Self::Lambda(p, ..) => write!(f, "<lambda ({})>", p.join(" ")),
        }
    }
}

/// Environment holding variable and function bindings.
pub struct LispEnv {
    /// Linear list of (name, value) pairs; later entries shadow earlier ones.
    bindings: Vec<(String, LispValue)>,
}

impl LispEnv {
    /// Create a new environment pre-loaded with built-in functions.
    pub fn new() -> Self {
        let mut e = LispEnv { bindings: Vec::new() };
        let b: &[(&str, fn(&[LispValue], &mut LispEnv) -> Result<LispValue, String>)] = &[
            ("+", bi_add), ("-", bi_sub), ("*", bi_mul), ("/", bi_div),
            ("=", bi_eq), ("<", bi_lt), (">", bi_gt),
            ("car", bi_car), ("cdr", bi_cdr), ("cons", bi_cons),
            ("list", bi_list), ("print", bi_print),
        ];
        for &(name, func) in b { e.bindings.push((name.to_owned(), LispValue::Func(func))); }
        e
    }
    /// Look up a binding by name (searches from the end for shadowing).
    pub fn get(&self, name: &str) -> Option<LispValue> {
        self.bindings.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    }
    /// Set or overwrite a binding.
    pub fn set(&mut self, name: &str, val: LispValue) {
        for (n, v) in self.bindings.iter_mut().rev() {
            if n == name { *v = val; return; }
        }
        self.bindings.push((name.to_owned(), val));
    }
    /// Push a temporary binding (for lambda scope).
    fn push(&mut self, name: &str, val: LispValue) {
        self.bindings.push((name.to_owned(), val));
    }
    /// Pop the last n bindings.
    fn pop_n(&mut self, n: usize) {
        self.bindings.truncate(self.bindings.len().saturating_sub(n));
    }
}

/// Tokenize an s-expression string into a flat list of tokens.
fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cs = input.chars().peekable();
    while let Some(&ch) = cs.peek() {
        match ch {
            ' ' | '\t' | '\n' | '\r' => { cs.next(); }
            '(' | ')' | '\'' => { tokens.push(String::from(ch)); cs.next(); }
            '"' => {
                cs.next();
                let mut s = String::from("\"");
                while let Some(&c) = cs.peek() { cs.next(); if c == '"' { break; } s.push(c); }
                s.push('"');
                tokens.push(s);
            }
            ';' => { while let Some(&c) = cs.peek() { cs.next(); if c == '\n' { break; } } }
            _ => {
                let mut sym = String::new();
                while let Some(&c) = cs.peek() {
                    if " \t\n\r()".contains(c) { break; }
                    sym.push(c); cs.next();
                }
                tokens.push(sym);
            }
        }
    }
    tokens
}

/// Parse a string of Lisp code into a `LispValue`.
pub fn parse(input: &str) -> Result<LispValue, String> {
    let tokens = tokenize(input);
    if tokens.is_empty() { return Ok(LispValue::Nil); }
    let (val, _) = parse_at(&tokens, 0)?;
    Ok(val)
}

/// Recursive descent parser over a token slice.
fn parse_at(tokens: &[String], pos: usize) -> Result<(LispValue, usize), String> {
    if pos >= tokens.len() { return Err("unexpected end of input".to_owned()); }
    match tokens[pos].as_str() {
        "(" => {
            let (mut list, mut i) = (Vec::new(), pos + 1);
            while i < tokens.len() && tokens[i] != ")" {
                let (val, next) = parse_at(tokens, i)?;
                list.push(val); i = next;
            }
            if i >= tokens.len() { return Err("missing closing )".to_owned()); }
            Ok((LispValue::List(list), i + 1))
        }
        ")" => Err("unexpected )".to_owned()),
        "'" => {
            let (val, next) = parse_at(tokens, pos + 1)?;
            Ok((LispValue::List(vec![LispValue::Symbol("quote".to_owned()), val]), next))
        }
        _ => Ok((parse_atom(&tokens[pos]), pos + 1)),
    }
}

/// Parse a single atom (number, string, nil, or symbol).
fn parse_atom(t: &str) -> LispValue {
    if t == "nil" { LispValue::Nil }
    else if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
        LispValue::Str(t[1..t.len()-1].to_owned())
    } else if let Ok(n) = t.parse::<i64>() { LispValue::Int(n) }
    else { LispValue::Symbol(t.to_owned()) }
}

/// Evaluate a `LispValue` in the given environment.
pub fn eval(expr: &LispValue, env: &mut LispEnv) -> Result<LispValue, String> {
    match expr {
        LispValue::Nil | LispValue::Int(_) | LispValue::Str(_)
        | LispValue::Func(_) | LispValue::Lambda(..) => Ok(expr.clone()),
        LispValue::Symbol(name) =>
            env.get(name).ok_or_else(|| format!("undefined symbol: {}", name)),
        LispValue::List(elems) => {
            if elems.is_empty() { return Ok(LispValue::Nil); }
            if let LispValue::Symbol(ref s) = elems[0] {
                match s.as_str() {
                    "quote" => return sf_quote(elems),
                    "if" => return sf_if(elems, env),
                    "define" => return sf_define(elems, env),
                    "lambda" => return sf_lambda(elems, env),
                    "begin" => return sf_begin(elems, env),
                    _ => {}
                }
            }
            let func = eval(&elems[0], env)?;
            let args: Result<Vec<_>, _> = elems[1..].iter().map(|a| eval(a, env)).collect();
            apply(&func, &args?, env)
        }
    }
}

/// Apply a function (builtin or lambda) to evaluated arguments.
fn apply(func: &LispValue, args: &[LispValue], env: &mut LispEnv) -> Result<LispValue, String> {
    match func {
        LispValue::Func(f) => f(args, env),
        LispValue::Lambda(params, body, captured) => {
            if args.len() != params.len() {
                return Err(format!("expected {} args, got {}", params.len(), args.len()));
            }
            let cc = captured.len();
            for (n, v) in captured { env.push(n, v.clone()); }
            for (n, v) in params.iter().zip(args) { env.push(n, v.clone()); }
            let r = eval(body, env);
            env.pop_n(params.len() + cc);
            r
        }
        _ => Err("not a function".to_owned()),
    }
}

/// `(quote expr)` — return expr unevaluated.
fn sf_quote(e: &[LispValue]) -> Result<LispValue, String> {
    if e.len() != 2 { return Err("quote requires 1 argument".to_owned()); }
    Ok(e[1].clone())
}
/// `(if cond then else?)` — conditional evaluation.
fn sf_if(e: &[LispValue], env: &mut LispEnv) -> Result<LispValue, String> {
    if e.len() < 3 || e.len() > 4 { return Err("if requires 2 or 3 arguments".to_owned()); }
    let truthy = !matches!(eval(&e[1], env)?, LispValue::Nil | LispValue::Int(0));
    if truthy { eval(&e[2], env) }
    else if e.len() == 4 { eval(&e[3], env) }
    else { Ok(LispValue::Nil) }
}
/// `(define name value)` or `(define (f params...) body)` — bind a name.
fn sf_define(e: &[LispValue], env: &mut LispEnv) -> Result<LispValue, String> {
    if e.len() != 3 { return Err("define requires 2 arguments".to_owned()); }
    match &e[1] {
        LispValue::Symbol(name) => { let v = eval(&e[2], env)?; env.set(name, v); Ok(LispValue::Nil) }
        LispValue::List(sig) if !sig.is_empty() => {
            if let LispValue::Symbol(fname) = &sig[0] {
                let params: Vec<String> = sig[1..].iter().filter_map(|p| {
                    if let LispValue::Symbol(s) = p { Some(s.clone()) } else { None }
                }).collect();
                let cap = env.bindings.clone();
                env.set(fname, LispValue::Lambda(params, Box::new(e[2].clone()), cap));
                Ok(LispValue::Nil)
            } else { Err("define: name must be a symbol".to_owned()) }
        }
        _ => Err("define: first arg must be symbol or (name params...)".to_owned()),
    }
}
/// `(lambda (params...) body)` — create an anonymous function with closure.
fn sf_lambda(e: &[LispValue], env: &mut LispEnv) -> Result<LispValue, String> {
    if e.len() != 3 { return Err("lambda requires 2 arguments".to_owned()); }
    let params = match &e[1] {
        LispValue::List(p) => p.iter().map(|v| {
            if let LispValue::Symbol(s) = v { Ok(s.clone()) }
            else { Err("lambda params must be symbols".to_owned()) }
        }).collect::<Result<Vec<_>, _>>()?,
        _ => return Err("lambda: first arg must be param list".to_owned()),
    };
    Ok(LispValue::Lambda(params, Box::new(e[2].clone()), env.bindings.clone()))
}
/// `(begin expr1 expr2 ...)` — evaluate forms in sequence, return last.
fn sf_begin(e: &[LispValue], env: &mut LispEnv) -> Result<LispValue, String> {
    let mut r = LispValue::Nil;
    for expr in &e[1..] { r = eval(expr, env)?; }
    Ok(r)
}

/// `(+ a b ...)` — sum of integers.
fn bi_add(args: &[LispValue], _: &mut LispEnv) -> Result<LispValue, String> {
    let mut s: i64 = 0;
    for a in args { if let LispValue::Int(n) = a { s += n } else { return Err("+ requires ints".to_owned()); } }
    Ok(LispValue::Int(s))
}
/// `(- a b)` — subtraction or negation.
fn bi_sub(args: &[LispValue], _: &mut LispEnv) -> Result<LispValue, String> {
    match args {
        [LispValue::Int(a)] => Ok(LispValue::Int(-a)),
        [LispValue::Int(a), LispValue::Int(b)] => Ok(LispValue::Int(a - b)),
        _ => Err("- requires 1 or 2 ints".to_owned()),
    }
}
/// `(* a b ...)` — product of integers.
fn bi_mul(args: &[LispValue], _: &mut LispEnv) -> Result<LispValue, String> {
    let mut p: i64 = 1;
    for a in args { if let LispValue::Int(n) = a { p *= n } else { return Err("* requires ints".to_owned()); } }
    Ok(LispValue::Int(p))
}
/// `(/ a b)` — integer division.
fn bi_div(args: &[LispValue], _: &mut LispEnv) -> Result<LispValue, String> {
    match args {
        [LispValue::Int(a), LispValue::Int(b)] if *b != 0 => Ok(LispValue::Int(a / b)),
        [LispValue::Int(_), LispValue::Int(_)] => Err("division by zero".to_owned()),
        _ => Err("/ requires 2 ints".to_owned()),
    }
}
/// `(= a b)` — equality comparison.
fn bi_eq(args: &[LispValue], _: &mut LispEnv) -> Result<LispValue, String> {
    if args.len() != 2 { return Err("= requires 2 arguments".to_owned()); }
    let eq = match (&args[0], &args[1]) {
        (LispValue::Int(a), LispValue::Int(b)) => a == b,
        (LispValue::Str(a), LispValue::Str(b)) => a == b,
        (LispValue::Nil, LispValue::Nil) => true,
        _ => false,
    };
    Ok(if eq { LispValue::Int(1) } else { LispValue::Nil })
}
/// `(< a b)` — less-than comparison.
fn bi_lt(args: &[LispValue], _: &mut LispEnv) -> Result<LispValue, String> {
    match args {
        [LispValue::Int(a), LispValue::Int(b)] => Ok(if a < b { LispValue::Int(1) } else { LispValue::Nil }),
        _ => Err("< requires 2 ints".to_owned()),
    }
}
/// `(> a b)` — greater-than comparison.
fn bi_gt(args: &[LispValue], _: &mut LispEnv) -> Result<LispValue, String> {
    match args {
        [LispValue::Int(a), LispValue::Int(b)] => Ok(if a > b { LispValue::Int(1) } else { LispValue::Nil }),
        _ => Err("> requires 2 ints".to_owned()),
    }
}
/// `(car lst)` — first element of a list.
fn bi_car(args: &[LispValue], _: &mut LispEnv) -> Result<LispValue, String> {
    match args {
        [LispValue::List(v)] if !v.is_empty() => Ok(v[0].clone()),
        [LispValue::List(_)] => Err("car: empty list".to_owned()),
        _ => Err("car requires 1 list".to_owned()),
    }
}
/// `(cdr lst)` — all but the first element.
fn bi_cdr(args: &[LispValue], _: &mut LispEnv) -> Result<LispValue, String> {
    match args {
        [LispValue::List(v)] if !v.is_empty() => Ok(LispValue::List(v[1..].to_vec())),
        [LispValue::List(_)] => Ok(LispValue::Nil),
        _ => Err("cdr requires 1 list".to_owned()),
    }
}
/// `(cons head tail)` — prepend an element to a list.
fn bi_cons(args: &[LispValue], _: &mut LispEnv) -> Result<LispValue, String> {
    if args.len() != 2 { return Err("cons requires 2 arguments".to_owned()); }
    let mut v = match &args[1] {
        LispValue::List(l) => l.clone(), LispValue::Nil => Vec::new(), _ => vec![args[1].clone()],
    };
    v.insert(0, args[0].clone());
    Ok(LispValue::List(v))
}
/// `(list a b c ...)` — construct a list from arguments.
fn bi_list(args: &[LispValue], _: &mut LispEnv) -> Result<LispValue, String> {
    Ok(LispValue::List(args.to_vec()))
}
/// `(print expr ...)` — display values to the console.
fn bi_print(args: &[LispValue], _: &mut LispEnv) -> Result<LispValue, String> {
    for (i, a) in args.iter().enumerate() {
        if i > 0 { print!(" "); }
        print!("{}", a);
    }
    println!();
    Ok(LispValue::Nil)
}

/// Returns `true` if the Lisp REPL is currently active.
pub fn is_running() -> bool { RUNNING.load(Ordering::SeqCst) }

/// Enter the interactive Lisp REPL.
pub fn enter() {
    *ENV.lock() = Some(LispEnv::new());
    RUNNING.store(true, Ordering::SeqCst);
    INPUT.lock().len = 0;
    println!("\x1b[33mMerlionOS Lisp v1.0\x1b[0m");
    println!("Type expressions in parentheses, 'exit' to quit.");
    print!("lisp> ");
}

/// Handle a keyboard event while the Lisp REPL is active.
pub fn handle_input(event: KeyEvent) {
    if !RUNNING.load(Ordering::SeqCst) { return; }
    let mut input = INPUT.lock();
    match event {
        KeyEvent::Char('\n') => {
            println!();
            let line = core::str::from_utf8(&input.buf[..input.len]).unwrap_or("").trim();
            if !line.is_empty() {
                let owned = String::from(line);
                input.len = 0;
                drop(input);
                if owned == "exit" {
                    RUNNING.store(false, Ordering::SeqCst);
                    *ENV.lock() = None;
                    return;
                }
                let mut el = ENV.lock();
                if let Some(ref mut env) = *el {
                    match parse(&owned) {
                        Ok(expr) => match eval(&expr, env) {
                            Ok(LispValue::Nil) => {}
                            Ok(val) => println!("{}", val),
                            Err(e) => println!("\x1b[31merror: {}\x1b[0m", e),
                        },
                        Err(e) => println!("\x1b[31mparse error: {}\x1b[0m", e),
                    }
                }
                drop(el);
            } else {
                input.len = 0;
                drop(input);
            }
            if RUNNING.load(Ordering::SeqCst) { print!("lisp> "); }
        }
        KeyEvent::Char('\x08') => {
            if input.len > 0 { input.len -= 1; print!("\x08 \x08"); }
        }
        KeyEvent::Char(ch) if ch.is_ascii() && !ch.is_ascii_control() => {
            if input.len < MAX_INPUT {
                { let idx = input.len; input.buf[idx] = ch as u8; }
                input.len += 1;
                print!("{}", ch);
            }
        }
        _ => {}
    }
}
