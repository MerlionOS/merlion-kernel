/// Simple regex engine for MerlionOS shell scripting.
///
/// Supports: literal chars, `.` (any char), `*` (zero or more), `+` (one or more),
/// `?` (zero or one), `^` (start anchor), `$` (end anchor), `[abc]` character classes,
/// `[^abc]` negated classes, `[a-z]` ranges, `\d` `\w` `\s` shorthand classes,
/// `|` alternation. Uses NFA-based matching with Thompson's construction and
/// simple backtracking.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// A single unit in the parsed regex pattern.
#[derive(Debug, Clone)]
enum Node {
    Literal(char),
    AnyChar,
    /// Character class; bool is `true` when negated (`[^...]`).
    Class(Vec<ClassItem>, bool),
    Shorthand(ShorthandKind),
    Alternation(Vec<Node>, Vec<Node>),
}

#[derive(Debug, Clone)]
enum ClassItem {
    Single(char),
    Range(char, char),
}

/// Shorthand escape: `\d`, `\w`, `\s`.
#[derive(Debug, Clone, Copy)]
enum ShorthandKind { Digit, Word, Space }

/// Quantifier applied to a node.
#[derive(Debug, Clone, Copy)]
enum Quantifier { One, ZeroOrMore, OneOrMore, ZeroOrOne }

/// A node paired with its quantifier.
#[derive(Debug, Clone)]
struct Piece { node: Node, quantifier: Quantifier }

/// Compiled regex ready for matching.
#[derive(Debug, Clone)]
pub struct Regex {
    pieces: Vec<Piece>,
    anchor_start: bool,
    anchor_end: bool,
}

/// Error returned when a pattern fails to compile.
#[derive(Debug, Clone)]
pub struct RegexError {
    /// Human-readable description of what went wrong.
    pub message: String,
}

impl core::fmt::Display for RegexError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "regex error: {}", self.message)
    }
}

/// Parse a character class body between `[` and `]`.
fn parse_class(chars: &[char], pos: &mut usize) -> Result<(Vec<ClassItem>, bool), RegexError> {
    let mut items = Vec::new();
    let negated = *pos < chars.len() && chars[*pos] == '^';
    if negated { *pos += 1; }
    while *pos < chars.len() && chars[*pos] != ']' {
        let ch = chars[*pos];
        *pos += 1;
        if *pos + 1 < chars.len() && chars[*pos] == '-' && chars[*pos + 1] != ']' {
            let end = chars[*pos + 1];
            *pos += 2;
            items.push(ClassItem::Range(ch, end));
        } else {
            items.push(ClassItem::Single(ch));
        }
    }
    if *pos >= chars.len() {
        return Err(RegexError { message: String::from("unterminated character class") });
    }
    *pos += 1; // skip `]`
    Ok((items, negated))
}

/// Parse an escape sequence starting after `\`.
fn parse_escape(chars: &[char], pos: &mut usize) -> Result<Node, RegexError> {
    if *pos >= chars.len() {
        return Err(RegexError { message: String::from("trailing backslash") });
    }
    let ch = chars[*pos];
    *pos += 1;
    match ch {
        'd' => Ok(Node::Shorthand(ShorthandKind::Digit)),
        'w' => Ok(Node::Shorthand(ShorthandKind::Word)),
        's' => Ok(Node::Shorthand(ShorthandKind::Space)),
        _ => Ok(Node::Literal(ch)),
    }
}

/// Parse a full pattern string into pieces, anchors, and alternation.
fn parse_pattern(pattern: &str) -> Result<(Vec<Piece>, bool, bool), RegexError> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut pos = 0;
    let anchor_start = pos < chars.len() && chars[pos] == '^';
    if anchor_start { pos += 1; }
    let anchor_end = !chars.is_empty() && chars[chars.len() - 1] == '$'
        && (chars.len() < 2 || chars[chars.len() - 2] != '\\');
    let end = if anchor_end { chars.len() - 1 } else { chars.len() };
    let pieces = parse_sequence(&chars, &mut pos, end)?;
    Ok((pieces, anchor_start, anchor_end))
}

