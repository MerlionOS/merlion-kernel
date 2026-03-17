/// Music player for MerlionOS.
/// Plays WAV/PCM audio through HDA, with playlist management,
/// playback controls, and visualization.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_PLAYLIST: usize = 64;
const MAX_LIBRARY: usize = 256;
const SAMPLE_RATE: u32 = 44100;
const FADE_SAMPLES: u32 = 4410; // 100ms at 44100 Hz

// ---------------------------------------------------------------------------
// Atomic state
// ---------------------------------------------------------------------------

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static VOLUME: AtomicU32 = AtomicU32::new(80);
static MUTED: AtomicBool = AtomicBool::new(false);
static PLAYING: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(false);
static REPEAT: AtomicBool = AtomicBool::new(false);
static SHUFFLE: AtomicBool = AtomicBool::new(false);
static TRACKS_PLAYED: AtomicU64 = AtomicU64::new(0);
static TOTAL_SAMPLES_SENT: AtomicU64 = AtomicU64::new(0);
static PEAK_LEVEL: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// WavHeader
// ---------------------------------------------------------------------------

/// Parsed WAV file header fields.
pub struct WavHeader {
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub data_size: u32,
}

impl WavHeader {
    /// Parse a WAV header from raw bytes. Returns None if invalid.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 44 { return None; }
        // "RIFF" check
        if data[0] != b'R' || data[1] != b'I' || data[2] != b'F' || data[3] != b'F' {
            return None;
        }
        // "WAVE" check
        if data[8] != b'W' || data[9] != b'A' || data[10] != b'V' || data[11] != b'E' {
            return None;
        }
        let channels = u16::from_le_bytes([data[22], data[23]]);
        let sample_rate = u32::from_le_bytes([data[24], data[25], data[26], data[27]]);
        let bits_per_sample = u16::from_le_bytes([data[34], data[35]]);
        let data_size = u32::from_le_bytes([data[40], data[41], data[42], data[43]]);
        Some(Self { channels, sample_rate, bits_per_sample, data_size })
    }

    /// Duration in milliseconds (integer math).
    pub fn duration_ms(&self) -> u32 {
        let bytes_per_sample = (self.bits_per_sample as u32) / 8;
        let total_samples = if bytes_per_sample > 0 && self.channels > 0 {
            self.data_size / (bytes_per_sample * self.channels as u32)
        } else {
            0
        };
        if self.sample_rate > 0 {
            (total_samples * 1000) / self.sample_rate
        } else {
            0
        }
    }
}

// ---------------------------------------------------------------------------
// Equalizer
// ---------------------------------------------------------------------------

/// Simple 3-band equalizer with integer scaling.
/// Each band ranges from -10 to +10.
pub struct Equalizer {
    pub bass: i32,   // -10..+10
    pub mid: i32,    // -10..+10
    pub treble: i32, // -10..+10
}

impl Equalizer {
    pub const fn new() -> Self {
        Self { bass: 0, mid: 0, treble: 0 }
    }

    /// Clamp a value to the valid range.
    fn clamp(val: i32) -> i32 {
        if val < -10 { -10 } else if val > 10 { 10 } else { val }
    }

    pub fn set_bass(&mut self, v: i32) { self.bass = Self::clamp(v); }
    pub fn set_mid(&mut self, v: i32) { self.mid = Self::clamp(v); }
    pub fn set_treble(&mut self, v: i32) { self.treble = Self::clamp(v); }

    /// Apply EQ to a sample. Very simplified: bass affects low amplitude,
    /// treble affects high amplitude, mid is overall gain tweak.
    /// Returns scaled sample using integer math (scale factor = 100 base).
    pub fn apply(&self, sample: i16) -> i16 {
        let s = sample as i32;
        // Combined gain: 100 + sum of adjustments * 5
        let gain = 100 + (self.bass + self.mid + self.treble) * 5;
        let result = (s * gain) / 100;
        // Clamp to i16 range
        if result > 32767 { 32767 }
        else if result < -32768 { -32768 }
        else { result as i16 }
    }
}

// ---------------------------------------------------------------------------
// Track
// ---------------------------------------------------------------------------

/// A track in the playlist.
#[derive(Clone)]
pub struct Track {
    pub path: String,
    pub name: String,
    pub artist: String,
    pub duration_ms: u32,
    pub size_bytes: u32,
}

