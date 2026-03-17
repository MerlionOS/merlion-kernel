/// Framebuffer terminal emulator for MerlionOS.
/// Replaces VGA text mode with a pixel-based terminal
/// supporting Unicode basics, 256 colors, and scrollback.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::font;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const CHAR_W: u32 = 8;
const CHAR_H: u32 = 16;
const DEFAULT_SCROLLBACK: usize = 1000;
const DEFAULT_TAB_WIDTH: u32 = 8;

// ---------------------------------------------------------------------------
// Color type
// ---------------------------------------------------------------------------

/// RGB color packed in a u32 (0x00RRGGBB).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub u32);

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self(((r as u32) << 16) | ((g as u32) << 8) | (b as u32))
    }

    pub fn r(self) -> u8 { ((self.0 >> 16) & 0xFF) as u8 }
    pub fn g(self) -> u8 { ((self.0 >> 8) & 0xFF) as u8 }
    pub fn b(self) -> u8 { (self.0 & 0xFF) as u8 }
}

// Standard 8 + bright 8 colors
static ANSI_16: [Color; 16] = [
    Color(0x00000000), Color(0x00AA0000), Color(0x0000AA00), Color(0x00AA5500),
    Color(0x000000AA), Color(0x00AA00AA), Color(0x0000AAAA), Color(0x00AAAAAA),
    Color(0x00555555), Color(0x00FF5555), Color(0x0055FF55), Color(0x00FFFF55),
    Color(0x005555FF), Color(0x00FF55FF), Color(0x0055FFFF), Color(0x00FFFFFF),
];

/// Map a 256-color index to an RGB Color.
fn color_256(idx: u8) -> Color {
    if idx < 16 {
        ANSI_16[idx as usize]
    } else if idx < 232 {
        // 6x6x6 color cube: indices 16..231
        let n = (idx - 16) as u32;
        let ri = n / 36;
        let gi = (n % 36) / 6;
        let bi = n % 6;
        // Each component: 0,95,135,175,215,255
        let comp = |c: u32| -> u8 {
            if c == 0 { 0 } else { (55 + c * 40) as u8 }
        };
        Color::rgb(comp(ri), comp(gi), comp(bi))
    } else {
        // Grayscale ramp: indices 232..255 -> 8,18,...,238
        let g = (8 + (idx - 232) as u32 * 10) as u8;
        Color::rgb(g, g, g)
    }
}

// ---------------------------------------------------------------------------
// Cursor style
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CursorStyle {
    Block,
    Underline,
    Bar,
}

// ---------------------------------------------------------------------------
// Terminal actions (from ANSI parser)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum TermAction {
    Print(u8),
    Newline,
    CarriageReturn,
    Tab,
    Backspace,
    Bell,
    CursorUp(u32),
    CursorDown(u32),
    CursorForward(u32),
    CursorBackward(u32),
    CursorPosition(u32, u32),
    EraseDisplay(u32),
    EraseLine(u32),
    SetGraphics(Vec<u32>),
}

// ---------------------------------------------------------------------------
// ANSI escape sequence parser
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum ParseState {
    Normal,
    Escape,
    Csi,
}

struct AnsiParser {
    state: ParseState,
    params: Vec<u32>,
    current_param: u32,
    has_param: bool,
}

impl AnsiParser {
    fn new() -> Self {
        Self {
            state: ParseState::Normal,
            params: Vec::new(),
            current_param: 0,
            has_param: false,
        }
    }

    fn reset(&mut self) {
        self.state = ParseState::Normal;
        self.params.clear();
        self.current_param = 0;
        self.has_param = false;
    }

