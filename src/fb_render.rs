/// Framebuffer rendering engine for MerlionOS.
/// Provides 2D pixel drawing, bitmap font rendering, text console,
/// and resolution management on UEFI GOP framebuffers.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::font;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const CHAR_WIDTH: u32 = 8;
const CHAR_HEIGHT: u32 = 16;
const MAX_COLS: usize = 320; // 2560 / 8
const MAX_ROWS: usize = 135; // 2160 / 16

/// Default colors (0xRRGGBB stored in u32)
const DEFAULT_FG: u32 = 0x00CCCCCC;
const DEFAULT_BG: u32 = 0x00000000;

// ---------------------------------------------------------------------------
// Framebuffer info
// ---------------------------------------------------------------------------

/// Describes the layout of a linear framebuffer.
#[derive(Clone, Copy, Debug)]
pub struct FramebufferInfo {
    pub addr: u64,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub bpp: u8,
    pub red_shift: u8,
    pub green_shift: u8,
    pub blue_shift: u8,
}

impl FramebufferInfo {
    const fn empty() -> Self {
        Self {
            addr: 0,
            width: 0,
            height: 0,
            pitch: 0,
            bpp: 4,
            red_shift: 16,
            green_shift: 8,
            blue_shift: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// ANSI color table (8 standard + 8 bright)
// ---------------------------------------------------------------------------

static ANSI_COLORS: [u32; 16] = [
    0x00000000, // 0 black
    0x00AA0000, // 1 red
    0x0000AA00, // 2 green
    0x00AA5500, // 3 yellow/brown
    0x000000AA, // 4 blue
    0x00AA00AA, // 5 magenta
    0x0000AAAA, // 6 cyan
    0x00AAAAAA, // 7 white
    0x00555555, // 8 bright black (gray)
    0x00FF5555, // 9 bright red
    0x0055FF55, // 10 bright green
    0x00FFFF55, // 11 bright yellow
    0x005555FF, // 12 bright blue
    0x00FF55FF, // 13 bright magenta
    0x0055FFFF, // 14 bright cyan
    0x00FFFFFF, // 15 bright white
];

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static FB_ACTIVE: AtomicBool = AtomicBool::new(false);
static PIXELS_DRAWN: AtomicU64 = AtomicU64::new(0);
static RECTS_DRAWN: AtomicU64 = AtomicU64::new(0);
static CHARS_DRAWN: AtomicU64 = AtomicU64::new(0);
static SCROLLS: AtomicU64 = AtomicU64::new(0);
static SWAP_COUNT: AtomicU64 = AtomicU64::new(0);

struct FbRenderState {
    info: FramebufferInfo,
    /// Text console cursor (character coordinates)
    col: u32,
    row: u32,
    cols: u32,
    rows: u32,
    fg: u32,
    bg: u32,
    bold: bool,
    /// Double-buffer backing store
    back_buffer: Vec<u8>,
    double_buffered: bool,
}

impl FbRenderState {
    const fn new() -> Self {
        Self {
            info: FramebufferInfo::empty(),
            col: 0,
            row: 0,
            cols: 0,
            rows: 0,
            fg: DEFAULT_FG,
            bg: DEFAULT_BG,
            bold: false,
            back_buffer: Vec::new(),
            double_buffered: false,
        }
    }
}

static STATE: Mutex<FbRenderState> = Mutex::new(FbRenderState::new());

// ---------------------------------------------------------------------------
// Pixel operations (raw framebuffer writes)
// ---------------------------------------------------------------------------

/// Pack (r, g, b) into a u32 according to the framebuffer channel shifts.
#[inline]
fn pack_color(info: &FramebufferInfo, r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << info.red_shift)
        | ((g as u32) << info.green_shift)
        | ((b as u32) << info.blue_shift)
}

/// Write a packed color to a pixel position in the given buffer address.
#[inline]
unsafe fn write_pixel(base: *mut u8, pitch: u32, bpp: u8, x: u32, y: u32, color: u32) {
    let offset = (y as usize) * (pitch as usize) + (x as usize) * (bpp as usize);
    let ptr = base.add(offset);
    // Write 3 or 4 bytes depending on bpp
    ptr.write_volatile(color as u8);
    ptr.add(1).write_volatile((color >> 8) as u8);
    ptr.add(2).write_volatile((color >> 16) as u8);
    if bpp == 4 {
        ptr.add(3).write_volatile(0);
    }
}

/// Get the write target (back-buffer pointer or direct framebuffer).
fn write_target(state: &mut FbRenderState) -> *mut u8 {
    if state.double_buffered && !state.back_buffer.is_empty() {
        state.back_buffer.as_mut_ptr()
    } else {
        state.info.addr as *mut u8
    }
}

/// Write one pixel at (x, y) with color (r, g, b).
pub fn put_pixel(x: u32, y: u32, r: u8, g: u8, b: u8) {
    let mut state = STATE.lock();
    if x >= state.info.width || y >= state.info.height {
        return;
    }
    let color = pack_color(&state.info, r, g, b);
    let base = write_target(&mut state);
    let pitch = state.info.pitch;
    let bpp = state.info.bpp;
    unsafe { write_pixel(base, pitch, bpp, x, y, color); }
    PIXELS_DRAWN.fetch_add(1, Ordering::Relaxed);
}

/// Fill a rectangle at (x, y) with size (w, h).
pub fn fill_rect(x: u32, y: u32, w: u32, h: u32, r: u8, g: u8, b: u8) {
    let mut state = STATE.lock();
    let color = pack_color(&state.info, r, g, b);
    let base = write_target(&mut state);
    let pitch = state.info.pitch;
    let bpp = state.info.bpp;
    let max_x = state.info.width;
    let max_y = state.info.height;
    for py in y..y.saturating_add(h).min(max_y) {
        for px in x..x.saturating_add(w).min(max_x) {
            unsafe { write_pixel(base, pitch, bpp, px, py, color); }
        }
    }
    RECTS_DRAWN.fetch_add(1, Ordering::Relaxed);
}

/// Draw a rectangle outline at (x, y) with size (w, h).
pub fn draw_rect(x: u32, y: u32, w: u32, h: u32, r: u8, g: u8, b: u8) {
    if w == 0 || h == 0 {
        return;
    }
    // Top and bottom edges
    fill_rect(x, y, w, 1, r, g, b);
    fill_rect(x, y + h - 1, w, 1, r, g, b);
    // Left and right edges
    fill_rect(x, y, 1, h, r, g, b);
    fill_rect(x + w - 1, y, 1, h, r, g, b);
}

/// Draw a line from (x0, y0) to (x1, y1) using Bresenham's algorithm.
pub fn draw_line(x0: i32, y0: i32, x1: i32, y1: i32, r: u8, g: u8, b: u8) {
    let mut state = STATE.lock();
    let color = pack_color(&state.info, r, g, b);
    let base = write_target(&mut state);
    let pitch = state.info.pitch;
    let bpp = state.info.bpp;
    let max_x = state.info.width as i32;
    let max_y = state.info.height as i32;

    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx: i32 = if x0 < x1 { 1 } else { -1 };
    let sy: i32 = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut cx = x0;
    let mut cy = y0;

    loop {
        if cx >= 0 && cy >= 0 && cx < max_x && cy < max_y {
            unsafe { write_pixel(base, pitch, bpp, cx as u32, cy as u32, color); }
        }
        if cx == x1 && cy == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            cx += sx;
        }
        if e2 <= dx {
            err += dx;
            cy += sy;
        }
    }
    PIXELS_DRAWN.fetch_add(1, Ordering::Relaxed);
}

/// Draw a circle outline using the midpoint algorithm.
pub fn draw_circle(cx: i32, cy: i32, radius: i32, r: u8, g: u8, b: u8) {
    let mut state = STATE.lock();
    let color = pack_color(&state.info, r, g, b);
    let base = write_target(&mut state);
    let pitch = state.info.pitch;
    let bpp = state.info.bpp;
    let max_x = state.info.width as i32;
    let max_y = state.info.height as i32;

    let mut x = radius;
    let mut y: i32 = 0;
    let mut err = 1 - radius;

    while x >= y {
        let points = [
            (cx + x, cy + y), (cx - x, cy + y),
            (cx + x, cy - y), (cx - x, cy - y),
            (cx + y, cy + x), (cx - y, cy + x),
            (cx + y, cy - x), (cx - y, cy - x),
        ];
        for (px, py) in points {
            if px >= 0 && py >= 0 && px < max_x && py < max_y {
                unsafe { write_pixel(base, pitch, bpp, px as u32, py as u32, color); }
            }
        }
        y += 1;
        if err < 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x) + 1;
        }
    }
    PIXELS_DRAWN.fetch_add(1, Ordering::Relaxed);
}

