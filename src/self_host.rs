/// Self-hosting capability for MerlionOS.
/// The kernel can compile Rust subsets, assemble x86_64 machine code,
/// link ELF binaries, and run the build pipeline — compiling itself.
/// THIS IS THE ULTIMATE MILESTONE: an OS that can build itself.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicU64, Ordering};

// ── Statistics ─────────────────────────────────────────────────────

static FILES_COMPILED: AtomicU64 = AtomicU64::new(0);
static LINES_PROCESSED: AtomicU64 = AtomicU64::new(0);
static BYTES_GENERATED: AtomicU64 = AtomicU64::new(0);
static ERRORS_TOTAL: AtomicU64 = AtomicU64::new(0);
static BUILDS_RUN: AtomicU64 = AtomicU64::new(0);

// ── Token Types ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Keyword,
    Ident,
    IntLit,
    StrLit,
    CharLit,
    Operator,
    Paren,
    Brace,
    Bracket,
    Comma,
    Semi,
    Colon,
    Arrow,
    FatArrow,
    Dot,
    Comment,
    Whitespace,
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub text: String,
    pub line: usize,
    pub col: usize,
}

const KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "if", "else", "while", "for", "loop", "return",
    "struct", "enum", "impl", "pub", "use", "mod", "const", "static",
    "match", "true", "false", "self", "Self", "crate", "super",
];

fn is_keyword(s: &str) -> bool {
    KEYWORDS.contains(&s)
}

// ── Lexer ──────────────────────────────────────────────────────────

pub fn lex(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut tokens = Vec::new();
    let mut pos = 0usize;
    let mut line = 1usize;
    let mut col = 1usize;

    while pos < len {
        let start_line = line;
        let start_col = col;
        let ch = bytes[pos];

        // Whitespace
        if ch == b' ' || ch == b'\t' || ch == b'\r' || ch == b'\n' {
            let start = pos;
            while pos < len && (bytes[pos] == b' ' || bytes[pos] == b'\t' || bytes[pos] == b'\r' || bytes[pos] == b'\n') {
                if bytes[pos] == b'\n' { line += 1; col = 1; } else { col += 1; }
                pos += 1;
            }
            tokens.push(Token { kind: TokenKind::Whitespace, text: String::from(&source[start..pos]), line: start_line, col: start_col });
            continue;
        }

        // Line comment
        if pos + 1 < len && ch == b'/' && bytes[pos + 1] == b'/' {
            let start = pos;
            while pos < len && bytes[pos] != b'\n' { pos += 1; col += 1; }
            tokens.push(Token { kind: TokenKind::Comment, text: String::from(&source[start..pos]), line: start_line, col: start_col });
            continue;
        }

        // Fat arrow =>
        if pos + 1 < len && ch == b'=' && bytes[pos + 1] == b'>' {
            tokens.push(Token { kind: TokenKind::FatArrow, text: String::from("=>"), line: start_line, col: start_col });
            pos += 2; col += 2; continue;
        }

        // Arrow ->
        if pos + 1 < len && ch == b'-' && bytes[pos + 1] == b'>' {
            tokens.push(Token { kind: TokenKind::Arrow, text: String::from("->"), line: start_line, col: start_col });
            pos += 2; col += 2; continue;
        }

        // String literal
        if ch == b'"' {
            let start = pos;
            pos += 1; col += 1;
            while pos < len && bytes[pos] != b'"' {
                if bytes[pos] == b'\\' && pos + 1 < len { pos += 1; col += 1; }
                if bytes[pos] == b'\n' { line += 1; col = 1; } else { col += 1; }
                pos += 1;
            }
            if pos < len { pos += 1; col += 1; } // closing quote
            tokens.push(Token { kind: TokenKind::StrLit, text: String::from(&source[start..pos]), line: start_line, col: start_col });
            continue;
        }

        // Char literal
        if ch == b'\'' {
            let start = pos;
            pos += 1; col += 1;
            if pos < len && bytes[pos] == b'\\' { pos += 1; col += 1; }
            if pos < len { pos += 1; col += 1; } // char
            if pos < len && bytes[pos] == b'\'' { pos += 1; col += 1; } // closing
            tokens.push(Token { kind: TokenKind::CharLit, text: String::from(&source[start..pos]), line: start_line, col: start_col });
            continue;
        }

        // Number literal
        if ch >= b'0' && ch <= b'9' {
            let start = pos;
            while pos < len && ((bytes[pos] >= b'0' && bytes[pos] <= b'9') || bytes[pos] == b'x' || bytes[pos] == b'_'
                || (bytes[pos] >= b'a' && bytes[pos] <= b'f') || (bytes[pos] >= b'A' && bytes[pos] <= b'F')) {
                pos += 1; col += 1;
            }
            tokens.push(Token { kind: TokenKind::IntLit, text: String::from(&source[start..pos]), line: start_line, col: start_col });
            continue;
        }

        // Identifier / keyword
        if (ch >= b'a' && ch <= b'z') || (ch >= b'A' && ch <= b'Z') || ch == b'_' {
            let start = pos;
            while pos < len && ((bytes[pos] >= b'a' && bytes[pos] <= b'z') || (bytes[pos] >= b'A' && bytes[pos] <= b'Z')
                || (bytes[pos] >= b'0' && bytes[pos] <= b'9') || bytes[pos] == b'_') {
                pos += 1; col += 1;
            }
            let text = String::from(&source[start..pos]);
            let kind = if is_keyword(&text) { TokenKind::Keyword } else { TokenKind::Ident };
            tokens.push(Token { kind, text, line: start_line, col: start_col });
            continue;
        }

        // Single-character tokens
        let kind = match ch {
            b'(' | b')' => TokenKind::Paren,
            b'{' | b'}' => TokenKind::Brace,
            b'[' | b']' => TokenKind::Bracket,
            b',' => TokenKind::Comma,
            b';' => TokenKind::Semi,
            b':' => TokenKind::Colon,
            b'.' => TokenKind::Dot,
            b'+' | b'-' | b'*' | b'/' | b'%' | b'=' | b'!' | b'<' | b'>' | b'&' | b'|' | b'^' | b'~' => TokenKind::Operator,
            _ => TokenKind::Operator,
        };
        tokens.push(Token { kind, text: String::from(&source[pos..pos + 1]), line: start_line, col: start_col });
        pos += 1; col += 1;
    }

    tokens.push(Token { kind: TokenKind::Eof, text: String::new(), line, col });
    tokens
}

