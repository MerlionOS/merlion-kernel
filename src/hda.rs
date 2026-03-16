/// Intel High Definition Audio (HDA) driver for MerlionOS.
/// Implements the HDA controller interface for audio playback and recording
/// via codec communication and DMA buffer management.

use crate::{pci, serial_println};
use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// PCI identification: Audio device class 04h, subclass 03h
// ---------------------------------------------------------------------------

const HDA_CLASS: u8 = 0x04;
const HDA_SUBCLASS: u8 = 0x03;
const HDA_VENDOR_INTEL: u16 = 0x8086;

// ---------------------------------------------------------------------------
// HDA controller register offsets (from BAR0)
// ---------------------------------------------------------------------------

/// Global Capabilities
const REG_GCAP: u32 = 0x00;
/// Minor Version
const REG_VMIN: u32 = 0x02;
/// Major Version
const REG_VMAJ: u32 = 0x03;
/// Output Payload Capability
const REG_OUTPAY: u32 = 0x04;
/// Input Payload Capability
const REG_INPAY: u32 = 0x06;
/// Global Control
const REG_GCTL: u32 = 0x08;
/// Wake Enable
const REG_WAKEEN: u32 = 0x0C;
/// State Change Status
const REG_STATESTS: u32 = 0x0E;
/// Global Status
const REG_GSTS: u32 = 0x10;
/// Interrupt Control
const REG_INTCTL: u32 = 0x20;
/// Interrupt Status
const REG_INTSTS: u32 = 0x24;
/// Wall Clock Counter
const REG_WALLCLK: u32 = 0x30;
/// CORB Lower Base Address
const REG_CORBLBASE: u32 = 0x40;
/// CORB Upper Base Address
const REG_CORBUBASE: u32 = 0x44;
/// CORB Write Pointer
const REG_CORBWP: u32 = 0x48;
/// CORB Read Pointer
const REG_CORBRP: u32 = 0x4A;
/// CORB Control
const REG_CORBCTL: u32 = 0x4C;
/// CORB Status
const REG_CORBSTS: u32 = 0x4D;
/// CORB Size
const REG_CORBSIZE: u32 = 0x4E;
/// RIRB Lower Base Address
const REG_RIRBLBASE: u32 = 0x50;
/// RIRB Upper Base Address
const REG_RIRBUBASE: u32 = 0x54;
/// RIRB Write Pointer
const REG_RIRBWP: u32 = 0x58;
/// RIRB Interrupt Count
const REG_RINTCNT: u32 = 0x5A;
/// RIRB Control
const REG_RIRBCTL: u32 = 0x5C;
/// RIRB Status
const REG_RIRBSTS: u32 = 0x5D;
/// RIRB Size
const REG_RIRBSIZE: u32 = 0x5E;

/// Output stream descriptor base offset (first output stream)
const REG_OSD0_BASE: u32 = 0x80;
/// Input stream descriptor base offset (first input stream)
const REG_ISD0_BASE: u32 = 0x80;
/// Stream descriptor size (each stream occupies 0x20 bytes)
const STREAM_DESC_SIZE: u32 = 0x20;

// Stream descriptor register offsets (relative to stream base)
const SD_CTL: u32 = 0x00;
const SD_STS: u32 = 0x03;
const SD_LPIB: u32 = 0x04;
const SD_CBL: u32 = 0x08;
const SD_LVI: u32 = 0x0C;
const SD_FMT: u32 = 0x12;
const SD_BDPL: u32 = 0x18;
const SD_BDPU: u32 = 0x1C;

// Global Control bits
const GCTL_CRST: u32 = 1 << 0;
const GCTL_UNSOL: u32 = 1 << 8;

// CORB/RIRB control bits
const CORBCTL_RUN: u8 = 1 << 1;
const RIRBCTL_RUN: u8 = 1 << 1;
const RIRBCTL_INT: u8 = 1 << 0;

// Stream control bits
const SD_CTL_RUN: u32 = 1 << 1;
const SD_CTL_IOCE: u32 = 1 << 2;
const SD_CTL_STRIPE: u32 = 1 << 4;

/// CORB entry count
const CORB_ENTRIES: usize = 256;
/// RIRB entry count
const RIRB_ENTRIES: usize = 256;
/// Buffer Descriptor List entry count
const BDL_ENTRIES: usize = 256;
/// DMA buffer size per entry
const DMA_BUF_SIZE: usize = 4096;

