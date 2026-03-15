/// Simple calculator — evaluates arithmetic expressions.
/// Supports: +, -, *, /, %, parentheses, and integer operands.
/// Uses recursive descent parsing.
///
///   calc 2 + 3 * 4      → 14
///   calc (2 + 3) * 4    → 20
///   calc 100 / 7         → 14
///   calc 2 * (10 - 3)   → 14

use alloc::string::String;

/// Evaluate an arithmetic expression string.
pub fn eval(expr: &str) -> Result<i64, &'static str> {
    let tokens = tokenize(expr)?;
    let mut pos = 0;
    let result = parse_expr(&tokens, &mut pos)?;
    if pos < tokens.len() {
        return Err("unexpected token after expression");
    }
    Ok(result)
}

#[derive(Debug, Clone)]
enum Token {
    Num(i64),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    LParen,
    RParen,
}

fn tokenize(s: &str) -> Result<alloc::vec::Vec<Token>, &'static str> {
    let mut tokens = alloc::vec::Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' => i += 1,
            b'+' => { tokens.push(Token::Plus); i += 1; }
            b'-' => {
                // Unary minus: if at start, or after operator/lparen
                let is_unary = tokens.is_empty() || matches!(
                    tokens.last(),
                    Some(Token::Plus | Token::Minus | Token::Star |
                         Token::Slash | Token::Percent | Token::LParen)
                );
                if is_unary && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
                    i += 1;
                    let start = i;
                    while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
                    let n: i64 = core::str::from_utf8(&bytes[start..i])
                        .map_err(|_| "bad number")?
                        .parse().map_err(|_| "bad number")?;
                    tokens.push(Token::Num(-n));
                } else {
                    tokens.push(Token::Minus);
                    i += 1;
                }
            }
            b'*' => { tokens.push(Token::Star); i += 1; }
            b'/' => { tokens.push(Token::Slash); i += 1; }
            b'%' => { tokens.push(Token::Percent); i += 1; }
            b'(' => { tokens.push(Token::LParen); i += 1; }
            b')' => { tokens.push(Token::RParen); i += 1; }
            b'0'..=b'9' => {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
                let n: i64 = core::str::from_utf8(&bytes[start..i])
                    .map_err(|_| "bad number")?
                    .parse().map_err(|_| "bad number")?;
                tokens.push(Token::Num(n));
            }
            _ => return Err("unexpected character"),
        }
    }

    Ok(tokens)
}

// Recursive descent: expr = term (('+' | '-') term)*
fn parse_expr(tokens: &[Token], pos: &mut usize) -> Result<i64, &'static str> {
    let mut left = parse_term(tokens, pos)?;
    while *pos < tokens.len() {
        match tokens[*pos] {
            Token::Plus => { *pos += 1; left += parse_term(tokens, pos)?; }
            Token::Minus => { *pos += 1; left -= parse_term(tokens, pos)?; }
            _ => break,
        }
    }
    Ok(left)
}

// term = factor (('*' | '/' | '%') factor)*
fn parse_term(tokens: &[Token], pos: &mut usize) -> Result<i64, &'static str> {
    let mut left = parse_factor(tokens, pos)?;
    while *pos < tokens.len() {
        match tokens[*pos] {
            Token::Star => { *pos += 1; left *= parse_factor(tokens, pos)?; }
            Token::Slash => {
                *pos += 1;
                let right = parse_factor(tokens, pos)?;
                if right == 0 { return Err("division by zero"); }
                left /= right;
            }
            Token::Percent => {
                *pos += 1;
                let right = parse_factor(tokens, pos)?;
                if right == 0 { return Err("modulo by zero"); }
                left %= right;
            }
            _ => break,
        }
    }
    Ok(left)
}

// factor = '(' expr ')' | number
fn parse_factor(tokens: &[Token], pos: &mut usize) -> Result<i64, &'static str> {
    if *pos >= tokens.len() {
        return Err("unexpected end of expression");
    }
    match &tokens[*pos] {
        Token::Num(n) => { let v = *n; *pos += 1; Ok(v) }
        Token::LParen => {
            *pos += 1;
            let val = parse_expr(tokens, pos)?;
            if *pos >= tokens.len() || !matches!(tokens[*pos], Token::RParen) {
                return Err("missing closing parenthesis");
            }
            *pos += 1;
            Ok(val)
        }
        _ => Err("expected number or '('"),
    }
}

/// Format result with commas for readability (e.g., 1,234,567).
pub fn format_number(n: i64) -> String {
    if n == 0 { return String::from("0"); }

    let neg = n < 0;
    let mut val = if neg { (-n) as u64 } else { n as u64 };
    let mut digits = alloc::vec::Vec::new();
    let mut count = 0;

    while val > 0 {
        if count > 0 && count % 3 == 0 {
            digits.push(b',');
        }
        digits.push(b'0' + (val % 10) as u8);
        val /= 10;
        count += 1;
    }

    if neg { digits.push(b'-'); }
    digits.reverse();

    String::from_utf8(digits).unwrap_or_else(|_| String::from("?"))
}