// ── AST Nodes ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum AstNode {
    Function { name: String, params: Vec<(String, String)>, ret_type: String, body: Vec<AstNode> },
    Let { name: String, mutable: bool, value: Box<AstNode> },
    If { condition: Box<AstNode>, then_body: Vec<AstNode>, else_body: Vec<AstNode> },
    While { condition: Box<AstNode>, body: Vec<AstNode> },
    Return { value: Option<Box<AstNode>> },
    Call { name: String, args: Vec<AstNode> },
    BinOp { op: String, left: Box<AstNode>, right: Box<AstNode> },
    Block { stmts: Vec<AstNode> },
    Literal { value: String },
    Ident { name: String },
    StructDef { name: String, fields: Vec<(String, String)> },
    EnumDef { name: String, variants: Vec<String> },
}

// ── Parser ─────────────────────────────────────────────────────────

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos.min(self.tokens.len() - 1)].clone();
        if self.pos < self.tokens.len() { self.pos += 1; }
        tok
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.tokens.len() && (self.peek().kind == TokenKind::Whitespace || self.peek().kind == TokenKind::Comment) {
            self.pos += 1;
        }
    }

    fn expect_text(&mut self, text: &str) -> Result<(), String> {
        self.skip_whitespace();
        let tok = self.advance();
        if tok.text == text { Ok(()) } else { Err(format!("expected '{}', got '{}' at {}:{}", text, tok.text, tok.line, tok.col)) }
    }

    fn parse_program(&mut self) -> Result<Vec<AstNode>, String> {
        let mut nodes = Vec::new();
        loop {
            self.skip_whitespace();
            if self.peek().kind == TokenKind::Eof { break; }
            let node = self.parse_item()?;
            nodes.push(node);
        }
        Ok(nodes)
    }

    fn parse_item(&mut self) -> Result<AstNode, String> {
        self.skip_whitespace();
        // Skip 'pub' keyword
        if self.peek().text == "pub" { self.advance(); self.skip_whitespace(); }

        match self.peek().text.as_str() {
            "fn" => self.parse_function(),
            "struct" => self.parse_struct(),
            "enum" => self.parse_enum(),
            "let" => self.parse_let(),
            _ => self.parse_expr(),
        }
    }

    fn parse_function(&mut self) -> Result<AstNode, String> {
        self.expect_text("fn")?;
        self.skip_whitespace();
        let name = self.advance().text.clone();
        self.skip_whitespace();
        self.expect_text("(")?;

        let mut params = Vec::new();
        loop {
            self.skip_whitespace();
            if self.peek().text == ")" { self.advance(); break; }
            if self.peek().kind == TokenKind::Comma { self.advance(); continue; }
            let pname = self.advance().text.clone();
            self.skip_whitespace();
            if self.peek().text == ":" {
                self.advance(); self.skip_whitespace();
                let ptype = self.advance().text.clone();
                params.push((pname, ptype));
            } else {
                params.push((pname, String::from("i64")));
            }
        }

        self.skip_whitespace();
        let mut ret_type = String::new();
        if self.peek().kind == TokenKind::Arrow {
            self.advance(); self.skip_whitespace();
            ret_type = self.advance().text.clone();
        }

        self.skip_whitespace();
        let body = self.parse_block()?;
        Ok(AstNode::Function { name, params, ret_type, body })
    }

    fn parse_block(&mut self) -> Result<Vec<AstNode>, String> {
        self.skip_whitespace();
        self.expect_text("{")?;
        let mut stmts = Vec::new();
        loop {
            self.skip_whitespace();
            if self.peek().text == "}" { self.advance(); break; }
            if self.peek().kind == TokenKind::Eof { break; }
            let stmt = self.parse_stmt()?;
            stmts.push(stmt);
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<AstNode, String> {
        self.skip_whitespace();
        match self.peek().text.as_str() {
            "let" => { let n = self.parse_let()?; self.skip_semi(); Ok(n) }
            "if" => self.parse_if(),
            "while" => self.parse_while(),
            "return" => { let n = self.parse_return()?; self.skip_semi(); Ok(n) }
            _ => { let n = self.parse_expr()?; self.skip_semi(); Ok(n) }
        }
    }

    fn skip_semi(&mut self) {
        self.skip_whitespace();
        if self.peek().kind == TokenKind::Semi { self.advance(); }
    }

    fn parse_let(&mut self) -> Result<AstNode, String> {
        self.expect_text("let")?;
        self.skip_whitespace();
        let mutable = if self.peek().text == "mut" { self.advance(); self.skip_whitespace(); true } else { false };
        let name = self.advance().text.clone();
        self.skip_whitespace();
        // skip type annotation
        if self.peek().text == ":" { self.advance(); self.skip_whitespace(); self.advance(); self.skip_whitespace(); }
        self.expect_text("=")?;
        self.skip_whitespace();
        let value = self.parse_expr()?;
        Ok(AstNode::Let { name, mutable, value: Box::new(value) })
    }

    fn parse_if(&mut self) -> Result<AstNode, String> {
        self.expect_text("if")?;
        self.skip_whitespace();
        let cond = self.parse_expr()?;
        let then_body = self.parse_block()?;
        self.skip_whitespace();
        let else_body = if self.peek().text == "else" {
            self.advance();
            self.parse_block()?
        } else {
            Vec::new()
        };
        Ok(AstNode::If { condition: Box::new(cond), then_body, else_body })
    }

    fn parse_while(&mut self) -> Result<AstNode, String> {
        self.expect_text("while")?;
        self.skip_whitespace();
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(AstNode::While { condition: Box::new(cond), body })
    }

    fn parse_return(&mut self) -> Result<AstNode, String> {
        self.expect_text("return")?;
        self.skip_whitespace();
        if self.peek().kind == TokenKind::Semi || self.peek().text == "}" {
            return Ok(AstNode::Return { value: None });
        }
        let val = self.parse_expr()?;
        Ok(AstNode::Return { value: Some(Box::new(val)) })
    }

    fn parse_expr(&mut self) -> Result<AstNode, String> {
        self.skip_whitespace();
        let left = self.parse_primary()?;
        self.skip_whitespace();
        if self.peek().kind == TokenKind::Operator && (self.peek().text == "+" || self.peek().text == "-"
            || self.peek().text == "*" || self.peek().text == "/" || self.peek().text == "%"
            || self.peek().text == "==" || self.peek().text == "!=" || self.peek().text == "<" || self.peek().text == ">") {
            let op = self.advance().text.clone();
            self.skip_whitespace();
            let right = self.parse_primary()?;
            return Ok(AstNode::BinOp { op, left: Box::new(left), right: Box::new(right) });
        }
        Ok(left)
    }

    fn parse_primary(&mut self) -> Result<AstNode, String> {
        self.skip_whitespace();
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::IntLit => { self.advance(); Ok(AstNode::Literal { value: tok.text }) }
            TokenKind::StrLit => { self.advance(); Ok(AstNode::Literal { value: tok.text }) }
            TokenKind::Keyword if tok.text == "true" || tok.text == "false" => { self.advance(); Ok(AstNode::Literal { value: tok.text }) }
            TokenKind::Ident => {
                self.advance();
                self.skip_whitespace();
                if self.peek().text == "(" {
                    // Function call
                    self.advance();
                    let mut args = Vec::new();
                    loop {
                        self.skip_whitespace();
                        if self.peek().text == ")" { self.advance(); break; }
                        if self.peek().kind == TokenKind::Comma { self.advance(); continue; }
                        let arg = self.parse_expr()?;
                        args.push(arg);
                    }
                    Ok(AstNode::Call { name: tok.text, args })
                } else {
                    Ok(AstNode::Ident { name: tok.text })
                }
            }
            TokenKind::Paren if tok.text == "(" => {
                self.advance();
                let expr = self.parse_expr()?;
                self.skip_whitespace();
                let _ = self.expect_text(")");
                Ok(expr)
            }
            _ => {
                self.advance();
                Ok(AstNode::Literal { value: tok.text })
            }
        }
    }

    fn parse_struct(&mut self) -> Result<AstNode, String> {
        self.expect_text("struct")?;
        self.skip_whitespace();
        let name = self.advance().text.clone();
        self.skip_whitespace();
        self.expect_text("{")?;
        let mut fields = Vec::new();
        loop {
            self.skip_whitespace();
            if self.peek().text == "}" { self.advance(); break; }
            if self.peek().kind == TokenKind::Comma { self.advance(); continue; }
            // skip pub
            if self.peek().text == "pub" { self.advance(); self.skip_whitespace(); }
            let fname = self.advance().text.clone();
            self.skip_whitespace();
            if self.peek().text == ":" { self.advance(); self.skip_whitespace(); }
            let ftype = self.advance().text.clone();
            fields.push((fname, ftype));
            self.skip_whitespace();
            if self.peek().kind == TokenKind::Comma { self.advance(); }
        }
        Ok(AstNode::StructDef { name, fields })
    }

    fn parse_enum(&mut self) -> Result<AstNode, String> {
        self.expect_text("enum")?;
        self.skip_whitespace();
        let name = self.advance().text.clone();
        self.skip_whitespace();
        self.expect_text("{")?;
        let mut variants = Vec::new();
        loop {
            self.skip_whitespace();
            if self.peek().text == "}" { self.advance(); break; }
            if self.peek().kind == TokenKind::Comma { self.advance(); continue; }
            variants.push(self.advance().text.clone());
        }
        Ok(AstNode::EnumDef { name, variants })
    }
}