impl Track {
    /// Create a track from a VFS path, extracting name/artist from filename.
    /// Expected format: "Artist - Title.wav" or just "Title.wav".
    pub fn from_path(path: &str) -> Self {
        let filename = path.rsplit('/').next().unwrap_or(path);
        let stem = if filename.ends_with(".wav") {
            &filename[..filename.len() - 4]
        } else {
            filename
        };
        let (artist, name) = if let Some(idx) = stem.find(" - ") {
            (String::from(&stem[..idx]), String::from(&stem[idx + 3..]))
        } else {
            (String::from("Unknown"), String::from(stem))
        };
        Self { path: String::from(path), name, artist, duration_ms: 0, size_bytes: 0 }
    }

    pub fn display(&self) -> String {
        format!("{} - {} ({}s)", self.artist, self.name, self.duration_ms / 1000)
    }
}

// ---------------------------------------------------------------------------
// PlayerState
// ---------------------------------------------------------------------------

struct PlayerState {
    playlist: Vec<Track>,
    current_index: usize,
    position_samples: u64,
    total_samples: u64,
    library: Vec<Track>,
    eq: Equalizer,
    fade_remaining: u32,
    fade_direction: bool, // true = fade in, false = fade out
}

impl PlayerState {
    const fn new() -> Self {
        Self {
            playlist: Vec::new(),
            current_index: 0,
            position_samples: 0,
            total_samples: 0,
            library: Vec::new(),
            eq: Equalizer::new(),
            fade_remaining: 0,
            fade_direction: true,
        }
    }
}

static STATE: Mutex<PlayerState> = Mutex::new(PlayerState::new());

// ---------------------------------------------------------------------------
// Volume helpers
// ---------------------------------------------------------------------------

/// Set volume (0-100).
pub fn set_volume(level: u32) {
    let clamped = if level > 100 { 100 } else { level };
    VOLUME.store(clamped, Ordering::Relaxed);
}

/// Get current volume.
pub fn get_volume() -> u32 {
    VOLUME.load(Ordering::Relaxed)
}

/// Toggle mute.
pub fn toggle_mute() {
    let prev = MUTED.load(Ordering::Relaxed);
    MUTED.store(!prev, Ordering::Relaxed);
}

/// Check if muted.
pub fn is_muted() -> bool {
    MUTED.load(Ordering::Relaxed)
}

/// Apply volume scaling to a sample (integer math).
fn apply_volume(sample: i16) -> i16 {
    if MUTED.load(Ordering::Relaxed) { return 0; }
    let vol = VOLUME.load(Ordering::Relaxed) as i32;
    ((sample as i32) * vol / 100) as i16
}

/// Apply fade to a sample given remaining fade samples.
fn apply_fade(sample: i16, remaining: u32, fade_in: bool) -> i16 {
    if remaining == 0 { return sample; }
    let progress = if fade_in {
        // fade in: ramp from 0 to full
        ((FADE_SAMPLES - remaining) as i32) * 100 / FADE_SAMPLES as i32
    } else {
        // fade out: ramp from full to 0
        (remaining as i32) * 100 / FADE_SAMPLES as i32
    };
    ((sample as i32) * progress / 100) as i16
}

// ---------------------------------------------------------------------------
// Playback controls
// ---------------------------------------------------------------------------

/// Load and play a WAV file from VFS path.
pub fn play(path: &str) {
    let data = match crate::vfs::cat(path) {
        Ok(d) => d,
        Err(_) => return,
    };
    let header = match WavHeader::parse(data.as_bytes()) {
        Some(h) => h,
        None => return,
    };
    let track = Track {
        path: String::from(path),
        name: Track::from_path(path).name,
        artist: Track::from_path(path).artist,
        duration_ms: header.duration_ms(),
        size_bytes: data.len() as u32,
    };
    let total = if header.bits_per_sample > 0 && header.channels > 0 {
        (header.data_size as u64) / ((header.bits_per_sample as u64 / 8) * header.channels as u64)
    } else {
        0
    };
    {
        let mut state = STATE.lock();
        state.current_index = state.playlist.len();
        state.playlist.push(track);
        state.position_samples = 0;
        state.total_samples = total;
        state.fade_remaining = FADE_SAMPLES;
        state.fade_direction = true;
    }
    PLAYING.store(true, Ordering::Relaxed);
    PAUSED.store(false, Ordering::Relaxed);
    TRACKS_PLAYED.fetch_add(1, Ordering::Relaxed);
}

/// Pause playback.
pub fn pause() {
    if PLAYING.load(Ordering::Relaxed) {
        PAUSED.store(true, Ordering::Relaxed);
    }
}

