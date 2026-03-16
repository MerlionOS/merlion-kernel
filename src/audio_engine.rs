/// Audio engine for MerlionOS.
/// Provides multi-channel audio mixing, WAV file playback, tone synthesis,
/// and an audio device abstraction layer. Uses i16 PCM samples at 44100 Hz.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SAMPLE_RATE: u32 = 44100;
const CHANNELS: usize = 2; // stereo
const BITS_PER_SAMPLE: u16 = 16;
const MAX_VOLUME: i32 = 256; // fixed-point scale (1.0 = 256)
const MAX_CHANNELS: usize = 8;

/// 256-entry sine lookup table: sin(i * 2*pi / 256) * 32767, pre-computed.
/// Values represent one full period of a sine wave scaled to i16 range.
static SINE_TABLE: [i16; 256] = [
    0, 804, 1608, 2410, 3212, 4011, 4808, 5602,
    6393, 7179, 7962, 8739, 9512, 10278, 11039, 11793,
    12539, 13279, 14010, 14732, 15446, 16151, 16846, 17530,
    18204, 18868, 19519, 20159, 20787, 21403, 22005, 22594,
    23170, 23731, 24279, 24811, 25329, 25832, 26319, 26790,
    27245, 27683, 28105, 28510, 28898, 29268, 29621, 29956,
    30273, 30571, 30852, 31113, 31356, 31580, 31785, 31971,
    32137, 32285, 32412, 32521, 32609, 32678, 32728, 32757,
    32767, 32757, 32728, 32678, 32609, 32521, 32412, 32285,
    32137, 31971, 31785, 31580, 31356, 31113, 30852, 30571,
    30273, 29956, 29621, 29268, 28898, 28510, 28105, 27683,
    27245, 26790, 26319, 25832, 25329, 24811, 24279, 23731,
    23170, 22594, 22005, 21403, 20787, 20159, 19519, 18868,
    18204, 17530, 16846, 16151, 15446, 14732, 14010, 13279,
    12539, 11793, 11039, 10278, 9512, 8739, 7962, 7179,
    6393, 5602, 4808, 4011, 3212, 2410, 1608, 804,
    0, -804, -1608, -2410, -3212, -4011, -4808, -5602,
    -6393, -7179, -7962, -8739, -9512, -10278, -11039, -11793,
    -12539, -13279, -14010, -14732, -15446, -16151, -16846, -17530,
    -18204, -18868, -19519, -20159, -20787, -21403, -22005, -22594,
    -23170, -23731, -24279, -24811, -25329, -25832, -26319, -26790,
    -27245, -27683, -28105, -28510, -28898, -29268, -29621, -29956,
    -30273, -30571, -30852, -31113, -31356, -31580, -31785, -31971,
    -32137, -32285, -32412, -32521, -32609, -32678, -32728, -32757,
    -32767, -32757, -32728, -32678, -32609, -32521, -32412, -32285,
    -32137, -31971, -31785, -31580, -31356, -31113, -30852, -30571,
    -30273, -29956, -29621, -29268, -28898, -28510, -28105, -27683,
    -27245, -26790, -26319, -25832, -25329, -24811, -24279, -23731,
    -23170, -22594, -22005, -21403, -20787, -20159, -19519, -18868,
    -18204, -17530, -16846, -16151, -15446, -14732, -14010, -13279,
    -12539, -11793, -11039, -10278, -9512, -8739, -7962, -7179,
    -6393, -5602, -4808, -4011, -3212, -2410, -1608, -804,
];

// ===========================================================================
// 1. Audio Format & Buffer
// ===========================================================================

/// A buffer of PCM audio samples.
pub struct AudioBuffer {
    /// Interleaved samples (L, R, L, R, ...) for stereo.
    pub samples: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
}

