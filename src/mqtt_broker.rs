/// Enhanced MQTT broker for MerlionOS.
/// Adds QoS levels, retained messages, last-will messages,
/// topic wildcards, persistent subscriptions, and broker statistics.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// QoS levels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QoS {
    AtMostOnce  = 0,  // Fire and forget
    AtLeastOnce = 1,  // Acknowledged delivery
    ExactlyOnce = 2,  // Assured delivery
}

impl QoS {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => QoS::AtMostOnce,
            1 => QoS::AtLeastOnce,
            _ => QoS::ExactlyOnce,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self { QoS::AtMostOnce => "QoS0", QoS::AtLeastOnce => "QoS1", QoS::ExactlyOnce => "QoS2" }
    }
}

/// A connected MQTT client.
#[derive(Debug, Clone)]
pub struct MqttClient {
    pub client_id: String,
    pub connected_tick: u64,
    pub last_activity: u64,
    pub subscriptions: Vec<String>,
    pub will_topic: Option<String>,
    pub will_message: Option<String>,
    pub will_qos: QoS,
    pub will_retain: bool,
    pub msgs_sent: u64,
    pub msgs_received: u64,
}

/// A retained message on a topic.
#[derive(Debug, Clone)]
pub struct RetainedMessage {
    pub topic: String,
    pub payload: String,
    pub qos: QoS,
    pub timestamp: u64,
}

/// A message in the broker queue.
#[derive(Debug, Clone)]
pub struct BrokerMessage {
    pub id: u32,
    pub topic: String,
    pub payload: String,
    pub qos: QoS,
    pub retained: bool,
    pub timestamp: u64,
    pub publisher: String,
}

const MAX_CLIENTS: usize = 16;
const MAX_RETAINED: usize = 64;
const MAX_QUEUE: usize = 256;

static CLIENTS: Mutex<Vec<MqttClient>> = Mutex::new(Vec::new());
static RETAINED: Mutex<Vec<RetainedMessage>> = Mutex::new(Vec::new());
static MSG_QUEUE: Mutex<Vec<BrokerMessage>> = Mutex::new(Vec::new());
static NEXT_MSG_ID: AtomicU32 = AtomicU32::new(1);
static TOTAL_PUBLISHED: AtomicU64 = AtomicU64::new(0);
static TOTAL_DELIVERED: AtomicU64 = AtomicU64::new(0);

/// Initialize the MQTT broker.
pub fn init() {
    crate::serial_println!("[mqtt_broker] enhanced broker initialized");
    crate::klog_println!("[mqtt_broker] initialized");
}

/// Connect a client.
pub fn connect(client_id: &str, will_topic: Option<&str>, will_msg: Option<&str>) -> Result<(), &'static str> {
    let mut clients = CLIENTS.lock();
    if clients.len() >= MAX_CLIENTS { return Err("mqtt: max clients"); }
    if clients.iter().any(|c| c.client_id == client_id) { return Err("mqtt: client already connected"); }

    clients.push(MqttClient {
        client_id: client_id.to_owned(),
        connected_tick: crate::timer::ticks(),
        last_activity: crate::timer::ticks(),
        subscriptions: Vec::new(),
        will_topic: will_topic.map(|s| s.to_owned()),
        will_message: will_msg.map(|s| s.to_owned()),
        will_qos: QoS::AtMostOnce,
        will_retain: false,
        msgs_sent: 0,
        msgs_received: 0,
    });
    crate::serial_println!("[mqtt_broker] client '{}' connected", client_id);
    Ok(())
}

/// Disconnect a client, publishing will message if set.
pub fn disconnect(client_id: &str) {
    let will = {
        let mut clients = CLIENTS.lock();
        let will = clients.iter()
            .find(|c| c.client_id == client_id)
            .and_then(|c| {
                c.will_topic.as_ref().map(|t| (t.clone(), c.will_message.clone().unwrap_or_default()))
            });
        clients.retain(|c| c.client_id != client_id);
        will
    };

    // Publish will message
    if let Some((topic, msg)) = will {
        let _ = publish(client_id, &topic, &msg, QoS::AtMostOnce, false);
    }
    crate::serial_println!("[mqtt_broker] client '{}' disconnected", client_id);
}

/// Subscribe a client to a topic pattern.
pub fn subscribe(client_id: &str, topic: &str) -> Result<(), &'static str> {
    let mut clients = CLIENTS.lock();
    let client = clients.iter_mut().find(|c| c.client_id == client_id)
        .ok_or("mqtt: client not found")?;
    if !client.subscriptions.iter().any(|s| s == topic) {
        client.subscriptions.push(topic.to_owned());
    }
    Ok(())
}

