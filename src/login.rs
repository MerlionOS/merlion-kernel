/// Login screen with username + password authentication.
/// Authenticates against the security module's user database.
/// Sets $USER and security context on successful login.

use crate::{print, println, serial_println, env, rtc, smp, version, keyboard::KeyEvent};
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use spin::Mutex;

const MAX_NAME: usize = 32;
const MAX_PASS: usize = 64;
const MAX_ATTEMPTS: u8 = 3;

static LOGGING_IN: AtomicBool = AtomicBool::new(false);
static PHASE: AtomicU8 = AtomicU8::new(0);  // 0 = username, 1 = password
static ATTEMPTS: AtomicU8 = AtomicU8::new(0);

static LOGIN_BUF: Mutex<LoginBuf> = Mutex::new(LoginBuf {
    name_buf: [0; MAX_NAME], name_len: 0,
    pass_buf: [0; MAX_PASS], pass_len: 0,
});

struct LoginBuf {
    name_buf: [u8; MAX_NAME],
    name_len: usize,
    pass_buf: [u8; MAX_PASS],
    pass_len: usize,
}

/// Show the login screen.
pub fn show() {
    LOGGING_IN.store(true, Ordering::SeqCst);
    PHASE.store(0, Ordering::SeqCst);
    ATTEMPTS.store(0, Ordering::SeqCst);
    let mut buf = LOGIN_BUF.lock();
    buf.name_len = 0;
    buf.pass_len = 0;
    drop(buf);

    let dt = rtc::read();
    let features = smp::detect_features();

    println!();
    println!("\x1b[36m{}\x1b[0m", version::banner());
    println!();
    println!("  \x1b[33m{}\x1b[0m", features.brand);
    println!("  {}", dt);
    println!();
    println!("  170 modules | 42K+ lines of Rust | 150+ commands");
    println!("  Type 'help' after login for available commands.");
    println!();
    print!("  login: ");
}

/// Handle keyboard during login.
pub fn handle_input(event: KeyEvent) {
    if !LOGGING_IN.load(Ordering::SeqCst) { return; }

    let phase = PHASE.load(Ordering::SeqCst);

    match phase {
        0 => handle_username(event),
        1 => handle_password(event),
        _ => {}
    }
}

fn handle_username(event: KeyEvent) {
    let mut buf = LOGIN_BUF.lock();

    match event {
        KeyEvent::Char('\n') => {
            println!();
            let name = core::str::from_utf8(&buf.name_buf[..buf.name_len])
                .unwrap_or("root")
                .trim();

            if name.is_empty() {
                // Default to root
                let root = b"root";
                buf.name_buf[..4].copy_from_slice(root);
                buf.name_len = 4;
            }

            buf.pass_len = 0;
            drop(buf);
            PHASE.store(1, Ordering::SeqCst);
            print!("  password: ");
        }
        KeyEvent::Char('\x08') => {
            if buf.name_len > 0 { buf.name_len -= 1; print!("\x08 \x08"); }
        }
        KeyEvent::Char(ch) if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' => {
            if buf.name_len < MAX_NAME {
                let len = buf.name_len;
                buf.name_buf[len] = ch as u8;
                buf.name_len = len + 1;
                print!("{}", ch);
            }
        }
        _ => {}
    }
}

fn handle_password(event: KeyEvent) {
    let mut buf = LOGIN_BUF.lock();

    match event {
        KeyEvent::Char('\n') => {
            println!();

            let username = core::str::from_utf8(&buf.name_buf[..buf.name_len])
                .unwrap_or("root")
                .trim();
            let password = core::str::from_utf8(&buf.pass_buf[..buf.pass_len])
                .unwrap_or("");

            let pw_hash = crate::security::hash_password(password);

            if crate::security::authenticate(username, pw_hash) {
                // Successful login
                serial_println!("[login] {} authenticated successfully", username);

                // Set security context
                let _ = crate::security::su(username, Some(password));
                env::set("USER", username);

                println!();
                println!("  Welcome, \x1b[32m{}\x1b[0m!", username);
                println!();

                buf.name_len = 0;
                buf.pass_len = 0;
                drop(buf);
                LOGGING_IN.store(false, Ordering::SeqCst);
            } else {
                let attempts = ATTEMPTS.fetch_add(1, Ordering::SeqCst) + 1;
                serial_println!("[login] authentication failed for {} (attempt {})", username, attempts);

                if attempts >= MAX_ATTEMPTS {
                    println!("  \x1b[31mToo many attempts. Try again.\x1b[0m");
                    println!();
                    ATTEMPTS.store(0, Ordering::SeqCst);
                    buf.name_len = 0;
                    buf.pass_len = 0;
                    drop(buf);
                    PHASE.store(0, Ordering::SeqCst);
                    print!("  login: ");
                } else {
                    println!("  \x1b[31mLogin incorrect.\x1b[0m");
                    buf.pass_len = 0;
                    drop(buf);
                    PHASE.store(1, Ordering::SeqCst);
                    print!("  password: ");
                }
            }
        }
        KeyEvent::Char('\x08') => {
            if buf.pass_len > 0 { buf.pass_len -= 1; print!("\x08 \x08"); }
        }
        KeyEvent::Char(ch) if ch.is_ascii() && !ch.is_ascii_control() => {
            if buf.pass_len < MAX_PASS {
                let len = buf.pass_len;
                buf.pass_buf[len] = ch as u8;
                buf.pass_len = len + 1;
                print!("*");  // mask password
            }
        }
        _ => {}
    }
}

pub fn is_logging_in() -> bool {
    LOGGING_IN.load(Ordering::SeqCst)
}