impl AudioBuffer {
    /// Create a new empty audio buffer.
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        BUFFERS_CREATED.fetch_add(1, Ordering::Relaxed);
        Self {
            samples: Vec::new(),
            sample_rate,
            channels,
            bits_per_sample: BITS_PER_SAMPLE,
        }
    }

    /// Duration of the buffer in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 || self.channels == 0 {
            return 0;
        }
        let total_frames = self.samples.len() as u64 / self.channels as u64;
        total_frames * 1000 / self.sample_rate as u64
    }

    /// Number of samples in the buffer.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Returns true if the buffer contains no samples.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Mix this buffer's samples into `dest` with the given volume (0..256).
    /// Clips to i16 range. Both buffers are assumed to have the same layout.
    pub fn mix_into(&self, dest: &mut [i16], volume: i32) {
        let vol = if volume > MAX_VOLUME { MAX_VOLUME } else if volume < 0 { 0 } else { volume };
        let count = core::cmp::min(self.samples.len(), dest.len());
        for i in 0..count {
            let scaled = (self.samples[i] as i32 * vol) >> 8; // divide by 256
            let mixed = dest[i] as i32 + scaled;
            // Clip to i16 range
            dest[i] = if mixed > 32767 {
                32767
            } else if mixed < -32768 {
                -32768
            } else {
                mixed as i16
            };
        }
    }
}

// ===========================================================================
// 2. Audio Channels / Mixer
// ===========================================================================

/// A single audio channel that can play one buffer at a time.
pub struct AudioChannel {
    pub id: u32,
    pub name: String,
    pub buffer: Option<AudioBuffer>,
    pub position: usize,
    pub volume: i32,   // 0..256 fixed-point
    pub pan: i32,      // -128 (full left) to +128 (full right), 0 = center
    pub playing: bool,
    pub looping: bool,
    pub muted: bool,
}

impl AudioChannel {
    fn new(id: u32, name: String) -> Self {
        Self {
            id,
            name,
            buffer: None,
            position: 0,
            volume: MAX_VOLUME,
            pan: 0,
            playing: false,
            looping: false,
            muted: false,
        }
    }
}

/// Multi-channel audio mixer. Combines up to MAX_CHANNELS sources into a
/// single stereo output buffer.
pub struct Mixer {
    channels: Vec<AudioChannel>,
    master_volume: i32,
    output_buffer: Vec<i16>,
    output_size: usize,
    next_id: u32,
}

