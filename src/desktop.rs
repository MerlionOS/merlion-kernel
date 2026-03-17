/// Desktop environment for MerlionOS.
/// Provides desktop icons, right-click menu, wallpaper,
/// application launcher, and system notifications overlay.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

const ICON_CELL_W: u32 = 80;
const ICON_CELL_H: u32 = 80;
const ICON_COLS: u32 = 12;
const ICON_ROWS: u32 = 8;
const MAX_NOTIFICATIONS: usize = 3;
const NOTIFICATION_WIDTH: u32 = 300;
const NOTIFICATION_HEIGHT: u32 = 60;
const NOTIFICATION_GAP: u32 = 8;
const NOTIFICATION_TIMEOUT_TICKS: u64 = 500; // ~5 seconds at 100Hz
const MAX_RECENT_APPS: usize = 8;
const MAX_CONTEXT_ITEMS: usize = 8;
const LAUNCHER_WIDTH: u32 = 400;
const LAUNCHER_HEIGHT: u32 = 500;

// ── Theme ─────────────────────────────────────────────────────────────────────

/// Desktop theme colours (ARGB).
pub struct DesktopTheme {
    pub taskbar_bg: u32,
    pub taskbar_fg: u32,
    pub window_bg: u32,
    pub window_title_bg: u32,
    pub window_title_fg: u32,
    pub desktop_bg: u32,
    pub accent: u32,
    pub text: u32,
    pub selection: u32,
}

impl DesktopTheme {
    /// Default dark theme with Merlion gold accent.
    const fn default_theme() -> Self {
        Self {
            taskbar_bg: 0xFF0D0D1A,
            taskbar_fg: 0xFFCCCCCC,
            window_bg: 0xFF1E1E2E,
            window_title_bg: 0xFF2A2A3E,
            window_title_fg: 0xFFEEEEEE,
            desktop_bg: 0xFF1A1A2E,
            accent: 0xFFCCA752,       // Merlion gold
            text: 0xFFE0E0E0,
            selection: 0xFF3366AA,
        }
    }

    fn clone_theme(&self) -> Self {
        Self {
            taskbar_bg: self.taskbar_bg,
            taskbar_fg: self.taskbar_fg,
            window_bg: self.window_bg,
            window_title_bg: self.window_title_bg,
            window_title_fg: self.window_title_fg,
            desktop_bg: self.desktop_bg,
            accent: self.accent,
            text: self.text,
            selection: self.selection,
        }
    }
}

// ── Desktop Icon ──────────────────────────────────────────────────────────────

struct DesktopIcon {
    label: &'static str,
    command: &'static str,
    col: u32,
    row: u32,
    color: u32,
}

impl DesktopIcon {
    const fn new(label: &'static str, command: &'static str, col: u32, row: u32, color: u32) -> Self {
        Self { label, command, col, row, color }
    }
}

static DEFAULT_ICONS: &[DesktopIcon] = &[
    DesktopIcon::new("Terminal", "bash", 0, 0, 0xFF44AA44),
    DesktopIcon::new("Files", "ls /", 1, 0, 0xFF4488CC),
    DesktopIcon::new("Settings", "sysctl-list", 2, 0, 0xFF888888),
    DesktopIcon::new("About", "version", 3, 0, 0xFFCCA752),
    DesktopIcon::new("Snake", "snake", 0, 1, 0xFF33BB33),
    DesktopIcon::new("Calculator", "calc", 1, 1, 0xFF6666CC),
    DesktopIcon::new("Editor", "edit", 2, 1, 0xFFCC8844),
];

// ── Application Categories ────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum AppCategory {
    System,
    Network,
    Development,
    Games,
    AI,
}

impl AppCategory {
    fn as_str(self) -> &'static str {
        match self {
            AppCategory::System => "System",
            AppCategory::Network => "Network",
            AppCategory::Development => "Development",
            AppCategory::Games => "Games",
            AppCategory::AI => "AI",
        }
    }

    fn all() -> &'static [AppCategory] {
        &[
            AppCategory::System,
            AppCategory::Network,
            AppCategory::Development,
            AppCategory::Games,
            AppCategory::AI,
        ]
    }
}

struct AppEntry {
    name: &'static str,
    command: &'static str,
    category: AppCategory,
}

