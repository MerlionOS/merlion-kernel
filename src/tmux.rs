/// Terminal multiplexer for MerlionOS — provides tmux/screen-style pane
/// management within the VGA text-mode console.  Supports horizontal and
/// vertical splits, per-pane scroll buffers, keyboard routing to the active
/// pane, and border rendering.

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use crate::keyboard::KeyEvent;
use spin::Mutex;

/// VGA text mode dimensions.
const VGA_W: usize = 80;
const VGA_H: usize = 25;
const MAX_PANES: usize = 8;
const MAX_SCROLL: usize = 200;
const VGA_BUF: usize = 0xB8000;

/// VGA colour helpers reused from `crate::vga`.
const BORDER_ATTR: u8 = 0x08; // dark-gray on black
const ACTIVE_ATTR: u8 = 0x0B; // light-cyan on black
const CONTENT_ATTR: u8 = 0x07; // light-gray on black
const STATUS_ATTR: u8 = 0x70; // black on light-gray

// ── Pane ─────────────────────────────────────────────────────────────────────

/// A single terminal pane with its own scroll buffer and cursor.
pub struct Pane {
    /// Unique pane identifier within the session.
    pub id: usize,
    /// Width in columns (content area).
    pub width: usize,
    /// Height in rows (content area).
    pub height: usize,
    /// Scroll-back buffer holding completed lines.
    pub scroll_buf: Vec<String>,
    /// Cursor column within the pane.
    pub cursor_x: usize,
    /// Cursor row within the pane.
    pub cursor_y: usize,
    /// Command currently running in this pane.
    pub active_command: String,
    /// Top-left column on the VGA screen.
    col0: usize,
    /// Top-left row on the VGA screen.
    row0: usize,
    /// Current incomplete line.
    line_buf: String,
}

impl Pane {
    fn new(id: usize, w: usize, h: usize, col0: usize, row0: usize) -> Self {
        Self { id, width: w, height: h, scroll_buf: Vec::new(),
               cursor_x: 0, cursor_y: 0, active_command: String::from("shell"),
               col0, row0, line_buf: String::new() }
    }

    /// Write a character, advancing the cursor and scrolling as needed.
    pub fn write_char(&mut self, ch: char) {
        match ch {
            '\n' => {
                let line = core::mem::replace(&mut self.line_buf, String::new());
                self.scroll_buf.push(line);
                if self.scroll_buf.len() > MAX_SCROLL { self.scroll_buf.remove(0); }
                self.cursor_x = 0;
                if self.cursor_y + 1 < self.height { self.cursor_y += 1; }
            }
            '\x08' => { if self.cursor_x > 0 { self.cursor_x -= 1; self.line_buf.pop(); } }
            c => {
                if self.cursor_x >= self.width {
                    self.write_char('\n');
                }
                self.line_buf.push(c);
                self.cursor_x += 1;
            }
        }
    }

    /// Write a full string into the pane.
    pub fn write_str(&mut self, s: &str) { for ch in s.chars() { self.write_char(ch); } }

    /// Clear pane content and reset cursor.
    pub fn clear(&mut self) {
        self.scroll_buf.clear();
        self.line_buf.clear();
        self.cursor_x = 0;
        self.cursor_y = 0;
    }
}

// ── Layout ───────────────────────────────────────────────────────────────────

/// Describes how panes are arranged within a session.
#[derive(Clone, Copy, PartialEq)]
pub enum PaneLayout {
    /// A single full-screen pane.
    Single,
    /// Two panes stacked (top / bottom).
    HorizontalSplit,
    /// Two panes side-by-side (left | right).
    VerticalSplit,
}

/// Direction for switching pane focus.
#[derive(Clone, Copy)]
pub enum Direction { Next, Prev }

// ── Session ──────────────────────────────────────────────────────────────────

/// A multiplexer session containing one or more panes.
pub struct Session {
    pub id: usize,
    pub panes: Vec<Pane>,
    pub active_pane: usize,
    pub layout: PaneLayout,
}

impl Session {
    fn new(id: usize) -> Self {
        let h = VGA_H - 1; // reserve last row for status bar
        Self { id, panes: vec![Pane::new(0, VGA_W, h, 0, 0)],
               active_pane: 0, layout: PaneLayout::Single }
    }

    /// Split the active pane horizontally (top / bottom).
    pub fn split_horizontal(&mut self) {
        if self.panes.len() >= MAX_PANES { return; }
        let h = VGA_H - 1;
        let top_h = h / 2;
        let bot_h = h - top_h - 1;
        if let Some(p) = self.panes.first_mut() { p.height = top_h; p.width = VGA_W; }
        let nid = self.panes.len();
        self.panes.push(Pane::new(nid, VGA_W, bot_h, 0, top_h + 1));
        self.layout = PaneLayout::HorizontalSplit;
    }