/// Blit a pixel buffer (RGBA, row-major) to (x, y) with size (w, h).
/// `pixels` is a slice of u32 packed as 0x00RRGGBB.
pub fn blit(x: u32, y: u32, w: u32, h: u32, pixels: &[u32]) {
    let mut state = STATE.lock();
    let base = write_target(&mut state);
    let pitch = state.info.pitch;
    let bpp = state.info.bpp;
    let info = state.info;
    let max_x = info.width;
    let max_y = info.height;

    for py in 0..h {
        for px in 0..w {
            let dx = x + px;
            let dy = y + py;
            if dx >= max_x || dy >= max_y {
                continue;
            }
            let idx = (py * w + px) as usize;
            if idx >= pixels.len() {
                continue;
            }
            let raw = pixels[idx];
            let pr = ((raw >> 16) & 0xFF) as u8;
            let pg = ((raw >> 8) & 0xFF) as u8;
            let pb = (raw & 0xFF) as u8;
            let color = pack_color(&info, pr, pg, pb);
            unsafe { write_pixel(base, pitch, bpp, dx, dy, color); }
        }
    }
    PIXELS_DRAWN.fetch_add((w as u64) * (h as u64), Ordering::Relaxed);
}

/// Scroll the framebuffer up by `lines` pixel rows.
pub fn scroll_up(lines: u32) {
    let mut state = STATE.lock();
    if lines == 0 || state.info.height == 0 {
        return;
    }
    let pitch = state.info.pitch as usize;
    let h = state.info.height;
    let base = write_target(&mut state);
    let line_bytes = lines as usize * pitch;
    let total_bytes = h as usize * pitch;

    if line_bytes >= total_bytes {
        // Clear entire screen
        unsafe {
            core::ptr::write_bytes(base, 0, total_bytes);
        }
    } else {
        // Move rows up
        unsafe {
            core::ptr::copy(base.add(line_bytes), base, total_bytes - line_bytes);
            // Clear the bottom lines
            core::ptr::write_bytes(base.add(total_bytes - line_bytes), 0, line_bytes);
        }
    }
    SCROLLS.fetch_add(1, Ordering::Relaxed);
}