static APPS: &[AppEntry] = &[
    AppEntry { name: "Terminal", command: "bash", category: AppCategory::System },
    AppEntry { name: "File Manager", command: "ls /", category: AppCategory::System },
    AppEntry { name: "Task Manager", command: "top", category: AppCategory::System },
    AppEntry { name: "System Info", command: "info", category: AppCategory::System },
    AppEntry { name: "Settings", command: "sysctl-list", category: AppCategory::System },
    AppEntry { name: "Vim", command: "vim", category: AppCategory::System },
    AppEntry { name: "Process List", command: "ps", category: AppCategory::System },
    AppEntry { name: "Network Info", command: "net", category: AppCategory::Network },
    AppEntry { name: "Ping", command: "ping 127.0.0.1", category: AppCategory::Network },
    AppEntry { name: "Wget", command: "wget", category: AppCategory::Network },
    AppEntry { name: "SSH", command: "ssh", category: AppCategory::Network },
    AppEntry { name: "Firewall", command: "iptables -L", category: AppCategory::Network },
    AppEntry { name: "DNS Lookup", command: "dns localhost", category: AppCategory::Network },
    AppEntry { name: "Build", command: "make", category: AppCategory::Development },
    AppEntry { name: "Editor", command: "edit", category: AppCategory::Development },
    AppEntry { name: "Git", command: "git", category: AppCategory::Development },
    AppEntry { name: "Debugger", command: "kdb", category: AppCategory::Development },
    AppEntry { name: "Profiler", command: "perf", category: AppCategory::Development },
    AppEntry { name: "Snake", command: "snake", category: AppCategory::Games },
    AppEntry { name: "Tetris", command: "tetris", category: AppCategory::Games },
    AppEntry { name: "Calculator", command: "calc", category: AppCategory::Games },
    AppEntry { name: "Fortune", command: "fortune", category: AppCategory::Games },
    AppEntry { name: "AI Shell", command: "ai", category: AppCategory::AI },
    AppEntry { name: "AI Agent", command: "agent", category: AppCategory::AI },
    AppEntry { name: "AI Monitor", command: "ai-monitor", category: AppCategory::AI },
    AppEntry { name: "ML Train", command: "ml-train", category: AppCategory::AI },
    AppEntry { name: "NN Inference", command: "nn-info", category: AppCategory::AI },
];

// ── Context Menu ──────────────────────────────────────────────────────────────

struct ContextMenuItem {
    label: &'static str,
    command: &'static str,
}

static CONTEXT_MENU_ITEMS: &[ContextMenuItem] = &[
    ContextMenuItem { label: "New File", command: "touch /tmp/new_file" },
    ContextMenuItem { label: "New Folder", command: "mkdir /tmp/new_folder" },
    ContextMenuItem { label: "Terminal", command: "bash" },
    ContextMenuItem { label: "Settings", command: "sysctl-list" },
    ContextMenuItem { label: "About MerlionOS", command: "version" },
];

// ── Notification ──────────────────────────────────────────────────────────────

struct Notification {
    id: u32,
    title: String,
    body: String,
    action_label: String,
    action_command: String,
    created_tick: u64,
}

// ── System Tray ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct TrayState {
    network_connected: bool,
    battery_percent: u8,
    battery_charging: bool,
    volume_percent: u8,
    volume_muted: bool,
    unread_notifications: u32,
}

impl TrayState {
    const fn new() -> Self {
        Self {
            network_connected: false,
            battery_percent: 100,
            battery_charging: false,
            volume_percent: 75,
            volume_muted: false,
            unread_notifications: 0,
        }
    }
}

// ── Desktop State ─────────────────────────────────────────────────────────────

struct DesktopState {
    theme: DesktopTheme,
    wallpaper_pattern: WallpaperPattern,
    notifications: Vec<Notification>,
    recent_apps: Vec<&'static str>,
    launcher_visible: bool,
    launcher_search: String,
    launcher_selected: usize,
    context_menu_visible: bool,
    context_menu_x: i32,
    context_menu_y: i32,
    tray: TrayState,
    initialized: bool,
}

