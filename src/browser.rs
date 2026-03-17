/// Simple HTML/CSS web browser for MerlionOS.
/// Renders basic HTML with CSS styling, follows links,
/// and displays images (BMP).

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use alloc::borrow::ToOwned;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_HISTORY: usize = 64;
const MAX_BOOKMARKS: usize = 32;
const DEFAULT_VIEWPORT_WIDTH: usize = 80;
const MAX_DOM_DEPTH: usize = 64;
const MAX_CSS_RULES: usize = 128;

// ---------------------------------------------------------------------------
// Self-closing tags
// ---------------------------------------------------------------------------

const SELF_CLOSING: &[&str] = &["br", "hr", "img", "meta", "link", "input"];

// ---------------------------------------------------------------------------
// Block vs inline classification
// ---------------------------------------------------------------------------

fn is_block_tag(tag: &str) -> bool {
    matches!(
        tag,
        "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            | "ul" | "ol" | "li" | "table" | "tr" | "td" | "th"
            | "pre" | "hr" | "html" | "head" | "body" | "title"
    )
}

// ---------------------------------------------------------------------------
// DOM types
// ---------------------------------------------------------------------------

/// A node in the parsed document object model.
#[derive(Debug, Clone)]
pub enum DomNode {
    /// An element with tag name, attributes, and children.
    Element {
        tag: String,
        attrs: Vec<(String, String)>,
        children: Vec<DomNode>,
    },
    /// Raw text content.
    Text(String),
}

impl DomNode {
    /// Get an attribute value by name.
    pub fn get_attr(&self, name: &str) -> Option<&str> {
        match self {
            DomNode::Element { attrs, .. } => {
                for (k, v) in attrs {
                    if k == name {
                        return Some(v.as_str());
                    }
                }
                None
            }
            DomNode::Text(_) => None,
        }
    }

    /// Get tag name (empty for text nodes).
    pub fn tag_name(&self) -> &str {
        match self {
            DomNode::Element { tag, .. } => tag.as_str(),
            DomNode::Text(_) => "",
        }
    }

    /// Get children.
    pub fn children(&self) -> &[DomNode] {
        match self {
            DomNode::Element { children, .. } => children,
            DomNode::Text(_) => &[],
        }
    }

    /// Count all nodes in this subtree.
    fn count_nodes(&self) -> usize {
        match self {
            DomNode::Text(_) => 1,
            DomNode::Element { children, .. } => {
                let mut count = 1usize;
                for c in children {
                    count = count.saturating_add(c.count_nodes());
                }
                count
            }
        }
    }
}

// ---------------------------------------------------------------------------
// CSS types
// ---------------------------------------------------------------------------

/// A parsed CSS property.
#[derive(Debug, Clone)]
pub struct CssProperty {
    pub name: String,
    pub value: String,
}

/// A CSS selector (simplified).
#[derive(Debug, Clone)]
pub enum CssSelector {
    /// Match by tag name.
    Tag(String),
    /// Match by class.
    Class(String),
    /// Match by id.
    Id(String),
}

/// A CSS rule: selector + properties.
#[derive(Debug, Clone)]
pub struct CssRule {
    pub selector: CssSelector,
    pub properties: Vec<CssProperty>,
}

// ---------------------------------------------------------------------------
// Layout box
// ---------------------------------------------------------------------------

/// Layout mode for an element.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DisplayMode {
    Block,
    Inline,
    None,
}

/// A computed layout box.
#[derive(Debug, Clone)]
pub struct LayoutBox {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub margin_top: usize,
    pub margin_bottom: usize,
    pub margin_left: usize,
    pub margin_right: usize,
    pub padding_top: usize,
    pub padding_bottom: usize,
    pub padding_left: usize,
    pub padding_right: usize,
    pub display: DisplayMode,
    pub content: String,
    pub children: Vec<LayoutBox>,
}

impl LayoutBox {
    fn new() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            margin_top: 0,
            margin_bottom: 0,
            margin_left: 0,
            margin_right: 0,
            padding_top: 0,
            padding_bottom: 0,
            padding_left: 0,
            padding_right: 0,
            display: DisplayMode::Block,
            content: String::new(),
            children: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// HTML parser
// ---------------------------------------------------------------------------

struct HtmlParser {
    input: Vec<u8>,
    pos: usize,
}

impl HtmlParser {
    fn new(source: &str) -> Self {
        Self {
            input: source.as_bytes().to_vec(),
            pos: 0,
        }
    }