    fn feed(&mut self, byte: u8) -> Option<TermAction> {
        match self.state {
            ParseState::Normal => {
                match byte {
                    0x1B => { self.state = ParseState::Escape; None }
                    b'\n' => Some(TermAction::Newline),
                    b'\r' => Some(TermAction::CarriageReturn),
                    b'\t' => Some(TermAction::Tab),
                    0x08 => Some(TermAction::Backspace),
                    0x07 => Some(TermAction::Bell),
                    _ => Some(TermAction::Print(byte)),
                }
            }
            ParseState::Escape => {
                match byte {
                    b'[' => {
                        self.state = ParseState::Csi;
                        self.params.clear();
                        self.current_param = 0;
                        self.has_param = false;
                        None
                    }
                    _ => {
                        self.reset();
                        None
                    }
                }
            }
            ParseState::Csi => {
                match byte {
                    b'0'..=b'9' => {
                        self.current_param = self.current_param * 10 + (byte - b'0') as u32;
                        self.has_param = true;
                        None
                    }
                    b';' => {
                        self.params.push(self.current_param);
                        self.current_param = 0;
                        self.has_param = false;
                        None
                    }
                    b'A' => {
                        let n = if self.has_param { self.current_param.max(1) } else { 1 };
                        self.reset();
                        Some(TermAction::CursorUp(n))
                    }
                    b'B' => {
                        let n = if self.has_param { self.current_param.max(1) } else { 1 };
                        self.reset();
                        Some(TermAction::CursorDown(n))
                    }
                    b'C' => {
                        let n = if self.has_param { self.current_param.max(1) } else { 1 };
                        self.reset();
                        Some(TermAction::CursorForward(n))
                    }
                    b'D' => {
                        let n = if self.has_param { self.current_param.max(1) } else { 1 };
                        self.reset();
                        Some(TermAction::CursorBackward(n))
                    }
                    b'H' | b'f' => {
                        if self.has_param {
                            self.params.push(self.current_param);
                        }
                        let row = if !self.params.is_empty() { self.params[0].max(1) - 1 } else { 0 };
                        let col = if self.params.len() > 1 { self.params[1].max(1) - 1 } else { 0 };
                        self.reset();
                        Some(TermAction::CursorPosition(row, col))
                    }
                    b'J' => {
                        let n = if self.has_param { self.current_param } else { 0 };
                        self.reset();
                        Some(TermAction::EraseDisplay(n))
                    }
                    b'K' => {
                        let n = if self.has_param { self.current_param } else { 0 };
                        self.reset();
                        Some(TermAction::EraseLine(n))
                    }
                    b'm' => {
                        if self.has_param {
                            self.params.push(self.current_param);
                        }
                        let params = core::mem::take(&mut self.params);
                        self.reset();
                        Some(TermAction::SetGraphics(params))
                    }
                    _ => {
                        // Unknown CSI final byte — ignore
                        self.reset();
                        None
                    }
                }
            }
        }
    }
}

/// Parse a full input byte slice into terminal actions.
pub fn parse_ansi(input: &[u8]) -> Vec<TermAction> {
    let mut parser = AnsiParser::new();
    let mut actions = Vec::new();
    for &b in input {
        if let Some(action) = parser.feed(b) {
            actions.push(action);
        }
    }
    actions
}

// ---------------------------------------------------------------------------
// Terminal state
// ---------------------------------------------------------------------------

pub struct FbTerminal {
    cols: u32,
    rows: u32,
    cursor_col: u32,
    cursor_row: u32,
    fg_color: Color,
    bg_color: Color,
    bold: bool,
    inverse: bool,
    underline: bool,
    scrollback: Vec<String>,
    max_scrollback: usize,
    scroll_offset: usize,
    tab_width: u32,
    cursor_visible: bool,
    cursor_style: CursorStyle,
    cursor_blink_on: bool,
    /// Selection start (row, col)
    sel_start: Option<(u32, u32)>,
    /// Selection end (row, col)
    sel_end: Option<(u32, u32)>,
    /// Current line buffer being built
    current_line: String,
    /// Parser state for escape sequences
    parser: AnsiParser,
    /// Framebuffer address (for rendering)
    fb_addr: u64,
    fb_width: u32,
    fb_height: u32,
    fb_pitch: u32,
    fb_bpp: u8,
    fb_red_shift: u8,
    fb_green_shift: u8,
    fb_blue_shift: u8,
    active: bool,
}

impl FbTerminal {
    const fn new() -> Self {
        Self {
            cols: 0,
            rows: 0,
            cursor_col: 0,
            cursor_row: 0,
            fg_color: Color(0x00CCCCCC),
            bg_color: Color(0x00000000),
            bold: false,
            inverse: false,
            underline: false,
            scrollback: Vec::new(),
            max_scrollback: DEFAULT_SCROLLBACK,
            scroll_offset: 0,
            tab_width: DEFAULT_TAB_WIDTH,
            cursor_visible: true,
            cursor_style: CursorStyle::Block,
            cursor_blink_on: true,
            sel_start: None,
            sel_end: None,
            current_line: String::new(),
            parser: AnsiParser {
                state: ParseState::Normal,
                params: Vec::new(),
                current_param: 0,
                has_param: false,
            },
            fb_addr: 0,
            fb_width: 0,
            fb_height: 0,
            fb_pitch: 0,
            fb_bpp: 4,
            fb_red_shift: 16,
            fb_green_shift: 8,
            fb_blue_shift: 0,
            active: false,
        }
    }
}