impl DesktopState {
    const fn new() -> Self {
        Self {
            theme: DesktopTheme::default_theme(),
            wallpaper_pattern: WallpaperPattern::Solid,
            notifications: Vec::new(),
            recent_apps: Vec::new(),
            launcher_visible: false,
            launcher_search: String::new(),
            launcher_selected: 0,
            context_menu_visible: false,
            context_menu_x: 0,
            context_menu_y: 0,
            tray: TrayState::new(),
            initialized: false,
        }
    }
}

static STATE: Mutex<DesktopState> = Mutex::new(DesktopState::new());
static NEXT_NOTIF_ID: AtomicU32 = AtomicU32::new(1);
static TOTAL_NOTIFICATIONS: AtomicU64 = AtomicU64::new(0);
static TOTAL_LAUNCHES: AtomicU64 = AtomicU64::new(0);
static CURRENT_TICK: AtomicU64 = AtomicU64::new(0);

// ── Wallpaper Patterns ────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum WallpaperPattern {
    Solid,
    HorizontalGradient,
    VerticalGradient,
    Checkerboard,
    DiagonalStripes,
}

impl WallpaperPattern {
    fn as_str(self) -> &'static str {
        match self {
            WallpaperPattern::Solid => "solid",
            WallpaperPattern::HorizontalGradient => "horizontal-gradient",
            WallpaperPattern::VerticalGradient => "vertical-gradient",
            WallpaperPattern::Checkerboard => "checkerboard",
            WallpaperPattern::DiagonalStripes => "diagonal-stripes",
        }
    }
}

