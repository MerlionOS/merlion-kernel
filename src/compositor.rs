/// Wayland-inspired window compositor for MerlionOS.
/// Manages windows with drag/resize/minimize, taskbar,
/// Alt+Tab switching, virtual desktops, and compositing.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

const MAX_WINDOWS: usize = 64;
const TITLE_BAR_HEIGHT: u32 = 20;
const BORDER_WIDTH: u32 = 1;
const TASKBAR_HEIGHT: u32 = 32;
const NUM_DESKTOPS: usize = 4;
const SNAP_THRESHOLD: i32 = 8;
const CLOSE_BTN_WIDTH: u32 = 20;
const MIN_BTN_WIDTH: u32 = 20;
const MAX_BTN_WIDTH: u32 = 20;
const DEFAULT_MIN_WIDTH: u32 = 100;
const DEFAULT_MIN_HEIGHT: u32 = 60;
const SCREEN_WIDTH: u32 = 1024;
const SCREEN_HEIGHT: u32 = 768;
const ALT_TAB_THUMB_W: u32 = 120;
const ALT_TAB_THUMB_H: u32 = 90;
const ALT_TAB_PADDING: u32 = 16;

// Default colours (ARGB)
const COLOR_DESKTOP_BG: u32 = 0xFF1A1A2E;
const COLOR_TITLE_FOCUSED: u32 = 0xFF3366AA;
const COLOR_TITLE_UNFOCUSED: u32 = 0xFF555555;
const COLOR_TITLE_TEXT: u32 = 0xFFFFFFFF;
const COLOR_BORDER: u32 = 0xFF222222;
const COLOR_CLOSE_BTN: u32 = 0xFFFF4444;
const COLOR_MIN_BTN: u32 = 0xFFFFBB33;
const COLOR_MAX_BTN: u32 = 0xFF33BB33;
const COLOR_TASKBAR_BG: u32 = 0xFF0D0D1A;
const COLOR_TASKBAR_FG: u32 = 0xFFCCCCCC;
const COLOR_TASKBAR_ACTIVE: u32 = 0xFF3366AA;
const COLOR_ALT_TAB_BG: u32 = 0xCC222222;
const COLOR_ALT_TAB_SEL: u32 = 0xFF4488CC;

// ── Counters ──────────────────────────────────────────────────────────────────

static NEXT_ID: AtomicU32 = AtomicU32::new(1);
static TOTAL_CREATED: AtomicU64 = AtomicU64::new(0);
static TOTAL_DESTROYED: AtomicU64 = AtomicU64::new(0);
static TOTAL_COMPOSITES: AtomicU64 = AtomicU64::new(0);
static DAMAGE_REDRAWS: AtomicU64 = AtomicU64::new(0);

// ── Window ────────────────────────────────────────────────────────────────────

/// A single managed window.
pub struct Window {
    pub id: u32,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub min_width: u32,
    pub min_height: u32,
    pub visible: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub focused: bool,
    pub z_order: u16,
    pub decorations: bool,
    pub pixels: Vec<u32>,
    pub dirty: bool,
    pub pid: u32,
    pub desktop: u8,
    // Saved geometry for restore-from-maximized
    saved_x: i32,
    saved_y: i32,
    saved_w: u32,
    saved_h: u32,
}

impl Window {
    fn new(id: u32, title: &str, x: i32, y: i32, width: u32, height: u32, pid: u32) -> Self {
        let pixel_count = (width as usize) * (height as usize);
        Self {
            id,
            title: String::from(title),
            x,
            y,
            width,
            height,
            min_width: DEFAULT_MIN_WIDTH,
            min_height: DEFAULT_MIN_HEIGHT,
            visible: true,
            minimized: false,
            maximized: false,
            focused: false,
            z_order: 0,
            decorations: true,
            pixels: vec![0xFF2A2A3E; pixel_count],
            dirty: true,
            pid,
            desktop: 0,
            saved_x: x,
            saved_y: y,
            saved_w: width,
            saved_h: height,
        }
    }

    /// Total height including decorations.
    fn total_height(&self) -> u32 {
        if self.decorations {
            self.height + TITLE_BAR_HEIGHT + BORDER_WIDTH * 2
        } else {
            self.height
        }
    }

    /// Total width including decorations.
    fn total_width(&self) -> u32 {
        if self.decorations {
            self.width + BORDER_WIDTH * 2
        } else {
            self.width
        }
    }

    /// Content area top-left in screen coordinates.
    fn content_x(&self) -> i32 {
        if self.decorations { self.x + BORDER_WIDTH as i32 } else { self.x }
    }

    fn content_y(&self) -> i32 {
        if self.decorations { self.y + TITLE_BAR_HEIGHT as i32 + BORDER_WIDTH as i32 } else { self.y }
    }

    /// Check if point is inside the title bar.
    fn in_title_bar(&self, px: i32, py: i32) -> bool {
        if !self.decorations { return false; }
        px >= self.x && px < self.x + self.total_width() as i32
            && py >= self.y && py < self.y + TITLE_BAR_HEIGHT as i32
    }