static TERMINAL: Mutex<FbTerminal> = Mutex::new(FbTerminal::new());
static BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);
static LINES_SCROLLED: AtomicU64 = AtomicU64::new(0);
static ESCAPE_SEQS: AtomicU64 = AtomicU64::new(0);
static BLINK_COUNTER: AtomicU32 = AtomicU32::new(0);
static TERM_ACTIVE: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Rendering helpers (internal to terminal)
// ---------------------------------------------------------------------------

#[inline]
fn term_pack_color(term: &FbTerminal, c: Color) -> u32 {
    ((c.r() as u32) << term.fb_red_shift)
        | ((c.g() as u32) << term.fb_green_shift)
        | ((c.b() as u32) << term.fb_blue_shift)
}

fn term_draw_char(term: &FbTerminal, col: u32, row: u32, ch: u8, fg: Color, bg: Color) {
    let px = col * CHAR_W;
    let py = row * CHAR_H;
    let fg_packed = term_pack_color(term, fg);
    let bg_packed = term_pack_color(term, bg);
    let glyph = font::glyph(ch);
    let base = term.fb_addr as *mut u8;
    let pitch = term.fb_pitch;
    let bpp = term.fb_bpp;

    for r in 0..CHAR_H {
        let bits = glyph[r as usize];
        for c in 0..CHAR_W {
            let dx = px + c;
            let dy = py + r;
            if dx >= term.fb_width || dy >= term.fb_height {
                continue;
            }
            let on = (bits >> (7 - c)) & 1 != 0;
            let color = if on { fg_packed } else { bg_packed };
            let offset = (dy as usize) * (pitch as usize) + (dx as usize) * (bpp as usize);
            unsafe {
                let ptr = base.add(offset);
                ptr.write_volatile(color as u8);
                ptr.add(1).write_volatile((color >> 8) as u8);
                ptr.add(2).write_volatile((color >> 16) as u8);
                if bpp == 4 {
                    ptr.add(3).write_volatile(0);
                }
            }
        }
    }
}

fn term_clear_row(term: &FbTerminal, row: u32) {
    let bg = term.bg_color;
    for c in 0..term.cols {
        term_draw_char(term, c, row, b' ', bg, bg);
    }
}