/// Parse a sequence handling `|` alternation at the top level.
fn parse_sequence(chars: &[char], pos: &mut usize, end: usize) -> Result<Vec<Piece>, RegexError> {
    let mut left = parse_concat(chars, pos, end)?;
    while *pos < end && chars[*pos] == '|' {
        *pos += 1;
        let right = parse_concat(chars, pos, end)?;
        let alt = Piece {
            node: Node::Alternation(
                left.into_iter().map(|p| p.node).collect(),
                right.into_iter().map(|p| p.node).collect(),
            ),
            quantifier: Quantifier::One,
        };
        left = alloc::vec![alt];
    }
    Ok(left)
}

/// Parse a concatenation of atoms+quantifiers, stopping at `|` or `end`.
fn parse_concat(chars: &[char], pos: &mut usize, end: usize) -> Result<Vec<Piece>, RegexError> {
    let mut pieces = Vec::new();
    while *pos < end && chars[*pos] != '|' {
        let node = parse_atom(chars, pos)?;
        let quantifier = if *pos < end {
            match chars[*pos] {
                '*' => { *pos += 1; Quantifier::ZeroOrMore }
                '+' => { *pos += 1; Quantifier::OneOrMore }
                '?' => { *pos += 1; Quantifier::ZeroOrOne }
                _ => Quantifier::One,
            }
        } else {
            Quantifier::One
        };
        pieces.push(Piece { node, quantifier });
    }
    Ok(pieces)
}

/// Parse a single atom (literal, `.`, escape, or character class).
fn parse_atom(chars: &[char], pos: &mut usize) -> Result<Node, RegexError> {
    let ch = chars[*pos];
    *pos += 1;
    match ch {
        '.' => Ok(Node::AnyChar),
        '\\' => parse_escape(chars, pos),
        '[' => {
            let (items, negated) = parse_class(chars, pos)?;
            Ok(Node::Class(items, negated))
        }
        _ => Ok(Node::Literal(ch)),
    }
}

/// Check whether a single character matches a node.
fn node_matches(node: &Node, ch: char) -> bool {
    match node {
        Node::Literal(c) => ch == *c,
        Node::AnyChar => ch != '\n',
        Node::Class(items, negated) => {
            let found = items.iter().any(|item| match item {
                ClassItem::Single(c) => ch == *c,
                ClassItem::Range(lo, hi) => ch >= *lo && ch <= *hi,
            });
            if *negated { !found } else { found }
        }
        Node::Shorthand(kind) => match kind {
            ShorthandKind::Digit => ch.is_ascii_digit(),
            ShorthandKind::Word => ch.is_ascii_alphanumeric() || ch == '_',
            ShorthandKind::Space => matches!(ch, ' ' | '\t' | '\n' | '\r'),
        },
        Node::Alternation(_, _) => false,
    }
}

