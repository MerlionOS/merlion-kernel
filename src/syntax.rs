/// Syntax highlighter for the MerlionOS text viewer/editor.
///
/// Provides lightweight, line-at-a-time highlighting for languages commonly
/// found in an OS kernel environment.  Designed for `#![no_std]` + `alloc`.

use alloc::string::String;
use alloc::vec::Vec;

/// Programming / configuration languages the highlighter understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust, Shell, Forth, Config, Plain,
}

/// Semantic category of a highlighted token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Keyword, String, Number, Comment, Operator,
    Identifier, Type, Builtin, Punctuation,
}

/// A single coloured fragment produced by the highlighter.
#[derive(Debug, Clone)]
pub struct HighlightedToken {
    /// The textual content of this token.
    pub text: String,
    /// Semantic kind used by the caller for styling decisions.
    pub kind: TokenKind,
    /// Suggested 24-bit RGB colour (e.g. `0xFF_D7_00`).
    pub color: u32,
}

impl HighlightedToken {
    /// Create a new highlighted token, deriving colour from kind.
    fn new(text: String, kind: TokenKind) -> Self {
        Self { text, kind, color: color_for_kind(kind) }
    }
}

/// Map a [`TokenKind`] to a 24-bit RGB colour value.
///
/// The palette targets dark-background terminals / framebuffer consoles.
pub fn color_for_kind(kind: TokenKind) -> u32 {
    match kind {
        TokenKind::Keyword     => 0xCC_7A_32, // warm orange
        TokenKind::String      => 0x6A_99_55, // muted green
        TokenKind::Number      => 0xB5_CE_A8, // pale green
        TokenKind::Comment     => 0x6A_9F_55, // grey-green
        TokenKind::Operator    => 0xD4_D4_D4, // light grey
        TokenKind::Identifier  => 0x9C_DC_FE, // light blue
        TokenKind::Type        => 0x4E_C9_B0, // teal
        TokenKind::Builtin     => 0xDC_DC_AA, // pale yellow
        TokenKind::Punctuation => 0xD4_D4_D4, // light grey
    }
}

/// Guess the [`Language`] from a filename or path.
pub fn detect_language(filename: &str) -> Language {
    if let Some(ext) = filename.rsplit('.').next() {
        match ext {
            "rs" => return Language::Rust,
            "sh" | "bash" | "zsh" => return Language::Shell,
            "fth" | "fs" | "4th" | "f" => return Language::Forth,
            "toml" | "ini" | "cfg" | "conf" | "env" | "yml" | "yaml"
                => return Language::Config,
            _ => {}
        }
    }
    let base = filename.rsplit('/').next().unwrap_or(filename);
    match base {
        "Makefile" | "Justfile" => Language::Shell,
        "Cargo.toml" | ".gitconfig" => Language::Config,
        _ => Language::Plain,
    }
}

// ---- keyword tables --------------------------------------------------------

const RUST_KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "if", "else", "for", "while", "match",
    "struct", "enum", "impl", "pub", "use", "mod", "return",
    "const", "static", "unsafe",
];

const SHELL_KEYWORDS: &[&str] = &[
    "if", "then", "else", "fi", "for", "do", "done", "while",
    "function", "echo", "exit",
];

const FORTH_BUILTINS: &[&str] = &[
    ":", ";", "if", "else", "then", "do", "loop", "+loop",
    "begin", "until", "while", "repeat", "dup", "drop", "swap",
    "over", "rot", "emit", "cr", ".", "variable", "constant",
    "create", "does>", "allot", "cells", "here",
];

fn is_in(word: &str, table: &[&str]) -> bool {
    table.iter().any(|&k| k == word)
}

/// Tokenise and highlight a single line of source code.
///
/// The concatenated `.text` fields reproduce the original `line` exactly.
pub fn highlight_line(line: &str, lang: Language) -> Vec<HighlightedToken> {
    match lang {
        Language::Rust   => hl_c_like(line, RUST_KEYWORDS, true),
        Language::Shell  => hl_shell(line),
        Language::Forth  => hl_forth(line),
        Language::Config => hl_config(line),
        Language::Plain  => {
            if line.is_empty() { Vec::new() }
            else { alloc::vec![HighlightedToken::new(String::from(line), TokenKind::Identifier)] }
        }
    }
}

// ---- helpers ---------------------------------------------------------------

/// Consume a run of whitespace starting at `i`, returning the new index.
fn eat_ws(tokens: &mut Vec<HighlightedToken>, line: &str, b: &[u8], mut i: usize) -> usize {
    let start = i;
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') { i += 1; }
    tokens.push(HighlightedToken::new(String::from(&line[start..i]), TokenKind::Identifier));
    i
}

/// Consume a quoted string (single or double) starting at `i`.
fn eat_string(tokens: &mut Vec<HighlightedToken>, line: &str, b: &[u8], mut i: usize) -> usize {
    let q = b[i];
    let start = i;
    i += 1;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() { i += 2; }
        else if b[i] == q { i += 1; break; }
        else { i += 1; }
    }
    tokens.push(HighlightedToken::new(String::from(&line[start..i]), TokenKind::String));
    i
}

/// Consume a numeric literal starting at `i`.
fn eat_number(tokens: &mut Vec<HighlightedToken>, line: &str, b: &[u8], mut i: usize) -> usize {
    let start = i;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') { i += 1; }
    tokens.push(HighlightedToken::new(String::from(&line[start..i]), TokenKind::Number));
    i
}

