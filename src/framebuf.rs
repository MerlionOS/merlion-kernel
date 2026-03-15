/// Software framebuffer with block-character rendering.
/// Uses VGA text mode half-block characters (▀▄█) to achieve
/// 160x50 pixel resolution with 16 colors — no mode switch needed.
///
/// Each text cell (80 cols × 25 rows) holds two vertical pixels
/// using upper/lower half-block characters with fg/bg colors.

use crate::vga::{Color, color_attr};
use spin::Mutex;

/// Framebuffer resolution: 160 wide × 50 tall
pub const FB_WIDTH: usize = 160;
pub const FB_HEIGHT: usize = 50;

/// VGA text mode dimensions
const TEXT_COLS: usize = 80;
const TEXT_ROWS: usize = 25;

pub static FRAMEBUF: Mutex<FrameBuffer> = Mutex::new(FrameBuffer::new());

pub struct FrameBuffer {
    /// Pixel buffer: each pixel is a 4-bit VGA color (0-15)
    pixels: [u8; FB_WIDTH * FB_HEIGHT],
}

impl FrameBuffer {
    const fn new() -> Self {
        Self {
            pixels: [0; FB_WIDTH * FB_HEIGHT],
        }
    }

    /// Clear the framebuffer to a color.
    pub fn clear(&mut self, color: u8) {
        self.pixels.fill(color & 0x0F);
    }

    /// Set a single pixel.
    pub fn set_pixel(&mut self, x: usize, y: usize, color: u8) {
        if x < FB_WIDTH && y < FB_HEIGHT {
            self.pixels[y * FB_WIDTH + x] = color & 0x0F;
        }
    }

    /// Get a single pixel.
    pub fn get_pixel(&self, x: usize, y: usize) -> u8 {
        if x < FB_WIDTH && y < FB_HEIGHT {
            self.pixels[y * FB_WIDTH + x]
        } else {
            0
        }
    }

    /// Draw a horizontal line.
    pub fn hline(&mut self, x: usize, y: usize, len: usize, color: u8) {
        for i in 0..len {
            self.set_pixel(x + i, y, color);
        }
    }

    /// Draw a vertical line.
    pub fn vline(&mut self, x: usize, y: usize, len: usize, color: u8) {
        for i in 0..len {
            self.set_pixel(x, y + i, color);
        }
    }

    /// Draw a line using Bresenham's algorithm.
    pub fn line(&mut self, x0: isize, y0: isize, x1: isize, y1: isize, color: u8) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx: isize = if x0 < x1 { 1 } else { -1 };
        let sy: isize = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let mut x = x0;
        let mut y = y0;

        loop {
            if x >= 0 && y >= 0 {
                self.set_pixel(x as usize, y as usize, color);
            }
            if x == x1 && y == y1 { break; }
            let e2 = 2 * err;
            if e2 >= dy { err += dy; x += sx; }
            if e2 <= dx { err += dx; y += sy; }
        }
    }

    /// Draw a rectangle outline.
    pub fn rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u8) {
        self.hline(x, y, w, color);
        self.hline(x, y + h - 1, w, color);
        self.vline(x, y, h, color);
        self.vline(x + w - 1, y, h, color);
    }

    /// Draw a filled rectangle.
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u8) {
        for row in y..y + h {
            self.hline(x, row, w, color);
        }
    }

    /// Draw a circle outline using midpoint algorithm.
    pub fn circle(&mut self, cx: isize, cy: isize, r: isize, color: u8) {
        let mut x = r;
        let mut y: isize = 0;
        let mut err = 1 - r;

        while x >= y {
            self.set_pixel((cx + x) as usize, (cy + y) as usize, color);
            self.set_pixel((cx - x) as usize, (cy + y) as usize, color);
            self.set_pixel((cx + x) as usize, (cy - y) as usize, color);
            self.set_pixel((cx - x) as usize, (cy - y) as usize, color);
            self.set_pixel((cx + y) as usize, (cy + x) as usize, color);
            self.set_pixel((cx - y) as usize, (cy + x) as usize, color);
            self.set_pixel((cx + y) as usize, (cy - x) as usize, color);
            self.set_pixel((cx - y) as usize, (cy - x) as usize, color);
            y += 1;
            if err < 0 {
                err += 2 * y + 1;
            } else {
                x -= 1;
                err += 2 * (y - x) + 1;
            }
        }
    }

    /// Draw a filled circle.
    pub fn fill_circle(&mut self, cx: isize, cy: isize, r: isize, color: u8) {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r * r {
                    self.set_pixel((cx + dx) as usize, (cy + dy) as usize, color);
                }
            }
        }
    }

    /// Render the framebuffer to VGA text mode using half-block characters.
    /// Each text cell represents 2 vertical pixels:
    ///   - top pixel = foreground color with ▀ (0xDF)
    ///   - bottom pixel = background color
    ///   - both same = █ (0xDB) with that fg color
    ///   - both black = space
    pub fn render(&self) {
        let vga = 0xB8000 as *mut u8;

        for row in 0..TEXT_ROWS {
            for col in 0..TEXT_COLS {
                let top_pixel = self.get_pixel(col * 2, row * 2);
                let top_pixel2 = self.get_pixel(col * 2 + 1, row * 2);
                let bot_pixel = self.get_pixel(col * 2, row * 2 + 1);
                let bot_pixel2 = self.get_pixel(col * 2 + 1, row * 2 + 1);

                // Average the two horizontal pixels for each half
                let top = if top_pixel > top_pixel2 { top_pixel } else { top_pixel2 };
                let bot = if bot_pixel > bot_pixel2 { bot_pixel } else { bot_pixel2 };

                let offset = (row * TEXT_COLS + col) * 2;

                let (ch, attr) = if top == 0 && bot == 0 {
                    (b' ', 0x00)
                } else if top == bot {
                    (0xDB, color_attr(to_color(top), Color::Black)) // █ full block
                } else if top == 0 {
                    (0xDC, color_attr(to_color(bot), Color::Black)) // ▄ bottom half
                } else if bot == 0 {
                    (0xDF, color_attr(to_color(top), Color::Black)) // ▀ top half
                } else {
                    // Both non-zero different colors: ▀ with top=fg, bot=bg
                    (0xDF, color_attr(to_color(top), to_color(bot)))
                };

                unsafe {
                    vga.add(offset).write_volatile(ch);
                    vga.add(offset + 1).write_volatile(attr);
                }
            }
        }
    }
}

