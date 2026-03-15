/// Lightweight MQTT message broker for MerlionOS.
///
/// Implements core MQTT 3.1.1 packet types, topic matching with wildcard
/// support (`+` single-level, `#` multi-level), and a simple in-kernel
/// publish/subscribe broker backed by `spin::Mutex`.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use spin::Mutex;

// ---------------------------------------------------------------------------
// MQTT packet type constants (4-bit identifiers from the MQTT spec)
// ---------------------------------------------------------------------------

/// Client request to connect to the broker.
pub const CONNECT: u8 = 1;
/// Broker acknowledgement of a connection.
pub const CONNACK: u8 = 2;
/// Publish a message to a topic.
pub const PUBLISH: u8 = 3;
/// Client subscribe request.
pub const SUBSCRIBE: u8 = 8;
/// Broker subscribe acknowledgement.
pub const SUBACK: u8 = 9;
/// Ping request (keep-alive).
pub const PINGREQ: u8 = 12;
/// Ping response.
pub const PINGRESP: u8 = 13;
/// Client disconnect notification.
pub const DISCONNECT: u8 = 14;

// ---------------------------------------------------------------------------
// Core data types
// ---------------------------------------------------------------------------

/// Quality-of-service level for a message.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Qos {
    /// At most once (fire and forget).
    AtMostOnce = 0,
    /// At least once (acknowledged delivery).
    AtLeastOnce = 1,
    /// Exactly once (assured delivery).
    ExactlyOnce = 2,
}

/// An MQTT message with topic, payload, QoS, and retain flag.
#[derive(Clone, Debug)]
pub struct MqttMessage {
    /// Topic string, e.g. `"sensors/temperature"`.
    pub topic: String,
    /// Raw payload bytes.
    pub payload: Vec<u8>,
    /// Quality of service level.
    pub qos: Qos,
    /// Whether the broker should retain this message for new subscribers.
    pub retain: bool,
}

/// A topic the broker knows about, together with its subscriber list and
/// optional retained message.
#[derive(Clone, Debug)]
pub struct Topic {
    /// The canonical topic string.
    pub name: String,
    /// Client identifiers currently subscribed to this topic.
    pub subscribers: Vec<String>,
    /// The most recent retained message, if any.
    pub retained: Option<MqttMessage>,
}

/// Broker-wide statistics.
#[derive(Clone, Debug)]
pub struct BrokerStats {
    /// Total number of messages published since boot.
    pub message_count: u64,
    /// Number of currently registered topics.
    pub topic_count: usize,
}

// ---------------------------------------------------------------------------
// Broker state (global, behind a spin-lock)
// ---------------------------------------------------------------------------

/// Maximum number of distinct topics the broker tracks.
const MAX_TOPICS: usize = 64;

struct BrokerInner {
    topics: Vec<Topic>,
    message_count: u64,
}

static BROKER: Mutex<BrokerInner> = Mutex::new(BrokerInner {
    topics: Vec::new(),
    message_count: 0,
});

// ---------------------------------------------------------------------------
// Public broker API
// ---------------------------------------------------------------------------

/// Subscribe `client_id` to `topic_filter`.
///
/// If the topic does not yet exist it is created.  Duplicate subscriptions
/// for the same client are silently ignored.
pub fn subscribe(client_id: &str, topic_filter: &str) -> bool {
    let mut broker = BROKER.lock();
    // Look for an existing topic entry.
    for topic in broker.topics.iter_mut() {
        if topic.name == topic_filter {
            if !topic.subscribers.iter().any(|s| s == client_id) {
                topic.subscribers.push(String::from(client_id));
            }
            return true;
        }
    }
    // Create a new topic slot.
    if broker.topics.len() >= MAX_TOPICS {
        return false;
    }
    broker.topics.push(Topic {
        name: String::from(topic_filter),
        subscribers: vec![String::from(client_id)],
        retained: None,
    });
    true
}

/// Remove `client_id` from `topic_filter`.
///
/// Returns `true` if the client was previously subscribed.
pub fn unsubscribe(client_id: &str, topic_filter: &str) -> bool {
    let mut broker = BROKER.lock();
    for topic in broker.topics.iter_mut() {
        if topic.name == topic_filter {
            let before = topic.subscribers.len();
            topic.subscribers.retain(|s| s != client_id);
            return topic.subscribers.len() < before;
        }
    }
    false
}

