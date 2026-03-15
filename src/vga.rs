/// VGA text mode (mode 3) console with scrolling and cursor.
/// 80 columns x 25 rows. Each cell is 2 bytes: [ascii, attribute].

use core::fmt;
use spin::Mutex;

const VGA_BUFFER: usize = 0xB8000;
const WIDTH: usize = 80;
const HEIGHT: usize = 25;

/// VGA color attributes.
#[allow(dead_code)]
#[repr(u8)]
pub enum Color {
    Black = 0x0,
    Blue = 0x1,
    Green = 0x2,
    Cyan = 0x3,
    Red = 0x4,
    Magenta = 0x5,
    Brown = 0x6,
    LightGray = 0x7,
    DarkGray = 0x8,
    LightBlue = 0x9,
    LightGreen = 0xA,
    LightCyan = 0xB,
    LightRed = 0xC,
    Pink = 0xD,
    Yellow = 0xE,
    White = 0xF,
}

pub const fn color_attr(fg: Color, bg: Color) -> u8 {
    (bg as u8) << 4 | (fg as u8)
}

pub static WRITER: Mutex<Writer> = Mutex::new(Writer::new());

pub struct Writer {
    col: usize,
    row: usize,
    attr: u8,
}

impl Writer {
    const fn new() -> Self {
        Self {
            col: 0,
            row: 0,
            attr: color_attr(Color::LightGray, Color::Black),
        }
    }

    fn buffer(&self) -> *mut u8 {
        VGA_BUFFER as *mut u8
    }

    pub fn set_attr(&mut self, attr: u8) {
        self.attr = attr;
    }

    pub fn clear(&mut self) {
        for i in 0..(WIDTH * HEIGHT) {
            unsafe {
                self.buffer().add(i * 2).write_volatile(b' ');
                self.buffer().add(i * 2 + 1).write_volatile(self.attr);
            }
        }
        self.row = 0;
        self.col = 0;
        self.update_cursor();
    }

    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\x08' => {
                // Backspace
                if self.col > 0 {
                    self.col -= 1;
                    self.put_char(self.row, self.col, b' ');
                }
            }
            byte => {
                if self.col >= WIDTH {
                    self.newline();
                }
                self.put_char(self.row, self.col, byte);
                self.col += 1;
            }
        }
        self.update_cursor();
    }

    fn put_char(&self, row: usize, col: usize, byte: u8) {
        let offset = (row * WIDTH + col) * 2;
        unsafe {
            self.buffer().add(offset).write_volatile(byte);
            self.buffer().add(offset + 1).write_volatile(self.attr);
        }
    }

    fn newline(&mut self) {
        if self.row < HEIGHT - 1 {
            self.row += 1;
        } else {
            self.scroll();
        }
        self.col = 0;
    }

    fn scroll(&mut self) {
        // Move rows 1..HEIGHT up by one
        for row in 1..HEIGHT {
            for col in 0..WIDTH {
                let src = (row * WIDTH + col) * 2;
                let dst = ((row - 1) * WIDTH + col) * 2;
                unsafe {
                    let ch = self.buffer().add(src).read_volatile();
                    let at = self.buffer().add(src + 1).read_volatile();
                    self.buffer().add(dst).write_volatile(ch);
                    self.buffer().add(dst + 1).write_volatile(at);
                }
            }
        }
        // Clear the last row
        for col in 0..WIDTH {
            self.put_char(HEIGHT - 1, col, b' ');
        }
    }

    fn update_cursor(&self) {
        let pos = (self.row * WIDTH + self.col) as u16;
        unsafe {
            use x86_64::instructions::port::Port;
            let mut cmd = Port::<u8>::new(0x3D4);
            let mut data = Port::<u8>::new(0x3D5);
            cmd.write(0x0F);
            data.write((pos & 0xFF) as u8);
            cmd.write(0x0E);
            data.write((pos >> 8) as u8);
        }
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            // Check for ANSI escape: ESC[ ... m
            if i + 2 < bytes.len() && bytes[i] == 0x1B && bytes[i + 1] == b'[' {
                // Parse the color code number
                let start = i + 2;
                let mut end = start;
                while end < bytes.len() && bytes[end] != b'm' {
                    end += 1;
                }
                if end < bytes.len() {
                    let code = core::str::from_utf8(&bytes[start..end]).unwrap_or("0");
                    self.apply_ansi_color(code);
                    i = end + 1;
                    continue;
                }
            }
            self.write_byte(bytes[i]);
            i += 1;
        }
        Ok(())
    }
}