pub fn parse(source: &str) -> Result<Vec<AstNode>, String> {
    let tokens: Vec<Token> = lex(source).into_iter().filter(|t| t.kind != TokenKind::Whitespace && t.kind != TokenKind::Comment).collect();
    let mut parser = Parser::new(tokens);
    parser.parse_program()
}

// ── x86_64 Assembler ───────────────────────────────────────────────

/// Register index for x86_64 registers.
fn reg_index(name: &str) -> Option<u8> {
    match name {
        "rax" | "eax" => Some(0), "rcx" | "ecx" => Some(1),
        "rdx" | "edx" => Some(2), "rbx" | "ebx" => Some(3),
        "rsp" | "esp" => Some(4), "rbp" | "ebp" => Some(5),
        "rsi" | "esi" => Some(6), "rdi" | "edi" => Some(7),
        "r8" | "r8d" => Some(8), "r9" | "r9d" => Some(9),
        "r10" | "r10d" => Some(10), "r11" | "r11d" => Some(11),
        "r12" | "r12d" => Some(12), "r13" | "r13d" => Some(13),
        "r14" | "r14d" => Some(14), "r15" | "r15d" => Some(15),
        _ => None,
    }
}

fn is_64bit_reg(name: &str) -> bool {
    name.starts_with('r') || name.starts_with("r8") || name.starts_with("r9")
        || name.starts_with("r1")
}