    /// Check if point is on the close button.
    fn in_close_button(&self, px: i32, py: i32) -> bool {
        if !self.decorations { return false; }
        let bx = self.x + self.total_width() as i32 - CLOSE_BTN_WIDTH as i32;
        px >= bx && px < bx + CLOSE_BTN_WIDTH as i32
            && py >= self.y && py < self.y + TITLE_BAR_HEIGHT as i32
    }

    /// Check if point is on the minimize button.
    fn in_minimize_button(&self, px: i32, py: i32) -> bool {
        if !self.decorations { return false; }
        let bx = self.x + self.total_width() as i32 - CLOSE_BTN_WIDTH as i32 - MIN_BTN_WIDTH as i32;
        px >= bx && px < bx + MIN_BTN_WIDTH as i32
            && py >= self.y && py < self.y + TITLE_BAR_HEIGHT as i32
    }

    /// Check if point is on the maximize button.
    fn in_maximize_button(&self, px: i32, py: i32) -> bool {
        if !self.decorations { return false; }
        let bx = self.x + self.total_width() as i32 - CLOSE_BTN_WIDTH as i32
            - MIN_BTN_WIDTH as i32 - MAX_BTN_WIDTH as i32;
        px >= bx && px < bx + MAX_BTN_WIDTH as i32
            && py >= self.y && py < self.y + TITLE_BAR_HEIGHT as i32
    }

    /// Check if point is on a resize edge. Returns (horizontal, vertical) indicators:
    /// -1 = left/top edge, 0 = not on edge, 1 = right/bottom edge
    fn on_resize_edge(&self, px: i32, py: i32) -> (i32, i32) {
        if !self.decorations { return (0, 0); }
        let grab = 4i32;
        let tw = self.total_width() as i32;
        let th = self.total_height() as i32;
        let mut h = 0i32;
        let mut v = 0i32;
        if px >= self.x - grab && px < self.x + grab { h = -1; }
        if px >= self.x + tw - grab && px < self.x + tw + grab { h = 1; }
        if py >= self.y - grab && py < self.y + grab { v = -1; }
        if py >= self.y + th - grab && py < self.y + th + grab { v = 1; }
        // Only count if we're within the window's bounding box (extended by grab)
        if px < self.x - grab || px >= self.x + tw + grab
            || py < self.y - grab || py >= self.y + th + grab
        {
            return (0, 0);
        }
        (h, v)
    }

    /// Check if a screen point is inside this window (including decorations).
    fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.total_width() as i32
            && py >= self.y && py < self.y + self.total_height() as i32
    }
}

// ── Damage Region ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct DamageRect {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
}

impl DamageRect {
    fn union(self, other: DamageRect) -> DamageRect {
        let x1 = if self.x < other.x { self.x } else { other.x };
        let y1 = if self.y < other.y { self.y } else { other.y };
        let r1 = self.x + self.w as i32;
        let r2 = other.x + other.w as i32;
        let b1 = self.y + self.h as i32;
        let b2 = other.y + other.h as i32;
        let x2 = if r1 > r2 { r1 } else { r2 };
        let y2 = if b1 > b2 { b1 } else { b2 };
        DamageRect {
            x: x1,
            y: y1,
            w: (x2 - x1) as u32,
            h: (y2 - y1) as u32,
        }
    }
}

// ── Interaction State ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum DragMode {
    None,
    Move,
    ResizeLeft,
    ResizeRight,
    ResizeTop,
    ResizeBottom,
    ResizeTopLeft,
    ResizeTopRight,
    ResizeBottomLeft,
    ResizeBottomRight,
}

struct InteractionState {
    drag_mode: DragMode,
    drag_window: u32,
    drag_start_x: i32,
    drag_start_y: i32,
    drag_win_x: i32,
    drag_win_y: i32,
    drag_win_w: u32,
    drag_win_h: u32,
    alt_tab_active: bool,
    alt_tab_index: usize,
}

impl InteractionState {
    const fn new() -> Self {
        Self {
            drag_mode: DragMode::None,
            drag_window: 0,
            drag_start_x: 0,
            drag_start_y: 0,
            drag_win_x: 0,
            drag_win_y: 0,
            drag_win_w: 0,
            drag_win_h: 0,
            alt_tab_active: false,
            alt_tab_index: 0,
        }
    }
}

// ── Compositor State ──────────────────────────────────────────────────────────

struct CompositorState {
    windows: Vec<Window>,
    active_desktop: u8,
    interaction: InteractionState,
    framebuffer: Vec<u32>,
    fb_width: u32,
    fb_height: u32,
    damage: Option<DamageRect>,
    taskbar_dirty: bool,
    initialized: bool,
}

impl CompositorState {
    const fn new() -> Self {
        Self {
            windows: Vec::new(),
            active_desktop: 0,
            interaction: InteractionState::new(),
            framebuffer: Vec::new(),
            fb_width: 0,
            fb_height: 0,
            damage: None,
            taskbar_dirty: true,
            initialized: false,
        }
    }
}

