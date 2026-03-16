/// GUI widget toolkit for MerlionOS.
/// Provides a component-based UI system with widgets, layout management,
/// event handling, and rendering to the framebuffer.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

// ── Geometry primitives ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x
            && p.y >= self.y
            && p.x < self.x + self.width as i32
            && p.y < self.y + self.height as i32
    }

    pub fn right(&self) -> i32 {
        self.x + self.width as i32
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height as i32
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const WHITE: Self = Self { r: 255, g: 255, b: 255 };
    pub const BLACK: Self = Self { r: 0, g: 0, b: 0 };
    pub const RED: Self = Self { r: 255, g: 0, b: 0 };
    pub const GREEN: Self = Self { r: 0, g: 200, b: 0 };
    pub const BLUE: Self = Self { r: 0, g: 100, b: 255 };
    pub const GRAY: Self = Self { r: 180, g: 180, b: 180 };
    pub const DARK_GRAY: Self = Self { r: 60, g: 60, b: 60 };
    pub const MERLION_GOLD: Self = Self { r: 255, g: 200, b: 50 };

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn to_u32(self) -> u32 {
        (self.r as u32) << 16 | (self.g as u32) << 8 | self.b as u32
    }
}

// ── Widget types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum WidgetType {
    Label { text: String, color: Color },
    Button { text: String, pressed: bool, on_click: Option<String> },
    TextInput { text: String, cursor: usize, max_len: usize, focused: bool },
    Checkbox { label: String, checked: bool },
    ProgressBar { value: u32, max: u32, color: Color },
    Panel { bg_color: Color, border: bool },
    List { items: Vec<String>, selected: Option<usize>, scroll_offset: usize },
    Image { width: u32, height: u32, pixels: Vec<u8> },
    Separator,
    Spacer(u32),
}

// ── Widget struct ───────────────────────────────────────────────────────────

pub struct Widget {
    pub id: u32,
    pub widget_type: WidgetType,
    pub bounds: Rect,
    pub visible: bool,
    pub enabled: bool,
    pub parent: Option<u32>,
    pub children: Vec<u32>,
    pub z_order: u16,
}

impl Widget {
    fn new(id: u32, widget_type: WidgetType) -> Self {
        Self {
            id,
            widget_type,
            bounds: Rect::new(0, 0, 100, 20),
            visible: true,
            enabled: true,
            parent: None,
            children: Vec::new(),
            z_order: 0,
        }
    }
}

// ── Layout engine ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Layout {
    Vertical { spacing: u32, padding: u32 },
    Horizontal { spacing: u32, padding: u32 },
    Grid { cols: u32, row_height: u32, col_width: u32, spacing: u32 },
    Absolute,
}

pub struct LayoutContainer {
    pub id: u32,
    pub layout: Layout,
    pub bounds: Rect,
    pub children: Vec<u32>,
}

pub fn layout_vertical(container: &LayoutContainer, widgets: &mut [Widget]) {
    let (spacing, padding) = match container.layout {
        Layout::Vertical { spacing, padding } => (spacing, padding),
        _ => (4, 4),
    };
    let mut y = container.bounds.y + padding as i32;
    let avail_w = container.bounds.width.saturating_sub(padding * 2);
    for wid in &container.children {
        for w in widgets.iter_mut() {
            if w.id == *wid && w.visible {
                w.bounds.x = container.bounds.x + padding as i32;
                w.bounds.y = y;
                w.bounds.width = avail_w;
                y += w.bounds.height as i32 + spacing as i32;
                break;
            }
        }
    }
}

pub fn layout_horizontal(container: &LayoutContainer, widgets: &mut [Widget]) {
    let (spacing, padding) = match container.layout {
        Layout::Horizontal { spacing, padding } => (spacing, padding),
        _ => (4, 4),
    };
    let mut x = container.bounds.x + padding as i32;
    let avail_h = container.bounds.height.saturating_sub(padding * 2);
    for wid in &container.children {
        for w in widgets.iter_mut() {
            if w.id == *wid && w.visible {
                w.bounds.x = x;
                w.bounds.y = container.bounds.y + padding as i32;
                w.bounds.height = avail_h;
                x += w.bounds.width as i32 + spacing as i32;
                break;
            }
        }
    }
}

