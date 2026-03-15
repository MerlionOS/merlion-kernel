/// Minimal VGA text mode (mode 3) writer.
/// Writes directly to the VGA buffer at 0xB8000.
/// 80 columns x 25 rows, each cell is 2 bytes: [ascii, attribute].

const VGA_BUFFER: *mut u8 = 0xB8000 as *mut u8;
const VGA_WIDTH: usize = 80;
const VGA_HEIGHT: usize = 25;

/// VGA color attribute: light gray on black
const DEFAULT_ATTR: u8 = 0x07;
/// VGA color attribute: light green on black (for the banner)
const BANNER_ATTR: u8 = 0x0A;

/// Clear the entire screen with spaces.
pub fn clear_screen() {
    for i in 0..(VGA_WIDTH * VGA_HEIGHT) {
        unsafe {
            VGA_BUFFER.add(i * 2).write_volatile(b' ');
            VGA_BUFFER.add(i * 2 + 1).write_volatile(DEFAULT_ATTR);
        }
    }
}

/// Write a string at a given row and column with the specified color attribute.
fn write_at(row: usize, col: usize, s: &str, attr: u8) {
    let mut offset = (row * VGA_WIDTH + col) * 2;
    for byte in s.bytes() {
        if offset >= VGA_WIDTH * VGA_HEIGHT * 2 {
            break;
        }
        unsafe {
            VGA_BUFFER.add(offset).write_volatile(byte);
            VGA_BUFFER.add(offset + 1).write_volatile(attr);
        }
        offset += 2;
    }
}

/// Print the MerlionOS boot banner.
pub fn print_banner() {
    clear_screen();
    write_at(0, 0, "========================================", BANNER_ATTR);
    write_at(1, 0, "  MerlionOS v0.1.0", BANNER_ATTR);
    write_at(2, 0, "  Hello from MerlionOS!", BANNER_ATTR);
    write_at(3, 0, "========================================", BANNER_ATTR);
    write_at(5, 0, "Kernel reached entry point. System halted.", DEFAULT_ATTR);
}
