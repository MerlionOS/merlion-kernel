/// Interactive kernel debugger for MerlionOS.
/// Breakpoint management, memory/register/stack inspection, single-step,
/// symbol lookup, and disassembly stubs.  Enter via `kdb`; exit with `quit`.

use alloc::string::String;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use crate::{print, println};
use crate::keyboard::KeyEvent;

const MAX_BP: usize = 16;

#[derive(Clone, Copy)]
struct Breakpoint { addr: u64, orig: u8, active: bool }
impl Breakpoint { const fn empty() -> Self { Self { addr: 0, orig: 0, active: false } } }
static BPS: Mutex<[Breakpoint; MAX_BP]> = Mutex::new([Breakpoint::empty(); MAX_BP]);

/// Add a software breakpoint — patches `int3` (0xCC) at `addr`.
pub fn breakpoint_add(addr: u64) -> Result<usize, &'static str> {
    let mut t = BPS.lock();
    if t.iter().any(|b| b.active && b.addr == addr) { return Err("already set"); }
    let slot = t.iter().position(|b| !b.active).ok_or("table full")?;
    let ptr = addr as *mut u8;
    let orig = unsafe { core::ptr::read_volatile(ptr) };
    unsafe { core::ptr::write_volatile(ptr, 0xCC) };
    t[slot] = Breakpoint { addr, orig, active: true };
    Ok(slot)
}

/// Remove breakpoint by slot index, restoring the original byte.
pub fn breakpoint_remove(slot: usize) -> Result<(), &'static str> {
    let mut t = BPS.lock();
    if slot >= MAX_BP || !t[slot].active { return Err("invalid slot"); }
    let bp = t[slot];
    unsafe { core::ptr::write_volatile(bp.addr as *mut u8, bp.orig) };
    t[slot].active = false;
    Ok(())
}

/// Print all active breakpoints.
pub fn breakpoint_list() {
    let t = BPS.lock();
    let mut any = false;
    for (i, b) in t.iter().enumerate() {
        if b.active {
            println!("  [{}] {:#018x}  orig={:#04x}", i, b.addr, b.orig);
            any = true;
        }
    }
    if !any { println!("  (no breakpoints)"); }
}

/// Dump `len` bytes at virtual address `addr` in hex+ASCII format.
pub fn memory_dump(addr: u64, len: usize) {
    let mut off = 0usize;
    while off < len {
        let row = core::cmp::min(16, len - off);
        print!("{:#018x}: ", addr + off as u64);
        for c in 0..row {
            let b = unsafe { *((addr + (off + c) as u64) as *const u8) };
            print!("{:02x} ", b);
            if c == 7 { print!(" "); }
        }
        for _ in row..16 { print!("   "); }
        print!("|");
        for c in 0..row {
            let b = unsafe { *((addr + (off + c) as u64) as *const u8) };
            print!("{}", if b >= 0x20 && b < 0x7F { b as char } else { '.' });
        }
        println!("|");
        off += 16;
    }
}

/// Dump physical memory by converting through the kernel identity map.
pub fn phys_dump(pa: u64, len: usize) {
    let va = crate::memory::phys_to_virt(x86_64::PhysAddr::new(pa));
    println!("  phys {:#x} -> virt {:#x}", pa, va.as_u64());
    memory_dump(va.as_u64(), len);
}