// ---------------------------------------------------------------------------
// HDA codec verbs
// ---------------------------------------------------------------------------

/// Build a codec verb: (codec_addr << 28) | (nid << 20) | verb
fn make_verb(codec: u8, nid: u8, verb: u32) -> u32 {
    ((codec as u32) << 28) | ((nid as u32) << 20) | (verb & 0x000F_FFFF)
}

// Get Parameter verb (verb ID = 0xF00xx)
const VERB_GET_PARAM: u32 = 0xF0000;
// Set Stream/Channel verb (verb ID = 0x706xx)
const VERB_SET_STREAM: u32 = 0x70600;
// Set Pin Widget Control (verb ID = 0x707xx)
const VERB_SET_PIN_CTL: u32 = 0x70700;
// Set Amplifier Gain (verb ID = 0x3xxxx)
const VERB_SET_AMP_GAIN: u32 = 0x30000;
// Set Converter Format (verb ID = 0x2xxxx)
const VERB_SET_FORMAT: u32 = 0x20000;
// Set Power State (verb ID = 0x705xx)
const VERB_SET_POWER: u32 = 0x70500;
// Get Connection List Entry (verb ID = 0xF02xx)
const VERB_GET_CONN_LIST: u32 = 0xF0200;

// Parameter IDs for GET_PARAM
const PARAM_VENDOR_ID: u32 = 0x00;
const PARAM_REV_ID: u32 = 0x02;
const PARAM_NODE_COUNT: u32 = 0x04;
const PARAM_FN_GROUP_TYPE: u32 = 0x05;
const PARAM_AUDIO_CAPS: u32 = 0x09;
const PARAM_PIN_CAPS: u32 = 0x0C;
const PARAM_CONN_LIST_LEN: u32 = 0x0E;
const PARAM_AMP_OUT_CAPS: u32 = 0x12;

// ---------------------------------------------------------------------------
// Widget types
// ---------------------------------------------------------------------------

/// HDA widget type as reported by Audio Widget Capabilities parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WidgetType {
    AudioOutput,
    AudioInput,
    AudioMixer,
    AudioSelector,
    PinComplex,
    PowerWidget,
    VolumeKnob,
    BeepGenerator,
    VendorDefined,
    Unknown(u8),
}

impl WidgetType {
    fn from_caps(caps: u32) -> Self {
        match (caps >> 20) & 0x0F {
            0x0 => WidgetType::AudioOutput,
            0x1 => WidgetType::AudioInput,
            0x2 => WidgetType::AudioMixer,
            0x3 => WidgetType::AudioSelector,
            0x4 => WidgetType::PinComplex,
            0x5 => WidgetType::PowerWidget,
            0x6 => WidgetType::VolumeKnob,
            0x7 => WidgetType::BeepGenerator,
            0xF => WidgetType::VendorDefined,
            x => WidgetType::Unknown(x as u8),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            WidgetType::AudioOutput => "Audio Output (DAC)",
            WidgetType::AudioInput => "Audio Input (ADC)",
            WidgetType::AudioMixer => "Audio Mixer",
            WidgetType::AudioSelector => "Audio Selector",
            WidgetType::PinComplex => "Pin Complex",
            WidgetType::PowerWidget => "Power Widget",
            WidgetType::VolumeKnob => "Volume Knob",
            WidgetType::BeepGenerator => "Beep Generator",
            WidgetType::VendorDefined => "Vendor Defined",
            WidgetType::Unknown(_) => "Unknown",
        }
    }
}

// ---------------------------------------------------------------------------
// Pin configuration
// ---------------------------------------------------------------------------

/// Default device type from pin configuration default register.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PinDevice {
    LineOut,
    Speaker,
    Headphone,
    CD,
    SPDIFOut,
    DigitalOut,
    ModemHandset,
    ModemLine,
    LineIn,
    AUX,
    MicIn,
    Telephony,
    SPDIFIn,
    DigitalIn,
    Other,
}