static STATE: Mutex<CompositorState> = Mutex::new(CompositorState::new());

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialize the compositor.
pub fn init() {
    let mut s = STATE.lock();
    s.fb_width = SCREEN_WIDTH;
    s.fb_height = SCREEN_HEIGHT;
    let total = (SCREEN_WIDTH as usize) * (SCREEN_HEIGHT as usize);
    s.framebuffer = vec![COLOR_DESKTOP_BG; total];
    s.initialized = true;
}

/// Create a new window. Returns the window id.
pub fn create_window(title: &str, width: u32, height: u32) -> u32 {
    create_window_ex(title, 40, 40, width, height, 0)
}

/// Create a window with full options.
pub fn create_window_ex(title: &str, x: i32, y: i32, width: u32, height: u32, pid: u32) -> u32 {
    let mut s = STATE.lock();
    if s.windows.len() >= MAX_WINDOWS {
        return 0;
    }
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let z = s.windows.len() as u16;
    let desktop = s.active_desktop;
    let mut win = Window::new(id, title, x, y, width, height, pid);
    win.z_order = z;
    win.desktop = desktop;
    // Unfocus all others, focus this one
    for w in s.windows.iter_mut() {
        w.focused = false;
    }
    win.focused = true;
    s.windows.push(win);
    add_full_damage(&mut s);
    TOTAL_CREATED.fetch_add(1, Ordering::SeqCst);
    id
}

/// Destroy a window by id.
pub fn destroy_window(id: u32) -> bool {
    let mut s = STATE.lock();
    if let Some(idx) = s.windows.iter().position(|w| w.id == id) {
        s.windows.remove(idx);
        recompute_z_order(&mut s.windows);
        add_full_damage(&mut s);
        TOTAL_DESTROYED.fetch_add(1, Ordering::SeqCst);
        // Focus the topmost window
        if let Some(top) = s.windows.last_mut() {
            top.focused = true;
        }
        true
    } else {
        false
    }
}

/// Move a window to new coordinates.
pub fn move_window(id: u32, x: i32, y: i32) {
    let mut s = STATE.lock();
    if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
        w.x = x;
        w.y = y;
        w.dirty = true;
        add_full_damage(&mut s);
    }
}

/// Resize a window.
pub fn resize_window(id: u32, width: u32, height: u32) {
    let mut s = STATE.lock();
    if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
        let nw = if width < w.min_width { w.min_width } else { width };
        let nh = if height < w.min_height { w.min_height } else { height };
        w.width = nw;
        w.height = nh;
        let count = (nw as usize) * (nh as usize);
        w.pixels.resize(count, 0xFF2A2A3E);
        w.dirty = true;
        add_full_damage(&mut s);
    }
}

/// Minimize a window.
pub fn minimize_window(id: u32) {
    let mut s = STATE.lock();
    if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
        w.minimized = true;
        w.visible = false;
        w.focused = false;
        w.dirty = true;
    }
    add_full_damage(&mut s);
    s.taskbar_dirty = true;
}

/// Maximize a window.
pub fn maximize_window(id: u32) {
    let mut s = STATE.lock();
    let fw = s.fb_width;
    let fh = s.fb_height;
    if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
        if w.maximized { return; }
        w.saved_x = w.x;
        w.saved_y = w.y;
        w.saved_w = w.width;
        w.saved_h = w.height;
        w.x = 0;
        w.y = 0;
        let usable_h = fh - TASKBAR_HEIGHT;
        w.width = if w.decorations { fw - BORDER_WIDTH * 2 } else { fw };
        w.height = if w.decorations {
            usable_h - TITLE_BAR_HEIGHT - BORDER_WIDTH * 2
        } else {
            usable_h
        };
        let count = (w.width as usize) * (w.height as usize);
        w.pixels.resize(count, 0xFF2A2A3E);
        w.maximized = true;
        w.dirty = true;
    }
    add_full_damage(&mut s);
}

/// Restore a window from maximized or minimized state.
pub fn restore_window(id: u32) {
    let mut s = STATE.lock();
    if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
        if w.maximized {
            w.x = w.saved_x;
            w.y = w.saved_y;
            w.width = w.saved_w;
            w.height = w.saved_h;
            let count = (w.width as usize) * (w.height as usize);
            w.pixels.resize(count, 0xFF2A2A3E);
            w.maximized = false;
        }
        if w.minimized {
            w.minimized = false;
            w.visible = true;
        }
        w.dirty = true;
    }
    add_full_damage(&mut s);
    s.taskbar_dirty = true;
}

/// Focus a window and raise it to the top.
pub fn focus_window(id: u32) {
    let mut s = STATE.lock();
    for w in s.windows.iter_mut() {
        w.focused = w.id == id;
    }
    raise_to_top(&mut s.windows, id);
    s.taskbar_dirty = true;
}

/// Raise a window to the top of the z-order.
pub fn raise_window(id: u32) {
    let mut s = STATE.lock();
    raise_to_top(&mut s.windows, id);
    add_full_damage(&mut s);
}

/// Lower a window to the bottom of the z-order.
pub fn lower_window(id: u32) {
    let mut s = STATE.lock();
    if let Some(idx) = s.windows.iter().position(|w| w.id == id) {
        let win = s.windows.remove(idx);
        s.windows.insert(0, win);
        recompute_z_order(&mut s.windows);
        add_full_damage(&mut s);
    }
}