/// Consume an identifier / keyword starting at `i`.
fn eat_word(
    tokens: &mut Vec<HighlightedToken>, line: &str, b: &[u8], mut i: usize,
    keywords: &[&str], detect_types: bool,
) -> usize {
    let start = i;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') { i += 1; }
    let word = &line[start..i];
    let kind = if is_in(word, keywords) { TokenKind::Keyword }
        else if detect_types && word.as_bytes()[0].is_ascii_uppercase() { TokenKind::Type }
        else { TokenKind::Identifier };
    tokens.push(HighlightedToken::new(String::from(word), kind));
    i
}

// ---- per-language scanners -------------------------------------------------

/// Highlight a line using C-family rules (Rust).
fn hl_c_like(line: &str, kw: &[&str], detect_types: bool) -> Vec<HighlightedToken> {
    let (mut tokens, b, len) = (Vec::new(), line.as_bytes(), line.len());
    let mut i = 0;
    while i < len {
        let ch = b[i];
        if ch == b' ' || ch == b'\t' { i = eat_ws(&mut tokens, line, b, i); }
        else if i + 1 < len && ch == b'/' && b[i + 1] == b'/' {
            tokens.push(HighlightedToken::new(String::from(&line[i..]), TokenKind::Comment));
            return tokens;
        }
        else if ch == b'"' || ch == b'\'' { i = eat_string(&mut tokens, line, b, i); }
        else if ch.is_ascii_digit() { i = eat_number(&mut tokens, line, b, i); }
        else if ch.is_ascii_alphabetic() || ch == b'_' {
            i = eat_word(&mut tokens, line, b, i, kw, detect_types);
        } else {
            let kind = match ch {
                b'{' | b'}' | b'(' | b')' | b'[' | b']' | b';' | b',' | b'.'
                    => TokenKind::Punctuation,
                _ => TokenKind::Operator,
            };
            tokens.push(HighlightedToken::new(String::from(&line[i..i + 1]), kind));
            i += 1;
        }
    }
    tokens
}

/// Highlight a line of shell script.
fn hl_shell(line: &str) -> Vec<HighlightedToken> {
    let (mut tokens, b, len) = (Vec::new(), line.as_bytes(), line.len());
    let mut i = 0;
    while i < len {
        let ch = b[i];
        if ch == b' ' || ch == b'\t' { i = eat_ws(&mut tokens, line, b, i); }
        else if ch == b'#' {
            tokens.push(HighlightedToken::new(String::from(&line[i..]), TokenKind::Comment));
            return tokens;
        }
        else if ch == b'"' || ch == b'\'' { i = eat_string(&mut tokens, line, b, i); }
        else if ch.is_ascii_digit() { i = eat_number(&mut tokens, line, b, i); }
        else if ch.is_ascii_alphabetic() || ch == b'_' {
            i = eat_word(&mut tokens, line, b, i, SHELL_KEYWORDS, false);
        } else {
            let kind = match ch {
                b';' | b'(' | b')' | b'{' | b'}' => TokenKind::Punctuation,
                _ => TokenKind::Operator,
            };
            tokens.push(HighlightedToken::new(String::from(&line[i..i + 1]), kind));
            i += 1;
        }
    }
    tokens
}

/// Highlight a line of Forth source (whitespace-delimited words).
fn hl_forth(line: &str) -> Vec<HighlightedToken> {
    let (mut tokens, b, len) = (Vec::new(), line.as_bytes(), line.len());
    let mut i = 0;
    while i < len {
        if b[i] == b' ' || b[i] == b'\t' { i = eat_ws(&mut tokens, line, b, i); continue; }
        // backslash line comment
        if b[i] == b'\\' && (i == 0 || b[i - 1] == b' ') {
            tokens.push(HighlightedToken::new(String::from(&line[i..]), TokenKind::Comment));
            return tokens;
        }
        // paren comment
        if b[i] == b'(' && (i + 1 >= len || b[i + 1] == b' ') {
            let start = i;
            while i < len && b[i] != b')' { i += 1; }
            if i < len { i += 1; }
            tokens.push(HighlightedToken::new(String::from(&line[start..i]), TokenKind::Comment));
            continue;
        }
        let start = i;
        while i < len && b[i] != b' ' && b[i] != b'\t' { i += 1; }
        let word = &line[start..i];
        let kind = if is_in(word, FORTH_BUILTINS) { TokenKind::Builtin }
            else if word.as_bytes()[0].is_ascii_digit() { TokenKind::Number }
            else { TokenKind::Identifier };
        tokens.push(HighlightedToken::new(String::from(word), kind));
    }
    tokens
}

/// Highlight a configuration / INI / TOML line.
fn hl_config(line: &str) -> Vec<HighlightedToken> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with(';') {
        return alloc::vec![HighlightedToken::new(String::from(line), TokenKind::Comment)];
    }
    if trimmed.starts_with('[') {
        return alloc::vec![HighlightedToken::new(String::from(line), TokenKind::Type)];
    }
    if let Some(eq) = line.find('=') {
        return alloc::vec![
            HighlightedToken::new(String::from(&line[..eq]), TokenKind::Keyword),
            HighlightedToken::new(String::from(&line[eq..eq + 1]), TokenKind::Operator),
            HighlightedToken::new(String::from(&line[eq + 1..]), TokenKind::String),
        ];
    }
    alloc::vec![HighlightedToken::new(String::from(line), TokenKind::Identifier)]
}
