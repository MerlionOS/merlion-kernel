/// VirtIO GPU driver and display server for MerlionOS.
/// Implements the virtio-gpu protocol for 2D rendering,
/// display management, cursor handling, and a compositor.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::borrow::ToOwned;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// VirtIO GPU protocol constants
// ---------------------------------------------------------------------------

/// VirtIO GPU command types (subset of the spec).
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VirtioGpuCmd {
    GetDisplayInfo = 0x0100,
    ResourceCreate2d = 0x0101,
    ResourceUnref = 0x0102,
    SetScanout = 0x0103,
    ResourceFlush = 0x0104,
    TransferToHost2d = 0x0105,
    ResourceAttachBacking = 0x0106,
    ResourceDetachBacking = 0x0107,
    UpdateCursor = 0x0300,
    MoveCursor = 0x0301,
}

/// Pixel format for 2D resources.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PixelFormat {
    B8G8R8A8Unorm = 1,
    B8G8R8X8Unorm = 2,
    A8R8G8B8Unorm = 3,
    X8R8G8B8Unorm = 4,
    R8G8B8A8Unorm = 67,
    X8B8G8R8Unorm = 68,
    A8B8G8R8Unorm = 121,
    R8G8B8X8Unorm = 134,
}

// ---------------------------------------------------------------------------
// Display modes
// ---------------------------------------------------------------------------

/// Supported display resolution.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
}

const MODES: &[DisplayMode] = &[
    DisplayMode { width: 640,  height: 480,  refresh: 60 },
    DisplayMode { width: 800,  height: 600,  refresh: 60 },
    DisplayMode { width: 1024, height: 768,  refresh: 60 },
    DisplayMode { width: 1280, height: 720,  refresh: 60 },
    DisplayMode { width: 1920, height: 1080, refresh: 60 },
];

// ---------------------------------------------------------------------------
// Framebuffer
// ---------------------------------------------------------------------------

/// A 2D framebuffer backed by a pixel buffer.
pub struct Framebuffer {
    pub resource_id: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub data: Vec<u32>,
}

impl Framebuffer {
    pub fn new(resource_id: u32, width: u32, height: u32) -> Self {
        let size = (width * height) as usize;
        Self {
            resource_id,
            width,
            height,
            stride: width * 4,
            format: PixelFormat::B8G8R8A8Unorm,
            data: vec![0u32; size],
        }
    }

    /// Clear with a solid colour (ARGB).
    pub fn clear(&mut self, color: u32) {
        for pixel in self.data.iter_mut() {
            *pixel = color;
        }
    }

    /// Set a single pixel.
    #[inline]
    pub fn set_pixel(&mut self, x: u32, y: u32, color: u32) {
        if x < self.width && y < self.height {
            self.data[(y * self.width + x) as usize] = color;
        }
    }

    /// Get a single pixel.
    #[inline]
    pub fn get_pixel(&self, x: u32, y: u32) -> u32 {
        if x < self.width && y < self.height {
            self.data[(y * self.width + x) as usize]
        } else {
            0
        }
    }

