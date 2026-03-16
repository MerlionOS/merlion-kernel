/// Dialog and form builder for MerlionOS GUI.
/// Provides high-level APIs for common UI patterns: message boxes,
/// file browsers, settings panels, and data forms.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

// ── Form builder ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum FieldType {
    Text,
    Number,
    Password,
    Select(Vec<String>),
    Toggle,
}

#[derive(Debug, Clone)]
pub struct FormField {
    pub name: String,
    pub field_type: FieldType,
    pub value: String,
    pub required: bool,
}

pub struct Form {
    pub title: String,
    pub fields: Vec<FormField>,
    pub submitted: bool,
    pub values: Vec<(String, String)>,
}

impl Form {
    pub fn new(title: &str) -> Self {
        Self {
            title: String::from(title),
            fields: Vec::new(),
            submitted: false,
            values: Vec::new(),
        }
    }

    pub fn add_field(&mut self, name: &str, ftype: FieldType, required: bool) {
        let default_val = match &ftype {
            FieldType::Toggle => String::from("false"),
            FieldType::Select(opts) => {
                if let Some(first) = opts.first() {
                    first.clone()
                } else {
                    String::new()
                }
            }
            _ => String::new(),
        };
        self.fields.push(FormField {
            name: String::from(name),
            field_type: ftype,
            value: default_val,
            required,
        });
    }

    pub fn render(&self) -> String {
        let mut out = format!("+--- {} ---+\n", self.title);
        let width = 40usize;
        for field in &self.fields {
            let req = if field.required { "*" } else { " " };
            match &field.field_type {
                FieldType::Text => {
                    out.push_str(&format!("{} {}: [{}]\n", req, field.name, field.value));
                }
                FieldType::Number => {
                    out.push_str(&format!("{} {} (num): [{}]\n", req, field.name, field.value));
                }
                FieldType::Password => {
                    let masked: String = field.value.chars().map(|_| '*').collect();
                    out.push_str(&format!("{} {}: [{}]\n", req, field.name, masked));
                }
                FieldType::Select(opts) => {
                    out.push_str(&format!("{} {}: ", req, field.name));
                    for (i, opt) in opts.iter().enumerate() {
                        if *opt == field.value {
                            out.push_str(&format!("({})", opt));
                        } else {
                            out.push_str(&format!(" {} ", opt));
                        }
                        if i + 1 < opts.len() {
                            out.push('|');
                        }
                    }
                    out.push('\n');
                }
                FieldType::Toggle => {
                    let on = field.value == "true";
                    out.push_str(&format!("{} {}: [{}]\n", req, field.name,
                        if on { "ON " } else { "OFF" }));
                }
            }
        }
        // Footer line
        for _ in 0..width {
            out.push('-');
        }
        out.push('\n');
        if self.submitted {
            out.push_str("  [Submitted]\n");
        } else {
            out.push_str("  [ Submit ]  [ Cancel ]\n");
        }
        out
    }