/// Compute wallpaper pixel using integer math only.
fn wallpaper_pixel(pattern: WallpaperPattern, base_color: u32, x: u32, y: u32, w: u32, h: u32) -> u32 {
    match pattern {
        WallpaperPattern::Solid => base_color,
        WallpaperPattern::HorizontalGradient => {
            // Darken from left to right: reduce each channel by x/w fraction
            let r = ((base_color >> 16) & 0xFF) * (w - x) / w;
            let g = ((base_color >> 8) & 0xFF) * (w - x) / w;
            let b = (base_color & 0xFF) * (w - x) / w;
            0xFF000000 | (r << 16) | (g << 8) | b
        }
        WallpaperPattern::VerticalGradient => {
            let r = ((base_color >> 16) & 0xFF) * (h - y) / h;
            let g = ((base_color >> 8) & 0xFF) * (h - y) / h;
            let b = (base_color & 0xFF) * (h - y) / h;
            0xFF000000 | (r << 16) | (g << 8) | b
        }
        WallpaperPattern::Checkerboard => {
            let cell = 32u32;
            if ((x / cell) + (y / cell)) % 2 == 0 {
                base_color
            } else {
                // Slightly lighter variant
                let r = (((base_color >> 16) & 0xFF) + 20).min(255);
                let g = (((base_color >> 8) & 0xFF) + 20).min(255);
                let b = ((base_color & 0xFF) + 20).min(255);
                0xFF000000 | (r << 16) | (g << 8) | b
            }
        }
        WallpaperPattern::DiagonalStripes => {
            let stripe = 16u32;
            if ((x + y) / stripe) % 2 == 0 {
                base_color
            } else {
                let r = (((base_color >> 16) & 0xFF) + 15).min(255);
                let g = (((base_color >> 8) & 0xFF) + 15).min(255);
                let b = ((base_color & 0xFF) + 15).min(255);
                0xFF000000 | (r << 16) | (g << 8) | b
            }
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialize the desktop environment.
pub fn init() {
    let mut s = STATE.lock();
    s.initialized = true;
    s.tray.network_connected = true;
}

/// Advance tick counter (called from timer interrupt).
pub fn tick() {
    CURRENT_TICK.fetch_add(1, Ordering::SeqCst);
}

/// Expire old notifications.
pub fn expire_notifications() {
    let mut s = STATE.lock();
    let now = CURRENT_TICK.load(Ordering::SeqCst);
    s.notifications.retain(|n| {
        now.wrapping_sub(n.created_tick) < NOTIFICATION_TIMEOUT_TICKS
    });
}

/// Post a new notification.
pub fn notify(title: &str, body: &str) -> u32 {
    notify_with_action(title, body, "", "")
}

/// Post a notification with an action button.
pub fn notify_with_action(title: &str, body: &str, action_label: &str, action_cmd: &str) -> u32 {
    let mut s = STATE.lock();
    let id = NEXT_NOTIF_ID.fetch_add(1, Ordering::SeqCst);
    let tick = CURRENT_TICK.load(Ordering::SeqCst);
    // Limit visible notifications
    while s.notifications.len() >= MAX_NOTIFICATIONS {
        s.notifications.remove(0);
    }
    s.notifications.push(Notification {
        id,
        title: String::from(title),
        body: String::from(body),
        action_label: String::from(action_label),
        action_command: String::from(action_cmd),
        created_tick: tick,
    });
    s.tray.unread_notifications += 1;
    TOTAL_NOTIFICATIONS.fetch_add(1, Ordering::SeqCst);
    id
}

/// Dismiss a notification by id.
pub fn dismiss_notification(id: u32) {
    let mut s = STATE.lock();
    if let Some(idx) = s.notifications.iter().position(|n| n.id == id) {
        s.notifications.remove(idx);
        if s.tray.unread_notifications > 0 {
            s.tray.unread_notifications -= 1;
        }
    }
}

// ── Launcher ──────────────────────────────────────────────────────────────────

/// Toggle the application launcher.
pub fn toggle_launcher() {
    let mut s = STATE.lock();
    s.launcher_visible = !s.launcher_visible;
    if s.launcher_visible {
        s.launcher_search = String::new();
        s.launcher_selected = 0;
    }
}

/// Show the launcher.
pub fn show_launcher() {
    let mut s = STATE.lock();
    s.launcher_visible = true;
    s.launcher_search = String::new();
    s.launcher_selected = 0;
}

/// Hide the launcher.
pub fn hide_launcher() {
    let mut s = STATE.lock();
    s.launcher_visible = false;
}

/// Update launcher search query.
pub fn launcher_set_search(query: &str) {
    let mut s = STATE.lock();
    s.launcher_search = String::from(query);
    s.launcher_selected = 0;
}

/// Move launcher selection up.
pub fn launcher_up() {
    let mut s = STATE.lock();
    if s.launcher_selected > 0 {
        s.launcher_selected -= 1;
    }
}

/// Move launcher selection down.
pub fn launcher_down() {
    let mut s = STATE.lock();
    let count = matching_app_count(&s.launcher_search);
    if s.launcher_selected + 1 < count {
        s.launcher_selected += 1;
    }
}

/// Launch the currently selected app from the launcher.
pub fn launcher_confirm() -> Option<String> {
    let mut s = STATE.lock();
    if !s.launcher_visible { return None; }
    let search = s.launcher_search.clone();
    let idx = s.launcher_selected;
    let mut count = 0usize;
    let mut result = None;
    for app in APPS.iter() {
        if !search.is_empty() {
            let name_lower = app.name.as_bytes();
            let search_bytes = search.as_bytes();
            if !contains_ci(name_lower, search_bytes) { continue; }
        }
        if count == idx {
            result = Some(String::from(app.command));
            // Track recent
            add_recent(&mut s.recent_apps, app.command);
            TOTAL_LAUNCHES.fetch_add(1, Ordering::SeqCst);
            break;
        }
        count += 1;
    }
    s.launcher_visible = false;
    result
}

/// Launch an application by name. Returns the command to execute.
pub fn launch_app(name: &str) -> Option<String> {
    let mut s = STATE.lock();
    for app in APPS.iter() {
        if eq_ci(app.name.as_bytes(), name.as_bytes()) || eq_ci(app.command.as_bytes(), name.as_bytes()) {
            add_recent(&mut s.recent_apps, app.command);
            TOTAL_LAUNCHES.fetch_add(1, Ordering::SeqCst);
            return Some(String::from(app.command));
        }
    }
    None
}

/// List all available apps.
pub fn list_apps() -> String {
    let mut out = String::from("CATEGORY       NAME              COMMAND\n");
    for cat in AppCategory::all() {
        for app in APPS.iter() {
            if app.category == *cat {
                out.push_str(&format!(
                    "{:<14} {:<17} {}\n",
                    cat.as_str(), app.name, app.command
                ));
            }
        }
    }
    out
}

// ── Context Menu ──────────────────────────────────────────────────────────────

/// Show context menu at position.
pub fn show_context_menu(x: i32, y: i32) {
    let mut s = STATE.lock();
    s.context_menu_visible = true;
    s.context_menu_x = x;
    s.context_menu_y = y;
}

/// Hide context menu.
pub fn hide_context_menu() {
    let mut s = STATE.lock();
    s.context_menu_visible = false;
}

/// Handle context menu item click. Returns command to execute if any.
pub fn context_menu_click(index: usize) -> Option<&'static str> {
    let mut s = STATE.lock();
    s.context_menu_visible = false;
    CONTEXT_MENU_ITEMS.get(index).map(|item| item.command)
}

// ── System Tray ───────────────────────────────────────────────────────────────

/// Update network status in tray.
pub fn set_network_status(connected: bool) {
    let mut s = STATE.lock();
    s.tray.network_connected = connected;
}

/// Update battery info.
pub fn set_battery(percent: u8, charging: bool) {
    let mut s = STATE.lock();
    s.tray.battery_percent = percent;
    s.tray.battery_charging = charging;
}

/// Update volume.
pub fn set_volume(percent: u8, muted: bool) {
    let mut s = STATE.lock();
    s.tray.volume_percent = percent;
    s.tray.volume_muted = muted;
}

/// Get the clock string (HH:MM) from RTC.
pub fn clock_string() -> String {
    let dt = crate::rtc::read();
    format!("{:02}:{:02}", dt.hour, dt.minute)
}

/// Get system tray info string.
pub fn tray_info() -> String {
    let s = STATE.lock();
    let net = if s.tray.network_connected { "Connected" } else { "Disconnected" };
    let batt = if s.tray.battery_charging {
        format!("{}% (charging)", s.tray.battery_percent)
    } else {
        format!("{}%", s.tray.battery_percent)
    };
    let vol = if s.tray.volume_muted {
        String::from("Muted")
    } else {
        format!("{}%", s.tray.volume_percent)
    };
    format!(
        "Clock: {} | Net: {} | Battery: {} | Volume: {} | Notifications: {}",
        clock_string(), net, batt, vol, s.tray.unread_notifications
    )
}

// ── Theme ─────────────────────────────────────────────────────────────────────

/// Set the desktop theme.
pub fn set_theme(theme: DesktopTheme) {
    let mut s = STATE.lock();
    s.theme = theme;
}

/// Get current theme info.
pub fn get_theme() -> String {
    let s = STATE.lock();
    format!(
        "Theme:\n  Taskbar BG:    #{:06X}\n  Taskbar FG:    #{:06X}\n\
         Desktop BG:    #{:06X}\n  Accent:        #{:06X}\n\
         Text:          #{:06X}\n  Selection:     #{:06X}\n\
         Window BG:     #{:06X}\n  Title BG:      #{:06X}\n  Title FG:      #{:06X}",
        s.theme.taskbar_bg & 0xFFFFFF, s.theme.taskbar_fg & 0xFFFFFF,
        s.theme.desktop_bg & 0xFFFFFF, s.theme.accent & 0xFFFFFF,
        s.theme.text & 0xFFFFFF, s.theme.selection & 0xFFFFFF,
        s.theme.window_bg & 0xFFFFFF, s.theme.window_title_bg & 0xFFFFFF,
        s.theme.window_title_fg & 0xFFFFFF,
    )
}

/// Set wallpaper pattern.
pub fn set_wallpaper(pattern_name: &str) {
    let mut s = STATE.lock();
    s.wallpaper_pattern = match pattern_name {
        "solid" => WallpaperPattern::Solid,
        "horizontal-gradient" | "hgradient" => WallpaperPattern::HorizontalGradient,
        "vertical-gradient" | "vgradient" => WallpaperPattern::VerticalGradient,
        "checkerboard" | "checker" => WallpaperPattern::Checkerboard,
        "diagonal-stripes" | "stripes" => WallpaperPattern::DiagonalStripes,
        _ => return,
    };
}

// ── Rendering ─────────────────────────────────────────────────────────────────

/// Render wallpaper into a pixel buffer.
pub fn render_wallpaper(fb: &mut [u32], width: u32, height: u32) {
    let s = STATE.lock();
    let pattern = s.wallpaper_pattern;
    let base = s.theme.desktop_bg;
    drop(s);

    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) as usize;
            if idx < fb.len() {
                fb[idx] = wallpaper_pixel(pattern, base, x, y, width, height);
            }
        }
    }
}

