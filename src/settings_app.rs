/// Unified system settings application for MerlionOS.
/// One-stop configuration for display, network, sound,
/// power, users, time, and system information.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Settings categories
// ---------------------------------------------------------------------------

/// Settings panel category.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Category {
    Display,
    Network,
    Sound,
    Power,
    Users,
    DateTime,
    Security,
    Keyboard,
    Mouse,
    About,
}

impl Category {
    fn as_str(&self) -> &'static str {
        match self {
            Category::Display => "Display",
            Category::Network => "Network",
            Category::Sound => "Sound",
            Category::Power => "Power",
            Category::Users => "Users",
            Category::DateTime => "Date & Time",
            Category::Security => "Security",
            Category::Keyboard => "Keyboard",
            Category::Mouse => "Mouse",
            Category::About => "About",
        }
    }

    fn from_str(s: &str) -> Option<Category> {
        match s {
            "display" => Some(Category::Display),
            "network" => Some(Category::Network),
            "sound" => Some(Category::Sound),
            "power" => Some(Category::Power),
            "users" => Some(Category::Users),
            "datetime" | "date" | "time" => Some(Category::DateTime),
            "security" => Some(Category::Security),
            "keyboard" | "kbd" => Some(Category::Keyboard),
            "mouse" => Some(Category::Mouse),
            "about" => Some(Category::About),
            _ => None,
        }
    }
}

const ALL_CATEGORIES: [Category; 10] = [
    Category::Display, Category::Network, Category::Sound,
    Category::Power, Category::Users, Category::DateTime,
    Category::Security, Category::Keyboard, Category::Mouse,
    Category::About,
];

// ---------------------------------------------------------------------------
// Settings values
// ---------------------------------------------------------------------------

/// Power profile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PowerProfile {
    Performance,
    Balanced,
    PowerSave,
}

impl PowerProfile {
    fn as_str(&self) -> &'static str {
        match self {
            PowerProfile::Performance => "performance",
            PowerProfile::Balanced => "balanced",
            PowerProfile::PowerSave => "powersave",
        }
    }
}

/// Lid close action.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LidAction {
    Sleep,
    Hibernate,
    Shutdown,
    Nothing,
}

impl LidAction {
    fn as_str(&self) -> &'static str {
        match self {
            LidAction::Sleep => "sleep",
            LidAction::Hibernate => "hibernate",
            LidAction::Shutdown => "shutdown",
            LidAction::Nothing => "nothing",
        }
    }
}

/// Scroll direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollDir {
    Natural,
    Traditional,
}

impl ScrollDir {
    fn as_str(&self) -> &'static str {
        match self {
            ScrollDir::Natural => "natural",
            ScrollDir::Traditional => "traditional",
        }
    }
}

/// Date format preference.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DateFormat {
    YmdDash,   // 2026-03-17
    DmySlash,  // 17/03/2026
    MdySlash,  // 03/17/2026
}

impl DateFormat {
    fn as_str(&self) -> &'static str {
        match self {
            DateFormat::YmdDash => "YYYY-MM-DD",
            DateFormat::DmySlash => "DD/MM/YYYY",
            DateFormat::MdySlash => "MM/DD/YYYY",
        }
    }
}

// ---------------------------------------------------------------------------
// Per-category settings structs
// ---------------------------------------------------------------------------

struct DisplaySettings {
    width: u32,
    height: u32,
    brightness: u8,         // 0-100
    refresh_hz: u32,
    multi_monitor: bool,
}

struct NetworkSettings {
    wifi_enabled: bool,
    ethernet_enabled: bool,
    vpn_enabled: bool,
    proxy_enabled: bool,
    dns_server: String,
}

struct SoundSettings {
    output_device: String,
    output_volume: u8,      // 0-100
    input_device: String,
    mic_level: u8,          // 0-100
}

struct PowerSettings {
    profile: PowerProfile,
    sleep_timeout_s: u32,
    lid_action: LidAction,
    battery_pct: u8,        // 0-100
}

struct UserEntry {
    name: String,
    uid: u32,
    groups: Vec<String>,
}

struct UserSettings {
    users: Vec<UserEntry>,
}

