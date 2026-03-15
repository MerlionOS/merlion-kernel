/// Forth interpreter — a stack-based programming language inside MerlionOS.
///
/// Supports: arithmetic, stack ops, comparison, control flow, variables,
/// and custom word definitions.
///
///   merlion> forth
///   forth> 2 3 + .
///   5
///   forth> : square dup * ;
///   forth> 7 square .
///   49
///   forth> exit

use crate::{print, println, keyboard::KeyEvent};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;

const STACK_SIZE: usize = 256;
const MAX_WORDS: usize = 64;
const MAX_INPUT: usize = 160;

static RUNNING: AtomicBool = AtomicBool::new(false);
static INPUT: Mutex<InputBuf> = Mutex::new(InputBuf { buf: [0; MAX_INPUT], len: 0 });

struct InputBuf { buf: [u8; MAX_INPUT], len: usize }

/// Forth virtual machine state.
struct Vm {
    stack: [i64; STACK_SIZE],
    sp: usize,
    rstack: [i64; 32],  // return stack
    rsp: usize,
    words: Vec<(String, Vec<String>)>,  // user-defined words: (name, body tokens)
    vars: Vec<(String, i64)>,           // variables
}

impl Vm {
    fn new() -> Self {
        Self {
            stack: [0; STACK_SIZE],
            sp: 0,
            rstack: [0; 32],
            rsp: 0,
            words: Vec::new(),
            vars: Vec::new(),
        }
    }

    fn push(&mut self, val: i64) {
        if self.sp < STACK_SIZE {
            self.stack[self.sp] = val;
            self.sp += 1;
        }
    }

    fn pop(&mut self) -> Option<i64> {
        if self.sp > 0 { self.sp -= 1; Some(self.stack[self.sp]) } else { None }
    }

    fn peek(&self) -> Option<i64> {
        if self.sp > 0 { Some(self.stack[self.sp - 1]) } else { None }
    }

    fn rpush(&mut self, val: i64) {
        if self.rsp < 32 { self.rstack[self.rsp] = val; self.rsp += 1; }
    }

    fn rpop(&mut self) -> Option<i64> {
        if self.rsp > 0 { self.rsp -= 1; Some(self.rstack[self.rsp]) } else { None }
    }

    fn exec_line(&mut self, line: &str) -> Result<(), String> {
        let tokens: Vec<String> = line.split_whitespace().map(|s| s.to_owned()).collect();
        self.exec_tokens(&tokens, 0)
    }

    fn exec_tokens(&mut self, tokens: &[String], start: usize) -> Result<(), String> {
        let mut i = start;
        while i < tokens.len() {
            let token = &tokens[i];

            // Word definition: : name body ;
            if token == ":" {
                i += 1;
                if i >= tokens.len() { return Err("missing word name".to_owned()); }
                let name = tokens[i].to_lowercase();
                i += 1;
                let mut body = Vec::new();
                while i < tokens.len() && tokens[i] != ";" {
                    body.push(tokens[i].clone());
                    i += 1;
                }
                if i >= tokens.len() { return Err("missing ;".to_owned()); }
                self.words.push((name, body));
                i += 1;
                continue;
            }

            // Variable definition
            if token.to_lowercase() == "variable" {
                i += 1;
                if i >= tokens.len() { return Err("missing variable name".to_owned()); }
                let name = tokens[i].to_lowercase();
                self.vars.push((name, 0));
                i += 1;
                continue;
            }

            self.exec_word(token)?;
            i += 1;
        }
        Ok(())
    }