/// Render desktop icons onto a pixel buffer.
pub fn render_icons(fb: &mut [u32], fb_width: u32, _fb_height: u32) {
    for icon in DEFAULT_ICONS.iter() {
        let ix = icon.col * ICON_CELL_W + 8;
        let iy = icon.row * ICON_CELL_H + 8;
        // Draw a simple colored square as icon (40x40)
        for dy in 0u32..40 {
            for dx in 0u32..40 {
                let px = ix + dx + 12;
                let py = iy + dy;
                let idx = (py * fb_width + px) as usize;
                if idx < fb.len() {
                    fb[idx] = icon.color;
                }
            }
        }
    }
}

/// Render notification overlay onto a pixel buffer (top-right corner).
pub fn render_notifications(fb: &mut [u32], fb_width: u32, _fb_height: u32) {
    let s = STATE.lock();
    let nx = fb_width - NOTIFICATION_WIDTH - 16;
    for (i, _notif) in s.notifications.iter().enumerate() {
        let ny = 16 + i as u32 * (NOTIFICATION_HEIGHT + NOTIFICATION_GAP);
        // Background
        for dy in 0..NOTIFICATION_HEIGHT {
            for dx in 0..NOTIFICATION_WIDTH {
                let idx = ((ny + dy) * fb_width + (nx + dx)) as usize;
                if idx < fb.len() {
                    fb[idx] = 0xE0222222;
                }
            }
        }
        // Accent stripe on left
        for dy in 0..NOTIFICATION_HEIGHT {
            for dx in 0..4u32 {
                let idx = ((ny + dy) * fb_width + (nx + dx)) as usize;
                if idx < fb.len() {
                    fb[idx] = s.theme.accent;
                }
            }
        }
    }
}

