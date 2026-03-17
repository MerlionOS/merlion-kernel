/// Display manager for MerlionOS.
/// Manages multiple displays, resolution modes, brightness,
/// and provides a display server abstraction.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Connector type for a display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectorType {
    VGA,
    HDMI,
    DisplayPort,
    DVI,
    LVDS,
    EDP,
    Unknown,
}

impl ConnectorType {
    fn as_str(&self) -> &'static str {
        match self {
            ConnectorType::VGA => "VGA",
            ConnectorType::HDMI => "HDMI",
            ConnectorType::DisplayPort => "DisplayPort",
            ConnectorType::DVI => "DVI",
            ConnectorType::LVDS => "LVDS",
            ConnectorType::EDP => "eDP",
            ConnectorType::Unknown => "Unknown",
        }
    }
}

/// DPMS power state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DpmsState {
    On,
    Standby,
    Suspend,
    Off,
}

impl DpmsState {
    fn as_str(&self) -> &'static str {
        match self {
            DpmsState::On => "On",
            DpmsState::Standby => "Standby",
            DpmsState::Suspend => "Suspend",
            DpmsState::Off => "Off",
        }
    }
}

/// Multi-monitor layout mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LayoutMode {
    Single,
    Mirror,
    Extend,
}

impl LayoutMode {
    fn as_str(&self) -> &'static str {
        match self {
            LayoutMode::Single => "Single",
            LayoutMode::Mirror => "Mirror",
            LayoutMode::Extend => "Extend",
        }
    }
}

/// A supported display mode.
#[derive(Debug, Clone, Copy)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
}

/// A display device.
pub struct Display {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub bpp: u8,
    pub refresh_hz: u32,
    pub connected: bool,
    pub primary: bool,
    pub brightness: u8,
    pub connector: ConnectorType,
    pub dpms: DpmsState,
    pub modes: Vec<DisplayMode>,
}

// ---------------------------------------------------------------------------
// Common display modes
// ---------------------------------------------------------------------------

