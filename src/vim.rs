/// Vim-like modal text editor for MerlionOS.
/// Provides Normal, Insert, Visual, and Command-line modes
/// with motions, operators, registers, undo/redo, search,
/// and syntax-aware display.

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use crate::keyboard::KeyEvent;
use crate::serial_println;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SCREEN_HEIGHT: usize = 25;
const STATUS_ROW: usize = 23;
const CMD_ROW: usize = 24;
const EDIT_ROWS: usize = 23;      // rows 0..22 for buffer content
const SCREEN_WIDTH: usize = 80;
const MAX_UNDO: usize = 100;
const NUM_REGISTERS: usize = 30;  // a-z + "0 + "" + "+ + spare

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static ACTIVE: AtomicBool = AtomicBool::new(false);
static EDITOR: Mutex<Option<Editor>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Mode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
    Command,
    Search,
    Replace,
}

// ---------------------------------------------------------------------------
// Buffer / Document
// ---------------------------------------------------------------------------

pub struct Buffer {
    lines: Vec<String>,
    filename: Option<String>,
    modified: bool,
    readonly: bool,
}

impl Buffer {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            filename: None,
            modified: false,
            readonly: false,
        }
    }

    pub fn from_string(content: &str) -> Self {
        let mut lines: Vec<String> = content.lines().map(|l| String::from(l)).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self { lines, filename: None, modified: false, readonly: false }
    }

    pub fn from_file(path: &str) -> Self {
        let mut buf = if let Ok(content) = crate::vfs::cat(path) {
            Self::from_string(&content)
        } else {
            Self::new()
        };
        buf.filename = Some(String::from(path));
        buf
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn get_line(&self, row: usize) -> Option<&str> {
        self.lines.get(row).map(|s| s.as_str())
    }

    fn line_len(&self, row: usize) -> usize {
        self.lines.get(row).map(|s| s.len()).unwrap_or(0)
    }

    pub fn insert_char(&mut self, row: usize, col: usize, ch: char) {
        if row < self.lines.len() {
            let col = col.min(self.lines[row].len());
            self.lines[row].insert(col, ch);
            self.modified = true;
        }
    }

    pub fn delete_char(&mut self, row: usize, col: usize) -> Option<char> {
        if row < self.lines.len() && col < self.lines[row].len() {
            self.modified = true;
            Some(self.lines[row].remove(col))
        } else {
            None
        }
    }

    pub fn insert_line(&mut self, row: usize, text: String) {
        let row = row.min(self.lines.len());
        self.lines.insert(row, text);
        self.modified = true;
    }

    pub fn delete_line(&mut self, row: usize) -> Option<String> {
        if row < self.lines.len() && self.lines.len() > 1 {
            self.modified = true;
            Some(self.lines.remove(row))
        } else if row < self.lines.len() {
            // Last line: clear it instead
            let old = core::mem::replace(&mut self.lines[row], String::new());
            self.modified = true;
            Some(old)
        } else {
            None
        }
    }

    pub fn replace_line(&mut self, row: usize, text: String) -> Option<String> {
        if row < self.lines.len() {
            let old = core::mem::replace(&mut self.lines[row], text);
            self.modified = true;
            Some(old)
        } else {
            None
        }
    }

    pub fn save(&mut self, path: Option<&str>) -> Result<(), &'static str> {
        let target = match path {
            Some(p) => {
                self.filename = Some(String::from(p));
                p
            }
            None => match &self.filename {
                Some(f) => f.as_str(),
                None => return Err("No filename"),
            },
        };
        // We need to copy the path out before the mutable borrow for building content
        let target_owned = String::from(target);
        let mut content = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            content.push_str(line);
            if i + 1 < self.lines.len() {
                content.push('\n');
            }
        }
        crate::vfs::write(&target_owned, &content)?;
        self.modified = false;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Cursor
// ---------------------------------------------------------------------------

pub struct Cursor {
    row: usize,
    col: usize,
    desired_col: usize,
}

impl Cursor {
    fn new() -> Self {
        Self { row: 0, col: 0, desired_col: 0 }
    }

    fn clamp(&mut self, buf: &Buffer) {
        if self.row >= buf.line_count() {
            self.row = buf.line_count().saturating_sub(1);
        }
        let max_col = buf.line_len(self.row);
        if self.col > max_col {
            self.col = max_col;
        }
    }

    fn clamp_normal(&mut self, buf: &Buffer) {
        if self.row >= buf.line_count() {
            self.row = buf.line_count().saturating_sub(1);
        }
        let max_col = buf.line_len(self.row).saturating_sub(1);
        if buf.line_len(self.row) == 0 {
            self.col = 0;
        } else if self.col > max_col {
            self.col = max_col;
        }
    }

    fn set_col(&mut self, col: usize) {
        self.col = col;
        self.desired_col = col;
    }

    fn move_to_desired(&mut self, buf: &Buffer) {
        let max = if buf.line_len(self.row) == 0 { 0 } else { buf.line_len(self.row).saturating_sub(1) };
        self.col = self.desired_col.min(max);
    }
}

// ---------------------------------------------------------------------------
// Viewport
// ---------------------------------------------------------------------------

pub struct Viewport {
    top_line: usize,
    height: usize,
    width: usize,
}

impl Viewport {
    fn new() -> Self {
        Self { top_line: 0, height: EDIT_ROWS, width: SCREEN_WIDTH }
    }

    fn ensure_visible(&mut self, cursor_row: usize) {
        if cursor_row < self.top_line {
            self.top_line = cursor_row;
        }
        if cursor_row >= self.top_line + self.height {
            self.top_line = cursor_row + 1 - self.height;
        }
    }
}

// ---------------------------------------------------------------------------
// Edit Actions (Undo/Redo)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum EditAction {
    InsertChar { row: usize, col: usize, ch: char },
    DeleteChar { row: usize, col: usize, ch: char },
    InsertLine { row: usize, text: String },
    DeleteLine { row: usize, text: String },
    ReplaceLine { row: usize, old: String, new: String },
    SplitLine { row: usize, col: usize },
    JoinLine { row: usize, col: usize },
    Batch(Vec<EditAction>),
}

// ---------------------------------------------------------------------------
// Register
// ---------------------------------------------------------------------------

struct Register {
    content: String,
    linewise: bool,
}

impl Register {
    fn new() -> Self {
        Self { content: String::new(), linewise: false }
    }
}

// ---------------------------------------------------------------------------
// Editor
// ---------------------------------------------------------------------------

pub struct Editor {
    buffer: Buffer,
    cursor: Cursor,
    viewport: Viewport,
    mode: Mode,
    registers: Vec<(char, Register)>,
    undo_stack: Vec<EditAction>,
    redo_stack: Vec<EditAction>,
    command_buffer: String,
    search_pattern: String,
    search_forward: bool,
    count: Option<usize>,
    pending_op: Option<char>,
    show_line_numbers: bool,
    message: String,
    running: bool,
    visual_start: (usize, usize),
}

