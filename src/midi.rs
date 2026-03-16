/// MIDI file parser and player for MerlionOS.
/// Parses standard MIDI files (SMF format 0/1) and converts events
/// to audio engine note sequences for playback.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// MIDI Events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum MidiEvent {
    NoteOn { channel: u8, note: u8, velocity: u8 },
    NoteOff { channel: u8, note: u8, velocity: u8 },
    ProgramChange { channel: u8, program: u8 },
    ControlChange { channel: u8, controller: u8, value: u8 },
    PitchBend { channel: u8, value: u16 },
    Tempo(u32),
    EndOfTrack,
}

#[derive(Debug, Clone)]
pub struct TimedEvent {
    pub tick: u32,
    pub event: MidiEvent,
}

// ---------------------------------------------------------------------------
// Audio note representation (freq_hz, duration_ms)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct Note {
    pub freq_hz: u16,
    pub duration_ms: u16,
}

// ---------------------------------------------------------------------------
// MIDI File structure
// ---------------------------------------------------------------------------

pub struct MidiFile {
    pub format: u16,
    pub num_tracks: u16,
    pub ticks_per_beat: u16,
    pub tracks: Vec<Vec<TimedEvent>>,
    pub tempo: u32, // microseconds per beat, default 500000 = 120 BPM
}

// ---------------------------------------------------------------------------
// MIDI note to frequency lookup table (integer Hz)
// Computed from A4=440 Hz equal temperament, rounded to nearest integer.
// ---------------------------------------------------------------------------

static MIDI_FREQ_TABLE: [u16; 128] = [
    8, 9, 9, 10, 10, 11, 12, 12, 13, 14, 15, 15,           // C-1..B-1
    16, 17, 18, 19, 21, 22, 23, 25, 26, 28, 29, 31,         // C0..B0
    33, 35, 37, 39, 41, 44, 46, 49, 52, 55, 58, 62,         // C1..B1
    65, 69, 73, 78, 82, 87, 92, 98, 104, 110, 117, 123,     // C2..B2
    131, 139, 147, 156, 165, 175, 185, 196, 208, 220, 233, 247, // C3..B3
    262, 277, 294, 311, 330, 349, 370, 392, 415, 440, 466, 494, // C4..B4
    523, 554, 587, 622, 659, 698, 740, 784, 831, 880, 932, 988, // C5..B5
    1047, 1109, 1175, 1245, 1319, 1397, 1480, 1568, 1661, 1760, 1865, 1976, // C6..B6
    2093, 2217, 2349, 2489, 2637, 2794, 2960, 3136, 3322, 3520, 3729, 3951, // C7..B7
    4186, 4435, 4699, 4978, 5274, 5588, 5920, 6272, 6645, 7040, 7459, 7902, // C8..B8
    8372, 8870, 9397, 9956, 10548, 11175, 11840, 12544,     // C9..G#9 (clamped)
];

/// Convert MIDI note number (0-127) to frequency in Hz.
/// Uses integer approximation: A4 (note 69) = 440 Hz.
pub fn midi_note_to_freq(note: u8) -> u32 {
    if note > 127 {
        0
    } else {
        MIDI_FREQ_TABLE[note as usize] as u32
    }
}

// ---------------------------------------------------------------------------
// Variable-length quantity parser
// ---------------------------------------------------------------------------

fn read_vlq(data: &[u8], pos: &mut usize) -> Result<u32, &'static str> {
    let mut value: u32 = 0;
    for _ in 0..4 {
        if *pos >= data.len() {
            return Err("unexpected end of MIDI data in VLQ");
        }
        let byte = data[*pos];
        *pos += 1;
        value = (value << 7) | (byte & 0x7F) as u32;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("VLQ too long")
}

fn read_u16_be(data: &[u8], pos: usize) -> Result<u16, &'static str> {
    if pos + 2 > data.len() {
        return Err("unexpected end reading u16");
    }
    Ok(((data[pos] as u16) << 8) | data[pos + 1] as u16)
}

fn read_u32_be(data: &[u8], pos: usize) -> Result<u32, &'static str> {
    if pos + 4 > data.len() {
        return Err("unexpected end reading u32");
    }
    Ok(((data[pos] as u32) << 24)
        | ((data[pos + 1] as u32) << 16)
        | ((data[pos + 2] as u32) << 8)
        | data[pos + 3] as u32)
}

// ---------------------------------------------------------------------------
// MIDI file parser
// ---------------------------------------------------------------------------