impl Mixer {
    /// Create a new mixer with no channels.
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
            master_volume: MAX_VOLUME,
            output_buffer: Vec::new(),
            output_size: 0,
            next_id: 0,
        }
    }

    /// Create a new channel and return its id.
    pub fn create_channel(&mut self, name: &str) -> u32 {
        if self.channels.len() >= MAX_CHANNELS {
            // Reuse lowest non-playing channel or return last id
            for ch in self.channels.iter() {
                if !ch.playing {
                    return ch.id;
                }
            }
            return self.channels.last().map(|c| c.id).unwrap_or(0);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.channels.push(AudioChannel::new(id, name.to_owned()));
        id
    }

    /// Remove a channel by id.
    pub fn remove_channel(&mut self, id: u32) {
        self.channels.retain(|ch| ch.id != id);
    }

    /// Start playing a buffer on the given channel.
    pub fn play(&mut self, channel_id: u32, buffer: AudioBuffer, looping: bool) {
        if let Some(ch) = self.channels.iter_mut().find(|c| c.id == channel_id) {
            ch.buffer = Some(buffer);
            ch.position = 0;
            ch.playing = true;
            ch.looping = looping;
        }
    }

    /// Stop playback on a channel and reset position.
    pub fn stop(&mut self, channel_id: u32) {
        if let Some(ch) = self.channels.iter_mut().find(|c| c.id == channel_id) {
            ch.playing = false;
            ch.position = 0;
        }
    }

    /// Pause playback (keeps position).
    pub fn pause(&mut self, channel_id: u32) {
        if let Some(ch) = self.channels.iter_mut().find(|c| c.id == channel_id) {
            ch.playing = false;
        }
    }

    /// Set volume for a channel (0..256).
    pub fn set_volume(&mut self, channel_id: u32, vol: i32) {
        if let Some(ch) = self.channels.iter_mut().find(|c| c.id == channel_id) {
            ch.volume = vol.clamp(0, MAX_VOLUME);
        }
    }

    /// Set pan for a channel (-128..128).
    pub fn set_pan(&mut self, channel_id: u32, pan: i32) {
        if let Some(ch) = self.channels.iter_mut().find(|c| c.id == channel_id) {
            ch.pan = pan.clamp(-128, 128);
        }
    }

    /// Toggle mute on a channel.
    pub fn mute(&mut self, channel_id: u32) {
        if let Some(ch) = self.channels.iter_mut().find(|c| c.id == channel_id) {
            ch.muted = !ch.muted;
        }
    }

    /// Set the master volume (0..256).
    pub fn set_master_volume(&mut self, vol: i32) {
        self.master_volume = vol.clamp(0, MAX_VOLUME);
    }

    /// Mix `samples_count` stereo samples from all playing channels.
    /// Returns a Vec<i16> of interleaved L/R samples.
    pub fn mix(&mut self, samples_count: usize) -> Vec<i16> {
        let total = samples_count * CHANNELS;
        // Zero the output buffer
        self.output_buffer.clear();
        self.output_buffer.resize(total, 0i16);
        self.output_size = total;

        for ch in self.channels.iter_mut() {
            if !ch.playing || ch.muted {
                continue;
            }
            let buf = match &ch.buffer {
                Some(b) => b,
                None => continue,
            };
            if buf.is_empty() {
                ch.playing = false;
                continue;
            }

            // Compute per-channel left/right volume from pan + volume.
            // pan = -128 => left = 256, right = 0
            // pan = 0    => left = 128, right = 128
            // pan = +128 => left = 0,   right = 256
            let left_gain = ((128 - ch.pan) * ch.volume) >> 8;
            let right_gain = ((128 + ch.pan) * ch.volume) >> 8;

            let buf_len = buf.samples.len();
            let buf_channels = buf.channels as usize;

            for i in 0..samples_count {
                let frame_pos = ch.position + i * buf_channels;
                if frame_pos >= buf_len {
                    if ch.looping {
                        // Wrap handled below after the loop
                        break;
                    } else {
                        ch.playing = false;
                        break;
                    }
                }
                let left_sample = buf.samples[frame_pos] as i32;
                let right_sample = if buf_channels >= 2 && frame_pos + 1 < buf_len {
                    buf.samples[frame_pos + 1] as i32
                } else {
                    left_sample
                };

                let out_idx = i * CHANNELS;
                // Mix left channel
                let ml = (left_sample * left_gain) >> 7; // extra >>7 because gain is 0..128 range
                let mixed_l = self.output_buffer[out_idx] as i32 + ml;
                self.output_buffer[out_idx] = mixed_l.clamp(-32768, 32767) as i16;
                // Mix right channel
                let mr = (right_sample * right_gain) >> 7;
                let mixed_r = self.output_buffer[out_idx + 1] as i32 + mr;
                self.output_buffer[out_idx + 1] = mixed_r.clamp(-32768, 32767) as i16;
            }

            // Advance position
            ch.position += samples_count * buf_channels;
            if ch.position >= buf_len {
                if ch.looping {
                    ch.position %= buf_len;
                } else {
                    ch.playing = false;
                    ch.position = 0;
                }
            }
        }

        // Apply master volume
        let mv = self.master_volume;
        for sample in self.output_buffer.iter_mut() {
            let s = (*sample as i32 * mv) >> 8;
            *sample = s.clamp(-32768, 32767) as i16;
        }

        SAMPLES_MIXED.fetch_add(total as u64, Ordering::Relaxed);
        self.output_buffer.clone()
    }

    /// Return a human-readable list of all channels and their state.
    pub fn list_channels(&self) -> String {
        let mut s = String::from("Audio Channels:\n");
        if self.channels.is_empty() {
            s.push_str("  (none)\n");
            return s;
        }
        for ch in &self.channels {
            let state = if ch.muted {
                "muted"
            } else if ch.playing {
                "playing"
            } else {
                "stopped"
            };
            let has_buf = ch.buffer.is_some();
            s.push_str(&format!(
                "  [{}] \"{}\" vol={} pan={} {} loop={} buf={}\n",
                ch.id, ch.name, ch.volume, ch.pan, state, ch.looping, has_buf
            ));
        }
        s
    }
}

