/// GPIO driver for Raspberry Pi.
/// Controls the 40-pin header GPIO pins for digital I/O,
/// PWM output, and interrupt-on-change.
/// On x86_64 builds, provides simulated GPIO for testing.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// MMIO base address for BCM2837 GPIO (Raspberry Pi 3).
#[cfg(target_arch = "aarch64")]
const GPIO_BASE: u64 = 0x3F200000;

/// Number of GPIO lines on the BCM283x (covers header pins + internal GPIOs).
const NUM_PINS: usize = 54;

/// GPIO pin function mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinMode {
    Input,
    Output,
    Alt0,
    Alt1,
    Alt2,
    Alt3,
    Alt4,
    Alt5,
    Disabled,
}

/// Internal pull-up/pull-down configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullMode {
    None,
    Up,
    Down,
}

/// Logic level on a pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinLevel {
    Low,
    High,
}

/// Software PWM state for a pin.
#[derive(Debug, Clone, Copy)]
struct PwmState {
    active: bool,
    freq_hz: u32,
    duty_pct: u8,
}

/// Internal state for a single GPIO pin.
struct GpioPin {
    number: u8,
    mode: PinMode,
    pull: PullMode,
    level: PinLevel,
    name: &'static str,
    pwm: PwmState,
}

impl GpioPin {
    fn new(number: u8, name: &'static str) -> Self {
        Self {
            number,
            mode: PinMode::Disabled,
            pull: PullMode::None,
            level: PinLevel::Low,
            name,
            pwm: PwmState { active: false, freq_hz: 0, duty_pct: 0 },
        }
    }
}

/// Returns the default name for a given GPIO pin number.
fn pin_name(n: u8) -> &'static str {
    match n {
        2 => "I2C1 SDA",
        3 => "I2C1 SCL",
        4 => "GPCLK0",
        7 => "SPI0 CE1",
        8 => "SPI0 CE0",
        9 => "SPI0 MISO",
        10 => "SPI0 MOSI",
        11 => "SPI0 SCLK",
        14 => "UART TX",
        15 => "UART RX",
        17 => "GP17",
        18 => "PWM0",
        22 => "GP22",
        23 => "GP23",
        24 => "GP24",
        25 => "GP25",
        27 => "GP27",
        47 => "ACT LED",
        _ => "GPIO",
    }
}

/// Atomic counters for GPIO operations.
struct GpioStats {
    reads: AtomicU64,
    writes: AtomicU64,
    mode_changes: AtomicU64,
    toggles: AtomicU64,
    pwm_starts: AtomicU64,
}

impl GpioStats {
    const fn new() -> Self {
        Self {
            reads: AtomicU64::new(0),
            writes: AtomicU64::new(0),
            mode_changes: AtomicU64::new(0),
            toggles: AtomicU64::new(0),
            pwm_starts: AtomicU64::new(0),
        }
    }
}

static STATS: GpioStats = GpioStats::new();

/// Global GPIO pin state (simulated on x86_64).
static GPIO_PINS: Mutex<Option<Vec<GpioPin>>> = Mutex::new(None);

/// Initialise the GPIO subsystem with default pin modes.
pub fn init() {
    let mut pins = Vec::with_capacity(NUM_PINS);
    for i in 0..NUM_PINS as u8 {
        let mut pin = GpioPin::new(i, pin_name(i));
        // Set common pins to sensible defaults
        match i {
            2 | 3 => {
                pin.mode = PinMode::Alt0; // I2C
                pin.pull = PullMode::Up;
            }
            14 | 15 => {
                pin.mode = PinMode::Alt0; // UART
            }
            47 => {
                pin.mode = PinMode::Output; // ACT LED
                pin.level = PinLevel::Low;
            }
            _ => {}
        }
        pins.push(pin);
    }
    *GPIO_PINS.lock() = Some(pins);
}

/// Set the function mode for a pin.
pub fn set_mode(pin: u8, mode: PinMode) -> Result<(), &'static str> {
    let mut lock = GPIO_PINS.lock();
    let pins = lock.as_mut().ok_or("GPIO not initialised")?;
    let p = pins.get_mut(pin as usize).ok_or("invalid pin number")?;
    p.mode = mode;
    STATS.mode_changes.fetch_add(1, Ordering::Relaxed);
    #[cfg(target_arch = "aarch64")]
    unsafe { mmio_set_mode(pin, mode); }
    Ok(())
}

/// Set the pull-up/pull-down mode for a pin.
pub fn set_pull(pin: u8, pull: PullMode) -> Result<(), &'static str> {
    let mut lock = GPIO_PINS.lock();
    let pins = lock.as_mut().ok_or("GPIO not initialised")?;
    let p = pins.get_mut(pin as usize).ok_or("invalid pin number")?;
    p.pull = pull;
    Ok(())
}

/// Write a logic level to an output pin.
pub fn write(pin: u8, level: PinLevel) -> Result<(), &'static str> {
    let mut lock = GPIO_PINS.lock();
    let pins = lock.as_mut().ok_or("GPIO not initialised")?;
    let p = pins.get_mut(pin as usize).ok_or("invalid pin number")?;
    if p.mode != PinMode::Output {
        return Err("pin is not in output mode");
    }
    p.level = level;
    STATS.writes.fetch_add(1, Ordering::Relaxed);
    #[cfg(target_arch = "aarch64")]
    unsafe { mmio_write(pin, level); }
    Ok(())
}

/// Read the current logic level of a pin.
pub fn read(pin: u8) -> Result<PinLevel, &'static str> {
    let lock = GPIO_PINS.lock();
    let pins = lock.as_ref().ok_or("GPIO not initialised")?;
    let p = pins.get(pin as usize).ok_or("invalid pin number")?;
    STATS.reads.fetch_add(1, Ordering::Relaxed);
    #[cfg(target_arch = "aarch64")]
    {
        // On real hardware, read from MMIO register
        return Ok(unsafe { mmio_read(pin) });
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        Ok(p.level)
    }
}