/// Clear the entire screen to (r, g, b).
pub fn clear(r: u8, g: u8, b: u8) {
    let mut state = STATE.lock();
    let color = pack_color(&state.info, r, g, b);
    let base = write_target(&mut state);
    let pitch = state.info.pitch;
    let bpp = state.info.bpp;
    let w = state.info.width;
    let h = state.info.height;
    for py in 0..h {
        for px in 0..w {
            unsafe { write_pixel(base, pitch, bpp, px, py, color); }
        }
    }
}

// ---------------------------------------------------------------------------
// Bitmap font rendering
// ---------------------------------------------------------------------------

/// Draw a single character at pixel position (x, y) with foreground/background.
pub fn draw_char(x: u32, y: u32, ch: u8, fg_r: u8, fg_g: u8, fg_b: u8, bg_r: u8, bg_g: u8, bg_b: u8) {
    let mut state = STATE.lock();
    let fg = pack_color(&state.info, fg_r, fg_g, fg_b);
    let bg = pack_color(&state.info, bg_r, bg_g, bg_b);
    let base = write_target(&mut state);
    let pitch = state.info.pitch;
    let bpp = state.info.bpp;
    let max_x = state.info.width;
    let max_y = state.info.height;
    let glyph = font::glyph(ch);

    for row in 0..CHAR_HEIGHT {
        let bits = glyph[row as usize];
        for col in 0..CHAR_WIDTH {
            let px = x + col;
            let py = y + row;
            if px >= max_x || py >= max_y {
                continue;
            }
            let on = (bits >> (7 - col)) & 1 != 0;
            let color = if on { fg } else { bg };
            unsafe { write_pixel(base, pitch, bpp, px, py, color); }
        }
    }
    CHARS_DRAWN.fetch_add(1, Ordering::Relaxed);
}

