/// PS/2 mouse driver for MerlionOS.
///
/// Handles standard 3-byte PS/2 mouse packets (flags, dx, dy) from IRQ12.
/// Tracks absolute cursor position clamped to screen bounds, plus button
/// state for left, right, and middle buttons.
///
/// # Usage
///
/// Call `init()` once during kernel startup to enable the auxiliary PS/2
/// device and begin data reporting.  Wire `handle_irq()` into the IRQ12
/// handler so each incoming byte is fed to the packet state machine.
/// Query the current cursor position and button state with `get_state()`.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use spin::Mutex;
use x86_64::instructions::port::Port;

// ---------------------------------------------------------------------------
// Screen bounds — defaults match standard VGA mode; fbconsole may differ.
// ---------------------------------------------------------------------------

/// Default screen width in pixels.
const SCREEN_WIDTH: u16 = 640;

/// Default screen height in pixels.
const SCREEN_HEIGHT: u16 = 480;

// ---------------------------------------------------------------------------
// PS/2 controller I/O ports
// ---------------------------------------------------------------------------

/// Data port shared with the keyboard controller.
const DATA_PORT: u16 = 0x60;

/// Status/command port for the PS/2 controller.
const CMD_PORT: u16 = 0x64;

// ---------------------------------------------------------------------------
// PS/2 commands and acknowledgement
// ---------------------------------------------------------------------------

/// Command: enable auxiliary (mouse) device.
const CMD_ENABLE_AUX: u8 = 0xA8;

/// Command: read controller status byte.
const CMD_READ_STATUS: u8 = 0x20;

/// Command: write controller status byte.
const CMD_WRITE_STATUS: u8 = 0x60;

/// Command: send the next byte written to 0x60 to the mouse.
const CMD_WRITE_MOUSE: u8 = 0xD4;

/// Mouse command: enable data reporting.
const MOUSE_ENABLE_REPORTING: u8 = 0xF4;

/// Acknowledgement byte sent by the mouse after a command.
const MOUSE_ACK: u8 = 0xFA;

// ---------------------------------------------------------------------------
// Packet flags (byte 0 of each 3-byte packet)
// ---------------------------------------------------------------------------

/// Bit 0 — left button pressed.
const FLAG_LEFT: u8 = 0x01;

/// Bit 1 — right button pressed.
const FLAG_RIGHT: u8 = 0x02;

/// Bit 2 — middle button pressed.
const FLAG_MIDDLE: u8 = 0x04;

/// Bit 4 — sign bit for X movement.
const FLAG_X_SIGN: u8 = 0x10;

/// Bit 5 — sign bit for Y movement.
const FLAG_Y_SIGN: u8 = 0x20;

/// Bit 6 — X overflow.
const FLAG_X_OVERFLOW: u8 = 0x40;

