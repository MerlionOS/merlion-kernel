/// Graphical terminal widget — renders a text console inside a pixel framebuffer.
///
/// This is a self-contained rendering widget: it manages a character buffer,
/// cursor state, and colors, then blits the entire terminal to a caller-supplied
/// pixel buffer using 8x16 bitmap glyphs from `crate::font`.
///
/// The window manager (`wm.rs`) creates a window and calls
/// [`GraphicalTerminal::render_to_buffer`] to paint shell output into it.

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use crate::font;

/// Default foreground: light grey (0xAA_AA_AA).
const DEFAULT_FG: u32 = 0x00AA_AAAA;
/// Default background: black.
const DEFAULT_BG: u32 = 0x0000_0000;

/// A graphical terminal that maintains a character grid and renders it
/// to a 32-bit ARGB pixel buffer on demand.
pub struct GraphicalTerminal {
    /// Number of character columns.
    cols: usize,
    /// Number of character rows.
    rows: usize,
    /// Current cursor column (0-based).
    cursor_x: usize,
    /// Current cursor row (0-based).
    cursor_y: usize,
    /// 2-D character buffer stored in row-major order (`rows * cols`).
    char_buffer: Vec<u8>,
    /// Per-cell foreground color (parallel to `char_buffer`).
    fg_buffer: Vec<u32>,
    /// Per-cell background color (parallel to `char_buffer`).
    bg_buffer: Vec<u32>,
    /// Active foreground color for newly written characters.
    fg_color: u32,
    /// Active background color for newly written characters.
    bg_color: u32,
}

impl GraphicalTerminal {
    /// Create a new terminal with the given dimensions (in characters).
    ///
    /// The pixel size of the terminal will be `cols * 8` wide by `rows * 16`
    /// tall (matching [`font::CHAR_WIDTH`] and [`font::CHAR_HEIGHT`]).
    pub fn new(cols: usize, rows: usize) -> Self {
        let size = cols * rows;
        Self {
            cols,
            rows,
            cursor_x: 0,
            cursor_y: 0,
            char_buffer: vec![b' '; size],
            fg_buffer: vec![DEFAULT_FG; size],
            bg_buffer: vec![DEFAULT_BG; size],
            fg_color: DEFAULT_FG,
            bg_color: DEFAULT_BG,
        }
    }

    /// Write a single byte to the terminal at the cursor position.
    ///
    /// Handles:
    /// - `\n` (0x0A): move cursor to start of next line, scrolling if needed.
    /// - `\r` (0x0D): carriage return — move cursor to column 0.
    /// - Backspace (0x08): move cursor back one column and erase the cell.
    /// - Tab (0x09): advance to the next 8-column tab stop.
    /// - All other bytes: place the character and advance the cursor.
    pub fn write_char(&mut self, ch: u8) {
        match ch {
            b'\n' => {
                self.cursor_x = 0;
                self.cursor_y += 1;
                if self.cursor_y >= self.rows {
                    self.scroll_up();
                    self.cursor_y = self.rows - 1;
                }
            }
            b'\r' => {
                self.cursor_x = 0;
            }
            0x08 => {
                // Backspace
                if self.cursor_x > 0 {
                    self.cursor_x -= 1;
                    let idx = self.cursor_y * self.cols + self.cursor_x;
                    self.char_buffer[idx] = b' ';
                    self.fg_buffer[idx] = self.fg_color;
                    self.bg_buffer[idx] = self.bg_color;
                }
            }
            b'\t' => {
                // Advance to next 8-column tab stop.
                let target = (self.cursor_x + 8) & !7;
                let target = if target > self.cols { self.cols } else { target };
                while self.cursor_x < target {
                    let idx = self.cursor_y * self.cols + self.cursor_x;
                    self.char_buffer[idx] = b' ';
                    self.fg_buffer[idx] = self.fg_color;
                    self.bg_buffer[idx] = self.bg_color;
                    self.cursor_x += 1;
                }
                if self.cursor_x >= self.cols {
                    self.cursor_x = 0;
                    self.cursor_y += 1;
                    if self.cursor_y >= self.rows {
                        self.scroll_up();
                        self.cursor_y = self.rows - 1;
                    }
                }
            }
            _ => {
                let idx = self.cursor_y * self.cols + self.cursor_x;
                self.char_buffer[idx] = ch;
                self.fg_buffer[idx] = self.fg_color;
                self.bg_buffer[idx] = self.bg_color;
                self.cursor_x += 1;
                if self.cursor_x >= self.cols {
                    self.cursor_x = 0;
                    self.cursor_y += 1;
                    if self.cursor_y >= self.rows {
                        self.scroll_up();
                        self.cursor_y = self.rows - 1;
                    }
                }
            }
        }
    }