/// Unsubscribe a client from a topic.
pub fn unsubscribe(client_id: &str, topic: &str) -> Result<(), &'static str> {
    let mut clients = CLIENTS.lock();
    let client = clients.iter_mut().find(|c| c.client_id == client_id)
        .ok_or("mqtt: client not found")?;
    client.subscriptions.retain(|s| s != topic);
    Ok(())
}

/// Check if a topic matches a pattern (supports + and # wildcards).
pub fn topic_matches(pattern: &str, topic: &str) -> bool {
    let pat_parts: Vec<&str> = pattern.split('/').collect();
    let top_parts: Vec<&str> = topic.split('/').collect();

    for (i, pat) in pat_parts.iter().enumerate() {
        if *pat == "#" { return true; }
        if i >= top_parts.len() { return false; }
        if *pat != "+" && *pat != top_parts[i] { return false; }
    }

    pat_parts.len() == top_parts.len()
}

/// Publish a message to a topic.
pub fn publish(publisher: &str, topic: &str, payload: &str, qos: QoS, retain: bool) -> u32 {
    let id = NEXT_MSG_ID.fetch_add(1, Ordering::Relaxed);
    let now = crate::timer::ticks();

    // Store retained message
    if retain {
        let mut retained = RETAINED.lock();
        retained.retain(|r| r.topic != topic);
        if retained.len() < MAX_RETAINED {
            retained.push(RetainedMessage {
                topic: topic.to_owned(),
                payload: payload.to_owned(),
                qos,
                timestamp: now,
            });
        }
    }

    // Queue message
    {
        let mut queue = MSG_QUEUE.lock();
        if queue.len() >= MAX_QUEUE { queue.remove(0); }
        queue.push(BrokerMessage {
            id, topic: topic.to_owned(), payload: payload.to_owned(),
            qos, retained: retain, timestamp: now, publisher: publisher.to_owned(),
        });
    }

    // Count matching subscribers
    let clients = CLIENTS.lock();
    let mut delivered = 0u64;
    for client in clients.iter() {
        for sub in &client.subscriptions {
            if topic_matches(sub, topic) {
                delivered += 1;
                break;
            }
        }
    }

    TOTAL_PUBLISHED.fetch_add(1, Ordering::Relaxed);
    TOTAL_DELIVERED.fetch_add(delivered, Ordering::Relaxed);

    id
}

/// Get messages for a client (all queued messages matching their subscriptions).
pub fn poll(client_id: &str) -> Vec<BrokerMessage> {
    let clients = CLIENTS.lock();
    let subs: Vec<String> = clients.iter()
        .find(|c| c.client_id == client_id)
        .map(|c| c.subscriptions.clone())
        .unwrap_or_default();
    drop(clients);

    let queue = MSG_QUEUE.lock();
    queue.iter()
        .filter(|m| subs.iter().any(|s| topic_matches(s, &m.topic)))
        .cloned()
        .collect()
}

/// Broker statistics.
pub fn broker_stats() -> String {
    let clients = CLIENTS.lock().len();
    let retained = RETAINED.lock().len();
    let queued = MSG_QUEUE.lock().len();
    format!(
        "MQTT Broker: {} clients, {} retained msgs, {} queued\nPublished: {} | Delivered: {}",
        clients, retained, queued,
        TOTAL_PUBLISHED.load(Ordering::Relaxed),
        TOTAL_DELIVERED.load(Ordering::Relaxed),
    )
}

/// List connected clients.
pub fn list_clients() -> String {
    let clients = CLIENTS.lock();
    if clients.is_empty() { return String::from("No MQTT clients connected.\n"); }
    let mut out = format!("MQTT clients ({}):\n", clients.len());
    for c in clients.iter() {
        out.push_str(&format!("  {} — {} subs, sent:{} recv:{}\n",
            c.client_id, c.subscriptions.len(), c.msgs_sent, c.msgs_received));
    }
    out
}

/// List retained messages.
pub fn list_retained() -> String {
    let retained = RETAINED.lock();
    if retained.is_empty() { return String::from("No retained messages.\n"); }
    let mut out = format!("Retained messages ({}):\n", retained.len());
    for r in retained.iter() {
        let preview = if r.payload.len() > 40 { format!("{}...", &r.payload[..37]) } else { r.payload.clone() };
        out.push_str(&format!("  {} [{}]: {}\n", r.topic, r.qos.as_str(), preview));
    }
    out
}
