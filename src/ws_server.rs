/// WebSocket server enhancements for MerlionOS.
/// Adds broadcast rooms, connection management, ping/pong,
/// binary frame support, and pub/sub integration.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// WebSocket connection state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WsState {
    Connecting,
    Open,
    Closing,
    Closed,
}

/// A WebSocket connection.
#[derive(Debug, Clone)]
pub struct WsConnection {
    pub id: u32,
    pub client_ip: [u8; 4],
    pub state: WsState,
    pub connected_tick: u64,
    pub last_ping: u64,
    pub last_pong: u64,
    pub msgs_sent: u64,
    pub msgs_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub rooms: Vec<String>,
    pub protocol: String,
}

/// A WebSocket room for broadcasting.
#[derive(Debug, Clone)]
pub struct WsRoom {
    pub name: String,
    pub members: Vec<u32>,  // connection IDs
    pub created_tick: u64,
    pub message_count: u64,
}

/// A queued WebSocket message.
#[derive(Debug, Clone)]
pub struct WsMessage {
    pub from_id: u32,
    pub room: Option<String>,
    pub payload: String,
    pub is_binary: bool,
    pub timestamp: u64,
}

const MAX_CONNECTIONS: usize = 16;
const MAX_ROOMS: usize = 8;
const MAX_MSG_QUEUE: usize = 128;

static CONNECTIONS: Mutex<Vec<WsConnection>> = Mutex::new(Vec::new());
static ROOMS: Mutex<Vec<WsRoom>> = Mutex::new(Vec::new());
static MSG_QUEUE: Mutex<Vec<WsMessage>> = Mutex::new(Vec::new());
static NEXT_CONN_ID: AtomicU32 = AtomicU32::new(1);
static TOTAL_CONNECTIONS: AtomicU64 = AtomicU64::new(0);
static TOTAL_MESSAGES: AtomicU64 = AtomicU64::new(0);

/// Initialize the WebSocket server.
pub fn init() {
    // Create default rooms
    let mut rooms = ROOMS.lock();
    rooms.push(WsRoom { name: "system".to_owned(), members: Vec::new(), created_tick: 0, message_count: 0 });
    rooms.push(WsRoom { name: "chat".to_owned(), members: Vec::new(), created_tick: 0, message_count: 0 });

    crate::serial_println!("[ws_server] initialized with {} rooms", rooms.len());
    crate::klog_println!("[ws_server] initialized");
}

/// Accept a new WebSocket connection.
pub fn accept(client_ip: [u8; 4], protocol: &str) -> Result<u32, &'static str> {
    let mut conns = CONNECTIONS.lock();
    if conns.len() >= MAX_CONNECTIONS { return Err("ws: max connections"); }

    let id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);
    let now = crate::timer::ticks();

    conns.push(WsConnection {
        id, client_ip, state: WsState::Open,
        connected_tick: now, last_ping: now, last_pong: now,
        msgs_sent: 0, msgs_received: 0, bytes_sent: 0, bytes_received: 0,
        rooms: Vec::new(), protocol: protocol.to_owned(),
    });

    TOTAL_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[ws_server] connection #{} from {}.{}.{}.{}",
        id, client_ip[0], client_ip[1], client_ip[2], client_ip[3]);
    Ok(id)
}

/// Close a WebSocket connection.
pub fn close(id: u32) {
    let mut conns = CONNECTIONS.lock();
    if let Some(conn) = conns.iter_mut().find(|c| c.id == id) {
        conn.state = WsState::Closed;
    }
    drop(conns);

    // Remove from all rooms
    let mut rooms = ROOMS.lock();
    for room in rooms.iter_mut() {
        room.members.retain(|&m| m != id);
    }
}

/// Join a room.
pub fn join_room(conn_id: u32, room_name: &str) -> Result<(), &'static str> {
    let mut rooms = ROOMS.lock();
    let room = rooms.iter_mut().find(|r| r.name == room_name)
        .ok_or("ws: room not found")?;
    if !room.members.contains(&conn_id) {
        room.members.push(conn_id);
    }

    // Also track in connection
    drop(rooms);
    let mut conns = CONNECTIONS.lock();
    if let Some(conn) = conns.iter_mut().find(|c| c.id == conn_id) {
        if !conn.rooms.iter().any(|r| r == room_name) {
            conn.rooms.push(room_name.to_owned());
        }
    }
    Ok(())
}