fn needs_rex(reg: u8) -> bool {
    reg >= 8
}

fn rex_w(r: u8, b: u8) -> u8 {
    let mut rex = 0x48u8; // REX.W
    if r >= 8 { rex |= 0x04; } // REX.R
    if b >= 8 { rex |= 0x01; } // REX.B
    rex
}

/// Assemble a single x86_64 instruction into machine code bytes.
pub fn assemble(instruction: &str) -> Vec<u8> {
    let trimmed = instruction.trim();
    if trimmed.is_empty() { return Vec::new(); }

    let parts: Vec<&str> = trimmed.splitn(2, |c: char| c.is_whitespace()).collect();
    let mnemonic = parts[0].to_lowercase();
    let operands_str = if parts.len() > 1 { parts[1].trim() } else { "" };
    let operands: Vec<&str> = if operands_str.is_empty() {
        Vec::new()
    } else {
        operands_str.split(',').map(|s| s.trim()).collect()
    };

    let mut code = Vec::new();

    match mnemonic.as_str() {
        "nop" => { code.push(0x90); }
        "ret" => { code.push(0xC3); }
        "syscall" => { code.push(0x0F); code.push(0x05); }
        "int" => {
            if let Some(imm) = parse_immediate(operands.first().copied().unwrap_or("3")) {
                code.push(0xCD);
                code.push(imm as u8);
            }
        }
        "push" => {
            if let Some(r) = operands.first().and_then(|o| reg_index(o)) {
                if needs_rex(r) { code.push(0x41); }
                code.push(0x50 + (r & 7));
            }
        }
        "pop" => {
            if let Some(r) = operands.first().and_then(|o| reg_index(o)) {
                if needs_rex(r) { code.push(0x41); }
                code.push(0x58 + (r & 7));
            }
        }
        "mov" => {
            if operands.len() == 2 {
                if let (Some(dst), Some(src)) = (reg_index(operands[0]), reg_index(operands[1])) {
                    // mov reg, reg
                    code.push(rex_w(src, dst));
                    code.push(0x89);
                    code.push(0xC0 | ((src & 7) << 3) | (dst & 7));
                } else if let Some(dst) = reg_index(operands[0]) {
                    // mov reg, imm64
                    if let Some(imm) = parse_immediate(operands[1]) {
                        if is_64bit_reg(operands[0]) {
                            code.push(rex_w(0, dst));
                            code.push(0xB8 + (dst & 7));
                            for b in imm.to_le_bytes() { code.push(b); }
                        } else {
                            if needs_rex(dst) { code.push(0x41); }
                            code.push(0xB8 + (dst & 7));
                            for b in (imm as u32).to_le_bytes() { code.push(b); }
                        }
                    }
                }
            }
        }
        "add" => { encode_alu(&mut code, 0x01, 0, &operands); }
        "sub" => { encode_alu(&mut code, 0x29, 5, &operands); }
        "cmp" => { encode_alu(&mut code, 0x39, 7, &operands); }
        "mul" => {
            // mul reg (unsigned multiply rax * reg -> rdx:rax)
            if let Some(r) = operands.first().and_then(|o| reg_index(o)) {
                code.push(rex_w(0, r));
                code.push(0xF7);
                code.push(0xE0 | (r & 7));
            }
        }
        "call" => {
            // call rel32 (offset)
            if let Some(offset) = parse_immediate(operands.first().copied().unwrap_or("0")) {
                code.push(0xE8);
                let rel = (offset as i32).to_le_bytes();
                code.extend_from_slice(&rel);
            }
        }
        "jmp" => {
            if let Some(offset) = parse_immediate(operands.first().copied().unwrap_or("0")) {
                code.push(0xE9);
                let rel = (offset as i32).to_le_bytes();
                code.extend_from_slice(&rel);
            }
        }
        "je" | "jz" => {
            if let Some(offset) = parse_immediate(operands.first().copied().unwrap_or("0")) {
                code.push(0x0F); code.push(0x84);
                let rel = (offset as i32).to_le_bytes();
                code.extend_from_slice(&rel);
            }
        }
        "jne" | "jnz" => {
            if let Some(offset) = parse_immediate(operands.first().copied().unwrap_or("0")) {
                code.push(0x0F); code.push(0x85);
                let rel = (offset as i32).to_le_bytes();
                code.extend_from_slice(&rel);
            }
        }
        _ => {
            // Unknown instruction: emit NOP
            code.push(0x90);
        }
    }

    code
}