/// Map a 4-bit color index to a VGA Color enum.
fn to_color(c: u8) -> Color {
    match c & 0x0F {
        0x0 => Color::Black,
        0x1 => Color::Blue,
        0x2 => Color::Green,
        0x3 => Color::Cyan,
        0x4 => Color::Red,
        0x5 => Color::Magenta,
        0x6 => Color::Brown,
        0x7 => Color::LightGray,
        0x8 => Color::DarkGray,
        0x9 => Color::LightBlue,
        0xA => Color::LightGreen,
        0xB => Color::LightCyan,
        0xC => Color::LightRed,
        0xD => Color::Pink,
        0xE => Color::Yellow,
        0xF => Color::White,
        _ => Color::Black,
    }
}

/// Color constants for convenience
pub const BLACK: u8 = 0;
pub const BLUE: u8 = 1;
pub const GREEN: u8 = 2;
pub const CYAN: u8 = 3;
pub const RED: u8 = 4;
pub const MAGENTA: u8 = 5;
pub const BROWN: u8 = 6;
pub const LIGHT_GRAY: u8 = 7;
pub const DARK_GRAY: u8 = 8;
pub const LIGHT_BLUE: u8 = 9;
pub const LIGHT_GREEN: u8 = 10;
pub const LIGHT_CYAN: u8 = 11;
pub const LIGHT_RED: u8 = 12;
pub const PINK: u8 = 13;
pub const YELLOW: u8 = 14;
pub const WHITE: u8 = 15;

/// Run a graphics demo: draws shapes, waits, then returns to text mode.
pub fn demo() {
    let mut fb = FRAMEBUF.lock();
    fb.clear(BLACK);

    // Singapore flag colors: red top half, white bottom half
    fb.fill_rect(10, 2, 60, 20, RED);
    fb.fill_rect(10, 22, 60, 20, WHITE);

    // Crescent moon (approximate with overlapping circles)
    fb.fill_circle(28, 12, 8, WHITE);
    fb.fill_circle(31, 12, 7, RED); // cut out to make crescent

    // Five stars (small dots)
    let stars = [(36, 8), (33, 11), (39, 11), (34, 15), (38, 15)];
    for (sx, sy) in stars {
        fb.fill_circle(sx, sy, 1, WHITE);
    }

    // "MerlionOS" label area
    fb.fill_rect(85, 5, 65, 15, BLUE);
    fb.rect(85, 5, 65, 15, LIGHT_CYAN);

    // Draw some demo shapes on the right
    fb.circle(120, 35, 10, YELLOW);
    fb.fill_rect(90, 30, 15, 10, GREEN);
    fb.line(10, 45, 150, 45, DARK_GRAY);

    // Colorful palette bar at bottom
    for i in 0..16u8 {
        fb.fill_rect(10 + (i as usize) * 9, 46, 8, 3, i);
    }

    fb.render();
}
