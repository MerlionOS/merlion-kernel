/// USB HID keyboard driver for MerlionOS.
///
/// Parses USB HID boot-protocol keyboard reports (8 bytes) and translates
/// HID usage keycodes into kernel `KeyEvent` values. This module does not
/// perform USB transport — that responsibility belongs to `xhci.rs`. It
/// only handles HID report interpretation and keycode-to-ASCII mapping.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::keyboard::KeyEvent;
use crate::driver;

// ---------------------------------------------------------------------------
// HID report descriptor constants (USB HID Usage Tables §10 — Keyboard)
// ---------------------------------------------------------------------------

/// HID usage page for keyboard/keypad (0x07).
pub const HID_USAGE_PAGE_KEYBOARD: u8 = 0x07;

/// Boot-protocol keyboard report length in bytes.
pub const HID_BOOT_REPORT_LEN: usize = 8;

/// Maximum number of simultaneous key slots in a boot report.
pub const HID_MAX_KEYS: usize = 6;

/// Modifier bit positions within the first byte of a boot report.
pub const MOD_LEFT_CTRL: u8   = 1 << 0;
pub const MOD_LEFT_SHIFT: u8  = 1 << 1;
pub const MOD_LEFT_ALT: u8    = 1 << 2;
pub const MOD_LEFT_GUI: u8    = 1 << 3;
pub const MOD_RIGHT_CTRL: u8  = 1 << 4;
pub const MOD_RIGHT_SHIFT: u8 = 1 << 5;
pub const MOD_RIGHT_ALT: u8   = 1 << 6;
pub const MOD_RIGHT_GUI: u8   = 1 << 7;

/// Phantom / rollover error code sent when too many keys are pressed.
pub const HID_ERR_ROLLOVER: u8 = 0x01;

// HID keycodes for special keys used during parsing.
const KEY_A: u8             = 0x04;
const KEY_Z: u8             = 0x1D;
const KEY_1: u8             = 0x1E;
const KEY_0: u8             = 0x27;
const KEY_ENTER: u8         = 0x28;
const KEY_ESCAPE: u8        = 0x29;
const KEY_BACKSPACE: u8     = 0x2A;
const KEY_TAB: u8           = 0x2B;
const KEY_SPACE: u8         = 0x2C;
const KEY_CAPS_LOCK: u8     = 0x39;
const KEY_DELETE: u8        = 0x4C;
const KEY_RIGHT_ARROW: u8   = 0x4F;
const KEY_LEFT_ARROW: u8    = 0x50;
const KEY_DOWN_ARROW: u8    = 0x51;
const KEY_UP_ARROW: u8      = 0x52;
const KEY_HOME: u8          = 0x4A;
const KEY_END: u8           = 0x4D;

// ---------------------------------------------------------------------------
// HID boot-protocol keyboard report
// ---------------------------------------------------------------------------

/// Parsed view of an 8-byte HID boot-protocol keyboard report.
///
/// Layout: `[modifier, reserved, key0, key1, key2, key3, key4, key5]`
#[derive(Debug, Clone, Copy)]
pub struct HidKeyboardReport {
    /// Modifier bitmap (ctrl, shift, alt, gui).
    pub modifier: u8,
    /// Reserved byte (always zero per spec).
    pub reserved: u8,
    /// Up to six simultaneous keycodes.
    pub keycodes: [u8; HID_MAX_KEYS],
}

impl HidKeyboardReport {
    /// Interpret a raw 8-byte slice as a keyboard report.
    pub fn from_bytes(report: &[u8; 8]) -> Self {
        Self {
            modifier: report[0],
            reserved: report[1],
            keycodes: [
                report[2], report[3], report[4],
                report[5], report[6], report[7],
            ],
        }
    }

    /// Returns `true` if either left or right shift modifier is active.
    pub fn shift_held(&self) -> bool {
        self.modifier & (MOD_LEFT_SHIFT | MOD_RIGHT_SHIFT) != 0
    }

    /// Returns `true` if either left or right ctrl modifier is active.
    pub fn ctrl_held(&self) -> bool {
        self.modifier & (MOD_LEFT_CTRL | MOD_RIGHT_CTRL) != 0
    }

    /// Returns `true` if either left or right alt modifier is active.
    pub fn alt_held(&self) -> bool {
        self.modifier & (MOD_LEFT_ALT | MOD_RIGHT_ALT) != 0
    }
}

// ---------------------------------------------------------------------------
// Keyboard state
// ---------------------------------------------------------------------------

/// Tracks persistent modifier state (e.g. caps-lock toggle) across reports.
pub struct KeyboardState {
    /// Caps-lock is a toggle — persists between reports.
    pub capslock: bool,
    /// Previous report keycodes, used to detect new key presses.
    prev_keycodes: [u8; HID_MAX_KEYS],
}

/// Global caps-lock state (shared with interrupt context).
static CAPSLOCK: AtomicBool = AtomicBool::new(false);

impl KeyboardState {
    /// Create a new keyboard state with all modifiers cleared.
    pub const fn new() -> Self {
        Self {
            capslock: false,
            prev_keycodes: [0u8; HID_MAX_KEYS],
        }
    }
}

