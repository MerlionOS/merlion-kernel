/// ANSI escape sequence parser for the MerlionOS terminal.
///
/// Parses a byte stream into structured [`AnsiAction`] values, tracking
/// terminal state (colors, bold/italic/underline) in [`AnsiState`].
///
/// Supported sequences:
/// - **SGR** (Select Graphic Rendition): codes 0-37 + 90-97
/// - **Cursor movement**: CUU (A), CUD (B), CUF (C), CUB (D), CUP (H)
/// - **Erase**: ED (J) — erase display, EL (K) — erase line

extern crate alloc;

use alloc::vec::Vec;
use crate::vga::Color;

const MAX_PARAMS: usize = 8;

/// Actions produced by the parser for each byte or completed escape sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnsiAction {
    /// Print a visible character at the current cursor position.
    PrintChar(u8),
    /// Set the VGA color attribute byte (fg | bg << 4).
    SetColor(u8),
    /// Move the cursor by a relative offset. Positive = down/right.
    MoveCursor { rows: i32, cols: i32 },
    /// Set the cursor to an absolute 1-based position (CUP).
    SetCursorPos { row: u16, col: u16 },
    /// Erase part of the display. Mode: 0 = below, 1 = above, 2 = all.
    EraseScreen(u8),
    /// Erase part of the current line. Mode: 0 = right, 1 = left, 2 = all.
    EraseLine(u8),
    /// Reset all attributes to defaults.
    Reset,
}

/// Internal parser state-machine phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Ground,
    Escape, // saw ESC (0x1B)
    Csi,    // inside CSI (ESC [)
}

/// Persistent terminal state tracking current colors and text attributes.
///
/// Create with [`AnsiState::new`], then feed bytes through [`process_byte`].
#[derive(Debug, Clone)]
pub struct AnsiState {
    /// Current foreground VGA color (0-15).
    pub fg: u8,
    /// Current background VGA color (0-15).
    pub bg: u8,
    /// Bold / bright attribute is active.
    pub bold: bool,
    /// Italic attribute is active.
    pub italic: bool,
    /// Underline attribute is active.
    pub underline: bool,
    phase: Phase,
    params: [u16; MAX_PARAMS],
    param_count: usize,
    param_started: bool,
}

impl AnsiState {
    /// Create a new parser state with default VGA colors (light-gray on black).
    pub fn new() -> Self {
        Self {
            fg: Color::LightGray as u8,
            bg: Color::Black as u8,
            bold: false, italic: false, underline: false,
            phase: Phase::Ground,
            params: [0; MAX_PARAMS],
            param_count: 0,
            param_started: false,
        }
    }

    /// Compute the VGA attribute byte from the current fg/bg.
    pub fn vga_attr(&self) -> u8 {
        (self.bg << 4) | self.fg
    }

    fn reset_attrs(&mut self) {
        self.fg = Color::LightGray as u8;
        self.bg = Color::Black as u8;
        self.bold = false;
        self.italic = false;
        self.underline = false;
    }

    fn begin_csi(&mut self) {
        self.params = [0; MAX_PARAMS];
        self.param_count = 0;
        self.param_started = false;
    }

    fn finalize_params(&mut self) -> usize {
        if self.param_started && self.param_count < MAX_PARAMS {
            self.param_count += 1;
        }
        self.param_count
    }

    fn param(&self, idx: usize, default: u16) -> u16 {
        if idx < self.param_count {
            let v = self.params[idx];
            if v == 0 { default } else { v }
        } else {
            default
        }
    }
}

/// Standard ANSI foreground codes 30-37 mapped to VGA palette indices.
const ANSI_TO_VGA: [u8; 8] = [
    Color::Black as u8,     // 30
    Color::Red as u8,       // 31
    Color::Green as u8,     // 32
    Color::Brown as u8,     // 33 (ANSI yellow -> VGA brown)
    Color::Blue as u8,      // 34
    Color::Magenta as u8,   // 35
    Color::Cyan as u8,      // 36
    Color::LightGray as u8, // 37
];

/// Bright ANSI foreground codes 90-97 mapped to VGA palette indices.
const ANSI_BRIGHT_TO_VGA: [u8; 8] = [
    Color::DarkGray as u8,   // 90
    Color::LightRed as u8,   // 91
    Color::LightGreen as u8, // 92
    Color::Yellow as u8,     // 93
    Color::LightBlue as u8,  // 94
    Color::Pink as u8,       // 95
    Color::LightCyan as u8,  // 96
    Color::White as u8,      // 97
];

/// Map ANSI SGR foreground code (30-37) to VGA color.
fn ansi_fg_to_vga(code: u16) -> u8 {
    ANSI_TO_VGA[(code - 30) as usize]
}

/// Map ANSI SGR background code (40-47) to VGA color.
fn ansi_bg_to_vga(code: u16) -> u8 {
    ANSI_TO_VGA[(code - 40) as usize]
}

/// Map bright ANSI SGR foreground code (90-97) to VGA color.
fn ansi_bright_to_vga(code: u16) -> u8 {
    ANSI_BRIGHT_TO_VGA[(code - 90) as usize]
}