pub fn layout_grid(container: &LayoutContainer, widgets: &mut [Widget]) {
    let (cols, row_h, col_w, spacing) = match container.layout {
        Layout::Grid { cols, row_height, col_width, spacing } => (cols, row_height, col_width, spacing),
        _ => return,
    };
    if cols == 0 { return; }
    for (i, wid) in container.children.iter().enumerate() {
        let col = (i as u32) % cols;
        let row = (i as u32) / cols;
        let cx = container.bounds.x + (col * (col_w + spacing)) as i32;
        let cy = container.bounds.y + (row * (row_h + spacing)) as i32;
        for w in widgets.iter_mut() {
            if w.id == *wid {
                w.bounds = Rect::new(cx, cy, col_w, row_h);
                break;
            }
        }
    }
}

// ── Event system ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum UiEvent {
    Click(Point),
    KeyPress(char),
    MouseMove(Point),
    FocusIn(u32),
    FocusOut(u32),
    Scroll(i32),
}

pub fn handle_event(event: UiEvent) -> Option<String> {
    let mut widgets = WIDGETS.lock();
    match event {
        UiEvent::Click(pt) => {
            // Find topmost widget at this point
            let mut hit: Option<(u32, u16)> = None;
            for w in widgets.iter() {
                if w.visible && w.enabled && w.bounds.contains(pt) {
                    match hit {
                        None => hit = Some((w.id, w.z_order)),
                        Some((_, z)) if w.z_order >= z => hit = Some((w.id, w.z_order)),
                        _ => {}
                    }
                }
            }
            if let Some((id, _)) = hit {
                for w in widgets.iter_mut() {
                    if w.id == id {
                        match &mut w.widget_type {
                            WidgetType::Button { pressed, on_click, .. } => {
                                *pressed = true;
                                return on_click.clone();
                            }
                            WidgetType::Checkbox { checked, .. } => {
                                *checked = !*checked;
                                return None;
                            }
                            WidgetType::TextInput { focused, .. } => {
                                *focused = true;
                                return None;
                            }
                            WidgetType::List { items, selected, .. } => {
                                if !items.is_empty() {
                                    let rel_y = (pt.y - w.bounds.y) as usize;
                                    let idx = rel_y / 16; // 16px per row
                                    if idx < items.len() {
                                        *selected = Some(idx);
                                    }
                                }
                                return None;
                            }
                            _ => return None,
                        }
                    }
                }
            }
            None
        }
        UiEvent::KeyPress(ch) => {
            // Deliver to focused text input
            for w in widgets.iter_mut() {
                if let WidgetType::TextInput { text, cursor, max_len, focused } = &mut w.widget_type {
                    if *focused {
                        if ch == '\x08' {
                            // Backspace
                            if *cursor > 0 {
                                *cursor -= 1;
                                text.remove(*cursor);
                            }
                        } else if text.len() < *max_len && ch >= ' ' {
                            text.insert(*cursor, ch);
                            *cursor += 1;
                        }
                        return None;
                    }
                }
            }
            None
        }
        UiEvent::MouseMove(_pt) => None,
        UiEvent::FocusIn(id) => {
            for w in widgets.iter_mut() {
                if let WidgetType::TextInput { focused, .. } = &mut w.widget_type {
                    *focused = w.id == id;
                }
            }
            None
        }
        UiEvent::FocusOut(id) => {
            for w in widgets.iter_mut() {
                if w.id == id {
                    if let WidgetType::TextInput { focused, .. } = &mut w.widget_type {
                        *focused = false;
                    }
                }
            }
            None
        }
        UiEvent::Scroll(delta) => {
            for w in widgets.iter_mut() {
                if let WidgetType::List { items, scroll_offset, .. } = &mut w.widget_type {
                    if delta < 0 && *scroll_offset > 0 {
                        *scroll_offset = scroll_offset.saturating_sub((-delta) as usize);
                    } else if delta > 0 {
                        let max = items.len().saturating_sub(1);
                        *scroll_offset = (*scroll_offset + delta as usize).min(max);
                    }
                    return None;
                }
            }
            None
        }
    }
}