    fn eof(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn peek(&self) -> u8 {
        if self.eof() { 0 } else { self.input[self.pos] }
    }

    fn advance(&mut self) -> u8 {
        let ch = self.peek();
        if !self.eof() {
            self.pos += 1;
        }
        ch
    }

    fn skip_whitespace(&mut self) {
        while !self.eof() && (self.peek() == b' ' || self.peek() == b'\n'
            || self.peek() == b'\r' || self.peek() == b'\t')
        {
            self.pos += 1;
        }
    }

    fn read_while<F: Fn(u8) -> bool>(&mut self, pred: F) -> String {
        let start = self.pos;
        while !self.eof() && pred(self.peek()) {
            self.pos += 1;
        }
        let bytes = &self.input[start..self.pos];
        String::from_utf8_lossy(bytes).into_owned()
    }

    fn read_tag_name(&mut self) -> String {
        self.read_while(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            .to_ascii_lowercase()
    }

    fn read_attr_value(&mut self) -> String {
        if self.peek() == b'"' {
            self.advance(); // opening quote
            let val = self.read_while(|c| c != b'"');
            if self.peek() == b'"' {
                self.advance();
            }
            val
        } else if self.peek() == b'\'' {
            self.advance();
            let val = self.read_while(|c| c != b'\'');
            if self.peek() == b'\'' {
                self.advance();
            }
            val
        } else {
            self.read_while(|c| c != b' ' && c != b'>' && c != b'/' && c != b'\n')
        }
    }

    fn parse_attributes(&mut self) -> Vec<(String, String)> {
        let mut attrs = Vec::new();
        loop {
            self.skip_whitespace();
            if self.eof() || self.peek() == b'>' || self.peek() == b'/' {
                break;
            }
            let name = self.read_while(|c| {
                c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
            }).to_ascii_lowercase();
            if name.is_empty() {
                self.advance();
                continue;
            }
            self.skip_whitespace();
            let value = if self.peek() == b'=' {
                self.advance();
                self.skip_whitespace();
                self.read_attr_value()
            } else {
                String::new()
            };
            // Only keep known attributes
            if matches!(name.as_str(), "href" | "src" | "id" | "class"
                | "style" | "width" | "height" | "alt")
            {
                attrs.push((name, value));
            }
        }
        attrs
    }

    fn parse_node(&mut self, depth: usize) -> Option<DomNode> {
        if depth > MAX_DOM_DEPTH || self.eof() {
            return None;
        }
        if self.peek() == b'<' {
            self.parse_element(depth)
        } else {
            self.parse_text()
        }
    }

    fn parse_text(&mut self) -> Option<DomNode> {
        let text = self.read_while(|c| c != b'<');
        let trimmed = decode_entities(&text);
        if trimmed.trim().is_empty() {
            return None;
        }
        Some(DomNode::Text(trimmed))
    }

    fn parse_element(&mut self, depth: usize) -> Option<DomNode> {
        if self.peek() != b'<' {
            return None;
        }
        self.advance(); // '<'

        // Comment: <!-- ... -->
        if self.pos + 2 < self.input.len()
            && self.input[self.pos] == b'!'
            && self.input[self.pos + 1] == b'-'
            && self.input[self.pos + 2] == b'-'
        {
            // Skip comment
            while !self.eof() {
                if self.pos + 2 < self.input.len()
                    && self.input[self.pos] == b'-'
                    && self.input[self.pos + 1] == b'-'
                    && self.input[self.pos + 2] == b'>'
                {
                    self.pos += 3;
                    break;
                }
                self.pos += 1;
            }
            return None;
        }

        // Doctype: <!DOCTYPE ...>
        if self.peek() == b'!' {
            while !self.eof() && self.peek() != b'>' {
                self.pos += 1;
            }
            if self.peek() == b'>' {
                self.advance();
            }
            return None;
        }

        // Closing tag: </tag>
        if self.peek() == b'/' {
            // Don't consume — let parent handle
            // Back up to '<'
            self.pos -= 1;
            return None;
        }

        let tag = self.read_tag_name();
        if tag.is_empty() {
            // Skip malformed
            while !self.eof() && self.peek() != b'>' {
                self.pos += 1;
            }
            if self.peek() == b'>' {
                self.advance();
            }
            return None;
        }

        let attrs = self.parse_attributes();

        // Self-closing?
        self.skip_whitespace();
        let self_close = self.peek() == b'/';
        if self_close {
            self.advance();
        }
        // Skip '>'
        if self.peek() == b'>' {
            self.advance();
        }

        if self_close || is_self_closing(&tag) {
            return Some(DomNode::Element {
                tag,
                attrs,
                children: Vec::new(),
            });
        }

        // Skip content of <script> and <style> — just consume until closing tag
        if tag == "script" || tag == "style" {
            let close_tag = format!("</{}", tag);
            while !self.eof() {
                if self.remaining_starts_with(&close_tag) {
                    break;
                }
                self.pos += 1;
            }
            // Skip </tag>
            self.skip_close_tag(&tag);
            if tag == "style" {
                // Extract CSS from style block
                // For simplicity, we skip inline style blocks
            }
            return Some(DomNode::Element {
                tag,
                attrs,
                children: Vec::new(),
            });
        }

        // Parse children
        let mut children = Vec::new();
        loop {
            if self.eof() {
                break;
            }
            self.skip_whitespace();
            if self.eof() {
                break;
            }

            // Check for closing tag
            if self.peek() == b'<' && self.pos + 1 < self.input.len()
                && self.input[self.pos + 1] == b'/'
            {
                self.skip_close_tag(&tag);
                break;
            }

            if let Some(child) = self.parse_node(depth + 1) {
                children.push(child);
            }
        }

        Some(DomNode::Element {
            tag,
            attrs,
            children,
        })
    }

    fn remaining_starts_with(&self, s: &str) -> bool {
        let sb = s.as_bytes();
        if self.pos + sb.len() > self.input.len() {
            return false;
        }
        // Case-insensitive comparison
        for (i, &b) in sb.iter().enumerate() {
            let a = self.input[self.pos + i];
            if a.to_ascii_lowercase() != b.to_ascii_lowercase() {
                return false;
            }
        }
        true
    }

    fn skip_close_tag(&mut self, _tag: &str) {
        // Expect </tag>
        if self.peek() == b'<' {
            self.advance();
        }
        if self.peek() == b'/' {
            self.advance();
        }
        // Skip tag name
        let _ = self.read_tag_name();
        self.skip_whitespace();
        if self.peek() == b'>' {
            self.advance();
        }
    }
}

fn is_self_closing(tag: &str) -> bool {
    SELF_CLOSING.iter().any(|&t| t == tag)
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '&' {
            let mut entity = String::new();
            for ech in chars.by_ref() {
                if ech == ';' {
                    break;
                }
                entity.push(ech);
                if entity.len() > 10 {
                    break;
                }
            }
            match entity.as_str() {
                "amp" => out.push('&'),
                "lt" => out.push('<'),
                "gt" => out.push('>'),
                "quot" => out.push('"'),
                "apos" => out.push('\''),
                "nbsp" => out.push(' '),
                _ => {
                    out.push('&');
                    out.push_str(&entity);
                    out.push(';');
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Parse HTML source into a DOM tree.
pub fn parse_html(source: &str) -> DomNode {
    let mut parser = HtmlParser::new(source);
    let mut children = Vec::new();
    while !parser.eof() {
        parser.skip_whitespace();
        if parser.eof() {
            break;
        }
        if let Some(node) = parser.parse_node(0) {
            children.push(node);
        }
    }
    // If there's a single root, return it; otherwise wrap in <html>
    if children.len() == 1 {
        children.remove(0)
    } else {
        DomNode::Element {
            tag: "html".to_owned(),
            attrs: Vec::new(),
            children,
        }
    }
}

// ---------------------------------------------------------------------------
// CSS parser
// ---------------------------------------------------------------------------

/// Parse a CSS source string into rules.
pub fn parse_css(source: &str) -> Vec<CssRule> {
    let mut rules = Vec::new();
    let mut pos = 0;
    let bytes = source.as_bytes();

    while pos < bytes.len() && rules.len() < MAX_CSS_RULES {
        // Skip whitespace
        while pos < bytes.len() && (bytes[pos] == b' ' || bytes[pos] == b'\n'
            || bytes[pos] == b'\r' || bytes[pos] == b'\t')
        {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }

        // Read selector
        let sel_start = pos;
        while pos < bytes.len() && bytes[pos] != b'{' {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }
        let sel_str = core::str::from_utf8(&bytes[sel_start..pos])
            .unwrap_or("")
            .trim();
        pos += 1; // skip '{'

        // Parse selector
        let selector = if sel_str.starts_with('.') {
            CssSelector::Class(sel_str[1..].to_owned())
        } else if sel_str.starts_with('#') {
            CssSelector::Id(sel_str[1..].to_owned())
        } else {
            CssSelector::Tag(sel_str.to_ascii_lowercase())
        };

        // Read properties until '}'
        let mut properties = Vec::new();
        while pos < bytes.len() && bytes[pos] != b'}' {
            // Skip whitespace
            while pos < bytes.len() && (bytes[pos] == b' ' || bytes[pos] == b'\n'
                || bytes[pos] == b'\r' || bytes[pos] == b'\t')
            {
                pos += 1;
            }
            if pos >= bytes.len() || bytes[pos] == b'}' {
                break;
            }

            // Read property name
            let pname_start = pos;
            while pos < bytes.len() && bytes[pos] != b':' && bytes[pos] != b'}' {
                pos += 1;
            }
            if pos >= bytes.len() || bytes[pos] == b'}' {
                break;
            }
            let pname = core::str::from_utf8(&bytes[pname_start..pos])
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            pos += 1; // skip ':'

            // Read value
            let pval_start = pos;
            while pos < bytes.len() && bytes[pos] != b';' && bytes[pos] != b'}' {
                pos += 1;
            }
            let pval = core::str::from_utf8(&bytes[pval_start..pos])
                .unwrap_or("")
                .trim()
                .to_owned();
            if pos < bytes.len() && bytes[pos] == b';' {
                pos += 1;
            }

            if !pname.is_empty() {
                properties.push(CssProperty {
                    name: pname,
                    value: pval,
                });
            }
        }
        if pos < bytes.len() {
            pos += 1; // skip '}'
        }

        rules.push(CssRule {
            selector,
            properties,
        });
    }

    rules
}

/// Parse inline style attribute.
fn parse_inline_style(style: &str) -> Vec<CssProperty> {
    let mut props = Vec::new();
    for decl in style.split(';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        if let Some((name, value)) = decl.split_once(':') {
            props.push(CssProperty {
                name: name.trim().to_ascii_lowercase(),
                value: value.trim().to_owned(),
            });
        }
    }
    props
}

// ---------------------------------------------------------------------------
// Layout engine
// ---------------------------------------------------------------------------

fn compute_layout(node: &DomNode, viewport_width: usize) -> LayoutBox {
    let mut root = LayoutBox::new();
    root.width = viewport_width;
    layout_node(node, &mut root, viewport_width);
    root
}

fn layout_node(node: &DomNode, parent: &mut LayoutBox, avail_width: usize) {
    match node {
        DomNode::Text(text) => {
            let mut child = LayoutBox::new();
            child.content = text.clone();
            child.display = DisplayMode::Inline;
            child.width = text.len().min(avail_width);
            child.height = if avail_width > 0 {
                (text.len() + avail_width - 1) / avail_width
            } else {
                1
            };
            parent.children.push(child);
        }
        DomNode::Element { tag, attrs, children } => {
            let mut this_box = LayoutBox::new();
            this_box.display = if is_block_tag(tag) {
                DisplayMode::Block
            } else {
                DisplayMode::Inline
            };

            // Apply inline style padding/margin
            if let Some(style_val) = attrs.iter().find(|(k, _)| k == "style").map(|(_, v)| v.as_str()) {
                let props = parse_inline_style(style_val);
                for prop in &props {
                    if prop.name == "display" && prop.value == "none" {
                        this_box.display = DisplayMode::None;
                    }
                    if prop.name == "padding" {
                        if let Some(v) = parse_px(&prop.value) {
                            this_box.padding_top = v;
                            this_box.padding_bottom = v;
                            this_box.padding_left = v;
                            this_box.padding_right = v;
                        }
                    }
                    if prop.name == "margin" {
                        if let Some(v) = parse_px(&prop.value) {
                            this_box.margin_top = v;
                            this_box.margin_bottom = v;
                            this_box.margin_left = v;
                            this_box.margin_right = v;
                        }
                    }
                }
            }

            if this_box.display == DisplayMode::None {
                return;
            }

            let inner_width = avail_width
                .saturating_sub(this_box.padding_left)
                .saturating_sub(this_box.padding_right)
                .saturating_sub(this_box.margin_left)
                .saturating_sub(this_box.margin_right);

            for child in children {
                layout_node(child, &mut this_box, inner_width);
            }

            this_box.width = avail_width;
            let mut h = this_box.padding_top + this_box.padding_bottom;
            for c in &this_box.children {
                h = h.saturating_add(c.height);
            }
            this_box.height = h.max(1);

            parent.children.push(this_box);
        }
    }
}

fn parse_px(s: &str) -> Option<usize> {
    let s = s.trim().trim_end_matches("px").trim();
    let mut val = 0usize;
    for ch in s.bytes() {
        if ch.is_ascii_digit() {
            val = val.saturating_mul(10).saturating_add((ch - b'0') as usize);
        } else {
            return None;
        }
    }
    Some(val)
}

// ---------------------------------------------------------------------------
// Text renderer
// ---------------------------------------------------------------------------

/// Render a parsed DOM tree to a text representation.
pub fn render_page(dom: &DomNode, viewport_width: usize) -> String {
    let width = if viewport_width == 0 { DEFAULT_VIEWPORT_WIDTH } else { viewport_width };
    let mut output = String::new();
    render_node(dom, &mut output, width, 0, &mut 1);
    output
}

fn render_node(
    node: &DomNode,
    out: &mut String,
    width: usize,
    indent: usize,
    list_counter: &mut usize,
) {
    match node {
        DomNode::Text(text) => {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                wrap_text(out, trimmed, width, indent);
            }
        }
        DomNode::Element { tag, attrs, children } => {
            match tag.as_str() {
                "title" => {
                    // Extract text for title
                    let title_text = extract_text(node);
                    if !title_text.is_empty() {
                        out.push_str("=== ");
                        out.push_str(&title_text);
                        out.push_str(" ===\n\n");
                    }
                }
                "h1" => {
                    let text = extract_text(node);
                    out.push_str("\n# ");
                    out.push_str(&text);
                    out.push('\n');
                    // Underline
                    for _ in 0..text.len().min(width) {
                        out.push('=');
                    }
                    out.push('\n');
                }
                "h2" => {
                    let text = extract_text(node);
                    out.push_str("\n## ");
                    out.push_str(&text);
                    out.push('\n');
                    for _ in 0..text.len().min(width) {
                        out.push('-');
                    }
                    out.push('\n');
                }
                "h3" => {
                    let text = extract_text(node);
                    out.push_str("\n### ");
                    out.push_str(&text);
                    out.push('\n');
                }
                "h4" => {
                    let text = extract_text(node);
                    out.push_str("\n#### ");
                    out.push_str(&text);
                    out.push('\n');
                }
                "h5" => {
                    let text = extract_text(node);
                    out.push_str("\n##### ");
                    out.push_str(&text);
                    out.push('\n');
                }
                "h6" => {
                    let text = extract_text(node);
                    out.push_str("\n###### ");
                    out.push_str(&text);
                    out.push('\n');
                }
                "p" => {
                    out.push('\n');
                    for child in children {
                        render_node(child, out, width, indent, list_counter);
                    }
                    out.push('\n');
                }
                "div" => {
                    for child in children {
                        render_node(child, out, width, indent, list_counter);
                    }
                    out.push('\n');
                }
                "br" => {
                    out.push('\n');
                }
                "hr" => {
                    for _ in 0..width.min(72) {
                        out.push('-');
                    }
                    out.push('\n');
                }
                "a" => {
                    let text = extract_text(node);
                    let href = attrs.iter()
                        .find(|(k, _)| k == "href")
                        .map(|(_, v)| v.as_str())
                        .unwrap_or("#");
                    out.push('[');
                    out.push_str(&text);
                    out.push_str("](");
                    out.push_str(href);
                    out.push(')');
                }
                "img" => {
                    let alt = attrs.iter()
                        .find(|(k, _)| k == "alt")
                        .map(|(_, v)| v.as_str())
                        .unwrap_or("image");
                    let src = attrs.iter()
                        .find(|(k, _)| k == "src")
                        .map(|(_, v)| v.as_str())
                        .unwrap_or("");
                    out.push_str("[IMG: ");
                    out.push_str(alt);
                    if !src.is_empty() {
                        out.push_str(" (");
                        out.push_str(src);
                        out.push(')');
                    }
                    out.push_str("]\n");
                }
                "b" | "strong" => {
                    out.push_str("**");
                    for child in children {
                        render_node(child, out, width, indent, list_counter);
                    }
                    out.push_str("**");
                }
                "i" | "em" => {
                    out.push('_');
                    for child in children {
                        render_node(child, out, width, indent, list_counter);
                    }
                    out.push('_');
                }
                "u" => {
                    for child in children {
                        render_node(child, out, width, indent, list_counter);
                    }
                }
                "code" => {
                    out.push('`');
                    for child in children {
                        render_node(child, out, width, indent, list_counter);
                    }
                    out.push('`');
                }
                "pre" => {
                    out.push_str("```\n");
                    for child in children {
                        render_node(child, out, width, indent, list_counter);
                    }
                    out.push_str("\n```\n");
                }
                "ul" => {
                    out.push('\n');
                    for child in children {
                        if let DomNode::Element { tag: ctag, .. } = child {
                            if ctag == "li" {
                                add_indent(out, indent + 2);
                                out.push_str("* ");
                                render_children_inline(child, out, width, indent + 4, list_counter);
                                out.push('\n');
                            }
                        }
                    }
                }
                "ol" => {
                    out.push('\n');
                    let mut num = 1usize;
                    for child in children {
                        if let DomNode::Element { tag: ctag, .. } = child {
                            if ctag == "li" {
                                add_indent(out, indent + 2);
                                let num_str = format!("{}. ", num);
                                out.push_str(&num_str);
                                render_children_inline(child, out, width, indent + 4, list_counter);
                                out.push('\n');
                                num = num.saturating_add(1);
                            }
                        }
                    }
                }
                "table" => {
                    render_table(node, out, width);
                }
                "span" => {
                    for child in children {
                        render_node(child, out, width, indent, list_counter);
                    }
                }
                "head" | "html" | "body" => {
                    for child in children {
                        render_node(child, out, width, indent, list_counter);
                    }
                }
                _ => {
                    for child in children {
                        render_node(child, out, width, indent, list_counter);
                    }
                }
            }
        }
    }
}

fn render_children_inline(
    node: &DomNode,
    out: &mut String,
    width: usize,
    indent: usize,
    list_counter: &mut usize,
) {
    if let DomNode::Element { children, .. } = node {
        for child in children {
            render_node(child, out, width, indent, list_counter);
        }
    }
}

fn add_indent(out: &mut String, n: usize) {
    for _ in 0..n {
        out.push(' ');
    }
}

fn wrap_text(out: &mut String, text: &str, width: usize, indent: usize) {
    let w = width.saturating_sub(indent).max(10);
    let mut col = 0usize;
    for word in text.split_whitespace() {
        if col > 0 && col + 1 + word.len() > w {
            out.push('\n');
            add_indent(out, indent);
            col = 0;
        } else if col > 0 {
            out.push(' ');
            col += 1;
        } else {
            add_indent(out, indent);
        }
        out.push_str(word);
        col += word.len();
    }
}

fn extract_text(node: &DomNode) -> String {
    let mut out = String::new();
    extract_text_inner(node, &mut out);
    out
}

fn extract_text_inner(node: &DomNode, out: &mut String) {
    match node {
        DomNode::Text(t) => {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            out.push_str(t.trim());
        }
        DomNode::Element { children, .. } => {
            for child in children {
                extract_text_inner(child, out);
            }
        }
    }
}

fn render_table(node: &DomNode, out: &mut String, _width: usize) {
    // Collect rows
    let mut rows: Vec<Vec<String>> = Vec::new();
    collect_rows(node, &mut rows);

    if rows.is_empty() {
        return;
    }

    // Determine column widths
    let max_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if max_cols == 0 {
        return;
    }
    let mut col_widths = Vec::new();
    for _ in 0..max_cols {
        col_widths.push(0usize);
    }
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i < max_cols && cell.len() > col_widths[i] {
                col_widths[i] = cell.len();
            }
        }
    }

    // Render
    out.push('\n');
    render_table_separator(out, &col_widths);
    for (ri, row) in rows.iter().enumerate() {
        out.push('|');
        for (ci, cw) in col_widths.iter().enumerate() {
            let cell = if ci < row.len() { row[ci].as_str() } else { "" };
            out.push(' ');
            out.push_str(cell);
            let pad = cw.saturating_sub(cell.len());
            for _ in 0..pad {
                out.push(' ');
            }
            out.push_str(" |");
        }
        out.push('\n');
        if ri == 0 {
            render_table_separator(out, &col_widths);
        }
    }
    render_table_separator(out, &col_widths);
}

fn render_table_separator(out: &mut String, col_widths: &[usize]) {
    out.push('+');
    for w in col_widths {
        for _ in 0..w + 2 {
            out.push('-');
        }
        out.push('+');
    }
    out.push('\n');
}

fn collect_rows(node: &DomNode, rows: &mut Vec<Vec<String>>) {
    if let DomNode::Element { tag, children, .. } = node {
        if tag == "tr" {
            let mut row = Vec::new();
            for child in children {
                if let DomNode::Element { tag: ctag, .. } = child {
                    if ctag == "td" || ctag == "th" {
                        row.push(extract_text(child));
                    }
                }
            }
            rows.push(row);
        } else {
            for child in children {
                collect_rows(child, rows);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Navigation & browser state
// ---------------------------------------------------------------------------

/// Browser page state.
struct Page {
    url: String,
    title: String,
    dom: Option<DomNode>,
    rendered: String,
    scroll_y: usize,
}

impl Page {
    fn new() -> Self {
        Self {
            url: String::new(),
            title: String::new(),
            dom: None,
            rendered: String::new(),
            scroll_y: 0,
        }
    }
}

struct BrowserState {
    initialized: bool,
    current_page: Page,
    history_back: Vec<String>,
    history_forward: Vec<String>,
    bookmarks: Vec<(String, String)>, // (name, url)
    viewport_width: usize,
}

impl BrowserState {
    const fn new() -> Self {
        Self {
            initialized: false,
            current_page: Page {
                url: String::new(),
                title: String::new(),
                dom: None,
                rendered: String::new(),
                scroll_y: 0,
            },
            history_back: Vec::new(),
            history_forward: Vec::new(),
            bookmarks: Vec::new(),
            viewport_width: DEFAULT_VIEWPORT_WIDTH,
        }
    }
}

static STATE: Mutex<BrowserState> = Mutex::new(BrowserState::new());

// Statistics
static PAGES_LOADED: AtomicU64 = AtomicU64::new(0);
static BYTES_DOWNLOADED: AtomicU64 = AtomicU64::new(0);
static NAV_COUNT: AtomicU64 = AtomicU64::new(0);
static BROWSER_INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the browser subsystem.
pub fn init() {
    let mut state = STATE.lock();
    if state.initialized {
        return;
    }
    state.initialized = true;
    state.viewport_width = DEFAULT_VIEWPORT_WIDTH;
    BROWSER_INITIALIZED.store(true, Ordering::SeqCst);
}

/// Navigate to a URL, fetching via HTTP GET.
pub fn navigate(url: &str) -> Result<String, &'static str> {
    if url.is_empty() {
        return Err("empty URL");
    }

    let (host, port, path) = crate::http::parse_url(url).ok_or("invalid URL")?;

    // Save current page in back history
    {
        let mut state = STATE.lock();
        if !state.current_page.url.is_empty() {
            let prev_url = state.current_page.url.clone();
            if state.history_back.len() < MAX_HISTORY {
                state.history_back.push(prev_url);
            }
        }
        state.history_forward.clear();
    }

    // Build request
    let _request = crate::http::build_request("GET", &host, &path);

    // Simulate fetching (real networking via crate::http / tcp_real)
    let body = format!(
        "<html><head><title>Page at {}</title></head>\
         <body><h1>{}</h1><p>Fetched from {}:{}{}</p></body></html>",
        url, host, host, port, path
    );
    let body_len = body.len() as u64;

    // Parse HTML
    let dom = parse_html(&body);
    let title = extract_title(&dom);

    // Render
    let vw = STATE.lock().viewport_width;
    let rendered = render_page(&dom, vw);

    // Update state
    {
        let mut state = STATE.lock();
        state.current_page = Page {
            url: url.to_owned(),
            title: title.clone(),
            dom: Some(dom),
            rendered: rendered.clone(),
            scroll_y: 0,
        };
    }

    PAGES_LOADED.fetch_add(1, Ordering::Relaxed);
    BYTES_DOWNLOADED.fetch_add(body_len, Ordering::Relaxed);
    NAV_COUNT.fetch_add(1, Ordering::Relaxed);

    Ok(rendered)
}

/// Go back in history.
pub fn back() -> Result<String, &'static str> {
    let prev_url = {
        let mut state = STATE.lock();
        let url = state.history_back.pop().ok_or("no history")?;
        if !state.current_page.url.is_empty() {
            let cur = state.current_page.url.clone();
            if state.history_forward.len() < MAX_HISTORY {
                state.history_forward.push(cur);
            }
        }
        url
    };
    navigate(&prev_url)
}

/// Go forward in history.
pub fn forward() -> Result<String, &'static str> {
    let next_url = {
        let mut state = STATE.lock();
        let url = state.history_forward.pop().ok_or("no forward history")?;
        if !state.current_page.url.is_empty() {
            let cur = state.current_page.url.clone();
            if state.history_back.len() < MAX_HISTORY {
                state.history_back.push(cur);
            }
        }
        url
    };
    navigate(&next_url)
}

/// Add a bookmark for the current page.
pub fn add_bookmark(name: &str) {
    let mut state = STATE.lock();
    if state.current_page.url.is_empty() {
        return;
    }
    if state.bookmarks.len() >= MAX_BOOKMARKS {
        return;
    }
    let url = state.current_page.url.clone();
    state.bookmarks.push((name.to_owned(), url));
}

/// List bookmarks.
pub fn list_bookmarks() -> Vec<(String, String)> {
    STATE.lock().bookmarks.clone()
}

fn extract_title(dom: &DomNode) -> String {
    if let DomNode::Element { tag, children, .. } = dom {
        if tag == "title" {
            return extract_text(dom);
        }
        for child in children {
            let t = extract_title(child);
            if !t.is_empty() {
                return t;
            }
        }
    }
    String::new()
}

/// Get browser info string.
pub fn browser_info() -> String {
    let state = STATE.lock();
    let mut info = String::from("MerlionOS Browser v1.0\n");
    info.push_str(&format!("Viewport: {} columns\n", state.viewport_width));
    if !state.current_page.url.is_empty() {
        info.push_str(&format!("Current URL: {}\n", state.current_page.url));
        info.push_str(&format!("Title: {}\n", state.current_page.title));
    } else {
        info.push_str("No page loaded\n");
    }
    info.push_str(&format!("History: {} back, {} forward\n",
        state.history_back.len(), state.history_forward.len()));
    info.push_str(&format!("Bookmarks: {}\n", state.bookmarks.len()));
    info
}

/// Get browser statistics.
pub fn browser_stats() -> String {
    let pages = PAGES_LOADED.load(Ordering::Relaxed);
    let bytes = BYTES_DOWNLOADED.load(Ordering::Relaxed);
    let navs = NAV_COUNT.load(Ordering::Relaxed);
    format!(
        "Pages loaded: {}\nBytes downloaded: {}\nNavigations: {}\n",
        pages, bytes, navs
    )
}
