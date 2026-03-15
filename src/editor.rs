/// Minimal text editor — edit files directly in VGA text mode.
/// Ctrl+S to save, Ctrl+Q to quit (or Esc), arrow keys to move cursor.

use crate::{vga, vfs, keyboard::KeyEvent, serial_println};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

const MAX_LINES: usize = 22;   // editing area (rows 1-22, row 0=status, row 24=help)
const MAX_COLS: usize = 80;
const MAX_LINE_LEN: usize = 79;

static EDITING: AtomicBool = AtomicBool::new(false);
static EDITOR: Mutex<EditorState> = Mutex::new(EditorState::new());

struct EditorState {
    lines: [[u8; MAX_COLS]; MAX_LINES],
    line_lens: [usize; MAX_LINES],
    num_lines: usize,
    cursor_x: usize,
    cursor_y: usize,
    path: [u8; 64],
    path_len: usize,
    modified: bool,
}

impl EditorState {
    const fn new() -> Self {
        Self {
            lines: [[b' '; MAX_COLS]; MAX_LINES],
            line_lens: [0; MAX_LINES],
            num_lines: 1,
            cursor_x: 0,
            cursor_y: 0,
            path: [0; 64],
            path_len: 0,
            modified: false,
        }
    }

    fn clear(&mut self) {
        for line in &mut self.lines { line.fill(b' '); }
        self.line_lens.fill(0);
        self.num_lines = 1;
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.modified = false;
    }

    fn load_content(&mut self, content: &str) {
        self.clear();
        for (i, line) in content.lines().enumerate() {
            if i >= MAX_LINES { break; }
            let bytes = line.as_bytes();
            let len = bytes.len().min(MAX_LINE_LEN);
            self.lines[i][..len].copy_from_slice(&bytes[..len]);
            self.line_lens[i] = len;
            self.num_lines = i + 1;
        }
        if self.num_lines == 0 { self.num_lines = 1; }
    }

    fn to_string(&self) -> alloc::string::String {
        let mut s = alloc::string::String::new();
        for i in 0..self.num_lines {
            let len = self.line_lens[i];
            if let Ok(line) = core::str::from_utf8(&self.lines[i][..len]) {
                s.push_str(line);
            }
            if i < self.num_lines - 1 { s.push('\n'); }
        }
        s
    }

    fn insert_char(&mut self, ch: u8) {
        let y = self.cursor_y;
        let x = self.cursor_x;
        if self.line_lens[y] < MAX_LINE_LEN {
            // Shift chars right
            let len = self.line_lens[y];
            for i in (x..len).rev() {
                self.lines[y][i + 1] = self.lines[y][i];
            }
            self.lines[y][x] = ch;
            self.line_lens[y] += 1;
            self.cursor_x += 1;
            self.modified = true;
        }
    }

    fn delete_char(&mut self) {
        let y = self.cursor_y;
        let x = self.cursor_x;
        if x > 0 {
            let len = self.line_lens[y];
            for i in (x - 1)..len.saturating_sub(1) {
                let next = self.lines[y][i + 1];
                self.lines[y][i] = next;
            }
            self.lines[y][len.saturating_sub(1)] = b' ';
            self.line_lens[y] = len.saturating_sub(1);
            self.cursor_x -= 1;
            self.modified = true;
        } else if y > 0 {
            let prev_len = self.line_lens[y - 1];
            let cur_len = self.line_lens[y];
            let merge_len = (prev_len + cur_len).min(MAX_LINE_LEN);
            let to_copy = merge_len - prev_len;
            // Copy current line data to temp buffer to avoid borrow conflict
            let mut tmp = [0u8; MAX_COLS];
            tmp[..to_copy].copy_from_slice(&self.lines[y][..to_copy]);
            self.lines[y - 1][prev_len..prev_len + to_copy]
                .copy_from_slice(&tmp[..to_copy]);
            self.line_lens[y - 1] = merge_len;
            for i in y..self.num_lines.saturating_sub(1) {
                let next_line = self.lines[i + 1];
                let next_len = self.line_lens[i + 1];
                self.lines[i] = next_line;
                self.line_lens[i] = next_len;
            }
            self.num_lines = self.num_lines.saturating_sub(1).max(1);
            self.cursor_y -= 1;
            self.cursor_x = prev_len;
            self.modified = true;
        }
    }

    fn new_line(&mut self) {
        if self.num_lines >= MAX_LINES { return; }
        let y = self.cursor_y;
        let x = self.cursor_x;
        // Shift lines down
        for i in (y + 1..self.num_lines).rev() {
            let src = self.lines[i];
            let src_len = self.line_lens[i];
            self.lines[i + 1] = src;
            self.line_lens[i + 1] = src_len;
        }
        // Copy remaining chars to temp, then split
        let remaining = self.line_lens[y] - x;
        let mut tmp = [b' '; MAX_COLS];
        tmp[..remaining].copy_from_slice(&self.lines[y][x..x + remaining]);
        self.lines[y + 1] = tmp;
        self.line_lens[y + 1] = remaining;
        for i in x..MAX_COLS { self.lines[y][i] = b' '; }
        self.line_lens[y] = x;
        self.num_lines += 1;
        self.cursor_y += 1;
        self.cursor_x = 0;
        self.modified = true;
    }
}