// ── Rendering ───────────────────────────────────────────────────────────────

/// Render a widget tree as text (for VGA text mode).
pub fn render_text(widget_id: u32) -> String {
    let widgets = WIDGETS.lock();
    let w = match widgets.iter().find(|w| w.id == widget_id) {
        Some(w) => w,
        None => return format!("[widget {} not found]", widget_id),
    };
    if !w.visible {
        return String::new();
    }
    let mut out = String::new();
    match &w.widget_type {
        WidgetType::Label { text, .. } => {
            out.push_str(text);
        }
        WidgetType::Button { text, pressed, .. } => {
            if *pressed {
                out.push_str(&format!("[*{}*]", text));
            } else {
                out.push_str(&format!("[ {} ]", text));
            }
        }
        WidgetType::TextInput { text, cursor, focused, max_len, .. } => {
            out.push('[');
            for (i, ch) in text.chars().enumerate() {
                if *focused && i == *cursor {
                    out.push('|');
                }
                out.push(ch);
            }
            if *focused && *cursor >= text.len() {
                out.push('|');
            }
            let pad = max_len.saturating_sub(text.len());
            for _ in 0..pad.min(20) {
                out.push('_');
            }
            out.push(']');
        }
        WidgetType::Checkbox { label, checked } => {
            out.push_str(&format!("[{}] {}", if *checked { "X" } else { " " }, label));
        }
        WidgetType::ProgressBar { value, max, .. } => {
            let bar_w = 20usize;
            let filled = if *max > 0 { (*value as usize * bar_w) / *max as usize } else { 0 };
            out.push('[');
            for i in 0..bar_w {
                out.push(if i < filled { '#' } else { '-' });
            }
            out.push_str(&format!("] {}%", if *max > 0 { *value * 100 / *max } else { 0 }));
        }
        WidgetType::Panel { border, .. } => {
            if *border {
                out.push_str("+--panel--+");
            } else {
                out.push_str("  panel   ");
            }
        }
        WidgetType::List { items, selected, scroll_offset } => {
            let visible = 10usize;
            let start = *scroll_offset;
            let end = (start + visible).min(items.len());
            for i in start..end {
                if Some(i) == *selected {
                    out.push_str(&format!("> {}\n", items[i]));
                } else {
                    out.push_str(&format!("  {}\n", items[i]));
                }
            }
            if items.len() > visible {
                out.push_str(&format!("  ({} more)", items.len() - visible));
            }
        }
        WidgetType::Image { width, height, .. } => {
            out.push_str(&format!("[image {}x{}]", width, height));
        }
        WidgetType::Separator => {
            out.push_str("────────────────────");
        }
        WidgetType::Spacer(h) => {
            for _ in 0..*h / 8 {
                out.push('\n');
            }
        }
    }
    // Render children
    for child_id in &w.children {
        let child_text = drop_and_render(*child_id, &widgets);
        if !child_text.is_empty() {
            out.push('\n');
            out.push_str(&child_text);
        }
    }
    out
}

/// Helper: render a child widget (avoids double-lock).
fn drop_and_render(widget_id: u32, widgets: &[Widget]) -> String {
    let w = match widgets.iter().find(|w| w.id == widget_id) {
        Some(w) => w,
        None => return String::new(),
    };
    if !w.visible {
        return String::new();
    }
    let mut out = String::new();
    match &w.widget_type {
        WidgetType::Label { text, .. } => out.push_str(text),
        WidgetType::Button { text, pressed, .. } => {
            out.push_str(&format!("{}{}{}", if *pressed { "[*" } else { "[ " }, text,
                if *pressed { "*]" } else { " ]" }));
        }
        WidgetType::Checkbox { label, checked } => {
            out.push_str(&format!("[{}] {}", if *checked { "X" } else { " " }, label));
        }
        WidgetType::Separator => out.push_str("----"),
        _ => out.push_str(&format!("[widget:{}]", widget_id)),
    }
    out
}