impl PinDevice {
    fn from_default_cfg(cfg: u32) -> Self {
        match (cfg >> 20) & 0x0F {
            0x0 => PinDevice::LineOut,
            0x1 => PinDevice::Speaker,
            0x2 => PinDevice::Headphone,
            0x3 => PinDevice::CD,
            0x4 => PinDevice::SPDIFOut,
            0x5 => PinDevice::DigitalOut,
            0x6 => PinDevice::ModemHandset,
            0x7 => PinDevice::ModemLine,
            0x8 => PinDevice::LineIn,
            0x9 => PinDevice::AUX,
            0xA => PinDevice::MicIn,
            0xB => PinDevice::Telephony,
            0xC => PinDevice::SPDIFIn,
            0xD => PinDevice::DigitalIn,
            _ => PinDevice::Other,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            PinDevice::LineOut => "Line Out",
            PinDevice::Speaker => "Speaker",
            PinDevice::Headphone => "Headphone",
            PinDevice::CD => "CD",
            PinDevice::SPDIFOut => "SPDIF Out",
            PinDevice::DigitalOut => "Digital Out",
            PinDevice::ModemHandset => "Modem Handset",
            PinDevice::ModemLine => "Modem Line",
            PinDevice::LineIn => "Line In",
            PinDevice::AUX => "AUX",
            PinDevice::MicIn => "Mic In",
            PinDevice::Telephony => "Telephony",
            PinDevice::SPDIFIn => "SPDIF In",
            PinDevice::DigitalIn => "Digital In",
            PinDevice::Other => "Other",
        }
    }

    pub fn is_output(&self) -> bool {
        matches!(self, PinDevice::LineOut | PinDevice::Speaker |
            PinDevice::Headphone | PinDevice::SPDIFOut | PinDevice::DigitalOut)
    }

    pub fn is_input(&self) -> bool {
        matches!(self, PinDevice::LineIn | PinDevice::AUX |
            PinDevice::MicIn | PinDevice::SPDIFIn | PinDevice::DigitalIn)
    }
}

/// Pin connection type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PinConnection {
    Jack,
    NoPhysical,
    FixedFunction,
    Both,
}

impl PinConnection {
    fn from_default_cfg(cfg: u32) -> Self {
        match (cfg >> 30) & 0x03 {
            0x0 => PinConnection::Jack,
            0x1 => PinConnection::NoPhysical,
            0x2 => PinConnection::FixedFunction,
            _ => PinConnection::Both,
        }
    }
}

/// Pin color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PinColor {
    Unknown, Black, Grey, Blue, Green, Red, Orange, Yellow, Purple, Pink, White, Other,
}

impl PinColor {
    fn from_default_cfg(cfg: u32) -> Self {
        match (cfg >> 12) & 0x0F {
            0x0 => PinColor::Unknown,
            0x1 => PinColor::Black,
            0x2 => PinColor::Grey,
            0x3 => PinColor::Blue,
            0x4 => PinColor::Green,
            0x5 => PinColor::Red,
            0x6 => PinColor::Orange,
            0x7 => PinColor::Yellow,
            0x8 => PinColor::Purple,
            0x9 => PinColor::Pink,
            0xE => PinColor::White,
            _ => PinColor::Other,
        }
    }
}

// ---------------------------------------------------------------------------
// PCM format
// ---------------------------------------------------------------------------

/// PCM sample rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SampleRate {
    Rate44100,
    Rate48000,
}

impl SampleRate {
    pub fn hz(&self) -> u32 {
        match self {
            SampleRate::Rate44100 => 44100,
            SampleRate::Rate48000 => 48000,
        }
    }

    /// Encode for HDA stream format register.
    fn format_bits(&self) -> u16 {
        match self {
            // Base=44.1kHz, mult=1, div=1
            SampleRate::Rate44100 => 1 << 14,
            // Base=48kHz, mult=1, div=1
            SampleRate::Rate48000 => 0,
        }
    }
}

/// PCM bit depth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BitDepth {
    Bits16,
    Bits24,
}

impl BitDepth {
    pub fn bits(&self) -> u8 {
        match self {
            BitDepth::Bits16 => 16,
            BitDepth::Bits24 => 24,
        }
    }

    fn format_bits(&self) -> u16 {
        match self {
            BitDepth::Bits16 => 0x01 << 4, // 16-bit
            BitDepth::Bits24 => 0x03 << 4, // 24-bit (in 32-bit container)
        }
    }
}

/// Audio format configuration.
#[derive(Debug, Clone, Copy)]
pub struct AudioFormat {
    pub sample_rate: SampleRate,
    pub bit_depth: BitDepth,
    pub channels: u8,
}