/// Render context menu onto a pixel buffer.
pub fn render_context_menu(fb: &mut [u32], fb_width: u32, _fb_height: u32) {
    let s = STATE.lock();
    if !s.context_menu_visible { return; }
    let mx = s.context_menu_x as u32;
    let my = s.context_menu_y as u32;
    let item_h = 24u32;
    let menu_w = 160u32;
    let menu_h = CONTEXT_MENU_ITEMS.len() as u32 * item_h;

    for dy in 0..menu_h {
        for dx in 0..menu_w {
            let idx = ((my + dy) * fb_width + (mx + dx)) as usize;
            if idx < fb.len() {
                fb[idx] = 0xF0333333;
            }
        }
    }
    // Highlight items with alternating shade
    for (i, _item) in CONTEXT_MENU_ITEMS.iter().enumerate() {
        if i % 2 == 1 {
            let iy = my + i as u32 * item_h;
            for dy in 0..item_h {
                for dx in 0..menu_w {
                    let idx = ((iy + dy) * fb_width + (mx + dx)) as usize;
                    if idx < fb.len() {
                        fb[idx] = 0xF03A3A3A;
                    }
                }
            }
        }
    }
}

/// Render launcher overlay.
pub fn render_launcher(fb: &mut [u32], fb_width: u32, fb_height: u32) {
    let s = STATE.lock();
    if !s.launcher_visible { return; }
    let lx = (fb_width - LAUNCHER_WIDTH) / 2;
    let ly = (fb_height - LAUNCHER_HEIGHT) / 2;

    // Background
    for dy in 0..LAUNCHER_HEIGHT {
        for dx in 0..LAUNCHER_WIDTH {
            let idx = ((ly + dy) * fb_width + (lx + dx)) as usize;
            if idx < fb.len() {
                fb[idx] = 0xF01A1A2E;
            }
        }
    }
    // Title bar
    for dy in 0..32u32 {
        for dx in 0..LAUNCHER_WIDTH {
            let idx = ((ly + dy) * fb_width + (lx + dx)) as usize;
            if idx < fb.len() {
                fb[idx] = s.theme.accent;
            }
        }
    }
}

// ── Keyboard Shortcuts ────────────────────────────────────────────────────────