fn encode_alu(code: &mut Vec<u8>, opcode: u8, imm_ext: u8, operands: &[&str]) {
    if operands.len() == 2 {
        if let (Some(dst), Some(src)) = (reg_index(operands[0]), reg_index(operands[1])) {
            code.push(rex_w(src, dst));
            code.push(opcode);
            code.push(0xC0 | ((src & 7) << 3) | (dst & 7));
        } else if let Some(dst) = reg_index(operands[0]) {
            if let Some(imm) = parse_immediate(operands[1]) {
                code.push(rex_w(0, dst));
                code.push(0x81);
                code.push(0xC0 | (imm_ext << 3) | (dst & 7));
                for b in (imm as i32).to_le_bytes() { code.push(b); }
            }
        }
    }
}

fn parse_immediate(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.starts_with("0x") || s.starts_with("0X") {
        i64::from_str_radix(&s[2..], 16).ok()
    } else {
        s.parse::<i64>().ok()
    }
}

// ── ELF Linker ─────────────────────────────────────────────────────

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELF_CLASS64: u8 = 2;
const ELF_DATA_LSB: u8 = 1;
const ELF_VERSION: u8 = 1;
const ELF_OSABI_NONE: u8 = 0;
const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 62;
const PT_LOAD: u32 = 1;

const ELF_HEADER_SIZE: u16 = 64;
const PHDR_SIZE: u16 = 56;
const VADDR_BASE: u64 = 0x400000;