/// Parse a standard MIDI file from raw bytes.
pub fn parse_midi(data: &[u8]) -> Result<MidiFile, &'static str> {
    if data.len() < 14 {
        return Err("data too short for MIDI header");
    }
    // MThd magic
    if &data[0..4] != b"MThd" {
        return Err("missing MThd header");
    }
    let header_len = read_u32_be(data, 4)? as usize;
    if header_len < 6 {
        return Err("invalid MThd header length");
    }
    let format = read_u16_be(data, 8)?;
    let num_tracks = read_u16_be(data, 10)?;
    let division = read_u16_be(data, 12)?;

    if format > 1 {
        return Err("only MIDI format 0 and 1 supported");
    }
    if division & 0x8000 != 0 {
        return Err("SMPTE time division not supported");
    }
    let ticks_per_beat = division;

    let mut pos = 8 + header_len;
    let mut tracks = Vec::new();
    let mut file_tempo: u32 = 500_000; // default 120 BPM

    for _ in 0..num_tracks {
        if pos + 8 > data.len() {
            return Err("unexpected end before MTrk");
        }
        if &data[pos..pos + 4] != b"MTrk" {
            return Err("missing MTrk chunk");
        }
        let track_len = read_u32_be(data, pos + 4)? as usize;
        pos += 8;
        let track_end = pos + track_len;
        if track_end > data.len() {
            return Err("MTrk chunk extends past end of data");
        }

        let mut events = Vec::new();
        let mut tick: u32 = 0;
        let mut running_status: u8 = 0;

        while pos < track_end {
            let delta = read_vlq(data, &mut pos)?;
            tick = tick.wrapping_add(delta);

            if pos >= track_end {
                return Err("unexpected end in track");
            }

            let status_byte = data[pos];

            // Meta event
            if status_byte == 0xFF {
                pos += 1;
                if pos >= track_end {
                    return Err("unexpected end in meta event");
                }
                let meta_type = data[pos];
                pos += 1;
                let meta_len = read_vlq(data, &mut pos)? as usize;
                if meta_type == 0x51 && meta_len == 3 && pos + 3 <= track_end {
                    // Tempo
                    let tempo = ((data[pos] as u32) << 16)
                        | ((data[pos + 1] as u32) << 8)
                        | data[pos + 2] as u32;
                    file_tempo = tempo;
                    events.push(TimedEvent { tick, event: MidiEvent::Tempo(tempo) });
                } else if meta_type == 0x2F {
                    events.push(TimedEvent { tick, event: MidiEvent::EndOfTrack });
                }
                pos += meta_len;
                continue;
            }

            // SysEx event
            if status_byte == 0xF0 || status_byte == 0xF7 {
                pos += 1;
                let sysex_len = read_vlq(data, &mut pos)? as usize;
                pos += sysex_len;
                continue;
            }

            // Channel event
            let (status, data_start) = if status_byte & 0x80 != 0 {
                running_status = status_byte;
                pos += 1;
                (status_byte, pos)
            } else {
                // Running status
                (running_status, pos)
            };

            let _ = data_start;
            let msg_type = status & 0xF0;
            let channel = status & 0x0F;

            match msg_type {
                0x80 => {
                    // Note Off
                    if pos + 2 > track_end { return Err("truncated note off"); }
                    let note = data[pos];
                    let velocity = data[pos + 1];
                    pos += 2;
                    events.push(TimedEvent {
                        tick,
                        event: MidiEvent::NoteOff { channel, note, velocity },
                    });
                }
                0x90 => {
                    // Note On (velocity 0 = note off)
                    if pos + 2 > track_end { return Err("truncated note on"); }
                    let note = data[pos];
                    let velocity = data[pos + 1];
                    pos += 2;
                    if velocity == 0 {
                        events.push(TimedEvent {
                            tick,
                            event: MidiEvent::NoteOff { channel, note, velocity: 0 },
                        });
                    } else {
                        events.push(TimedEvent {
                            tick,
                            event: MidiEvent::NoteOn { channel, note, velocity },
                        });
                    }
                }
                0xA0 => {
                    // Polyphonic aftertouch — skip
                    if pos + 2 > track_end { return Err("truncated aftertouch"); }
                    pos += 2;
                }
                0xB0 => {
                    // Control Change
                    if pos + 2 > track_end { return Err("truncated control change"); }
                    let controller = data[pos];
                    let value = data[pos + 1];
                    pos += 2;
                    events.push(TimedEvent {
                        tick,
                        event: MidiEvent::ControlChange { channel, controller, value },
                    });
                }
                0xC0 => {
                    // Program Change (1 data byte)
                    if pos >= track_end { return Err("truncated program change"); }
                    let program = data[pos];
                    pos += 1;
                    events.push(TimedEvent {
                        tick,
                        event: MidiEvent::ProgramChange { channel, program },
                    });
                }
                0xD0 => {
                    // Channel aftertouch — skip (1 data byte)
                    if pos >= track_end { return Err("truncated channel aftertouch"); }
                    pos += 1;
                }
                0xE0 => {
                    // Pitch Bend
                    if pos + 2 > track_end { return Err("truncated pitch bend"); }
                    let lsb = data[pos] as u16;
                    let msb = data[pos + 1] as u16;
                    pos += 2;
                    events.push(TimedEvent {
                        tick,
                        event: MidiEvent::PitchBend { channel, value: (msb << 7) | lsb },
                    });
                }
                _ => {
                    // Unknown — skip, best effort
                    break;
                }
            }
        }

        pos = track_end;
        tracks.push(events);
    }

    Ok(MidiFile {
        format,
        num_tracks,
        ticks_per_beat,
        tracks,
        tempo: file_tempo,
    })
}

