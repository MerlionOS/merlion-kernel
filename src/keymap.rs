/// Keyboard layout manager for MerlionOS.
/// Supports multiple keyboard layouts with runtime switching.
/// Layouts: US QWERTY, UK, German (QWERTZ), French (AZERTY),
/// Dvorak, and Chinese Pinyin input method.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Layout definitions
// ---------------------------------------------------------------------------

/// A keyboard layout mapping scancodes to characters.
pub struct KeyLayout {
    pub name: &'static str,
    pub normal: [char; 128],
    pub shifted: [char; 128],
    pub altgr: [char; 128],
}

/// Modifier key state tracked globally.
#[derive(Debug, Clone, Copy)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub altgr: bool,
    pub super_key: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
}

impl Modifiers {
    const fn new() -> Self {
        Self {
            shift: false,
            ctrl: false,
            alt: false,
            altgr: false,
            super_key: false,
            caps_lock: false,
            num_lock: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Dead key support
// ---------------------------------------------------------------------------

/// Dead key accents for compose sequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeadKey {
    None,
    Acute,      // ´
    Grave,      // `
    Circumflex, // ^
    Diaeresis,  // ¨
    Tilde,      // ~
}

/// Resolve a dead key + base character into an accented character.
fn resolve_dead_key(dead: DeadKey, base: char) -> Option<char> {
    match (dead, base) {
        (DeadKey::Acute, 'a') => Some('\u{00E1}'),  // á
        (DeadKey::Acute, 'e') => Some('\u{00E9}'),  // é
        (DeadKey::Acute, 'i') => Some('\u{00ED}'),  // í
        (DeadKey::Acute, 'o') => Some('\u{00F3}'),  // ó
        (DeadKey::Acute, 'u') => Some('\u{00FA}'),  // ú
        (DeadKey::Acute, 'A') => Some('\u{00C1}'),  // Á
        (DeadKey::Acute, 'E') => Some('\u{00C9}'),  // É
        (DeadKey::Acute, 'I') => Some('\u{00CD}'),  // Í
        (DeadKey::Acute, 'O') => Some('\u{00D3}'),  // Ó
        (DeadKey::Acute, 'U') => Some('\u{00DA}'),  // Ú
        (DeadKey::Grave, 'a') => Some('\u{00E0}'),  // à
        (DeadKey::Grave, 'e') => Some('\u{00E8}'),  // è
        (DeadKey::Grave, 'i') => Some('\u{00EC}'),  // ì
        (DeadKey::Grave, 'o') => Some('\u{00F2}'),  // ò
        (DeadKey::Grave, 'u') => Some('\u{00F9}'),  // ù
        (DeadKey::Circumflex, 'a') => Some('\u{00E2}'), // â
        (DeadKey::Circumflex, 'e') => Some('\u{00EA}'), // ê
        (DeadKey::Circumflex, 'i') => Some('\u{00EE}'), // î
        (DeadKey::Circumflex, 'o') => Some('\u{00F4}'), // ô
        (DeadKey::Circumflex, 'u') => Some('\u{00FB}'), // û
        (DeadKey::Diaeresis, 'a') => Some('\u{00E4}'),  // ä
        (DeadKey::Diaeresis, 'e') => Some('\u{00EB}'),  // ë
        (DeadKey::Diaeresis, 'i') => Some('\u{00EF}'),  // ï
        (DeadKey::Diaeresis, 'o') => Some('\u{00F6}'),  // ö
        (DeadKey::Diaeresis, 'u') => Some('\u{00FC}'),  // ü
        (DeadKey::Diaeresis, 'A') => Some('\u{00C4}'),  // Ä
        (DeadKey::Diaeresis, 'O') => Some('\u{00D6}'),  // Ö
        (DeadKey::Diaeresis, 'U') => Some('\u{00DC}'),  // Ü
        (DeadKey::Tilde, 'n') => Some('\u{00F1}'),      // ñ
        (DeadKey::Tilde, 'N') => Some('\u{00D1}'),      // Ñ
        (DeadKey::Tilde, 'a') => Some('\u{00E3}'),      // ã
        (DeadKey::Tilde, 'o') => Some('\u{00F5}'),      // õ
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Pinyin input method (simplified lookup table)
// ---------------------------------------------------------------------------

/// Common pinyin → Chinese character mappings.
const PINYIN_TABLE: &[(&str, &str)] = &[
    ("nihao", "你好"),
    ("ni", "你"),
    ("hao", "好"),
    ("wo", "我"),
    ("shi", "是"),
    ("de", "的"),
    ("le", "了"),
    ("ma", "吗"),
    ("bu", "不"),
    ("zai", "在"),
    ("you", "有"),
    ("ren", "人"),
    ("ta", "他"),
    ("she", "她"),
    ("men", "们"),
    ("zhe", "这"),
    ("na", "那"),
    ("ge", "个"),
    ("da", "大"),
    ("xiao", "小"),
    ("zhong", "中"),
    ("guo", "国"),
    ("zhongguo", "中国"),
    ("xin", "新"),
    ("jia", "加"),
    ("xinjiapo", "新加坡"),
    ("po", "坡"),
    ("ai", "爱"),
    ("he", "和"),
    ("dui", "对"),
    ("shang", "上"),
    ("xia", "下"),
    ("lai", "来"),
    ("qu", "去"),
    ("kan", "看"),
    ("shuo", "说"),
    ("ting", "听"),
    ("xie", "写"),
    ("du", "读"),
    ("chi", "吃"),
    ("he2", "喝"),
    ("shui", "水"),
    ("huo", "火"),
    ("tian", "天"),
    ("di", "地"),
    ("ri", "日"),
    ("yue", "月"),
    ("nian", "年"),
    ("hao3", "号"),
    ("ming", "名"),
    ("zi", "字"),
];

/// Look up a pinyin string and return matching candidates.
fn pinyin_lookup(input: &str) -> Vec<&'static str> {
    let mut results = Vec::new();
    let lower = input.to_ascii_lowercase();
    for &(pinyin, hanzi) in PINYIN_TABLE {
        if pinyin == lower.as_str() {
            results.push(hanzi);
        }
    }
    // Also add partial-prefix matches as secondary candidates
    if results.is_empty() {
        for &(pinyin, hanzi) in PINYIN_TABLE {
            if pinyin.starts_with(lower.as_str()) {
                results.push(hanzi);
                if results.len() >= 5 {
                    break;
                }
            }
        }
    }
    results
}

// Helper extension trait for ASCII lowercase on &str
trait AsciiLowercase {
    fn to_ascii_lowercase(&self) -> String;
}

impl AsciiLowercase for str {
    fn to_ascii_lowercase(&self) -> String {
        let mut s = String::new();
        for c in self.chars() {
            if c.is_ascii_uppercase() {
                s.push((c as u8 + 32) as char);
            } else {
                s.push(c);
            }
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Layout data: US QWERTY
// ---------------------------------------------------------------------------

const fn make_empty_map() -> [char; 128] {
    ['\0'; 128]
}

/// Build the US QWERTY normal map (scancode set 1).
const fn us_normal() -> [char; 128] {
    let mut m = make_empty_map();
    m[0x02] = '1'; m[0x03] = '2'; m[0x04] = '3'; m[0x05] = '4';
    m[0x06] = '5'; m[0x07] = '6'; m[0x08] = '7'; m[0x09] = '8';
    m[0x0A] = '9'; m[0x0B] = '0'; m[0x0C] = '-'; m[0x0D] = '=';
    m[0x0E] = '\x08'; m[0x0F] = '\t';
    m[0x10] = 'q'; m[0x11] = 'w'; m[0x12] = 'e'; m[0x13] = 'r';
    m[0x14] = 't'; m[0x15] = 'y'; m[0x16] = 'u'; m[0x17] = 'i';
    m[0x18] = 'o'; m[0x19] = 'p'; m[0x1A] = '['; m[0x1B] = ']';
    m[0x1C] = '\n';
    m[0x1E] = 'a'; m[0x1F] = 's'; m[0x20] = 'd'; m[0x21] = 'f';
    m[0x22] = 'g'; m[0x23] = 'h'; m[0x24] = 'j'; m[0x25] = 'k';
    m[0x26] = 'l'; m[0x27] = ';'; m[0x28] = '\''; m[0x29] = '`';
    m[0x2B] = '\\';
    m[0x2C] = 'z'; m[0x2D] = 'x'; m[0x2E] = 'c'; m[0x2F] = 'v';
    m[0x30] = 'b'; m[0x31] = 'n'; m[0x32] = 'm'; m[0x33] = ',';
    m[0x34] = '.'; m[0x35] = '/'; m[0x39] = ' ';
    m
}

const fn us_shifted() -> [char; 128] {
    let mut m = make_empty_map();
    m[0x02] = '!'; m[0x03] = '@'; m[0x04] = '#'; m[0x05] = '$';
    m[0x06] = '%'; m[0x07] = '^'; m[0x08] = '&'; m[0x09] = '*';
    m[0x0A] = '('; m[0x0B] = ')'; m[0x0C] = '_'; m[0x0D] = '+';
    m[0x0E] = '\x08'; m[0x0F] = '\t';
    m[0x10] = 'Q'; m[0x11] = 'W'; m[0x12] = 'E'; m[0x13] = 'R';
    m[0x14] = 'T'; m[0x15] = 'Y'; m[0x16] = 'U'; m[0x17] = 'I';
    m[0x18] = 'O'; m[0x19] = 'P'; m[0x1A] = '{'; m[0x1B] = '}';
    m[0x1C] = '\n';
    m[0x1E] = 'A'; m[0x1F] = 'S'; m[0x20] = 'D'; m[0x21] = 'F';
    m[0x22] = 'G'; m[0x23] = 'H'; m[0x24] = 'J'; m[0x25] = 'K';
    m[0x26] = 'L'; m[0x27] = ':'; m[0x28] = '"'; m[0x29] = '~';
    m[0x2B] = '|';
    m[0x2C] = 'Z'; m[0x2D] = 'X'; m[0x2E] = 'C'; m[0x2F] = 'V';
    m[0x30] = 'B'; m[0x31] = 'N'; m[0x32] = 'M'; m[0x33] = '<';
    m[0x34] = '>'; m[0x35] = '?'; m[0x39] = ' ';
    m
}

// ---------------------------------------------------------------------------
// Layout data: UK
// ---------------------------------------------------------------------------

const fn uk_normal() -> [char; 128] {
    let mut m = us_normal();
    m[0x29] = '`';  // same
    m[0x2B] = '#';  // UK: # instead of backslash
    m
}

const fn uk_shifted() -> [char; 128] {
    let mut m = us_shifted();
    m[0x03] = '"';  // Shift+2 = " (not @)
    m[0x04] = '\u{00A3}'; // Shift+3 = £
    m[0x28] = '@';  // Shift+' = @
    m[0x29] = '\u{00AC}'; // Shift+` = ¬
    m[0x2B] = '~';
    m
}

// ---------------------------------------------------------------------------
// Layout data: German QWERTZ
// ---------------------------------------------------------------------------

const fn de_normal() -> [char; 128] {
    let mut m = us_normal();
    // Y ↔ Z swap
    m[0x15] = 'z';  // scancode for Y position → z
    m[0x2C] = 'y';  // scancode for Z position → y
    m[0x0C] = '\u{00DF}'; // ß
    m[0x1A] = '\u{00FC}'; // ü
    m[0x27] = '\u{00F6}'; // ö
    m[0x28] = '\u{00E4}'; // ä
    m
}

const fn de_shifted() -> [char; 128] {
    let mut m = us_shifted();
    m[0x15] = 'Z';
    m[0x2C] = 'Y';
    m[0x1A] = '\u{00DC}'; // Ü
    m[0x27] = '\u{00D6}'; // Ö
    m[0x28] = '\u{00C4}'; // Ä
    m
}

const fn de_altgr() -> [char; 128] {
    let mut m = make_empty_map();
    m[0x03] = '\u{00B2}'; // ²
    m[0x04] = '\u{00B3}'; // ³
    m[0x12] = '\u{20AC}'; // € on AltGr+E
    m[0x10] = '@';         // @ on AltGr+Q
    m
}

// ---------------------------------------------------------------------------
// Layout data: French AZERTY
// ---------------------------------------------------------------------------

const fn fr_normal() -> [char; 128] {
    let mut m = us_normal();
    // A ↔ Q swap
    m[0x10] = 'a';  // Q position → a
    m[0x1E] = 'q';  // A position → q
    // Z ↔ W swap
    m[0x11] = 'z';  // W position → z
    m[0x2C] = 'w';  // Z position → w
    // Number row: unshifted = symbols in French
    m[0x02] = '&'; m[0x03] = '\u{00E9}'; m[0x04] = '"';
    m[0x05] = '\''; m[0x06] = '('; m[0x07] = '-';
    m[0x08] = '\u{00E8}'; m[0x09] = '_'; m[0x0A] = '\u{00E7}';
    m[0x0B] = '\u{00E0}';
    m[0x32] = ','; m[0x33] = ';'; m[0x34] = ':'; m[0x35] = '!';
    m
}

const fn fr_shifted() -> [char; 128] {
    let mut m = us_shifted();
    m[0x10] = 'A'; m[0x1E] = 'Q';
    m[0x11] = 'Z'; m[0x2C] = 'W';
    m[0x02] = '1'; m[0x03] = '2'; m[0x04] = '3'; m[0x05] = '4';
    m[0x06] = '5'; m[0x07] = '6'; m[0x08] = '7'; m[0x09] = '8';
    m[0x0A] = '9'; m[0x0B] = '0';
    m[0x32] = '?'; m[0x33] = '.'; m[0x34] = '/'; m[0x35] = '\u{00A7}';
    m
}

// ---------------------------------------------------------------------------
// Layout data: Dvorak
// ---------------------------------------------------------------------------

const fn dvorak_normal() -> [char; 128] {
    let mut m = make_empty_map();
    m[0x02] = '1'; m[0x03] = '2'; m[0x04] = '3'; m[0x05] = '4';
    m[0x06] = '5'; m[0x07] = '6'; m[0x08] = '7'; m[0x09] = '8';
    m[0x0A] = '9'; m[0x0B] = '0'; m[0x0C] = '['; m[0x0D] = ']';
    m[0x0E] = '\x08'; m[0x0F] = '\t';
    m[0x10] = '\''; m[0x11] = ','; m[0x12] = '.'; m[0x13] = 'p';
    m[0x14] = 'y'; m[0x15] = 'f'; m[0x16] = 'g'; m[0x17] = 'c';
    m[0x18] = 'r'; m[0x19] = 'l'; m[0x1A] = '/'; m[0x1B] = '=';
    m[0x1C] = '\n';
    m[0x1E] = 'a'; m[0x1F] = 'o'; m[0x20] = 'e'; m[0x21] = 'u';
    m[0x22] = 'i'; m[0x23] = 'd'; m[0x24] = 'h'; m[0x25] = 't';
    m[0x26] = 'n'; m[0x27] = 's'; m[0x28] = '-'; m[0x29] = '`';
    m[0x2B] = '\\';
    m[0x2C] = ';'; m[0x2D] = 'q'; m[0x2E] = 'j'; m[0x2F] = 'k';
    m[0x30] = 'x'; m[0x31] = 'b'; m[0x32] = 'm'; m[0x33] = 'w';
    m[0x34] = 'v'; m[0x35] = 'z'; m[0x39] = ' ';
    m
}

const fn dvorak_shifted() -> [char; 128] {
    let mut m = make_empty_map();
    m[0x02] = '!'; m[0x03] = '@'; m[0x04] = '#'; m[0x05] = '$';
    m[0x06] = '%'; m[0x07] = '^'; m[0x08] = '&'; m[0x09] = '*';
    m[0x0A] = '('; m[0x0B] = ')'; m[0x0C] = '{'; m[0x0D] = '}';
    m[0x0E] = '\x08'; m[0x0F] = '\t';
    m[0x10] = '"'; m[0x11] = '<'; m[0x12] = '>'; m[0x13] = 'P';
    m[0x14] = 'Y'; m[0x15] = 'F'; m[0x16] = 'G'; m[0x17] = 'C';
    m[0x18] = 'R'; m[0x19] = 'L'; m[0x1A] = '?'; m[0x1B] = '+';
    m[0x1C] = '\n';
    m[0x1E] = 'A'; m[0x1F] = 'O'; m[0x20] = 'E'; m[0x21] = 'U';
    m[0x22] = 'I'; m[0x23] = 'D'; m[0x24] = 'H'; m[0x25] = 'T';
    m[0x26] = 'N'; m[0x27] = 'S'; m[0x28] = '_'; m[0x29] = '~';
    m[0x2B] = '|';
    m[0x2C] = ':'; m[0x2D] = 'Q'; m[0x2E] = 'J'; m[0x2F] = 'K';
    m[0x30] = 'X'; m[0x31] = 'B'; m[0x32] = 'M'; m[0x33] = 'W';
    m[0x34] = 'V'; m[0x35] = 'Z'; m[0x39] = ' ';
    m
}

// ---------------------------------------------------------------------------
// Static layout table
// ---------------------------------------------------------------------------

static LAYOUTS: &[KeyLayout] = &[
    KeyLayout { name: "us",     normal: us_normal(),     shifted: us_shifted(),     altgr: make_empty_map() },
    KeyLayout { name: "uk",     normal: uk_normal(),     shifted: uk_shifted(),     altgr: make_empty_map() },
    KeyLayout { name: "de",     normal: de_normal(),     shifted: de_shifted(),     altgr: de_altgr() },
    KeyLayout { name: "fr",     normal: fr_normal(),     shifted: fr_shifted(),     altgr: make_empty_map() },
    KeyLayout { name: "dvorak", normal: dvorak_normal(), shifted: dvorak_shifted(), altgr: make_empty_map() },
];

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Index into LAYOUTS for the active layout.
static CURRENT_LAYOUT: AtomicU8 = AtomicU8::new(0);
static INITIALIZED: AtomicBool = AtomicBool::new(false);

static MODIFIERS: Mutex<Modifiers> = Mutex::new(Modifiers::new());
static DEAD_KEY: Mutex<DeadKey> = Mutex::new(DeadKey::None);

/// Pinyin input buffer for Chinese input method.
static PINYIN_BUF: Mutex<PinyinState> = Mutex::new(PinyinState::new());
static PINYIN_MODE: AtomicBool = AtomicBool::new(false);

struct PinyinState {
    buf: [u8; 32],
    len: usize,
}

impl PinyinState {
    const fn new() -> Self {
        Self { buf: [0u8; 32], len: 0 }
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn push(&mut self, c: u8) {
        if self.len < 31 {
            self.buf[self.len] = c;
            self.len += 1;
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

// ---------------------------------------------------------------------------
// Scancode constants for modifier keys
// ---------------------------------------------------------------------------

const SC_LSHIFT: u8   = 0x2A;
const SC_RSHIFT: u8   = 0x36;
const SC_LCTRL: u8    = 0x1D;
const SC_LALT: u8     = 0x38;
const SC_CAPS: u8     = 0x3A;
const SC_NUMLOCK: u8  = 0x45;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the keyboard layout manager with US QWERTY as default.
pub fn init() {
    CURRENT_LAYOUT.store(0, Ordering::SeqCst);
    INITIALIZED.store(true, Ordering::SeqCst);
    crate::serial_println!("[keymap] Keyboard layout manager initialized (US QWERTY)");
}

/// Translate a PS/2 scancode into a character using the current layout.
///
/// Updates modifier state internally. Returns `None` for modifier-only
/// keys, unrecognized scancodes, or incomplete dead key sequences.
pub fn translate_scancode(scancode: u8, _modifiers: &Modifiers) -> Option<char> {
    // Use internal modifier tracking
    let mut mods = MODIFIERS.lock();

    // Extended prefix (0xE0) handling: right-Alt = AltGr
    // The caller should filter 0xE0 prefix and set AltGr flag externally
    // if needed, or we detect right-alt via extended scancode.

    // Key release (break code)
    if scancode & 0x80 != 0 {
        let make = scancode & 0x7F;
        match make {
            SC_LSHIFT | SC_RSHIFT => mods.shift = false,
            SC_LCTRL => mods.ctrl = false,
            SC_LALT => mods.alt = false,
            _ => {}
        }
        return None;
    }

    // Key press (make code)
    match scancode {
        SC_LSHIFT | SC_RSHIFT => { mods.shift = true; return None; }
        SC_LCTRL => { mods.ctrl = true; return None; }
        SC_LALT => { mods.alt = true; return None; }
        SC_CAPS => { mods.caps_lock = !mods.caps_lock; return None; }
        SC_NUMLOCK => { mods.num_lock = !mods.num_lock; return None; }
        _ => {}
    }

    let idx = CURRENT_LAYOUT.load(Ordering::Relaxed) as usize;
    if idx >= LAYOUTS.len() {
        return None;
    }
    let layout = &LAYOUTS[idx];
    let sc = scancode as usize;
    if sc >= 128 {
        return None;
    }

    // Pick the right map
    let effective_shift = mods.shift ^ mods.caps_lock;
    let ch = if mods.altgr && layout.altgr[sc] != '\0' {
        layout.altgr[sc]
    } else if effective_shift {
        layout.shifted[sc]
    } else {
        layout.normal[sc]
    };

    if ch == '\0' {
        return None;
    }

    // Pinyin input mode
    if PINYIN_MODE.load(Ordering::Relaxed) {
        return handle_pinyin_char(ch);
    }

    // Dead key handling
    let mut dead = DEAD_KEY.lock();
    if *dead != DeadKey::None {
        let pending = *dead;
        *dead = DeadKey::None;
        if let Some(accented) = resolve_dead_key(pending, ch) {
            return Some(accented);
        }
        // If dead key doesn't compose, return the base character
        return Some(ch);
    }

    // Check if this character starts a dead key sequence
    // (for layouts that use dead keys, e.g. international variants)
    match ch {
        '\u{00B4}' => { *dead = DeadKey::Acute; return None; }     // ´
        '\u{0060}' => { *dead = DeadKey::Grave; return None; }     // ` — only in dead-key mode
        '\u{005E}' => { *dead = DeadKey::Circumflex; return None; } // ^
        '\u{00A8}' => { *dead = DeadKey::Diaeresis; return None; } // ¨
        _ => {}
    }

    Some(ch)
}

/// Handle a character in Pinyin input mode.
fn handle_pinyin_char(ch: char) -> Option<char> {
    if ch == ' ' || ch == '\n' {
        // Commit: look up pinyin buffer
        let mut pb = PINYIN_BUF.lock();
        let input = pb.as_str();
        if input.is_empty() {
            pb.clear();
            return Some(ch);
        }
        let candidates = pinyin_lookup(input);
        pb.clear();
        if let Some(hanzi) = candidates.first() {
            // Return first character of the matched string
            return hanzi.chars().next();
        }
        return Some(ch);
    }

    if ch.is_ascii_alphabetic() {
        let mut pb = PINYIN_BUF.lock();
        pb.push(ch as u8);
        return None; // Still composing
    }

    Some(ch)
}

/// Set the active layout by name. Returns true on success.
pub fn set_layout(name: &str) -> bool {
    for (i, layout) in LAYOUTS.iter().enumerate() {
        if layout.name == name {
            CURRENT_LAYOUT.store(i as u8, Ordering::SeqCst);
            crate::serial_println!("[keymap] Layout switched to: {}", name);
            return true;
        }
    }
    // Check for "pinyin" as a special pseudo-layout
    if name == "pinyin" {
        PINYIN_MODE.store(true, Ordering::SeqCst);
        crate::serial_println!("[keymap] Pinyin input method enabled");
        return true;
    }
    false
}

/// Cycle to the next layout.
pub fn next_layout() {
    let cur = CURRENT_LAYOUT.load(Ordering::Relaxed) as usize;
    let next = (cur + 1) % LAYOUTS.len();
    CURRENT_LAYOUT.store(next as u8, Ordering::SeqCst);
    PINYIN_MODE.store(false, Ordering::Relaxed);
    crate::serial_println!("[keymap] Layout switched to: {}", LAYOUTS[next].name);
}

/// Return a list of available layout names.
pub fn list_layouts() -> Vec<&'static str> {
    let mut v = Vec::new();
    for layout in LAYOUTS.iter() {
        v.push(layout.name);
    }
    v.push("pinyin");
    v
}

/// Return the name of the current layout.
pub fn current_layout() -> &'static str {
    if PINYIN_MODE.load(Ordering::Relaxed) {
        return "pinyin";
    }
    let idx = CURRENT_LAYOUT.load(Ordering::Relaxed) as usize;
    if idx < LAYOUTS.len() {
        LAYOUTS[idx].name
    } else {
        "unknown"
    }
}

/// Return the current modifier state.
pub fn get_modifiers() -> Modifiers {
    *MODIFIERS.lock()
}

/// Toggle Pinyin input mode on/off.
pub fn toggle_pinyin() {
    let cur = PINYIN_MODE.load(Ordering::Relaxed);
    PINYIN_MODE.store(!cur, Ordering::SeqCst);
    if !cur {
        PINYIN_BUF.lock().clear();
    }
}

/// Return human-readable keymap information.
pub fn keymap_info() -> String {
    let mods = MODIFIERS.lock();
    let pinyin = PINYIN_MODE.load(Ordering::Relaxed);
    format!(
        "Keyboard Layout Manager\n\
         Current layout: {}\n\
         Available:      {}\n\
         Pinyin mode:    {}\n\
         Caps Lock:      {}\n\
         Num Lock:       {}\n\
         Shift:          {}\n\
         Ctrl:           {}\n\
         Alt:            {}\n\
         AltGr:          {}\n\
         Super:          {}",
        current_layout(),
        list_layouts().join(", "),
        pinyin,
        mods.caps_lock,
        mods.num_lock,
        mods.shift,
        mods.ctrl,
        mods.alt,
        mods.altgr,
        mods.super_key,
    )
}