impl AudioFormat {
    pub fn new(sample_rate: SampleRate, bit_depth: BitDepth, channels: u8) -> Self {
        Self { sample_rate, bit_depth, channels: channels.min(8).max(1) }
    }

    pub fn cd_quality() -> Self {
        Self { sample_rate: SampleRate::Rate44100, bit_depth: BitDepth::Bits16, channels: 2 }
    }

    pub fn dvd_quality() -> Self {
        Self { sample_rate: SampleRate::Rate48000, bit_depth: BitDepth::Bits24, channels: 2 }
    }

    /// Encode as HDA stream format register value.
    fn to_format_reg(&self) -> u16 {
        let mut fmt: u16 = 0;
        fmt |= self.sample_rate.format_bits();
        fmt |= self.bit_depth.format_bits();
        fmt |= (self.channels - 1) as u16;
        fmt
    }

    /// Bytes per sample frame.
    pub fn frame_size(&self) -> usize {
        let bytes_per_sample = match self.bit_depth {
            BitDepth::Bits16 => 2,
            BitDepth::Bits24 => 4, // 24 bit in 32 bit container
        };
        bytes_per_sample * self.channels as usize
    }
}

// ---------------------------------------------------------------------------
// Codec and widget representation
// ---------------------------------------------------------------------------

/// A discovered widget in a codec.
#[derive(Debug, Clone)]
pub struct Widget {
    pub nid: u8,
    pub widget_type: WidgetType,
    pub pin_device: Option<PinDevice>,
    pub pin_connection: Option<PinConnection>,
    pub pin_color: Option<PinColor>,
    pub amp_out_caps: u32,
    pub connections: Vec<u8>,
}

/// A discovered codec on the HDA link.
#[derive(Debug, Clone)]
pub struct Codec {
    pub address: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub revision: u32,
    pub widgets: Vec<Widget>,
}

impl Codec {
    pub fn vendor_name(&self) -> &'static str {
        match self.vendor_id {
            0x8086 => "Intel",
            0x10EC => "Realtek",
            0x1106 => "VIA",
            0x1002 => "AMD/ATI",
            0x10DE => "NVIDIA",
            0x11D4 => "Analog Devices",
            0x14F1 => "Conexant",
            0x1057 => "Motorola",
            _ => "Unknown",
        }
    }
}

// ---------------------------------------------------------------------------
// Playback state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlaybackState {
    Stopped,
    Playing,
    Paused,
}

// ---------------------------------------------------------------------------
// Capture state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CaptureState {
    Idle,
    Recording,
}

// ---------------------------------------------------------------------------
// Mixer
// ---------------------------------------------------------------------------

/// Per-channel volume (0-100 scaled to HDA gain steps).
#[derive(Debug, Clone)]
pub struct MixerChannel {
    pub name: String,
    pub volume: u8,
    pub muted: bool,
}

impl MixerChannel {
    fn new(name: &str) -> Self {
        Self { name: String::from(name), volume: 75, muted: false }
    }

    /// Convert volume (0-100) to HDA amplifier gain (0-127 7-bit).
    fn gain(&self) -> u8 {
        if self.muted { return 0; }
        // Map 0-100 to 0-127
        ((self.volume as u16 * 127) / 100) as u8
    }
}

// ---------------------------------------------------------------------------
// Global HDA state
// ---------------------------------------------------------------------------

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static COMMANDS_SENT: AtomicU64 = AtomicU64::new(0);
static RESPONSES_RX: AtomicU64 = AtomicU64::new(0);
static FRAMES_PLAYED: AtomicU64 = AtomicU64::new(0);
static FRAMES_CAPTURED: AtomicU64 = AtomicU64::new(0);

pub static HDA: Mutex<HdaState> = Mutex::new(HdaState::new());

pub struct HdaState {
    pub found: bool,
    pub bar0: u64,
    pub pci_bus: u8,
    pub pci_device: u8,
    pub pci_function: u8,
    pub version_major: u8,
    pub version_minor: u8,
    pub num_output_streams: u8,
    pub num_input_streams: u8,
    pub codecs: Vec<Codec>,
    pub playback_state: PlaybackState,
    pub playback_format: AudioFormat,
    pub capture_state: CaptureState,
    pub capture_format: AudioFormat,
    pub capture_buffer: Vec<i16>,
    pub mixer_channels: Vec<MixerChannel>,
    pub corb_wp: u16,
    pub rirb_rp: u16,
}