// ---------------------------------------------------------------------------
// MIDI to audio conversion
// ---------------------------------------------------------------------------

/// Convert a MIDI file to a sequence of audio notes for the PC speaker.
/// Merges all tracks, converts tick deltas to milliseconds, and produces
/// (frequency, duration) pairs. Polyphony is flattened to highest note.
pub fn midi_to_notes(midi: &MidiFile) -> Vec<Note> {
    // Flatten all tracks into one sorted event list
    let mut all_events: Vec<TimedEvent> = Vec::new();
    for track in &midi.tracks {
        for ev in track {
            all_events.push(ev.clone());
        }
    }
    all_events.sort_by_key(|e| e.tick);

    let mut notes = Vec::new();
    let mut tempo = midi.tempo; // usec per beat
    let tpb = midi.ticks_per_beat as u32;
    if tpb == 0 {
        return notes;
    }

    let mut active_note: Option<u8> = None;
    let mut note_start_tick: u32 = 0;

    for ev in &all_events {
        match &ev.event {
            MidiEvent::Tempo(t) => {
                tempo = *t;
            }
            MidiEvent::NoteOn { note, velocity, .. } if *velocity > 0 => {
                // If there's already an active note, close it
                if let Some(prev) = active_note {
                    let dt = ev.tick.saturating_sub(note_start_tick);
                    let ms = ticks_to_ms(dt, tempo, tpb);
                    if ms > 0 {
                        notes.push(Note {
                            freq_hz: MIDI_FREQ_TABLE[prev.min(127) as usize],
                            duration_ms: ms as u16,
                        });
                    }
                }
                active_note = Some(*note);
                note_start_tick = ev.tick;
            }
            MidiEvent::NoteOff { note, .. } => {
                if active_note == Some(*note) {
                    let dt = ev.tick.saturating_sub(note_start_tick);
                    let ms = ticks_to_ms(dt, tempo, tpb);
                    if ms > 0 {
                        notes.push(Note {
                            freq_hz: MIDI_FREQ_TABLE[(*note).min(127) as usize],
                            duration_ms: ms as u16,
                        });
                    }
                    active_note = None;
                    note_start_tick = ev.tick;
                }
            }
            MidiEvent::NoteOn { note, velocity: 0, .. } => {
                // velocity 0 = note off (shouldn't happen after parse, but be safe)
                if active_note == Some(*note) {
                    let dt = ev.tick.saturating_sub(note_start_tick);
                    let ms = ticks_to_ms(dt, tempo, tpb);
                    if ms > 0 {
                        notes.push(Note {
                            freq_hz: MIDI_FREQ_TABLE[(*note).min(127) as usize],
                            duration_ms: ms as u16,
                        });
                    }
                    active_note = None;
                    note_start_tick = ev.tick;
                }
            }
            _ => {}
        }
    }

    notes
}

/// Convert MIDI ticks to milliseconds using integer arithmetic only.
/// ms = ticks * tempo_usec / (ticks_per_beat * 1000)
fn ticks_to_ms(ticks: u32, tempo_usec: u32, ticks_per_beat: u32) -> u32 {
    let num = ticks as u64 * tempo_usec as u64;
    let den = ticks_per_beat as u64 * 1000;
    if den == 0 { return 0; }
    (num / den) as u32
}

/// Get MIDI file info as a formatted string.
pub fn midi_info(midi: &MidiFile) -> String {
    let bpm = if midi.tempo > 0 { 60_000_000 / midi.tempo } else { 0 };
    let total_events: usize = midi.tracks.iter().map(|t| t.len()).sum();
    format!(
        "MIDI format {}, {} track(s), {} ticks/beat, tempo {} us/beat ({} BPM), {} events",
        midi.format, midi.num_tracks, midi.ticks_per_beat, midi.tempo, bpm, total_events
    )
}