    pub fn validate(&self) -> Result<(), String> {
        for field in &self.fields {
            if field.required && field.value.is_empty() {
                return Err(format!("Field '{}' is required", field.name));
            }
            if let FieldType::Number = field.field_type {
                if !field.value.is_empty() {
                    let valid = field.value.chars().all(|c| c.is_ascii_digit() || c == '-');
                    if !valid {
                        return Err(format!("Field '{}' must be a number", field.name));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn get_values(&self) -> Vec<(String, String)> {
        self.fields.iter().map(|f| (f.name.clone(), f.value.clone())).collect()
    }
}

// ── File browser ────────────────────────────────────────────────────────────

pub fn file_browser(start_path: &str) -> String {
    // Render a VFS file listing with navigation cues
    let entries = crate::vfs::ls(start_path).unwrap_or_default();
    let mut out = format!("File Browser: {}\n", start_path);
    out.push_str("----------------------------------------\n");
    out.push_str("  [..] (parent directory)\n");
    for (name, kind) in &entries {
        out.push_str(&format!("  [{}] {}\n", kind, name));
    }
    if entries.is_empty() {
        out.push_str("  (empty directory)\n");
    }
    out.push_str("----------------------------------------\n");
    out.push_str(&format!("{} item(s)\n", entries.len()));
    out
}

pub fn file_picker(start_path: &str, extension: Option<&str>) -> String {
    let entries = crate::vfs::ls(start_path).unwrap_or_default();
    let mut out = format!("File Picker: {}", start_path);
    if let Some(ext) = extension {
        out.push_str(&format!("  (filter: *.{})", ext));
    }
    out.push('\n');
    out.push_str("----------------------------------------\n");
    out.push_str("  [..] (parent)\n");
    for (name, kind) in &entries {
        let show = match extension {
            Some(ext) => name.ends_with(ext) || *kind == 'd',
            None => true,
        };
        if show {
            out.push_str(&format!("  [{}] {}\n", kind, name));
        }
    }
    out.push_str("----------------------------------------\n");
    out.push_str("  [ Open ]  [ Cancel ]\n");
    out
}

// ── Settings panels ─────────────────────────────────────────────────────────

pub fn system_settings() -> String {
    let mut form = Form::new("System Settings");
    form.add_field("Hostname", FieldType::Text, true);
    form.add_field("Timezone", FieldType::Select(vec![
        String::from("UTC"), String::from("SGT"), String::from("EST"), String::from("PST"),
    ]), false);
    form.add_field("Verbose boot", FieldType::Toggle, false);
    form.add_field("Max processes", FieldType::Number, false);
    form.render()
}

pub fn network_settings() -> String {
    let mut form = Form::new("Network Settings");
    form.add_field("DHCP", FieldType::Toggle, false);
    form.add_field("IP Address", FieldType::Text, false);
    form.add_field("Subnet Mask", FieldType::Text, false);
    form.add_field("Gateway", FieldType::Text, false);
    form.add_field("DNS Server", FieldType::Text, false);
    form.render()
}

pub fn security_settings() -> String {
    let mut form = Form::new("Security Settings");
    form.add_field("Firewall", FieldType::Toggle, false);
    form.add_field("Log Level", FieldType::Select(vec![
        String::from("Error"), String::from("Warn"), String::from("Info"), String::from("Debug"),
    ]), false);
    form.add_field("Root password", FieldType::Password, true);
    form.add_field("SSH enabled", FieldType::Toggle, false);
    form.render()
}

pub fn display_settings() -> String {
    let mut form = Form::new("Display Settings");
    form.add_field("Resolution", FieldType::Select(vec![
        String::from("640x480"), String::from("800x600"), String::from("1024x768"),
    ]), false);
    form.add_field("Color depth", FieldType::Select(vec![
        String::from("16"), String::from("24"), String::from("32"),
    ]), false);
    form.add_field("VSync", FieldType::Toggle, false);
    form.add_field("Font size", FieldType::Number, false);
    form.render()
}

// ── Table renderer ──────────────────────────────────────────────────────────

pub fn render_table(headers: &[&str], rows: &[Vec<String>], col_widths: &[usize]) -> String {
    let mut out = String::new();
    let ncols = headers.len().min(col_widths.len());

    // Header
    out.push('|');
    for i in 0..ncols {
        let w = col_widths[i];
        let h = headers[i];
        out.push(' ');
        out.push_str(h);
        let pad = w.saturating_sub(h.len());
        for _ in 0..pad {
            out.push(' ');
        }
        out.push_str(" |");
    }
    out.push('\n');

    // Separator
    out.push('|');
    for i in 0..ncols {
        out.push('-');
        for _ in 0..col_widths[i] {
            out.push('-');
        }
        out.push_str("-|");
    }
    out.push('\n');

    // Rows
    for row in rows {
        out.push('|');
        for i in 0..ncols {
            let w = col_widths[i];
            let cell = if i < row.len() { &row[i] } else { "" };
            out.push(' ');
            let truncated: String = cell.chars().take(w).collect();
            out.push_str(&truncated);
            let pad = w.saturating_sub(truncated.len());
            for _ in 0..pad {
                out.push(' ');
            }
            out.push_str(" |");
        }
        out.push('\n');
    }
    out
}

// ── Notification system ─────────────────────────────────────────────────────

pub struct Notification {
    pub id: u32,
    pub title: String,
    pub message: String,
    pub level: NotifLevel,
    pub timestamp: u64,
    pub read: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum NotifLevel {
    Info,
    Warning,
    Error,
    Success,
}

static NOTIFICATIONS: Mutex<Vec<Notification>> = Mutex::new(Vec::new());
static NEXT_NOTIF_ID: AtomicU32 = AtomicU32::new(1);

pub fn notify(title: &str, msg: &str, level: NotifLevel) {
    let id = NEXT_NOTIF_ID.fetch_add(1, Ordering::SeqCst);
    let ts = crate::timer::ticks();
    NOTIFICATIONS.lock().push(Notification {
        id,
        title: String::from(title),
        message: String::from(msg),
        level,
        timestamp: ts,
        read: false,
    });
}

pub fn list_notifications() -> String {
    let notifs = NOTIFICATIONS.lock();
    if notifs.is_empty() {
        return String::from("No notifications.\n");
    }
    let mut out = format!("{} notification(s):\n", notifs.len());
    for n in notifs.iter().rev() {
        let level_str = match n.level {
            NotifLevel::Info => "INFO",
            NotifLevel::Warning => "WARN",
            NotifLevel::Error => " ERR",
            NotifLevel::Success => "  OK",
        };
        let read_mark = if n.read { " " } else { "*" };
        out.push_str(&format!(
            " {} [{}] #{} {} - {}\n",
            read_mark, level_str, n.id, n.title, n.message
        ));
    }
    let unread = notifs.iter().filter(|n| !n.read).count();
    out.push_str(&format!("({} unread)\n", unread));
    out
}

pub fn dismiss(id: u32) {
    let mut notifs = NOTIFICATIONS.lock();
    if let Some(n) = notifs.iter_mut().find(|n| n.id == id) {
        n.read = true;
    }
}

pub fn unread_count() -> usize {
    NOTIFICATIONS.lock().iter().filter(|n| !n.read).count()
}

// ── Init, demo ──────────────────────────────────────────────────────────────

pub fn init() {
    NOTIFICATIONS.lock().clear();
    NEXT_NOTIF_ID.store(1, Ordering::SeqCst);
}

pub fn dialog_demo() -> String {
    let mut out = String::from("=== Dialog & Form Demo ===\n\n");

    // Form demo
    let mut form = Form::new("User Registration");
    form.add_field("Username", FieldType::Text, true);
    form.add_field("Password", FieldType::Password, true);
    form.add_field("Age", FieldType::Number, false);
    form.add_field("Role", FieldType::Select(vec![
        String::from("User"), String::from("Admin"), String::from("Guest"),
    ]), false);
    form.add_field("Newsletter", FieldType::Toggle, false);
    out.push_str(&form.render());
    out.push('\n');

    // Validation demo
    match form.validate() {
        Ok(()) => out.push_str("Validation: PASS\n"),
        Err(e) => out.push_str(&format!("Validation: FAIL - {}\n", e)),
    }
    out.push('\n');

    // Settings demos
    out.push_str("--- System Settings ---\n");
    out.push_str(&system_settings());
    out.push('\n');

    out.push_str("--- Display Settings ---\n");
    out.push_str(&display_settings());
    out.push('\n');

    // Table demo
    out.push_str("--- Table Demo ---\n");
    let headers = &["PID", "Name", "Status", "CPU"];
    let rows = vec![
        vec![String::from("1"), String::from("kernel"), String::from("running"), String::from("12")],
        vec![String::from("2"), String::from("shell"), String::from("waiting"), String::from("3")],
        vec![String::from("3"), String::from("netd"), String::from("running"), String::from("8")],
    ];
    out.push_str(&render_table(headers, &rows, &[5, 10, 10, 5]));
    out.push('\n');

    // Notification demo
    init();
    notify("Boot", "System started successfully", NotifLevel::Success);
    notify("Network", "DHCP lease acquired", NotifLevel::Info);
    notify("Disk", "Low disk space on /", NotifLevel::Warning);
    out.push_str("--- Notifications ---\n");
    out.push_str(&list_notifications());
    dismiss(1);
    out.push_str(&format!("After dismiss: {} unread\n", unread_count()));

    // File browser demo
    out.push('\n');
    out.push_str("--- File Browser ---\n");
    out.push_str(&file_browser("/"));

    out
}