    /// Split the active pane vertically (left | right).
    pub fn split_vertical(&mut self) {
        if self.panes.len() >= MAX_PANES { return; }
        let h = VGA_H - 1;
        let left_w = VGA_W / 2;
        let right_w = VGA_W - left_w - 1;
        if let Some(p) = self.panes.first_mut() { p.width = left_w; p.height = h; }
        let nid = self.panes.len();
        self.panes.push(Pane::new(nid, right_w, h, left_w + 1, 0));
        self.layout = PaneLayout::VerticalSplit;
    }

    /// Switch pane focus in the given direction.
    pub fn switch_pane(&mut self, dir: Direction) {
        if self.panes.len() <= 1 { return; }
        match dir {
            Direction::Next => self.active_pane = (self.active_pane + 1) % self.panes.len(),
            Direction::Prev => self.active_pane = if self.active_pane == 0
                { self.panes.len() - 1 } else { self.active_pane - 1 },
        }
    }

    /// Resize the active pane by `delta` (positive = grow).
    pub fn resize_pane(&mut self, delta: isize) {
        if self.panes.len() < 2 { return; }
        let (a, b) = (self.active_pane, if self.active_pane == 0 { 1 } else { 0 });
        match self.layout {
            PaneLayout::VerticalSplit => {
                let total = self.panes[a].width + self.panes[b].width + 1;
                let nw = (self.panes[a].width as isize + delta).clamp(4, total as isize - 5) as usize;
                self.panes[a].width = nw;
                self.panes[b].width = total - nw - 1;
                self.panes[0].col0 = 0;
                self.panes[1].col0 = self.panes[0].width + 1;
            }
            PaneLayout::HorizontalSplit => {
                let total = self.panes[a].height + self.panes[b].height + 1;
                let nh = (self.panes[a].height as isize + delta).clamp(2, total as isize - 3) as usize;
                self.panes[a].height = nh;
                self.panes[b].height = total - nh - 1;
                self.panes[0].row0 = 0;
                self.panes[1].row0 = self.panes[0].height + 1;
            }
            PaneLayout::Single => {}
        }
    }

    /// Close the active pane; revert to single layout when one remains.
    pub fn close_pane(&mut self) {
        if self.panes.len() <= 1 { return; }
        self.panes.remove(self.active_pane);
        if self.active_pane >= self.panes.len() { self.active_pane = self.panes.len() - 1; }
        if self.panes.len() == 1 {
            self.layout = PaneLayout::Single;
            let p = &mut self.panes[0];
            p.width = VGA_W; p.height = VGA_H - 1; p.col0 = 0; p.row0 = 0;
        }
    }

    /// Render all panes with borders and status bar onto the VGA buffer.
    pub fn render(&self) {
        let buf = VGA_BUF as *mut u8;
        // Clear screen
        for i in 0..(VGA_W * VGA_H) {
            unsafe { buf.add(i * 2).write_volatile(b' '); buf.add(i * 2 + 1).write_volatile(CONTENT_ATTR); }
        }
        for (pi, pane) in self.panes.iter().enumerate() {
            let battr = if pi == self.active_pane { ACTIVE_ATTR } else { BORDER_ATTR };
            // Content: visible portion of scroll_buf + current line
            let vis_start = pane.scroll_buf.len().saturating_sub(pane.height);
            for row in 0..pane.height {
                let idx = vis_start + row;
                let text = if idx < pane.scroll_buf.len() { Some(pane.scroll_buf[idx].as_str()) }
                           else if idx == pane.scroll_buf.len() { Some(pane.line_buf.as_str()) }
                           else { None };
                if let Some(t) = text {
                    let sr = pane.row0 + row;
                    for (ci, ch) in t.bytes().enumerate().take(pane.width) {
                        let sc = pane.col0 + ci;
                        if sr < VGA_H && sc < VGA_W {
                            let off = (sr * VGA_W + sc) * 2;
                            unsafe { buf.add(off).write_volatile(ch); buf.add(off + 1).write_volatile(CONTENT_ATTR); }
                        }
                    }
                }
            }
            // Vertical border (right of first pane in vertical split)
            if self.layout == PaneLayout::VerticalSplit && pi == 0 {
                let bc = pane.col0 + pane.width;
                for r in 0..(VGA_H - 1) { Self::put(buf, r, bc, 0xB3, battr); }
            }
            // Horizontal border (below first pane in horizontal split)
            if self.layout == PaneLayout::HorizontalSplit && pi == 0 {
                let br = pane.row0 + pane.height;
                for c in 0..VGA_W { Self::put(buf, br, c, 0xC4, battr); }
            }
        }
        // Status bar
        let row = VGA_H - 1;
        for c in 0..VGA_W { Self::put(buf, row, c, b' ', STATUS_ATTR); }
        let mut col = 1;
        for &b in b"[tmux] " { Self::put(buf, row, col, b, STATUS_ATTR); col += 1; }
        for (i, pane) in self.panes.iter().enumerate() {
            let marker = if i == self.active_pane { b'*' } else { b' ' };
            for &b in &[b'0' + (i as u8 % 10), b':', marker] {
                Self::put(buf, row, col, b, STATUS_ATTR); col += 1;
            }
            for b in pane.active_command.bytes() { Self::put(buf, row, col, b, STATUS_ATTR); col += 1; }
            Self::put(buf, row, col, b' ', STATUS_ATTR); col += 1;
        }
    }