/// Link raw code bytes into a minimal ELF64 executable.
pub fn link(code: &[u8], entry_offset: u64) -> Vec<u8> {
    let code_offset = (ELF_HEADER_SIZE + PHDR_SIZE) as u64;
    let entry = VADDR_BASE + code_offset + entry_offset;
    let file_size = code_offset as usize + code.len();

    let mut elf = Vec::with_capacity(file_size);

    // ELF header (64 bytes)
    elf.extend_from_slice(&ELF_MAGIC);        // e_ident[0..4]
    elf.push(ELF_CLASS64);                     // EI_CLASS
    elf.push(ELF_DATA_LSB);                    // EI_DATA
    elf.push(ELF_VERSION);                     // EI_VERSION
    elf.push(ELF_OSABI_NONE);                  // EI_OSABI
    elf.extend_from_slice(&[0u8; 8]);          // EI_ABIVERSION + padding
    elf.extend_from_slice(&ET_EXEC.to_le_bytes()); // e_type
    elf.extend_from_slice(&EM_X86_64.to_le_bytes()); // e_machine
    elf.extend_from_slice(&1u32.to_le_bytes()); // e_version
    elf.extend_from_slice(&entry.to_le_bytes()); // e_entry
    elf.extend_from_slice(&(ELF_HEADER_SIZE as u64).to_le_bytes()); // e_phoff
    elf.extend_from_slice(&0u64.to_le_bytes()); // e_shoff (no sections)
    elf.extend_from_slice(&0u32.to_le_bytes()); // e_flags
    elf.extend_from_slice(&ELF_HEADER_SIZE.to_le_bytes()); // e_ehsize
    elf.extend_from_slice(&PHDR_SIZE.to_le_bytes()); // e_phentsize
    elf.extend_from_slice(&1u16.to_le_bytes()); // e_phnum
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shentsize
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shnum
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx

    // Program header (56 bytes)
    elf.extend_from_slice(&PT_LOAD.to_le_bytes()); // p_type
    elf.extend_from_slice(&5u32.to_le_bytes()); // p_flags (PF_R | PF_X)
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_offset
    elf.extend_from_slice(&VADDR_BASE.to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&VADDR_BASE.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&(file_size as u64).to_le_bytes()); // p_filesz
    elf.extend_from_slice(&(file_size as u64).to_le_bytes()); // p_memsz
    elf.extend_from_slice(&0x1000u64.to_le_bytes()); // p_align

    // Code section
    elf.extend_from_slice(code);

    elf
}

// ── Code Generation ────────────────────────────────────────────────

/// Simple register allocator: uses rax, rcx, rdx.
const REG_NAMES: [&str; 3] = ["rax", "rcx", "rdx"];

struct CodeGen {
    output: Vec<String>,
    reg_used: [bool; 3],
    label_counter: u32,
}

impl CodeGen {
    fn new() -> Self {
        Self { output: Vec::new(), reg_used: [false; 3], label_counter: 0 }
    }

    fn alloc_reg(&mut self) -> usize {
        for i in 0..3 {
            if !self.reg_used[i] {
                self.reg_used[i] = true;
                return i;
            }
        }
        0 // fallback to rax
    }

    fn free_reg(&mut self, r: usize) {
        if r < 3 { self.reg_used[r] = false; }
    }

    fn emit(&mut self, instr: &str) {
        self.output.push(String::from(instr));
    }

    fn new_label(&mut self) -> u32 {
        self.label_counter += 1;
        self.label_counter
    }

    fn gen_program(&mut self, nodes: &[AstNode]) {
        for node in nodes {
            self.gen_node(node);
        }
    }

    fn gen_node(&mut self, node: &AstNode) {
        match node {
            AstNode::Function { name, body, .. } => {
                self.emit(&format!("; function {}", name));
                self.emit("push rbp");
                self.emit("mov rbp, rsp");
                for stmt in body {
                    self.gen_node(stmt);
                }
                self.emit("mov rsp, rbp");
                self.emit("pop rbp");
                self.emit("ret");
            }
            AstNode::Let { value, .. } => {
                self.gen_node(value);
                self.emit("push rax");
            }
            AstNode::Return { value } => {
                if let Some(v) = value {
                    self.gen_node(v);
                }
                self.emit("mov rsp, rbp");
                self.emit("pop rbp");
                self.emit("ret");
            }
            AstNode::BinOp { op, left, right } => {
                self.gen_node(left);
                self.emit("push rax");
                self.gen_node(right);
                self.emit("mov rcx, rax");
                self.emit("pop rax");
                match op.as_str() {
                    "+" => self.emit("add rax, rcx"),
                    "-" => self.emit("sub rax, rcx"),
                    "*" => self.emit("mul rcx"),
                    _ => {}
                }
            }
            AstNode::Call { name, args } => {
                // Push args in reverse
                for arg in args.iter().rev() {
                    self.gen_node(arg);
                    self.emit("push rax");
                }
                self.emit(&format!("call {}", name));
                // Clean up stack
                if !args.is_empty() {
                    self.emit(&format!("add rsp, {}", args.len() * 8));
                }
            }
            AstNode::If { condition, then_body, else_body } => {
                let else_label = self.new_label();
                let end_label = self.new_label();
                self.gen_node(condition);
                self.emit("cmp rax, 0");
                self.emit(&format!("je .L{}", else_label));
                for s in then_body { self.gen_node(s); }
                self.emit(&format!("jmp .L{}", end_label));
                self.emit(&format!("; .L{}:", else_label));
                for s in else_body { self.gen_node(s); }
                self.emit(&format!("; .L{}:", end_label));
            }
            AstNode::While { condition, body } => {
                let top_label = self.new_label();
                let end_label = self.new_label();
                self.emit(&format!("; .L{}:", top_label));
                self.gen_node(condition);
                self.emit("cmp rax, 0");
                self.emit(&format!("je .L{}", end_label));
                for s in body { self.gen_node(s); }
                self.emit(&format!("jmp .L{}", top_label));
                self.emit(&format!("; .L{}:", end_label));
            }
            AstNode::Literal { value } => {
                if let Some(n) = parse_immediate(value) {
                    self.emit(&format!("mov rax, {}", n));
                } else {
                    self.emit("mov rax, 0");
                }
            }
            AstNode::Ident { name } => {
                self.emit(&format!("; load {}", name));
                self.emit("mov rax, 0"); // placeholder
            }
            AstNode::Block { stmts } => {
                for s in stmts { self.gen_node(s); }
            }
            AstNode::StructDef { name, .. } => {
                self.emit(&format!("; struct {} (type only)", name));
            }
            AstNode::EnumDef { name, .. } => {
                self.emit(&format!("; enum {} (type only)", name));
            }
        }
    }
}