/// Draw a string at pixel position (x, y).
/// fg and bg are packed as 0x00RRGGBB.
pub fn draw_string(x: u32, y: u32, text: &str, fg: u32, bg: u32) {
    let mut state = STATE.lock();
    let fg_packed = pack_color(&state.info,
        ((fg >> 16) & 0xFF) as u8,
        ((fg >> 8) & 0xFF) as u8,
        (fg & 0xFF) as u8);
    let bg_packed = pack_color(&state.info,
        ((bg >> 16) & 0xFF) as u8,
        ((bg >> 8) & 0xFF) as u8,
        (bg & 0xFF) as u8);
    let base = write_target(&mut state);
    let pitch = state.info.pitch;
    let bpp = state.info.bpp;
    let max_x = state.info.width;
    let max_y = state.info.height;

    let mut cx = x;
    for ch in text.bytes() {
        if cx + CHAR_WIDTH > max_x {
            break;
        }
        let glyph = font::glyph(ch);
        for row in 0..CHAR_HEIGHT {
            let bits = glyph[row as usize];
            for col in 0..CHAR_WIDTH {
                let px = cx + col;
                let py = y + row;
                if px >= max_x || py >= max_y {
                    continue;
                }
                let on = (bits >> (7 - col)) & 1 != 0;
                let color = if on { fg_packed } else { bg_packed };
                unsafe { write_pixel(base, pitch, bpp, px, py, color); }
            }
        }
        cx += CHAR_WIDTH;
        CHARS_DRAWN.fetch_add(1, Ordering::Relaxed);
    }
}

/// Return pixel width of a text string.
pub fn text_width(text: &str) -> u32 {
    text.len() as u32 * CHAR_WIDTH
}

/// Return pixel height of a single line of text.
pub fn text_height() -> u32 {
    CHAR_HEIGHT
}

// ---------------------------------------------------------------------------
// Framebuffer text console
// ---------------------------------------------------------------------------

/// Unpack a u32 color to (r, g, b).
fn unpack_rgb(c: u32) -> (u8, u8, u8) {
    (((c >> 16) & 0xFF) as u8, ((c >> 8) & 0xFF) as u8, (c & 0xFF) as u8)
}

/// Print a single character to the framebuffer console with wrap/scroll.
pub fn fb_putc(ch: u8) {
    let mut state = STATE.lock();
    if state.info.addr == 0 || state.cols == 0 {
        return;
    }

    match ch {
        b'\n' => {
            state.col = 0;
            state.row += 1;
            if state.row >= state.rows {
                fb_scroll_one(&mut state);
                state.row = state.rows - 1;
            }
        }
        b'\r' => {
            state.col = 0;
        }
        b'\t' => {
            let next = (state.col + 4) & !3;
            state.col = if next < state.cols { next } else { state.cols - 1 };
        }
        0x08 => {
            // Backspace
            if state.col > 0 {
                state.col -= 1;
                let px = state.col * CHAR_WIDTH;
                let py = state.row * CHAR_HEIGHT;
                let bg_color = state.bg;
                draw_char_inner(&mut state, px, py, b' ', bg_color, bg_color);
            }
        }
        _ => {
            let px = state.col * CHAR_WIDTH;
            let py = state.row * CHAR_HEIGHT;
            let fg = if state.bold {
                state.fg | 0x00555555
            } else {
                state.fg
            };
            let bg_color = state.bg;
            draw_char_inner(&mut state, px, py, ch, fg, bg_color);
            state.col += 1;
            if state.col >= state.cols {
                state.col = 0;
                state.row += 1;
                if state.row >= state.rows {
                    fb_scroll_one(&mut state);
                    state.row = state.rows - 1;
                }
            }
        }
    }
}

