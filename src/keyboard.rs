/// Simple PS/2 keyboard scancode set 1 decoder.
/// Converts make codes (key press) to ASCII characters.
/// Only handles basic printable characters — no modifier tracking yet.

/// Decode a scancode set 1 make code to an ASCII character.
/// Returns None for key releases (bit 7 set) and unmapped keys.
pub fn scancode_to_ascii(scancode: u8) -> Option<char> {
    // Bit 7 set = key release (break code)
    if scancode & 0x80 != 0 {
        return None;
    }

    // Scancode set 1 lookup table (make codes only)
    match scancode {
        0x02 => Some('1'),
        0x03 => Some('2'),
        0x04 => Some('3'),
        0x05 => Some('4'),
        0x06 => Some('5'),
        0x07 => Some('6'),
        0x08 => Some('7'),
        0x09 => Some('8'),
        0x0A => Some('9'),
        0x0B => Some('0'),
        0x0C => Some('-'),
        0x0D => Some('='),
        0x0E => Some('\x08'), // backspace
        0x0F => Some('\t'),
        0x10 => Some('q'),
        0x11 => Some('w'),
        0x12 => Some('e'),
        0x13 => Some('r'),
        0x14 => Some('t'),
        0x15 => Some('y'),
        0x16 => Some('u'),
        0x17 => Some('i'),
        0x18 => Some('o'),
        0x19 => Some('p'),
        0x1A => Some('['),
        0x1B => Some(']'),
        0x1C => Some('\n'), // enter
        0x1E => Some('a'),
        0x1F => Some('s'),
        0x20 => Some('d'),
        0x21 => Some('f'),
        0x22 => Some('g'),
        0x23 => Some('h'),
        0x24 => Some('j'),
        0x25 => Some('k'),
        0x26 => Some('l'),
        0x27 => Some(';'),
        0x28 => Some('\''),
        0x29 => Some('`'),
        0x2B => Some('\\'),
        0x2C => Some('z'),
        0x2D => Some('x'),
        0x2E => Some('c'),
        0x2F => Some('v'),
        0x30 => Some('b'),
        0x31 => Some('n'),
        0x32 => Some('m'),
        0x33 => Some(','),
        0x34 => Some('.'),
        0x35 => Some('/'),
        0x39 => Some(' '),
        _ => None,
    }
}