impl Writer {
    /// Map ANSI SGR color codes to VGA attributes.
    fn apply_ansi_color(&mut self, code: &str) {
        match code {
            "0"  => self.attr = color_attr(Color::LightGray, Color::Black), // reset
            "1"  => self.attr = (self.attr & 0xF0) | 0x0F, // bold (white fg)
            "30" => self.attr = (self.attr & 0xF0) | Color::Black as u8,
            "31" => self.attr = (self.attr & 0xF0) | Color::LightRed as u8,
            "32" => self.attr = (self.attr & 0xF0) | Color::LightGreen as u8,
            "33" => self.attr = (self.attr & 0xF0) | Color::Yellow as u8,
            "34" => self.attr = (self.attr & 0xF0) | Color::LightBlue as u8,
            "35" => self.attr = (self.attr & 0xF0) | Color::Pink as u8,
            "36" => self.attr = (self.attr & 0xF0) | Color::LightCyan as u8,
            "37" => self.attr = (self.attr & 0xF0) | Color::White as u8,
            "90" => self.attr = (self.attr & 0xF0) | Color::DarkGray as u8,
            _ => {} // ignore unknown codes
        }
    }
}

// --- Public convenience functions ---

/// Print the MerlionOS boot banner with ASCII art Merlion.
pub fn print_banner() {
    let mut w = WRITER.lock();
    w.clear();
    use fmt::Write;

    // Water spray — cyan
    w.set_attr(color_attr(Color::LightCyan, Color::Black));
    let _ = w.write_str("                                  ~ ~ ~  .  ~ ~\n");
    let _ = w.write_str("                              ~ ~  . ~  ~  . ~ ~ ~\n");
    let _ = w.write_str("                           ~  ~ ~ ~  ~ . ~  ~\n");

    // Lion head — yellow/brown
    w.set_attr(color_attr(Color::Yellow, Color::Black));
    let _ = w.write_str("                       /\\_/\\~\n");
    let _ = w.write_str("                      ( o.o )    ");
    w.set_attr(color_attr(Color::White, Color::Black));
    let _ = w.write_str("MerlionOS v0.1.0\n");
    w.set_attr(color_attr(Color::Yellow, Color::Black));
    let _ = w.write_str("                       > ^ <     ");
    w.set_attr(color_attr(Color::DarkGray, Color::Black));
    let _ = w.write_str("x86_64 hobby OS\n");

    // Body/tail — green (fish scales)
    w.set_attr(color_attr(Color::LightGreen, Color::Black));
    let _ = w.write_str("                      /|   |\\      \n");
    let _ = w.write_str("                     ( |   | )   ");
    w.set_attr(color_attr(Color::LightGray, Color::Black));
    let _ = w.write_str("Written in Rust\n");
    w.set_attr(color_attr(Color::LightGreen, Color::Black));
    let _ = w.write_str("                      \\|___|/\n");
    let _ = w.write_str("                       |   |\n");

    // Fish tail
    w.set_attr(color_attr(Color::Cyan, Color::Black));
    let _ = w.write_str("                      /~~~~~\\\n");
    let _ = w.write_str("                     {  ><  }\n");
    let _ = w.write_str("                      \\_____/\n");

    // Waves
    w.set_attr(color_attr(Color::Blue, Color::Black));
    let _ = w.write_str("    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~\n");

    w.set_attr(color_attr(Color::LightGray, Color::Black));
    let _ = w.write_str("\n");
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::vga::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    ()            => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::print!("{}\n", format_args!($($arg)*)) };
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use fmt::Write;
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        WRITER.lock().write_fmt(args).unwrap();
    });
}