/// Handle a keyboard shortcut. Returns an optional command to execute.
pub fn handle_shortcut(key: u8, alt: bool, ctrl: bool, super_key: bool) -> Option<String> {
    // Alt+F2: launcher
    if alt && key == 0x3C { // F2 scancode
        toggle_launcher();
        return None;
    }
    // Alt+F4: close focused window
    if alt && key == 0x3E { // F4 scancode
        return Some(String::from("__close_focused"));
    }
    // Ctrl+Alt+Del: task manager
    if ctrl && alt && key == 0x53 { // Del scancode
        return Some(String::from("top"));
    }
    // Super: toggle launcher
    if super_key && key == 0 {
        toggle_launcher();
        return None;
    }
    // Ctrl+Alt+1..4: switch desktop
    if ctrl && alt {
        match key {
            0x02 => { crate::compositor::switch_desktop(0); return None; } // '1'
            0x03 => { crate::compositor::switch_desktop(1); return None; } // '2'
            0x04 => { crate::compositor::switch_desktop(2); return None; } // '3'
            0x05 => { crate::compositor::switch_desktop(3); return None; } // '4'
            _ => {}
        }
    }
    None
}

// ── Info/Stats ────────────────────────────────────────────────────────────────

/// Desktop info string.
pub fn desktop_info() -> String {
    let s = STATE.lock();
    let pattern = s.wallpaper_pattern.as_str();
    let notifs = s.notifications.len();
    let recent = s.recent_apps.len();
    format!(
        "Desktop Environment: MerlionOS Desktop\n\
         Wallpaper: {} (#{:06X})\n\
         Icons: {}\n\
         Notifications: {} visible\n\
         Launcher: {}\n\
         Recent apps: {}\n\
         Tray: {}",
        pattern, s.theme.desktop_bg & 0xFFFFFF,
        DEFAULT_ICONS.len(),
        notifs,
        if s.launcher_visible { "visible" } else { "hidden" },
        recent,
        tray_summary(&s.tray),
    )
}

/// Desktop stats string.
pub fn desktop_stats() -> String {
    let s = STATE.lock();
    format!(
        "Total notifications: {}\n\
         Total app launches: {}\n\
         Current tick: {}\n\
         Active notifications: {}\n\
         Recent apps: {}\n\
         Apps registered: {}\n\
         Desktop icons: {}\n\
         Context items: {}",
        TOTAL_NOTIFICATIONS.load(Ordering::SeqCst),
        TOTAL_LAUNCHES.load(Ordering::SeqCst),
        CURRENT_TICK.load(Ordering::SeqCst),
        s.notifications.len(),
        s.recent_apps.len(),
        APPS.len(),
        DEFAULT_ICONS.len(),
        CONTEXT_MENU_ITEMS.len(),
    )
}

// ── Internal Helpers ──────────────────────────────────────────────────────────

fn matching_app_count(search: &str) -> usize {
    if search.is_empty() { return APPS.len(); }
    let sb = search.as_bytes();
    APPS.iter().filter(|a| contains_ci(a.name.as_bytes(), sb)).count()
}

fn add_recent(recent: &mut Vec<&'static str>, cmd: &str) {
    // Remove if already present
    recent.retain(|&c| c != cmd);
    // Find matching static entry
    for app in APPS.iter() {
        if app.command == cmd {
            recent.insert(0, app.command);
            break;
        }
    }
    if recent.len() > MAX_RECENT_APPS {
        recent.pop();
    }
}

/// Case-insensitive byte-level contains check.
fn contains_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() { return true; }
    if needle.len() > haystack.len() { return false; }
    'outer: for i in 0..=(haystack.len() - needle.len()) {
        for j in 0..needle.len() {
            if to_lower(haystack[i + j]) != to_lower(needle[j]) {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

/// Case-insensitive equality.
fn eq_ci(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    for i in 0..a.len() {
        if to_lower(a[i]) != to_lower(b[i]) { return false; }
    }
    true
}

fn to_lower(b: u8) -> u8 {
    if b >= b'A' && b <= b'Z' { b + 32 } else { b }
}

fn tray_summary(tray: &TrayState) -> String {
    let net = if tray.network_connected { "up" } else { "down" };
    let vol = if tray.volume_muted {
        String::from("muted")
    } else {
        format!("{}%", tray.volume_percent)
    };
    format!(
        "net={} batt={}%{} vol={} notifs={}",
        net, tray.battery_percent,
        if tray.battery_charging { "+" } else { "" },
        vol, tray.unread_notifications
    )
}
