/// Simple pixel drawing program for the MerlionOS framebuffer.
///
/// Provides a basic paint application with pen, line, rectangle, flood-fill,
/// and eraser tools. Draws directly to a pixel canvas backed by a `Vec<u32>`
/// buffer, which is rendered to the framebuffer on each frame.
///
/// # Controls
///
/// - **Mouse**: left-click to draw, right-click to pick color from canvas
/// - **Keyboard**: `p` pen, `l` line, `r` rect, `f` fill, `e` eraser,
///   `1`-`9` / `0` select palette colors, `c` clear, `s` save BMP, `q` quit
/// - **Brush size**: `+` / `-` to increase / decrease

use alloc::vec;
use alloc::vec::Vec;
use crate::bmp;
use crate::fbconsole;
use crate::keyboard::KeyEvent;
use crate::mouse::MouseState;

// ---------------------------------------------------------------------------
// VGA 16-color palette (0x00RRGGBB)
// ---------------------------------------------------------------------------

/// Standard 16-color VGA palette.
const COLOR_PALETTE: [u32; 16] = [
    0x00000000, // 0  black
    0x000000AA, // 1  blue
    0x0000AA00, // 2  green
    0x0000AAAA, // 3  cyan
    0x00AA0000, // 4  red
    0x00AA00AA, // 5  magenta
    0x00AA5500, // 6  brown
    0x00AAAAAA, // 7  light gray
    0x00555555, // 8  dark gray
    0x005555FF, // 9  light blue
    0x0055FF55, // 10 light green
    0x0055FFFF, // 11 light cyan
    0x00FF5555, // 12 light red
    0x00FF55FF, // 13 light magenta
    0x00FFFF55, // 14 yellow
    0x00FFFFFF, // 15 white
];

// ---------------------------------------------------------------------------
// Drawing tool
// ---------------------------------------------------------------------------

/// Available drawing tools.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tool {
    /// Freehand drawing that follows the cursor.
    Pen,
    /// Straight line from click to release.
    Line,
    /// Axis-aligned rectangle from click to release.
    Rect,
    /// Flood-fill a contiguous region with the current color.
    Fill,
    /// Eraser — draws with the background color.
    Eraser,
}

// ---------------------------------------------------------------------------
// PaintApp
// ---------------------------------------------------------------------------

/// Main paint application state.
pub struct PaintApp {
    /// Pixel canvas stored as 0x00RRGGBB values, row-major.
    pub canvas: Vec<u32>,
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// Currently selected foreground color.
    pub current_color: u32,
    /// Brush radius in pixels (for Pen and Eraser).
    pub brush_size: u32,
    /// Active drawing tool.
    pub tool: Tool,
    /// Current cursor X position.
    pub cursor_x: u32,
    /// Current cursor Y position.
    pub cursor_y: u32,
    /// Whether the user has requested to quit.
    pub quit: bool,
    /// Anchor point for line/rect tools (set on mouse-down).
    anchor_x: u32,
    anchor_y: u32,
    /// Whether a drag operation is in progress (line/rect).
    dragging: bool,
    /// Previous mouse-left state for edge detection.
    prev_left: bool,
}

impl PaintApp {
    /// Create a new paint application sized to the framebuffer.
    ///
    /// Falls back to 640x480 if no framebuffer is available.
    pub fn new() -> Self {
        let (w, h) = {
            let console = fbconsole::CONSOLE.lock();
            match console.fb() {
                Some(fb) => (fb.width, fb.height),
                None => (640, 480),
            }
        };
        let pixel_count = (w as usize) * (h as usize);
        Self {
            canvas: vec![0x00000000; pixel_count],
            width: w,
            height: h,
            current_color: COLOR_PALETTE[15], // white
            brush_size: 1,
            tool: Tool::Pen,
            cursor_x: w / 2,
            cursor_y: h / 2,
            quit: false,
            anchor_x: 0,
            anchor_y: 0,
            dragging: false,
            prev_left: false,
        }
    }

    // -----------------------------------------------------------------------
    // Primitive drawing operations (all operate on the canvas buffer)
    // -----------------------------------------------------------------------

    /// Set a single pixel on the canvas. Bounds-checked.
    pub fn draw_pixel(&mut self, x: u32, y: u32) {
        if x < self.width && y < self.height {
            self.canvas[(y as usize) * (self.width as usize) + (x as usize)] = self.current_color;
        }
    }