struct DateTimeSettings {
    timezone: String,
    ntp_server: String,
    ntp_enabled: bool,
    date_format: DateFormat,
}

struct SecuritySettings {
    firewall_enabled: bool,
    seccomp_enabled: bool,
    capabilities_enabled: bool,
}

struct KeyboardSettings {
    layout: String,
    repeat_rate_ms: u32,
    input_method: String,
}

struct MouseSettings {
    speed: u8,              // 1-10
    acceleration: bool,
    scroll_dir: ScrollDir,
    touchpad_sensitivity: u8, // 1-10
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static PANEL_OPENS: AtomicU32 = AtomicU32::new(0);
static SAVES: AtomicU32 = AtomicU32::new(0);
static LOADS: AtomicU32 = AtomicU32::new(0);
static SEARCH_COUNT: AtomicU32 = AtomicU32::new(0);

struct SettingsState {
    current_panel: Category,
    display: DisplaySettings,
    network: NetworkSettings,
    sound: SoundSettings,
    power: PowerSettings,
    users: UserSettings,
    datetime: DateTimeSettings,
    security: SecuritySettings,
    keyboard: KeyboardSettings,
    mouse: MouseSettings,
    pending_changes: bool,
}

impl SettingsState {
    const fn new() -> Self {
        Self {
            current_panel: Category::About,
            display: DisplaySettings {
                width: 1920,
                height: 1080,
                brightness: 80,
                refresh_hz: 60,
                multi_monitor: false,
            },
            network: NetworkSettings {
                wifi_enabled: true,
                ethernet_enabled: true,
                vpn_enabled: false,
                proxy_enabled: false,
                dns_server: String::new(),
            },
            sound: SoundSettings {
                output_device: String::new(),
                output_volume: 75,
                input_device: String::new(),
                mic_level: 50,
            },
            power: PowerSettings {
                profile: PowerProfile::Balanced,
                sleep_timeout_s: 600,
                lid_action: LidAction::Sleep,
                battery_pct: 100,
            },
            users: UserSettings { users: Vec::new() },
            datetime: DateTimeSettings {
                timezone: String::new(),
                ntp_server: String::new(),
                ntp_enabled: true,
                date_format: DateFormat::YmdDash,
            },
            security: SecuritySettings {
                firewall_enabled: true,
                seccomp_enabled: true,
                capabilities_enabled: true,
            },
            keyboard: KeyboardSettings {
                layout: String::new(),
                repeat_rate_ms: 30,
                input_method: String::new(),
            },
            mouse: MouseSettings {
                speed: 5,
                acceleration: true,
                scroll_dir: ScrollDir::Natural,
                touchpad_sensitivity: 5,
            },
            pending_changes: false,
        }
    }
}

static STATE: Mutex<SettingsState> = Mutex::new(SettingsState::new());

const SETTINGS_PATH: &str = "/etc/settings.conf";

// ---------------------------------------------------------------------------
// Panel display
// ---------------------------------------------------------------------------

fn show_display(s: &DisplaySettings) {
    crate::println!("  Resolution:    {}x{}", s.width, s.height);
    crate::println!("  Brightness:    {}%", s.brightness);
    crate::println!("  Refresh rate:  {} Hz", s.refresh_hz);
    crate::println!("  Multi-monitor: {}", if s.multi_monitor { "yes" } else { "no" });
}

fn show_network(s: &NetworkSettings) {
    crate::println!("  WiFi:     {}", if s.wifi_enabled { "on" } else { "off" });
    crate::println!("  Ethernet: {}", if s.ethernet_enabled { "on" } else { "off" });
    crate::println!("  VPN:      {}", if s.vpn_enabled { "on" } else { "off" });
    crate::println!("  Proxy:    {}", if s.proxy_enabled { "on" } else { "off" });
    crate::println!("  DNS:      {}", if s.dns_server.is_empty() { "auto" } else { &s.dns_server });
}

fn show_sound(s: &SoundSettings) {
    crate::println!("  Output:   {} ({}%)",
        if s.output_device.is_empty() { "default" } else { &s.output_device },
        s.output_volume);
    crate::println!("  Input:    {} ({}%)",
        if s.input_device.is_empty() { "default" } else { &s.input_device },
        s.mic_level);
}

fn show_power(s: &PowerSettings) {
    crate::println!("  Profile:  {}", s.profile.as_str());
    crate::println!("  Sleep:    {} sec", s.sleep_timeout_s);
    crate::println!("  Lid:      {}", s.lid_action.as_str());
    crate::println!("  Battery:  {}%", s.battery_pct);
}

fn show_users(s: &UserSettings) {
    if s.users.is_empty() {
        crate::println!("  (no users configured)");
    } else {
        for u in &s.users {
            let groups_str = if u.groups.is_empty() {
                String::from("(none)")
            } else {
                let mut gs = String::new();
                for (i, g) in u.groups.iter().enumerate() {
                    if i > 0 { gs.push_str(", "); }
                    gs.push_str(g);
                }
                gs
            };
            crate::println!("  {} (uid={}) groups=[{}]", u.name, u.uid, groups_str);
        }
    }
}

fn show_datetime(s: &DateTimeSettings) {
    crate::println!("  Timezone: {}",
        if s.timezone.is_empty() { "UTC" } else { &s.timezone });
    crate::println!("  NTP:      {} ({})",
        if s.ntp_enabled { "on" } else { "off" },
        if s.ntp_server.is_empty() { "pool.ntp.org" } else { &s.ntp_server });
    crate::println!("  Format:   {}", s.date_format.as_str());
}

fn show_security(s: &SecuritySettings) {
    crate::println!("  Firewall:     {}", if s.firewall_enabled { "on" } else { "off" });
    crate::println!("  Seccomp:      {}", if s.seccomp_enabled { "on" } else { "off" });
    crate::println!("  Capabilities: {}", if s.capabilities_enabled { "on" } else { "off" });
}

fn show_keyboard(s: &KeyboardSettings) {
    crate::println!("  Layout:       {}",
        if s.layout.is_empty() { "us" } else { &s.layout });
    crate::println!("  Repeat rate:  {} ms", s.repeat_rate_ms);
    crate::println!("  Input method: {}",
        if s.input_method.is_empty() { "none" } else { &s.input_method });
}

fn show_mouse(s: &MouseSettings) {
    crate::println!("  Speed:        {}/10", s.speed);
    crate::println!("  Acceleration: {}", if s.acceleration { "on" } else { "off" });
    crate::println!("  Scroll:       {}", s.scroll_dir.as_str());
    crate::println!("  Touchpad:     {}/10", s.touchpad_sensitivity);
}

fn show_about() {
    let heap = crate::allocator::stats();
    let mem = crate::memory::stats();
    let (h, m, s) = crate::timer::uptime_hms();
    crate::println!("  OS:      {} {}", crate::version::NAME, crate::version::VERSION);
    crate::println!("  Kernel:  {}", crate::version::full());
    crate::println!("  Arch:    {}", crate::version::ARCH);
    crate::println!("  CPU:     {} core(s)", crate::smp::online_cpus());
    crate::println!("  Memory:  {} KiB / {} KiB",
        mem.allocated_frames * 4, mem.total_usable_bytes / 1024);
    crate::println!("  Heap:    {} / {} bytes", heap.used, heap.total);
    crate::println!("  Uptime:  {:02}:{:02}:{:02}", h, m, s);
}

// ---------------------------------------------------------------------------
// Search across categories
// ---------------------------------------------------------------------------

/// Search for a keyword across all settings categories.
pub fn search_settings(query: &str) -> Vec<(Category, &'static str)> {
    SEARCH_COUNT.fetch_add(1, Ordering::Relaxed);
    let q = query.to_ascii_lowercase();
    let mut results = Vec::new();

    let entries: &[(Category, &[&str])] = &[
        (Category::Display, &["resolution", "brightness", "refresh", "monitor", "display"]),
        (Category::Network, &["wifi", "ethernet", "vpn", "proxy", "dns", "network"]),
        (Category::Sound, &["volume", "output", "input", "mic", "sound", "audio"]),
        (Category::Power, &["battery", "sleep", "lid", "profile", "power", "performance"]),
        (Category::Users, &["user", "group", "password", "uid"]),
        (Category::DateTime, &["timezone", "ntp", "date", "time", "format"]),
        (Category::Security, &["firewall", "seccomp", "capabilities", "security"]),
        (Category::Keyboard, &["layout", "shortcut", "repeat", "keyboard", "input method"]),
        (Category::Mouse, &["speed", "acceleration", "scroll", "touchpad", "mouse"]),
        (Category::About, &["version", "kernel", "cpu", "memory", "uptime", "about"]),
    ];

    for (cat, keywords) in entries {
        for kw in *keywords {
            if kw.contains(q.as_str()) {
                results.push((*cat, *kw));
                break;
            }
        }
    }
    results
}

// ---------------------------------------------------------------------------
// Persistence — TOML-like format
// ---------------------------------------------------------------------------

/// Serialize settings to TOML-like string.
fn serialize(st: &SettingsState) -> String {
    let mut out = String::new();
    out.push_str("# MerlionOS Settings\n\n");

    out.push_str("[display]\n");
    out.push_str(&format!("width = {}\n", st.display.width));
    out.push_str(&format!("height = {}\n", st.display.height));
    out.push_str(&format!("brightness = {}\n", st.display.brightness));
    out.push_str(&format!("refresh_hz = {}\n", st.display.refresh_hz));
    out.push_str(&format!("multi_monitor = {}\n\n", st.display.multi_monitor));

    out.push_str("[network]\n");
    out.push_str(&format!("wifi = {}\n", st.network.wifi_enabled));
    out.push_str(&format!("ethernet = {}\n", st.network.ethernet_enabled));
    out.push_str(&format!("vpn = {}\n", st.network.vpn_enabled));
    out.push_str(&format!("proxy = {}\n", st.network.proxy_enabled));
    out.push_str(&format!("dns = \"{}\"\n\n", st.network.dns_server));

    out.push_str("[sound]\n");
    out.push_str(&format!("output_device = \"{}\"\n", st.sound.output_device));
    out.push_str(&format!("output_volume = {}\n", st.sound.output_volume));
    out.push_str(&format!("input_device = \"{}\"\n", st.sound.input_device));
    out.push_str(&format!("mic_level = {}\n\n", st.sound.mic_level));

    out.push_str("[power]\n");
    out.push_str(&format!("profile = \"{}\"\n", st.power.profile.as_str()));
    out.push_str(&format!("sleep_timeout = {}\n", st.power.sleep_timeout_s));
    out.push_str(&format!("lid_action = \"{}\"\n", st.power.lid_action.as_str()));
    out.push_str(&format!("battery = {}\n\n", st.power.battery_pct));

    out.push_str("[datetime]\n");
    out.push_str(&format!("timezone = \"{}\"\n", st.datetime.timezone));
    out.push_str(&format!("ntp_server = \"{}\"\n", st.datetime.ntp_server));
    out.push_str(&format!("ntp_enabled = {}\n", st.datetime.ntp_enabled));
    out.push_str(&format!("date_format = \"{}\"\n\n", st.datetime.date_format.as_str()));

    out.push_str("[security]\n");
    out.push_str(&format!("firewall = {}\n", st.security.firewall_enabled));
    out.push_str(&format!("seccomp = {}\n", st.security.seccomp_enabled));
    out.push_str(&format!("capabilities = {}\n\n", st.security.capabilities_enabled));

    out.push_str("[keyboard]\n");
    out.push_str(&format!("layout = \"{}\"\n", st.keyboard.layout));
    out.push_str(&format!("repeat_rate = {}\n", st.keyboard.repeat_rate_ms));
    out.push_str(&format!("input_method = \"{}\"\n\n", st.keyboard.input_method));

    out.push_str("[mouse]\n");
    out.push_str(&format!("speed = {}\n", st.mouse.speed));
    out.push_str(&format!("acceleration = {}\n", st.mouse.acceleration));
    out.push_str(&format!("scroll = \"{}\"\n", st.mouse.scroll_dir.as_str()));
    out.push_str(&format!("touchpad_sensitivity = {}\n", st.mouse.touchpad_sensitivity));

    out
}

/// Parse a simple "key = value" line, returning (key, value).
fn parse_kv(line: &str) -> Option<(&str, &str)> {
    if let Some(eq) = line.find('=') {
        let key = line[..eq].trim();
        let val = line[eq + 1..].trim().trim_matches('"');
        Some((key, val))
    } else {
        None
    }
}

/// Parse a boolean value.
fn parse_bool(v: &str) -> bool {
    v == "true" || v == "1" || v == "yes"
}

/// Parse a u32 value.
fn parse_u32(v: &str) -> u32 {
    let mut n = 0u32;
    for b in v.bytes() {
        if b >= b'0' && b <= b'9' {
            n = n.wrapping_mul(10).wrapping_add((b - b'0') as u32);
        }
    }
    n
}

/// Parse a u8 value.
fn parse_u8(v: &str) -> u8 {
    let n = parse_u32(v);
    if n > 255 { 255 } else { n as u8 }
}

/// Deserialize settings from TOML-like string.
fn deserialize(st: &mut SettingsState, content: &str) {
    let mut section = "";
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = &trimmed[1..trimmed.len() - 1];
            continue;
        }
        if let Some((key, val)) = parse_kv(trimmed) {
            match section {
                "display" => match key {
                    "width" => st.display.width = parse_u32(val),
                    "height" => st.display.height = parse_u32(val),
                    "brightness" => st.display.brightness = parse_u8(val),
                    "refresh_hz" => st.display.refresh_hz = parse_u32(val),
                    "multi_monitor" => st.display.multi_monitor = parse_bool(val),
                    _ => {}
                },
                "network" => match key {
                    "wifi" => st.network.wifi_enabled = parse_bool(val),
                    "ethernet" => st.network.ethernet_enabled = parse_bool(val),
                    "vpn" => st.network.vpn_enabled = parse_bool(val),
                    "proxy" => st.network.proxy_enabled = parse_bool(val),
                    "dns" => st.network.dns_server = String::from(val),
                    _ => {}
                },
                "sound" => match key {
                    "output_device" => st.sound.output_device = String::from(val),
                    "output_volume" => st.sound.output_volume = parse_u8(val),
                    "input_device" => st.sound.input_device = String::from(val),
                    "mic_level" => st.sound.mic_level = parse_u8(val),
                    _ => {}
                },
                "power" => match key {
                    "profile" => st.power.profile = match val {
                        "performance" => PowerProfile::Performance,
                        "powersave" => PowerProfile::PowerSave,
                        _ => PowerProfile::Balanced,
                    },
                    "sleep_timeout" => st.power.sleep_timeout_s = parse_u32(val),
                    "lid_action" => st.power.lid_action = match val {
                        "hibernate" => LidAction::Hibernate,
                        "shutdown" => LidAction::Shutdown,
                        "nothing" => LidAction::Nothing,
                        _ => LidAction::Sleep,
                    },
                    "battery" => st.power.battery_pct = parse_u8(val),
                    _ => {}
                },
                "datetime" => match key {
                    "timezone" => st.datetime.timezone = String::from(val),
                    "ntp_server" => st.datetime.ntp_server = String::from(val),
                    "ntp_enabled" => st.datetime.ntp_enabled = parse_bool(val),
                    "date_format" => st.datetime.date_format = match val {
                        "DD/MM/YYYY" => DateFormat::DmySlash,
                        "MM/DD/YYYY" => DateFormat::MdySlash,
                        _ => DateFormat::YmdDash,
                    },
                    _ => {}
                },
                "security" => match key {
                    "firewall" => st.security.firewall_enabled = parse_bool(val),
                    "seccomp" => st.security.seccomp_enabled = parse_bool(val),
                    "capabilities" => st.security.capabilities_enabled = parse_bool(val),
                    _ => {}
                },
                "keyboard" => match key {
                    "layout" => st.keyboard.layout = String::from(val),
                    "repeat_rate" => st.keyboard.repeat_rate_ms = parse_u32(val),
                    "input_method" => st.keyboard.input_method = String::from(val),
                    _ => {}
                },
                "mouse" => match key {
                    "speed" => st.mouse.speed = parse_u8(val),
                    "acceleration" => st.mouse.acceleration = parse_bool(val),
                    "scroll" => st.mouse.scroll_dir = match val {
                        "traditional" => ScrollDir::Traditional,
                        _ => ScrollDir::Natural,
                    },
                    "touchpad_sensitivity" => st.mouse.touchpad_sensitivity = parse_u8(val),
                    _ => {}
                },
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the settings application with defaults.
pub fn init() {
    let mut st = STATE.lock();
    // Populate default users
    st.users.users.push(UserEntry {
        name: String::from("root"),
        uid: 0,
        groups: {
            let mut g = Vec::new();
            g.push(String::from("root"));
            g.push(String::from("wheel"));
            g
        },
    });
    st.users.users.push(UserEntry {
        name: String::from("merlion"),
        uid: 1000,
        groups: {
            let mut g = Vec::new();
            g.push(String::from("users"));
            g.push(String::from("sudo"));
            g
        },
    });
    // Default device names
    st.sound.output_device = String::from("HDA Intel PCH");
    st.sound.input_device = String::from("HDA Intel Mic");
    st.keyboard.layout = String::from("us");
    st.datetime.timezone = String::from("Asia/Singapore");
    st.datetime.ntp_server = String::from("pool.ntp.org");
    st.network.dns_server = String::from("8.8.8.8");
}

/// Open a specific settings panel and display it.
pub fn open_panel(category: &str) {
    PANEL_OPENS.fetch_add(1, Ordering::Relaxed);
    let cat = match Category::from_str(category) {
        Some(c) => c,
        None => {
            crate::println!("Unknown settings panel: '{}'", category);
            crate::println!("Available: display, network, sound, power, users, datetime, security, keyboard, mouse, about");
            return;
        }
    };
    let mut st = STATE.lock();
    st.current_panel = cat;

    crate::println!("Settings > {}", cat.as_str());
    crate::println!("───────────────────────────────────");
    match cat {
        Category::Display => show_display(&st.display),
        Category::Network => show_network(&st.network),
        Category::Sound => show_sound(&st.sound),
        Category::Power => show_power(&st.power),
        Category::Users => show_users(&st.users),
        Category::DateTime => show_datetime(&st.datetime),
        Category::Security => show_security(&st.security),
        Category::Keyboard => show_keyboard(&st.keyboard),
        Category::Mouse => show_mouse(&st.mouse),
        Category::About => {
            drop(st);
            show_about();
        }
    }
}

/// Show the main settings overview — all categories.
pub fn show_overview() {
    crate::println!("System Settings (Super+I)");
    crate::println!("════════════════════════════════════");
    for cat in &ALL_CATEGORIES {
        crate::println!("  [{}]", cat.as_str());
    }
    crate::println!();
    crate::println!("Use: settings <panel> to open a panel");
    crate::println!("      settings-save  to persist to {}", SETTINGS_PATH);
    crate::println!("      settings-load  to reload from {}", SETTINGS_PATH);
}

/// Return info summary string.
pub fn settings_info() -> String {
    let st = STATE.lock();
    format!(
        "Settings: panel={} display={}x{} power={} kbd={} pending={}",
        st.current_panel.as_str(),
        st.display.width,
        st.display.height,
        st.power.profile.as_str(),
        if st.keyboard.layout.is_empty() { "us" } else { &st.keyboard.layout },
        st.pending_changes,
    )
}

/// Save current settings to VFS.
pub fn save_settings() {
    SAVES.fetch_add(1, Ordering::Relaxed);
    let st = STATE.lock();
    let data = serialize(&st);
    drop(st);
    let _ = crate::vfs::write(SETTINGS_PATH, &data);
    STATE.lock().pending_changes = false;
    crate::println!("Settings saved to {}", SETTINGS_PATH);
}

/// Load settings from VFS.
pub fn load_settings() {
    LOADS.fetch_add(1, Ordering::Relaxed);
    let data = match crate::vfs::cat(SETTINGS_PATH) {
        Ok(d) => d,
        Err(_) => {
            crate::println!("No settings file at {} (using defaults)", SETTINGS_PATH);
            return;
        }
    };
    let mut st = STATE.lock();
    deserialize(&mut st, &data);
    st.pending_changes = false;
    drop(st);
    crate::println!("Settings loaded from {}", SETTINGS_PATH);
}