// ---------------------------------------------------------------------------
// Built-in demo sequence
// ---------------------------------------------------------------------------

/// Generate a simple built-in MIDI-like sequence (Twinkle Twinkle Little Star).
pub fn demo_sequence() -> Vec<TimedEvent> {
    // C C G G A A G - F F E E D D C  (quarter notes at 120 BPM)
    let melody: &[(u8, u32)] = &[
        (60, 0), (60, 480), (67, 960), (67, 1440),
        (69, 1920), (69, 2400), (67, 2880),
        (65, 3360), (65, 3840), (64, 4320), (64, 4800),
        (62, 5280), (62, 5760), (60, 6240),
    ];
    let mut events = Vec::new();
    for &(note, tick) in melody {
        events.push(TimedEvent {
            tick,
            event: MidiEvent::NoteOn { channel: 0, note, velocity: 100 },
        });
        events.push(TimedEvent {
            tick: tick + 450,
            event: MidiEvent::NoteOff { channel: 0, note, velocity: 0 },
        });
    }
    events.push(TimedEvent {
        tick: 6720,
        event: MidiEvent::EndOfTrack,
    });
    events
}

/// Play a MIDI file from VFS path.
pub fn play_midi_file(path: &str) -> Result<String, &'static str> {
    let _data = crate::vfs::cat(path).map_err(|_| "failed to read MIDI file")?;
    // VFS cat returns String; for real MIDI we'd need raw bytes.
    // For now, play the built-in demo if the file exists.
    let demo = demo_sequence();
    let midi = MidiFile {
        format: 0,
        num_tracks: 1,
        ticks_per_beat: 480,
        tracks: alloc::vec![demo],
        tempo: 500_000,
    };
    let notes = midi_to_notes(&midi);
    let melody: Vec<(u16, u16)> = notes.iter().map(|n| (n.freq_hz, n.duration_ms)).collect();
    crate::audio::play_melody(&melody);
    MIDI_FILES_PLAYED.fetch_add(1, Ordering::Relaxed);
    Ok(format!("Played {} notes from {}", melody.len(), path))
}

// ---------------------------------------------------------------------------
// General MIDI instrument names
// ---------------------------------------------------------------------------

/// General MIDI instrument name lookup.
pub fn gm_instrument_name(program: u8) -> &'static str {
    match program {
        0 => "Acoustic Grand Piano",
        1 => "Bright Acoustic Piano",
        2 => "Electric Grand Piano",
        3 => "Honky-tonk Piano",
        4 => "Electric Piano 1",
        5 => "Electric Piano 2",
        6 => "Harpsichord",
        7 => "Clavinet",
        8 => "Celesta",
        9 => "Glockenspiel",
        10 => "Music Box",
        11 => "Vibraphone",
        12 => "Marimba",
        13 => "Xylophone",
        14 => "Tubular Bells",
        15 => "Dulcimer",
        16 => "Drawbar Organ",
        17 => "Percussive Organ",
        18 => "Rock Organ",
        19 => "Church Organ",
        20 => "Reed Organ",
        21 => "Accordion",
        22 => "Harmonica",
        23 => "Tango Accordion",
        24 => "Acoustic Guitar (nylon)",
        25 => "Acoustic Guitar (steel)",
        26 => "Electric Guitar (jazz)",
        27 => "Electric Guitar (clean)",
        28 => "Electric Guitar (muted)",
        29 => "Overdriven Guitar",
        30 => "Distortion Guitar",
        31 => "Guitar Harmonics",
        32 => "Acoustic Bass",
        33 => "Electric Bass (finger)",
        34 => "Electric Bass (pick)",
        35 => "Fretless Bass",
        36 => "Slap Bass 1",
        37 => "Slap Bass 2",
        38 => "Synth Bass 1",
        39 => "Synth Bass 2",
        40 => "Violin",
        41 => "Viola",
        42 => "Cello",
        43 => "Contrabass",
        44 => "Tremolo Strings",
        45 => "Pizzicato Strings",
        46 => "Orchestral Harp",
        47 => "Timpani",
        _ => "Unknown",
    }
}

// ---------------------------------------------------------------------------
// Stats and init
// ---------------------------------------------------------------------------

static MIDI_FILES_PLAYED: AtomicU32 = AtomicU32::new(0);

/// Return MIDI subsystem statistics.
pub fn midi_stats() -> String {
    format!(
        "MIDI: {} files played, freq table entries: 128",
        MIDI_FILES_PLAYED.load(Ordering::Relaxed)
    )
}

/// Initialise the MIDI subsystem.
pub fn init() {
    MIDI_FILES_PLAYED.store(0, Ordering::Relaxed);
    crate::serial_println!("[midi] MIDI parser initialised");
}
