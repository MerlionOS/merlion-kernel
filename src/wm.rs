/// Window manager for MerlionOS — renders rectangular windows on the pixel
/// framebuffer.  Maintains a stack of windows (back-to-front) with title bars,
/// borders, close buttons, and per-window content buffers.
///
/// Uses `crate::fbconsole::FbInfo` for framebuffer access.  When the
/// framebuffer console is not active, falls back to a simulated 640x480
/// off-screen buffer so the data structures remain exercisable.

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use crate::font;
use crate::fbconsole;
use spin::Mutex;

/// Title bar height in pixels.
const TITLE_BAR_HEIGHT: usize = 20;
/// Border thickness in pixels.
const BORDER_WIDTH: usize = 1;
/// Desktop background colour (dark teal).
const DEFAULT_BG_COLOR: u32 = 0x00335566;
/// Title bar colour for the focused window.
const FOCUSED_TITLE_COLOR: u32 = 0x003366AA;
/// Title bar colour for unfocused windows.
const UNFOCUSED_TITLE_COLOR: u32 = 0x00666666;
/// Border colour.
const BORDER_COLOR: u32 = 0x00222222;
/// Content area fill colour.
const CONTENT_BG_COLOR: u32 = 0x00FFFFFF;
/// Title text colour.
const TITLE_TEXT_COLOR: u32 = 0x00FFFFFF;
/// Close-button `[X]` colour.
const CLOSE_BUTTON_COLOR: u32 = 0x00FF4444;
/// Fallback resolution when no real framebuffer is available.
const FALLBACK_WIDTH: u32 = 640;
const FALLBACK_HEIGHT: u32 = 480;
const FALLBACK_BPP: u8 = 4;

// ── Window ──────────────────────────────────────────────────────────────────

/// A single rectangular window managed by the WM.
pub struct Window {
    /// Top-left X position (pixels).
    pub x: usize,
    /// Top-left Y position (pixels).
    pub y: usize,
    /// Total width including borders (pixels).
    pub width: usize,
    /// Total height including title bar and borders (pixels).
    pub height: usize,
    /// Title shown in the title bar.
    pub title: String,
    /// Whether the window is drawn during compositing.
    pub visible: bool,
    /// Whether this window currently has input focus.
    pub focused: bool,
    /// Raw pixel buffer for the client area (row-major, 4 bytes/pixel).
    pub content_buf: Vec<u8>,
}

impl Window {
    /// Create a window with its content buffer pre-filled to `CONTENT_BG_COLOR`.
    fn new(title: &str, x: usize, y: usize, width: usize, height: usize) -> Self {
        let cw = content_width(width);
        let ch = content_height(height);
        let pixels = cw * ch;
        let mut buf = vec![0u8; pixels * 4];
        for i in 0..pixels {
            let o = i * 4;
            buf[o]     = (CONTENT_BG_COLOR & 0xFF) as u8;
            buf[o + 1] = ((CONTENT_BG_COLOR >> 8) & 0xFF) as u8;
            buf[o + 2] = ((CONTENT_BG_COLOR >> 16) & 0xFF) as u8;
            buf[o + 3] = 0;
        }
        Self { x, y, width, height, title: String::from(title),
               visible: true, focused: false, content_buf: buf }
    }
}

/// Usable content width inside a window.
#[inline]
fn content_width(w: usize) -> usize {
    w.saturating_sub(2 * BORDER_WIDTH)
}

/// Usable content height inside a window.
#[inline]
fn content_height(h: usize) -> usize {
    h.saturating_sub(TITLE_BAR_HEIGHT + 2 * BORDER_WIDTH)
}

// ── WindowManager ───────────────────────────────────────────────────────────

/// Global window-manager state protected by a `Mutex`.
pub struct WindowManager {
    /// Window list ordered back-to-front (last element is topmost).
    pub windows: Vec<Window>,
    /// Desktop background colour.
    pub background_color: u32,
    /// Simulated framebuffer used when no hardware FB is available.
    sim_buf: Vec<u8>,
}

impl WindowManager {
    const fn new() -> Self {
        Self { windows: Vec::new(), background_color: DEFAULT_BG_COLOR, sim_buf: Vec::new() }
    }
}