/// Internal char draw that operates on an already-locked state.
fn draw_char_inner(state: &mut FbRenderState, x: u32, y: u32, ch: u8, fg: u32, bg: u32) {
    let base = write_target(state);
    let pitch = state.info.pitch;
    let bpp = state.info.bpp;
    let info = state.info;
    let max_x = info.width;
    let max_y = info.height;
    let fg_packed = pack_color(&info,
        ((fg >> 16) & 0xFF) as u8,
        ((fg >> 8) & 0xFF) as u8,
        (fg & 0xFF) as u8);
    let bg_packed = pack_color(&info,
        ((bg >> 16) & 0xFF) as u8,
        ((bg >> 8) & 0xFF) as u8,
        (bg & 0xFF) as u8);
    let glyph = font::glyph(ch);

    for row in 0..CHAR_HEIGHT {
        let bits = glyph[row as usize];
        for col in 0..CHAR_WIDTH {
            let px = x + col;
            let py = y + row;
            if px >= max_x || py >= max_y {
                continue;
            }
            let on = (bits >> (7 - col)) & 1 != 0;
            let color = if on { fg_packed } else { bg_packed };
            unsafe { write_pixel(base, pitch, bpp, px, py, color); }
        }
    }
    CHARS_DRAWN.fetch_add(1, Ordering::Relaxed);
}

/// Scroll console up by one text row.
fn fb_scroll_one(state: &mut FbRenderState) {
    let pitch = state.info.pitch as usize;
    let char_h = CHAR_HEIGHT as usize;
    let line_bytes = char_h * pitch;
    let total_bytes = state.info.height as usize * pitch;
    let base = write_target(state);

    if line_bytes >= total_bytes {
        return;
    }

    unsafe {
        core::ptr::copy(base.add(line_bytes), base, total_bytes - line_bytes);
        core::ptr::write_bytes(base.add(total_bytes - line_bytes), 0, line_bytes);
    }
    SCROLLS.fetch_add(1, Ordering::Relaxed);
}

/// Print a string to the framebuffer console.
pub fn fb_puts(s: &str) {
    for ch in s.bytes() {
        fb_putc(ch);
    }
}

/// Print a string followed by newline to the framebuffer console.
pub fn fb_println(s: &str) {
    fb_puts(s);
    fb_putc(b'\n');
}

/// Set console foreground from ANSI SGR color code.
pub fn set_ansi_fg(code: u32) {
    let mut state = STATE.lock();
    match code {
        0 => {
            state.fg = DEFAULT_FG;
            state.bg = DEFAULT_BG;
            state.bold = false;
        }
        1 => { state.bold = true; }
        22 => { state.bold = false; }
        30..=37 => { state.fg = ANSI_COLORS[(code - 30) as usize]; }
        40..=47 => { state.bg = ANSI_COLORS[(code - 40) as usize]; }
        90..=97 => { state.fg = ANSI_COLORS[(code - 90 + 8) as usize]; }
        100..=107 => { state.bg = ANSI_COLORS[(code - 100 + 8) as usize]; }
        _ => {}
    }
}

/// Get current cursor position in character coordinates.
pub fn cursor_pos() -> (u32, u32) {
    let state = STATE.lock();
    (state.col, state.row)
}

/// Set cursor position.
pub fn set_cursor_pos(col: u32, row: u32) {
    let mut state = STATE.lock();
    if col < state.cols {
        state.col = col;
    }
    if row < state.rows {
        state.row = row;
    }
}