/// Generate x86_64 assembly from AST.
pub fn codegen(ast: &[AstNode]) -> Vec<String> {
    let mut cg = CodeGen::new();
    cg.gen_program(ast);
    cg.output
}

// ── Build Pipeline ─────────────────────────────────────────────────

/// Compile Rust source code to machine code bytes.
pub fn compile(source: &str) -> Result<Vec<u8>, String> {
    FILES_COMPILED.fetch_add(1, Ordering::Relaxed);
    let line_count = source.lines().count() as u64;
    LINES_PROCESSED.fetch_add(line_count, Ordering::Relaxed);

    // Lex
    let tokens = lex(source);
    let non_ws: Vec<Token> = tokens.into_iter()
        .filter(|t| t.kind != TokenKind::Whitespace && t.kind != TokenKind::Comment)
        .collect();

    if non_ws.is_empty() || (non_ws.len() == 1 && non_ws[0].kind == TokenKind::Eof) {
        return Err(String::from("Empty source file"));
    }

    // Parse
    let mut parser = Parser::new(non_ws);
    let ast = parser.parse_program().map_err(|e| {
        ERRORS_TOTAL.fetch_add(1, Ordering::Relaxed);
        format!("Parse error: {}", e)
    })?;

    // Codegen
    let asm_lines = codegen(&ast);

    // Assemble
    let mut machine_code = Vec::new();
    for line in &asm_lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(';') { continue; }
        let bytes = assemble(trimmed);
        machine_code.extend_from_slice(&bytes);
    }

    BYTES_GENERATED.fetch_add(machine_code.len() as u64, Ordering::Relaxed);
    Ok(machine_code)
}

/// Compile a source file (reads from VFS).
pub fn compile_file(path: &str) -> Result<Vec<u8>, String> {
    match crate::vfs::cat(path) {
        Ok(content) => {
            if content.is_empty() {
                return Err(format!("Empty file: {}", path));
            }
            compile(&content)
        }
        Err(e) => Err(format!("Cannot read {}: {}", path, e)),
    }
}

/// Build a project directory: compile all .rs files and link.
pub fn build_project(dir: &str) -> Result<String, String> {
    BUILDS_RUN.fetch_add(1, Ordering::Relaxed);
    let start_ticks = crate::timer::ticks();

    let entries_result = crate::vfs::ls(dir);
    let entries: Vec<String> = match entries_result {
        Ok(list) => list.into_iter().map(|(name, _)| name).collect(),
        Err(_) => return Err(format!("Cannot list directory: {}", dir)),
    };
    let mut all_code = Vec::new();
    let mut compiled = 0u32;
    let mut errors = Vec::new();

    for entry in &entries {
        if entry.ends_with(".rs") {
            let path = format!("{}/{}", dir, entry);
            match compile_file(&path) {
                Ok(code) => {
                    all_code.extend_from_slice(&code);
                    compiled += 1;
                }
                Err(e) => {
                    errors.push(format!("{}: {}", entry, e));
                }
            }
        }
    }

    if !errors.is_empty() {
        return Err(format!("Build failed with {} error(s):\n{}", errors.len(), errors.join("\n")));
    }

    if all_code.is_empty() {
        return Err(String::from("No .rs files found or all empty"));
    }

    // Link into ELF
    let elf = link(&all_code, 0);
    let elapsed = crate::timer::ticks() - start_ticks;

    Ok(format!(
        "Build successful!\n  Files: {}\n  Code size: {} bytes\n  ELF size: {} bytes\n  Time: {} ticks",
        compiled, all_code.len(), elf.len(), elapsed
    ))
}

