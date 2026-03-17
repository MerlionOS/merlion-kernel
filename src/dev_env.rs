/// Integrated development environment for MerlionOS.
/// Enhanced editing, syntax highlighting, build integration,
/// and debugging support for kernel development.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_BUFFERS: usize = 32;
const MAX_BREAKPOINTS: usize = 64;
const MAX_WATCH: usize = 16;
const MAX_ERRORS: usize = 128;

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static FILES_OPENED: AtomicU64 = AtomicU64::new(0);
static BUILDS_RUN: AtomicU64 = AtomicU64::new(0);
static HIGHLIGHTS_DONE: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Language
// ---------------------------------------------------------------------------

/// Supported programming languages for syntax highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    C,
    Python,
    JavaScript,
    Shell,
    Go,
    Java,
    Toml,
    Json,
    Markdown,
    Unknown,
}

impl Language {
    pub fn from_extension(ext: &str) -> Self {
        match ext {
            "rs" => Self::Rust,
            "c" | "h" => Self::C,
            "py" => Self::Python,
            "js" => Self::JavaScript,
            "sh" | "bash" => Self::Shell,
            "go" => Self::Go,
            "java" => Self::Java,
            "toml" => Self::Toml,
            "json" => Self::Json,
            "md" | "markdown" => Self::Markdown,
            _ => Self::Unknown,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::C => "C",
            Self::Python => "Python",
            Self::JavaScript => "JavaScript",
            Self::Shell => "Shell",
            Self::Go => "Go",
            Self::Java => "Java",
            Self::Toml => "TOML",
            Self::Json => "JSON",
            Self::Markdown => "Markdown",
            Self::Unknown => "Plain",
        }
    }

    /// Get keywords for this language.
    pub fn keywords(&self) -> &'static [&'static str] {
        match self {
            Self::Rust => &[
                "fn", "let", "mut", "pub", "use", "mod", "struct", "enum", "impl",
                "trait", "for", "while", "loop", "if", "else", "match", "return",
                "self", "super", "crate", "const", "static", "unsafe", "async", "await",
                "where", "type", "as", "in", "ref", "move", "true", "false",
            ],
            Self::C => &[
                "int", "char", "void", "float", "double", "long", "short", "unsigned",
                "signed", "if", "else", "for", "while", "do", "switch", "case",
                "break", "continue", "return", "struct", "union", "enum", "typedef",
                "const", "static", "extern", "sizeof", "NULL", "true", "false",
            ],
            Self::Python => &[
                "def", "class", "if", "elif", "else", "for", "while", "return",
                "import", "from", "as", "try", "except", "finally", "raise", "with",
                "yield", "lambda", "pass", "break", "continue", "and", "or", "not",
                "in", "is", "None", "True", "False", "self", "global", "nonlocal",
            ],
            Self::JavaScript => &[
                "function", "var", "let", "const", "if", "else", "for", "while",
                "return", "class", "new", "this", "super", "import", "export",
                "default", "switch", "case", "break", "continue", "try", "catch",
                "finally", "throw", "async", "await", "yield", "typeof", "instanceof",
                "true", "false", "null", "undefined",
            ],
            Self::Shell => &[
                "if", "then", "else", "elif", "fi", "for", "while", "do", "done",
                "case", "esac", "function", "return", "exit", "echo", "export",
                "local", "readonly", "shift", "set", "unset", "true", "false",
            ],
            Self::Go => &[
                "func", "var", "const", "type", "struct", "interface", "map",
                "chan", "go", "select", "if", "else", "for", "range", "switch",
                "case", "default", "break", "continue", "return", "defer",
                "package", "import", "true", "false", "nil",
            ],
            Self::Java => &[
                "class", "interface", "enum", "public", "private", "protected",
                "static", "final", "abstract", "void", "int", "long", "boolean",
                "char", "byte", "short", "double", "float", "if", "else", "for",
                "while", "do", "switch", "case", "break", "continue", "return",
                "new", "this", "super", "try", "catch", "finally", "throw",
                "import", "package", "extends", "implements", "true", "false", "null",
            ],
            Self::Toml | Self::Json | Self::Markdown | Self::Unknown => &[],
        }
    }
}

/// Detect language from a file path.
pub fn detect_language(path: &str) -> Language {
    if let Some(dot_pos) = path.rfind('.') {
        Language::from_extension(&path[dot_pos + 1..])
    } else {
        Language::Unknown
    }
}

// ---------------------------------------------------------------------------
// HighlightKind
// ---------------------------------------------------------------------------

/// The kind of syntax element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Function,
    Operator,
    Punctuation,
    Normal,
}