/// Module-level persistent state.
static mut STATE: KeyboardState = KeyboardState::new();

// ---------------------------------------------------------------------------
// Keycode to ASCII translation
// ---------------------------------------------------------------------------

/// Symbols on number row when shift is held (keycodes 0x1E–0x27 → '1'–'0').
static SHIFT_NUMBERS: [char; 10] = ['!', '@', '#', '$', '%', '^', '&', '*', '(', ')'];

/// Non-shifted symbols for keycodes 0x2D–0x38 (minus through slash).
static SYMBOLS_NORMAL: [char; 12] = [
    '-', '=', '[', ']', '\\', '#', ';', '\'', '`', ',', '.', '/',
];

/// Shifted symbols for the same range.
static SYMBOLS_SHIFTED: [char; 12] = [
    '_', '+', '{', '}', '|', '~', ':', '"', '~', '<', '>', '?',
];

/// Translate a HID keyboard usage code to an ASCII character.
///
/// Returns `None` for non-printable or unmapped keycodes.
pub fn hid_keycode_to_ascii(keycode: u8, shifted: bool) -> Option<char> {
    match keycode {
        // Letters a–z / A–Z
        KEY_A..=KEY_Z => {
            let base = b'a' + (keycode - KEY_A);
            let caps = CAPSLOCK.load(Ordering::Relaxed) ^ shifted;
            Some(if caps { (base - 32) as char } else { base as char })
        }
        // Number row 1–9, 0
        KEY_1..=KEY_0 => {
            let idx = (keycode - KEY_1) as usize;
            if shifted {
                Some(SHIFT_NUMBERS[idx])
            } else if idx == 9 {
                Some('0')
            } else {
                Some((b'1' + idx as u8) as char)
            }
        }
        // Whitespace and control
        KEY_ENTER     => Some('\n'),
        KEY_TAB       => Some('\t'),
        KEY_BACKSPACE => Some('\x08'),
        KEY_SPACE     => Some(' '),
        // Symbols (minus through slash)
        0x2D..=0x38 => {
            let idx = (keycode - 0x2D) as usize;
            if shifted {
                Some(SYMBOLS_SHIFTED[idx])
            } else {
                Some(SYMBOLS_NORMAL[idx])
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Report parsing
// ---------------------------------------------------------------------------

/// Parse an 8-byte HID boot-protocol keyboard report into kernel key events.
///
/// Only newly pressed keys (not present in the previous report) produce
/// events. Modifier state (shift, ctrl, alt) is taken from the report
/// directly; caps-lock is toggled on each press.
///
/// # Safety
///
/// Accesses module-level `STATE` — must only be called from a single
/// context (e.g. the xHCI interrupt handler).
pub fn parse_report(report: &[u8; 8]) -> Vec<KeyEvent> {
    let parsed = HidKeyboardReport::from_bytes(report);
    let mut events = Vec::new();

    // Ignore rollover reports.
    if parsed.keycodes[0] == HID_ERR_ROLLOVER {
        return events;
    }

    let shifted = parsed.shift_held();

    // Safety: single-context access from xHCI handler.
    let state = unsafe { &mut *(&raw mut STATE) };

    for &keycode in &parsed.keycodes {
        if keycode == 0 {
            continue;
        }

        // Only emit events for keys not in the previous report.
        if state.prev_keycodes.contains(&keycode) {
            continue;
        }

        // Toggle caps-lock on press.
        if keycode == KEY_CAPS_LOCK {
            state.capslock = !state.capslock;
            CAPSLOCK.store(state.capslock, Ordering::Relaxed);
            continue;
        }

        // Navigation / special keys.
        let event = match keycode {
            KEY_UP_ARROW    => Some(KeyEvent::ArrowUp),
            KEY_DOWN_ARROW  => Some(KeyEvent::ArrowDown),
            KEY_LEFT_ARROW  => Some(KeyEvent::ArrowLeft),
            KEY_RIGHT_ARROW => Some(KeyEvent::ArrowRight),
            KEY_HOME        => Some(KeyEvent::Home),
            KEY_END         => Some(KeyEvent::End),
            KEY_DELETE      => Some(KeyEvent::Delete),
            _ => {
                // Try ASCII translation.
                hid_keycode_to_ascii(keycode, shifted).map(|ch| {
                    // Ctrl+letter produces control characters (0x01–0x1A).
                    if parsed.ctrl_held() && ch.is_ascii_alphabetic() {
                        let ctrl_char = (ch.to_ascii_lowercase() as u8 - b'a' + 1) as char;
                        KeyEvent::Char(ctrl_char)
                    } else {
                        KeyEvent::Char(ch)
                    }
                })
            }
        };

        if let Some(ev) = event {
            events.push(ev);
        }
    }

    // Save current keycodes for next comparison.
    state.prev_keycodes = parsed.keycodes;

    events
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Register the USB HID keyboard driver with the kernel driver framework.
///
/// This should be called after xHCI enumeration discovers a HID keyboard
/// device. It does not set up USB transport — only registers the driver
/// so it appears in `drivers` listings.
pub fn init() {
    driver::register("usb-hid-keyboard", driver::DriverKind::Keyboard);
    crate::serial_println!("[usb_hid] USB HID keyboard driver registered");
}