/// Open the editor with a file path.
pub fn open(path: &str) {
    {
        let mut ed = EDITOR.lock();
        ed.clear();

        let bytes = path.as_bytes();
        let len = bytes.len().min(64);
        ed.path[..len].copy_from_slice(&bytes[..len]);
        ed.path_len = len;

        // Load existing content
        if let Ok(content) = vfs::cat(path) {
            ed.load_content(&content);
        }
    }

    EDITING.store(true, Ordering::SeqCst);
    redraw();
}

/// Handle key input during editing.
pub fn handle_input(event: KeyEvent) {
    if !EDITING.load(Ordering::SeqCst) { return; }

    let mut ed = EDITOR.lock();
    let mut need_redraw = true;

    match event {
        // Ctrl+Q or Esc = quit
        KeyEvent::Char('\x11') | KeyEvent::Char('\x1B') => {
            drop(ed);
            EDITING.store(false, Ordering::SeqCst);
            return;
        }
        // Ctrl+S = save
        KeyEvent::Char('\x13') => {
            let content = ed.to_string();
            let mut path_buf = [0u8; 64];
            let path_len = ed.path_len;
            path_buf[..path_len].copy_from_slice(&ed.path[..path_len]);
            ed.modified = false;
            let path = core::str::from_utf8(&path_buf[..path_len]).unwrap_or("");
            let _ = vfs::write(path, &content);
            serial_println!("[editor] saved '{}'", path);
        }
        // Enter
        KeyEvent::Char('\n') => ed.new_line(),
        // Backspace
        KeyEvent::Char('\x08') => ed.delete_char(),
        // Tab → 4 spaces
        KeyEvent::Char('\t') => {
            for _ in 0..4 { ed.insert_char(b' '); }
        }
        // Normal character
        KeyEvent::Char(ch) if ch.is_ascii() && !ch.is_ascii_control() => {
            ed.insert_char(ch as u8);
        }
        // Arrow keys
        KeyEvent::ArrowUp => {
            if ed.cursor_y > 0 {
                ed.cursor_y -= 1;
                ed.cursor_x = ed.cursor_x.min(ed.line_lens[ed.cursor_y]);
            }
        }
        KeyEvent::ArrowDown => {
            if ed.cursor_y < ed.num_lines - 1 {
                ed.cursor_y += 1;
                ed.cursor_x = ed.cursor_x.min(ed.line_lens[ed.cursor_y]);
            }
        }
        KeyEvent::ArrowLeft => {
            if ed.cursor_x > 0 { ed.cursor_x -= 1; }
        }
        KeyEvent::ArrowRight => {
            if ed.cursor_x < ed.line_lens[ed.cursor_y] { ed.cursor_x += 1; }
        }
        KeyEvent::Home => ed.cursor_x = 0,
        KeyEvent::End => ed.cursor_x = ed.line_lens[ed.cursor_y],
        _ => { need_redraw = false; }
    }

    if need_redraw {
        let cy = ed.cursor_y;
        let cx = ed.cursor_x;
        drop(ed);
        redraw();
        update_cursor(cx, cy);
    }
}

pub fn is_editing() -> bool {
    EDITING.load(Ordering::SeqCst)
}

fn redraw() {
    let ed = EDITOR.lock();
    let vga = 0xB8000 as *mut u8;

    // Status bar (row 0)
    let path = core::str::from_utf8(&ed.path[..ed.path_len]).unwrap_or("?");
    let modified = if ed.modified { " [modified]" } else { "" };
    let status = alloc::format!(" EDIT: {}{} — L{} C{}",
        path, modified, ed.cursor_y + 1, ed.cursor_x + 1);
    for x in 0..80 {
        let ch = status.as_bytes().get(x).copied().unwrap_or(b' ');
        unsafe {
            vga.add(x * 2).write_volatile(ch);
            vga.add(x * 2 + 1).write_volatile(0x70); // black on white
        }
    }

    // Editing area (rows 1-22)
    for y in 0..MAX_LINES {
        let row = y + 1;
        for x in 0..80 {
            let ch = if x < ed.line_lens[y] { ed.lines[y][x] } else { b' ' };
            let attr = if y < ed.num_lines { 0x07 } else { 0x08 }; // gray for unused
            unsafe {
                let off = (row * 80 + x) * 2;
                vga.add(off).write_volatile(ch);
                vga.add(off + 1).write_volatile(attr);
            }
        }
    }

    // Line numbers (row 23)
    let info = alloc::format!(" Lines: {} | Ctrl+S: Save | Ctrl+Q: Quit ", ed.num_lines);
    for x in 0..80 {
        let ch = info.as_bytes().get(x).copied().unwrap_or(b' ');
        unsafe {
            let off = (23 * 80 + x) * 2;
            vga.add(off).write_volatile(ch);
            vga.add(off + 1).write_volatile(0x70);
        }
    }

    // Clear last row
    for x in 0..80 {
        unsafe {
            let off = (24 * 80 + x) * 2;
            vga.add(off).write_volatile(b' ');
            vga.add(off + 1).write_volatile(0x00);
        }
    }
}

fn update_cursor(x: usize, y: usize) {
    let pos = ((y + 1) * 80 + x) as u16; // +1 for status bar
    unsafe {
        use x86_64::instructions::port::Port;
        let mut cmd = Port::<u8>::new(0x3D4);
        let mut data = Port::<u8>::new(0x3D5);
        cmd.write(0x0F);
        data.write((pos & 0xFF) as u8);
        cmd.write(0x0E);
        data.write((pos >> 8) as u8);
    }
}