impl HighlightKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::String => "string",
            Self::Comment => "comment",
            Self::Number => "number",
            Self::Type => "type",
            Self::Function => "function",
            Self::Operator => "operator",
            Self::Punctuation => "punctuation",
            Self::Normal => "normal",
        }
    }

    /// ANSI color code for terminal display.
    pub fn ansi_color(&self) -> &'static str {
        match self {
            Self::Keyword => "\x1b[35m",     // magenta
            Self::String => "\x1b[32m",      // green
            Self::Comment => "\x1b[90m",     // gray
            Self::Number => "\x1b[33m",      // yellow
            Self::Type => "\x1b[36m",        // cyan
            Self::Function => "\x1b[34m",    // blue
            Self::Operator => "\x1b[31m",    // red
            Self::Punctuation => "\x1b[37m", // white
            Self::Normal => "\x1b[0m",       // reset
        }
    }
}

/// A highlighted span: (start_offset, length, kind).
pub struct HighlightSpan {
    pub start: usize,
    pub len: usize,
    pub kind: HighlightKind,
}

/// Highlight source code, returning spans with their kinds.
pub fn highlight(source: &str, language: Language) -> Vec<HighlightSpan> {
    let mut spans = Vec::new();
    let keywords = language.keywords();
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i];
        // Line comments
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            let start = i;
            while i < bytes.len() && bytes[i] != b'\n' { i += 1; }
            spans.push(HighlightSpan { start, len: i - start, kind: HighlightKind::Comment });
            continue;
        }
        // Hash comments (Python, Shell, TOML)
        if ch == b'#' && matches!(language, Language::Python | Language::Shell | Language::Toml) {
            let start = i;
            while i < bytes.len() && bytes[i] != b'\n' { i += 1; }
            spans.push(HighlightSpan { start, len: i - start, kind: HighlightKind::Comment });
            continue;
        }
        // Strings
        if ch == b'"' || ch == b'\'' {
            let quote = ch;
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' { i += 1; } // skip escaped
                i += 1;
            }
            if i < bytes.len() { i += 1; } // closing quote
            spans.push(HighlightSpan { start, len: i - start, kind: HighlightKind::String });
            continue;
        }
        // Numbers
        if ch.is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit()
                || bytes[i] == b'.' || bytes[i] == b'x' || bytes[i] == b'_') {
                i += 1;
            }
            spans.push(HighlightSpan { start, len: i - start, kind: HighlightKind::Number });
            continue;
        }
        // Identifiers / keywords
        if ch.is_ascii_alphabetic() || ch == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = core::str::from_utf8(&bytes[start..i]).unwrap_or("");
            let kind = if keywords.iter().any(|&k| k == word) {
                HighlightKind::Keyword
            } else if !word.is_empty() && word.as_bytes()[0].is_ascii_uppercase() {
                HighlightKind::Type
            } else if i < bytes.len() && bytes[i] == b'(' {
                HighlightKind::Function
            } else {
                HighlightKind::Normal
            };
            spans.push(HighlightSpan { start, len: i - start, kind });
            continue;
        }
        // Operators
        if matches!(ch, b'+' | b'-' | b'*' | b'/' | b'=' | b'<' | b'>'
            | b'!' | b'&' | b'|' | b'^' | b'%' | b'~') {
            spans.push(HighlightSpan { start: i, len: 1, kind: HighlightKind::Operator });
            i += 1;
            continue;
        }
        // Punctuation
        if matches!(ch, b'(' | b')' | b'[' | b']' | b'{' | b'}' | b';'
            | b',' | b'.' | b':') {
            spans.push(HighlightSpan { start: i, len: 1, kind: HighlightKind::Punctuation });
            i += 1;
            continue;
        }
        // Whitespace and other
        i += 1;
    }
    HIGHLIGHTS_DONE.fetch_add(1, Ordering::Relaxed);
    spans
}

/// Render highlighted source to a string with ANSI color codes.
pub fn highlight_to_ansi(source: &str, language: Language) -> String {
    let spans = highlight(source, language);
    let mut out = String::new();
    let mut last_end = 0;
    for span in &spans {
        // Gap between spans (whitespace etc.)
        if span.start > last_end {
            out.push_str(&source[last_end..span.start]);
        }
        out.push_str(span.kind.ansi_color());
        out.push_str(&source[span.start..span.start + span.len]);
        out.push_str("\x1b[0m");
        last_end = span.start + span.len;
    }
    if last_end < source.len() {
        out.push_str(&source[last_end..]);
    }
    out
}

// ---------------------------------------------------------------------------
// Code navigation
// ---------------------------------------------------------------------------