fn standard_modes() -> Vec<DisplayMode> {
    let mut v = Vec::new();
    let modes: [(u32, u32, u32); 7] = [
        (640, 480, 60),
        (800, 600, 60),
        (1024, 768, 60),
        (1280, 720, 60),
        (1920, 1080, 60),
        (2560, 1440, 60),
        (3840, 2160, 30),
    ];
    for (w, h, r) in modes {
        v.push(DisplayMode { width: w, height: h, refresh_hz: r });
    }
    v
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static MODE_CHANGES: AtomicU32 = AtomicU32::new(0);
static BRIGHTNESS_CHANGES: AtomicU32 = AtomicU32::new(0);
static DPMS_CHANGES: AtomicU32 = AtomicU32::new(0);
static DISPLAY_COUNT: AtomicU32 = AtomicU32::new(0);
static IDLE_TIMEOUT_S: AtomicU64 = AtomicU64::new(600); // 10 min default

struct DisplayMgrState {
    displays: Vec<Display>,
    layout: LayoutMode,
}

impl DisplayMgrState {
    const fn new() -> Self {
        Self {
            displays: Vec::new(),
            layout: LayoutMode::Single,
        }
    }
}

static STATE: Mutex<DisplayMgrState> = Mutex::new(DisplayMgrState::new());

// ---------------------------------------------------------------------------
// Display detection
// ---------------------------------------------------------------------------

/// Register a detected display. Called during boot.
pub fn register_display(
    id: u32,
    name: &str,
    width: u32,
    height: u32,
    connector: ConnectorType,
    primary: bool,
) {
    let mut state = STATE.lock();
    let disp = Display {
        id,
        name: String::from(name),
        width,
        height,
        bpp: 32,
        refresh_hz: 60,
        connected: true,
        primary,
        brightness: 80,
        connector,
        dpms: DpmsState::On,
        modes: standard_modes(),
    };
    state.displays.push(disp);
    DISPLAY_COUNT.store(state.displays.len() as u32, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Mode management
// ---------------------------------------------------------------------------

/// Set the resolution/refresh of a display. Returns Ok on success.
pub fn set_mode(display_id: u32, width: u32, height: u32, refresh: u32) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    let disp = state.displays.iter_mut().find(|d| d.id == display_id);
    match disp {
        Some(d) => {
            // Verify the mode is supported
            let valid = d.modes.iter().any(|m| m.width == width && m.height == height && m.refresh_hz == refresh);
            if !valid {
                return Err("unsupported mode");
            }
            d.width = width;
            d.height = height;
            d.refresh_hz = refresh;
            MODE_CHANGES.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        None => Err("display not found"),
    }
}

/// List available modes for a display.
pub fn list_modes(display_id: u32) -> Vec<DisplayMode> {
    let state = STATE.lock();
    match state.displays.iter().find(|d| d.id == display_id) {
        Some(d) => d.modes.clone(),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Brightness control
// ---------------------------------------------------------------------------

/// Set brightness (0-100) for a display.
pub fn set_brightness(display_id: u32, percent: u8) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    let disp = state.displays.iter_mut().find(|d| d.id == display_id);
    match disp {
        Some(d) => {
            d.brightness = if percent > 100 { 100 } else { percent };
            BRIGHTNESS_CHANGES.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        None => Err("display not found"),
    }
}

/// Get brightness for a display.
pub fn get_brightness(display_id: u32) -> u8 {
    let state = STATE.lock();
    match state.displays.iter().find(|d| d.id == display_id) {
        Some(d) => d.brightness,
        None => 0,
    }
}

/// Simulated auto-brightness based on an ambient light level (0-100).
pub fn auto_brightness(display_id: u32, ambient: u8) -> Result<(), &'static str> {
    // Map ambient 0..100 to brightness 20..100 linearly using integer math
    let brightness = 20 + (ambient as u32 * 80 / 100) as u8;
    set_brightness(display_id, brightness)
}

// ---------------------------------------------------------------------------
// Multi-monitor
// ---------------------------------------------------------------------------

/// Get the current layout mode.
pub fn get_layout() -> LayoutMode {
    let state = STATE.lock();
    state.layout
}

/// Set the multi-monitor layout mode.
pub fn set_layout(mode: LayoutMode) {
    let mut state = STATE.lock();
    state.layout = mode;
}

/// Set which display is primary.
pub fn set_primary(display_id: u32) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    let found = state.displays.iter().any(|d| d.id == display_id);
    if !found {
        return Err("display not found");
    }
    for d in state.displays.iter_mut() {
        d.primary = d.id == display_id;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// DPMS (Display Power Management)
// ---------------------------------------------------------------------------

/// Set DPMS state for a display.
pub fn dpms_set(display_id: u32, dpms: DpmsState) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    let disp = state.displays.iter_mut().find(|d| d.id == display_id);
    match disp {
        Some(d) => {
            d.dpms = dpms;
            DPMS_CHANGES.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        None => Err("display not found"),
    }
}

/// Get DPMS state for a display.
pub fn dpms_get(display_id: u32) -> DpmsState {
    let state = STATE.lock();
    match state.displays.iter().find(|d| d.id == display_id) {
        Some(d) => d.dpms,
        None => DpmsState::Off,
    }
}

/// Set the idle timeout in seconds before auto-standby.
pub fn set_idle_timeout(seconds: u64) {
    IDLE_TIMEOUT_S.store(seconds, Ordering::SeqCst);
}

/// Get the idle timeout in seconds.
pub fn get_idle_timeout() -> u64 {
    IDLE_TIMEOUT_S.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

/// List all displays as a formatted string.
pub fn list_displays() -> String {
    let state = STATE.lock();
    if state.displays.is_empty() {
        return String::from("No displays detected.");
    }
    let mut s = String::from("Displays:\n");
    for d in &state.displays {
        s.push_str(&format!(
            "  [{}] {} — {}x{}@{}Hz {} {} brightness={}% dpms={}\n",
            d.id,
            d.name,
            d.width, d.height, d.refresh_hz,
            d.connector.as_str(),
            if d.primary { "(primary)" } else { "" },
            d.brightness,
            d.dpms.as_str(),
        ));
    }
    s.push_str(&format!("  Layout: {}\n", state.layout.as_str()));
    s
}

/// Get info for a specific display.
pub fn display_info(id: u32) -> String {
    let state = STATE.lock();
    match state.displays.iter().find(|d| d.id == id) {
        Some(d) => {
            let mut s = format!(
                "Display {} — {}:\n  Resolution: {}x{}\n  Refresh: {} Hz\n  \
                 BPP: {}\n  Connector: {}\n  Primary: {}\n  \
                 Brightness: {}%\n  DPMS: {}\n  Connected: {}\n  Modes:\n",
                d.id, d.name,
                d.width, d.height,
                d.refresh_hz,
                d.bpp,
                d.connector.as_str(),
                d.primary,
                d.brightness,
                d.dpms.as_str(),
                d.connected,
            );
            for m in &d.modes {
                s.push_str(&format!("    {}x{}@{}Hz\n", m.width, m.height, m.refresh_hz));
            }
            s
        }
        None => format!("Display {} not found.", id),
    }
}

// ---------------------------------------------------------------------------
// Init & info/stats
// ---------------------------------------------------------------------------

/// Initialize the display manager and detect available displays.
pub fn init() {
    MODE_CHANGES.store(0, Ordering::SeqCst);
    BRIGHTNESS_CHANGES.store(0, Ordering::SeqCst);
    DPMS_CHANGES.store(0, Ordering::SeqCst);

    // Register a default display (QEMU VGA / UEFI GOP)
    register_display(0, "QEMU VGA", 1024, 768, ConnectorType::VGA, true);
}

/// General info string.
pub fn display_mgr_info() -> String {
    let count = DISPLAY_COUNT.load(Ordering::Relaxed);
    let state = STATE.lock();
    let timeout = IDLE_TIMEOUT_S.load(Ordering::Relaxed);
    format!(
        "Display Manager:\n  Displays: {}\n  Layout: {}\n  Idle timeout: {}s",
        count,
        state.layout.as_str(),
        timeout,
    )
}

/// Statistics string.
pub fn display_mgr_stats() -> String {
    let mc = MODE_CHANGES.load(Ordering::Relaxed);
    let bc = BRIGHTNESS_CHANGES.load(Ordering::Relaxed);
    let dc = DPMS_CHANGES.load(Ordering::Relaxed);
    let count = DISPLAY_COUNT.load(Ordering::Relaxed);
    format!(
        "Display Manager Stats:\n  Registered displays: {}\n  Mode changes: {}\n  \
         Brightness changes: {}\n  DPMS changes: {}",
        count, mc, bc, dc,
    )
}