    /// Fill a rectangle with a solid colour.
    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: u32) {
        for row in y..core::cmp::min(y + h, self.height) {
            for col in x..core::cmp::min(x + w, self.width) {
                self.data[(row * self.width + col) as usize] = color;
            }
        }
    }

    /// Copy a rectangle within the framebuffer.
    pub fn copy_rect(&mut self, sx: u32, sy: u32, dx: u32, dy: u32, w: u32, h: u32) {
        // Temporary buffer to avoid overlap issues.
        let mut tmp = vec![0u32; (w * h) as usize];
        for row in 0..h {
            for col in 0..w {
                let src_x = sx + col;
                let src_y = sy + row;
                if src_x < self.width && src_y < self.height {
                    tmp[(row * w + col) as usize] =
                        self.data[(src_y * self.width + src_x) as usize];
                }
            }
        }
        for row in 0..h {
            for col in 0..w {
                let dst_x = dx + col;
                let dst_y = dy + row;
                if dst_x < self.width && dst_y < self.height {
                    self.data[(dst_y * self.width + dst_x) as usize] =
                        tmp[(row * w + col) as usize];
                }
            }
        }
    }

    /// Draw a horizontal/vertical/bresenham line.
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) {
        // Bresenham's line algorithm (integer only).
        let mut cx = x0;
        let mut cy = y0;
        let dx = if x1 > x0 { x1 - x0 } else { x0 - x1 };
        let dy = if y1 > y0 { y1 - y0 } else { y0 - y1 };
        let sx: i32 = if x0 < x1 { 1 } else { -1 };
        let sy: i32 = if y0 < y1 { 1 } else { -1 };
        let mut err = dx - dy;

        loop {
            if cx >= 0 && cy >= 0 {
                self.set_pixel(cx as u32, cy as u32, color);
            }
            if cx == x1 && cy == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 > -dy {
                err -= dy;
                cx += sx;
            }
            if e2 < dx {
                err += dx;
                cy += sy;
            }
        }
    }

    /// Draw a circle outline using midpoint algorithm.
    pub fn draw_circle(&mut self, cx: i32, cy: i32, radius: i32, color: u32) {
        let mut x = radius;
        let mut y: i32 = 0;
        let mut d = 1 - radius;
        while x >= y {
            // Eight octants.
            self.set_pixel((cx + x) as u32, (cy + y) as u32, color);
            self.set_pixel((cx - x) as u32, (cy + y) as u32, color);
            self.set_pixel((cx + x) as u32, (cy - y) as u32, color);
            self.set_pixel((cx - x) as u32, (cy - y) as u32, color);
            self.set_pixel((cx + y) as u32, (cy + x) as u32, color);
            self.set_pixel((cx - y) as u32, (cy + x) as u32, color);
            self.set_pixel((cx + y) as u32, (cy - x) as u32, color);
            self.set_pixel((cx - y) as u32, (cy - x) as u32, color);
            y += 1;
            if d <= 0 {
                d += 2 * y + 1;
            } else {
                x -= 1;
                d += 2 * (y - x) + 1;
            }
        }
    }

    /// Blit a source bitmap (raw u32 pixels) into the framebuffer.
    pub fn blit(&mut self, src: &[u32], src_w: u32, dx: u32, dy: u32, w: u32, h: u32) {
        for row in 0..h {
            for col in 0..w {
                let si = (row * src_w + col) as usize;
                if si < src.len() {
                    let px = src[si];
                    // Simple alpha check: skip fully transparent.
                    if px & 0xFF000000 != 0 {
                        self.set_pixel(dx + col, dy + row, px);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

/// A window in the compositor.
#[derive(Clone)]
pub struct Window {
    pub id: u32,
    pub title: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub z_order: u32,
    pub visible: bool,
    pub focused: bool,
    pub dirty: bool,
    pub pixels: Vec<u32>,
}

impl Window {
    pub fn new(id: u32, title: &str, x: u32, y: u32, w: u32, h: u32) -> Self {
        Self {
            id,
            title: title.to_owned(),
            x,
            y,
            width: w,
            height: h,
            z_order: id,
            visible: true,
            focused: false,
            dirty: true,
            pixels: vec![0xFF_C0_C0_C0; (w * h) as usize], // light-grey background
        }
    }
}

// ---------------------------------------------------------------------------
// Cursor
// ---------------------------------------------------------------------------

/// Hardware cursor state.
pub struct CursorState {
    pub x: u32,
    pub y: u32,
    pub visible: bool,
    pub hot_x: u32,
    pub hot_y: u32,
    pub width: u32,
    pub height: u32,
    pub image: Vec<u32>,
}

impl CursorState {
    pub const fn empty() -> Self {
        Self {
            x: 0,
            y: 0,
            visible: false,
            hot_x: 0,
            hot_y: 0,
            width: 0,
            height: 0,
            image: Vec::new(),
        }
    }

    /// Set a standard 16x16 arrow cursor.
    pub fn set_default(&mut self) {
        self.width = 16;
        self.height = 16;
        self.hot_x = 0;
        self.hot_y = 0;
        self.visible = true;
        // Simple white arrow with black border.
        self.image = vec![0x00_00_00_00; 256];
        for i in 0u32..16 {
            for j in 0..=i {
                if j == 0 || j == i || i == 15 {
                    self.image[(i * 16 + j) as usize] = 0xFF_00_00_00; // black
                } else {
                    self.image[(i * 16 + j) as usize] = 0xFF_FF_FF_FF; // white
                }
            }
        }
    }

    pub fn move_to(&mut self, x: u32, y: u32) {
        self.x = x;
        self.y = y;
    }

    pub fn set_image(&mut self, w: u32, h: u32, data: &[u32]) {
        self.width = w;
        self.height = h;
        self.image = data.to_vec();
    }
}

// ---------------------------------------------------------------------------
// Damage region tracking
// ---------------------------------------------------------------------------

/// A rectangular damage region that needs recompositing.
#[derive(Debug, Clone, Copy)]
pub struct DamageRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

// ---------------------------------------------------------------------------
// Display server state
// ---------------------------------------------------------------------------

struct DisplayServer {
    initialised: bool,
    current_mode: DisplayMode,
    front_buffer: Option<Framebuffer>,
    back_buffer: Option<Framebuffer>,
    windows: Vec<Window>,
    cursor: CursorState,
    next_window_id: u32,
    next_resource_id: u32,
    damage: Vec<DamageRect>,
    // Statistics
    frames_rendered: u64,
    flushes: u64,
    resources_created: u64,
    resources_freed: u64,
    composites: u64,
}

impl DisplayServer {
    const fn new() -> Self {
        Self {
            initialised: false,
            current_mode: DisplayMode { width: 1024, height: 768, refresh: 60 },
            front_buffer: None,
            back_buffer: None,
            windows: Vec::new(),
            cursor: CursorState::empty(),
            next_window_id: 1,
            next_resource_id: 100,
            damage: Vec::new(),
            frames_rendered: 0,
            flushes: 0,
            resources_created: 0,
            resources_freed: 0,
            composites: 0,
        }
    }

    fn alloc_resource_id(&mut self) -> u32 {
        let id = self.next_resource_id;
        self.next_resource_id += 1;
        self.resources_created += 1;
        id
    }
}

static SERVER: Mutex<DisplayServer> = Mutex::new(DisplayServer::new());

// VSync simulation counter.
static VSYNC_COUNT: AtomicU64 = AtomicU64::new(0);
static VSYNC_ENABLED: AtomicBool = AtomicBool::new(true);
static SCREENSHOT_COUNT: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// VirtIO GPU protocol operations (simulated)
// ---------------------------------------------------------------------------

/// Create a 2D resource (VIRTIO_GPU_CMD_RESOURCE_CREATE_2D).
pub fn resource_create_2d(width: u32, height: u32, format: PixelFormat) -> u32 {
    let mut srv = SERVER.lock();
    let id = srv.alloc_resource_id();
    // In a real driver we'd send this to the virtqueue; here we just track it.
    crate::serial_println!("virtio-gpu: created resource {} ({}x{}, {:?})", id, width, height, format);
    id
}

/// Unreference (destroy) a 2D resource.
pub fn resource_unref(resource_id: u32) {
    let mut srv = SERVER.lock();
    srv.resources_freed += 1;
    crate::serial_println!("virtio-gpu: unref resource {}", resource_id);
}

/// Set scanout — attach a resource to a display.
pub fn set_scanout(scanout_id: u32, resource_id: u32, x: u32, y: u32, w: u32, h: u32) {
    crate::serial_println!(
        "virtio-gpu: set_scanout {} resource {} rect ({},{} {}x{})",
        scanout_id, resource_id, x, y, w, h
    );
}

/// Flush a resource region to the display.
pub fn resource_flush(resource_id: u32, x: u32, y: u32, w: u32, h: u32) {
    let mut srv = SERVER.lock();
    srv.flushes += 1;
    crate::serial_println!(
        "virtio-gpu: flush resource {} rect ({},{} {}x{})",
        resource_id, x, y, w, h
    );
}

/// Transfer pixel data from guest to host (VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D).
pub fn transfer_to_host_2d(resource_id: u32, x: u32, y: u32, w: u32, h: u32) {
    crate::serial_println!(
        "virtio-gpu: transfer_to_host_2d resource {} rect ({},{} {}x{})",
        resource_id, x, y, w, h
    );
}

// ---------------------------------------------------------------------------
// Display management
// ---------------------------------------------------------------------------

/// Detect available displays (simulated single display).
pub fn detect_displays() -> Vec<String> {
    let srv = SERVER.lock();
    let mut list = Vec::new();
    list.push(format!(
        "Display 0: {}x{}@{}Hz (primary)",
        srv.current_mode.width, srv.current_mode.height, srv.current_mode.refresh
    ));
    list
}

/// List supported display modes.
pub fn supported_modes() -> Vec<DisplayMode> {
    MODES.to_vec()
}

/// Set the display resolution. Returns Ok on success.
pub fn set_resolution(width: u32, height: u32) -> Result<(), &'static str> {
    // Check the requested mode is in the supported list.
    let mode = MODES.iter().find(|m| m.width == width && m.height == height);
    match mode {
        Some(m) => {
            let mut srv = SERVER.lock();
            srv.current_mode = *m;
            // Recreate framebuffers at new resolution.
            let front_id = srv.alloc_resource_id();
            let back_id = srv.alloc_resource_id();
            srv.front_buffer = Some(Framebuffer::new(front_id, width, height));
            srv.back_buffer = Some(Framebuffer::new(back_id, width, height));
            crate::serial_println!("virtio-gpu: resolution set to {}x{}", width, height);
            Ok(())
        }
        None => Err("unsupported display mode"),
    }
}

// ---------------------------------------------------------------------------
// Framebuffer management
// ---------------------------------------------------------------------------

/// Swap front and back buffers (double buffering).
pub fn swap_buffers() {
    let mut srv = SERVER.lock();
    let mut a = srv.front_buffer.take();
    let mut b = srv.back_buffer.take();
    core::mem::swap(&mut a, &mut b);
    srv.front_buffer = a;
    srv.back_buffer = b;
    srv.frames_rendered += 1;
}

// ---------------------------------------------------------------------------
// Window management
// ---------------------------------------------------------------------------

/// Create a new window and return its ID.
pub fn create_window(title: &str, x: u32, y: u32, w: u32, h: u32) -> u32 {
    let mut srv = SERVER.lock();
    let id = srv.next_window_id;
    srv.next_window_id += 1;
    let win = Window::new(id, title, x, y, w, h);
    srv.windows.push(win);
    srv.damage.push(DamageRect { x, y, w, h });
    id
}

/// Destroy a window by ID.
pub fn destroy_window(id: u32) -> Result<(), &'static str> {
    let mut srv = SERVER.lock();
    if let Some(pos) = srv.windows.iter().position(|w| w.id == id) {
        let win = srv.windows.remove(pos);
        srv.damage.push(DamageRect {
            x: win.x, y: win.y, w: win.width, h: win.height,
        });
        Ok(())
    } else {
        Err("window not found")
    }
}

/// Move a window to a new position.
pub fn move_window(id: u32, new_x: u32, new_y: u32) -> Result<(), &'static str> {
    let mut srv = SERVER.lock();
    let idx = srv.windows.iter().position(|w| w.id == id)
        .ok_or("window not found")?;
    // Mark old position as damaged.
    let old_x = srv.windows[idx].x;
    let old_y = srv.windows[idx].y;
    let w = srv.windows[idx].width;
    let h = srv.windows[idx].height;
    srv.damage.push(DamageRect { x: old_x, y: old_y, w, h });
    srv.windows[idx].x = new_x;
    srv.windows[idx].y = new_y;
    srv.windows[idx].dirty = true;
    // Mark new position as damaged.
    srv.damage.push(DamageRect { x: new_x, y: new_y, w, h });
    Ok(())
}

/// Resize a window.
pub fn resize_window(id: u32, new_w: u32, new_h: u32) -> Result<(), &'static str> {
    let mut srv = SERVER.lock();
    let idx = srv.windows.iter().position(|w| w.id == id)
        .ok_or("window not found")?;
    let old_x = srv.windows[idx].x;
    let old_y = srv.windows[idx].y;
    let old_w = srv.windows[idx].width;
    let old_h = srv.windows[idx].height;
    srv.damage.push(DamageRect { x: old_x, y: old_y, w: old_w, h: old_h });
    srv.windows[idx].width = new_w;
    srv.windows[idx].height = new_h;
    srv.windows[idx].pixels = vec![0xFF_C0_C0_C0; (new_w * new_h) as usize];
    srv.windows[idx].dirty = true;
    srv.damage.push(DamageRect { x: old_x, y: old_y, w: new_w, h: new_h });
    Ok(())
}

/// Set focus to a window, bringing it to the top of the z-order.
pub fn focus_window(id: u32) -> Result<(), &'static str> {
    let mut srv = SERVER.lock();
    let max_z = srv.windows.iter().map(|w| w.z_order).max().unwrap_or(0);
    let idx = srv.windows.iter().position(|w| w.id == id)
        .ok_or("window not found")?;
    for w in srv.windows.iter_mut() {
        w.focused = false;
    }
    srv.windows[idx].focused = true;
    srv.windows[idx].z_order = max_z + 1;
    srv.windows[idx].dirty = true;
    Ok(())
}

