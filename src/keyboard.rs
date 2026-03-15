/// PS/2 keyboard scancode set 1 decoder.
/// Handles basic ASCII keys, extended scancodes (arrow keys),
/// and shift modifier for uppercase/symbols.

use core::sync::atomic::{AtomicBool, Ordering};

/// Whether the next scancode is an extended key (preceded by 0xE0).
static EXTENDED: AtomicBool = AtomicBool::new(false);
/// Whether left or right shift is held.
static SHIFT: AtomicBool = AtomicBool::new(false);

/// Key events that can be returned to the shell.
#[derive(Debug, Clone, Copy)]
pub enum KeyEvent {
    Char(char),
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    Delete,
}

/// Process a scancode and return a key event if applicable.
pub fn process_scancode(scancode: u8) -> Option<KeyEvent> {
    // Extended scancode prefix
    if scancode == 0xE0 {
        EXTENDED.store(true, Ordering::SeqCst);
        return None;
    }

    let is_extended = EXTENDED.load(Ordering::SeqCst);
    EXTENDED.store(false, Ordering::SeqCst);

    // Key release (break code)
    if scancode & 0x80 != 0 {
        let make = scancode & 0x7F;
        // Track shift release
        if make == 0x2A || make == 0x36 {
            SHIFT.store(false, Ordering::SeqCst);
        }
        return None;
    }

    // Track shift press
    if scancode == 0x2A || scancode == 0x36 {
        SHIFT.store(true, Ordering::SeqCst);
        return None;
    }

    // Extended keys (arrow keys, etc.)
    if is_extended {
        return match scancode {
            0x48 => Some(KeyEvent::ArrowUp),
            0x50 => Some(KeyEvent::ArrowDown),
            0x4B => Some(KeyEvent::ArrowLeft),
            0x4D => Some(KeyEvent::ArrowRight),
            0x47 => Some(KeyEvent::Home),
            0x4F => Some(KeyEvent::End),
            0x53 => Some(KeyEvent::Delete),
            _ => None,
        };
    }

    let shifted = SHIFT.load(Ordering::SeqCst);

    // Normal keys
    let ch = match scancode {
        0x02 => if shifted { '!' } else { '1' },
        0x03 => if shifted { '@' } else { '2' },
        0x04 => if shifted { '#' } else { '3' },
        0x05 => if shifted { '$' } else { '4' },
        0x06 => if shifted { '%' } else { '5' },
        0x07 => if shifted { '^' } else { '6' },
        0x08 => if shifted { '&' } else { '7' },
        0x09 => if shifted { '*' } else { '8' },
        0x0A => if shifted { '(' } else { '9' },
        0x0B => if shifted { ')' } else { '0' },
        0x0C => if shifted { '_' } else { '-' },
        0x0D => if shifted { '+' } else { '=' },
        0x0E => '\x08', // backspace
        0x0F => '\t',
        0x10 => if shifted { 'Q' } else { 'q' },
        0x11 => if shifted { 'W' } else { 'w' },
        0x12 => if shifted { 'E' } else { 'e' },
        0x13 => if shifted { 'R' } else { 'r' },
        0x14 => if shifted { 'T' } else { 't' },
        0x15 => if shifted { 'Y' } else { 'y' },
        0x16 => if shifted { 'U' } else { 'u' },
        0x17 => if shifted { 'I' } else { 'i' },
        0x18 => if shifted { 'O' } else { 'o' },
        0x19 => if shifted { 'P' } else { 'p' },
        0x1A => if shifted { '{' } else { '[' },
        0x1B => if shifted { '}' } else { ']' },
        0x1C => '\n',
        0x1E => if shifted { 'A' } else { 'a' },
        0x1F => if shifted { 'S' } else { 's' },
        0x20 => if shifted { 'D' } else { 'd' },
        0x21 => if shifted { 'F' } else { 'f' },
        0x22 => if shifted { 'G' } else { 'g' },
        0x23 => if shifted { 'H' } else { 'h' },
        0x24 => if shifted { 'J' } else { 'j' },
        0x25 => if shifted { 'K' } else { 'k' },
        0x26 => if shifted { 'L' } else { 'l' },
        0x27 => if shifted { ':' } else { ';' },
        0x28 => if shifted { '"' } else { '\'' },
        0x29 => if shifted { '~' } else { '`' },
        0x2B => if shifted { '|' } else { '\\' },
        0x2C => if shifted { 'Z' } else { 'z' },
        0x2D => if shifted { 'X' } else { 'x' },
        0x2E => if shifted { 'C' } else { 'c' },
        0x2F => if shifted { 'V' } else { 'v' },
        0x30 => if shifted { 'B' } else { 'b' },
        0x31 => if shifted { 'N' } else { 'n' },
        0x32 => if shifted { 'M' } else { 'm' },
        0x33 => if shifted { '<' } else { ',' },
        0x34 => if shifted { '>' } else { '.' },
        0x35 => if shifted { '?' } else { '/' },
        0x39 => ' ',
        _ => return None,
    };

    Some(KeyEvent::Char(ch))
}
