/// Login screen — displayed after boot, before entering the shell.
/// Simple username prompt (no password for now).
/// Sets the $USER environment variable.

use crate::{print, println, env, rtc, smp, version, keyboard::KeyEvent};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

const MAX_NAME: usize = 32;

static LOGGING_IN: AtomicBool = AtomicBool::new(false);
static LOGIN_BUF: Mutex<LoginBuf> = Mutex::new(LoginBuf { buf: [0; MAX_NAME], len: 0 });

struct LoginBuf { buf: [u8; MAX_NAME], len: usize }

/// Show the login screen.
pub fn show() {
    LOGGING_IN.store(true, Ordering::SeqCst);
    LOGIN_BUF.lock().len = 0;

    let dt = rtc::read();
    let features = smp::detect_features();

    println!();
    println!("\x1b[36m{}\x1b[0m", version::banner());
    println!();
    println!("  \x1b[33m{}\x1b[0m", features.brand);
    println!("  {}", dt);
    println!();
    println!("  85 modules | 16K+ lines of Rust | 120+ commands");
    println!("  Type 'help' after login for available commands.");
    println!();
    print!("  login: ");
}

/// Handle keyboard during login.
pub fn handle_input(event: KeyEvent) {
    if !LOGGING_IN.load(Ordering::SeqCst) { return; }

    let mut buf = LOGIN_BUF.lock();

    match event {
        KeyEvent::Char('\n') => {
            println!();
            let name = core::str::from_utf8(&buf.buf[..buf.len])
                .unwrap_or("root")
                .trim();

            let username = if name.is_empty() { "root" } else { name };

            // Set environment variables
            env::set("USER", username);

            println!();
            println!("  Welcome, \x1b[32m{}\x1b[0m!", username);
            println!();

            buf.len = 0;
            drop(buf);
            LOGGING_IN.store(false, Ordering::SeqCst);
        }
        KeyEvent::Char('\x08') => {
            if buf.len > 0 { buf.len -= 1; print!("\x08 \x08"); }
        }
        KeyEvent::Char(ch) if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' => {
            if buf.len < MAX_NAME {
                let len = buf.len;
                buf.buf[len] = ch as u8;
                buf.len = len + 1;
                print!("{}", ch);
            }
        }
        _ => {}
    }
}

pub fn is_logging_in() -> bool {
    LOGGING_IN.load(Ordering::SeqCst)
}