/// Draw commands for framebuffer rendering.
#[derive(Debug, Clone)]
pub enum DrawCommand {
    FillRect(Rect, Color),
    DrawText(Point, String, Color),
    DrawLine(Point, Point, Color),
    DrawBorder(Rect, Color),
}

/// Render a widget tree as pixel commands (for framebuffer).
pub fn render_fb(widget_id: u32) -> Vec<DrawCommand> {
    let widgets = WIDGETS.lock();
    let mut cmds = Vec::new();
    render_fb_inner(widget_id, &widgets, &mut cmds);
    cmds
}

fn render_fb_inner(widget_id: u32, widgets: &[Widget], cmds: &mut Vec<DrawCommand>) {
    let w = match widgets.iter().find(|w| w.id == widget_id) {
        Some(w) => w,
        None => return,
    };
    if !w.visible { return; }

    match &w.widget_type {
        WidgetType::Label { text, color } => {
            cmds.push(DrawCommand::DrawText(
                Point::new(w.bounds.x, w.bounds.y), text.clone(), *color,
            ));
        }
        WidgetType::Button { text, pressed, .. } => {
            let bg = if *pressed { Color::DARK_GRAY } else { Color::GRAY };
            cmds.push(DrawCommand::FillRect(w.bounds, bg));
            cmds.push(DrawCommand::DrawBorder(w.bounds, Color::BLACK));
            cmds.push(DrawCommand::DrawText(
                Point::new(w.bounds.x + 4, w.bounds.y + 2), text.clone(), Color::BLACK,
            ));
        }
        WidgetType::TextInput { text, focused, .. } => {
            cmds.push(DrawCommand::FillRect(w.bounds, Color::WHITE));
            let border = if *focused { Color::BLUE } else { Color::GRAY };
            cmds.push(DrawCommand::DrawBorder(w.bounds, border));
            cmds.push(DrawCommand::DrawText(
                Point::new(w.bounds.x + 2, w.bounds.y + 2), text.clone(), Color::BLACK,
            ));
        }
        WidgetType::Checkbox { label, checked } => {
            let mark = if *checked { format!("[X] {}", label) } else { format!("[ ] {}", label) };
            cmds.push(DrawCommand::DrawText(
                Point::new(w.bounds.x, w.bounds.y), mark, Color::BLACK,
            ));
        }
        WidgetType::ProgressBar { value, max, color } => {
            cmds.push(DrawCommand::FillRect(w.bounds, Color::DARK_GRAY));
            cmds.push(DrawCommand::DrawBorder(w.bounds, Color::GRAY));
            if *max > 0 {
                let fill_w = (*value as u32 * w.bounds.width) / *max;
                cmds.push(DrawCommand::FillRect(
                    Rect::new(w.bounds.x, w.bounds.y, fill_w, w.bounds.height), *color,
                ));
            }
        }
        WidgetType::Panel { bg_color, border } => {
            cmds.push(DrawCommand::FillRect(w.bounds, *bg_color));
            if *border {
                cmds.push(DrawCommand::DrawBorder(w.bounds, Color::GRAY));
            }
        }
        WidgetType::List { items, selected, scroll_offset } => {
            cmds.push(DrawCommand::FillRect(w.bounds, Color::WHITE));
            cmds.push(DrawCommand::DrawBorder(w.bounds, Color::GRAY));
            let row_h = 16i32;
            let visible = (w.bounds.height as i32 / row_h) as usize;
            let start = *scroll_offset;
            let end = (start + visible).min(items.len());
            for i in start..end {
                let y = w.bounds.y + ((i - start) as i32) * row_h;
                if Some(i) == *selected {
                    cmds.push(DrawCommand::FillRect(
                        Rect::new(w.bounds.x, y, w.bounds.width, row_h as u32), Color::BLUE,
                    ));
                    cmds.push(DrawCommand::DrawText(
                        Point::new(w.bounds.x + 4, y + 2), items[i].clone(), Color::WHITE,
                    ));
                } else {
                    cmds.push(DrawCommand::DrawText(
                        Point::new(w.bounds.x + 4, y + 2), items[i].clone(), Color::BLACK,
                    ));
                }
            }
        }
        WidgetType::Image { .. } => {
            // Image rendering would blit pixels directly; emit a placeholder rect
            cmds.push(DrawCommand::FillRect(w.bounds, Color::GRAY));
            cmds.push(DrawCommand::DrawBorder(w.bounds, Color::BLACK));
        }
        WidgetType::Separator => {
            let mid_y = w.bounds.y + w.bounds.height as i32 / 2;
            cmds.push(DrawCommand::DrawLine(
                Point::new(w.bounds.x, mid_y),
                Point::new(w.bounds.right(), mid_y),
                Color::GRAY,
            ));
        }
        WidgetType::Spacer(_) => {}
    }
    for child_id in &w.children {
        render_fb_inner(*child_id, widgets, cmds);
    }
}

