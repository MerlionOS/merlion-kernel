/// Interactive AI chat mode.
/// Multi-turn conversation with the AI assistant.
/// Uses LLM proxy when connected, keyword engine as fallback.
/// Type 'exit' or 'quit' to leave chat mode.

use crate::{print, println, ai_proxy, ai_syscall, keyboard::KeyEvent};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

const MAX_INPUT: usize = 160;

static CHATTING: AtomicBool = AtomicBool::new(false);
static CHAT_BUF: Mutex<ChatBuffer> = Mutex::new(ChatBuffer::new());

struct ChatBuffer {
    buf: [u8; MAX_INPUT],
    len: usize,
}

impl ChatBuffer {
    const fn new() -> Self {
        Self { buf: [0; MAX_INPUT], len: 0 }
    }
}

pub fn is_chatting() -> bool {
    CHATTING.load(Ordering::SeqCst)
}

/// Enter chat mode.
pub fn enter() {
    CHATTING.store(true, Ordering::SeqCst);
    CHAT_BUF.lock().len = 0;

    println!();
    println!("\x1b[36m╔══════════════════════════════════════════╗\x1b[0m");
    println!("\x1b[36m║\x1b[0m  \x1b[1mMerlionOS AI Chat\x1b[0m                       \x1b[36m║\x1b[0m");
    println!("\x1b[36m║\x1b[0m  Born for AI. Built by AI.               \x1b[36m║\x1b[0m");
    println!("\x1b[36m║\x1b[0m  Type 'exit' to leave chat.              \x1b[36m║\x1b[0m");
    println!("\x1b[36m╚══════════════════════════════════════════╝\x1b[0m");

    let status = if ai_proxy::is_connected() {
        "\x1b[32mLLM proxy connected\x1b[0m"
    } else {
        "\x1b[33mKeyword engine (connect LLM proxy for full AI)\x1b[0m"
    };
    println!("  Backend: {}", status);
    println!();
    print!("\x1b[36myou>\x1b[0m ");
}

/// Handle keyboard input during chat.
pub fn handle_input(event: KeyEvent) {
    if !CHATTING.load(Ordering::SeqCst) { return; }

    let mut buf = CHAT_BUF.lock();

    match event {
        KeyEvent::Char('\n') => {
            println!();
            let input = core::str::from_utf8(&buf.buf[..buf.len]).unwrap_or("").trim();

            if input == "exit" || input == "quit" {
                buf.len = 0;
                drop(buf);
                CHATTING.store(false, Ordering::SeqCst);
                println!("\x1b[90mChat ended.\x1b[0m");
                return;
            }

            if !input.is_empty() {
                // Copy input before dropping lock
                let input_owned = alloc::string::String::from(input);
                buf.len = 0;
                drop(buf);

                // Get AI response
                let response = ai_syscall::infer(&input_owned);
                println!("\x1b[36m ai>\x1b[0m {}", response);
                println!();
            } else {
                buf.len = 0;
                drop(buf);
            }

            print!("\x1b[36myou>\x1b[0m ");
        }
        KeyEvent::Char('\x08') => {
            if buf.len > 0 {
                buf.len -= 1;
                print!("\x08 \x08");
            }
        }
        KeyEvent::Char(ch) if ch.is_ascii() && !ch.is_ascii_control() => {
            if buf.len < MAX_INPUT {
                let len = buf.len;
                buf.buf[len] = ch as u8;
                buf.len = len + 1;
                print!("{}", ch);
            }
        }
        _ => {}
    }
}