/// Snapshot and print CPU registers (rax-r15, rip, rflags, cr0/cr3/cr4).
pub fn register_dump() {
    let (rax, rbx, rcx, rdx, rsi, rdi, rbp, rsp_v): (u64,u64,u64,u64,u64,u64,u64,u64);
    let (r8, r9, r10, r11, r12, r13, r14, r15): (u64,u64,u64,u64,u64,u64,u64,u64);
    let (rip_v, rfl, cr0, cr3, cr4): (u64, u64, u64, u64, u64);
    unsafe {
        core::arch::asm!("mov {}, rax", out(reg) rax);
        core::arch::asm!("mov {}, rbx", out(reg) rbx);
        core::arch::asm!("mov {}, rcx", out(reg) rcx);
        core::arch::asm!("mov {}, rdx", out(reg) rdx);
        core::arch::asm!("mov {}, rsi", out(reg) rsi);
        core::arch::asm!("mov {}, rdi", out(reg) rdi);
        core::arch::asm!("mov {}, rbp", out(reg) rbp);
        core::arch::asm!("mov {}, rsp", out(reg) rsp_v);
        core::arch::asm!("mov {}, r8",  out(reg) r8);
        core::arch::asm!("mov {}, r9",  out(reg) r9);
        core::arch::asm!("mov {}, r10", out(reg) r10);
        core::arch::asm!("mov {}, r11", out(reg) r11);
        core::arch::asm!("mov {}, r12", out(reg) r12);
        core::arch::asm!("mov {}, r13", out(reg) r13);
        core::arch::asm!("mov {}, r14", out(reg) r14);
        core::arch::asm!("mov {}, r15", out(reg) r15);
        core::arch::asm!("lea {}, [rip]", out(reg) rip_v);
        core::arch::asm!("pushfq; pop {}", out(reg) rfl);
        core::arch::asm!("mov {}, cr0", out(reg) cr0);
        core::arch::asm!("mov {}, cr3", out(reg) cr3);
        core::arch::asm!("mov {}, cr4", out(reg) cr4);
    }
    println!("  rax={:#018x}  rbx={:#018x}  rcx={:#018x}", rax, rbx, rcx);
    println!("  rdx={:#018x}  rsi={:#018x}  rdi={:#018x}", rdx, rsi, rdi);
    println!("  rbp={:#018x}  rsp={:#018x}", rbp, rsp_v);
    println!("  r8 ={:#018x}  r9 ={:#018x}  r10={:#018x}", r8, r9, r10);
    println!("  r11={:#018x}  r12={:#018x}  r13={:#018x}", r11, r12, r13);
    println!("  r14={:#018x}  r15={:#018x}", r14, r15);
    println!("  rip={:#018x}  rflags={:#018x}", rip_v, rfl);
    println!("  cr0={:#018x}  cr3={:#018x}  cr4={:#018x}", cr0, cr3, cr4);
}

/// Set the Trap Flag (TF) in RFLAGS to arm single-step mode.
pub fn single_step_enable() {
    unsafe {
        core::arch::asm!("pushfq", "or qword ptr [rsp], 0x100", "popfq",
                         options(nomem, nostack));
    }
    println!("  TF set — single-step armed");
}

/// Resolve an address to the nearest symbol via `crate::ksyms`.
pub fn sym_lookup_addr(addr: u64) {
    match crate::ksyms::lookup(addr) {
        Some((name, off)) => println!("  {:#018x} = <{}+{:#x}>", addr, name, off),
        None => println!("  {:#018x} = <unknown>", addr),
    }
}

/// Search for a symbol by name.  Falls back to hex-address resolution.
pub fn sym_lookup_name(query: &str) {
    if let Ok(a) = u64::from_str_radix(query.trim_start_matches("0x"), 16) {
        sym_lookup_addr(a);
        return;
    }
    println!("  name lookup for '{}' not yet indexed", query);
}

/// Dump raw instruction bytes at `addr` (offline disassembly stub).
pub fn disassemble(addr: u64, count: usize) {
    let n = if count == 0 { 16 } else { count };
    println!("  raw bytes at {:#018x} ({} bytes):", addr, n);
    for i in 0..n {
        if i % 16 == 0 {
            if i > 0 { println!(); }
            print!("  {:#018x}: ", addr + i as u64);
        }
        let b = unsafe { *((addr + i as u64) as *const u8) };
        print!("{:02x} ", b);
    }
    println!();
}

/// Walk the frame-pointer chain and show a symbolised backtrace.
pub fn stack_trace() {
    let frames = crate::ksyms::backtrace();
    if frames.is_empty() { println!("  (no frames)"); return; }
    println!("  backtrace ({} frames):", frames.len());
    for (i, &a) in frames.iter().enumerate() {
        match crate::ksyms::lookup(a) {
            Some((name, off)) => println!("  #{}: {:#018x} <{}+{:#x}>", i, a, name, off),
            None => println!("  #{}: {:#018x} <unknown>", i, a),
        }
    }
}

static KDB_ACTIVE: AtomicBool = AtomicBool::new(false);
static KDB_BUF: Mutex<([u8; 128], usize)> = Mutex::new(([0u8; 128], 0));

/// Enter the interactive kernel debugger.
pub fn enter() {
    println!();
    println!("=== MerlionOS Kernel Debugger ===");
    println!("Type 'help' for commands, 'quit' to exit.");
    KDB_ACTIVE.store(true, Ordering::SeqCst);
    KDB_BUF.lock().1 = 0;
    print!("kdb> ");
}

/// Returns `true` while the debugger is active.
pub fn is_active() -> bool { KDB_ACTIVE.load(Ordering::SeqCst) }