// ── Widget registry ─────────────────────────────────────────────────────────

static WIDGETS: Mutex<Vec<Widget>> = Mutex::new(Vec::new());
static NEXT_WIDGET_ID: AtomicU32 = AtomicU32::new(1);

pub fn create_widget(wtype: WidgetType) -> u32 {
    let id = NEXT_WIDGET_ID.fetch_add(1, Ordering::SeqCst);
    let w = Widget::new(id, wtype);
    WIDGETS.lock().push(w);
    id
}

pub fn destroy_widget(id: u32) {
    let mut widgets = WIDGETS.lock();
    // Remove from parent's children list
    let parent = widgets.iter().find(|w| w.id == id).and_then(|w| w.parent);
    if let Some(pid) = parent {
        if let Some(p) = widgets.iter_mut().find(|w| w.id == pid) {
            p.children.retain(|c| *c != id);
        }
    }
    // Recursively destroy children
    let child_ids: Vec<u32> = widgets
        .iter()
        .find(|w| w.id == id)
        .map(|w| w.children.clone())
        .unwrap_or_default();
    widgets.retain(|w| w.id != id);
    drop(widgets);
    for cid in child_ids {
        destroy_widget(cid);
    }
}

pub fn set_bounds(id: u32, rect: Rect) {
    if let Some(w) = WIDGETS.lock().iter_mut().find(|w| w.id == id) {
        w.bounds = rect;
    }
}

pub fn set_visible(id: u32, vis: bool) {
    if let Some(w) = WIDGETS.lock().iter_mut().find(|w| w.id == id) {
        w.visible = vis;
    }
}

pub fn set_enabled(id: u32, en: bool) {
    if let Some(w) = WIDGETS.lock().iter_mut().find(|w| w.id == id) {
        w.enabled = en;
    }
}

pub fn find_widget_at(point: Point) -> Option<u32> {
    let widgets = WIDGETS.lock();
    let mut best: Option<(u32, u16)> = None;
    for w in widgets.iter() {
        if w.visible && w.bounds.contains(point) {
            match best {
                None => best = Some((w.id, w.z_order)),
                Some((_, z)) if w.z_order >= z => best = Some((w.id, w.z_order)),
                _ => {}
            }
        }
    }
    best.map(|(id, _)| id)
}