/// Set window title.
pub fn set_title(id: u32, title: &str) {
    let mut s = STATE.lock();
    if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
        w.title = String::from(title);
        w.dirty = true;
    }
}

/// Switch to a virtual desktop (0..3).
pub fn switch_desktop(desktop: u8) {
    if desktop >= NUM_DESKTOPS as u8 {
        return;
    }
    let mut s = STATE.lock();
    s.active_desktop = desktop;
    // Update visibility
    for w in s.windows.iter_mut() {
        if w.desktop == desktop {
            if !w.minimized {
                w.visible = true;
            }
        } else {
            w.visible = false;
        }
    }
    add_full_damage(&mut s);
    s.taskbar_dirty = true;
}

/// Snap a window to the left half of the screen.
pub fn snap_left(id: u32) {
    let mut s = STATE.lock();
    let fw = s.fb_width;
    let fh = s.fb_height;
    if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
        w.x = 0;
        w.y = 0;
        let usable_h = fh - TASKBAR_HEIGHT;
        w.width = if w.decorations { fw / 2 - BORDER_WIDTH * 2 } else { fw / 2 };
        w.height = if w.decorations {
            usable_h - TITLE_BAR_HEIGHT - BORDER_WIDTH * 2
        } else {
            usable_h
        };
        let count = (w.width as usize) * (w.height as usize);
        w.pixels.resize(count, 0xFF2A2A3E);
        w.maximized = false;
        w.dirty = true;
    }
    add_full_damage(&mut s);
}

/// Snap a window to the right half of the screen.
pub fn snap_right(id: u32) {
    let mut s = STATE.lock();
    let fw = s.fb_width;
    let fh = s.fb_height;
    if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
        w.x = (fw / 2) as i32;
        w.y = 0;
        let usable_h = fh - TASKBAR_HEIGHT;
        w.width = if w.decorations { fw / 2 - BORDER_WIDTH * 2 } else { fw / 2 };
        w.height = if w.decorations {
            usable_h - TITLE_BAR_HEIGHT - BORDER_WIDTH * 2
        } else {
            usable_h
        };
        let count = (w.width as usize) * (w.height as usize);
        w.pixels.resize(count, 0xFF2A2A3E);
        w.maximized = false;
        w.dirty = true;
    }
    add_full_damage(&mut s);
}

// ── Mouse Interaction ─────────────────────────────────────────────────────────

/// Handle mouse button press.
pub fn on_mouse_down(mx: i32, my: i32) {
    let mut s = STATE.lock();
    // Check taskbar click first
    if my >= (s.fb_height - TASKBAR_HEIGHT) as i32 {
        handle_taskbar_click(&mut s, mx);
        return;
    }
    // Find topmost window under cursor (iterate back-to-front, last = top)
    let mut found_id = None;
    for w in s.windows.iter().rev() {
        if !w.visible || w.minimized { continue; }
        if w.desktop != s.active_desktop { continue; }
        if w.contains(mx, my) {
            found_id = Some(w.id);
            break;
        }
    }
    let id = match found_id {
        Some(id) => id,
        None => return,
    };
    // Focus and raise
    for w in s.windows.iter_mut() {
        w.focused = w.id == id;
    }
    raise_to_top(&mut s.windows, id);
    s.taskbar_dirty = true;

    let w = match s.windows.iter().find(|w| w.id == id) {
        Some(w) => w,
        None => return,
    };

    // Close button
    if w.in_close_button(mx, my) {
        let wid = w.id;
        drop(s);
        destroy_window(wid);
        return;
    }
    // Minimize button
    if w.in_minimize_button(mx, my) {
        let wid = w.id;
        drop(s);
        minimize_window(wid);
        return;
    }
    // Maximize / restore button
    if w.in_maximize_button(mx, my) {
        let wid = w.id;
        let is_max = w.maximized;
        drop(s);
        if is_max { restore_window(wid); } else { maximize_window(wid); }
        return;
    }
    // Drag title bar
    if w.in_title_bar(mx, my) {
        let wx = w.x;
        let wy = w.y;
        s.interaction.drag_mode = DragMode::Move;
        s.interaction.drag_window = id;
        s.interaction.drag_start_x = mx;
        s.interaction.drag_start_y = my;
        s.interaction.drag_win_x = wx;
        s.interaction.drag_win_y = wy;
        return;
    }
    // Resize edges
    let (rh, rv) = w.on_resize_edge(mx, my);
    if rh != 0 || rv != 0 {
        let wx = w.x;
        let wy = w.y;
        let ww = w.width;
        let wh = w.height;
        let mode = match (rh, rv) {
            (-1, 0) => DragMode::ResizeLeft,
            (1, 0) => DragMode::ResizeRight,
            (0, -1) => DragMode::ResizeTop,
            (0, 1) => DragMode::ResizeBottom,
            (-1, -1) => DragMode::ResizeTopLeft,
            (1, -1) => DragMode::ResizeTopRight,
            (-1, 1) => DragMode::ResizeBottomLeft,
            (1, 1) => DragMode::ResizeBottomRight,
            _ => DragMode::None,
        };
        s.interaction.drag_mode = mode;
        s.interaction.drag_window = id;
        s.interaction.drag_start_x = mx;
        s.interaction.drag_start_y = my;
        s.interaction.drag_win_x = wx;
        s.interaction.drag_win_y = wy;
        s.interaction.drag_win_w = ww;
        s.interaction.drag_win_h = wh;
    }
}

