/// Framebuffer console — renders text to a pixel framebuffer.
/// Replaces VGA text mode on UEFI/real hardware where 0xB8000 doesn't exist.
/// Uses the bitmap font from font.rs (8×16 pixels per character).
///
/// This console is resolution-independent: works with any framebuffer
/// provided by UEFI GOP or Limine.

use crate::font;
use spin::Mutex;
use core::fmt;

pub static CONSOLE: Mutex<FbConsole> = Mutex::new(FbConsole::uninitialized());

/// Framebuffer information (provided by bootloader).
#[derive(Clone, Copy)]
pub struct FbInfo {
    pub addr: u64,          // virtual address of framebuffer
    pub width: u32,         // pixels
    pub height: u32,        // pixels
    pub stride: u32,        // bytes per row (may be > width * bpp)
    pub bpp: u8,            // bytes per pixel (typically 4 = 32-bit)
}

/// Console state.
pub struct FbConsole {
    fb: Option<FbInfo>,
    col: usize,             // current column (in characters)
    row: usize,             // current row (in characters)
    cols: usize,            // total columns
    rows: usize,            // total rows
    fg: u32,                // foreground color (0xRRGGBB)
    bg: u32,                // background color
}

impl FbConsole {
    const fn uninitialized() -> Self {
        Self {
            fb: None,
            col: 0, row: 0,
            cols: 0, rows: 0,
            fg: 0x00CCCCCC,   // light gray
            bg: 0x00000000,   // black
        }
    }

    /// Initialize with framebuffer info from the bootloader.
    pub fn init(&mut self, info: FbInfo) {
        self.fb = Some(info);
        self.cols = info.width as usize / font::CHAR_WIDTH;
        self.rows = info.height as usize / font::CHAR_HEIGHT;
        self.col = 0;
        self.row = 0;
        self.clear();
    }

    /// Check if the framebuffer console is active.
    pub fn is_active(&self) -> bool {
        self.fb.is_some()
    }

    /// Return the underlying framebuffer info (if initialized).
    pub fn fb(&self) -> Option<FbInfo> {
        self.fb
    }

    /// Clear the screen.
    pub fn clear(&mut self) {
        let fb = match self.fb { Some(f) => f, None => return };
        let ptr = fb.addr as *mut u8;
        let total_bytes = fb.stride as usize * fb.height as usize;
        unsafe {
            core::ptr::write_bytes(ptr, 0, total_bytes);
        }
        self.col = 0;
        self.row = 0;
    }

    /// Set foreground color.
    pub fn set_fg(&mut self, color: u32) { self.fg = color; }
    /// Set background color.
    pub fn set_bg(&mut self, color: u32) { self.bg = color; }

    /// Write a single character.
    pub fn write_char(&mut self, ch: u8) {
        match ch {
            b'\n' => self.newline(),
            b'\r' => self.col = 0,
            b'\x08' => { // backspace
                if self.col > 0 {
                    self.col -= 1;
                    self.draw_char(self.row, self.col, b' ');
                }
            }
            ch => {
                if self.col >= self.cols {
                    self.newline();
                }
                self.draw_char(self.row, self.col, ch);
                self.col += 1;
            }
        }
    }

    fn newline(&mut self) {
        self.col = 0;
        if self.row < self.rows - 1 {
            self.row += 1;
        } else {
            self.scroll();
        }
    }

    fn scroll(&mut self) {
        let fb = match self.fb { Some(f) => f, None => return };
        let ptr = fb.addr as *mut u8;
        let row_bytes = fb.stride as usize * font::CHAR_HEIGHT;
        let total_rows = self.rows;

        unsafe {
            // Move rows up by one character height
            let total = row_bytes * (total_rows - 1);
            core::ptr::copy(ptr.add(row_bytes), ptr, total);
            // Clear last row
            core::ptr::write_bytes(ptr.add(total), 0, row_bytes);
        }
    }

    /// Draw a character at (row, col) using the bitmap font.
    fn draw_char(&self, row: usize, col: usize, ch: u8) {
        let fb = match self.fb { Some(f) => f, None => return };
        let glyph = font::glyph(ch);
        let px = col * font::CHAR_WIDTH;
        let py = row * font::CHAR_HEIGHT;

        for gy in 0..font::CHAR_HEIGHT {
            let bits = glyph[gy];
            for gx in 0..font::CHAR_WIDTH {
                let pixel_on = bits & (0x80 >> gx) != 0;
                let color = if pixel_on { self.fg } else { self.bg };
                self.put_pixel(fb, px + gx, py + gy, color);
            }
        }
    }

    /// Set a pixel in the framebuffer (32-bit BGR format, typical for UEFI).
    #[inline(always)]
    fn put_pixel(&self, fb: FbInfo, x: usize, y: usize, color: u32) {
        if x >= fb.width as usize || y >= fb.height as usize { return; }
        let offset = y * fb.stride as usize + x * fb.bpp as usize;
        unsafe {
            let ptr = (fb.addr as *mut u8).add(offset);
            // Write as 32-bit (works for both RGB and BGR since we'll match format)
            (ptr as *mut u32).write_volatile(color);
        }
    }

    /// Get console dimensions.
    pub fn dimensions(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }
}

impl fmt::Write for FbConsole {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.write_char(byte);
        }
        Ok(())
    }
}

/// Color constants (RGB format).
pub const WHITE: u32 = 0x00FFFFFF;
pub const BLACK: u32 = 0x00000000;
pub const RED: u32 = 0x00FF4444;
pub const GREEN: u32 = 0x0044FF44;
pub const BLUE: u32 = 0x004444FF;
pub const CYAN: u32 = 0x0044FFFF;
pub const YELLOW: u32 = 0x00FFFF44;
pub const GRAY: u32 = 0x00AAAAAA;
pub const DARK_GRAY: u32 = 0x00666666;