/// Result of a "go to definition" search.
pub struct DefinitionResult {
    pub file: String,
    pub line: usize,
    pub snippet: String,
}

/// Simple go-to-definition: search for `fn name`, `struct name`, etc.
pub fn goto_definition(source: &str, identifier: &str) -> Vec<DefinitionResult> {
    let mut results = Vec::new();
    let patterns = [
        format!("fn {}", identifier),
        format!("struct {}", identifier),
        format!("enum {}", identifier),
        format!("trait {}", identifier),
        format!("type {}", identifier),
        format!("const {}", identifier),
        format!("static {}", identifier),
        format!("mod {}", identifier),
    ];
    for (line_num, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        for pat in &patterns {
            if trimmed.contains(pat.as_str()) {
                results.push(DefinitionResult {
                    file: String::from("<buffer>"),
                    line: line_num + 1,
                    snippet: String::from(trimmed),
                });
                break;
            }
        }
    }
    results
}

/// Find all references to an identifier in source.
pub fn find_references(source: &str, identifier: &str) -> Vec<(usize, String)> {
    let mut results = Vec::new();
    for (line_num, line) in source.lines().enumerate() {
        if line.contains(identifier) {
            results.push((line_num + 1, String::from(line.trim())));
        }
    }
    results
}

// ---------------------------------------------------------------------------
// Auto-indent & bracket matching
// ---------------------------------------------------------------------------

/// Compute indentation level based on brace/bracket depth.
pub fn auto_indent(source: &str, line_index: usize) -> usize {
    let mut depth: i32 = 0;
    for (i, line) in source.lines().enumerate() {
        if i >= line_index { break; }
        for ch in line.chars() {
            match ch {
                '{' | '(' | '[' => depth += 1,
                '}' | ')' | ']' => { if depth > 0 { depth -= 1; } }
                _ => {}
            }
        }
    }
    if depth < 0 { 0 } else { depth as usize * 4 }
}

/// Find matching bracket position. Returns (line, col) or None.
pub fn find_matching_bracket(source: &str, line: usize, col: usize) -> Option<(usize, usize)> {
    let lines: Vec<&str> = source.lines().collect();
    if line >= lines.len() { return None; }
    let target_line = lines[line];
    if col >= target_line.len() { return None; }
    let ch = target_line.as_bytes()[col];
    let (open, close, forward) = match ch {
        b'(' => (b'(', b')', true),
        b')' => (b'(', b')', false),
        b'[' => (b'[', b']', true),
        b']' => (b'[', b']', false),
        b'{' => (b'{', b'}', true),
        b'}' => (b'{', b'}', false),
        _ => return None,
    };
    let mut depth: i32 = 0;
    if forward {
        for (li, &ln) in lines.iter().enumerate().skip(line) {
            let start_col = if li == line { col } else { 0 };
            for (ci, &b) in ln.as_bytes().iter().enumerate().skip(start_col) {
                if b == open { depth += 1; }
                if b == close { depth -= 1; }
                if depth == 0 { return Some((li, ci)); }
            }
        }
    } else {
        // Search backward
        for li in (0..=line).rev() {
            let end_col = if li == line { col + 1 } else { lines[li].len() };
            for ci in (0..end_col).rev() {
                let b = lines[li].as_bytes()[ci];
                if b == close { depth += 1; }
                if b == open { depth -= 1; }
                if depth == 0 { return Some((li, ci)); }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Buffer management
// ---------------------------------------------------------------------------

/// An open file buffer.
struct Buffer {
    path: String,
    content: String,
    language: Language,
    modified: bool,
    cursor_line: usize,
    cursor_col: usize,
}

/// Split view configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SplitMode {
    None,
    Horizontal,
    Vertical,
}

struct DevEnvState {
    buffers: Vec<Buffer>,
    active_buffer: usize,
    split_mode: SplitMode,
    split_buffer: usize, // second buffer index for split
    breakpoints: Vec<Breakpoint>,
    watch_list: Vec<WatchEntry>,
    build_errors: Vec<BuildError>,
    project_root: String,
}

struct Breakpoint {
    file: String,
    line: usize,
    enabled: bool,
}

struct WatchEntry {
    expression: String,
    last_value: String,
}

struct BuildError {
    file: String,
    line: usize,
    col: usize,
    message: String,
}

impl DevEnvState {
    const fn new() -> Self {
        Self {
            buffers: Vec::new(),
            active_buffer: 0,
            split_mode: SplitMode::None,
            split_buffer: 0,
            breakpoints: Vec::new(),
            watch_list: Vec::new(),
            build_errors: Vec::new(),
            project_root: String::new(),
        }
    }
}

static STATE: Mutex<DevEnvState> = Mutex::new(DevEnvState::new());

/// Open a file in a new buffer.
pub fn open_file(path: &str) -> Result<usize, &'static str> {
    let content = crate::vfs::cat(path).map_err(|_| "failed to read file")?;
    let language = detect_language(path);
    let mut state = STATE.lock();
    if state.buffers.len() >= MAX_BUFFERS {
        return Err("too many open buffers");
    }
    let idx = state.buffers.len();
    state.buffers.push(Buffer {
        path: String::from(path),
        content,
        language,
        modified: false,
        cursor_line: 0,
        cursor_col: 0,
    });
    state.active_buffer = idx;
    FILES_OPENED.fetch_add(1, Ordering::Relaxed);
    Ok(idx)
}

/// Save the active buffer back to VFS.
pub fn save_active() -> Result<(), &'static str> {
    let state = STATE.lock();
    if state.buffers.is_empty() { return Err("no buffer open"); }
    let buf = &state.buffers[state.active_buffer];
    crate::vfs::write(&buf.path, &buf.content).map_err(|_| "failed to write")?;
    Ok(())
}

/// Switch to buffer by index.
pub fn switch_buffer(idx: usize) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    if idx >= state.buffers.len() { return Err("invalid buffer index"); }
    state.active_buffer = idx;
    Ok(())
}