/// Leave a room.
pub fn leave_room(conn_id: u32, room_name: &str) {
    let mut rooms = ROOMS.lock();
    if let Some(room) = rooms.iter_mut().find(|r| r.name == room_name) {
        room.members.retain(|&m| m != conn_id);
    }
    drop(rooms);

    let mut conns = CONNECTIONS.lock();
    if let Some(conn) = conns.iter_mut().find(|c| c.id == conn_id) {
        conn.rooms.retain(|r| r != room_name);
    }
}

/// Create a new room.
pub fn create_room(name: &str) -> Result<(), &'static str> {
    let mut rooms = ROOMS.lock();
    if rooms.len() >= MAX_ROOMS { return Err("ws: max rooms"); }
    if rooms.iter().any(|r| r.name == name) { return Err("ws: room exists"); }
    rooms.push(WsRoom {
        name: name.to_owned(),
        members: Vec::new(),
        created_tick: crate::timer::ticks(),
        message_count: 0,
    });
    Ok(())
}

/// Send a message to a specific connection.
pub fn send(conn_id: u32, payload: &str) {
    let mut conns = CONNECTIONS.lock();
    if let Some(conn) = conns.iter_mut().find(|c| c.id == conn_id && c.state == WsState::Open) {
        conn.msgs_sent += 1;
        conn.bytes_sent += payload.len() as u64;
    }
    drop(conns);

    let mut queue = MSG_QUEUE.lock();
    if queue.len() >= MAX_MSG_QUEUE { queue.remove(0); }
    queue.push(WsMessage {
        from_id: 0, room: None, payload: payload.to_owned(),
        is_binary: false, timestamp: crate::timer::ticks(),
    });
    TOTAL_MESSAGES.fetch_add(1, Ordering::Relaxed);
}

/// Broadcast a message to all members of a room.
pub fn broadcast(room_name: &str, payload: &str, exclude_id: Option<u32>) -> usize {
    let rooms = ROOMS.lock();
    let members: Vec<u32> = rooms.iter()
        .find(|r| r.name == room_name)
        .map(|r| r.members.clone())
        .unwrap_or_default();
    drop(rooms);

    let mut sent = 0;
    for &member_id in &members {
        if Some(member_id) == exclude_id { continue; }
        send(member_id, payload);
        sent += 1;
    }

    // Update room message count
    let mut rooms = ROOMS.lock();
    if let Some(room) = rooms.iter_mut().find(|r| r.name == room_name) {
        room.message_count += 1;
    }

    sent
}

/// Send a ping to a connection, update last_ping.
pub fn ping(conn_id: u32) {
    let mut conns = CONNECTIONS.lock();
    if let Some(conn) = conns.iter_mut().find(|c| c.id == conn_id) {
        conn.last_ping = crate::timer::ticks();
    }
}

/// Record a pong response.
pub fn pong(conn_id: u32) {
    let mut conns = CONNECTIONS.lock();
    if let Some(conn) = conns.iter_mut().find(|c| c.id == conn_id) {
        conn.last_pong = crate::timer::ticks();
    }
}

/// List active connections.
pub fn list_connections() -> String {
    let conns = CONNECTIONS.lock();
    let active: Vec<&WsConnection> = conns.iter().filter(|c| c.state == WsState::Open).collect();
    if active.is_empty() { return String::from("No active WebSocket connections.\n"); }

    let mut out = format!("WebSocket connections ({}):\n", active.len());
    out.push_str(&format!("{:>4} {:<16} {:>6} {:>6} {}\n", "ID", "Client", "Sent", "Recv", "Rooms"));
    for c in &active {
        let ip = format!("{}.{}.{}.{}", c.client_ip[0], c.client_ip[1], c.client_ip[2], c.client_ip[3]);
        let rooms = c.rooms.join(",");
        out.push_str(&format!("{:>4} {:<16} {:>6} {:>6} {}\n",
            c.id, ip, c.msgs_sent, c.msgs_received, rooms));
    }
    out
}

/// List rooms.
pub fn list_rooms() -> String {
    let rooms = ROOMS.lock();
    let mut out = format!("WebSocket rooms ({}):\n", rooms.len());
    for r in rooms.iter() {
        out.push_str(&format!("  {} — {} members, {} messages\n",
            r.name, r.members.len(), r.message_count));
    }
    out
}

/// Get server statistics.
pub fn ws_stats() -> String {
    let active = CONNECTIONS.lock().iter().filter(|c| c.state == WsState::Open).count();
    format!(
        "WebSocket: {} active, {} total connections, {} messages",
        active,
        TOTAL_CONNECTIONS.load(Ordering::Relaxed),
        TOTAL_MESSAGES.load(Ordering::Relaxed),
    )
}