// ===========================================================================
// 3. WAV Parser
// ===========================================================================

/// Read a little-endian u16 from a byte slice.
fn read_u16_le(data: &[u8], offset: usize) -> u16 {
    (data[offset] as u16) | ((data[offset + 1] as u16) << 8)
}

/// Read a little-endian u32 from a byte slice.
fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    (data[offset] as u32)
        | ((data[offset + 1] as u32) << 8)
        | ((data[offset + 2] as u32) << 16)
        | ((data[offset + 3] as u32) << 24)
}

/// Parse a WAV file from raw bytes.
/// Supports PCM format (format code 1), 8-bit or 16-bit, mono or stereo.
pub fn parse_wav(data: &[u8]) -> Result<AudioBuffer, &'static str> {
    if data.len() < 44 {
        return Err("WAV: file too short");
    }
    // RIFF header
    if &data[0..4] != b"RIFF" {
        return Err("WAV: missing RIFF header");
    }
    if &data[8..12] != b"WAVE" {
        return Err("WAV: not a WAVE file");
    }

    // Find fmt chunk
    let mut pos = 12;
    let mut fmt_found = false;
    let mut audio_format: u16 = 0;
    let mut num_channels: u16 = 0;
    let mut sample_rate: u32 = 0;
    let mut bits: u16 = 0;

    while pos + 8 <= data.len() {
        let chunk_id = &data[pos..pos + 4];
        let chunk_size = read_u32_le(data, pos + 4) as usize;

        if chunk_id == b"fmt " {
            if chunk_size < 16 || pos + 8 + 16 > data.len() {
                return Err("WAV: fmt chunk too small");
            }
            audio_format = read_u16_le(data, pos + 8);
            num_channels = read_u16_le(data, pos + 10);
            sample_rate = read_u32_le(data, pos + 12);
            // skip byte rate (pos+16) and block align (pos+20)
            bits = read_u16_le(data, pos + 22);
            fmt_found = true;
        }

        if chunk_id == b"data" && fmt_found {
            if audio_format != 1 {
                return Err("WAV: only PCM format supported");
            }
            if num_channels == 0 || num_channels > 2 {
                return Err("WAV: only mono/stereo supported");
            }
            if bits != 8 && bits != 16 {
                return Err("WAV: only 8/16-bit supported");
            }

            let data_start = pos + 8;
            let data_end = core::cmp::min(data_start + chunk_size, data.len());
            let raw = &data[data_start..data_end];

            let mut buf = AudioBuffer::new(sample_rate, num_channels);
            buf.bits_per_sample = bits;

            if bits == 16 {
                // 16-bit signed PCM
                let sample_count = raw.len() / 2;
                buf.samples.reserve(sample_count);
                for i in 0..sample_count {
                    let s = read_u16_le(raw, i * 2) as i16;
                    buf.samples.push(s);
                }
            } else {
                // 8-bit unsigned PCM -> convert to i16
                buf.samples.reserve(raw.len());
                for &byte in raw {
                    let s = ((byte as i16) - 128) * 256; // scale 0..255 -> -32768..32512
                    buf.samples.push(s);
                }
            }

            return Ok(buf);
        }

        pos += 8 + chunk_size;
        // Chunks are word-aligned
        if chunk_size % 2 != 0 {
            pos += 1;
        }
    }

    Err("WAV: data chunk not found")
}