/// Close buffer by index.
pub fn close_buffer(idx: usize) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    if idx >= state.buffers.len() { return Err("invalid buffer index"); }
    state.buffers.remove(idx);
    if state.active_buffer >= state.buffers.len() && !state.buffers.is_empty() {
        state.active_buffer = state.buffers.len() - 1;
    }
    Ok(())
}

/// List all open buffers.
pub fn list_buffers() -> String {
    let state = STATE.lock();
    if state.buffers.is_empty() {
        return String::from("No buffers open.");
    }
    let mut out = format!("Open buffers ({}):\n", state.buffers.len());
    for (i, buf) in state.buffers.iter().enumerate() {
        let active = if i == state.active_buffer { ">" } else { " " };
        let modified = if buf.modified { "[+]" } else { "   " };
        out.push_str(&format!("  {} {:2}. {} {} ({})\n",
            active, i, modified, buf.path, buf.language.label()));
    }
    out
}

/// Go to line in active buffer.
pub fn goto_line(line: usize) {
    let mut state = STATE.lock();
    if state.buffers.is_empty() { return; }
    let idx = state.active_buffer;
    let max_line = state.buffers[idx].content.lines().count();
    state.buffers[idx].cursor_line = if line > max_line { max_line } else { line };
    state.buffers[idx].cursor_col = 0;
}

/// Set split view mode.
pub fn set_split(mode: SplitMode, second_buffer: usize) {
    let mut state = STATE.lock();
    state.split_mode = mode;
    if second_buffer < state.buffers.len() {
        state.split_buffer = second_buffer;
    }
}

// ---------------------------------------------------------------------------
// Build integration
// ---------------------------------------------------------------------------

/// Run a build (invokes build_system module).
pub fn build() -> String {
    BUILDS_RUN.fetch_add(1, Ordering::Relaxed);
    let mut state = STATE.lock();
    state.build_errors.clear();
    // Simulate invoking the build system
    let result = crate::build_system::build_stats();
    // Parse for errors (simple heuristic: lines containing "error")
    for line in result.lines() {
        if line.contains("error") {
            if state.build_errors.len() < MAX_ERRORS {
                state.build_errors.push(BuildError {
                    file: String::from("<unknown>"),
                    line: 0,
                    col: 0,
                    message: String::from(line),
                });
            }
        }
    }
    if state.build_errors.is_empty() {
        String::from("Build succeeded.")
    } else {
        format!("Build completed with {} error(s).", state.build_errors.len())
    }
}

/// Get build errors.
pub fn build_errors() -> String {
    let state = STATE.lock();
    if state.build_errors.is_empty() {
        return String::from("No build errors.");
    }
    let mut out = format!("Build errors ({}):\n", state.build_errors.len());
    for (i, err) in state.build_errors.iter().enumerate() {
        out.push_str(&format!("  {:2}. {}:{}:{} {}\n",
            i + 1, err.file, err.line, err.col, err.message));
    }
    out
}

// ---------------------------------------------------------------------------
// Debug integration
// ---------------------------------------------------------------------------

/// Set a breakpoint at a file:line.
pub fn set_breakpoint(file: &str, line: usize) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    if state.breakpoints.len() >= MAX_BREAKPOINTS {
        return Err("too many breakpoints");
    }
    state.breakpoints.push(Breakpoint {
        file: String::from(file),
        line,
        enabled: true,
    });
    Ok(())
}