/// Resume playback.
pub fn resume() {
    PAUSED.store(false, Ordering::Relaxed);
}

/// Stop playback completely.
pub fn stop() {
    PLAYING.store(false, Ordering::Relaxed);
    PAUSED.store(false, Ordering::Relaxed);
    let mut state = STATE.lock();
    state.position_samples = 0;
}

/// Skip to next track.
pub fn next() {
    let mut state = STATE.lock();
    if state.playlist.is_empty() { return; }
    if SHUFFLE.load(Ordering::Relaxed) {
        // Simple pseudo-random: use current position as seed
        let seed = state.position_samples as usize;
        state.current_index = seed % state.playlist.len();
    } else {
        state.current_index = (state.current_index + 1) % state.playlist.len();
    }
    state.position_samples = 0;
    state.fade_remaining = FADE_SAMPLES;
    state.fade_direction = true;
    TRACKS_PLAYED.fetch_add(1, Ordering::Relaxed);
}

/// Skip to previous track.
pub fn prev() {
    let mut state = STATE.lock();
    if state.playlist.is_empty() { return; }
    if state.current_index == 0 {
        state.current_index = state.playlist.len() - 1;
    } else {
        state.current_index -= 1;
    }
    state.position_samples = 0;
    state.fade_remaining = FADE_SAMPLES;
    state.fade_direction = true;
}

/// Seek to a percentage (0-100).
pub fn seek(percent: u32) {
    let mut state = STATE.lock();
    let pct = if percent > 100 { 100 } else { percent };
    state.position_samples = state.total_samples * pct as u64 / 100;
}

/// Toggle repeat mode.
pub fn toggle_repeat() {
    let prev = REPEAT.load(Ordering::Relaxed);
    REPEAT.store(!prev, Ordering::Relaxed);
}