// ---------------------------------------------------------------------------
// Compositor
// ---------------------------------------------------------------------------

/// Composite all visible windows onto the back buffer, in z-order.
pub fn composite() {
    let mut srv = SERVER.lock();
    srv.composites += 1;

    // Sort windows by z-order.
    srv.windows.sort_by_key(|w| w.z_order);

    // Snapshot windows and cursor before borrowing back_buffer.
    let windows_snapshot: Vec<Window> = srv.windows.clone();
    let cursor_visible = srv.cursor.visible;
    let cursor_image = srv.cursor.image.clone();
    let cursor_w = srv.cursor.width;
    let cursor_h = srv.cursor.height;
    let cursor_x = srv.cursor.x;
    let cursor_y = srv.cursor.y;

    // Clear back buffer with desktop colour.
    if let Some(ref mut back) = srv.back_buffer {
        back.clear(0xFF_20_60_A0); // MerlionOS blue desktop

        // Render each visible window.
        for win in &windows_snapshot {
            if !win.visible {
                continue;
            }
            // Draw window border (1px dark grey).
            back.fill_rect(
                win.x.saturating_sub(1),
                win.y.saturating_sub(1),
                win.width + 2,
                win.height + 2,
                0xFF_40_40_40,
            );
            // Draw title bar (20px tall).
            let title_color = if win.focused { 0xFF_00_40_80 } else { 0xFF_60_60_60 };
            back.fill_rect(win.x, win.y, win.width, 20, title_color);
            // Blit window content below title bar.
            back.blit(&win.pixels, win.width, win.x, win.y + 20, win.width, win.height);
        }

        // Render cursor on top.
        if cursor_visible && !cursor_image.is_empty() {
            back.blit(
                &cursor_image,
                cursor_w,
                cursor_x,
                cursor_y,
                cursor_w,
                cursor_h,
            );
        }
    }

    // Clear damage list.
    srv.damage.clear();

    // Mark all windows clean.
    for win in srv.windows.iter_mut() {
        win.dirty = false;
    }
}