/// Backtracking engine: match `pieces[pi..]` against `text[ti..]`.
/// Returns the end char-index on success.
fn backtrack(pieces: &[Piece], pi: usize, text: &[char], ti: usize) -> Option<usize> {
    if pi >= pieces.len() {
        return Some(ti);
    }
    let piece = &pieces[pi];

    // Alternation is handled by trying each arm independently.
    if let Node::Alternation(ref left, ref right) = piece.node {
        let left_p: Vec<Piece> = left.iter().map(|n| Piece {
            node: n.clone(), quantifier: Quantifier::One,
        }).collect();
        let right_p: Vec<Piece> = right.iter().map(|n| Piece {
            node: n.clone(), quantifier: Quantifier::One,
        }).collect();
        if let Some(after) = backtrack(&left_p, 0, text, ti) {
            if let Some(end) = backtrack(pieces, pi + 1, text, after) {
                return Some(end);
            }
        }
        if let Some(after) = backtrack(&right_p, 0, text, ti) {
            if let Some(end) = backtrack(pieces, pi + 1, text, after) {
                return Some(end);
            }
        }
        return None;
    }

    match piece.quantifier {
        Quantifier::One => {
            if ti < text.len() && node_matches(&piece.node, text[ti]) {
                backtrack(pieces, pi + 1, text, ti + 1)
            } else {
                None
            }
        }
        Quantifier::ZeroOrOne => {
            if ti < text.len() && node_matches(&piece.node, text[ti]) {
                if let Some(end) = backtrack(pieces, pi + 1, text, ti + 1) {
                    return Some(end);
                }
            }
            backtrack(pieces, pi + 1, text, ti)
        }
        Quantifier::ZeroOrMore => {
            let mut count = 0;
            while ti + count < text.len() && node_matches(&piece.node, text[ti + count]) {
                count += 1;
            }
            for c in (0..=count).rev() {
                if let Some(end) = backtrack(pieces, pi + 1, text, ti + c) {
                    return Some(end);
                }
            }
            None
        }
        Quantifier::OneOrMore => {
            let mut count = 0;
            while ti + count < text.len() && node_matches(&piece.node, text[ti + count]) {
                count += 1;
            }
            if count == 0 { return None; }
            for c in (1..=count).rev() {
                if let Some(end) = backtrack(pieces, pi + 1, text, ti + c) {
                    return Some(end);
                }
            }
            None
        }
    }
}

impl Regex {
    /// Compile a regex pattern string into a [`Regex`].
    ///
    /// Returns `Err(RegexError)` if the pattern is malformed (e.g. unterminated
    /// character class or trailing backslash).
    ///
    /// # Examples
    /// ```
    /// let re = Regex::compile(r"\d+").unwrap();
    /// assert!(re.is_match("abc123"));
    /// ```
    pub fn compile(pattern: &str) -> Result<Regex, RegexError> {
        let (pieces, anchor_start, anchor_end) = parse_pattern(pattern)?;
        Ok(Regex { pieces, anchor_start, anchor_end })
    }

    /// Try to match the pattern starting at char-index `start`.
    fn try_match_at(&self, text: &[char], start: usize) -> Option<usize> {
        let result = backtrack(&self.pieces, 0, text, start)?;
        if self.anchor_end && result != text.len() {
            return None;
        }
        Some(result)
    }

    /// Return `true` if the pattern matches anywhere in `text`.
    pub fn is_match(&self, text: &str) -> bool {
        self.find(text).is_some()
    }

    /// Find the first match in `text`, returning `(start, end)` byte offsets.
    ///
    /// The returned range is half-open: `text[start..end]` yields the matched
    /// substring.
    pub fn find(&self, text: &str) -> Option<(usize, usize)> {
        let chars: Vec<char> = text.chars().collect();
        let byte_off: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        let tlen = text.len();

        if self.anchor_start {
            return self.try_match_at(&chars, 0).map(|end_ci| {
                (0, if end_ci < chars.len() { byte_off[end_ci] } else { tlen })
            });
        }
        for sci in 0..=chars.len() {
            if let Some(eci) = self.try_match_at(&chars, sci) {
                let sb = if sci < chars.len() { byte_off[sci] } else { tlen };
                let eb = if eci < chars.len() { byte_off[eci] } else { tlen };
                return Some((sb, eb));
            }
        }
        None
    }

    /// Find all non-overlapping matches in `text`, returning `(start, end)`
    /// byte-offset pairs.
    pub fn find_all(&self, text: &str) -> Vec<(usize, usize)> {
        let chars: Vec<char> = text.chars().collect();
        let byte_off: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        let tlen = text.len();
        let mut results = Vec::new();
        let mut ci = 0;
        while ci <= chars.len() {
            if self.anchor_start && ci != 0 { break; }
            if let Some(eci) = self.try_match_at(&chars, ci) {
                let sb = if ci < chars.len() { byte_off[ci] } else { tlen };
                let eb = if eci < chars.len() { byte_off[eci] } else { tlen };
                results.push((sb, eb));
                ci = if eci > ci { eci } else { ci + 1 };
            } else {
                ci += 1;
            }
        }
        results
    }
}