/// Toggle shuffle mode.
pub fn toggle_shuffle() {
    let prev = SHUFFLE.load(Ordering::Relaxed);
    SHUFFLE.store(!prev, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Playlist management
// ---------------------------------------------------------------------------

/// Add a track to the playlist.
pub fn playlist_add(path: &str) {
    let mut state = STATE.lock();
    if state.playlist.len() >= MAX_PLAYLIST { return; }
    state.playlist.push(Track::from_path(path));
}

/// Remove a track from the playlist by index.
pub fn playlist_remove(index: usize) {
    let mut state = STATE.lock();
    if index < state.playlist.len() {
        state.playlist.remove(index);
        if state.current_index >= state.playlist.len() && !state.playlist.is_empty() {
            state.current_index = state.playlist.len() - 1;
        }
    }
}

/// Show the current playlist.
pub fn playlist_show() -> String {
    let state = STATE.lock();
    if state.playlist.is_empty() {
        return String::from("Playlist is empty.");
    }
    let mut out = String::from("Playlist:\n");
    for (i, track) in state.playlist.iter().enumerate() {
        let marker = if i == state.current_index { ">" } else { " " };
        out.push_str(&format!("  {} {:2}. {}\n", marker, i + 1, track.display()));
    }
    out
}

// ---------------------------------------------------------------------------
// Now playing & VU meter
// ---------------------------------------------------------------------------

/// Get now-playing information.
pub fn now_playing() -> String {
    let state = STATE.lock();
    if !PLAYING.load(Ordering::Relaxed) || state.playlist.is_empty() {
        return String::from("Nothing playing.");
    }
    let track = &state.playlist[state.current_index];
    let progress = if state.total_samples > 0 {
        (state.position_samples * 100 / state.total_samples) as u32
    } else {
        0
    };
    let vol = VOLUME.load(Ordering::Relaxed);
    let muted_str = if MUTED.load(Ordering::Relaxed) { " [MUTED]" } else { "" };
    let paused_str = if PAUSED.load(Ordering::Relaxed) { " [PAUSED]" } else { "" };
    let repeat_str = if REPEAT.load(Ordering::Relaxed) { " [RPT]" } else { "" };
    let shuffle_str = if SHUFFLE.load(Ordering::Relaxed) { " [SHUF]" } else { "" };
    format!(
        "Now playing: {} - {}\nDuration: {}s | Progress: {}% | Vol: {}%{}{}{}{}\nTrack {}/{}",
        track.artist, track.name,
        track.duration_ms / 1000, progress, vol,
        muted_str, paused_str, repeat_str, shuffle_str,
        state.current_index + 1, state.playlist.len()
    )
}

/// Compute VU meter level from recent peak (0-16 bars).
pub fn vu_meter() -> String {
    let peak = PEAK_LEVEL.load(Ordering::Relaxed);
    // Scale 0-32767 to 0-16
    let bars = (peak as u64 * 16 / 32768) as usize;
    let mut out = String::from("[");
    for i in 0..16 {
        if i < bars { out.push('#'); } else { out.push('-'); }
    }
    out.push(']');
    out
}

/// Update peak level from a buffer of samples.
fn update_peak(samples: &[i16]) {
    let mut max: i32 = 0;
    for &s in samples {
        let abs = if s < 0 { -(s as i32) } else { s as i32 };
        if abs > max { max = abs; }
    }
    PEAK_LEVEL.store(max as u32, Ordering::Relaxed);
}

/// Process a buffer of samples: apply EQ, volume, fade, and update VU meter.
pub fn process_samples(samples: &mut [i16]) {
    let mut state = STATE.lock();
    for sample in samples.iter_mut() {
        *sample = state.eq.apply(*sample);
        *sample = apply_volume(*sample);
        if state.fade_remaining > 0 {
            *sample = apply_fade(*sample, state.fade_remaining, state.fade_direction);
            state.fade_remaining -= 1;
        }
    }
    let count = samples.len() as u64;
    state.position_samples += count;
    TOTAL_SAMPLES_SENT.fetch_add(count, Ordering::Relaxed);
    drop(state);
    update_peak(samples);
}

// ---------------------------------------------------------------------------
// Library
// ---------------------------------------------------------------------------

/// Scan a VFS directory for .wav files and build the library.
pub fn scan_library(dir_path: &str) {
    let entries = match crate::vfs::ls(dir_path) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut state = STATE.lock();
    state.library.clear();
    for (name, type_char) in entries {
        if type_char == 'f' && name.ends_with(".wav") {
            let full_path = if dir_path.ends_with('/') {
                format!("{}{}", dir_path, name)
            } else {
                format!("{}/{}", dir_path, name)
            };
            if state.library.len() < MAX_LIBRARY {
                state.library.push(Track::from_path(&full_path));
            }
        }
    }
}

/// List library tracks.
pub fn list_library() -> String {
    let state = STATE.lock();
    if state.library.is_empty() {
        return String::from("Library is empty. Use 'scan_library' to populate.");
    }
    let mut out = format!("Library ({} tracks):\n", state.library.len());
    for (i, track) in state.library.iter().enumerate() {
        out.push_str(&format!("  {:3}. {} - {}\n", i + 1, track.artist, track.name));
    }
    out
}

// ---------------------------------------------------------------------------
// Info & Stats
// ---------------------------------------------------------------------------

/// Player information summary.
pub fn player_info() -> String {
    let state = STATE.lock();
    format!(
        "Music Player v1.0\n\
         State: {}\n\
         Volume: {}%{}\n\
         Repeat: {} | Shuffle: {}\n\
         EQ: bass={} mid={} treble={}\n\
         Playlist: {} tracks | Library: {} tracks\n\
         Sample rate: {} Hz",
        if PLAYING.load(Ordering::Relaxed) {
            if PAUSED.load(Ordering::Relaxed) { "Paused" } else { "Playing" }
        } else { "Stopped" },
        VOLUME.load(Ordering::Relaxed),
        if MUTED.load(Ordering::Relaxed) { " [MUTED]" } else { "" },
        REPEAT.load(Ordering::Relaxed),
        SHUFFLE.load(Ordering::Relaxed),
        state.eq.bass, state.eq.mid, state.eq.treble,
        state.playlist.len(), state.library.len(),
        SAMPLE_RATE
    )
}

/// Player statistics.
pub fn player_stats() -> String {
    format!(
        "Music Player Stats:\n\
         Tracks played: {}\n\
         Total samples sent: {}\n\
         Peak level: {}\n\
         VU: {}",
        TRACKS_PLAYED.load(Ordering::Relaxed),
        TOTAL_SAMPLES_SENT.load(Ordering::Relaxed),
        PEAK_LEVEL.load(Ordering::Relaxed),
        vu_meter()
    )
}

/// Initialize the music player subsystem.
pub fn init() {
    INITIALIZED.store(true, Ordering::Relaxed);
    VOLUME.store(80, Ordering::Relaxed);
    crate::serial_println!("[ok] Music player initialized");
}