pub fn list_widgets() -> String {
    let widgets = WIDGETS.lock();
    if widgets.is_empty() {
        return String::from("No widgets registered.");
    }
    let mut out = format!("{} widget(s):\n", widgets.len());
    for w in widgets.iter() {
        let kind = match &w.widget_type {
            WidgetType::Label { text, .. } => format!("Label(\"{}\")", text),
            WidgetType::Button { text, .. } => format!("Button(\"{}\")", text),
            WidgetType::TextInput { .. } => String::from("TextInput"),
            WidgetType::Checkbox { label, checked } => format!("Checkbox(\"{}\", {})", label, checked),
            WidgetType::ProgressBar { value, max, .. } => format!("Progress({}/{})", value, max),
            WidgetType::Panel { .. } => String::from("Panel"),
            WidgetType::List { items, .. } => format!("List({} items)", items.len()),
            WidgetType::Image { width, height, .. } => format!("Image({}x{})", width, height),
            WidgetType::Separator => String::from("Separator"),
            WidgetType::Spacer(h) => format!("Spacer({})", h),
        };
        let vis = if w.visible { "" } else { " [hidden]" };
        let en = if w.enabled { "" } else { " [disabled]" };
        out.push_str(&format!(
            "  #{}: {} at ({},{} {}x{}){}{}\n",
            w.id, kind, w.bounds.x, w.bounds.y, w.bounds.width, w.bounds.height, vis, en
        ));
    }
    out
}

pub fn widget_tree() -> String {
    let widgets = WIDGETS.lock();
    if widgets.is_empty() {
        return String::from("(empty widget tree)");
    }
    // Find root widgets (no parent)
    let roots: Vec<u32> = widgets.iter().filter(|w| w.parent.is_none()).map(|w| w.id).collect();
    let mut out = String::from("Widget tree:\n");
    for rid in &roots {
        tree_recurse(*rid, &widgets, &mut out, 0);
    }
    out
}

fn tree_recurse(id: u32, widgets: &[Widget], out: &mut String, depth: usize) {
    let w = match widgets.iter().find(|w| w.id == id) {
        Some(w) => w,
        None => return,
    };
    for _ in 0..depth {
        out.push_str("  ");
    }
    let kind = match &w.widget_type {
        WidgetType::Label { text, .. } => format!("Label(\"{}\")", text),
        WidgetType::Button { text, .. } => format!("Button(\"{}\")", text),
        WidgetType::TextInput { .. } => String::from("TextInput"),
        WidgetType::Checkbox { label, .. } => format!("Checkbox(\"{}\")", label),
        WidgetType::ProgressBar { value, max, .. } => format!("Progress({}/{})", value, max),
        WidgetType::Panel { .. } => String::from("Panel"),
        WidgetType::List { items, .. } => format!("List({} items)", items.len()),
        WidgetType::Image { width, height, .. } => format!("Image({}x{})", width, height),
        WidgetType::Separator => String::from("---"),
        WidgetType::Spacer(h) => format!("Spacer({})", h),
    };
    out.push_str(&format!("#{} {}\n", w.id, kind));
    for cid in &w.children {
        tree_recurse(*cid, widgets, out, depth + 1);
    }
}

// ── Pre-built dialogs ───────────────────────────────────────────────────────

pub fn dialog_message(title: &str, message: &str) -> u32 {
    let panel = create_widget(WidgetType::Panel { bg_color: Color::WHITE, border: true });
    set_bounds(panel, Rect::new(100, 80, 300, 120));

    let title_lbl = create_widget(WidgetType::Label {
        text: String::from(title), color: Color::BLACK,
    });
    set_bounds(title_lbl, Rect::new(110, 85, 280, 16));
    add_child(panel, title_lbl);

    let msg_lbl = create_widget(WidgetType::Label {
        text: String::from(message), color: Color::DARK_GRAY,
    });
    set_bounds(msg_lbl, Rect::new(110, 110, 280, 32));
    add_child(panel, msg_lbl);

    let ok_btn = create_widget(WidgetType::Button {
        text: String::from("OK"), pressed: false, on_click: Some(String::from("dialog_close")),
    });
    set_bounds(ok_btn, Rect::new(210, 160, 80, 24));
    add_child(panel, ok_btn);

    panel
}