/// Clear a breakpoint at file:line.
pub fn clear_breakpoint(file: &str, line: usize) -> bool {
    let mut state = STATE.lock();
    let before = state.breakpoints.len();
    state.breakpoints.retain(|bp| !(bp.file == file && bp.line == line));
    state.breakpoints.len() < before
}

/// List breakpoints.
pub fn list_breakpoints() -> String {
    let state = STATE.lock();
    if state.breakpoints.is_empty() {
        return String::from("No breakpoints set.");
    }
    let mut out = format!("Breakpoints ({}):\n", state.breakpoints.len());
    for (i, bp) in state.breakpoints.iter().enumerate() {
        let status = if bp.enabled { "ON " } else { "OFF" };
        out.push_str(&format!("  {:2}. [{}] {}:{}\n", i + 1, status, bp.file, bp.line));
    }
    out
}

/// Add a variable to the watch list.
pub fn watch_add(expr: &str) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    if state.watch_list.len() >= MAX_WATCH {
        return Err("watch list full");
    }
    state.watch_list.push(WatchEntry {
        expression: String::from(expr),
        last_value: String::from("<unknown>"),
    });
    Ok(())
}

/// List watched variables.
pub fn watch_list() -> String {
    let state = STATE.lock();
    if state.watch_list.is_empty() {
        return String::from("Watch list is empty.");
    }
    let mut out = format!("Watch list ({}):\n", state.watch_list.len());
    for (i, w) in state.watch_list.iter().enumerate() {
        out.push_str(&format!("  {:2}. {} = {}\n", i + 1, w.expression, w.last_value));
    }
    out
}

// ---------------------------------------------------------------------------
// Project tree
// ---------------------------------------------------------------------------

/// Open a project directory and set as root.
pub fn open_project(path: &str) -> String {
    let mut state = STATE.lock();
    state.project_root = String::from(path);
    let entries = match crate::vfs::ls(path) {
        Ok(e) => e,
        Err(_) => return format!("Failed to open project at {}", path),
    };
    let mut out = format!("Project: {}\n", path);
    for (name, type_char) in &entries {
        let icon = if *type_char == 'd' { "+" } else { " " };
        out.push_str(&format!("  {} {}\n", icon, name));
    }
    out.push_str(&format!("{} entries\n", entries.len()));
    out
}

/// Highlight a file from VFS and print with ANSI colors.
pub fn highlight_file(path: &str) -> String {
    let content = match crate::vfs::cat(path) {
        Ok(c) => c,
        Err(_) => return format!("Cannot read: {}", path),
    };
    let lang = detect_language(path);
    let mut out = format!("--- {} [{}] ---\n", path, lang.label());
    let highlighted = highlight_to_ansi(&content, lang);
    for (i, line) in highlighted.lines().enumerate() {
        out.push_str(&format!("{:4} | {}\n", i + 1, line));
    }
    out
}

// ---------------------------------------------------------------------------
// Info & Stats
// ---------------------------------------------------------------------------

/// Dev environment information.
pub fn dev_env_info() -> String {
    let state = STATE.lock();
    let active_name = if !state.buffers.is_empty() {
        state.buffers[state.active_buffer].path.as_str()
    } else {
        "(none)"
    };
    format!(
        "Development Environment v1.0\n\
         Project root: {}\n\
         Open buffers: {}/{}\n\
         Active buffer: {}\n\
         Split mode: {:?}\n\
         Breakpoints: {}\n\
         Watch entries: {}\n\
         Build errors: {}\n\
         Languages: Rust, C, Python, JS, Shell, Go, Java, TOML, JSON, Markdown",
        if state.project_root.is_empty() { "(none)" } else { &state.project_root },
        state.buffers.len(), MAX_BUFFERS,
        active_name,
        state.split_mode,
        state.breakpoints.len(),
        state.watch_list.len(),
        state.build_errors.len(),
    )
}

/// Dev environment statistics.
pub fn dev_env_stats() -> String {
    format!(
        "Dev Environment Stats:\n\
         Files opened: {}\n\
         Builds run: {}\n\
         Highlights performed: {}",
        FILES_OPENED.load(Ordering::Relaxed),
        BUILDS_RUN.load(Ordering::Relaxed),
        HIGHLIGHTS_DONE.load(Ordering::Relaxed),
    )
}

/// Initialize the development environment.
pub fn init() {
    INITIALIZED.store(true, Ordering::Relaxed);
    crate::serial_println!("[ok] Development environment initialized");
}
