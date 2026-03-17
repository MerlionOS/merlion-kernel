/// PS/2 Synaptics touchpad driver for MerlionOS.
/// Supports basic touch, tap-to-click, two-finger scroll,
/// and palm rejection.
///
/// Communicates via the PS/2 auxiliary port, identifies Synaptics hardware
/// through the IDENTIFY command, and switches to absolute mode for precise
/// finger position and pressure tracking.

use alloc::string::String;
use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// PS/2 Synaptics protocol constants
// ---------------------------------------------------------------------------

/// PS/2 data port.
const DATA_PORT: u16 = 0x60;

/// PS/2 command/status port.
const CMD_PORT: u16 = 0x64;

/// Command: send next byte to auxiliary (mouse/touchpad) device.
const CMD_WRITE_AUX: u8 = 0xD4;

/// Touchpad command: identify device.
const TP_CMD_IDENTIFY: u8 = 0xF2;

/// Touchpad command: set sample rate (used for mode switching).
const TP_CMD_SET_RATE: u8 = 0xF3;

/// Touchpad command: enable data reporting.
const TP_CMD_ENABLE: u8 = 0xF4;

/// Touchpad command: disable data reporting.
const TP_CMD_DISABLE: u8 = 0xF5;

/// Acknowledgement byte from the touchpad.
const TP_ACK: u8 = 0xFA;

/// Synaptics absolute mode magic rate sequence.
/// Send SET_RATE with values 200, 100, 80 to enter absolute mode.
const SYNAPTICS_MAGIC_RATES: [u8; 3] = [200, 100, 80];

// ---------------------------------------------------------------------------
// Gesture thresholds (integer math, no floating point)
// ---------------------------------------------------------------------------

/// Minimum pressure to register as a touch (0-255 range).
const MIN_PRESSURE: u8 = 25;

/// Maximum finger width before palm rejection triggers.
const PALM_WIDTH_THRESHOLD: u8 = 10;

/// Tap duration threshold in ticks (100Hz PIT, ~200ms).
const TAP_THRESHOLD_TICKS: u64 = 20;

/// Two-finger scroll minimum delta to register.
const SCROLL_THRESHOLD: i32 = 5;

/// Edge scroll zone width (rightmost pixels for vertical scroll).
const EDGE_SCROLL_ZONE: i32 = 200;

/// Edge scroll zone height (bottom pixels for horizontal scroll).
const EDGE_SCROLL_BOTTOM: i32 = 200;