/// Route a keyboard event to the debugger line editor.
pub fn handle_key_event(event: KeyEvent) {
    match event {
        KeyEvent::Char('\n') | KeyEvent::Char('\r') => {
            println!();
            let cmd: String = {
                let b = KDB_BUF.lock();
                core::str::from_utf8(&b.0[..b.1]).unwrap_or("").into()
            };
            dispatch(&cmd);
            KDB_BUF.lock().1 = 0;
            if KDB_ACTIVE.load(Ordering::SeqCst) { print!("kdb> "); }
        }
        KeyEvent::Char('\x08') | KeyEvent::Char('\x7f') => {
            let mut b = KDB_BUF.lock();
            if b.1 > 0 { b.1 -= 1; print!("\x08 \x08"); }
        }
        KeyEvent::Char(c) => {
            let mut b = KDB_BUF.lock();
            { let idx = b.1; if idx < 127 { b.0[idx] = c as u8; b.1 = idx + 1; print!("{}", c); } }
        }
        _ => {}
    }
}

/// Parse and execute a debugger command.
fn dispatch(line: &str) {
    let line = line.trim();
    if line.is_empty() { return; }
    let mut p = line.splitn(3, ' ');
    let cmd  = p.next().unwrap_or("");
    let arg1 = p.next().unwrap_or("");
    let arg2 = p.next().unwrap_or("");
    match cmd {
        "help" | "h" | "?" => {
            println!("  bp add <addr>    — set breakpoint");
            println!("  bp rm <slot>     — remove breakpoint");
            println!("  bp list          — list breakpoints");
            println!("  md <addr> [len]  — dump virtual memory (default 64)");
            println!("  mdp <addr> [len] — dump physical memory");
            println!("  regs             — CPU register dump");
            println!("  step             — arm single-step (TF)");
            println!("  sym <addr>       — symbol lookup by address");
            println!("  symn <name>      — symbol lookup by name");
            println!("  dis <addr> [n]   — raw disassembly bytes");
            println!("  bt               — stack backtrace");
            println!("  quit             — exit debugger");
        }
        "bp" => match arg1 {
            "add" => match parse_hex(arg2) {
                Some(a) => match breakpoint_add(a) {
                    Ok(s)  => println!("  bp {} at {:#018x}", s, a),
                    Err(e) => println!("  error: {}", e),
                },
                None => println!("  usage: bp add <hex-address>"),
            },
            "rm" | "del" => match arg2.parse::<usize>() {
                Ok(s) => match breakpoint_remove(s) {
                    Ok(())  => println!("  bp {} removed", s),
                    Err(e)  => println!("  error: {}", e),
                },
                Err(_) => println!("  usage: bp rm <slot>"),
            },
            "list" | "ls" | "" => breakpoint_list(),
            _ => println!("  unknown bp sub: {}", arg1),
        },
        "md" | "x" => match parse_hex(arg1) {
            Some(a) => memory_dump(a, parse_usize(arg2).unwrap_or(64)),
            None => println!("  usage: md <addr> [len]"),
        },
        "mdp" => match parse_hex(arg1) {
            Some(a) => phys_dump(a, parse_usize(arg2).unwrap_or(64)),
            None => println!("  usage: mdp <addr> [len]"),
        },
        "regs" | "reg" => register_dump(),
        "step" | "ss"  => single_step_enable(),
        "sym" => match parse_hex(arg1) {
            Some(a) => sym_lookup_addr(a),
            None => println!("  usage: sym <hex-address>"),
        },
        "symn" if !arg1.is_empty() => sym_lookup_name(arg1),
        "symn" => println!("  usage: symn <name>"),
        "dis" | "disasm" => match parse_hex(arg1) {
            Some(a) => disassemble(a, parse_usize(arg2).unwrap_or(16)),
            None => println!("  usage: dis <addr> [count]"),
        },
        "bt" | "backtrace" => stack_trace(),
        "quit" | "q" => {
            println!("  leaving kdb");
            KDB_ACTIVE.store(false, Ordering::SeqCst);
        }
        _ => println!("  unknown command '{}' — try 'help'", cmd),
    }
}

fn parse_hex(s: &str) -> Option<u64> {
    let s = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    if s.is_empty() { return None; }
    u64::from_str_radix(s, 16).ok()
}

fn parse_usize(s: &str) -> Option<usize> {
    let s = s.trim();
    if s.is_empty() { return None; }
    if s.starts_with("0x") { usize::from_str_radix(&s[2..], 16).ok() }
    else { s.parse().ok() }
}