/// Display metadata about a WAV file.
pub fn wav_info(data: &[u8]) -> String {
    match parse_wav(data) {
        Ok(buf) => format!(
            "WAV: {} Hz, {}-bit, {} ch, {} samples, {} ms",
            buf.sample_rate,
            buf.bits_per_sample,
            buf.channels,
            buf.samples.len(),
            buf.duration_ms()
        ),
        Err(e) => format!("WAV error: {}", e),
    }
}

// ===========================================================================
// 4. Tone Synthesis
// ===========================================================================

/// Generate a mono audio buffer with the given number of samples at SAMPLE_RATE.
fn make_mono_buffer(num_samples: u32) -> AudioBuffer {
    let mut buf = AudioBuffer::new(SAMPLE_RATE, 1);
    buf.samples.resize(num_samples as usize, 0);
    buf
}

/// Compute total number of samples for a given duration.
fn duration_to_samples(duration_ms: u32) -> u32 {
    (SAMPLE_RATE / 1000) * duration_ms
}

/// Generate a sine wave approximation using the integer lookup table.
pub fn generate_sine(freq_hz: u32, duration_ms: u32, amplitude: i16) -> AudioBuffer {
    let num_samples = duration_to_samples(duration_ms);
    let mut buf = make_mono_buffer(num_samples);

    if freq_hz == 0 {
        return buf;
    }

    // Phase accumulator: we step through the 256-entry table.
    // step_fp = freq_hz * 256 * 65536 / SAMPLE_RATE (16.16 fixed point)
    let step_fp: u64 = (freq_hz as u64 * 256 * 65536) / SAMPLE_RATE as u64;
    let mut phase_fp: u64 = 0;

    for i in 0..num_samples as usize {
        let table_idx = ((phase_fp >> 16) & 0xFF) as usize;
        let sine_val = SINE_TABLE[table_idx] as i32;
        let sample = (sine_val * amplitude as i32) / 32767;
        buf.samples[i] = sample as i16;
        phase_fp += step_fp;
    }

    buf
}

/// Generate a square wave.
pub fn generate_square(freq_hz: u32, duration_ms: u32, amplitude: i16) -> AudioBuffer {
    let num_samples = duration_to_samples(duration_ms);
    let mut buf = make_mono_buffer(num_samples);

    if freq_hz == 0 {
        return buf;
    }

    // Half-period in samples (fixed-point 16.16)
    let period_samples_fp: u64 = (SAMPLE_RATE as u64 * 65536) / freq_hz as u64;
    let half_period_fp = period_samples_fp / 2;
    let mut phase_fp: u64 = 0;

    for i in 0..num_samples as usize {
        let in_first_half = (phase_fp % period_samples_fp) < half_period_fp;
        buf.samples[i] = if in_first_half { amplitude } else { -amplitude };
        phase_fp += 65536;
    }

    buf
}

/// Generate a sawtooth wave.
pub fn generate_sawtooth(freq_hz: u32, duration_ms: u32, amplitude: i16) -> AudioBuffer {
    let num_samples = duration_to_samples(duration_ms);
    let mut buf = make_mono_buffer(num_samples);

    if freq_hz == 0 {
        return buf;
    }

    let period_samples_fp: u64 = (SAMPLE_RATE as u64 * 65536) / freq_hz as u64;

    let mut phase_fp: u64 = 0;
    for i in 0..num_samples as usize {
        let pos_in_period = phase_fp % period_samples_fp;
        // Map position to -amplitude..+amplitude using fixed-point
        let sample = ((pos_in_period as i64 * 2 * amplitude as i64) / period_samples_fp as i64)
            - amplitude as i64;
        buf.samples[i] = sample.clamp(-32768, 32767) as i16;
        phase_fp += 65536;
    }

    buf
}