// ---------------------------------------------------------------------------
// Resolution management
// ---------------------------------------------------------------------------

/// Configure the active framebuffer.
pub fn set_framebuffer(info: FramebufferInfo) {
    let mut state = STATE.lock();
    state.info = info;
    state.cols = if info.width > 0 { info.width / CHAR_WIDTH } else { 0 };
    state.rows = if info.height > 0 { info.height / CHAR_HEIGHT } else { 0 };
    state.col = 0;
    state.row = 0;
    state.fg = DEFAULT_FG;
    state.bg = DEFAULT_BG;
    state.bold = false;
    FB_ACTIVE.store(true, Ordering::SeqCst);
}

/// Get the current resolution as (width, height).
pub fn get_resolution() -> (u32, u32) {
    let state = STATE.lock();
    (state.info.width, state.info.height)
}

/// Check if framebuffer rendering is active.
pub fn is_active() -> bool {
    FB_ACTIVE.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Double buffering
// ---------------------------------------------------------------------------

/// Enable double buffering by allocating a back-buffer on the heap.
pub fn enable_double_buffer() {
    let mut state = STATE.lock();
    let size = state.info.pitch as usize * state.info.height as usize;
    if size == 0 {
        return;
    }
    let mut buf = Vec::new();
    buf.resize(size, 0u8);
    state.back_buffer = buf;
    state.double_buffered = true;
}

/// Swap the back buffer to the front (copy to framebuffer).
pub fn swap_buffers() {
    let state = STATE.lock();
    if !state.double_buffered || state.back_buffer.is_empty() {
        return;
    }
    let size = state.info.pitch as usize * state.info.height as usize;
    let dst = state.info.addr as *mut u8;
    let src = state.back_buffer.as_ptr();
    unsafe {
        core::ptr::copy_nonoverlapping(src, dst, size);
    }
    SWAP_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Check if double buffering is enabled.
pub fn is_double_buffered() -> bool {
    let state = STATE.lock();
    state.double_buffered
}

// ---------------------------------------------------------------------------
// Init & info
// ---------------------------------------------------------------------------

/// Initialize the framebuffer rendering engine.
pub fn init() {
    FB_ACTIVE.store(false, Ordering::SeqCst);
    PIXELS_DRAWN.store(0, Ordering::SeqCst);
    RECTS_DRAWN.store(0, Ordering::SeqCst);
    CHARS_DRAWN.store(0, Ordering::SeqCst);
    SCROLLS.store(0, Ordering::SeqCst);
    SWAP_COUNT.store(0, Ordering::SeqCst);
}

/// Return a human-readable info string.
pub fn fb_render_info() -> String {
    let state = STATE.lock();
    let active = FB_ACTIVE.load(Ordering::Relaxed);
    let dbl = state.double_buffered;
    format!(
        "Framebuffer Render Engine:\n  Active: {}\n  Resolution: {}x{}\n  \
         Pitch: {} bytes/row\n  BPP: {}\n  Text grid: {}x{} chars\n  \
         Double buffered: {}\n  Channel shifts: R={} G={} B={}",
        active,
        state.info.width, state.info.height,
        state.info.pitch, state.info.bpp,
        state.cols, state.rows,
        dbl,
        state.info.red_shift, state.info.green_shift, state.info.blue_shift,
    )
}

/// Return rendering statistics.
pub fn fb_render_stats() -> String {
    let px = PIXELS_DRAWN.load(Ordering::Relaxed);
    let rx = RECTS_DRAWN.load(Ordering::Relaxed);
    let ch = CHARS_DRAWN.load(Ordering::Relaxed);
    let sc = SCROLLS.load(Ordering::Relaxed);
    let sw = SWAP_COUNT.load(Ordering::Relaxed);
    format!(
        "FB Render Stats:\n  Pixels drawn: {}\n  Rectangles: {}\n  \
         Characters: {}\n  Scrolls: {}\n  Buffer swaps: {}",
        px, rx, ch, sc, sw,
    )
}