/// Handle mouse drag (move).
pub fn on_mouse_move(mx: i32, my: i32) {
    let mut s = STATE.lock();
    if s.interaction.drag_mode == DragMode::None { return; }
    let dx = mx - s.interaction.drag_start_x;
    let dy = my - s.interaction.drag_start_y;
    let id = s.interaction.drag_window;
    let mode = s.interaction.drag_mode;
    let dwx = s.interaction.drag_win_x;
    let dwy = s.interaction.drag_win_y;
    let dww = s.interaction.drag_win_w;
    let dwh = s.interaction.drag_win_h;

    match mode {
        DragMode::Move => {
            if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
                w.x = dwx + dx;
                w.y = dwy + dy;
                w.dirty = true;
            }
            add_full_damage(&mut s);
        }
        DragMode::ResizeRight => {
            if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
                let nw = (dww as i32 + dx).max(w.min_width as i32) as u32;
                w.width = nw;
                let count = (nw as usize) * (w.height as usize);
                w.pixels.resize(count, 0xFF2A2A3E);
                w.dirty = true;
            }
            add_full_damage(&mut s);
        }
        DragMode::ResizeBottom => {
            if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
                let nh = (dwh as i32 + dy).max(w.min_height as i32) as u32;
                w.height = nh;
                let count = (w.width as usize) * (nh as usize);
                w.pixels.resize(count, 0xFF2A2A3E);
                w.dirty = true;
            }
            add_full_damage(&mut s);
        }
        DragMode::ResizeLeft => {
            if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
                let nw = (dww as i32 - dx).max(w.min_width as i32) as u32;
                w.x = dwx + (dww as i32 - nw as i32);
                w.width = nw;
                let count = (nw as usize) * (w.height as usize);
                w.pixels.resize(count, 0xFF2A2A3E);
                w.dirty = true;
            }
            add_full_damage(&mut s);
        }
        DragMode::ResizeTop => {
            if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
                let nh = (dwh as i32 - dy).max(w.min_height as i32) as u32;
                w.y = dwy + (dwh as i32 - nh as i32);
                w.height = nh;
                let count = (w.width as usize) * (nh as usize);
                w.pixels.resize(count, 0xFF2A2A3E);
                w.dirty = true;
            }
            add_full_damage(&mut s);
        }
        DragMode::ResizeBottomRight => {
            if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
                let nw = (dww as i32 + dx).max(w.min_width as i32) as u32;
                let nh = (dwh as i32 + dy).max(w.min_height as i32) as u32;
                w.width = nw;
                w.height = nh;
                let count = (nw as usize) * (nh as usize);
                w.pixels.resize(count, 0xFF2A2A3E);
                w.dirty = true;
            }
            add_full_damage(&mut s);
        }
        DragMode::ResizeTopLeft => {
            if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
                let nw = (dww as i32 - dx).max(w.min_width as i32) as u32;
                let nh = (dwh as i32 - dy).max(w.min_height as i32) as u32;
                w.x = dwx + (dww as i32 - nw as i32);
                w.y = dwy + (dwh as i32 - nh as i32);
                w.width = nw;
                w.height = nh;
                let count = (nw as usize) * (nh as usize);
                w.pixels.resize(count, 0xFF2A2A3E);
                w.dirty = true;
            }
            add_full_damage(&mut s);
        }
        DragMode::ResizeTopRight => {
            if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
                let nw = (dww as i32 + dx).max(w.min_width as i32) as u32;
                let nh = (dwh as i32 - dy).max(w.min_height as i32) as u32;
                w.y = dwy + (dwh as i32 - nh as i32);
                w.width = nw;
                w.height = nh;
                let count = (nw as usize) * (nh as usize);
                w.pixels.resize(count, 0xFF2A2A3E);
                w.dirty = true;
            }
            add_full_damage(&mut s);
        }
        DragMode::ResizeBottomLeft => {
            if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
                let nw = (dww as i32 - dx).max(w.min_width as i32) as u32;
                let nh = (dwh as i32 + dy).max(w.min_height as i32) as u32;
                w.x = dwx + (dww as i32 - nw as i32);
                w.width = nw;
                w.height = nh;
                let count = (nw as usize) * (nh as usize);
                w.pixels.resize(count, 0xFF2A2A3E);
                w.dirty = true;
            }
            add_full_damage(&mut s);
        }
        DragMode::None => {}
    }
}

/// Handle mouse button release.
pub fn on_mouse_up(mx: i32, _my: i32) {
    let mut s = STATE.lock();
    // Check for edge snapping on move release
    if s.interaction.drag_mode == DragMode::Move {
        let id = s.interaction.drag_window;
        if let Some(w) = s.windows.iter().find(|w| w.id == id) {
            let snap_id = w.id;
            let fw = s.fb_width;
            if w.x <= SNAP_THRESHOLD {
                drop(s);
                snap_left(snap_id);
                return;
            } else if w.x + w.total_width() as i32 >= fw as i32 - SNAP_THRESHOLD {
                drop(s);
                snap_right(snap_id);
                return;
            }
        }
    }
    s.interaction.drag_mode = DragMode::None;
    // Check double-click on title bar (simplified: we don't track timing, skip)
    let _ = mx;
}