    /// Write a byte+attr at (row, col) in VGA memory.
    fn put(buf: *mut u8, row: usize, col: usize, byte: u8, attr: u8) {
        if row < VGA_H && col < VGA_W {
            let off = (row * VGA_W + col) * 2;
            unsafe { buf.add(off).write_volatile(byte); buf.add(off + 1).write_volatile(attr); }
        }
    }
}

// ── Global API ───────────────────────────────────────────────────────────────

struct TmuxState { sessions: Vec<Session>, active: usize, next_id: usize }
impl TmuxState { const fn new() -> Self { Self { sessions: Vec::new(), active: 0, next_id: 0 } } }
static TMUX: Mutex<TmuxState> = Mutex::new(TmuxState::new());

/// Create a new session and return its id.
pub fn create_session() -> usize {
    let mut st = TMUX.lock();
    let id = st.next_id; st.next_id += 1;
    st.sessions.push(Session::new(id));
    st.active = st.sessions.len() - 1;
    id
}

/// Split the active pane horizontally in the current session.
pub fn split_horizontal() {
    let mut st = TMUX.lock();
    if let Some(s) = { let idx = st.active; st.sessions.get_mut(idx) } { s.split_horizontal(); }
}

/// Split the active pane vertically in the current session.
pub fn split_vertical() {
    let mut st = TMUX.lock();
    if let Some(s) = { let idx = st.active; st.sessions.get_mut(idx) } { s.split_vertical(); }
}

/// Switch pane focus in the given direction.
pub fn switch_pane(dir: Direction) {
    let mut st = TMUX.lock();
    if let Some(s) = { let idx = st.active; st.sessions.get_mut(idx) } { s.switch_pane(dir); }
}

/// Resize the active pane by `delta` characters.
pub fn resize_pane(delta: isize) {
    let mut st = TMUX.lock();
    if let Some(s) = { let idx = st.active; st.sessions.get_mut(idx) } { s.resize_pane(delta); }
}

/// Close the active pane in the current session.
pub fn close_pane() {
    let mut st = TMUX.lock();
    if let Some(s) = { let idx = st.active; st.sessions.get_mut(idx) } { s.close_pane(); }
}

/// Render the current session to VGA text-mode memory.
pub fn render_session() {
    let st = TMUX.lock();
    if let Some(s) = { let idx = st.active; st.sessions.get(idx) } { s.render(); }
}

/// Route a keyboard event to the active pane of the current session.
pub fn route_key(event: KeyEvent) {
    let mut st = TMUX.lock();
    let s = match { let idx = st.active; st.sessions.get_mut(idx) } { Some(s) => s, None => return };
    let pane = match s.panes.get_mut(s.active_pane) { Some(p) => p, None => return };
    match event {
        KeyEvent::Char(c) => pane.write_char(c),
        KeyEvent::ArrowLeft  => { if pane.cursor_x > 0 { pane.cursor_x -= 1; } }
        KeyEvent::ArrowRight => { if pane.cursor_x < pane.width.saturating_sub(1) { pane.cursor_x += 1; } }
        KeyEvent::ArrowUp | KeyEvent::ArrowDown => {} // reserved for scroll-back
        KeyEvent::Home   => pane.cursor_x = 0,
        KeyEvent::End    => pane.cursor_x = pane.line_buf.len().min(pane.width.saturating_sub(1)),
        KeyEvent::Delete => { if pane.cursor_x < pane.line_buf.len() { pane.line_buf.remove(pane.cursor_x); } }
        KeyEvent::Escape => {} // ignored in tmux
    }
}