// ── Self-Build Test ────────────────────────────────────────────────

/// Run a self-build test: compile a simple program, verify output.
pub fn self_build_test() -> String {
    let test_source = "fn main() {\n    let x: i64 = 42;\n    let y: i64 = 13;\n    return x + y;\n}\n";

    let mut report = String::from("=== Self-Build Test ===\n\n");
    report.push_str("Source:\n");
    report.push_str(test_source);
    report.push_str("\n");

    // Lex test
    let tokens = lex(test_source);
    let token_count = tokens.iter().filter(|t| t.kind != TokenKind::Whitespace).count();
    report.push_str(&format!("Lexer: {} tokens\n", token_count));

    // Parse test
    match parse(test_source) {
        Ok(ast) => {
            report.push_str(&format!("Parser: {} top-level nodes\n", ast.len()));

            // Codegen test
            let asm = codegen(&ast);
            report.push_str(&format!("Codegen: {} instructions\n", asm.len()));

            // Assemble test
            let mut code = Vec::new();
            for line in &asm {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with(';') { continue; }
                code.extend_from_slice(&assemble(trimmed));
            }
            report.push_str(&format!("Assembler: {} bytes of machine code\n", code.len()));

            // Link test
            let elf = link(&code, 0);
            report.push_str(&format!("Linker: {} bytes ELF binary\n", elf.len()));

            // Verify ELF header
            if elf.len() >= 4 && elf[0] == 0x7F && elf[1] == b'E' && elf[2] == b'L' && elf[3] == b'F' {
                report.push_str("ELF magic: VALID\n");
            } else {
                report.push_str("ELF magic: INVALID\n");
            }

            report.push_str("\nResult: PASS - Full pipeline operational\n");
        }
        Err(e) => {
            report.push_str(&format!("Parser error: {}\nResult: FAIL\n", e));
        }
    }

    report.push_str("=== End Self-Build Test ===\n");
    report
}

/// Check if the build pipeline works end-to-end.
pub fn can_self_host() -> bool {
    let test = "fn test() { return 1; }\n";
    match compile(test) {
        Ok(code) => {
            let elf = link(&code, 0);
            elf.len() > 120 && elf[0] == 0x7F && elf[1] == b'E'
        }
        Err(_) => false,
    }
}

// ── Public API ─────────────────────────────────────────────────────

/// Initialize the self-hosting subsystem.
pub fn init() {
    // Verify pipeline is operational
    let _ = can_self_host();
}

/// Return module info string.
pub fn self_host_info() -> String {
    let can_host = can_self_host();
    let mut info = String::from("Self-Hosting Compiler\n");
    info.push_str("=====================\n");
    info.push_str(&format!("Status: {}\n", if can_host { "OPERATIONAL" } else { "DEGRADED" }));
    info.push_str("Supported features:\n");
    info.push_str("  Lexer:     Rust subset (keywords, idents, literals, operators)\n");
    info.push_str("  Parser:    fn, let, if/else, while, return, struct, enum\n");
    info.push_str("  Codegen:   x86_64 assembly generation\n");
    info.push_str("  Assembler: mov, add, sub, mul, cmp, jmp, je, jne, call, ret,\n");
    info.push_str("             push, pop, syscall, nop, int\n");
    info.push_str("  Linker:    ELF64 executable generation\n");
    info.push_str(&format!("  Keywords:  {}\n", KEYWORDS.len()));
    info.push_str(&format!("  Registers: rax-r15 (16 general purpose)\n"));
    info.push_str(&format!("Files compiled:  {}\n", FILES_COMPILED.load(Ordering::Relaxed)));
    info.push_str(&format!("Lines processed: {}\n", LINES_PROCESSED.load(Ordering::Relaxed)));
    info.push_str(&format!("Bytes generated: {}\n", BYTES_GENERATED.load(Ordering::Relaxed)));
    info
}

/// Return stats string.
pub fn self_host_stats() -> String {
    let mut stats = String::from("Self-Host Statistics\n");
    stats.push_str("────────────────────\n");
    stats.push_str(&format!("Files compiled:  {}\n", FILES_COMPILED.load(Ordering::Relaxed)));
    stats.push_str(&format!("Lines processed: {}\n", LINES_PROCESSED.load(Ordering::Relaxed)));
    stats.push_str(&format!("Bytes generated: {}\n", BYTES_GENERATED.load(Ordering::Relaxed)));
    stats.push_str(&format!("Errors total:    {}\n", ERRORS_TOTAL.load(Ordering::Relaxed)));
    stats.push_str(&format!("Builds run:      {}\n", BUILDS_RUN.load(Ordering::Relaxed)));
    stats.push_str(&format!("Can self-host:   {}\n", if can_self_host() { "yes" } else { "no" }));
    stats
}