impl Editor {
    pub fn new() -> Self {
        Self {
            buffer: Buffer::new(),
            cursor: Cursor::new(),
            viewport: Viewport::new(),
            mode: Mode::Normal,
            registers: Self::init_registers(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            command_buffer: String::new(),
            search_pattern: String::new(),
            search_forward: true,
            count: None,
            pending_op: None,
            show_line_numbers: true,
            message: String::from("Welcome to MerlionVim — type :help for info"),
            running: true,
            visual_start: (0, 0),
        }
    }

    pub fn open(path: &str) -> Self {
        let mut ed = Self::new();
        ed.buffer = Buffer::from_file(path);
        ed.message = format!("\"{}\" {} lines", path, ed.buffer.line_count());
        ed
    }

    fn init_registers() -> Vec<(char, Register)> {
        let mut regs = Vec::new();
        regs.push(('"', Register::new())); // default
        regs.push(('0', Register::new())); // yank
        regs.push(('+', Register::new())); // clipboard
        for c in b'a'..=b'z' {
            regs.push((c as char, Register::new()));
        }
        regs
    }

    fn get_register(&self, name: char) -> &Register {
        for (n, r) in &self.registers {
            if *n == name { return r; }
        }
        // fallback to default
        &self.registers[0].1
    }

    fn set_register(&mut self, name: char, content: String, linewise: bool) {
        for (n, r) in &mut self.registers {
            if *n == name {
                r.content = content;
                r.linewise = linewise;
                return;
            }
        }
    }

    fn effective_count(&mut self) -> usize {
        self.count.take().unwrap_or(1)
    }

    // ---------------------------------------------------------------------------
    // Undo / Redo
    // ---------------------------------------------------------------------------

    fn push_undo(&mut self, action: EditAction) {
        if self.undo_stack.len() >= MAX_UNDO {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(action);
        self.redo_stack.clear();
    }

    fn apply_action(&mut self, action: &EditAction) {
        match action {
            EditAction::InsertChar { row, col, ch } => {
                self.buffer.insert_char(*row, *col, *ch);
            }
            EditAction::DeleteChar { row, col, .. } => {
                self.buffer.delete_char(*row, *col);
            }
            EditAction::InsertLine { row, text } => {
                self.buffer.insert_line(*row, text.clone());
            }
            EditAction::DeleteLine { row, .. } => {
                self.buffer.delete_line(*row);
            }
            EditAction::ReplaceLine { row, new, .. } => {
                self.buffer.replace_line(*row, new.clone());
            }
            EditAction::SplitLine { row, col } => {
                if *row < self.buffer.line_count() {
                    let rest = if *col < self.buffer.lines[*row].len() {
                        String::from(&self.buffer.lines[*row][*col..])
                    } else {
                        String::new()
                    };
                    self.buffer.lines[*row].truncate(*col);
                    self.buffer.insert_line(*row + 1, rest);
                }
            }
            EditAction::JoinLine { row, .. } => {
                if *row + 1 < self.buffer.line_count() {
                    let next = self.buffer.lines[*row + 1].clone();
                    self.buffer.lines.remove(*row + 1);
                    self.buffer.lines[*row].push_str(&next);
                    self.buffer.modified = true;
                }
            }
            EditAction::Batch(actions) => {
                for a in actions {
                    self.apply_action(a);
                }
            }
        }
    }

    fn reverse_action(&mut self, action: &EditAction) {
        match action {
            EditAction::InsertChar { row, col, .. } => {
                self.buffer.delete_char(*row, *col);
            }
            EditAction::DeleteChar { row, col, ch } => {
                self.buffer.insert_char(*row, *col, *ch);
            }
            EditAction::InsertLine { row, .. } => {
                self.buffer.delete_line(*row);
            }
            EditAction::DeleteLine { row, text } => {
                self.buffer.insert_line(*row, text.clone());
            }
            EditAction::ReplaceLine { row, old, .. } => {
                self.buffer.replace_line(*row, old.clone());
            }
            EditAction::SplitLine { row, col: _ } => {
                // Reverse of split = join
                if *row + 1 < self.buffer.line_count() {
                    let next = self.buffer.lines[*row + 1].clone();
                    self.buffer.lines.remove(*row + 1);
                    self.buffer.lines[*row].push_str(&next);
                    self.buffer.modified = true;
                }
            }
            EditAction::JoinLine { row, col } => {
                // Reverse of join = split
                if *row < self.buffer.line_count() && *col <= self.buffer.lines[*row].len() {
                    let rest = if *col < self.buffer.lines[*row].len() {
                        String::from(&self.buffer.lines[*row][*col..])
                    } else {
                        String::new()
                    };
                    self.buffer.lines[*row].truncate(*col);
                    self.buffer.insert_line(*row + 1, rest);
                }
            }
            EditAction::Batch(actions) => {
                for a in actions.iter().rev() {
                    self.reverse_action(a);
                }
            }
        }
    }

    fn undo(&mut self) {
        if let Some(action) = self.undo_stack.pop() {
            self.reverse_action(&action);
            self.redo_stack.push(action);
            self.cursor.clamp(&self.buffer);
            self.message = String::from("Undo");
        } else {
            self.message = String::from("Already at oldest change");
        }
    }

    fn redo(&mut self) {
        if let Some(action) = self.redo_stack.pop() {
            self.apply_action(&action);
            self.undo_stack.push(action);
            self.cursor.clamp(&self.buffer);
            self.message = String::from("Redo");
        } else {
            self.message = String::from("Already at newest change");
        }
    }

    // ---------------------------------------------------------------------------
    // Word motions
    // ---------------------------------------------------------------------------

    fn word_end(&self) -> (usize, usize) {
        let mut row = self.cursor.row;
        let mut col = self.cursor.col;
        let line = self.buffer.get_line(row).unwrap_or("");
        let bytes = line.as_bytes();
        // Move past current word
        if col < bytes.len() {
            col += 1;
        }
        loop {
            let line = self.buffer.get_line(row).unwrap_or("");
            let bytes = line.as_bytes();
            // Skip whitespace
            while col < bytes.len() && (bytes[col] == b' ' || bytes[col] == b'\t') {
                col += 1;
            }
            if col < bytes.len() {
                // Find end of word
                let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
                let start_type = is_word(bytes[col]);
                while col + 1 < bytes.len() {
                    if is_word(bytes[col + 1]) == start_type {
                        col += 1;
                    } else {
                        break;
                    }
                }
                return (row, col);
            }
            // Move to next line
            if row + 1 < self.buffer.line_count() {
                row += 1;
                col = 0;
            } else {
                return (row, bytes.len().saturating_sub(1));
            }
        }
    }

    fn word_forward(&self) -> (usize, usize) {
        let mut row = self.cursor.row;
        let mut col = self.cursor.col;
        loop {
            let line = self.buffer.get_line(row).unwrap_or("");
            let bytes = line.as_bytes();
            if col < bytes.len() {
                let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
                let start_type = is_word(bytes[col]);
                // Skip rest of current word type
                while col < bytes.len() && is_word(bytes[col]) == start_type {
                    col += 1;
                }
                // Skip whitespace
                while col < bytes.len() && (bytes[col] == b' ' || bytes[col] == b'\t') {
                    col += 1;
                }
                if col < bytes.len() {
                    return (row, col);
                }
            }
            if row + 1 < self.buffer.line_count() {
                row += 1;
                col = 0;
                // Skip leading whitespace on new line
                let line = self.buffer.get_line(row).unwrap_or("");
                let bytes = line.as_bytes();
                while col < bytes.len() && (bytes[col] == b' ' || bytes[col] == b'\t') {
                    col += 1;
                }
                return (row, col);
            } else {
                return (row, self.buffer.line_len(row).saturating_sub(1));
            }
        }
    }

    fn word_backward(&self) -> (usize, usize) {
        let mut row = self.cursor.row;
        let mut col = self.cursor.col;
        if col == 0 {
            if row > 0 {
                row -= 1;
                col = self.buffer.line_len(row);
            } else {
                return (0, 0);
            }
        }
        let line = self.buffer.get_line(row).unwrap_or("");
        let bytes = line.as_bytes();
        if col > 0 { col -= 1; }
        // Skip whitespace backward
        while col > 0 && (bytes[col] == b' ' || bytes[col] == b'\t') {
            col -= 1;
        }
        if bytes.is_empty() { return (row, 0); }
        let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        let cur_type = is_word(bytes[col]);
        while col > 0 && is_word(bytes[col - 1]) == cur_type {
            col -= 1;
        }
        (row, col)
    }

    fn first_non_blank(&self, row: usize) -> usize {
        let line = self.buffer.get_line(row).unwrap_or("");
        for (i, b) in line.bytes().enumerate() {
            if b != b' ' && b != b'\t' { return i; }
        }
        0
    }

    fn paragraph_forward(&self) -> usize {
        let mut row = self.cursor.row + 1;
        // Skip non-empty lines
        while row < self.buffer.line_count() {
            if self.buffer.line_len(row) == 0 { break; }
            row += 1;
        }
        // Skip empty lines
        while row < self.buffer.line_count() && self.buffer.line_len(row) == 0 {
            row += 1;
        }
        row.min(self.buffer.line_count().saturating_sub(1))
    }

    fn paragraph_backward(&self) -> usize {
        if self.cursor.row == 0 { return 0; }
        let mut row = self.cursor.row - 1;
        // Skip empty lines
        while row > 0 && self.buffer.line_len(row) == 0 {
            row -= 1;
        }
        // Skip non-empty lines
        while row > 0 && self.buffer.line_len(row) > 0 {
            row -= 1;
        }
        row
    }

    // ---------------------------------------------------------------------------
    // Delete / Yank helpers
    // ---------------------------------------------------------------------------

    fn delete_range_chars(&mut self, from_row: usize, from_col: usize, to_row: usize, to_col: usize) -> String {
        let mut yanked = String::new();
        if from_row == to_row {
            let start = from_col.min(to_col);
            let end = from_col.max(to_col);
            let line = self.buffer.get_line(from_row).unwrap_or("");
            let end = end.min(line.len());
            let start = start.min(end);
            yanked.push_str(&line[start..end]);
            let mut batch = Vec::new();
            for i in (start..end).rev() {
                if let Some(ch) = self.buffer.delete_char(from_row, i) {
                    batch.push(EditAction::DeleteChar { row: from_row, col: i, ch });
                }
            }
            if !batch.is_empty() {
                batch.reverse();
                self.push_undo(EditAction::Batch(batch));
            }
        } else {
            // Multi-line delete: simplify by using line-based operations
            let line = self.buffer.get_line(from_row).unwrap_or("");
            yanked.push_str(&line[from_col..]);
            yanked.push('\n');
            for r in (from_row + 1)..to_row {
                yanked.push_str(self.buffer.get_line(r).unwrap_or(""));
                yanked.push('\n');
            }
            let to_line = self.buffer.get_line(to_row).unwrap_or("");
            let end_col = to_col.min(to_line.len());
            yanked.push_str(&to_line[..end_col]);

            // Rebuild: keep prefix of from_row and suffix of to_row
            let prefix = String::from(&self.buffer.lines[from_row][..from_col]);
            let to_line_ref = self.buffer.get_line(to_row).unwrap_or("");
            let suffix = if end_col < to_line_ref.len() {
                String::from(&to_line_ref[end_col..])
            } else {
                String::new()
            };
            let new_line = format!("{}{}", prefix, suffix);

            let mut batch = Vec::new();
            // Remove lines from_row+1..=to_row
            for r in (from_row + 1..=to_row).rev() {
                if let Some(text) = self.buffer.delete_line(r) {
                    batch.push(EditAction::DeleteLine { row: r, text });
                }
            }
            let old = self.buffer.replace_line(from_row, new_line.clone()).unwrap_or_default();
            batch.push(EditAction::ReplaceLine { row: from_row, old, new: new_line });
            self.push_undo(EditAction::Batch(batch));
        }
        yanked
    }

    fn delete_lines(&mut self, start: usize, count: usize) -> String {
        let mut yanked = String::new();
        let end = (start + count).min(self.buffer.line_count());
        let actual = end - start;
        let mut batch = Vec::new();
        for _ in 0..actual {
            if let Some(text) = self.buffer.delete_line(start) {
                if !yanked.is_empty() { yanked.push('\n'); }
                yanked.push_str(&text);
                batch.push(EditAction::DeleteLine { row: start, text });
            }
        }
        if !batch.is_empty() {
            self.push_undo(EditAction::Batch(batch));
        }
        yanked
    }

    fn yank_lines(&self, start: usize, count: usize) -> String {
        let mut yanked = String::new();
        let end = (start + count).min(self.buffer.line_count());
        for i in start..end {
            if !yanked.is_empty() { yanked.push('\n'); }
            yanked.push_str(self.buffer.get_line(i).unwrap_or(""));
        }
        yanked
    }

    fn indent_line(&mut self, row: usize) {
        if row < self.buffer.line_count() {
            let old = self.buffer.lines[row].clone();
            self.buffer.lines[row].insert_str(0, "    ");
            self.buffer.modified = true;
            self.push_undo(EditAction::ReplaceLine {
                row, old, new: self.buffer.lines[row].clone(),
            });
        }
    }

    fn dedent_line(&mut self, row: usize) {
        if row < self.buffer.line_count() {
            let old = self.buffer.lines[row].clone();
            let bytes = old.as_bytes();
            let mut remove = 0;
            while remove < 4 && remove < bytes.len() && bytes[remove] == b' ' {
                remove += 1;
            }
            if remove == 0 && !bytes.is_empty() && bytes[0] == b'\t' {
                remove = 1;
            }
            if remove > 0 {
                let new = String::from(&old[remove..]);
                self.buffer.lines[row] = new.clone();
                self.buffer.modified = true;
                self.push_undo(EditAction::ReplaceLine { row, old, new });
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Search
    // ---------------------------------------------------------------------------

    fn search_next(&mut self) {
        if self.search_pattern.is_empty() {
            self.message = String::from("No search pattern");
            return;
        }
        let total = self.buffer.line_count();
        let start_row = self.cursor.row;
        let start_col = self.cursor.col + 1;

        if self.search_forward {
            for offset in 0..total {
                let row = (start_row + offset) % total;
                let line = self.buffer.get_line(row).unwrap_or("");
                let search_from = if offset == 0 { start_col } else { 0 };
                if search_from < line.len() {
                    if let Some(pos) = line[search_from..].find(self.search_pattern.as_str()) {
                        self.cursor.row = row;
                        self.cursor.set_col(search_from + pos);
                        self.message = String::new();
                        return;
                    }
                }
            }
        } else {
            for offset in 0..total {
                let row = (start_row + total - offset) % total;
                let line = self.buffer.get_line(row).unwrap_or("");
                let search_end = if offset == 0 { self.cursor.col } else { line.len() };
                if let Some(pos) = line[..search_end].rfind(self.search_pattern.as_str()) {
                    self.cursor.row = row;
                    self.cursor.set_col(pos);
                    self.message = String::new();
                    return;
                }
            }
        }
        self.message = format!("Pattern not found: {}", self.search_pattern);
    }

    fn search_prev(&mut self) {
        self.search_forward = !self.search_forward;
        self.search_next();
        self.search_forward = !self.search_forward;
    }

    // ---------------------------------------------------------------------------
    // Command-line mode
    // ---------------------------------------------------------------------------

    fn execute_command(&mut self) -> Result<String, String> {
        let cmd = self.command_buffer.clone();
        let cmd = cmd.trim();

        if cmd.is_empty() {
            return Ok(String::new());
        }

        // :line_number — go to line
        if let Ok(n) = cmd.parse::<usize>() {
            let target = n.saturating_sub(1).min(self.buffer.line_count().saturating_sub(1));
            self.cursor.row = target;
            self.cursor.set_col(self.first_non_blank(target));
            return Ok(format!("Line {}", n));
        }

        // :%s/old/new/g — search and replace
        if cmd.starts_with("%s/") {
            return self.search_replace(&cmd[3..]);
        }

        match cmd {
            "w" => {
                match self.buffer.save(None) {
                    Ok(()) => {
                        let name = self.buffer.filename.clone().unwrap_or_else(|| String::from("[No Name]"));
                        Ok(format!("\"{}\" written, {} lines", name, self.buffer.line_count()))
                    }
                    Err(e) => Err(String::from(e)),
                }
            }
            "q" => {
                if self.buffer.modified {
                    Err(String::from("No write since last change (use :q! to override)"))
                } else {
                    self.running = false;
                    Ok(String::new())
                }
            }
            "q!" => {
                self.running = false;
                Ok(String::new())
            }
            "wq" | "x" => {
                if let Err(e) = self.buffer.save(None) {
                    return Err(String::from(e));
                }
                self.running = false;
                Ok(String::new())
            }
            "set number" => {
                self.show_line_numbers = true;
                Ok(String::from("Line numbers on"))
            }
            "set nonumber" | "set nonu" => {
                self.show_line_numbers = false;
                Ok(String::from("Line numbers off"))
            }
            "help" => {
                Ok(String::from("MerlionVim: i=insert a=append :w=save :q=quit /=search u=undo dd=del-line yy=yank p=paste"))
            }
            _ => {
                // :w filename
                if let Some(rest) = cmd.strip_prefix("w ") {
                    let path = rest.trim();
                    match self.buffer.save(Some(path)) {
                        Ok(()) => Ok(format!("\"{}\" written", path)),
                        Err(e) => Err(String::from(e)),
                    }
                }
                // :e filename
                else if let Some(rest) = cmd.strip_prefix("e ") {
                    let path = rest.trim();
                    self.buffer = Buffer::from_file(path);
                    self.cursor = Cursor::new();
                    self.viewport = Viewport::new();
                    self.undo_stack.clear();
                    self.redo_stack.clear();
                    Ok(format!("\"{}\" {} lines", path, self.buffer.line_count()))
                }
                else {
                    Err(format!("Not a command: {}", cmd))
                }
            }
        }
    }

    fn search_replace(&mut self, args: &str) -> Result<String, String> {
        // Parse old/new/flags from "old/new/g"
        let parts: Vec<&str> = args.splitn(3, '/').collect();
        if parts.len() < 2 {
            return Err(String::from("Invalid substitute syntax"));
        }
        let old = parts[0];
        let new = parts[1];
        let global = parts.get(2).map(|f| f.contains('g')).unwrap_or(false);

        if old.is_empty() {
            return Err(String::from("Empty search pattern"));
        }

        let mut total_count: usize = 0;
        for row in 0..self.buffer.line_count() {
            let line = self.buffer.lines[row].clone();
            let new_line = if global {
                line.replace(old, new)
            } else {
                // Replace first occurrence only
                if let Some(pos) = line.find(old) {
                    let mut s = String::from(&line[..pos]);
                    s.push_str(new);
                    s.push_str(&line[pos + old.len()..]);
                    s
                } else {
                    continue;
                }
            };
            if new_line != line {
                // Count replacements (approximate for global)
                let mut count = 0;
                let mut search_from = 0;
                let orig = &line;
                loop {
                    if let Some(pos) = orig[search_from..].find(old) {
                        count += 1;
                        search_from += pos + old.len();
                        if !global { break; }
                    } else {
                        break;
                    }
                }
                total_count += count;
                let old_line = core::mem::replace(&mut self.buffer.lines[row], new_line.clone());
                self.buffer.modified = true;
                self.push_undo(EditAction::ReplaceLine { row, old: old_line, new: new_line });
            }
        }
        Ok(format!("{} substitution(s)", total_count))
    }

    // ---------------------------------------------------------------------------
    // Main input handler
    // ---------------------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        match self.mode {
            Mode::Normal => self.handle_normal(key),
            Mode::Insert => self.handle_insert(key),
            Mode::Visual | Mode::VisualLine => self.handle_visual(key),
            Mode::Command => self.handle_command_input(key),
            Mode::Search => self.handle_search_input(key),
            Mode::Replace => self.handle_replace(key),
        }
        self.cursor.clamp(&self.buffer);
        self.viewport.ensure_visible(self.cursor.row);
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        match key {
            KeyEvent::Char(ch) if ch.is_ascii_digit() && (ch != '0' || self.count.is_some()) => {
                let digit = (ch as u8 - b'0') as usize;
                let cur = self.count.unwrap_or(0);
                self.count = Some(cur * 10 + digit);
            }

            // Motions
            KeyEvent::Char('h') | KeyEvent::ArrowLeft => {
                let n = self.effective_count();
                self.cursor.col = self.cursor.col.saturating_sub(n);
                self.cursor.desired_col = self.cursor.col;
            }
            KeyEvent::Char('l') | KeyEvent::ArrowRight => {
                let n = self.effective_count();
                let max = self.buffer.line_len(self.cursor.row).saturating_sub(1);
                self.cursor.col = (self.cursor.col + n).min(max);
                self.cursor.desired_col = self.cursor.col;
            }
            KeyEvent::Char('j') | KeyEvent::ArrowDown => {
                let n = self.effective_count();
                self.cursor.row = (self.cursor.row + n).min(self.buffer.line_count().saturating_sub(1));
                self.cursor.move_to_desired(&self.buffer);
            }
            KeyEvent::Char('k') | KeyEvent::ArrowUp => {
                let n = self.effective_count();
                self.cursor.row = self.cursor.row.saturating_sub(n);
                self.cursor.move_to_desired(&self.buffer);
            }
            KeyEvent::Char('w') => {
                if self.pending_op.is_some() {
                    let count = self.effective_count();
                    let op = self.pending_op.take().unwrap();
                    for _ in 0..count {
                        let (tr, tc) = self.word_forward();
                        match op {
                            'd' => {
                                let yanked = self.delete_range_chars(self.cursor.row, self.cursor.col, tr, tc);
                                self.set_register('"', yanked.clone(), false);
                                self.set_register('0', yanked, false);
                            }
                            'c' => {
                                let yanked = self.delete_range_chars(self.cursor.row, self.cursor.col, tr, tc);
                                self.set_register('"', yanked, false);
                                self.mode = Mode::Insert;
                            }
                            'y' => {
                                let line = self.buffer.get_line(self.cursor.row).unwrap_or("");
                                let end = tc.min(line.len());
                                let start = self.cursor.col.min(end);
                                let text = String::from(&line[start..end]);
                                self.set_register('"', text.clone(), false);
                                self.set_register('0', text, false);
                                self.message = String::from("Yanked");
                            }
                            _ => {}
                        }
                    }
                } else {
                    let n = self.effective_count();
                    for _ in 0..n {
                        let (row, col) = self.word_forward();
                        self.cursor.row = row;
                        self.cursor.set_col(col);
                    }
                }
            }
            KeyEvent::Char('b') => {
                let n = self.effective_count();
                for _ in 0..n {
                    let (row, col) = self.word_backward();
                    self.cursor.row = row;
                    self.cursor.set_col(col);
                }
            }
            KeyEvent::Char('e') => {
                let n = self.effective_count();
                for _ in 0..n {
                    let (row, col) = self.word_end();
                    self.cursor.row = row;
                    self.cursor.set_col(col);
                }
            }
            KeyEvent::Char('0') => {
                self.cursor.set_col(0);
            }
            KeyEvent::Char('$') | KeyEvent::End => {
                let len = self.buffer.line_len(self.cursor.row);
                self.cursor.set_col(len.saturating_sub(1).max(0));
            }
            KeyEvent::Char('^') | KeyEvent::Home => {
                let col = self.first_non_blank(self.cursor.row);
                self.cursor.set_col(col);
            }
            KeyEvent::Char('G') => {
                let count = self.count.take();
                match count {
                    Some(n) => {
                        let target = n.saturating_sub(1).min(self.buffer.line_count().saturating_sub(1));
                        self.cursor.row = target;
                    }
                    None => {
                        self.cursor.row = self.buffer.line_count().saturating_sub(1);
                    }
                }
                self.cursor.set_col(self.first_non_blank(self.cursor.row));
            }
            KeyEvent::Char('g') => {
                // gg — go to top (simplified: single g also goes to top with pending)
                if self.pending_op == Some('g') {
                    self.pending_op = None;
                    let count = self.count.take();
                    let target = count.map(|n| n.saturating_sub(1)).unwrap_or(0);
                    self.cursor.row = target.min(self.buffer.line_count().saturating_sub(1));
                    self.cursor.set_col(self.first_non_blank(self.cursor.row));
                } else if self.pending_op.is_none() {
                    self.pending_op = Some('g');
                }
            }
            KeyEvent::Char('{') => {
                self.count.take();
                self.cursor.row = self.paragraph_backward();
                self.cursor.set_col(0);
            }
            KeyEvent::Char('}') => {
                self.count.take();
                self.cursor.row = self.paragraph_forward();
                self.cursor.set_col(0);
            }

            // Operators
            KeyEvent::Char('d') => {
                if self.pending_op == Some('d') {
                    // dd — delete line(s)
                    self.pending_op = None;
                    let n = self.effective_count();
                    let yanked = self.delete_lines(self.cursor.row, n);
                    self.set_register('"', yanked.clone(), true);
                    self.set_register('0', yanked, true);
                    self.cursor.clamp_normal(&self.buffer);
                    self.message = format!("{} lines deleted", n);
                } else {
                    self.pending_op = Some('d');
                }
            }
            KeyEvent::Char('y') => {
                if self.pending_op == Some('y') {
                    // yy — yank line(s)
                    self.pending_op = None;
                    let n = self.effective_count();
                    let yanked = self.yank_lines(self.cursor.row, n);
                    self.set_register('"', yanked.clone(), true);
                    self.set_register('0', yanked, true);
                    self.message = format!("{} lines yanked", n);
                } else {
                    self.pending_op = Some('y');
                }
            }
            KeyEvent::Char('c') => {
                if self.pending_op == Some('c') {
                    // cc — change line
                    self.pending_op = None;
                    let n = self.effective_count();
                    let yanked = self.delete_lines(self.cursor.row, n);
                    self.set_register('"', yanked, true);
                    // Insert an empty line to type on
                    self.buffer.insert_line(self.cursor.row, String::new());
                    self.push_undo(EditAction::InsertLine { row: self.cursor.row, text: String::new() });
                    self.cursor.set_col(0);
                    self.mode = Mode::Insert;
                } else {
                    self.pending_op = Some('c');
                }
            }

            // D — delete to end of line
            KeyEvent::Char('D') => {
                let row = self.cursor.row;
                let col = self.cursor.col;
                let line = self.buffer.get_line(row).unwrap_or("");
                if col < line.len() {
                    let yanked = String::from(&line[col..]);
                    let old = line.to_string();
                    let new = String::from(&old[..col]);
                    self.buffer.replace_line(row, new.clone());
                    self.push_undo(EditAction::ReplaceLine { row, old, new });
                    self.set_register('"', yanked, false);
                    self.cursor.clamp_normal(&self.buffer);
                }
            }
            // C — change to end of line
            KeyEvent::Char('C') => {
                let row = self.cursor.row;
                let col = self.cursor.col;
                let line = self.buffer.get_line(row).unwrap_or("");
                if col < line.len() {
                    let yanked = String::from(&line[col..]);
                    let old = line.to_string();
                    let new = String::from(&old[..col]);
                    self.buffer.replace_line(row, new.clone());
                    self.push_undo(EditAction::ReplaceLine { row, old, new });
                    self.set_register('"', yanked, false);
                }
                self.mode = Mode::Insert;
            }

            // x — delete char under cursor
            KeyEvent::Char('x') | KeyEvent::Delete => {
                let n = self.effective_count();
                let mut batch = Vec::new();
                for _ in 0..n {
                    if let Some(ch) = self.buffer.delete_char(self.cursor.row, self.cursor.col) {
                        batch.push(EditAction::DeleteChar { row: self.cursor.row, col: self.cursor.col, ch });
                    }
                }
                if !batch.is_empty() {
                    self.push_undo(EditAction::Batch(batch));
                }
                self.cursor.clamp_normal(&self.buffer);
            }

            // r — replace char
            KeyEvent::Char('r') => {
                self.mode = Mode::Replace;
            }

            // J — join lines
            KeyEvent::Char('J') => {
                let n = self.effective_count();
                for _ in 0..n {
                    if self.cursor.row + 1 < self.buffer.line_count() {
                        let col = self.buffer.line_len(self.cursor.row);
                        // Add a space when joining
                        self.buffer.lines[self.cursor.row].push(' ');
                        let next = self.buffer.lines[self.cursor.row + 1].clone();
                        // Trim leading whitespace from joined line
                        let trimmed = next.trim_start();
                        self.buffer.lines[self.cursor.row].push_str(trimmed);
                        self.buffer.lines.remove(self.cursor.row + 1);
                        self.buffer.modified = true;
                        self.push_undo(EditAction::JoinLine { row: self.cursor.row, col });
                    }
                }
            }

            // Paste
            KeyEvent::Char('p') => {
                let reg = self.get_register('"');
                let content = reg.content.clone();
                let linewise = reg.linewise;
                if linewise {
                    let lines: Vec<&str> = content.split('\n').collect();
                    let insert_at = self.cursor.row + 1;
                    let mut batch = Vec::new();
                    for (i, line_text) in lines.iter().enumerate() {
                        let text = String::from(*line_text);
                        self.buffer.insert_line(insert_at + i, text.clone());
                        batch.push(EditAction::InsertLine { row: insert_at + i, text });
                    }
                    self.cursor.row = insert_at;
                    self.cursor.set_col(self.first_non_blank(insert_at));
                    if !batch.is_empty() {
                        self.push_undo(EditAction::Batch(batch));
                    }
                } else {
                    // Insert after cursor
                    let col = self.cursor.col + 1;
                    let mut batch = Vec::new();
                    for (i, ch) in content.chars().enumerate() {
                        self.buffer.insert_char(self.cursor.row, col + i, ch);
                        batch.push(EditAction::InsertChar { row: self.cursor.row, col: col + i, ch });
                    }
                    self.cursor.set_col(col + content.len().saturating_sub(1));
                    if !batch.is_empty() {
                        self.push_undo(EditAction::Batch(batch));
                    }
                }
            }
            KeyEvent::Char('P') => {
                let reg = self.get_register('"');
                let content = reg.content.clone();
                let linewise = reg.linewise;
                if linewise {
                    let lines: Vec<&str> = content.split('\n').collect();
                    let insert_at = self.cursor.row;
                    let mut batch = Vec::new();
                    for (i, line_text) in lines.iter().enumerate() {
                        let text = String::from(*line_text);
                        self.buffer.insert_line(insert_at + i, text.clone());
                        batch.push(EditAction::InsertLine { row: insert_at + i, text });
                    }
                    self.cursor.set_col(self.first_non_blank(insert_at));
                    if !batch.is_empty() {
                        self.push_undo(EditAction::Batch(batch));
                    }
                } else {
                    let col = self.cursor.col;
                    let mut batch = Vec::new();
                    for (i, ch) in content.chars().enumerate() {
                        self.buffer.insert_char(self.cursor.row, col + i, ch);
                        batch.push(EditAction::InsertChar { row: self.cursor.row, col: col + i, ch });
                    }
                    if !batch.is_empty() {
                        self.push_undo(EditAction::Batch(batch));
                    }
                }
            }

            // Undo / Redo
            KeyEvent::Char('u') => {
                self.count.take();
                self.undo();
            }
            KeyEvent::Char('\x12') => {
                // Ctrl+R = redo
                self.count.take();
                self.redo();
            }

            // Insert mode entry points
            KeyEvent::Char('i') => {
                self.count.take();
                self.pending_op = None;
                self.mode = Mode::Insert;
            }
            KeyEvent::Char('a') => {
                self.count.take();
                self.pending_op = None;
                let len = self.buffer.line_len(self.cursor.row);
                if len > 0 {
                    self.cursor.col = (self.cursor.col + 1).min(len);
                }
                self.mode = Mode::Insert;
            }
            KeyEvent::Char('I') => {
                self.count.take();
                self.pending_op = None;
                self.cursor.set_col(self.first_non_blank(self.cursor.row));
                self.mode = Mode::Insert;
            }
            KeyEvent::Char('A') => {
                self.count.take();
                self.pending_op = None;
                self.cursor.set_col(self.buffer.line_len(self.cursor.row));
                self.mode = Mode::Insert;
            }
            KeyEvent::Char('o') => {
                self.count.take();
                self.pending_op = None;
                let row = self.cursor.row + 1;
                self.buffer.insert_line(row, String::new());
                self.push_undo(EditAction::InsertLine { row, text: String::new() });
                self.cursor.row = row;
                self.cursor.set_col(0);
                self.mode = Mode::Insert;
            }
            KeyEvent::Char('O') => {
                self.count.take();
                self.pending_op = None;
                let row = self.cursor.row;
                self.buffer.insert_line(row, String::new());
                self.push_undo(EditAction::InsertLine { row, text: String::new() });
                self.cursor.set_col(0);
                self.mode = Mode::Insert;
            }

            // Visual mode
            KeyEvent::Char('v') => {
                self.count.take();
                self.pending_op = None;
                self.visual_start = (self.cursor.row, self.cursor.col);
                self.mode = Mode::Visual;
            }
            KeyEvent::Char('V') => {
                self.count.take();
                self.pending_op = None;
                self.visual_start = (self.cursor.row, 0);
                self.mode = Mode::VisualLine;
            }

            // Command and search
            KeyEvent::Char(':') => {
                self.count.take();
                self.pending_op = None;
                self.command_buffer.clear();
                self.mode = Mode::Command;
            }
            KeyEvent::Char('/') => {
                self.count.take();
                self.pending_op = None;
                self.command_buffer.clear();
                self.search_forward = true;
                self.mode = Mode::Search;
            }
            KeyEvent::Char('?') => {
                self.count.take();
                self.pending_op = None;
                self.command_buffer.clear();
                self.search_forward = false;
                self.mode = Mode::Search;
            }
            KeyEvent::Char('n') => {
                self.count.take();
                self.search_next();
            }
            KeyEvent::Char('N') => {
                self.count.take();
                self.search_prev();
            }

            // Escape clears pending
            KeyEvent::Escape | KeyEvent::Char('\x1B') => {
                self.count = None;
                self.pending_op = None;
                self.message.clear();
            }

            _ => {
                self.count = None;
                self.pending_op = None;
            }
        }
    }

    fn handle_insert(&mut self, key: KeyEvent) {
        match key {
            KeyEvent::Escape | KeyEvent::Char('\x1B') => {
                // Escape — back to normal
                self.mode = Mode::Normal;
                if self.cursor.col > 0 {
                    self.cursor.col -= 1;
                }
                self.cursor.desired_col = self.cursor.col;
            }
            KeyEvent::Char('\n') => {
                // Enter — split line
                let row = self.cursor.row;
                let col = self.cursor.col;
                let rest = if col < self.buffer.lines[row].len() {
                    String::from(&self.buffer.lines[row][col..])
                } else {
                    String::new()
                };
                self.buffer.lines[row].truncate(col);
                self.buffer.insert_line(row + 1, rest);
                self.buffer.modified = true;
                self.push_undo(EditAction::SplitLine { row, col });
                self.cursor.row += 1;
                self.cursor.set_col(0);
            }
            KeyEvent::Char('\x08') => {
                // Backspace
                if self.cursor.col > 0 {
                    let col = self.cursor.col - 1;
                    if let Some(ch) = self.buffer.delete_char(self.cursor.row, col) {
                        self.push_undo(EditAction::DeleteChar { row: self.cursor.row, col, ch });
                        self.cursor.col -= 1;
                        self.cursor.desired_col = self.cursor.col;
                    }
                } else if self.cursor.row > 0 {
                    // Join with previous line
                    let prev_row = self.cursor.row - 1;
                    let col = self.buffer.line_len(prev_row);
                    let current = self.buffer.lines[self.cursor.row].clone();
                    self.buffer.lines[prev_row].push_str(&current);
                    self.buffer.lines.remove(self.cursor.row);
                    self.buffer.modified = true;
                    self.push_undo(EditAction::JoinLine { row: prev_row, col });
                    self.cursor.row = prev_row;
                    self.cursor.set_col(col);
                }
            }
            KeyEvent::Delete => {
                if self.cursor.col < self.buffer.line_len(self.cursor.row) {
                    if let Some(ch) = self.buffer.delete_char(self.cursor.row, self.cursor.col) {
                        self.push_undo(EditAction::DeleteChar { row: self.cursor.row, col: self.cursor.col, ch });
                    }
                }
            }
            KeyEvent::Char('\t') => {
                // Tab = 4 spaces
                for i in 0..4 {
                    let col = self.cursor.col + i;
                    self.buffer.insert_char(self.cursor.row, col, ' ');
                    self.push_undo(EditAction::InsertChar { row: self.cursor.row, col, ch: ' ' });
                }
                self.cursor.col += 4;
                self.cursor.desired_col = self.cursor.col;
            }
            KeyEvent::Char(ch) if !ch.is_ascii_control() => {
                let row = self.cursor.row;
                let col = self.cursor.col;
                self.buffer.insert_char(row, col, ch);
                self.push_undo(EditAction::InsertChar { row, col, ch });
                self.cursor.col += 1;
                self.cursor.desired_col = self.cursor.col;
            }
            KeyEvent::ArrowUp => {
                if self.cursor.row > 0 {
                    self.cursor.row -= 1;
                    self.cursor.move_to_desired(&self.buffer);
                }
            }
            KeyEvent::ArrowDown => {
                if self.cursor.row + 1 < self.buffer.line_count() {
                    self.cursor.row += 1;
                    self.cursor.move_to_desired(&self.buffer);
                }
            }
            KeyEvent::ArrowLeft => {
                if self.cursor.col > 0 {
                    self.cursor.col -= 1;
                    self.cursor.desired_col = self.cursor.col;
                }
            }
            KeyEvent::ArrowRight => {
                if self.cursor.col < self.buffer.line_len(self.cursor.row) {
                    self.cursor.col += 1;
                    self.cursor.desired_col = self.cursor.col;
                }
            }
            KeyEvent::Home => self.cursor.set_col(0),
            KeyEvent::End => self.cursor.set_col(self.buffer.line_len(self.cursor.row)),
            _ => {}
        }
    }

    fn handle_visual(&mut self, key: KeyEvent) {
        match key {
            KeyEvent::Escape | KeyEvent::Char('\x1B') => {
                self.mode = Mode::Normal;
                self.message.clear();
                return;
            }
            // Motions (same as normal)
            KeyEvent::Char('h') | KeyEvent::ArrowLeft => {
                if self.cursor.col > 0 { self.cursor.col -= 1; }
                self.cursor.desired_col = self.cursor.col;
            }
            KeyEvent::Char('l') | KeyEvent::ArrowRight => {
                let max = self.buffer.line_len(self.cursor.row);
                if self.cursor.col + 1 < max { self.cursor.col += 1; }
                self.cursor.desired_col = self.cursor.col;
            }
            KeyEvent::Char('j') | KeyEvent::ArrowDown => {
                if self.cursor.row + 1 < self.buffer.line_count() {
                    self.cursor.row += 1;
                    self.cursor.move_to_desired(&self.buffer);
                }
            }
            KeyEvent::Char('k') | KeyEvent::ArrowUp => {
                if self.cursor.row > 0 {
                    self.cursor.row -= 1;
                    self.cursor.move_to_desired(&self.buffer);
                }
            }
            KeyEvent::Char('G') => {
                self.cursor.row = self.buffer.line_count().saturating_sub(1);
                self.cursor.move_to_desired(&self.buffer);
            }
            KeyEvent::Char('0') => self.cursor.set_col(0),
            KeyEvent::Char('$') => {
                self.cursor.set_col(self.buffer.line_len(self.cursor.row).saturating_sub(1));
            }

            // Operations on selection
            KeyEvent::Char('d') | KeyEvent::Char('x') => {
                let (start_row, start_col, end_row, end_col) = self.visual_range();
                if self.mode == Mode::VisualLine {
                    let count = end_row - start_row + 1;
                    let yanked = self.delete_lines(start_row, count);
                    self.set_register('"', yanked, true);
                    self.cursor.row = start_row.min(self.buffer.line_count().saturating_sub(1));
                } else {
                    let yanked = self.delete_range_chars(start_row, start_col, end_row, end_col + 1);
                    self.set_register('"', yanked, false);
                    self.cursor.row = start_row;
                    self.cursor.set_col(start_col);
                }
                self.cursor.clamp_normal(&self.buffer);
                self.mode = Mode::Normal;
            }
            KeyEvent::Char('y') => {
                let (start_row, start_col, end_row, end_col) = self.visual_range();
                if self.mode == Mode::VisualLine {
                    let count = end_row - start_row + 1;
                    let yanked = self.yank_lines(start_row, count);
                    self.set_register('"', yanked.clone(), true);
                    self.set_register('0', yanked, true);
                } else {
                    let mut yanked = String::new();
                    if start_row == end_row {
                        let line = self.buffer.get_line(start_row).unwrap_or("");
                        let s = start_col.min(line.len());
                        let e = (end_col + 1).min(line.len());
                        yanked.push_str(&line[s..e]);
                    } else {
                        let line = self.buffer.get_line(start_row).unwrap_or("");
                        yanked.push_str(&line[start_col.min(line.len())..]);
                        for r in start_row + 1..end_row {
                            yanked.push('\n');
                            yanked.push_str(self.buffer.get_line(r).unwrap_or(""));
                        }
                        yanked.push('\n');
                        let line = self.buffer.get_line(end_row).unwrap_or("");
                        yanked.push_str(&line[..(end_col + 1).min(line.len())]);
                    }
                    self.set_register('"', yanked.clone(), false);
                    self.set_register('0', yanked, false);
                }
                self.message = String::from("Yanked");
                self.mode = Mode::Normal;
            }
            KeyEvent::Char('c') => {
                let (start_row, start_col, end_row, end_col) = self.visual_range();
                if self.mode == Mode::VisualLine {
                    let count = end_row - start_row + 1;
                    let yanked = self.delete_lines(start_row, count);
                    self.set_register('"', yanked, true);
                    self.buffer.insert_line(start_row, String::new());
                    self.push_undo(EditAction::InsertLine { row: start_row, text: String::new() });
                    self.cursor.row = start_row;
                    self.cursor.set_col(0);
                } else {
                    let yanked = self.delete_range_chars(start_row, start_col, end_row, end_col + 1);
                    self.set_register('"', yanked, false);
                    self.cursor.row = start_row;
                    self.cursor.set_col(start_col);
                }
                self.mode = Mode::Insert;
            }
            KeyEvent::Char('>') => {
                let (start_row, _, end_row, _) = self.visual_range();
                for r in start_row..=end_row {
                    self.indent_line(r);
                }
                self.mode = Mode::Normal;
                self.message = format!("{} lines indented", end_row - start_row + 1);
            }
            KeyEvent::Char('<') => {
                let (start_row, _, end_row, _) = self.visual_range();
                for r in start_row..=end_row {
                    self.dedent_line(r);
                }
                self.mode = Mode::Normal;
                self.message = format!("{} lines dedented", end_row - start_row + 1);
            }
            _ => {}
        }
    }

    fn visual_range(&self) -> (usize, usize, usize, usize) {
        let (sr, sc) = self.visual_start;
        let (er, ec) = (self.cursor.row, self.cursor.col);
        if sr < er || (sr == er && sc <= ec) {
            (sr, sc, er, ec)
        } else {
            (er, ec, sr, sc)
        }
    }

    fn handle_command_input(&mut self, key: KeyEvent) {
        match key {
            KeyEvent::Escape | KeyEvent::Char('\x1B') => {
                self.mode = Mode::Normal;
                self.command_buffer.clear();
            }
            KeyEvent::Char('\n') => {
                self.mode = Mode::Normal;
                match self.execute_command() {
                    Ok(msg) => self.message = msg,
                    Err(msg) => self.message = msg,
                }
                self.command_buffer.clear();
            }
            KeyEvent::Char('\x08') => {
                if self.command_buffer.is_empty() {
                    self.mode = Mode::Normal;
                } else {
                    self.command_buffer.pop();
                }
            }
            KeyEvent::Char(ch) if !ch.is_ascii_control() => {
                self.command_buffer.push(ch);
            }
            _ => {}
        }
    }

    fn handle_search_input(&mut self, key: KeyEvent) {
        match key {
            KeyEvent::Escape | KeyEvent::Char('\x1B') => {
                self.mode = Mode::Normal;
                self.command_buffer.clear();
            }
            KeyEvent::Char('\n') => {
                self.search_pattern = self.command_buffer.clone();
                self.command_buffer.clear();
                self.mode = Mode::Normal;
                // Move cursor back one so search_next finds from current position
                if self.cursor.col > 0 { self.cursor.col -= 1; }
                self.search_next();
            }
            KeyEvent::Char('\x08') => {
                if self.command_buffer.is_empty() {
                    self.mode = Mode::Normal;
                } else {
                    self.command_buffer.pop();
                }
            }
            KeyEvent::Char(ch) if !ch.is_ascii_control() => {
                self.command_buffer.push(ch);
            }
            _ => {}
        }
    }

    fn handle_replace(&mut self, key: KeyEvent) {
        match key {
            KeyEvent::Escape | KeyEvent::Char('\x1B') => {
                self.mode = Mode::Normal;
            }
            KeyEvent::Char(ch) if !ch.is_ascii_control() => {
                let row = self.cursor.row;
                let col = self.cursor.col;
                if col < self.buffer.line_len(row) {
                    let old_ch = self.buffer.lines[row].as_bytes()[col] as char;
                    let mut chars: Vec<char> = self.buffer.lines[row].chars().collect();
                    chars[col] = ch;
                    let new_line: String = chars.into_iter().collect();
                    let old_line = self.buffer.replace_line(row, new_line.clone()).unwrap_or_default();
                    self.push_undo(EditAction::ReplaceLine { row, old: old_line, new: new_line });
                    let _ = old_ch; // suppress unused warning
                }
                self.mode = Mode::Normal;
            }
            _ => {
                self.mode = Mode::Normal;
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Rendering
    // ---------------------------------------------------------------------------

    /// Render the editor screen to a String (for VGA text output).
    pub fn render(&self) -> String {
        let mut out = String::new();
        let num_width = if self.show_line_numbers {
            let max_line = self.viewport.top_line + self.viewport.height;
            if max_line >= 1000 { 5 } else if max_line >= 100 { 4 } else { 3 }
        } else {
            0
        };
        let text_width = SCREEN_WIDTH.saturating_sub(num_width);

        // Buffer content lines
        for screen_row in 0..EDIT_ROWS {
            let buf_row = self.viewport.top_line + screen_row;
            if buf_row < self.buffer.line_count() {
                if self.show_line_numbers {
                    let num_str = format!("{:>width$} ", buf_row + 1, width = num_width - 1);
                    out.push_str(&num_str);
                }
                let line = self.buffer.get_line(buf_row).unwrap_or("");
                let display_len = line.len().min(text_width);
                out.push_str(&line[..display_len]);
                // Pad rest
                for _ in display_len..text_width {
                    out.push(' ');
                }
            } else {
                // Tilde lines
                if self.show_line_numbers {
                    for _ in 0..num_width { out.push(' '); }
                }
                out.push('~');
                for _ in 1..text_width {
                    out.push(' ');
                }
            }
        }

        // Status line (row 23)
        let filename = self.buffer.filename.as_deref().unwrap_or("[No Name]");
        let modified_flag = if self.buffer.modified { " [+]" } else { "" };
        let mode_str = match self.mode {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "V-LINE",
            Mode::Command => "COMMAND",
            Mode::Search => "SEARCH",
            Mode::Replace => "REPLACE",
        };
        let total = self.buffer.line_count();
        let pct = if total == 0 { 0usize }
                  else { (self.cursor.row * 100) / total };
        let status = format!(
            " {}{} -- {} -- {}:{} | {} lines | {}%",
            filename, modified_flag, mode_str,
            self.cursor.row + 1, self.cursor.col + 1,
            total, pct
        );
        let status_len = status.len().min(SCREEN_WIDTH);
        out.push_str(&status[..status_len]);
        for _ in status_len..SCREEN_WIDTH {
            out.push(' ');
        }

        // Command line (row 24)
        match self.mode {
            Mode::Command => {
                let cmd_display = format!(":{}", self.command_buffer);
                let len = cmd_display.len().min(SCREEN_WIDTH);
                out.push_str(&cmd_display[..len]);
                for _ in len..SCREEN_WIDTH { out.push(' '); }
            }
            Mode::Search => {
                let prefix = if self.search_forward { '/' } else { '?' };
                let cmd_display = format!("{}{}", prefix, self.command_buffer);
                let len = cmd_display.len().min(SCREEN_WIDTH);
                out.push_str(&cmd_display[..len]);
                for _ in len..SCREEN_WIDTH { out.push(' '); }
            }
            _ => {
                let msg_len = self.message.len().min(SCREEN_WIDTH);
                out.push_str(&self.message[..msg_len]);
                for _ in msg_len..SCREEN_WIDTH { out.push(' '); }
            }
        }

        out
    }

    pub fn vim_info(&self) -> String {
        let filename = self.buffer.filename.as_deref().unwrap_or("[No Name]");
        format!(
            "MerlionVim: file={} lines={} mode={:?} cursor={}:{} modified={} undo={} redo={}",
            filename, self.buffer.line_count(), self.mode,
            self.cursor.row + 1, self.cursor.col + 1,
            self.buffer.modified, self.undo_stack.len(), self.redo_stack.len()
        )
    }
}

// ---------------------------------------------------------------------------
// VGA rendering (direct to framebuffer like editor.rs)
// ---------------------------------------------------------------------------

fn redraw_vga() {
    let guard = EDITOR.lock();
    let editor = match guard.as_ref() {
        Some(e) => e,
        None => return,
    };

    let vga = 0xB8000 as *mut u8;
    let rendered = editor.render();
    let bytes = rendered.as_bytes();

    let num_width: usize = if editor.show_line_numbers {
        let max_line = editor.viewport.top_line + editor.viewport.height;
        if max_line >= 1000 { 5 } else if max_line >= 100 { 4 } else { 3 }
    } else {
        0
    };

    let mut byte_idx = 0;
    for row in 0..SCREEN_HEIGHT {
        for col in 0..SCREEN_WIDTH {
            let ch = if byte_idx < bytes.len() { bytes[byte_idx] } else { b' ' };
            byte_idx += 1;

            let attr = if row == STATUS_ROW {
                // Status line: inverted
                0x70
            } else if row == CMD_ROW {
                // Command line: normal
                0x07
            } else if row < EDIT_ROWS {
                let buf_row = editor.viewport.top_line + row;
                if buf_row >= editor.buffer.line_count() {
                    // Tilde lines
                    if col == num_width { 0x09 } else { 0x08 }
                } else if editor.show_line_numbers && col < num_width {
                    // Line number color
                    0x08
                } else {
                    // Check visual highlight
                    let in_visual = if editor.mode == Mode::Visual || editor.mode == Mode::VisualLine {
                        let (sr, sc, er, ec) = editor.visual_range();
                        let text_col = if editor.show_line_numbers { col.saturating_sub(num_width) } else { col };
                        if editor.mode == Mode::VisualLine {
                            buf_row >= sr && buf_row <= er
                        } else {
                            if buf_row > sr && buf_row < er {
                                true
                            } else if buf_row == sr && buf_row == er {
                                text_col >= sc && text_col <= ec
                            } else if buf_row == sr {
                                text_col >= sc
                            } else if buf_row == er {
                                text_col <= ec
                            } else {
                                false
                            }
                        }
                    } else {
                        false
                    };
                    if in_visual { 0x70 } else { 0x07 }
                }
            } else {
                0x07
            };

            unsafe {
                let offset = (row * SCREEN_WIDTH + col) * 2;
                vga.add(offset).write_volatile(ch);
                vga.add(offset + 1).write_volatile(attr);
            }
        }
    }

    // Update hardware cursor
    let cursor_screen_row = editor.cursor.row.saturating_sub(editor.viewport.top_line);
    let cursor_screen_col = if editor.show_line_numbers {
        num_width + editor.cursor.col
    } else {
        editor.cursor.col
    };

    // In command/search mode, cursor is on the command line
    let (cur_row, cur_col) = match editor.mode {
        Mode::Command => (CMD_ROW, 1 + editor.command_buffer.len()),
        Mode::Search => (CMD_ROW, 1 + editor.command_buffer.len()),
        _ => (cursor_screen_row, cursor_screen_col),
    };

    let pos = (cur_row * SCREEN_WIDTH + cur_col) as u16;
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

// ---------------------------------------------------------------------------
// Public API / Global entry points
// ---------------------------------------------------------------------------

/// Initialize the vim module.
pub fn init() {
    serial_println!("[vim] module initialized");
}

/// Start the vim editor on a file.
/// Called from shell as `vim <filename>`.
pub fn start(filename: Option<&str>) {
    let editor = match filename {
        Some(path) => Editor::open(path),
        None => Editor::new(),
    };
    {
        let mut guard = EDITOR.lock();
        *guard = Some(editor);
    }
    ACTIVE.store(true, Ordering::SeqCst);
    redraw_vga();
}

/// Handle a key event when vim is active.
pub fn handle_input(event: KeyEvent) {
    if !ACTIVE.load(Ordering::SeqCst) { return; }

    let still_running;
    {
        let mut guard = EDITOR.lock();
        if let Some(ref mut editor) = *guard {
            editor.handle_key(event);
            still_running = editor.running;
        } else {
            return;
        }
    }

    if !still_running {
        ACTIVE.store(false, Ordering::SeqCst);
        // Clean up
        let mut guard = EDITOR.lock();
        *guard = None;
        return;
    }

    redraw_vga();
}

/// Check if vim is currently active.
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}