/// Toggle an output pin between Low and High.
pub fn toggle(pin: u8) -> Result<(), &'static str> {
    let mut lock = GPIO_PINS.lock();
    let pins = lock.as_mut().ok_or("GPIO not initialised")?;
    let p = pins.get_mut(pin as usize).ok_or("invalid pin number")?;
    if p.mode != PinMode::Output {
        return Err("pin is not in output mode");
    }
    p.level = match p.level {
        PinLevel::Low => PinLevel::High,
        PinLevel::High => PinLevel::Low,
    };
    STATS.toggles.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Set a pin to an alternate function (for UART, I2C, SPI, etc.).
pub fn set_alt(pin: u8, alt_fn: PinMode) -> Result<(), &'static str> {
    match alt_fn {
        PinMode::Alt0 | PinMode::Alt1 | PinMode::Alt2 |
        PinMode::Alt3 | PinMode::Alt4 | PinMode::Alt5 => {}
        _ => return Err("not an alternate function mode"),
    }
    set_mode(pin, alt_fn)
}

/// Start software PWM on a pin (simulated).
pub fn pwm_start(pin: u8, freq_hz: u32, duty_pct: u8) -> Result<(), &'static str> {
    if duty_pct > 100 {
        return Err("duty cycle must be 0-100");
    }
    let mut lock = GPIO_PINS.lock();
    let pins = lock.as_mut().ok_or("GPIO not initialised")?;
    let p = pins.get_mut(pin as usize).ok_or("invalid pin number")?;
    if p.mode != PinMode::Output {
        return Err("pin must be in output mode for PWM");
    }
    p.pwm = PwmState {
        active: true,
        freq_hz,
        duty_pct,
    };
    STATS.pwm_starts.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Stop software PWM on a pin.
pub fn pwm_stop(pin: u8) -> Result<(), &'static str> {
    let mut lock = GPIO_PINS.lock();
    let pins = lock.as_mut().ok_or("GPIO not initialised")?;
    let p = pins.get_mut(pin as usize).ok_or("invalid pin number")?;
    p.pwm.active = false;
    p.pwm.freq_hz = 0;
    p.pwm.duty_pct = 0;
    Ok(())
}

fn fmt_mode(m: PinMode) -> &'static str {
    match m {
        PinMode::Input => "IN   ",
        PinMode::Output => "OUT  ",
        PinMode::Alt0 => "ALT0 ",
        PinMode::Alt1 => "ALT1 ",
        PinMode::Alt2 => "ALT2 ",
        PinMode::Alt3 => "ALT3 ",
        PinMode::Alt4 => "ALT4 ",
        PinMode::Alt5 => "ALT5 ",
        PinMode::Disabled => "OFF  ",
    }
}

fn fmt_pull(p: PullMode) -> &'static str {
    match p {
        PullMode::None => "none",
        PullMode::Up => "up  ",
        PullMode::Down => "down",
    }
}

fn fmt_level(l: PinLevel) -> &'static str {
    match l {
        PinLevel::Low => "LOW ",
        PinLevel::High => "HIGH",
    }
}

/// Return a formatted table of all GPIO pin states.
pub fn gpio_info() -> String {
    let lock = GPIO_PINS.lock();
    let pins = match lock.as_ref() {
        Some(p) => p,
        None => return String::from("GPIO not initialised\n"),
    };
    let mut out = String::new();
    out.push_str("Raspberry Pi GPIO (simulated on x86_64)\n");
    out.push_str("PIN  MODE  PULL  LEVEL  PWM        NAME\n");
    out.push_str("---- ----- ----- ------ ---------- ----------\n");
    for p in pins.iter() {
        let pwm_str = if p.pwm.active {
            format!("{}Hz {}%", p.pwm.freq_hz, p.pwm.duty_pct)
        } else {
            String::from("-")
        };
        out.push_str(&format!(
            "{:<4} {} {}  {}   {:<10} {}\n",
            p.number, fmt_mode(p.mode), fmt_pull(p.pull),
            fmt_level(p.level), pwm_str, p.name,
        ));
    }
    out
}

/// Return GPIO operation statistics.
pub fn gpio_stats() -> String {
    let r = STATS.reads.load(Ordering::Relaxed);
    let w = STATS.writes.load(Ordering::Relaxed);
    let m = STATS.mode_changes.load(Ordering::Relaxed);
    let t = STATS.toggles.load(Ordering::Relaxed);
    let p = STATS.pwm_starts.load(Ordering::Relaxed);
    format!(
        "GPIO Statistics:\n  Reads:        {}\n  Writes:       {}\n  Mode changes: {}\n  Toggles:      {}\n  PWM starts:   {}\n",
        r, w, m, t, p
    )
}

// ---- aarch64 MMIO stubs (real hardware) -----------------------------------

#[cfg(target_arch = "aarch64")]
unsafe fn mmio_set_mode(_pin: u8, _mode: PinMode) {
    // TODO: Write to GPFSEL registers at GPIO_BASE + 0x00..0x14
}

#[cfg(target_arch = "aarch64")]
unsafe fn mmio_write(_pin: u8, _level: PinLevel) {
    // TODO: Write to GPSET/GPCLR registers at GPIO_BASE + 0x1C/0x28
}

#[cfg(target_arch = "aarch64")]
unsafe fn mmio_read(_pin: u8) -> PinLevel {
    // TODO: Read from GPLEV registers at GPIO_BASE + 0x34
    PinLevel::Low
}