fn term_scroll_up_one(term: &FbTerminal) {
    let pitch = term.fb_pitch as usize;
    let char_h = CHAR_H as usize;
    let line_bytes = char_h * pitch;
    let total_bytes = term.fb_height as usize * pitch;
    let base = term.fb_addr as *mut u8;

    if line_bytes >= total_bytes {
        return;
    }
    unsafe {
        core::ptr::copy(base.add(line_bytes), base, total_bytes - line_bytes);
        core::ptr::write_bytes(base.add(total_bytes - line_bytes), 0, line_bytes);
    }
    LINES_SCROLLED.fetch_add(1, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Text rendering / terminal operations
// ---------------------------------------------------------------------------

/// Process a single terminal action.
fn process_action(term: &mut FbTerminal, action: TermAction) {
    if !term.active || term.cols == 0 {
        return;
    }

    let (fg, bg) = if term.inverse {
        (term.bg_color, term.fg_color)
    } else {
        (term.fg_color, term.bg_color)
    };
    let draw_fg = if term.bold {
        Color(fg.0 | 0x00555555)
    } else {
        fg
    };

    match action {
        TermAction::Print(ch) => {
            term_draw_char(term, term.cursor_col, term.cursor_row, ch, draw_fg, bg);
            term.current_line.push(ch as char);
            term.cursor_col += 1;
            if term.cursor_col >= term.cols {
                term.cursor_col = 0;
                term.cursor_row += 1;
                if term.cursor_row >= term.rows {
                    // Save line to scrollback
                    let line = core::mem::take(&mut term.current_line);
                    term.scrollback.push(line);
                    if term.scrollback.len() > term.max_scrollback {
                        term.scrollback.remove(0);
                    }
                    term_scroll_up_one(term);
                    term.cursor_row = term.rows - 1;
                }
            }
        }
        TermAction::Newline => {
            // Save current line to scrollback
            let line = core::mem::take(&mut term.current_line);
            if !line.is_empty() {
                term.scrollback.push(line);
                if term.scrollback.len() > term.max_scrollback {
                    term.scrollback.remove(0);
                }
            }
            term.cursor_col = 0;
            term.cursor_row += 1;
            if term.cursor_row >= term.rows {
                term_scroll_up_one(term);
                term.cursor_row = term.rows - 1;
            }
        }
        TermAction::CarriageReturn => {
            term.cursor_col = 0;
        }
        TermAction::Tab => {
            let next = (term.cursor_col / term.tab_width + 1) * term.tab_width;
            term.cursor_col = next.min(term.cols - 1);
        }
        TermAction::Backspace => {
            if term.cursor_col > 0 {
                term.cursor_col -= 1;
                term_draw_char(term, term.cursor_col, term.cursor_row, b' ', bg, bg);
                term.current_line.pop();
            }
        }
        TermAction::Bell => {
            // No audible bell — ignore
        }
        TermAction::CursorUp(n) => {
            term.cursor_row = term.cursor_row.saturating_sub(n);
        }
        TermAction::CursorDown(n) => {
            term.cursor_row = (term.cursor_row + n).min(term.rows - 1);
        }
        TermAction::CursorForward(n) => {
            term.cursor_col = (term.cursor_col + n).min(term.cols - 1);
        }
        TermAction::CursorBackward(n) => {
            term.cursor_col = term.cursor_col.saturating_sub(n);
        }
        TermAction::CursorPosition(row, col) => {
            term.cursor_row = row.min(term.rows.saturating_sub(1));
            term.cursor_col = col.min(term.cols.saturating_sub(1));
        }
        TermAction::EraseDisplay(mode) => {
            match mode {
                0 => {
                    // Erase from cursor to end
                    for c in term.cursor_col..term.cols {
                        term_draw_char(term, c, term.cursor_row, b' ', bg, bg);
                    }
                    for r in (term.cursor_row + 1)..term.rows {
                        term_clear_row(term, r);
                    }
                }
                1 => {
                    // Erase from start to cursor
                    for r in 0..term.cursor_row {
                        term_clear_row(term, r);
                    }
                    for c in 0..=term.cursor_col {
                        term_draw_char(term, c, term.cursor_row, b' ', bg, bg);
                    }
                }
                2 => {
                    // Erase entire display
                    for r in 0..term.rows {
                        term_clear_row(term, r);
                    }
                }
                _ => {}
            }
        }
        TermAction::EraseLine(mode) => {
            match mode {
                0 => {
                    for c in term.cursor_col..term.cols {
                        term_draw_char(term, c, term.cursor_row, b' ', bg, bg);
                    }
                }
                1 => {
                    for c in 0..=term.cursor_col {
                        term_draw_char(term, c, term.cursor_row, b' ', bg, bg);
                    }
                }
                2 => {
                    term_clear_row(term, term.cursor_row);
                }
                _ => {}
            }
        }
        TermAction::SetGraphics(params) => {
            ESCAPE_SEQS.fetch_add(1, Ordering::Relaxed);
            apply_sgr(term, &params);
        }
    }
}

/// Apply SGR (Select Graphic Rendition) parameters.
fn apply_sgr(term: &mut FbTerminal, params: &[u32]) {
    if params.is_empty() {
        // Reset
        term.fg_color = Color(0x00CCCCCC);
        term.bg_color = Color(0x00000000);
        term.bold = false;
        term.inverse = false;
        term.underline = false;
        return;
    }

    let mut i = 0;
    while i < params.len() {
        let p = params[i];
        match p {
            0 => {
                term.fg_color = Color(0x00CCCCCC);
                term.bg_color = Color(0x00000000);
                term.bold = false;
                term.inverse = false;
                term.underline = false;
            }
            1 => { term.bold = true; }
            4 => { term.underline = true; }
            7 => { term.inverse = true; }
            22 => { term.bold = false; }
            24 => { term.underline = false; }
            27 => { term.inverse = false; }
            30..=37 => { term.fg_color = ANSI_16[(p - 30) as usize]; }
            38 => {
                // 256-color foreground: 38;5;N
                if i + 2 < params.len() && params[i + 1] == 5 {
                    term.fg_color = color_256(params[i + 2] as u8);
                    i += 2;
                }
            }
            39 => { term.fg_color = Color(0x00CCCCCC); } // default fg
            40..=47 => { term.bg_color = ANSI_16[(p - 40) as usize]; }
            48 => {
                // 256-color background: 48;5;N
                if i + 2 < params.len() && params[i + 1] == 5 {
                    term.bg_color = color_256(params[i + 2] as u8);
                    i += 2;
                }
            }
            49 => { term.bg_color = Color(0x00000000); } // default bg
            90..=97 => { term.fg_color = ANSI_16[(p - 90 + 8) as usize]; }
            100..=107 => { term.bg_color = ANSI_16[(p - 100 + 8) as usize]; }
            _ => {}
        }
        i += 1;
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Process a single character through the terminal emulator.
pub fn term_putc(ch: u8) {
    let mut term = TERMINAL.lock();
    let action = term.parser.feed(ch);
    if let Some(a) = action {
        process_action(&mut term, a);
    }
    BYTES_WRITTEN.fetch_add(1, Ordering::Relaxed);
}

/// Process a byte stream through the terminal emulator.
pub fn term_write(data: &[u8]) {
    let mut term = TERMINAL.lock();
    for &b in data {
        let action = term.parser.feed(b);
        if let Some(a) = action {
            process_action(&mut term, a);
        }
    }
    BYTES_WRITTEN.fetch_add(data.len() as u64, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Scrollback
// ---------------------------------------------------------------------------

/// Scroll up in the scrollback buffer (PageUp).
pub fn scroll_up() {
    let mut term = TERMINAL.lock();
    if term.scroll_offset < term.scrollback.len() {
        term.scroll_offset += 1;
    }
}

/// Scroll down in the scrollback buffer (PageDown).
pub fn scroll_down() {
    let mut term = TERMINAL.lock();
    if term.scroll_offset > 0 {
        term.scroll_offset -= 1;
    }
}

/// Jump to the bottom of the scrollback.
pub fn scroll_to_bottom() {
    let mut term = TERMINAL.lock();
    term.scroll_offset = 0;
}

// ---------------------------------------------------------------------------
// Cursor
// ---------------------------------------------------------------------------

/// Toggle cursor blink state (call from a timer at ~2 Hz).
pub fn blink_cursor() {
    BLINK_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut term = TERMINAL.lock();
    if !term.cursor_visible || !term.active {
        return;
    }
    term.cursor_blink_on = !term.cursor_blink_on;
    let col = term.cursor_col;
    let row = term.cursor_row;
    if term.cursor_blink_on {
        let fg = term.fg_color;
        match term.cursor_style {
            CursorStyle::Block => {
                term_draw_char(&term, col, row, b' ', Color(0x00000000), fg);
            }
            CursorStyle::Underline | CursorStyle::Bar => {
                term_draw_char(&term, col, row, b'_', fg, term.bg_color);
            }
        }
    } else {
        let bg = term.bg_color;
        term_draw_char(&term, col, row, b' ', bg, bg);
    }
}

/// Show the cursor.
pub fn show_cursor() {
    let mut term = TERMINAL.lock();
    term.cursor_visible = true;
}

/// Hide the cursor.
pub fn hide_cursor() {
    let mut term = TERMINAL.lock();
    term.cursor_visible = false;
}

/// Set cursor style.
pub fn set_cursor_style(style: CursorStyle) {
    let mut term = TERMINAL.lock();
    term.cursor_style = style;
}

// ---------------------------------------------------------------------------
// Selection (copy/paste)
// ---------------------------------------------------------------------------

/// Start a selection at (row, col).
pub fn select_start(row: u32, col: u32) {
    let mut term = TERMINAL.lock();
    term.sel_start = Some((row, col));
    term.sel_end = None;
}

/// Update selection end to (row, col).
pub fn select_end(row: u32, col: u32) {
    let mut term = TERMINAL.lock();
    term.sel_end = Some((row, col));
}

/// Get the text of the current selection.
pub fn get_selection() -> String {
    let term = TERMINAL.lock();
    let (start, end) = match (term.sel_start, term.sel_end) {
        (Some(s), Some(e)) => (s, e),
        _ => return String::new(),
    };

    // Normalize so start <= end
    let (sr, sc, er, ec) = if start.0 < end.0 || (start.0 == end.0 && start.1 <= end.1) {
        (start.0, start.1, end.0, end.1)
    } else {
        (end.0, end.1, start.0, start.1)
    };

    // Gather from scrollback lines within range
    let mut result = String::new();
    for row in sr..=er {
        let idx = row as usize;
        if idx < term.scrollback.len() {
            let line = &term.scrollback[idx];
            let start_col = if row == sr { sc as usize } else { 0 };
            let end_col = if row == er { (ec as usize) + 1 } else { line.len() };
            let end_col = end_col.min(line.len());
            if start_col < end_col {
                result.push_str(&line[start_col..end_col]);
            }
        }
        if row < er {
            result.push('\n');
        }
    }
    result
}

/// Clear selection.
pub fn clear_selection() {
    let mut term = TERMINAL.lock();
    term.sel_start = None;
    term.sel_end = None;
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the terminal on a given framebuffer.
pub fn term_init(
    fb_addr: u64,
    width: u32,
    height: u32,
    pitch: u32,
    bpp: u8,
    red_shift: u8,
    green_shift: u8,
    blue_shift: u8,
) {
    let mut term = TERMINAL.lock();
    term.fb_addr = fb_addr;
    term.fb_width = width;
    term.fb_height = height;
    term.fb_pitch = pitch;
    term.fb_bpp = bpp;
    term.fb_red_shift = red_shift;
    term.fb_green_shift = green_shift;
    term.fb_blue_shift = blue_shift;
    term.cols = if width > 0 { width / CHAR_W } else { 0 };
    term.rows = if height > 0 { height / CHAR_H } else { 0 };
    term.cursor_col = 0;
    term.cursor_row = 0;
    term.fg_color = Color(0x00CCCCCC);
    term.bg_color = Color(0x00000000);
    term.bold = false;
    term.inverse = false;
    term.underline = false;
    term.scrollback.clear();
    term.scroll_offset = 0;
    term.cursor_visible = true;
    term.cursor_style = CursorStyle::Block;
    term.active = true;
    TERM_ACTIVE.store(true, Ordering::SeqCst);
}

/// Module init — sets up counters. The terminal is not active until
/// term_init() is called with framebuffer info.
pub fn init() {
    BYTES_WRITTEN.store(0, Ordering::SeqCst);
    LINES_SCROLLED.store(0, Ordering::SeqCst);
    ESCAPE_SEQS.store(0, Ordering::SeqCst);
    BLINK_COUNTER.store(0, Ordering::SeqCst);
    TERM_ACTIVE.store(false, Ordering::SeqCst);
}

/// Check if the framebuffer terminal is active.
pub fn is_active() -> bool {
    TERM_ACTIVE.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Info & Stats
// ---------------------------------------------------------------------------

/// Return terminal info string.
pub fn fb_terminal_info() -> String {
    let term = TERMINAL.lock();
    let active = TERM_ACTIVE.load(Ordering::Relaxed);
    format!(
        "Framebuffer Terminal:\n  Active: {}\n  Grid: {}x{} chars\n  \
         Cursor: ({}, {})\n  Style: {:?}\n  Scrollback: {}/{}\n  \
         Scroll offset: {}\n  Bold: {} Inverse: {} Underline: {}\n  \
         Tab width: {}",
        active,
        term.cols, term.rows,
        term.cursor_col, term.cursor_row,
        term.cursor_style,
        term.scrollback.len(), term.max_scrollback,
        term.scroll_offset,
        term.bold, term.inverse, term.underline,
        term.tab_width,
    )
}

/// Return terminal statistics string.
pub fn fb_terminal_stats() -> String {
    let bw = BYTES_WRITTEN.load(Ordering::Relaxed);
    let ls = LINES_SCROLLED.load(Ordering::Relaxed);
    let es = ESCAPE_SEQS.load(Ordering::Relaxed);
    let bc = BLINK_COUNTER.load(Ordering::Relaxed);
    format!(
        "FB Terminal Stats:\n  Bytes written: {}\n  Lines scrolled: {}\n  \
         Escape sequences: {}\n  Blink toggles: {}",
        bw, ls, es, bc,
    )
}