    fn exec_word(&mut self, word: &str) -> Result<(), String> {
        let w = word.to_lowercase();

        // Try number
        if let Ok(n) = word.parse::<i64>() {
            self.push(n);
            return Ok(());
        }

        // Built-in words
        match w.as_str() {
            // Arithmetic
            "+" => { let b = self.need_pop()?; let a = self.need_pop()?; self.push(a + b); }
            "-" => { let b = self.need_pop()?; let a = self.need_pop()?; self.push(a - b); }
            "*" => { let b = self.need_pop()?; let a = self.need_pop()?; self.push(a * b); }
            "/" => { let b = self.need_pop()?; let a = self.need_pop()?;
                     if b == 0 { return Err("division by zero".to_owned()); }
                     self.push(a / b); }
            "mod" => { let b = self.need_pop()?; let a = self.need_pop()?; self.push(a % b); }
            "negate" => { let a = self.need_pop()?; self.push(-a); }
            "abs" => { let a = self.need_pop()?; self.push(a.abs()); }
            "max" => { let b = self.need_pop()?; let a = self.need_pop()?; self.push(a.max(b)); }
            "min" => { let b = self.need_pop()?; let a = self.need_pop()?; self.push(a.min(b)); }

            // Stack manipulation
            "dup" => { let a = self.need_peek()?; self.push(a); }
            "drop" => { self.need_pop()?; }
            "swap" => { let b = self.need_pop()?; let a = self.need_pop()?; self.push(b); self.push(a); }
            "over" => { let b = self.need_pop()?; let a = self.need_pop()?;
                        self.push(a); self.push(b); self.push(a); }
            "rot" => { let c = self.need_pop()?; let b = self.need_pop()?; let a = self.need_pop()?;
                        self.push(b); self.push(c); self.push(a); }
            "depth" => { self.push(self.sp as i64); }

            // Comparison
            "=" => { let b = self.need_pop()?; let a = self.need_pop()?; self.push(if a == b { -1 } else { 0 }); }
            "<" => { let b = self.need_pop()?; let a = self.need_pop()?; self.push(if a < b { -1 } else { 0 }); }
            ">" => { let b = self.need_pop()?; let a = self.need_pop()?; self.push(if a > b { -1 } else { 0 }); }
            "0=" => { let a = self.need_pop()?; self.push(if a == 0 { -1 } else { 0 }); }

            // Logic
            "and" => { let b = self.need_pop()?; let a = self.need_pop()?; self.push(a & b); }
            "or" => { let b = self.need_pop()?; let a = self.need_pop()?; self.push(a | b); }
            "not" | "invert" => { let a = self.need_pop()?; self.push(!a); }

            // Output
            "." => { let a = self.need_pop()?; print!("{} ", a); }
            ".s" => {
                print!("<{}> ", self.sp);
                for j in 0..self.sp { print!("{} ", self.stack[j]); }
                println!();
            }
            "cr" => { println!(); }
            "emit" => { let a = self.need_pop()?; print!("{}", a as u8 as char); }
            ".\"" => { /* inline string — not supported in this simple version */ }

            // Return stack
            ">r" => { let a = self.need_pop()?; self.rpush(a); }
            "r>" => { let a = self.rpop().ok_or("return stack underflow")?; self.push(a); }

            // Variable access
            "!" => {
                let name_idx = self.need_pop()? as usize;
                let val = self.need_pop()?;
                if name_idx < self.vars.len() { self.vars[name_idx].1 = val; }
            }
            "@" => {
                let name_idx = self.need_pop()? as usize;
                if name_idx < self.vars.len() { self.push(self.vars[name_idx].1); }
            }

            // System
            "words" => {
                print!("Built-in: + - * / mod dup drop swap over rot . .s cr emit = < > depth\n");
                if !self.words.is_empty() {
                    print!("User: ");
                    for (name, _) in &self.words { print!("{} ", name); }
                    println!();
                }
            }
            "bye" | "exit" => { RUNNING.store(false, Ordering::SeqCst); }

            // Try user-defined words
            _ => {
                let body = self.words.iter()
                    .find(|(n, _)| n == &w)
                    .map(|(_, b)| b.clone());
                if let Some(body) = body {
                    self.exec_tokens(&body, 0)?;
                } else {
                    return Err(alloc::format!("unknown word: {}", word));
                }
            }
        }

        Ok(())
    }

    fn need_pop(&mut self) -> Result<i64, String> {
        self.pop().ok_or_else(|| "stack underflow".to_owned())
    }

    fn need_peek(&self) -> Result<i64, String> {
        self.peek().ok_or_else(|| "stack empty".to_owned())
    }
}

// --- Interactive mode ---

static VM: Mutex<Option<Vm>> = Mutex::new(None);

pub fn is_running() -> bool { RUNNING.load(Ordering::SeqCst) }

pub fn enter() {
    *VM.lock() = Some(Vm::new());
    RUNNING.store(true, Ordering::SeqCst);
    INPUT.lock().len = 0;

    println!("\x1b[33mMerlionOS Forth v1.0\x1b[0m");
    println!("Type 'words' for built-ins, 'exit' to quit.");
    print!("forth> ");
}

pub fn handle_input(event: KeyEvent) {
    if !RUNNING.load(Ordering::SeqCst) { return; }

    let mut input = INPUT.lock();
    match event {
        KeyEvent::Char('\n') => {
            println!();
            let line = core::str::from_utf8(&input.buf[..input.len]).unwrap_or("").trim();
            if !line.is_empty() {
                let line_owned = String::from(line);
                input.len = 0;
                drop(input);

                let mut vm_lock = VM.lock();
                if let Some(ref mut vm) = *vm_lock {
                    match vm.exec_line(&line_owned) {
                        Ok(()) => println!(" ok"),
                        Err(e) => println!(" \x1b[31merror: {}\x1b[0m", e),
                    }
                }
                drop(vm_lock);
            } else {
                input.len = 0;
                drop(input);
            }
            if RUNNING.load(Ordering::SeqCst) {
                print!("forth> ");
            }
        }
        KeyEvent::Char('\x08') => {
            if input.len > 0 { input.len -= 1; print!("\x08 \x08"); }
        }
        KeyEvent::Char(ch) if ch.is_ascii() && !ch.is_ascii_control() => {
            if input.len < MAX_INPUT {
                let len = input.len;
                input.buf[len] = ch as u8;
                input.len = len + 1;
                print!("{}", ch);
            }
        }
        _ => {}
    }
}