/// Handle double-click on title bar to maximize/restore.
pub fn on_double_click_title(id: u32) {
    let s = STATE.lock();
    let is_max = s.windows.iter().find(|w| w.id == id).map(|w| w.maximized).unwrap_or(false);
    drop(s);
    if is_max {
        restore_window(id);
    } else {
        maximize_window(id);
    }
}

// ── Alt+Tab ───────────────────────────────────────────────────────────────────

/// Start or cycle the Alt+Tab switcher.
pub fn alt_tab_next() {
    let mut s = STATE.lock();
    let desktop = s.active_desktop;
    let candidates: Vec<u32> = s.windows.iter()
        .filter(|w| w.desktop == desktop && !w.minimized)
        .map(|w| w.id)
        .collect();
    if candidates.is_empty() { return; }
    if !s.interaction.alt_tab_active {
        s.interaction.alt_tab_active = true;
        s.interaction.alt_tab_index = 0;
    }
    s.interaction.alt_tab_index = (s.interaction.alt_tab_index + 1) % candidates.len();
    add_full_damage(&mut s);
}

/// Confirm Alt+Tab selection.
pub fn alt_tab_confirm() {
    let mut s = STATE.lock();
    if !s.interaction.alt_tab_active { return; }
    let desktop = s.active_desktop;
    let candidates: Vec<u32> = s.windows.iter()
        .filter(|w| w.desktop == desktop && !w.minimized)
        .map(|w| w.id)
        .collect();
    s.interaction.alt_tab_active = false;
    if let Some(&id) = candidates.get(s.interaction.alt_tab_index) {
        for w in s.windows.iter_mut() {
            w.focused = w.id == id;
        }
        raise_to_top(&mut s.windows, id);
    }
    add_full_damage(&mut s);
}

// ── Compositing ───────────────────────────────────────────────────────────────

/// Composite all visible windows onto the internal framebuffer.
pub fn composite() {
    let mut s = STATE.lock();
    if !s.initialized { return; }

    let has_damage = s.damage.is_some() || s.windows.iter().any(|w| w.dirty);
    if !has_damage { return; }

    let fw = s.fb_width as usize;
    let fh = s.fb_height as usize;
    let usable_h = fh - TASKBAR_HEIGHT as usize;

    // Clear desktop background
    for y in 0..usable_h {
        for x in 0..fw {
            s.framebuffer[y * fw + x] = COLOR_DESKTOP_BG;
        }
    }

    // Render windows back-to-front — use split borrow via index
    let desktop = s.active_desktop;
    let alt_tab_active = s.interaction.alt_tab_active;
    let alt_tab_index = s.interaction.alt_tab_index;

    // We must split windows and framebuffer. Use a temporary swap approach:
    // take ownership of framebuffer, render, then put it back.
    let mut fb = core::mem::take(&mut s.framebuffer);
    for win in s.windows.iter() {
        if !win.visible || win.minimized { continue; }
        if win.desktop != desktop { continue; }
        render_window_to_fb(win, &mut fb, fw, usable_h);
    }

    // Render taskbar
    render_taskbar(&s.windows, desktop, &mut fb, fw, fh);

    // Render Alt+Tab overlay if active
    if alt_tab_active {
        render_alt_tab_overlay(&s.windows, desktop, alt_tab_index, &mut fb, fw, fh);
    }

    s.framebuffer = fb;

    // Clear damage and dirty flags
    s.damage = None;
    s.taskbar_dirty = false;
    for w in s.windows.iter_mut() {
        w.dirty = false;
    }

    TOTAL_COMPOSITES.fetch_add(1, Ordering::SeqCst);
    DAMAGE_REDRAWS.fetch_add(1, Ordering::SeqCst);
}

fn render_window_to_fb(win: &Window, fb: &mut [u32], fw: usize, fh: usize) {
    let wx = win.x;
    let wy = win.y;
    let tw = win.total_width() as i32;
    let th = win.total_height() as i32;

    if win.decorations {
        // Draw border
        draw_rect(fb, fw, fh, wx, wy, tw as u32, th as u32, COLOR_BORDER);

        // Draw title bar
        let tbx = wx + BORDER_WIDTH as i32;
        let tby = wy;
        let tbw = win.width;
        let tbh = TITLE_BAR_HEIGHT;
        let title_color = if win.focused { COLOR_TITLE_FOCUSED } else { COLOR_TITLE_UNFOCUSED };
        fill_rect(fb, fw, fh, tbx, tby, tbw, tbh, title_color);

        // Close button
        let cbx = wx + tw - BORDER_WIDTH as i32 - CLOSE_BTN_WIDTH as i32;
        fill_rect(fb, fw, fh, cbx, tby, CLOSE_BTN_WIDTH, tbh, COLOR_CLOSE_BTN);

        // Minimize button
        let mbx = cbx - MIN_BTN_WIDTH as i32;
        fill_rect(fb, fw, fh, mbx, tby, MIN_BTN_WIDTH, tbh, COLOR_MIN_BTN);

        // Maximize button
        let xbx = mbx - MAX_BTN_WIDTH as i32;
        fill_rect(fb, fw, fh, xbx, tby, MAX_BTN_WIDTH, tbh, COLOR_MAX_BTN);
    }

    // Draw content area
    let cx = win.content_x();
    let cy = win.content_y();
    let cw = win.width as usize;
    let ch = win.height as usize;
    for row in 0..ch {
        let sy = cy + row as i32;
        if sy < 0 || sy >= fh as i32 { continue; }
        for col in 0..cw {
            let sx = cx + col as i32;
            if sx < 0 || sx >= fw as i32 { continue; }
            let pidx = row * cw + col;
            if pidx < win.pixels.len() {
                fb[sy as usize * fw + sx as usize] = win.pixels[pidx];
            }
        }
    }
}