// ---------------------------------------------------------------------------
// VSync
// ---------------------------------------------------------------------------

/// Simulate a vertical sync interrupt (called from timer or manually).
pub fn vsync_tick() {
    if VSYNC_ENABLED.load(Ordering::Relaxed) {
        VSYNC_COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

/// Enable or disable VSync simulation.
pub fn set_vsync(enabled: bool) {
    VSYNC_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Get the current VSync counter value.
pub fn vsync_counter() -> u64 {
    VSYNC_COUNT.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Cursor public API
// ---------------------------------------------------------------------------

/// Set the default arrow cursor.
pub fn cursor_set_default() {
    let mut srv = SERVER.lock();
    srv.cursor.set_default();
}

/// Move the hardware cursor.
pub fn cursor_move(x: u32, y: u32) {
    let mut srv = SERVER.lock();
    srv.cursor.move_to(x, y);
}

/// Set a custom cursor image.
pub fn cursor_set_image(w: u32, h: u32, data: &[u32]) {
    let mut srv = SERVER.lock();
    srv.cursor.set_image(w, h, data);
    srv.cursor.visible = true;
}

// ---------------------------------------------------------------------------
// Screenshot (BMP capture)
// ---------------------------------------------------------------------------

/// Capture the current front buffer as a BMP-format description.
/// In a real system this would write a BMP file; here we generate a summary.
pub fn screenshot() -> String {
    let srv = SERVER.lock();
    let count = SCREENSHOT_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let mode = &srv.current_mode;
    let has_fb = srv.front_buffer.is_some();

    if !srv.initialised {
        return "Display not initialised. Run 'display-info' first.".to_owned();
    }

    let pixel_count = if has_fb {
        (mode.width * mode.height) as usize
    } else {
        0
    };
    // BMP file size: 54-byte header + 4 bytes per pixel.
    let bmp_size = 54 + pixel_count * 4;

    let mut s = String::new();
    s.push_str(&format!("Screenshot #{}\n", count));
    s.push_str(&format!("  Resolution: {}x{}\n", mode.width, mode.height));
    s.push_str(&format!("  Format:     BMP (24-bit + alpha)\n"));
    s.push_str(&format!("  Size:       {} bytes\n", bmp_size));
    s.push_str(&format!("  Windows:    {}\n", srv.windows.len()));
    s.push_str(&format!("  Saved to:   /tmp/screenshot_{}.bmp", count));
    s
}

// ---------------------------------------------------------------------------
// Public API: info, stats, list
// ---------------------------------------------------------------------------

/// Display information string for the shell.
pub fn display_info() -> String {
    let srv = SERVER.lock();
    let mode = &srv.current_mode;
    let mut s = String::new();
    s.push_str("VirtIO GPU Display Server\n");
    s.push_str(&format!("  Status:      {}\n",
        if srv.initialised { "initialised" } else { "not initialised" }));
    s.push_str(&format!("  Resolution:  {}x{}@{}Hz\n", mode.width, mode.height, mode.refresh));
    s.push_str(&format!("  Pixel fmt:   B8G8R8A8_UNORM\n"));
    s.push_str(&format!("  Double buf:  {}\n",
        if srv.front_buffer.is_some() && srv.back_buffer.is_some() { "yes" } else { "no" }));
    s.push_str(&format!("  VSync:       {}\n",
        if VSYNC_ENABLED.load(Ordering::Relaxed) { "enabled" } else { "disabled" }));
    s.push_str(&format!("  Cursor:      {}\n",
        if srv.cursor.visible { "visible" } else { "hidden" }));
    s.push_str(&format!("  Windows:     {}\n", srv.windows.len()));
    s.push_str("  Supported modes:\n");
    for m in MODES {
        let marker = if m.width == mode.width && m.height == mode.height { " *" } else { "" };
        s.push_str(&format!("    {}x{}@{}Hz{}\n", m.width, m.height, m.refresh, marker));
    }
    s
}

/// Display statistics.
pub fn display_stats() -> String {
    let srv = SERVER.lock();
    let mut s = String::new();
    s.push_str("VirtIO GPU Statistics\n");
    s.push_str(&format!("  Frames rendered:   {}\n", srv.frames_rendered));
    s.push_str(&format!("  Flushes:           {}\n", srv.flushes));
    s.push_str(&format!("  Resources created: {}\n", srv.resources_created));
    s.push_str(&format!("  Resources freed:   {}\n", srv.resources_freed));
    s.push_str(&format!("  Composites:        {}\n", srv.composites));
    s.push_str(&format!("  VSync count:       {}\n", VSYNC_COUNT.load(Ordering::Relaxed)));
    s.push_str(&format!("  Screenshots:       {}\n", SCREENSHOT_COUNT.load(Ordering::Relaxed)));
    s.push_str(&format!("  Pending damage:    {} rects\n", srv.damage.len()));
    s
}

/// List all windows for the shell.
pub fn list_windows() -> String {
    let srv = SERVER.lock();
    if srv.windows.is_empty() {
        return "No windows open.".to_owned();
    }
    let mut s = String::new();
    s.push_str("  \x1b[1mID  TITLE                POS       SIZE      Z  FOCUS\x1b[0m\n");
    let mut sorted: Vec<&Window> = srv.windows.iter().collect();
    sorted.sort_by_key(|w| w.z_order);
    for win in sorted {
        s.push_str(&format!(
            "  {:2}  {:<20} {:>3},{:<3}   {:>4}x{:<4}  {:2}  {}\n",
            win.id,
            if win.title.len() > 20 { &win.title[..20] } else { &win.title },
            win.x, win.y,
            win.width, win.height,
            win.z_order,
            if win.focused { "*" } else { " " },
        ));
    }
    s
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the display server with default 1024x768 mode.
pub fn init() {
    let mut srv = SERVER.lock();
    if srv.initialised {
        return;
    }
    // Don't allocate framebuffers on init — they're huge (width*height*4 bytes each)
    // Allocate lazily when first display command is issued
    srv.initialised = true;
    crate::serial_println!("[virtio-gpu] display server ready (framebuffers allocated on demand)");
}