/// Generate a triangle wave.
pub fn generate_triangle(freq_hz: u32, duration_ms: u32, amplitude: i16) -> AudioBuffer {
    let num_samples = duration_to_samples(duration_ms);
    let mut buf = make_mono_buffer(num_samples);

    if freq_hz == 0 {
        return buf;
    }

    let period_samples_fp: u64 = (SAMPLE_RATE as u64 * 65536) / freq_hz as u64;
    let half = period_samples_fp / 2;

    let mut phase_fp: u64 = 0;
    for i in 0..num_samples as usize {
        let pos = phase_fp % period_samples_fp;
        let sample = if pos < half {
            // Rising: -amplitude to +amplitude
            ((pos as i64 * 2 * amplitude as i64) / half as i64) - amplitude as i64
        } else {
            // Falling: +amplitude to -amplitude
            amplitude as i64
                - (((pos - half) as i64 * 2 * amplitude as i64) / half as i64)
        };
        buf.samples[i] = sample.clamp(-32768, 32767) as i16;
        phase_fp += 65536;
    }

    buf
}

/// Generate white noise using a linear congruential PRNG.
pub fn generate_noise(duration_ms: u32, amplitude: i16) -> AudioBuffer {
    let num_samples = duration_to_samples(duration_ms);
    let mut buf = make_mono_buffer(num_samples);

    let mut seed: u32 = 0xDEAD_BEEF;
    for i in 0..num_samples as usize {
        // LCG: seed = seed * 1103515245 + 12345
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        // Map upper bits to -amplitude..+amplitude
        let raw = (seed >> 16) as i16; // pseudo-random i16
        let sample = (raw as i32 * amplitude as i32) / 32767;
        buf.samples[i] = sample as i16;
    }

    buf
}

// ===========================================================================
// 5. Music Sequencer
// ===========================================================================

/// A single musical note with frequency, duration, and velocity.
pub struct Note {
    pub freq_hz: u32,
    pub duration_ms: u32,
    pub velocity: i16, // 0..127
}

impl Note {
    pub fn new(freq_hz: u32, duration_ms: u32, velocity: i16) -> Self {
        Self {
            freq_hz,
            duration_ms,
            velocity: velocity.clamp(0, 127),
        }
    }

    /// Create a rest (silence) of the given duration.
    pub fn rest(duration_ms: u32) -> Self {
        Self {
            freq_hz: 0,
            duration_ms,
            velocity: 0,
        }
    }
}

/// Convert a note name like "C4", "A4", "G#5", "Bb3" to its frequency in Hz.
/// Uses a lookup table for the 4th octave and shifts for other octaves.
/// Returns 0 for unrecognized notes.
pub fn note_freq(note: &str) -> u32 {
    let bytes = note.as_bytes();
    if bytes.is_empty() {
        return 0;
    }

    // Base frequencies for octave 4 (C4 through B4), multiplied by 100 for
    // fixed-point precision (avoids floating point).
    // C4=26163, C#4=27718, D4=29366, D#4=31113, E4=32963,
    // F4=34923, F#4=36999, G4=39200, G#4=41530, A4=44000,
    // A#4=46616, B4=49388
    static BASE_FREQ_X100: [u32; 12] = [
        26163, 27718, 29366, 31113, 32963, 34923,
        36999, 39200, 41530, 44000, 46616, 49388,
    ];

    let mut idx = 0;
    let base_note = match bytes[0] {
        b'C' => 0,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return 0,
    };
    idx += 1;

    let mut semitone = base_note as i32;

    // Check for sharp/flat
    if idx < bytes.len() && bytes[idx] == b'#' {
        semitone += 1;
        idx += 1;
    } else if idx < bytes.len() && bytes[idx] == b'b' {
        semitone -= 1;
        idx += 1;
    }

    // Wrap semitone
    if semitone < 0 {
        semitone += 12;
    }
    let semitone = (semitone % 12) as usize;

    // Parse octave number
    if idx >= bytes.len() || bytes[idx] < b'0' || bytes[idx] > b'9' {
        return 0;
    }
    let octave = (bytes[idx] - b'0') as i32;

    let freq_x100 = BASE_FREQ_X100[semitone];

    // Shift octave relative to 4
    let shift = octave - 4;
    let freq = if shift >= 0 {
        freq_x100 << (shift as u32)
    } else {
        freq_x100 >> ((-shift) as u32)
    };

    // Round and convert from x100
    (freq + 50) / 100
}