    /// Write a UTF-8 string to the terminal, byte by byte.
    ///
    /// Non-ASCII bytes are passed through to `write_char` which renders
    /// them as the fallback glyph (space).
    pub fn write_str(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.write_char(b);
        }
    }

    /// Clear the entire terminal: fill the character buffer with spaces
    /// and reset the cursor to the top-left corner.
    pub fn clear(&mut self) {
        for cell in self.char_buffer.iter_mut() {
            *cell = b' ';
        }
        for c in self.fg_buffer.iter_mut() {
            *c = self.fg_color;
        }
        for c in self.bg_buffer.iter_mut() {
            *c = self.bg_color;
        }
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    /// Render the full terminal into a 32-bit ARGB pixel buffer.
    ///
    /// # Arguments
    /// - `buf` — destination pixel buffer (at least `stride * rows * CHAR_HEIGHT` elements).
    /// - `stride` — number of **pixels** (u32 elements) per row of the destination buffer.
    ///
    /// Each character cell is rendered using [`font::glyph`], producing an
    /// 8-wide by 16-tall block of pixels with the cell's foreground and
    /// background colors.
    pub fn render_to_buffer(&self, buf: &mut [u32], stride: usize) {
        for row in 0..self.rows {
            for col in 0..self.cols {
                let idx = row * self.cols + col;
                let ch = self.char_buffer[idx];
                let fg = self.fg_buffer[idx];
                let bg = self.bg_buffer[idx];
                let glyph = font::glyph(ch);

                let px_x = col * font::CHAR_WIDTH;
                let px_y = row * font::CHAR_HEIGHT;

                for gy in 0..font::CHAR_HEIGHT {
                    let row_bits = glyph[gy];
                    let dest_y = px_y + gy;
                    for gx in 0..font::CHAR_WIDTH {
                        let dest_x = px_x + gx;
                        let offset = dest_y * stride + dest_x;
                        if offset < buf.len() {
                            // MSB = leftmost pixel in each glyph byte.
                            let lit = (row_bits >> (7 - gx)) & 1 != 0;
                            buf[offset] = if lit { fg } else { bg };
                        }
                    }
                }
            }
        }

        // Draw a simple block cursor at the current position.
        self.render_cursor(buf, stride);
    }

    /// Draw a blinking-style block cursor by inverting the cell at the
    /// current cursor position.
    fn render_cursor(&self, buf: &mut [u32], stride: usize) {
        if self.cursor_y >= self.rows || self.cursor_x >= self.cols {
            return;
        }
        let px_x = self.cursor_x * font::CHAR_WIDTH;
        let px_y = self.cursor_y * font::CHAR_HEIGHT;
        // Underscore-style cursor: invert the bottom two rows of the cell.
        for gy in (font::CHAR_HEIGHT - 2)..font::CHAR_HEIGHT {
            for gx in 0..font::CHAR_WIDTH {
                let offset = (px_y + gy) * stride + (px_x + gx);
                if offset < buf.len() {
                    buf[offset] = self.fg_color;
                }
            }
        }
    }

    /// Scroll the terminal up by one line: move every row up, then clear
    /// the bottom row with spaces in the current background color.
    pub fn scroll_up(&mut self) {
        // Shift rows 1..rows into 0..rows-1.
        let cols = self.cols;
        for row in 1..self.rows {
            let src = row * cols;
            let dst = (row - 1) * cols;
            for c in 0..cols {
                self.char_buffer[dst + c] = self.char_buffer[src + c];
                self.fg_buffer[dst + c] = self.fg_buffer[src + c];
                self.bg_buffer[dst + c] = self.bg_buffer[src + c];
            }
        }
        // Clear the last row.
        let last = (self.rows - 1) * cols;
        for c in 0..cols {
            self.char_buffer[last + c] = b' ';
            self.fg_buffer[last + c] = self.fg_color;
            self.bg_buffer[last + c] = self.bg_color;
        }
    }

    /// Change the foreground and background colors for subsequently
    /// written characters. Existing characters are not affected.
    pub fn set_colors(&mut self, fg: u32, bg: u32) {
        self.fg_color = fg;
        self.bg_color = bg;
    }

    /// Return the terminal dimensions as `(cols, rows)`.
    pub fn get_size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// Return the current cursor position as `(col, row)`.
    pub fn get_cursor(&self) -> (usize, usize) {
        (self.cursor_x, self.cursor_y)
    }

    /// Return the required pixel dimensions for this terminal:
    /// `(width_pixels, height_pixels)`.
    pub fn pixel_size(&self) -> (usize, usize) {
        (self.cols * font::CHAR_WIDTH, self.rows * font::CHAR_HEIGHT)
    }
}

/// Implements `core::fmt::Write` so the terminal can be used with
/// `write!` / `writeln!` macros.
impl fmt::Write for GraphicalTerminal {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        GraphicalTerminal::write_str(self, s);
        Ok(())
    }
}