/// Apply a single SGR code. Returns `true` if a color attribute changed.
fn apply_sgr(state: &mut AnsiState, code: u16) -> bool {
    match code {
        0 => { state.reset_attrs(); true }
        1 => {
            state.bold = true;
            if state.fg < 8 { state.fg += 8; }
            true
        }
        3 => { state.italic = true; false }
        4 => { state.underline = true; false }
        22 => {
            state.bold = false;
            if state.fg >= 8 { state.fg -= 8; }
            true
        }
        23 => { state.italic = false; false }
        24 => { state.underline = false; false }
        30..=37 => {
            state.fg = ansi_fg_to_vga(code);
            if state.bold && state.fg < 8 { state.fg += 8; }
            true
        }
        40..=47 => { state.bg = ansi_bg_to_vga(code); true }
        90..=97 => { state.fg = ansi_bright_to_vga(code); true }
        _ => false,
    }
}

/// Feed one byte through the ANSI state machine and return resulting actions.
///
/// Most bytes produce zero or one action. An SGR sequence with multiple
/// parameters (e.g. `ESC[1;31m`) can yield several [`AnsiAction::SetColor`]
/// values as each parameter is applied.
///
/// # Example
/// ```ignore
/// let mut state = AnsiState::new();
/// let mut actions = Vec::new();
/// for &b in b"\x1b[31mHello" {
///     actions.extend(process_byte(&mut state, b));
/// }
/// // SetColor(0x04), PrintChar(b'H'), PrintChar(b'e'), ...
/// ```
pub fn process_byte(state: &mut AnsiState, byte: u8) -> Vec<AnsiAction> {
    let mut out = Vec::new();

    match state.phase {
        Phase::Ground => {
            if byte == 0x1B {
                state.phase = Phase::Escape;
            } else {
                out.push(AnsiAction::PrintChar(byte));
            }
        }

        Phase::Escape => {
            if byte == b'[' {
                state.phase = Phase::Csi;
                state.begin_csi();
            } else {
                // Unknown escape — drop ESC, emit byte as-is.
                state.phase = Phase::Ground;
                out.push(AnsiAction::PrintChar(byte));
            }
        }

        Phase::Csi => match byte {
            b'0'..=b'9' => {
                let idx = state.param_count;
                if idx < MAX_PARAMS {
                    state.params[idx] = state.params[idx]
                        .saturating_mul(10)
                        .saturating_add((byte - b'0') as u16);
                    state.param_started = true;
                }
            }
            b';' => {
                if state.param_count < MAX_PARAMS {
                    if !state.param_started {
                        state.params[state.param_count] = 0;
                    }
                    state.param_count += 1;
                    state.param_started = false;
                }
            }
            // SGR — Select Graphic Rendition
            b'm' => {
                state.phase = Phase::Ground;
                let count = state.finalize_params();
                if count == 0 {
                    apply_sgr(state, 0);
                    out.push(AnsiAction::Reset);
                    out.push(AnsiAction::SetColor(state.vga_attr()));
                } else {
                    for i in 0..count {
                        let code = state.params[i];
                        let changed = apply_sgr(state, code);
                        if code == 0 { out.push(AnsiAction::Reset); }
                        if changed { out.push(AnsiAction::SetColor(state.vga_attr())); }
                    }
                }
            }
            // CUU/CUD/CUF/CUB — Cursor Up/Down/Forward/Backward
            b'A' | b'B' | b'C' | b'D' => {
                state.phase = Phase::Ground;
                state.finalize_params();
                let n = state.param(0, 1) as i32;
                let (r, c) = match byte {
                    b'A' => (-n, 0),
                    b'B' => (n, 0),
                    b'C' => (0, n),
                    _    => (0, -n), // D
                };
                out.push(AnsiAction::MoveCursor { rows: r, cols: c });
            }
            // CUP — Cursor Position (H) / HVP (f)
            b'H' | b'f' => {
                state.phase = Phase::Ground;
                state.finalize_params();
                let row = state.param(0, 1);
                let col = state.param(1, 1);
                out.push(AnsiAction::SetCursorPos { row, col });
            }
            // ED — Erase in Display
            b'J' => {
                state.phase = Phase::Ground;
                state.finalize_params();
                out.push(AnsiAction::EraseScreen(state.param(0, 0) as u8));
            }
            // EL — Erase in Line
            b'K' => {
                state.phase = Phase::Ground;
                state.finalize_params();
                out.push(AnsiAction::EraseLine(state.param(0, 0) as u8));
            }
            _ => {
                // Unrecognized final byte — abort sequence.
                state.phase = Phase::Ground;
            }
        },
    }

    out
}

/// Convenience: process an entire byte slice and collect all actions.
pub fn process_bytes(state: &mut AnsiState, data: &[u8]) -> Vec<AnsiAction> {
    let mut out = Vec::new();
    for &b in data {
        out.extend(process_byte(state, b));
    }
    out
}