/// Render a sequence of notes into a single audio buffer using sine synthesis.
pub fn play_sequence(notes: &[Note]) -> AudioBuffer {
    let mut total_samples: usize = 0;
    for n in notes {
        total_samples += duration_to_samples(n.duration_ms) as usize;
    }

    let mut result = AudioBuffer::new(SAMPLE_RATE, 1);
    result.samples.reserve(total_samples);

    for n in notes {
        let amp = ((n.velocity as i32) * 24000 / 127) as i16; // scale velocity to amplitude
        let tone = generate_sine(n.freq_hz, n.duration_ms, amp);
        result.samples.extend_from_slice(&tone.samples);
    }

    result
}

/// Boot startup jingle: ascending C-E-G-C5 arpeggio.
pub fn melody_startup() -> Vec<Note> {
    alloc::vec![
        Note::new(note_freq("C4"), 120, 100),
        Note::new(note_freq("E4"), 120, 100),
        Note::new(note_freq("G4"), 120, 110),
        Note::new(note_freq("C5"), 200, 120),
    ]
}

/// Alert sound: sharp A4-rest-C4 descending.
pub fn melody_alert() -> Vec<Note> {
    alloc::vec![
        Note::new(note_freq("A4"), 150, 127),
        Note::rest(50),
        Note::new(note_freq("C4"), 300, 110),
    ]
}

/// Success chime: quick E4-C5 rising.
pub fn melody_success() -> Vec<Note> {
    alloc::vec![
        Note::new(note_freq("E4"), 100, 100),
        Note::new(note_freq("C5"), 200, 120),
    ]
}

// ===========================================================================
// 6. Audio Device Abstraction
// ===========================================================================

/// Supported audio output backends.
pub enum AudioBackend {
    /// No audio hardware detected.
    None,
    /// Legacy PC speaker (beep only, no PCM).
    PcSpeaker,
    /// Intel AC'97 codec.
    AC97,
    /// Intel High Definition Audio.
    HDA,
    /// Software-only buffer (no hardware output).
    Software,
}

impl AudioBackend {
    fn name(&self) -> &'static str {
        match self {
            AudioBackend::None => "None",
            AudioBackend::PcSpeaker => "PC Speaker",
            AudioBackend::AC97 => "AC97",
            AudioBackend::HDA => "HDA",
            AudioBackend::Software => "Software",
        }
    }
}

/// Detect the available audio backend.
/// Currently defaults to Software since we don't probe PCI for AC97/HDA yet.
fn detect_backend() -> AudioBackend {
    // TODO: PCI enumeration to detect AC97 (vendor 0x8086, device 0x2415) or
    //       HDA (class 0x0403). For now, assume software mixing.
    AudioBackend::Software
}

// ===========================================================================
// 7. Global State & API
// ===========================================================================

static AUDIO: Mutex<Option<Mixer>> = Mutex::new(None);
static AUDIO_BACKEND: Mutex<AudioBackend> = Mutex::new(AudioBackend::None);
static SAMPLES_MIXED: AtomicU64 = AtomicU64::new(0);
static BUFFERS_CREATED: AtomicU32 = AtomicU32::new(0);
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Initialize the audio engine: detect backend, create mixer, set up default
/// channels.
pub fn init() {
    let backend = detect_backend();
    let mut mixer = Mixer::new();
    // Create a default channel for system sounds.
    mixer.create_channel("system");
    mixer.create_channel("music");
    mixer.create_channel("sfx");

    *AUDIO_BACKEND.lock() = backend;
    *AUDIO.lock() = Some(mixer);
    INITIALIZED.store(true, Ordering::SeqCst);

    crate::serial_println!("[audio_engine] initialized (backend: {})", AUDIO_BACKEND.lock().name());
}