fn render_taskbar(windows: &[Window], desktop: u8, fb: &mut [u32], fw: usize, fh: usize) {
    let ty = (fh - TASKBAR_HEIGHT as usize) as i32;

    // Background
    fill_rect(fb, fw, fh, 0, ty, fw as u32, TASKBAR_HEIGHT, COLOR_TASKBAR_BG);

    // Desktop indicators
    for d in 0..NUM_DESKTOPS {
        let dx = 4 + d as i32 * 24;
        let color = if d as u8 == desktop { COLOR_TASKBAR_ACTIVE } else { 0xFF444444 };
        fill_rect(fb, fw, fh, dx, ty + 4, 20, 24, color);
    }

    // Window buttons
    let mut bx = 4 + (NUM_DESKTOPS as i32) * 24 + 8;
    for w in windows.iter() {
        if w.desktop != desktop { continue; }
        let color = if w.focused { COLOR_TASKBAR_ACTIVE } else { 0xFF333333 };
        fill_rect(fb, fw, fh, bx, ty + 4, 80, 24, color);
        bx += 84;
        if bx > fw as i32 - 120 { break; }
    }

    // System tray area (right side) — clock placeholder
    let clock_x = fw as i32 - 60;
    fill_rect(fb, fw, fh, clock_x, ty + 4, 56, 24, 0xFF333333);
}

fn render_alt_tab_overlay(windows: &[Window], desktop: u8, selected: usize,
                          fb: &mut [u32], fw: usize, fh: usize) {
    let candidates: Vec<&Window> = windows.iter()
        .filter(|w| w.desktop == desktop && !w.minimized)
        .collect();
    if candidates.is_empty() { return; }

    let count = candidates.len() as u32;
    let total_w = count * ALT_TAB_THUMB_W + (count + 1) * ALT_TAB_PADDING;
    let total_h = ALT_TAB_THUMB_H + ALT_TAB_PADDING * 2;
    let ox = (fw as i32 - total_w as i32) / 2;
    let oy = (fh as i32 - total_h as i32) / 2;

    // Background
    fill_rect(fb, fw, fh, ox, oy, total_w, total_h, COLOR_ALT_TAB_BG);

    // Thumbnails
    for (i, _w) in candidates.iter().enumerate() {
        let tx = ox + ALT_TAB_PADDING as i32 + i as i32 * (ALT_TAB_THUMB_W as i32 + ALT_TAB_PADDING as i32);
        let tty = oy + ALT_TAB_PADDING as i32;
        let border = if i == selected { COLOR_ALT_TAB_SEL } else { 0xFF555555 };
        draw_rect(fb, fw, fh, tx - 2, tty - 2, ALT_TAB_THUMB_W + 4, ALT_TAB_THUMB_H + 4, border);
        fill_rect(fb, fw, fh, tx, tty, ALT_TAB_THUMB_W, ALT_TAB_THUMB_H, 0xFF333333);
    }
}

// ── Drawing Helpers ───────────────────────────────────────────────────────────

fn fill_rect(fb: &mut [u32], fw: usize, fh: usize, x: i32, y: i32, w: u32, h: u32, color: u32) {
    for row in 0..h as i32 {
        let sy = y + row;
        if sy < 0 || sy >= fh as i32 { continue; }
        for col in 0..w as i32 {
            let sx = x + col;
            if sx < 0 || sx >= fw as i32 { continue; }
            fb[sy as usize * fw + sx as usize] = color;
        }
    }
}

fn draw_rect(fb: &mut [u32], fw: usize, fh: usize, x: i32, y: i32, w: u32, h: u32, color: u32) {
    // Top and bottom edges
    for col in 0..w as i32 {
        let sx = x + col;
        if sx >= 0 && sx < fw as i32 {
            if y >= 0 && y < fh as i32 {
                fb[y as usize * fw + sx as usize] = color;
            }
            let by = y + h as i32 - 1;
            if by >= 0 && by < fh as i32 {
                fb[by as usize * fw + sx as usize] = color;
            }
        }
    }
    // Left and right edges
    for row in 0..h as i32 {
        let sy = y + row;
        if sy >= 0 && sy < fh as i32 {
            if x >= 0 && x < fw as i32 {
                fb[sy as usize * fw + x as usize] = color;
            }
            let rx = x + w as i32 - 1;
            if rx >= 0 && rx < fw as i32 {
                fb[sy as usize * fw + rx as usize] = color;
            }
        }
    }
}