pub fn dialog_confirm(title: &str, message: &str) -> u32 {
    let panel = create_widget(WidgetType::Panel { bg_color: Color::WHITE, border: true });
    set_bounds(panel, Rect::new(100, 80, 300, 130));

    let title_lbl = create_widget(WidgetType::Label {
        text: String::from(title), color: Color::BLACK,
    });
    set_bounds(title_lbl, Rect::new(110, 85, 280, 16));
    add_child(panel, title_lbl);

    let msg_lbl = create_widget(WidgetType::Label {
        text: String::from(message), color: Color::DARK_GRAY,
    });
    set_bounds(msg_lbl, Rect::new(110, 110, 280, 32));
    add_child(panel, msg_lbl);

    let yes_btn = create_widget(WidgetType::Button {
        text: String::from("Yes"), pressed: false, on_click: Some(String::from("dialog_yes")),
    });
    set_bounds(yes_btn, Rect::new(150, 160, 80, 24));
    add_child(panel, yes_btn);

    let no_btn = create_widget(WidgetType::Button {
        text: String::from("No"), pressed: false, on_click: Some(String::from("dialog_no")),
    });
    set_bounds(no_btn, Rect::new(260, 160, 80, 24));
    add_child(panel, no_btn);

    panel
}

pub fn dialog_input(title: &str, prompt: &str) -> u32 {
    let panel = create_widget(WidgetType::Panel { bg_color: Color::WHITE, border: true });
    set_bounds(panel, Rect::new(100, 80, 300, 140));

    let title_lbl = create_widget(WidgetType::Label {
        text: String::from(title), color: Color::BLACK,
    });
    set_bounds(title_lbl, Rect::new(110, 85, 280, 16));
    add_child(panel, title_lbl);

    let prompt_lbl = create_widget(WidgetType::Label {
        text: String::from(prompt), color: Color::DARK_GRAY,
    });
    set_bounds(prompt_lbl, Rect::new(110, 108, 280, 16));
    add_child(panel, prompt_lbl);

    let input = create_widget(WidgetType::TextInput {
        text: String::new(), cursor: 0, max_len: 64, focused: true,
    });
    set_bounds(input, Rect::new(110, 130, 280, 22));
    add_child(panel, input);

    let ok_btn = create_widget(WidgetType::Button {
        text: String::from("OK"), pressed: false, on_click: Some(String::from("dialog_submit")),
    });
    set_bounds(ok_btn, Rect::new(210, 165, 80, 24));
    add_child(panel, ok_btn);

    panel
}

pub fn dialog_progress(title: &str, max: u32) -> u32 {
    let panel = create_widget(WidgetType::Panel { bg_color: Color::WHITE, border: true });
    set_bounds(panel, Rect::new(100, 100, 300, 80));

    let title_lbl = create_widget(WidgetType::Label {
        text: String::from(title), color: Color::BLACK,
    });
    set_bounds(title_lbl, Rect::new(110, 105, 280, 16));
    add_child(panel, title_lbl);

    let bar = create_widget(WidgetType::ProgressBar {
        value: 0, max, color: Color::GREEN,
    });
    set_bounds(bar, Rect::new(110, 130, 280, 20));
    add_child(panel, bar);

    panel
}

fn add_child(parent_id: u32, child_id: u32) {
    let mut widgets = WIDGETS.lock();
    if let Some(child) = widgets.iter_mut().find(|w| w.id == child_id) {
        child.parent = Some(parent_id);
    }
    if let Some(parent) = widgets.iter_mut().find(|w| w.id == parent_id) {
        parent.children.push(child_id);
    }
}

// ── Theme ───────────────────────────────────────────────────────────────────

pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub button_bg: Color,
    pub button_fg: Color,
    pub input_bg: Color,
    pub border: Color,
    pub font_size: u8,
}

