/// PC Speaker audio driver and simple tone/melody player.
///
/// Uses PIT channel 2 to drive the PC speaker at a specified frequency.
/// Timing relies on the PIT channel 0 tick counter from [`crate::timer`].

use x86_64::instructions::port::Port;
use crate::timer;

// ---------------------------------------------------------------------------
// PIT / speaker hardware constants
// ---------------------------------------------------------------------------

/// PIT oscillator base frequency (Hz).
const PIT_BASE_FREQ: u32 = 1_193_182;

/// PIT command register (port 0x43).
const PIT_CMD_PORT: u16 = 0x43;

/// PIT channel 2 data register (port 0x42).
const PIT_CH2_PORT: u16 = 0x42;

/// System control port B (port 0x61) — bits 0-1 gate the speaker.
const SPEAKER_PORT: u16 = 0x61;

// ---------------------------------------------------------------------------
// Note frequency constants (Hz) — standard equal temperament, octave 4-5
// ---------------------------------------------------------------------------

/// C4 (Middle C) — 262 Hz
pub const C4: u16 = 262;
/// D4 — 294 Hz
pub const D4: u16 = 294;
/// E4 — 330 Hz
pub const E4: u16 = 330;
/// F4 — 349 Hz
pub const F4: u16 = 349;
/// G4 — 392 Hz
pub const G4: u16 = 392;
/// A4 (concert pitch) — 440 Hz
pub const A4: u16 = 440;
/// B4 — 494 Hz
pub const B4: u16 = 494;
/// C5 — 523 Hz
pub const C5: u16 = 523;

/// Rest (silence) — frequency 0 means no tone.
pub const REST: u16 = 0;

// ---------------------------------------------------------------------------
// Predefined melodies — each entry is (frequency_hz, duration_ms)
// ---------------------------------------------------------------------------

/// Cheerful ascending arpeggio played at boot.
pub const STARTUP_MELODY: &[(u16, u16)] = &[
    (C4, 120),
    (E4, 120),
    (G4, 120),
    (C5, 200),
];

/// Short descending two-tone error alert.
pub const ERROR_BEEP: &[(u16, u16)] = &[
    (A4, 150),
    (REST, 50),
    (C4, 300),
];

/// Quick rising two-tone success indicator.
pub const SUCCESS_BEEP: &[(u16, u16)] = &[
    (E4, 100),
    (C5, 200),
];

// ---------------------------------------------------------------------------
// Low-level speaker control
// ---------------------------------------------------------------------------

/// Enable PIT channel 2 output and connect it to the PC speaker.
fn speaker_on() {
    unsafe {
        let mut port = Port::<u8>::new(SPEAKER_PORT);
        let val = port.read();
        if val & 0x03 != 0x03 {
            port.write(val | 0x03);
        }
    }
}

/// Disconnect the PC speaker from PIT channel 2.
fn speaker_off() {
    unsafe {
        let mut port = Port::<u8>::new(SPEAKER_PORT);
        let val = port.read();
        port.write(val & 0xFC);
    }
}

/// Program PIT channel 2 to oscillate at `freq` Hz (mode 3, square wave).
fn set_pit_channel2(freq: u32) {
    let divisor = PIT_BASE_FREQ / freq;
    let divisor = if divisor > 0xFFFF { 0xFFFF } else { divisor as u16 };

    unsafe {
        // 0xB6 = channel 2, lobyte/hibyte, mode 3 (square wave), binary
        let mut cmd = Port::<u8>::new(PIT_CMD_PORT);
        cmd.write(0xB6);

        let mut data = Port::<u8>::new(PIT_CH2_PORT);
        data.write((divisor & 0xFF) as u8);
        data.write((divisor >> 8) as u8);
    }
}

/// Busy-wait for `ms` milliseconds using the PIT tick counter.
fn wait_ms(ms: u16) {
    let ticks_needed = ((ms as u64) * timer::PIT_FREQUENCY_HZ + 999) / 1000;
    let ticks_needed = if ticks_needed == 0 { 1 } else { ticks_needed };
    let target = timer::ticks() + ticks_needed;
    while timer::ticks() < target {
        x86_64::instructions::hlt();
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Emit a single beep at `frequency_hz` for `duration_ms` milliseconds.
///
/// A frequency of 0 produces silence for the given duration (rest).
pub fn beep(frequency_hz: u16, duration_ms: u16) {
    if frequency_hz == 0 || duration_ms == 0 {
        // Treat as a rest — just wait silently.
        if duration_ms > 0 {
            wait_ms(duration_ms);
        }
        return;
    }

    set_pit_channel2(frequency_hz as u32);
    speaker_on();
    wait_ms(duration_ms);
    speaker_off();
}

/// Play a single tone — alias for [`beep`] with clearer musical intent.
pub fn play_tone(freq: u16, duration: u16) {
    beep(freq, duration);
}

/// Play a sequence of notes (each is `(freq_hz, duration_ms)`).
///
/// A 10 ms gap is inserted between consecutive non-rest notes so that
/// repeated pitches are distinguishable.
pub fn play_melody(notes: &[(u16, u16)]) {
    for (i, &(freq, dur)) in notes.iter().enumerate() {
        // Insert a short inter-note gap for articulation (skip before the
        // first note and after rests).
        if i > 0 && freq != REST {
            wait_ms(10);
        }
        beep(freq, dur);
    }
}

// ---------------------------------------------------------------------------
// Shell command interface
// ---------------------------------------------------------------------------

/// Handle audio-related shell commands (`beep`, `play startup/error/success`).
///
/// Returns `true` if the command was handled, `false` otherwise.
pub fn shell_command(cmd: &str) -> bool {
    let cmd = cmd.trim();

    if cmd == "beep" {
        beep(A4, 200);
        return true;
    }

    if cmd.starts_with("beep ") {
        return handle_beep_args(cmd);
    }

    if cmd == "play startup" {
        crate::println!("Playing startup melody...");
        play_melody(STARTUP_MELODY);
        return true;
    }

    if cmd == "play error" {
        crate::println!("Playing error beep...");
        play_melody(ERROR_BEEP);
        return true;
    }

    if cmd == "play success" {
        crate::println!("Playing success beep...");
        play_melody(SUCCESS_BEEP);
        return true;
    }

    false
}

/// Parse `beep <freq> <duration_ms>` and play the tone.
fn handle_beep_args(cmd: &str) -> bool {
    let args = cmd.trim_start_matches("beep").trim();
    let mut parts = args.split_whitespace();

    let freq: u16 = match parts.next().and_then(|s| parse_u16(s)) {
        Some(f) => f,
        None => {
            crate::println!("Usage: beep <frequency_hz> <duration_ms>");
            return true;
        }
    };

    let duration: u16 = match parts.next().and_then(|s| parse_u16(s)) {
        Some(d) => d,
        None => {
            crate::println!("Usage: beep <frequency_hz> <duration_ms>");
            return true;
        }
    };

    if freq > 20_000 {
        crate::println!("Frequency out of range (max 20000 Hz).");
        return true;
    }

    crate::println!("Beep: {} Hz for {} ms", freq, duration);
    beep(freq, duration);
    true
}

/// Minimal u16 parser (no alloc, no core::str::parse dependency issues).
fn parse_u16(s: &str) -> Option<u16> {
    let mut result: u32 = 0;
    for &b in s.as_bytes() {
        if b < b'0' || b > b'9' {
            return None;
        }
        result = result * 10 + (b - b'0') as u32;
        if result > u16::MAX as u32 {
            return None;
        }
    }
    if s.is_empty() {
        return None;
    }
    Some(result as u16)
}