// ── Internal Helpers ──────────────────────────────────────────────────────────

fn handle_taskbar_click(s: &mut CompositorState, mx: i32) {
    // Desktop buttons
    for d in 0..NUM_DESKTOPS {
        let dx = 4 + d as i32 * 24;
        if mx >= dx && mx < dx + 20 {
            let desktop = d as u8;
            s.active_desktop = desktop;
            for w in s.windows.iter_mut() {
                if w.desktop == desktop {
                    if !w.minimized { w.visible = true; }
                } else {
                    w.visible = false;
                }
            }
            add_full_damage(s);
            s.taskbar_dirty = true;
            return;
        }
    }

    // Window buttons
    let mut bx = 4 + (NUM_DESKTOPS as i32) * 24 + 8;
    let desktop = s.active_desktop;
    let mut target_id = None;
    for w in s.windows.iter() {
        if w.desktop != desktop { continue; }
        if mx >= bx && mx < bx + 80 {
            target_id = Some((w.id, w.minimized));
            break;
        }
        bx += 84;
    }
    if let Some((id, minimized)) = target_id {
        if minimized {
            if let Some(w) = s.windows.iter_mut().find(|w| w.id == id) {
                w.minimized = false;
                w.visible = true;
                w.focused = true;
            }
            raise_to_top(&mut s.windows, id);
        } else {
            for w in s.windows.iter_mut() {
                w.focused = w.id == id;
            }
            raise_to_top(&mut s.windows, id);
        }
        add_full_damage(s);
        s.taskbar_dirty = true;
    }
}

fn raise_to_top(windows: &mut Vec<Window>, id: u32) {
    if let Some(idx) = windows.iter().position(|w| w.id == id) {
        let win = windows.remove(idx);
        windows.push(win);
        recompute_z_order(windows);
    }
}

fn recompute_z_order(windows: &mut [Window]) {
    for (i, w) in windows.iter_mut().enumerate() {
        w.z_order = i as u16;
    }
}

fn add_full_damage(s: &mut CompositorState) {
    s.damage = Some(DamageRect { x: 0, y: 0, w: s.fb_width, h: s.fb_height });
}

// ── Info/Stats API ────────────────────────────────────────────────────────────

/// Return compositor info string.
pub fn compositor_info() -> String {
    let s = STATE.lock();
    let win_count = s.windows.len();
    let desktop = s.active_desktop;
    let focused = s.windows.iter().find(|w| w.focused).map(|w| w.id).unwrap_or(0);
    format!(
        "Compositor: {}x{} | Desktop {}/{} | Windows: {} | Focused: {} | Composites: {}",
        s.fb_width, s.fb_height, desktop + 1, NUM_DESKTOPS,
        win_count, focused, TOTAL_COMPOSITES.load(Ordering::SeqCst)
    )
}

/// Return compositor stats.
pub fn compositor_stats() -> String {
    let s = STATE.lock();
    let visible = s.windows.iter().filter(|w| w.visible && !w.minimized).count();
    let minimized = s.windows.iter().filter(|w| w.minimized).count();
    let total_pixels: usize = s.windows.iter().map(|w| w.pixels.len()).sum();
    format!(
        "Windows: {} total, {} visible, {} minimized\n\
         Pixel buffers: {} total pixels ({} KiB)\n\
         Created: {} | Destroyed: {}\n\
         Composites: {} | Damage redraws: {}\n\
         Desktops: {} | Active: {}",
        s.windows.len(), visible, minimized,
        total_pixels, total_pixels * 4 / 1024,
        TOTAL_CREATED.load(Ordering::SeqCst),
        TOTAL_DESTROYED.load(Ordering::SeqCst),
        TOTAL_COMPOSITES.load(Ordering::SeqCst),
        DAMAGE_REDRAWS.load(Ordering::SeqCst),
        NUM_DESKTOPS, s.active_desktop + 1
    )
}

/// List all windows.
pub fn list_windows() -> String {
    let s = STATE.lock();
    if s.windows.is_empty() {
        return String::from("No windows.");
    }
    let mut out = String::from("ID    TITLE                PID   POS        SIZE       Z  DESK  FLAGS\n");
    for w in s.windows.iter() {
        let mut flags = String::new();
        if w.focused { flags.push('F'); }
        if w.minimized { flags.push('m'); }
        if w.maximized { flags.push('M'); }
        if !w.visible { flags.push('h'); }
        if w.dirty { flags.push('d'); }
        out.push_str(&format!(
            "{:<5} {:<20} {:<5} {:>4},{:<4} {:>4}x{:<4} {:<2} {:<5} {}\n",
            w.id,
            if w.title.len() > 20 { &w.title[..20] } else { &w.title },
            w.pid, w.x, w.y, w.width, w.height, w.z_order, w.desktop + 1, flags
        ));
    }
    out
}