/// Bit 7 — Y overflow.
const FLAG_Y_OVERFLOW: u8 = 0x80;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Snapshot of the current mouse state.
#[derive(Debug, Clone, Copy)]
pub struct MouseState {
    /// Absolute X position in pixels (0 .. SCREEN_WIDTH-1).
    pub x: u16,
    /// Absolute Y position in pixels (0 .. SCREEN_HEIGHT-1).
    pub y: u16,
    /// Left button is held.
    pub left: bool,
    /// Right button is held.
    pub right: bool,
    /// Middle button is held.
    pub middle: bool,
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

/// Mutable cursor + button state, protected by a spinlock.
struct InnerState {
    x: i32,
    y: i32,
    left: bool,
    right: bool,
    middle: bool,
    /// Horizontal movement delta from the most recent packet.
    dx: i32,
    /// Vertical movement delta from the most recent packet.
    dy: i32,
}

static STATE: Mutex<InnerState> = Mutex::new(InnerState {
    x: 0,
    y: 0,
    left: false,
    right: false,
    middle: false,
    dx: 0,
    dy: 0,
});

/// Index into the current 3-byte packet (0, 1, or 2).
static PACKET_IDX: AtomicU8 = AtomicU8::new(0);

/// Temporary storage for the first two bytes of a packet.
static PACKET_BUF: Mutex<[u8; 2]> = Mutex::new([0u8; 2]);

/// Set to `true` after `init()` completes successfully.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Spin until the PS/2 controller input buffer is clear (bit 1 of status).
fn wait_write() {
    unsafe {
        let mut status = Port::<u8>::new(CMD_PORT);
        while status.read() & 0x02 != 0 {}
    }
}

/// Spin until the PS/2 controller output buffer is full (bit 0 of status).
fn wait_read() {
    unsafe {
        let mut status = Port::<u8>::new(CMD_PORT);
        while status.read() & 0x01 == 0 {}
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the PS/2 mouse.
///
/// Enables the auxiliary device, sets IRQ12 delivery, and tells the mouse
/// to start sending movement/button packets.  Logs progress to serial.
pub fn init() {
    unsafe {
        let mut cmd = Port::<u8>::new(CMD_PORT);
        let mut data = Port::<u8>::new(DATA_PORT);

        // 1. Enable auxiliary (mouse) device.
        wait_write();
        cmd.write(CMD_ENABLE_AUX);

        // 2. Read the controller configuration byte, set bit 1 (IRQ12).
        wait_write();
        cmd.write(CMD_READ_STATUS);
        wait_read();
        let mut status = data.read();
        status |= 0x02; // enable IRQ12
        status &= !0x20; // clear "disable mouse clock" bit

        wait_write();
        cmd.write(CMD_WRITE_STATUS);
        wait_write();
        data.write(status);

        // 3. Tell the controller the next byte goes to the mouse.
        wait_write();
        cmd.write(CMD_WRITE_MOUSE);
        wait_write();
        data.write(MOUSE_ENABLE_REPORTING);

        // 4. Wait for ACK (0xFA) from the mouse.
        wait_read();
        let ack = data.read();
        if ack == MOUSE_ACK {
            crate::serial_println!("[mouse] PS/2 mouse initialized, ACK received");
        } else {
            crate::serial_println!("[mouse] warning: expected ACK 0xFA, got 0x{:02X}", ack);
        }
    }

    // Centre the cursor.
    {
        let mut s = STATE.lock();
        s.x = SCREEN_WIDTH as i32 / 2;
        s.y = SCREEN_HEIGHT as i32 / 2;
    }

    INITIALIZED.store(true, Ordering::SeqCst);
}

/// Feed one byte from the IRQ12 handler into the packet state machine.
///
/// The PS/2 mouse sends data as a stream of 3-byte packets:
///
/// | Byte | Contents                                     |
/// |------|----------------------------------------------|
/// |  0   | Flags: buttons, sign bits, overflow           |
/// |  1   | X movement (unsigned, sign in flags byte)     |
/// |  2   | Y movement (unsigned, sign in flags byte)     |
///
/// This function tracks which byte of the current packet we are expecting,
/// reassembles signed deltas, and updates the global cursor state.
pub fn handle_irq(byte: u8) {
    let idx = PACKET_IDX.load(Ordering::SeqCst);

    match idx {
        0 => {
            // Byte 0 must always have bit 3 set (alignment bit).
            if byte & 0x08 == 0 {
                // Out of sync — stay at index 0 and wait for a valid flags byte.
                return;
            }
            PACKET_BUF.lock()[0] = byte;
            PACKET_IDX.store(1, Ordering::SeqCst);
        }
        1 => {
            PACKET_BUF.lock()[1] = byte;
            PACKET_IDX.store(2, Ordering::SeqCst);
        }
        2 => {
            // Complete packet: flags, raw_dx, raw_dy.
            let buf = *PACKET_BUF.lock();
            let flags = buf[0];
            let raw_dx = buf[1];
            let raw_dy = byte;

            PACKET_IDX.store(0, Ordering::SeqCst);

            // Discard packets with overflow bits set.
            if flags & (FLAG_X_OVERFLOW | FLAG_Y_OVERFLOW) != 0 {
                return;
            }

            // Sign-extend the 9-bit deltas.
            let dx: i32 = if flags & FLAG_X_SIGN != 0 {
                raw_dx as i32 - 256
            } else {
                raw_dx as i32
            };

            // PS/2 Y axis is inverted (positive = up), so negate for screen coords.
            let dy: i32 = if flags & FLAG_Y_SIGN != 0 {
                -(raw_dy as i32 - 256)
            } else {
                -(raw_dy as i32)
            };

            let mut s = STATE.lock();
            s.dx = dx;
            s.dy = dy;
            s.x = (s.x + dx).clamp(0, SCREEN_WIDTH as i32 - 1);
            s.y = (s.y + dy).clamp(0, SCREEN_HEIGHT as i32 - 1);
            s.left = flags & FLAG_LEFT != 0;
            s.right = flags & FLAG_RIGHT != 0;
            s.middle = flags & FLAG_MIDDLE != 0;
        }
        _ => {
            // Should never happen; reset to safe state.
            PACKET_IDX.store(0, Ordering::SeqCst);
        }
    }
}

/// Return a snapshot of the current mouse position and button state.
pub fn get_state() -> MouseState {
    let s = STATE.lock();
    MouseState {
        x: s.x as u16,
        y: s.y as u16,
        left: s.left,
        right: s.right,
        middle: s.middle,
    }
}