/// Return information about the audio engine configuration.
pub fn audio_info() -> String {
    let backend_name = AUDIO_BACKEND.lock().name();
    let master_vol = AUDIO
        .lock()
        .as_ref()
        .map(|m| m.master_volume)
        .unwrap_or(0);

    format!(
        "Audio Engine:\n  Backend: {}\n  Sample rate: {} Hz\n  Channels: {} (stereo)\n  Bits/sample: {}\n  Master volume: {}/{}",
        backend_name, SAMPLE_RATE, CHANNELS, BITS_PER_SAMPLE, master_vol, MAX_VOLUME
    )
}

/// Return runtime statistics.
pub fn audio_stats() -> String {
    let samples = SAMPLES_MIXED.load(Ordering::Relaxed);
    let buffers = BUFFERS_CREATED.load(Ordering::Relaxed);
    let active = AUDIO
        .lock()
        .as_ref()
        .map(|m| m.channels.iter().filter(|c| c.playing).count())
        .unwrap_or(0);
    let total_ch = AUDIO
        .lock()
        .as_ref()
        .map(|m| m.channels.len())
        .unwrap_or(0);

    format!(
        "Audio Stats:\n  Samples mixed: {}\n  Buffers created: {}\n  Active channels: {}/{}\n  Initialized: {}",
        samples, buffers, active, total_ch, INITIALIZED.load(Ordering::Relaxed)
    )
}

/// Return a human-readable list of all channels and their state.
pub fn list_channels() -> String {
    let lock = AUDIO.lock();
    match lock.as_ref() {
        Some(mixer) => mixer.list_channels(),
        None => String::from("Audio not initialized"),
    }
}

/// Convenience function: generate a sine tone and play it on channel 0 ("system").
pub fn play_tone(freq: u32, duration_ms: u32) {
    let buf = generate_sine(freq, duration_ms, 24000);
    let mut lock = AUDIO.lock();
    if let Some(mixer) = lock.as_mut() {
        // Channel 0 is "system"
        if let Some(ch) = mixer.channels.first() {
            let id = ch.id;
            mixer.play(id, buf, false);
        }
    }
}

/// Load a WAV file from the VFS and play it on the "music" channel.
pub fn play_wav_file(path: &str) -> Result<(), &'static str> {
    // Read the file contents from VFS
    let contents = crate::vfs::cat(path).map_err(|_| "failed to read WAV file from VFS")?;
    let data = contents.as_bytes();
    let buf = parse_wav(data)?;

    let mut lock = AUDIO.lock();
    if let Some(mixer) = lock.as_mut() {
        // Find or use the "music" channel (id 1)
        let music_id = mixer
            .channels
            .iter()
            .find(|c| c.name == "music")
            .map(|c| c.id)
            .unwrap_or(1);
        mixer.play(music_id, buf, false);
        Ok(())
    } else {
        Err("audio engine not initialized")
    }
}

/// Demo function: play the startup melody and return engine info.
pub fn demo() -> String {
    if !INITIALIZED.load(Ordering::Relaxed) {
        init();
    }

    // Generate the startup melody
    let notes = melody_startup();
    let melody_buf = play_sequence(&notes);
    let duration = melody_buf.duration_ms();

    // Play it on the system channel
    {
        let mut lock = AUDIO.lock();
        if let Some(mixer) = lock.as_mut() {
            if let Some(ch) = mixer.channels.first() {
                let id = ch.id;
                mixer.play(id, melody_buf, false);
            }
            // Mix a chunk to "render" the audio
            let chunk_size = (SAMPLE_RATE / 10) as usize; // 100ms worth
            let _ = mixer.mix(chunk_size);
        }
    }

    let info = audio_info();
    let stats = audio_stats();

    format!(
        "{}\n{}\n\nPlayed startup melody ({} ms, {} notes)",
        info,
        stats,
        duration,
        notes.len()
    )
}