/// Publish a message.  The message is delivered to every topic whose name
/// matches the publish topic (including wildcard topics).  Returns the
/// number of individual subscriber deliveries.
pub fn publish(message: &MqttMessage) -> usize {
    let mut broker = BROKER.lock();
    broker.message_count += 1;
    let mut deliveries: usize = 0;

    for topic in broker.topics.iter_mut() {
        if match_topic(&topic.name, &message.topic) {
            deliveries += topic.subscribers.len();
            if message.retain {
                topic.retained = Some(message.clone());
            }
        }
    }

    // If no topic slot exists yet for an exact match, create one to hold a
    // retained message (even with zero subscribers).
    if message.retain {
        let exact = broker.topics.iter().any(|t| t.name == message.topic);
        if !exact && broker.topics.len() < MAX_TOPICS {
            broker.topics.push(Topic {
                name: message.topic.clone(),
                subscribers: Vec::new(),
                retained: Some(message.clone()),
            });
        }
    }
    deliveries
}

/// Return a list of all currently known client identifiers (deduplicated).
pub fn client_list() -> Vec<String> {
    let broker = BROKER.lock();
    let mut clients: Vec<String> = Vec::new();
    for topic in broker.topics.iter() {
        for sub in topic.subscribers.iter() {
            if !clients.iter().any(|c| c == sub) {
                clients.push(sub.clone());
            }
        }
    }
    clients
}

/// Return aggregate broker statistics.
pub fn message_count() -> BrokerStats {
    let broker = BROKER.lock();
    BrokerStats {
        message_count: broker.message_count,
        topic_count: broker.topics.len(),
    }
}

// ---------------------------------------------------------------------------
// Topic matching with MQTT wildcards
// ---------------------------------------------------------------------------

/// Match an MQTT topic `filter` (may contain `+` / `#` wildcards) against
/// a concrete `topic` name.  `+` matches one level, `#` matches the rest.
pub fn match_topic(filter: &str, topic: &str) -> bool {
    let mut f_iter = filter.split('/');
    let mut t_iter = topic.split('/');

    loop {
        match (f_iter.next(), t_iter.next()) {
            (Some("#"), _) => return true,
            (Some("+"), Some(_)) => continue,
            (Some(f), Some(t)) if f == t => continue,
            (None, None) => return true,
            _ => return false,
        }
    }
}

// ---------------------------------------------------------------------------
// MQTT wire-format helpers
// ---------------------------------------------------------------------------

/// Encode the MQTT "remaining length" field into 1–4 bytes.
fn encode_remaining_length(mut len: usize, out: &mut Vec<u8>) {
    loop {
        let mut byte = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if len == 0 {
            break;
        }
    }
}

/// Decode an MQTT "remaining length" from `data` starting at `offset`.
/// Returns `(value, bytes_consumed)` or `None` on malformed input.
fn decode_remaining_length(data: &[u8], offset: usize) -> Option<(usize, usize)> {
    let mut multiplier: usize = 1;
    let mut value: usize = 0;
    let mut idx = offset;
    loop {
        if idx >= data.len() {
            return None;
        }
        let byte = data[idx];
        value += (byte & 0x7F) as usize * multiplier;
        multiplier *= 128;
        idx += 1;
        if byte & 0x80 == 0 {
            return Some((value, idx - offset));
        }
        if multiplier > 128 * 128 * 128 * 128 {
            return None; // malformed
        }
    }
}

/// Encode an MQTT fixed header + raw payload into a packet byte vector.
///
/// `packet_type` is one of the `CONNECT`..`DISCONNECT` constants.
/// `flags` occupies the lower 4 bits of the first header byte.
pub fn encode_packet(packet_type: u8, flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut pkt = Vec::new();
    pkt.push((packet_type << 4) | (flags & 0x0F));
    encode_remaining_length(payload.len(), &mut pkt);
    pkt.extend_from_slice(payload);
    pkt
}

/// Decoded representation of a raw MQTT packet.
#[derive(Debug)]
pub struct RawPacket {
    /// Packet type (upper 4 bits of byte 0).
    pub packet_type: u8,
    /// Flags (lower 4 bits of byte 0).
    pub flags: u8,
    /// Variable header + payload bytes.
    pub payload: Vec<u8>,
}

/// Decode a single MQTT packet from the start of `data`.
///
/// Returns the parsed [`RawPacket`] together with the total number of
/// bytes consumed, or `None` if the buffer is incomplete or malformed.
pub fn decode_packet(data: &[u8]) -> Option<(RawPacket, usize)> {
    if data.is_empty() {
        return None;
    }
    let header = data[0];
    let packet_type = header >> 4;
    let flags = header & 0x0F;

    let (remaining_len, len_bytes) = decode_remaining_length(data, 1)?;
    let total = 1 + len_bytes + remaining_len;
    if data.len() < total {
        return None; // not enough data yet
    }
    let start = 1 + len_bytes;
    let payload = data[start..start + remaining_len].to_vec();
    Some((
        RawPacket {
            packet_type,
            flags,
            payload,
        },
        total,
    ))
}