/// Touchpad coordinate range (Synaptics typical).
const TP_X_MIN: i32 = 1472;
const TP_X_MAX: i32 = 5472;
const TP_Y_MIN: i32 = 1408;
const TP_Y_MAX: i32 = 4448;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Touchpad gesture events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchpadGesture {
    SingleTap,
    TwoFingerTap,
    ThreeFingerTap,
    ScrollVertical(i32),
    ScrollHorizontal(i32),
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

struct TouchpadState {
    /// Whether the touchpad hardware was detected.
    detected: bool,
    /// Whether the touchpad is enabled.
    enabled: bool,
    /// Whether absolute mode is active.
    absolute_mode: bool,
    /// Current finger X position (absolute, in touchpad coordinates).
    finger_x: i32,
    /// Current finger Y position (absolute, in touchpad coordinates).
    finger_y: i32,
    /// Previous finger X for delta calculation.
    prev_x: i32,
    /// Previous finger Y for delta calculation.
    prev_y: i32,
    /// Current pressure (0-255).
    pressure: u8,
    /// Current finger width (0-15).
    finger_width: u8,
    /// Number of fingers currently touching.
    finger_count: u8,
    /// Whether a finger is currently down.
    finger_down: bool,
    /// Tick when finger first touched down.
    touch_start_tick: u64,
    /// Sensitivity level (1-10, default 5).
    sensitivity: u8,
    /// Total packets processed.
    packets: u64,
    /// Total taps detected.
    taps: u64,
    /// Total scroll events generated.
    scrolls: u64,
    /// Palm rejections.
    palm_rejects: u64,
    /// Pending gesture event.
    pending_gesture: Option<TouchpadGesture>,
    /// Packet assembly buffer (6 bytes for Synaptics absolute).
    pkt_buf: [u8; 6],
    /// Current byte index in packet assembly.
    pkt_idx: u8,
    /// Last scroll Y for two-finger scroll delta.
    last_scroll_y: i32,
    /// Last scroll X for two-finger scroll delta.
    last_scroll_x: i32,
}

static STATE: Mutex<TouchpadState> = Mutex::new(TouchpadState {
    detected: false,
    enabled: false,
    absolute_mode: false,
    finger_x: 0,
    finger_y: 0,
    prev_x: 0,
    prev_y: 0,
    pressure: 0,
    finger_width: 0,
    finger_count: 0,
    finger_down: false,
    touch_start_tick: 0,
    sensitivity: 5,
    packets: 0,
    taps: 0,
    scrolls: 0,
    palm_rejects: 0,
    pending_gesture: None,
    pkt_buf: [0u8; 6],
    pkt_idx: 0,
    last_scroll_y: 0,
    last_scroll_x: 0,
});

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static ENABLED: AtomicBool = AtomicBool::new(false);
static GESTURE_COUNT: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// PS/2 I/O helpers
// ---------------------------------------------------------------------------

/// Wait until the PS/2 input buffer is clear.
fn wait_write() {
    unsafe {
        let mut status = x86_64::instructions::port::Port::<u8>::new(CMD_PORT);
        for _ in 0..10000u32 {
            if status.read() & 0x02 == 0 {
                return;
            }
        }
    }
}

/// Wait until the PS/2 output buffer has data.
fn wait_read() {
    unsafe {
        let mut status = x86_64::instructions::port::Port::<u8>::new(CMD_PORT);
        for _ in 0..10000u32 {
            if status.read() & 0x01 != 0 {
                return;
            }
        }
    }
}

/// Send a command byte to the auxiliary device.
fn send_aux(byte: u8) -> u8 {
    unsafe {
        let mut cmd = x86_64::instructions::port::Port::<u8>::new(CMD_PORT);
        let mut data = x86_64::instructions::port::Port::<u8>::new(DATA_PORT);
        wait_write();
        cmd.write(CMD_WRITE_AUX);
        wait_write();
        data.write(byte);
        wait_read();
        data.read()
    }
}

// ---------------------------------------------------------------------------
// Packet processing
// ---------------------------------------------------------------------------

impl TouchpadState {
    /// Process a complete 6-byte Synaptics absolute mode packet.
    fn process_absolute_packet(&mut self) {
        // Synaptics 6-byte absolute packet layout:
        // Byte 0: [1][0][Finger][Reserved][0][Gesture][Right][Left]
        // Byte 1: Y position [11:8] (high nibble), X position [11:8] (low nibble)
        // Byte 2: Z pressure (0-255)
        // Byte 3: [1][1][Y[12]][X[12]][0][Gesture][Right][Left]
        // Byte 4: X position [7:0]
        // Byte 5: Y position [7:0]

        let b0 = self.pkt_buf[0];
        let b1 = self.pkt_buf[1];
        let b2 = self.pkt_buf[2];
        let b3 = self.pkt_buf[3];
        let b4 = self.pkt_buf[4];
        let b5 = self.pkt_buf[5];

        // Validate packet markers
        if b0 & 0xC0 != 0x80 || b3 & 0xC0 != 0xC0 {
            // Invalid packet, discard
            return;
        }

        // Extract X position (13 bits)
        let x_high = ((b3 as i32 & 0x10) << 8) | ((b1 as i32 & 0x0F) << 8);
        let x = x_high | b4 as i32;

        // Extract Y position (13 bits)
        let y_high = ((b3 as i32 & 0x20) << 7) | ((b1 as i32 & 0xF0) << 4);
        let y = y_high | b5 as i32;

        let pressure = b2;
        let finger = (b0 >> 5) & 0x01;
        let width = if pressure > 0 && finger != 0 { ((b0 >> 4) & 0x01) | ((b1 >> 2) & 0x06) } else { 0 };

        self.pressure = pressure;
        self.finger_width = width as u8;
        self.packets += 1;

        // Palm rejection
        if self.finger_width >= PALM_WIDTH_THRESHOLD {
            self.palm_rejects += 1;
            return;
        }

        let tick = crate::timer::ticks();

        // Sensitivity scaling: delta * sensitivity / 5
        let _sens = self.sensitivity as i32;

        if pressure >= MIN_PRESSURE && finger != 0 {
            if !self.finger_down {
                // Finger just touched down
                self.finger_down = true;
                self.touch_start_tick = tick;
                self.prev_x = x;
                self.prev_y = y;
                self.last_scroll_y = y;
                self.last_scroll_x = x;
            }

            self.finger_x = x;
            self.finger_y = y;

            // Detect number of fingers from width heuristic
            // width 0-3 = 1 finger, 4-7 = 2 fingers, 8+ = 3 fingers
            self.finger_count = if self.finger_width < 4 {
                1
            } else if self.finger_width < 8 {
                2
            } else {
                3
            };

            // Two-finger scroll detection
            if self.finger_count >= 2 {
                let dy = y - self.last_scroll_y;
                let dx = x - self.last_scroll_x;
                if dy.abs() > SCROLL_THRESHOLD {
                    let scaled_dy = (dy * self.sensitivity as i32) / 5;
                    self.pending_gesture = Some(TouchpadGesture::ScrollVertical(scaled_dy));
                    self.scrolls += 1;
                    self.last_scroll_y = y;
                    GESTURE_COUNT.fetch_add(1, Ordering::Relaxed);
                }
                if dx.abs() > SCROLL_THRESHOLD {
                    let scaled_dx = (dx * self.sensitivity as i32) / 5;
                    self.pending_gesture = Some(TouchpadGesture::ScrollHorizontal(scaled_dx));
                    self.scrolls += 1;
                    self.last_scroll_x = x;
                    GESTURE_COUNT.fetch_add(1, Ordering::Relaxed);
                }
            }

            // Edge scrolling (single finger at right/bottom edge)
            if self.finger_count == 1 {
                if x > TP_X_MAX - EDGE_SCROLL_ZONE {
                    let dy = y - self.prev_y;
                    if dy.abs() > SCROLL_THRESHOLD {
                        let scaled_dy = (dy * self.sensitivity as i32) / 5;
                        self.pending_gesture = Some(TouchpadGesture::ScrollVertical(scaled_dy));
                        self.scrolls += 1;
                        GESTURE_COUNT.fetch_add(1, Ordering::Relaxed);
                    }
                }
                if y < TP_Y_MIN + EDGE_SCROLL_BOTTOM {
                    let dx = x - self.prev_x;
                    if dx.abs() > SCROLL_THRESHOLD {
                        let scaled_dx = (dx * self.sensitivity as i32) / 5;
                        self.pending_gesture = Some(TouchpadGesture::ScrollHorizontal(scaled_dx));
                        self.scrolls += 1;
                        GESTURE_COUNT.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }

            self.prev_x = x;
            self.prev_y = y;
        } else if self.finger_down {
            // Finger lifted — check for tap
            self.finger_down = false;
            let duration = tick.wrapping_sub(self.touch_start_tick);

            if duration < TAP_THRESHOLD_TICKS {
                let gesture = match self.finger_count {
                    1 => TouchpadGesture::SingleTap,
                    2 => TouchpadGesture::TwoFingerTap,
                    _ => TouchpadGesture::ThreeFingerTap,
                };
                self.pending_gesture = Some(gesture);
                self.taps += 1;
                GESTURE_COUNT.fetch_add(1, Ordering::Relaxed);
            }

            self.finger_count = 0;
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the touchpad driver.
///
/// Attempts to identify a Synaptics touchpad on the PS/2 auxiliary port
/// and enable absolute mode for precise tracking.
pub fn init() {
    // Try to identify the touchpad
    let ack = send_aux(TP_CMD_IDENTIFY);

    let mut s = STATE.lock();
    if ack == TP_ACK {
        s.detected = true;

        // Attempt to enter Synaptics absolute mode via magic rate sequence
        for &rate in &SYNAPTICS_MAGIC_RATES {
            send_aux(TP_CMD_SET_RATE);
            send_aux(rate);
        }
        s.absolute_mode = true;

        // Enable data reporting
        send_aux(TP_CMD_ENABLE);
        s.enabled = true;
        ENABLED.store(true, Ordering::SeqCst);
        crate::serial_println!("[touchpad] Synaptics touchpad detected, absolute mode enabled");
    } else {
        s.detected = false;
        crate::serial_println!("[touchpad] No Synaptics touchpad detected (ACK=0x{:02X})", ack);
    }

    INITIALIZED.store(true, Ordering::SeqCst);
}

/// Feed one byte from the touchpad IRQ into the packet state machine.
pub fn handle_irq(byte: u8) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }

    let mut s = STATE.lock();
    let idx = s.pkt_idx as usize;

    if idx == 0 {
        // First byte must have bit 7 set, bit 6 clear for Synaptics
        if byte & 0xC0 != 0x80 {
            return; // Not a valid first byte
        }
    }

    if idx < 6 {
        s.pkt_buf[idx] = byte;
        s.pkt_idx += 1;
    }

    if s.pkt_idx >= 6 {
        s.pkt_idx = 0;
        s.process_absolute_packet();
    }
}

/// Poll for the next pending gesture event.
pub fn poll_gesture() -> Option<TouchpadGesture> {
    let mut s = STATE.lock();
    s.pending_gesture.take()
}

/// Enable the touchpad.
pub fn touchpad_enable() {
    if !INITIALIZED.load(Ordering::Relaxed) {
        return;
    }
    send_aux(TP_CMD_ENABLE);
    ENABLED.store(true, Ordering::SeqCst);
    STATE.lock().enabled = true;
    crate::serial_println!("[touchpad] Enabled");
}

/// Disable the touchpad.
pub fn touchpad_disable() {
    send_aux(TP_CMD_DISABLE);
    ENABLED.store(false, Ordering::SeqCst);
    STATE.lock().enabled = false;
    crate::serial_println!("[touchpad] Disabled");
}

/// Set touchpad sensitivity (1-10).
pub fn set_sensitivity(level: u8) {
    let clamped = if level < 1 { 1 } else if level > 10 { 10 } else { level };
    STATE.lock().sensitivity = clamped;
}

/// Return human-readable touchpad information.
pub fn touchpad_info() -> String {
    let s = STATE.lock();
    format!(
        "PS/2 Synaptics Touchpad\n\
         Detected:      {}\n\
         Enabled:       {}\n\
         Absolute mode: {}\n\
         Finger down:   {}\n\
         Position:      ({}, {})\n\
         Pressure:      {}\n\
         Finger width:  {}\n\
         Finger count:  {}\n\
         Sensitivity:   {}/10\n\
         Coord range:   X[{}..{}] Y[{}..{}]",
        s.detected,
        s.enabled,
        s.absolute_mode,
        s.finger_down,
        s.finger_x, s.finger_y,
        s.pressure,
        s.finger_width,
        s.finger_count,
        s.sensitivity,
        TP_X_MIN, TP_X_MAX, TP_Y_MIN, TP_Y_MAX,
    )
}

/// Return touchpad statistics.
pub fn touchpad_stats() -> String {
    let s = STATE.lock();
    format!(
        "Touchpad Statistics\n\
         Packets:        {}\n\
         Taps detected:  {}\n\
         Scroll events:  {}\n\
         Palm rejects:   {}\n\
         Total gestures: {}",
        s.packets,
        s.taps,
        s.scrolls,
        s.palm_rejects,
        GESTURE_COUNT.load(Ordering::Relaxed),
    )
}