/// The global WM singleton.
pub static WM: Mutex<WindowManager> = Mutex::new(WindowManager::new());

// ── Framebuffer helpers ─────────────────────────────────────────────────────

/// Cached `FbInfo` set at boot so the WM can access the framebuffer without
/// reaching into `FbConsole`'s private fields.
static FB_INFO_CACHE: Mutex<Option<fbconsole::FbInfo>> = Mutex::new(None);

/// Store a copy of `FbInfo` for the WM.  Call once after fbconsole init.
pub fn cache_fb_info(info: fbconsole::FbInfo) {
    *FB_INFO_CACHE.lock() = Some(info);
}

/// Resolve the active framebuffer, falling back to a simulated 640x480 buffer.
fn resolve_fb(wm: &mut WindowManager) -> fbconsole::FbInfo {
    if let Some(info) = *FB_INFO_CACHE.lock() {
        return info;
    }
    // Fallback: allocate / reuse a simulated buffer.
    let total = FALLBACK_WIDTH as usize * FALLBACK_HEIGHT as usize * FALLBACK_BPP as usize;
    if wm.sim_buf.len() != total {
        wm.sim_buf = vec![0u8; total];
    }
    fbconsole::FbInfo {
        addr: wm.sim_buf.as_mut_ptr() as u64,
        width: FALLBACK_WIDTH,
        height: FALLBACK_HEIGHT,
        stride: FALLBACK_WIDTH * FALLBACK_BPP as u32,
        bpp: FALLBACK_BPP,
    }
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Create a new window and return its ID (index in the window list).
/// The window is visible and auto-focused.
pub fn create_window(title: &str, x: usize, y: usize, w: usize, h: usize) -> usize {
    let mut wm = WM.lock();
    wm.windows.push(Window::new(title, x, y, w, h));
    let id = wm.windows.len() - 1;
    focus_window_inner(&mut wm, id);
    id
}

/// Close (remove) a window by ID.  Higher IDs shift down by one.
pub fn close_window(id: usize) {
    let mut wm = WM.lock();
    if id < wm.windows.len() {
        wm.windows.remove(id);
    }
}

/// Move a window to a new screen position.
pub fn move_window(id: usize, new_x: usize, new_y: usize) {
    let mut wm = WM.lock();
    if let Some(win) = wm.windows.get_mut(id) {
        win.x = new_x;
        win.y = new_y;
    }
}

/// Bring a window to the front and give it focus.
pub fn focus_window(id: usize) {
    let mut wm = WM.lock();
    focus_window_inner(&mut wm, id);
}

/// Inner focus helper — operates on an already-locked WM.
fn focus_window_inner(wm: &mut WindowManager, id: usize) {
    if id >= wm.windows.len() { return; }
    for win in wm.windows.iter_mut() { win.focused = false; }
    let mut win = wm.windows.remove(id);
    win.focused = true;
    wm.windows.push(win);
}

/// Return a snapshot of all windows as `(id, title, visible, focused)`.
pub fn list_windows() -> Vec<(usize, String, bool, bool)> {
    let wm = WM.lock();
    wm.windows.iter().enumerate()
        .map(|(i, w)| (i, w.title.clone(), w.visible, w.focused))
        .collect()
}

// ── Rendering ───────────────────────────────────────────────────────────────

/// Composite every visible window onto the framebuffer.
///
/// 1. Fill background.
/// 2. For each visible window (back-to-front):
///    - Draw 1px border
///    - Draw title bar (coloured by focus state)
///    - Draw close button `[X]` in the title bar
///    - Blit window content area
pub fn render_all() {
    let mut wm = WM.lock();
    let fb = resolve_fb(&mut wm);

    // Desktop background.
    draw_rect(&fb, 0, 0, fb.width as usize, fb.height as usize, wm.background_color);

    // Windows back-to-front.
    let count = wm.windows.len();
    for i in 0..count {
        let win = &wm.windows[i];
        if !win.visible { continue; }

        let (wx, wy, ww, wh) = (win.x, win.y, win.width, win.height);
        let focused = win.focused;

        // Border — drawn as a filled rect behind title bar + content.
        draw_rect(&fb, wx, wy, ww, wh, BORDER_COLOR);

        // Title bar.
        let tb_color = if focused { FOCUSED_TITLE_COLOR } else { UNFOCUSED_TITLE_COLOR };
        draw_rect(&fb, wx + BORDER_WIDTH, wy + BORDER_WIDTH,
                  ww.saturating_sub(2 * BORDER_WIDTH), TITLE_BAR_HEIGHT, tb_color);

        // Title text — left-aligned, vertically centred.
        let text_y = wy + BORDER_WIDTH + TITLE_BAR_HEIGHT.saturating_sub(font::CHAR_HEIGHT) / 2;
        draw_text(&fb, wx + BORDER_WIDTH + 4, text_y, &win.title, TITLE_TEXT_COLOR);

        // Close button [X] — right side of title bar.
        let close_x = (wx + ww).saturating_sub(BORDER_WIDTH + font::CHAR_WIDTH * 3 + 4);
        draw_text(&fb, close_x, text_y, "[X]", CLOSE_BUTTON_COLOR);

        // Content area — blit from the window's pixel buffer.
        let cx = wx + BORDER_WIDTH;
        let cy = wy + BORDER_WIDTH + TITLE_BAR_HEIGHT;
        let cw = content_width(ww);
        let ch = content_height(wh);
        blit_content(&fb, cx, cy, cw, ch, &win.content_buf);
    }
}

/// Blit a raw pixel buffer into the framebuffer at `(dx, dy)`.
/// Clips to screen bounds.
fn blit_content(fb: &fbconsole::FbInfo, dx: usize, dy: usize,
                w: usize, h: usize, buf: &[u8]) {
    let (fb_w, fb_h) = (fb.width as usize, fb.height as usize);
    let (stride, bpp) = (fb.stride as usize, fb.bpp as usize);

    for row in 0..h {
        let sy = dy + row;
        if sy >= fb_h { break; }
        for col in 0..w {
            let sx = dx + col;
            if sx >= fb_w { break; }
            let off = (row * w + col) * 4;
            if off + 3 >= buf.len() { return; }
            let pixel = u32::from_le_bytes([buf[off], buf[off+1], buf[off+2], buf[off+3]]);
            put_pixel(fb.addr, stride, bpp, sx, sy, pixel);
        }
    }
}

// ── Drawing primitives ──────────────────────────────────────────────────────

/// Draw a filled rectangle.  Pixels outside screen bounds are clipped.
pub fn draw_rect(fb: &fbconsole::FbInfo, x: usize, y: usize, w: usize, h: usize, color: u32) {
    let (fb_w, fb_h) = (fb.width as usize, fb.height as usize);
    let (stride, bpp) = (fb.stride as usize, fb.bpp as usize);
    let x_end = core::cmp::min(x + w, fb_w);
    let y_end = core::cmp::min(y + h, fb_h);
    for py in y..y_end {
        for px in x..x_end {
            put_pixel(fb.addr, stride, bpp, px, py, color);
        }
    }
}

/// Render an ASCII string using `crate::font` (8x16 glyphs).
/// Only foreground pixels are drawn (transparent background).
pub fn draw_text(fb: &fbconsole::FbInfo, x: usize, y: usize, text: &str, color: u32) {
    let (fb_w, fb_h) = (fb.width as usize, fb.height as usize);
    let (stride, bpp) = (fb.stride as usize, fb.bpp as usize);
    let mut cx = x;
    for byte in text.bytes() {
        if cx + font::CHAR_WIDTH > fb_w { break; }
        let glyph = font::glyph(byte);
        for gy in 0..font::CHAR_HEIGHT {
            let py = y + gy;
            if py >= fb_h { break; }
            let bits = glyph[gy];
            for gx in 0..font::CHAR_WIDTH {
                if bits & (0x80 >> gx) != 0 {
                    let px = cx + gx;
                    if px < fb_w {
                        put_pixel(fb.addr, stride, bpp, px, py, color);
                    }
                }
            }
        }
        cx += font::CHAR_WIDTH;
    }
}

/// Write a single 32-bit pixel to the framebuffer.
#[inline(always)]
fn put_pixel(addr: u64, stride: usize, bpp: usize, x: usize, y: usize, color: u32) {
    let offset = y * stride + x * bpp;
    unsafe {
        let ptr = (addr as *mut u8).add(offset);
        (ptr as *mut u32).write_volatile(color);
    }
}