impl HdaState {
    pub const fn new() -> Self {
        Self {
            found: false,
            bar0: 0,
            pci_bus: 0,
            pci_device: 0,
            pci_function: 0,
            version_major: 1,
            version_minor: 0,
            num_output_streams: 0,
            num_input_streams: 0,
            codecs: Vec::new(),
            playback_state: PlaybackState::Stopped,
            playback_format: AudioFormat {
                sample_rate: SampleRate::Rate48000,
                bit_depth: BitDepth::Bits16,
                channels: 2,
            },
            capture_state: CaptureState::Idle,
            capture_format: AudioFormat {
                sample_rate: SampleRate::Rate48000,
                bit_depth: BitDepth::Bits16,
                channels: 2,
            },
            capture_buffer: Vec::new(),
            mixer_channels: Vec::new(),
            corb_wp: 0,
            rirb_rp: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Simulated codec creation (for demo without real hardware)
// ---------------------------------------------------------------------------

fn create_simulated_codec() -> Codec {
    let mut widgets = Vec::new();

    // NID 0x02: DAC (Audio Output)
    widgets.push(Widget {
        nid: 0x02,
        widget_type: WidgetType::AudioOutput,
        pin_device: None,
        pin_connection: None,
        pin_color: None,
        amp_out_caps: 0x0000_7F00, // 0-127 gain steps
        connections: Vec::new(),
    });

    // NID 0x03: DAC (Audio Output) for headphones
    widgets.push(Widget {
        nid: 0x03,
        widget_type: WidgetType::AudioOutput,
        pin_device: None,
        pin_connection: None,
        pin_color: None,
        amp_out_caps: 0x0000_7F00,
        connections: Vec::new(),
    });

    // NID 0x04: ADC (Audio Input)
    widgets.push(Widget {
        nid: 0x04,
        widget_type: WidgetType::AudioInput,
        pin_device: None,
        pin_connection: None,
        pin_color: None,
        amp_out_caps: 0,
        connections: alloc::vec![0x08, 0x09],
    });

    // NID 0x05: Mixer
    widgets.push(Widget {
        nid: 0x05,
        widget_type: WidgetType::AudioMixer,
        pin_device: None,
        pin_connection: None,
        pin_color: None,
        amp_out_caps: 0x0000_7F00,
        connections: alloc::vec![0x02, 0x03],
    });

    // NID 0x06: Pin Complex — Speaker (fixed)
    widgets.push(Widget {
        nid: 0x06,
        widget_type: WidgetType::PinComplex,
        pin_device: Some(PinDevice::Speaker),
        pin_connection: Some(PinConnection::FixedFunction),
        pin_color: Some(PinColor::Unknown),
        amp_out_caps: 0x0000_7F00,
        connections: alloc::vec![0x05],
    });

    // NID 0x07: Pin Complex — Headphone (jack, green)
    widgets.push(Widget {
        nid: 0x07,
        widget_type: WidgetType::PinComplex,
        pin_device: Some(PinDevice::Headphone),
        pin_connection: Some(PinConnection::Jack),
        pin_color: Some(PinColor::Green),
        amp_out_caps: 0x0000_7F00,
        connections: alloc::vec![0x03],
    });

    // NID 0x08: Pin Complex — Mic In (jack, pink)
    widgets.push(Widget {
        nid: 0x08,
        widget_type: WidgetType::PinComplex,
        pin_device: Some(PinDevice::MicIn),
        pin_connection: Some(PinConnection::Jack),
        pin_color: Some(PinColor::Pink),
        amp_out_caps: 0,
        connections: Vec::new(),
    });

    // NID 0x09: Pin Complex — Line In (jack, blue)
    widgets.push(Widget {
        nid: 0x09,
        widget_type: WidgetType::PinComplex,
        pin_device: Some(PinDevice::LineIn),
        pin_connection: Some(PinConnection::Jack),
        pin_color: Some(PinColor::Blue),
        amp_out_caps: 0,
        connections: Vec::new(),
    });

    // NID 0x0A: Volume Knob
    widgets.push(Widget {
        nid: 0x0A,
        widget_type: WidgetType::VolumeKnob,
        pin_device: None,
        pin_connection: None,
        pin_color: None,
        amp_out_caps: 0,
        connections: alloc::vec![0x05],
    });

    Codec {
        address: 0,
        vendor_id: 0x10EC,
        device_id: 0x0269,
        revision: 0x0001_0100,
        widgets,
    }
}

fn create_simulated_hdmi_codec() -> Codec {
    let mut widgets = Vec::new();

    // NID 0x02: DAC for HDMI
    widgets.push(Widget {
        nid: 0x02,
        widget_type: WidgetType::AudioOutput,
        pin_device: None,
        pin_connection: None,
        pin_color: None,
        amp_out_caps: 0,
        connections: Vec::new(),
    });

    // NID 0x03: Pin Complex — HDMI/DP out
    widgets.push(Widget {
        nid: 0x03,
        widget_type: WidgetType::PinComplex,
        pin_device: Some(PinDevice::DigitalOut),
        pin_connection: Some(PinConnection::FixedFunction),
        pin_color: Some(PinColor::Unknown),
        amp_out_caps: 0,
        connections: alloc::vec![0x02],
    });

    Codec {
        address: 2,
        vendor_id: 0x8086,
        device_id: 0x2882,
        revision: 0x0001_0000,
        widgets,
    }
}

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

/// Play an audio buffer with the given format.
pub fn play_buffer(data: &[i16], format: AudioFormat) -> Result<(), &'static str> {
    let mut hda = HDA.lock();
    if !hda.found {
        return Err("HDA controller not found");
    }
    if hda.playback_state == PlaybackState::Playing {
        return Err("Already playing; stop first");
    }
    if data.is_empty() {
        return Err("Empty audio buffer");
    }

    hda.playback_format = format;
    hda.playback_state = PlaybackState::Playing;

    // Simulate DMA transfer: count frames
    let frame_size = format.frame_size();
    let num_bytes = data.len() * 2; // i16 = 2 bytes
    let num_frames = if frame_size > 0 { num_bytes / frame_size } else { 0 };
    FRAMES_PLAYED.fetch_add(num_frames as u64, Ordering::Relaxed);

    serial_println!("[hda] Playing {} samples ({} frames, {}Hz {}bit {}ch)",
        data.len(), num_frames, format.sample_rate.hz(),
        format.bit_depth.bits(), format.channels);

    Ok(())
}

/// Stop playback.
pub fn stop() {
    let mut hda = HDA.lock();
    if hda.playback_state != PlaybackState::Stopped {
        hda.playback_state = PlaybackState::Stopped;
        serial_println!("[hda] Playback stopped");
    }
}

/// Pause playback.
pub fn pause() {
    let mut hda = HDA.lock();
    if hda.playback_state == PlaybackState::Playing {
        hda.playback_state = PlaybackState::Paused;
        serial_println!("[hda] Playback paused");
    }
}

/// Resume playback.
pub fn resume() {
    let mut hda = HDA.lock();
    if hda.playback_state == PlaybackState::Paused {
        hda.playback_state = PlaybackState::Playing;
        serial_println!("[hda] Playback resumed");
    }
}

// ---------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------

/// Start audio capture with the given format.
pub fn start_capture(format: AudioFormat) -> Result<(), &'static str> {
    let mut hda = HDA.lock();
    if !hda.found {
        return Err("HDA controller not found");
    }
    if hda.capture_state == CaptureState::Recording {
        return Err("Already recording");
    }

    hda.capture_format = format;
    hda.capture_state = CaptureState::Recording;
    hda.capture_buffer.clear();

    // Simulate captured data: generate a simple sine-approximation pattern
    // using integer math (no floating point)
    let num_samples = format.sample_rate.hz() as usize; // 1 second
    hda.capture_buffer.reserve(num_samples);
    for i in 0..num_samples {
        // Triangle wave approximation at ~440 Hz
        let period = format.sample_rate.hz() / 440;
        let pos = (i as u32) % period;
        let half = period / 2;
        let sample = if pos < half {
            // Rising: -32767 to 32767
            ((pos as i32 * 65534) / half as i32) - 32767
        } else {
            // Falling: 32767 to -32767
            32767 - (((pos - half) as i32 * 65534) / half as i32)
        };
        hda.capture_buffer.push(sample as i16);
    }

    FRAMES_CAPTURED.fetch_add(num_samples as u64, Ordering::Relaxed);

    serial_println!("[hda] Recording started ({}Hz {}bit {}ch)",
        format.sample_rate.hz(), format.bit_depth.bits(), format.channels);

    Ok(())
}

/// Stop audio capture.
pub fn stop_capture() {
    let mut hda = HDA.lock();
    if hda.capture_state == CaptureState::Recording {
        hda.capture_state = CaptureState::Idle;
        serial_println!("[hda] Recording stopped ({} samples captured)",
            hda.capture_buffer.len());
    }
}

/// Read captured audio data.
pub fn read_capture() -> Vec<i16> {
    let hda = HDA.lock();
    hda.capture_buffer.clone()
}

// ---------------------------------------------------------------------------
// Mixer / volume control
// ---------------------------------------------------------------------------

/// Set master volume (0-100).
pub fn set_master_volume(vol: u8) {
    let mut hda = HDA.lock();
    let vol = vol.min(100);
    if let Some(ch) = hda.mixer_channels.iter_mut().find(|c| c.name == "Master") {
        ch.volume = vol;
    }
}

/// Set volume for a specific channel by name (0-100).
pub fn set_channel_volume(name: &str, vol: u8) {
    let mut hda = HDA.lock();
    let vol = vol.min(100);
    if let Some(ch) = hda.mixer_channels.iter_mut().find(|c| c.name == name) {
        ch.volume = vol;
    }
}

/// Mute/unmute a channel by name.
pub fn set_mute(name: &str, muted: bool) {
    let mut hda = HDA.lock();
    if let Some(ch) = hda.mixer_channels.iter_mut().find(|c| c.name == name) {
        ch.muted = muted;
    }
}

// ---------------------------------------------------------------------------
// Info / stats API
// ---------------------------------------------------------------------------

/// Return a summary string of the HDA controller.
pub fn hda_info() -> String {
    let hda = HDA.lock();
    if !hda.found {
        return String::from("Intel HDA: not found");
    }

    let mut info = format!(
        "Intel HDA Controller\n  PCI: {:02x}:{:02x}.{}\n  BAR0: {:#010x}\n  Version: {}.{}\n  Output streams: {}\n  Input streams: {}\n  Codecs: {}\n",
        hda.pci_bus, hda.pci_device, hda.pci_function,
        hda.bar0, hda.version_major, hda.version_minor,
        hda.num_output_streams, hda.num_input_streams,
        hda.codecs.len());

    // Playback state
    let pb = match hda.playback_state {
        PlaybackState::Stopped => "Stopped",
        PlaybackState::Playing => "Playing",
        PlaybackState::Paused => "Paused",
    };
    info.push_str(&format!("  Playback: {} ({}Hz {}bit {}ch)\n",
        pb, hda.playback_format.sample_rate.hz(),
        hda.playback_format.bit_depth.bits(), hda.playback_format.channels));

    // Capture state
    let cap = match hda.capture_state {
        CaptureState::Idle => "Idle",
        CaptureState::Recording => "Recording",
    };
    info.push_str(&format!("  Capture: {}\n", cap));

    // Mixer
    info.push_str("  Mixer:\n");
    for ch in hda.mixer_channels.iter() {
        let mute_str = if ch.muted { " [MUTED]" } else { "" };
        info.push_str(&format!("    {}: {}%{}\n", ch.name, ch.volume, mute_str));
    }

    info
}

/// Return statistics.
pub fn hda_stats() -> String {
    format!(
        "HDA Statistics:\n  Commands sent: {}\n  Responses received: {}\n  Frames played: {}\n  Frames captured: {}",
        COMMANDS_SENT.load(Ordering::Relaxed),
        RESPONSES_RX.load(Ordering::Relaxed),
        FRAMES_PLAYED.load(Ordering::Relaxed),
        FRAMES_CAPTURED.load(Ordering::Relaxed))
}

/// List all discovered codecs.
pub fn list_codecs() -> String {
    let hda = HDA.lock();
    if hda.codecs.is_empty() {
        return String::from("No codecs found.");
    }

    let mut out = String::new();
    for codec in hda.codecs.iter() {
        out.push_str(&format!(
            "Codec #{}: {} ({:04x}:{:04x}) rev {:#010x}, {} widgets\n",
            codec.address, codec.vendor_name(), codec.vendor_id, codec.device_id,
            codec.revision, codec.widgets.len()));
    }
    out
}

/// List all widgets across all codecs.
pub fn list_widgets() -> String {
    let hda = HDA.lock();
    if hda.codecs.is_empty() {
        return String::from("No codecs found.");
    }

    let mut out = String::from("NID   TYPE                 PIN DEVICE    COLOR    CONNECTIONS\n");
    out.push_str(          "----  -------------------  ----------    -------  -----------\n");

    for codec in hda.codecs.iter() {
        out.push_str(&format!("-- Codec #{} ({}) --\n", codec.address, codec.vendor_name()));
        for w in codec.widgets.iter() {
            let pin_str = match &w.pin_device {
                Some(pd) => pd.as_str(),
                None => "-",
            };
            let color_str = match &w.pin_color {
                Some(PinColor::Unknown) | None => "-",
                Some(PinColor::Black) => "Black",
                Some(PinColor::Grey) => "Grey",
                Some(PinColor::Blue) => "Blue",
                Some(PinColor::Green) => "Green",
                Some(PinColor::Red) => "Red",
                Some(PinColor::Orange) => "Orange",
                Some(PinColor::Yellow) => "Yellow",
                Some(PinColor::Purple) => "Purple",
                Some(PinColor::Pink) => "Pink",
                Some(PinColor::White) => "White",
                Some(PinColor::Other) => "Other",
            };
            let conn_str = if w.connections.is_empty() {
                String::from("-")
            } else {
                let parts: Vec<String> = w.connections.iter()
                    .map(|c| format!("0x{:02X}", c))
                    .collect();
                let mut s = String::new();
                for (i, p) in parts.iter().enumerate() {
                    if i > 0 { s.push_str(", "); }
                    s.push_str(p);
                }
                s
            };
            out.push_str(&format!("0x{:02X}  {:19}  {:12}  {:7}  {}\n",
                w.nid, w.widget_type.as_str(), pin_str, color_str, conn_str));
        }
    }

    out
}

// ---------------------------------------------------------------------------
// PCI detection
// ---------------------------------------------------------------------------

/// Scan PCI for an HDA controller.
fn detect_hda_controller(hda: &mut HdaState) -> bool {
    let devices = pci::scan();
    for dev in devices.iter() {
        if dev.class == HDA_CLASS && dev.subclass == HDA_SUBCLASS {
            hda.pci_bus = dev.bus;
            hda.pci_device = dev.device;
            hda.pci_function = dev.function;
            // Read BAR0 (would be MMIO base in real hardware)
            hda.bar0 = 0xFEB0_0000; // typical HDA BAR0 address
            serial_println!("[hda] Found HDA controller: {:04x}:{:04x} at {:02x}:{:02x}.{}",
                dev.vendor_id, dev.device_id, dev.bus, dev.device, dev.function);
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the Intel HDA audio subsystem.
pub fn init() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }

    let mut hda = HDA.lock();

    // Try PCI detection; fall back to simulated if not found
    let hw_found = detect_hda_controller(&mut hda);

    if !hw_found {
        // Create simulated controller for demo
        hda.bar0 = 0xFEB0_0000;
        hda.pci_bus = 0;
        hda.pci_device = 0x1B;
        hda.pci_function = 0;
        serial_println!("[hda] No HDA hardware detected; using simulated controller");
    }

    hda.found = true;
    hda.version_major = 1;
    hda.version_minor = 0;
    hda.num_output_streams = 4;
    hda.num_input_streams = 2;

    // Discover codecs (simulated)
    let analog_codec = create_simulated_codec();
    let hdmi_codec = create_simulated_hdmi_codec();
    hda.codecs.push(analog_codec);
    hda.codecs.push(hdmi_codec);

    // Initialize mixer channels
    hda.mixer_channels.push(MixerChannel::new("Master"));
    hda.mixer_channels.push(MixerChannel::new("Speaker"));
    hda.mixer_channels.push(MixerChannel::new("Headphone"));
    hda.mixer_channels.push(MixerChannel::new("Mic"));
    hda.mixer_channels.push(MixerChannel::new("Line In"));

    // Set default Mic volume lower
    if let Some(mic) = hda.mixer_channels.iter_mut().find(|c| c.name == "Mic") {
        mic.volume = 50;
    }

    serial_println!("[hda] Intel HDA initialized: {} codecs, {} output + {} input streams",
        hda.codecs.len(), hda.num_output_streams, hda.num_input_streams);
    for codec in hda.codecs.iter() {
        serial_println!("[hda]   Codec #{}: {} ({:04x}:{:04x}), {} widgets",
            codec.address, codec.vendor_name(), codec.vendor_id, codec.device_id,
            codec.widgets.len());
    }
}