pub fn default_theme() -> Theme {
    Theme {
        bg: Color::WHITE,
        fg: Color::BLACK,
        accent: Color::BLUE,
        button_bg: Color::GRAY,
        button_fg: Color::BLACK,
        input_bg: Color::WHITE,
        border: Color::DARK_GRAY,
        font_size: 8,
    }
}

pub fn merlion_theme() -> Theme {
    Theme {
        bg: Color::new(20, 20, 30),
        fg: Color::MERLION_GOLD,
        accent: Color::MERLION_GOLD,
        button_bg: Color::new(50, 50, 70),
        button_fg: Color::MERLION_GOLD,
        input_bg: Color::new(30, 30, 45),
        border: Color::new(100, 80, 20),
        font_size: 8,
    }
}

// ── Init, stats, demo ───────────────────────────────────────────────────────

pub fn init() {
    // Reset widget state
    WIDGETS.lock().clear();
    NEXT_WIDGET_ID.store(1, Ordering::SeqCst);
}

pub fn widget_stats() -> String {
    let widgets = WIDGETS.lock();
    let total = widgets.len();
    let visible = widgets.iter().filter(|w| w.visible).count();
    let enabled = widgets.iter().filter(|w| w.enabled).count();
    let roots = widgets.iter().filter(|w| w.parent.is_none()).count();
    let next_id = NEXT_WIDGET_ID.load(Ordering::SeqCst);
    format!(
        "Widget stats:\n  Total: {}\n  Visible: {}\n  Enabled: {}\n  Root widgets: {}\n  Next ID: {}\n",
        total, visible, enabled, roots, next_id
    )
}

pub fn demo() -> String {
    init();
    let mut out = String::from("Widget toolkit demo:\n\n");

    // Create a panel with various widgets
    let panel = create_widget(WidgetType::Panel { bg_color: Color::WHITE, border: true });
    set_bounds(panel, Rect::new(10, 10, 300, 250));

    let title = create_widget(WidgetType::Label {
        text: String::from("MerlionOS Widget Demo"), color: Color::MERLION_GOLD,
    });
    set_bounds(title, Rect::new(20, 15, 280, 16));
    add_child(panel, title);

    let _sep = create_widget(WidgetType::Separator);
    set_bounds(_sep, Rect::new(20, 35, 280, 4));

    let btn = create_widget(WidgetType::Button {
        text: String::from("Click Me"), pressed: false, on_click: Some(String::from("demo_click")),
    });
    set_bounds(btn, Rect::new(20, 45, 100, 24));

    let chk = create_widget(WidgetType::Checkbox {
        label: String::from("Enable feature"), checked: true,
    });
    set_bounds(chk, Rect::new(20, 75, 200, 16));

    let input = create_widget(WidgetType::TextInput {
        text: String::from("Hello"), cursor: 5, max_len: 32, focused: false,
    });
    set_bounds(input, Rect::new(20, 100, 260, 22));

    let bar = create_widget(WidgetType::ProgressBar {
        value: 65, max: 100, color: Color::GREEN,
    });
    set_bounds(bar, Rect::new(20, 130, 260, 16));

    let list = create_widget(WidgetType::List {
        items: vec![
            String::from("kernel"),
            String::from("shell"),
            String::from("vfs"),
            String::from("network"),
            String::from("graphics"),
        ],
        selected: Some(0),
        scroll_offset: 0,
    });
    set_bounds(list, Rect::new(20, 155, 260, 80));

    // Render text representations
    out.push_str(&render_text(panel));
    out.push('\n');
    out.push_str(&format!("\nButton: {}\n", render_text(btn)));
    out.push_str(&format!("Checkbox: {}\n", render_text(chk)));
    out.push_str(&format!("Input: {}\n", render_text(input)));
    out.push_str(&format!("Progress: {}\n", render_text(bar)));
    out.push_str(&format!("List:\n{}\n", render_text(list)));

    out.push_str("\n--- Stats ---\n");
    out.push_str(&widget_stats());
    out.push_str("\n--- Tree ---\n");
    out.push_str(&widget_tree());

    out
}