    /// Draw a filled circle of `brush_size` radius at (`cx`, `cy`).
    fn draw_brush(&mut self, cx: u32, cy: u32) {
        let r = self.brush_size as i32;
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r * r {
                    let px = cx as i32 + dx;
                    let py = cy as i32 + dy;
                    if px >= 0 && py >= 0 {
                        self.draw_pixel(px as u32, py as u32);
                    }
                }
            }
        }
    }

    /// Draw a line from (`x0`, `y0`) to (`x1`, `y1`) using Bresenham's algorithm.
    pub fn draw_line(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) {
        let mut x0 = x0 as i32;
        let mut y0 = y0 as i32;
        let x1 = x1 as i32;
        let y1 = y1 as i32;

        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx: i32 = if x0 < x1 { 1 } else { -1 };
        let sy: i32 = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;

        loop {
            if x0 >= 0 && y0 >= 0 {
                self.draw_pixel(x0 as u32, y0 as u32);
            }
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    /// Draw an axis-aligned rectangle outline from (`x`, `y`) to (`x+w`, `y+h`).
    pub fn draw_rect(&mut self, x: u32, y: u32, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        let x2 = x + w - 1;
        let y2 = y + h - 1;
        self.draw_line(x, y, x2, y);      // top
        self.draw_line(x, y2, x2, y2);    // bottom
        self.draw_line(x, y, x, y2);      // left
        self.draw_line(x2, y, x2, y2);    // right
    }

    /// Flood-fill a contiguous region starting at (`x`, `y`) with `color`.
    ///
    /// Replaces all connected pixels that match the original color at (`x`, `y`).
    /// Uses an iterative stack to avoid deep recursion.
    pub fn flood_fill(&mut self, x: u32, y: u32, color: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        let target = self.canvas[(y as usize) * (self.width as usize) + (x as usize)];
        if target == color {
            return;
        }

        let mut stack: Vec<(u32, u32)> = Vec::with_capacity(256);
        stack.push((x, y));

        let w = self.width as usize;
        while let Some((px, py)) = stack.pop() {
            let idx = (py as usize) * w + (px as usize);
            if self.canvas[idx] != target {
                continue;
            }
            self.canvas[idx] = color;

            if px > 0 {
                stack.push((px - 1, py));
            }
            if px + 1 < self.width {
                stack.push((px + 1, py));
            }
            if py > 0 {
                stack.push((px, py - 1));
            }
            if py + 1 < self.height {
                stack.push((px, py + 1));
            }
        }
    }

    /// Clear the entire canvas to black.
    pub fn clear(&mut self) {
        for pixel in self.canvas.iter_mut() {
            *pixel = 0x00000000;
        }
    }

    // -----------------------------------------------------------------------
    // Framebuffer rendering
    // -----------------------------------------------------------------------

    /// Render the canvas to the framebuffer, then draw a small crosshair cursor.
    pub fn render(&self) {
        let console = fbconsole::CONSOLE.lock();
        let fb = match console.fb() {
            Some(f) => f,
            None => return,
        };

        let stride = fb.stride as usize;
        let bpp = fb.bpp as usize;
        let fb_ptr = fb.addr as *mut u8;
        let w = self.width.min(fb.width) as usize;
        let h = self.height.min(fb.height) as usize;

        // Blit canvas to framebuffer.
        for y in 0..h {
            for x in 0..w {
                let color = self.canvas[y * (self.width as usize) + x];
                let offset = y * stride + x * bpp;
                unsafe {
                    (fb_ptr.add(offset) as *mut u32).write_volatile(color);
                }
            }
        }

        // Draw a small crosshair cursor (inverted color).
        let cx = self.cursor_x as usize;
        let cy = self.cursor_y as usize;
        let cursor_color: u32 = 0x00FFFFFF;
        for d in 0..=4_usize {
            for &(dx, dy) in &[(d, 0), (0, d), (-(d as isize) as usize, 0)] {
                // Use wrapping to handle the signed offset trick above.
                let px = cx.wrapping_add(dx);
                let py = cy.wrapping_add(dy);
                if px < w && py < h {
                    let offset = py * stride + px * bpp;
                    unsafe {
                        (fb_ptr.add(offset) as *mut u32).write_volatile(cursor_color);
                    }
                }
            }
            // Negative y offset.
            let py = cy.wrapping_sub(d);
            if cx < w && py < h {
                let offset = py * stride + cx * bpp;
                unsafe {
                    (fb_ptr.add(offset) as *mut u32).write_volatile(cursor_color);
                }
            }
            // Positive y offset.
            let py = cy + d;
            if cx < w && py < h {
                let offset = py * stride + cx * bpp;
                unsafe {
                    (fb_ptr.add(offset) as *mut u32).write_volatile(cursor_color);
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Input handling
    // -----------------------------------------------------------------------

    /// Handle a mouse state update. Draws on the canvas based on the active tool.
    pub fn handle_mouse(&mut self, state: MouseState) {
        self.cursor_x = (state.x as u32).min(self.width.saturating_sub(1));
        self.cursor_y = (state.y as u32).min(self.height.saturating_sub(1));

        let just_pressed = state.left && !self.prev_left;
        let just_released = !state.left && self.prev_left;

        match self.tool {
            Tool::Pen => {
                if state.left {
                    self.draw_brush(self.cursor_x, self.cursor_y);
                }
            }
            Tool::Eraser => {
                if state.left {
                    let saved = self.current_color;
                    self.current_color = 0x00000000;
                    self.draw_brush(self.cursor_x, self.cursor_y);
                    self.current_color = saved;
                }
            }
            Tool::Line => {
                if just_pressed {
                    self.anchor_x = self.cursor_x;
                    self.anchor_y = self.cursor_y;
                    self.dragging = true;
                } else if just_released && self.dragging {
                    self.draw_line(self.anchor_x, self.anchor_y, self.cursor_x, self.cursor_y);
                    self.dragging = false;
                }
            }
            Tool::Rect => {
                if just_pressed {
                    self.anchor_x = self.cursor_x;
                    self.anchor_y = self.cursor_y;
                    self.dragging = true;
                } else if just_released && self.dragging {
                    let rx = self.anchor_x.min(self.cursor_x);
                    let ry = self.anchor_y.min(self.cursor_y);
                    let rw = self.anchor_x.abs_diff(self.cursor_x) + 1;
                    let rh = self.anchor_y.abs_diff(self.cursor_y) + 1;
                    self.draw_rect(rx, ry, rw, rh);
                    self.dragging = false;
                }
            }
            Tool::Fill => {
                if just_pressed {
                    let color = self.current_color;
                    self.flood_fill(self.cursor_x, self.cursor_y, color);
                }
            }
        }

        // Right-click: pick color from canvas.
        if state.right && self.cursor_x < self.width && self.cursor_y < self.height {
            let idx = (self.cursor_y as usize) * (self.width as usize) + (self.cursor_x as usize);
            self.current_color = self.canvas[idx];
        }

        self.prev_left = state.left;
    }

    /// Handle a keyboard event. Returns `true` if the app should continue running.
    pub fn handle_key(&mut self, event: KeyEvent) {
        match event {
            KeyEvent::Char('p') => self.tool = Tool::Pen,
            KeyEvent::Char('l') => self.tool = Tool::Line,
            KeyEvent::Char('r') => self.tool = Tool::Rect,
            KeyEvent::Char('f') => self.tool = Tool::Fill,
            KeyEvent::Char('e') => self.tool = Tool::Eraser,
            KeyEvent::Char('c') => self.clear(),
            KeyEvent::Char('s') => { self.save_to_bmp(); }
            KeyEvent::Char('q') => self.quit = true,
            KeyEvent::Char('+') | KeyEvent::Char('=') => {
                if self.brush_size < 16 {
                    self.brush_size += 1;
                }
            }
            KeyEvent::Char('-') => {
                if self.brush_size > 1 {
                    self.brush_size -= 1;
                }
            }
            // Number keys select palette colors: 1-9 => palette[1..9], 0 => palette[0].
            KeyEvent::Char(ch @ '0'..='9') => {
                let idx = if ch == '0' { 0 } else { (ch as usize) - ('0' as usize) };
                if idx < COLOR_PALETTE.len() {
                    self.current_color = COLOR_PALETTE[idx];
                }
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // File output
    // -----------------------------------------------------------------------

    /// Encode the current canvas as a 24-bit BMP file using [`crate::bmp::create_bmp`].
    ///
    /// Returns the raw BMP bytes. The caller (or a future VFS integration)
    /// is responsible for persisting the data.
    pub fn save_to_bmp(&self) -> Vec<u8> {
        bmp::create_bmp(self.width, self.height, &self.canvas)
    }
}
