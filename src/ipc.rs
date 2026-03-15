/// Inter-process communication via bounded channels.
/// A channel is a fixed-size ring buffer that tasks can send/receive bytes through.

use spin::Mutex;

const MAX_CHANNELS: usize = 4;
const CHANNEL_BUF_SIZE: usize = 64;

static CHANNELS: Mutex<[ChannelSlot; MAX_CHANNELS]> =
    Mutex::new([const { ChannelSlot::Free }; MAX_CHANNELS]);

enum ChannelSlot {
    Free,
    Active(Channel),
}

struct Channel {
    buf: [u8; CHANNEL_BUF_SIZE],
    read_pos: usize,
    write_pos: usize,
    count: usize,
}

impl Channel {
    const fn new() -> Self {
        Self {
            buf: [0; CHANNEL_BUF_SIZE],
            read_pos: 0,
            write_pos: 0,
            count: 0,
        }
    }

    fn send(&mut self, byte: u8) -> bool {
        if self.count >= CHANNEL_BUF_SIZE {
            return false; // full
        }
        self.buf[self.write_pos] = byte;
        self.write_pos = (self.write_pos + 1) % CHANNEL_BUF_SIZE;
        self.count += 1;
        true
    }

    fn recv(&mut self) -> Option<u8> {
        if self.count == 0 {
            return None;
        }
        let byte = self.buf[self.read_pos];
        self.read_pos = (self.read_pos + 1) % CHANNEL_BUF_SIZE;
        self.count -= 1;
        Some(byte)
    }

    fn len(&self) -> usize {
        self.count
    }
}

/// Create a new channel. Returns the channel ID or None if full.
pub fn create() -> Option<usize> {
    let mut channels = CHANNELS.lock();
    for (i, slot) in channels.iter_mut().enumerate() {
        if matches!(slot, ChannelSlot::Free) {
            *slot = ChannelSlot::Active(Channel::new());
            return Some(i);
        }
    }
    None
}

/// Destroy a channel by ID.
pub fn destroy(id: usize) {
    let mut channels = CHANNELS.lock();
    if id < MAX_CHANNELS {
        channels[id] = ChannelSlot::Free;
    }
}

/// Send a byte to a channel. Returns false if the channel is full or invalid.
pub fn send(id: usize, byte: u8) -> bool {
    let mut channels = CHANNELS.lock();
    if id >= MAX_CHANNELS {
        return false;
    }
    match &mut channels[id] {
        ChannelSlot::Active(ch) => ch.send(byte),
        ChannelSlot::Free => false,
    }
}

/// Receive a byte from a channel. Returns None if empty or invalid.
pub fn recv(id: usize) -> Option<u8> {
    let mut channels = CHANNELS.lock();
    if id >= MAX_CHANNELS {
        return None;
    }
    match &mut channels[id] {
        ChannelSlot::Active(ch) => ch.recv(),
        ChannelSlot::Free => None,
    }
}

/// Send a string to a channel byte-by-byte.
pub fn send_str(id: usize, s: &str) -> usize {
    let mut sent = 0;
    for byte in s.bytes() {
        if send(id, byte) {
            sent += 1;
        } else {
            break;
        }
    }
    sent
}

/// Read all available bytes from a channel into a String.
pub fn recv_all(id: usize) -> alloc::string::String {
    let mut result = alloc::string::String::new();
    while let Some(byte) = recv(id) {
        result.push(byte as char);
    }
    result
}

/// Channel info for display.
pub struct ChannelInfo {
    pub id: usize,
    pub pending: usize,
}

/// List all active channels.
pub fn list() -> alloc::vec::Vec<ChannelInfo> {
    let channels = CHANNELS.lock();
    let mut result = alloc::vec::Vec::new();
    for (i, slot) in channels.iter().enumerate() {
        if let ChannelSlot::Active(ch) = slot {
            result.push(ChannelInfo { id: i, pending: ch.len() });
        }
    }
    result
}
